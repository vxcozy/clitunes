//! Shared Spotify playback + auth state for the daemon.
//!
//! The daemon creates one [`SpotifyHandle`] at startup and shares it
//! between the source pipeline and the Web API cache. It owns four
//! conceptually distinct things, each with a different lifetime and
//! reason for being singletonised:
//!
//! 1. **Auth cache** — [`AuthResult`] from `load_credentials`. Shared
//!    so both playback and Web API go through a single read of
//!    `credentials.json` and can't race on the rotated `refresh_token`
//!    write. Populated lazily on first playback or token-provider call.
//!
//! 2. **Session** — `librespot_core::Session`. Shared and **pinned to
//!    the daemon tokio runtime**. `Session::new` captures
//!    `Handle::current()` at construction time, so the first caller
//!    decides the session's runtime forever. Keeping the session on
//!    the daemon runtime (which outlives any per-track runtime) means
//!    it keeps working across tracks; the v1.1 behaviour of building a
//!    fresh session inside each per-track `current_thread` runtime
//!    left the session pointing at a runtime that had already died.
//!
//! 3. **Player** — `Arc<librespot_playback::player::Player>`. Shared
//!    across tracks and v1.2 OAuth-URI playback. Player spawns its own
//!    decoder thread and owns its own private tokio runtime, so it
//!    doesn't care about the caller's runtime — but the sink it wraps
//!    *does* care who receives PCM. See the sink handle.
//!
//!    Spotify Connect deliberately does *not* share this singleton.
//!    `Spirc::new` itself calls `session.connect(credentials, true)`
//!    after wiring its dealer listeners, and librespot-core's
//!    `tx_connection` `OnceLock` means a Session can only be connected
//!    once — so Connect needs a fresh, *unconnected* Session per
//!    Discovery credential arrival, plus a matching Player. See
//!    `connect::ConnectRuntime`.
//!
//! 4. **Sink handle** — [`SpotifySinkHandle`]. Because `Player::new`
//!    consumes its sink via `FnOnce`, the sink is singletonised
//!    alongside the Player. Each playback *binds* a fresh PCM channel
//!    onto the sink for its duration and *unbinds* when it ends, so
//!    consumers can rotate freely without rebuilding the Player.
//!
//! ## Initialisation
//!
//! The session + player + sink triple is initialised lazily on the
//! first [`start_playback`](SpotifyHandle::start_playback) call and
//! cached in a `OnceCell`. We dispatch the init onto the daemon
//! runtime via a stored `tokio::runtime::Handle` so `Session::new`'s
//! `Handle::current()` capture lands on the long-lived daemon runtime
//! regardless of where `start_playback` is called from.
//!
//! ## Shutdown
//!
//! [`PlaybackGuard`] is RAII: on drop it calls `player.stop()`, unbinds
//! the sink, and drops the PCM receiver. Dropping the receiver wakes
//! any blocked `SyncSender::send` inside the sink with `Disconnected`
//! — the mirror of the PR #29 drop-order fix, now lifted into the
//! guard itself so every playback gets it for free.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_playback::config::PlayerConfig;
use librespot_playback::mixer::NoOpVolume;
use librespot_playback::player::{Player, PlayerEventChannel};
use tokio::runtime::Handle as RuntimeHandle;
use tokio::sync::OnceCell;
use tracing::{info, warn};

use clitunes_core::StereoFrame;

use super::auth::{self, AuthResult};
use super::sink::{new_sink, SpotifySinkHandle};
#[cfg(feature = "webapi")]
use super::token::SharedTokenProvider;

/// Handle to the daemon's shared Spotify state. Cheap to clone via
/// `Arc<SpotifyHandle>`. Thread-safe.
pub struct SpotifyHandle {
    cred_path: PathBuf,
    /// Handle to the daemon tokio runtime. We dispatch session +
    /// player construction onto this runtime so the resulting Session
    /// captures a long-lived `tokio::runtime::Handle` instead of
    /// whatever per-track runtime happened to call `start_playback`.
    daemon_runtime: RuntimeHandle,
    inner: tokio::sync::Mutex<Inner>,
    /// Session + Player + sink, built once on first playback attempt.
    playback: OnceCell<PlaybackState>,
}

/// Mutable auth state guarded by the handle's tokio mutex. Parallel
/// callers on an empty cache coalesce into a single `load_credentials`
/// call instead of racing on the on-disk `refresh_token` rotation.
struct Inner {
    last_auth: Option<AuthResult>,
}

/// Long-lived playback state built once per daemon. Cloned fields here
/// are cheap — `Arc<Player>` is an Arc, `Session` is `Arc<SessionData>`
/// internally, `SpotifySinkHandle` is an Arc-backed slot.
struct PlaybackState {
    player: Arc<Player>,
    session: Session,
    sink_handle: SpotifySinkHandle,
    /// Sample rate the sink's resampler was built for. Locked at first
    /// initialisation; subsequent `start_playback` calls must agree or
    /// we error loudly (device rate should never change within a daemon
    /// lifetime — if it ever did we'd need a different design).
    target_rate: u32,
}

/// RAII guard over one active playback. Binds a PCM channel onto the
/// shared sink and tears it down on drop. Exposes everything the
/// playback loop needs — the shared `Arc<Player>`, a clone of the
/// shared Session, the PCM receiver, and a `Notify` that fires when
/// the sink pushes frames.
pub struct PlaybackGuard {
    player: Arc<Player>,
    session: Session,
    sink_handle: SpotifySinkHandle,
    pcm_rx: std::sync::mpsc::Receiver<Vec<StereoFrame>>,
    pcm_notify: Arc<tokio::sync::Notify>,
    /// Player event subscription for this playback. Each
    /// `start_playback` call subscribes a fresh receiver — librespot
    /// hands out new `UnboundedReceiver`s from
    /// `get_player_event_channel()`, so no state leaks between tracks.
    player_events: Option<PlayerEventChannel>,
}

impl PlaybackGuard {
    pub fn player(&self) -> &Arc<Player> {
        &self.player
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn pcm_rx(&self) -> &std::sync::mpsc::Receiver<Vec<StereoFrame>> {
        &self.pcm_rx
    }

    pub fn pcm_notify(&self) -> &Arc<tokio::sync::Notify> {
        &self.pcm_notify
    }

    /// Take ownership of the player-event receiver. Callable once per
    /// guard; subsequent calls return `None`. Owned rather than
    /// borrowed so callers can pass it into a `tokio::select!`
    /// without fighting the borrow checker over the rest of the guard.
    pub fn take_player_events(&mut self) -> Option<PlayerEventChannel> {
        self.player_events.take()
    }
}

impl Drop for PlaybackGuard {
    fn drop(&mut self) {
        // Stop the decoder — sends a Stop command to PlayerInternal's
        // dedicated thread; returns immediately. Keeps the Player
        // responsive for the next `player.load()` on the next track.
        self.player.stop();

        // Stop routing PCM. Any in-flight frames being resampled will
        // land in an unbound sink and be silently discarded — better
        // than `SinkError` noise during normal track teardown.
        self.sink_handle.unbind();

        // Drop order: `pcm_rx` drops after `self.sink_handle.unbind()`
        // returns (it's the next field in struct-drop order). If a
        // `sink.write` call is blocked in `SyncSender::send` waiting
        // for this receiver, dropping `pcm_rx` wakes it with
        // `Disconnected` (swallowed inside the sink). This is the
        // PR #29 shutdown-deadlock fix, now owned by the guard.
    }
}

impl SpotifyHandle {
    /// Construct a handle. No disk I/O, no network — the first call to
    /// [`start_playback`](Self::start_playback) or
    /// [`token_provider`](Self::token_provider) triggers credential
    /// loading. `daemon_runtime` must be the daemon's long-lived tokio
    /// runtime; it's where the Session will be pinned.
    pub fn new(cred_path: PathBuf, daemon_runtime: RuntimeHandle) -> Self {
        Self {
            cred_path,
            daemon_runtime,
            inner: tokio::sync::Mutex::new(Inner { last_auth: None }),
            playback: OnceCell::new(),
        }
    }

    /// Path to the on-disk credential cache. Exposed for logging /
    /// diagnostics; callers should prefer [`start_playback`](Self::start_playback)
    /// and [`token_provider`](Self::token_provider) over reloading
    /// credentials themselves.
    pub fn cred_path(&self) -> &Path {
        &self.cred_path
    }

    /// Lazy-initialise the Player + Session + sink triple and bind a
    /// fresh PCM channel onto the sink. Returns an RAII [`PlaybackGuard`]
    /// whose drop tears down the binding.
    ///
    /// Must be called after the audio device rate is known; the sink's
    /// resampler is configured for `target_rate` at first init. Passing
    /// a different rate on a later call is a programmer error.
    pub async fn start_playback(&self, target_rate: u32) -> Result<PlaybackGuard> {
        let state = self.ensure_playback_state(target_rate).await?;

        if state.target_rate != target_rate {
            anyhow::bail!(
                "spotify: playback state was initialised at {} Hz but called at {} Hz — \
                 the device rate must not change during a daemon lifetime",
                state.target_rate,
                target_rate
            );
        }

        let (pcm_rx, pcm_notify) = state.sink_handle.bind();
        let player_events = state.player.get_player_event_channel();

        Ok(PlaybackGuard {
            player: Arc::clone(&state.player),
            session: state.session.clone(),
            sink_handle: state.sink_handle.clone(),
            pcm_rx,
            pcm_notify,
            player_events: Some(player_events),
        })
    }

    /// Force a fresh credential load and reconnect the shared Session.
    /// Used on `PlayerEvent::SessionDisconnected` — 3 attempts with
    /// 1s/2s/4s backoff, force-refreshing auth on each attempt because
    /// the `refresh_token` on disk may have rotated since the last call.
    ///
    /// Preserves v1.1 `attempt_reconnect` behaviour. Runs the
    /// `session.connect` call on the daemon runtime so the session's
    /// internal work keeps dispatching to the runtime it was born on.
    pub async fn reconnect(&self) -> Result<()> {
        const DELAYS: [Duration; 3] = [
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4),
        ];

        let state = self
            .playback
            .get()
            .context("spotify: reconnect called before start_playback — no session to reconnect")?;

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

            let session = state.session.clone();
            let connect_result = self
                .daemon_runtime
                .spawn(async move { session.connect(credentials, false).await })
                .await
                .context("spotify: daemon runtime task panicked during reconnect")?;

            match connect_result {
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

    /// Build a [`SharedTokenProvider`] snapshot from the cached auth
    /// state, loading credentials from disk if this is the first call.
    /// The returned provider owns a clone of the current OAuth token;
    /// subsequent refreshes via [`SharedTokenProvider::refresh`] operate
    /// on the provider's copy and don't invalidate the handle's cache.
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

    /// Initialise [`PlaybackState`] if this is the first caller. All
    /// the construction runs on the daemon runtime (via `spawn`) so
    /// `Session::new`'s `Handle::current()` capture pins the session
    /// to the long-lived runtime.
    async fn ensure_playback_state(&self, target_rate: u32) -> Result<&PlaybackState> {
        self.playback
            .get_or_try_init(|| async {
                let credentials = {
                    let mut inner = self.inner.lock().await;
                    ensure_auth(&mut inner, &self.cred_path).await?;
                    inner
                        .last_auth
                        .as_ref()
                        .expect("ensure_auth populated last_auth")
                        .credentials
                        .clone()
                };

                let rt = self.daemon_runtime.clone();
                let state =
                    rt.clone()
                        .spawn(async move {
                            let session = Session::new(SessionConfig::default(), None);
                            session.connect(credentials, false).await.map_err(|e| {
                                anyhow::anyhow!("Spotify session connect failed: {e}")
                            })?;
                            info!("spotify: session connected");

                            ensure_premium(&session).await?;

                            let (sink, sink_handle) = new_sink(target_rate);
                            let player = Player::new(
                                PlayerConfig::default(),
                                session.clone(),
                                Box::new(NoOpVolume),
                                move || Box::new(sink),
                            );
                            info!(
                                target_rate,
                                "spotify: player + sink initialised (singleton)"
                            );

                            Ok::<_, anyhow::Error>(PlaybackState {
                                player,
                                session,
                                sink_handle,
                                target_rate,
                            })
                        })
                        .await
                        .context("spotify: daemon runtime task panicked during init")??;

                Ok(state)
            })
            .await
    }
}

/// Populate `inner.last_auth` if empty. Runs `load_credentials` on the
/// blocking pool so the token-refresh HTTP round-trip doesn't stall
/// the current-thread runtime the source pipeline runs on.
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

/// Wait up to ~1s for librespot to receive the user-data `type`
/// attribute and bail with `premium_required` if the account isn't
/// Premium. Runs once at session-init time (inside the handle's
/// lazy init) rather than once per playback — a Premium subscription
/// doesn't change between tracks.
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

    fn test_handle(cred_path: &str) -> SpotifyHandle {
        SpotifyHandle::new(PathBuf::from(cred_path), tokio::runtime::Handle::current())
    }

    #[tokio::test]
    async fn new_handle_does_no_io() {
        // Construction must not read from disk or hit the network; the
        // daemon builds a handle unconditionally at startup, including
        // on runs that never touch Spotify.
        let handle = test_handle("/tmp/clitunes-test-nonexistent.json");
        assert_eq!(
            handle.cred_path(),
            Path::new("/tmp/clitunes-test-nonexistent.json")
        );
    }

    #[tokio::test]
    async fn start_playback_fails_fast_without_credentials() {
        // Missing credential file → auth::load_credentials returns an
        // anyhow error containing "no cached Spotify credentials". We
        // want that error to surface with its original wording so the
        // daemon can present the right remediation to the user.
        let handle = test_handle("/tmp/clitunes-test-handle-missing-creds.json");
        let err = match handle.start_playback(48_000).await {
            Ok(_) => panic!("start_playback should fail with no cached credentials"),
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
        let handle = test_handle("/tmp/clitunes-test-handle-missing-tp.json");
        let err = match handle.token_provider().await {
            Ok(_) => panic!("token_provider() should fail with no cached credentials"),
            Err(e) => e,
        };
        assert!(
            format!("{err:#}").contains("no cached Spotify credentials"),
            "unexpected error: {err:#}"
        );
    }

    #[tokio::test]
    async fn reconnect_without_initialised_session_errors_cleanly() {
        // `reconnect` is only meaningful once a session exists. Calling
        // it before `start_playback` should surface a clear error
        // rather than panicking — the daemon's error path relies on
        // this to produce a usable SourceError message.
        let handle = test_handle("/tmp/clitunes-test-reconnect-no-session.json");
        let err = match handle.reconnect().await {
            Ok(_) => panic!("reconnect() without an initialised session should fail"),
            Err(e) => e,
        };
        assert!(
            format!("{err:#}").contains("before start_playback"),
            "unexpected error: {err:#}"
        );
    }
}
