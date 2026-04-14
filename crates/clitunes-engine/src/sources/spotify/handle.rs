//! Shared Spotify auth-state handle.
//!
//! The daemon creates one [`SpotifyHandle`] at startup and shares it between
//! the source pipeline (for playback session construction) and the Web API
//! cache (for token-provider construction). Before this existed both paths
//! independently called [`crate::sources::spotify::auth::load_credentials`],
//! which writes a rotated `refresh_token` back to disk — two concurrent
//! callers could race on that write and leave the cache with an invalidated
//! token. The handle funnels both paths through a single `load_credentials`
//! call guarded by a `tokio::sync::Mutex`.
//!
//! ## Scope
//!
//! - Caches the last [`AuthResult`] so both `connect()` and
//!   `token_provider()` reuse it rather than re-reading `credentials.json`
//!   and rotating the on-disk `refresh_token` under concurrent callers.
//! - Builds a fresh [`Session`] on each `connect()` call. **Session is
//!   never cached** — `Session::new` captures the current `tokio::runtime::Handle`,
//!   and the source pipeline runs each track on its own per-track
//!   `current_thread` runtime that dies when playback ends. A cached
//!   Session from track N would reference a dead runtime on track N+1.
//! - Exposes `reconnect()` for the source pipeline's `SessionDisconnected`
//!   path — re-loads credentials (forcing a fresh token refresh) and
//!   re-calls `session.connect` on the caller's session, preserving v1.1
//!   retry/backoff semantics.
//!
//! ## Out of scope (deferred)
//!
//! - Sharing the [`Player`] across playbacks. `Player` is tightly coupled
//!   to [`SpotifySink`](super::sink::SpotifySink), which is bound to the
//!   per-source `PcmWriter`; it must be rebuilt for each track.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use tracing::{info, warn};

use super::auth::{self, AuthResult};
#[cfg(feature = "webapi")]
use super::token::SharedTokenProvider;

/// Handle to the daemon's shared Spotify auth state.
///
/// Cheap to clone via `Arc<SpotifyHandle>`. Thread-safe.
pub struct SpotifyHandle {
    cred_path: PathBuf,
    inner: tokio::sync::Mutex<Inner>,
}

/// Mutable state guarded by the handle's tokio mutex. Parallel callers on
/// an empty cache coalesce into a single `load_credentials` call instead
/// of racing on the on-disk `refresh_token` rotation.
struct Inner {
    last_auth: Option<AuthResult>,
}

impl SpotifyHandle {
    /// Construct a handle. No disk I/O, no network — the first call to
    /// [`connect`](Self::connect) or [`token_provider`](Self::token_provider)
    /// triggers credential loading.
    pub fn new(cred_path: PathBuf) -> Self {
        Self {
            cred_path,
            inner: tokio::sync::Mutex::new(Inner { last_auth: None }),
        }
    }

    /// Path to the on-disk credential cache. Exposed for logging /
    /// diagnostics; callers should prefer [`connect`](Self::connect)
    /// and [`token_provider`](Self::token_provider) over reloading
    /// credentials themselves.
    pub fn cred_path(&self) -> &Path {
        &self.cred_path
    }

    /// Build and connect a fresh [`Session`], checking the Premium gate
    /// before returning. The Session is owned by the caller — this handle
    /// intentionally does **not** cache it (see module docs).
    ///
    /// Must be called from inside a tokio runtime; `Session::new` captures
    /// `Handle::current()` and spawns background work on it.
    pub async fn connect(&self) -> Result<Session> {
        let mut inner = self.inner.lock().await;
        ensure_auth(&mut inner, &self.cred_path).await?;
        let credentials = inner
            .last_auth
            .as_ref()
            .expect("ensure_auth populated last_auth")
            .credentials
            .clone();
        drop(inner);

        let session = Session::new(SessionConfig::default(), None);
        session
            .connect(credentials, false)
            .await
            .map_err(|e| anyhow::anyhow!("Spotify session connect failed: {e}"))?;
        info!("spotify: session connected");

        ensure_premium(&session).await?;
        Ok(session)
    }

    /// Build a [`SharedTokenProvider`] snapshot from the cached auth state,
    /// loading credentials from disk if this is the first call. The returned
    /// provider owns a clone of the current OAuth token; subsequent refreshes
    /// via [`SharedTokenProvider::refresh`] operate on the provider's copy
    /// and don't invalidate the handle's cache.
    #[cfg(feature = "webapi")]
    pub async fn token_provider(&self) -> Result<SharedTokenProvider> {
        let mut inner = self.inner.lock().await;
        ensure_auth(&mut inner, &self.cred_path).await?;
        let token = inner
            .last_auth
            .as_ref()
            .expect("ensure_auth populated last_auth")
            .token
            .clone();
        Ok(SharedTokenProvider::new(token, self.cred_path.clone()))
    }

    /// Reload credentials from disk and re-call `connect` on the caller's
    /// session. 3 attempts with 1s/2s/4s backoff, force-refreshing auth
    /// each attempt — the `refresh_token` on disk may have rotated since
    /// the last call. Preserves the v1.1 `attempt_reconnect` behavior.
    pub async fn reconnect(&self, session: &Session) -> Result<()> {
        const DELAYS: [Duration; 3] = [
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4),
        ];

        let mut last_err: Option<anyhow::Error> = None;
        for (i, delay) in DELAYS.iter().enumerate() {
            info!(attempt = i + 1, "spotify: reconnect attempt");
            tokio::time::sleep(*delay).await;

            let credentials = {
                let mut inner = self.inner.lock().await;
                inner.last_auth = None;
                if let Err(e) = ensure_auth(&mut inner, &self.cred_path).await {
                    warn!(
                        attempt = i + 1,
                        error = %e,
                        "spotify: credential reload failed during reconnect"
                    );
                    last_err = Some(e);
                    continue;
                }
                inner
                    .last_auth
                    .as_ref()
                    .expect("ensure_auth populated last_auth")
                    .credentials
                    .clone()
            };

            match session.connect(credentials, false).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    warn!(
                        attempt = i + 1,
                        error = %e,
                        "spotify: reconnect attempt failed"
                    );
                    last_err = Some(anyhow::anyhow!("reconnect attempt {}: {e}", i + 1));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("reconnect failed after 3 attempts")))
    }
}

/// Populate `inner.last_auth` if empty. Runs `load_credentials` on the
/// blocking pool so the token-refresh HTTP round-trip doesn't stall the
/// current-thread runtime the source pipeline runs on.
async fn ensure_auth(inner: &mut Inner, cred_path: &Path) -> Result<()> {
    if inner.last_auth.is_some() {
        return Ok(());
    }
    let cred_path_owned = cred_path.to_path_buf();
    let auth_result = tokio::task::spawn_blocking(move || auth::load_credentials(&cred_path_owned))
        .await
        .context("credential task panicked")?
        .context("Spotify authentication failed")?;
    inner.last_auth = Some(auth_result);
    Ok(())
}

/// Wait up to ~1s for librespot to receive the user-data `type` attribute
/// and bail with `premium_required` if the account isn't Premium. Identical
/// to the existing check in `run_spotify_playback`, moved here so the gate
/// runs once per handle lifetime instead of once per playback.
async fn ensure_premium(session: &Session) -> Result<()> {
    for _ in 0..10 {
        let catalogue = session
            .user_data()
            .attributes
            .get("type")
            .cloned()
            .unwrap_or_default();
        if !catalogue.is_empty() {
            if catalogue != "premium" {
                anyhow::bail!(
                    "premium_required: Spotify Premium is required for playback. \
                     Visit spotify.com/premium to upgrade."
                );
            }
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // User-data never arrived within 1s. Let playback proceed; a
    // `PlayerEvent::Unavailable` will surface the issue if it is one.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_handle_does_no_io() {
        // Construction must not read from disk or hit the network; the
        // daemon builds a handle unconditionally at startup, including
        // on runs that never touch Spotify.
        let handle = SpotifyHandle::new(PathBuf::from("/tmp/clitunes-test-nonexistent.json"));
        assert_eq!(
            handle.cred_path(),
            Path::new("/tmp/clitunes-test-nonexistent.json")
        );
    }

    #[tokio::test]
    async fn connect_fails_fast_without_credentials() {
        // Missing credential file → auth::load_credentials returns an
        // anyhow error containing "no cached Spotify credentials".
        let handle = SpotifyHandle::new(PathBuf::from(
            "/tmp/clitunes-test-handle-missing-creds.json",
        ));
        // `Session` isn't Debug, so unwrap via match rather than expect_err.
        let err = match handle.connect().await {
            Ok(_) => panic!("connect() should fail with no cached credentials"),
            Err(e) => e,
        };
        assert!(
            format!("{err:#}").contains("no cached Spotify credentials"),
            "unexpected error: {err:#}"
        );
    }

    #[cfg(feature = "webapi")]
    #[tokio::test]
    async fn token_provider_fails_fast_without_credentials() {
        let handle = SpotifyHandle::new(PathBuf::from("/tmp/clitunes-test-handle-missing-tp.json"));
        let err = match handle.token_provider().await {
            Ok(_) => panic!("token_provider() should fail with no cached credentials"),
            Err(e) => e,
        };
        assert!(
            format!("{err:#}").contains("no cached Spotify credentials"),
            "unexpected error: {err:#}"
        );
    }

    /// Reconnect's retry loop sleeps 1s/2s/4s between attempts. The v1.1
    /// behavior held the handle mutex across that entire window; this
    /// test pins that the refactor releases the mutex around each sleep
    /// so concurrent Web API callers aren't blocked for seconds on a
    /// lost-session retry.
    #[cfg(feature = "webapi")]
    #[tokio::test]
    async fn reconnect_sleep_does_not_block_token_provider() {
        use librespot_core::authentication::Credentials;
        use librespot_oauth::OAuthToken;
        use std::time::Instant;

        let cred_path = PathBuf::from("/tmp/clitunes-reconnect-scope-test.json");
        let handle = std::sync::Arc::new(SpotifyHandle::new(cred_path));

        // Seed the auth cache so `token_provider()` short-circuits instead
        // of hitting the (missing) credential file.
        {
            let mut inner = handle.inner.lock().await;
            inner.last_auth = Some(AuthResult {
                token: OAuthToken {
                    access_token: "test-access".into(),
                    refresh_token: "test-refresh".into(),
                    expires_at: Instant::now() + Duration::from_secs(3600),
                    token_type: "Bearer".into(),
                    scopes: vec![],
                },
                credentials: Credentials::with_access_token("test-access"),
            });
        }

        // Disconnected session — `session.connect` will error against bogus
        // credentials, driving reconnect through its retry loop.
        let session = Session::new(SessionConfig::default(), None);

        let reconnect_handle = std::sync::Arc::clone(&handle);
        let reconnect_task = tokio::spawn(async move {
            let _ = reconnect_handle.reconnect(&session).await;
        });

        // Let reconnect enter its first `tokio::time::sleep(1s)`.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let start = Instant::now();
        let _ = handle.token_provider().await;
        let elapsed = start.elapsed();

        reconnect_task.abort();

        assert!(
            elapsed < Duration::from_millis(500),
            "token_provider blocked {}ms during reconnect sleep — the \
             handle mutex must not span `tokio::time::sleep`",
            elapsed.as_millis()
        );
    }
}
