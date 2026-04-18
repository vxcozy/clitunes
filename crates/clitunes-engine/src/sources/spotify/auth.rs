//! Spotify OAuth2 PKCE authentication with credential caching.
//!
//! Wraps librespot-oauth for the browser-based PKCE flow and caches
//! credentials at `~/.config/clitunes/spotify/credentials.json` (mode 0600).
//!
//! Supports headless/SSH environments: when no local display is available,
//! the flow prints the OAuth URL and port-forward instructions instead of
//! opening a browser.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use librespot_core::authentication::Credentials;
use librespot_oauth::{OAuthClientBuilder, OAuthError, OAuthToken};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tracing::{info, warn};

/// Shared librespot PKCE client ID — the same one spotifyd, ncspot, and
/// every other librespot-backed client ship with. Fine for playback, but
/// **rate-limited across every app that uses it**, so the Web API (search,
/// library, playlist) routinely serves HTTP 429 on a warm afternoon.
///
/// Users who register their own Spotify Developer App and set
/// `$CLITUNES_SPOTIFY_CLIENT_ID` bypass that shared quota. See
/// [`spotify_client_id`] for the resolution order.
pub(crate) const LIBRESPOT_SHARED_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";

/// Environment variable that overrides the OAuth client_id.
///
/// Must come from an app registered on the Spotify Developer dashboard
/// with redirect URI `http://127.0.0.1:8898/login` (see
/// [`SPOTIFY_REDIRECT_URI`]). Registering under a different URI will
/// make the OAuth callback fail.
pub(crate) const CLIENT_ID_ENV: &str = "CLITUNES_SPOTIFY_CLIENT_ID";

/// Resolve the OAuth client_id: user override via `$CLITUNES_SPOTIFY_CLIENT_ID`
/// if set (and non-empty), otherwise [`LIBRESPOT_SHARED_CLIENT_ID`].
///
/// Read at every OAuth / refresh call. Changing the env var between
/// auth and refresh will cause refresh to 401 — the caller falls
/// through to interactive re-auth, which is the expected UX.
///
/// # Examples
///
/// ```
/// use clitunes_engine::sources::spotify::auth::spotify_client_id;
/// # std::env::set_var("CLITUNES_SPOTIFY_CLIENT_ID", "my-developer-app-id");
/// assert_eq!(spotify_client_id(), "my-developer-app-id");
/// # std::env::remove_var("CLITUNES_SPOTIFY_CLIENT_ID");
/// ```
pub fn spotify_client_id() -> String {
    std::env::var(CLIENT_ID_ENV)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| LIBRESPOT_SHARED_CLIENT_ID.to_owned())
}

/// Redirect URI registered for the PKCE client. Must use this exact port.
pub(crate) const SPOTIFY_REDIRECT_URI: &str = "http://127.0.0.1:8898/login";

/// All scopes needed for playback + Web API (v1.2).
pub(crate) const SPOTIFY_SCOPES: &[&str] = &[
    "streaming",
    "user-library-read",
    "playlist-read-private",
    "user-read-recently-played",
];

/// HTML response shown in the browser after successful OAuth callback.
const REDIRECT_RESPONSE: &str = r#"<!doctype html>
<html><body><h1>Authenticated — return to your terminal.</h1></body></html>"#;

/// On-disk credential cache.
#[derive(Serialize, Deserialize)]
struct CachedCredentials {
    refresh_token: String,
    consent_given: bool,
    /// Scopes that were granted during the last OAuth flow. Old credential
    /// files (pre-v1.2) lack this field — `#[serde(default)]` deserializes
    /// them as `vec![]`, which triggers re-auth when the required scopes
    /// aren't satisfied.
    #[serde(default)]
    scopes: Vec<String>,
}

/// Default path: `~/.config/clitunes/spotify/credentials.json`.
pub fn default_credentials_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| {
        base.join("clitunes")
            .join("spotify")
            .join("credentials.json")
    })
}

/// Result of a successful credential load or authentication: both the
/// raw OAuth token (for rspotify) and librespot-ready credentials.
pub struct AuthResult {
    /// The raw OAuth token (access + refresh). Used by [`SharedTokenProvider`](super::token::SharedTokenProvider)
    /// to construct both rspotify and librespot credentials.
    pub token: OAuthToken,
    /// librespot-ready session credentials.
    pub credentials: Credentials,
}

/// Load cached credentials and refresh the access token. Returns
/// session-ready `Credentials`. If no cache exists, refresh fails,
/// or cached scopes are insufficient, returns an error — **never**
/// prompts interactively.
///
/// This is the daemon-safe entry point. The daemon is a double-forked
/// detached process with no terminal; interactive auth must be driven
/// by the client via [`load_or_authenticate`].
///
/// # Examples
///
/// A missing credential file produces a clean `Err` — no prompt, no
/// hang. This is the invariant the daemon relies on:
///
/// ```
/// use std::path::PathBuf;
/// use clitunes_engine::sources::spotify::auth::load_credentials;
///
/// let missing = PathBuf::from("/tmp/clitunes-doctest-nonexistent.json");
/// match load_credentials(&missing) {
///     Ok(_) => panic!("expected an error for a missing credential file"),
///     Err(e) => assert!(e.to_string().contains("no cached Spotify credentials")),
/// }
/// ```
pub fn load_credentials(cred_path: &Path) -> Result<AuthResult> {
    let cached = load_cached(cred_path)?
        .ok_or_else(|| anyhow::anyhow!("no cached Spotify credentials; run `clitunes auth` or play a Spotify URI from the client to authenticate"))?;

    if !scopes_sufficient(&cached.scopes) {
        anyhow::bail!(
            "cached Spotify credentials lack required scopes; \
             run `clitunes auth` to re-authenticate with expanded permissions"
        );
    }

    let token = refresh_access_token(&cached.refresh_token)
        .map_err(|e| anyhow::anyhow!("Spotify token refresh failed: {e}"))?;

    info!("Spotify token refreshed successfully");
    // Update the cache with the new refresh token (Spotify may rotate it).
    save_cached(
        cred_path,
        &CachedCredentials {
            refresh_token: token.refresh_token.clone(),
            consent_given: true,
            scopes: required_scopes(),
        },
    )?;

    Ok(AuthResult {
        credentials: Credentials::with_access_token(token.access_token.clone()),
        token,
    })
}

/// Returns `true` when the session appears to be headless (SSH without
/// X11/Wayland forwarding). In headless mode the OAuth flow prints the
/// URL instead of opening a browser.
///
/// Detection: `$SSH_CONNECTION` or `$SSH_TTY` is set AND neither
/// `$DISPLAY` nor `$WAYLAND_DISPLAY` is set.
pub fn is_headless() -> bool {
    let ssh = std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
    let display =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    detect_headless(ssh, display)
}

/// Pure logic for headless detection, factored out for testability.
fn detect_headless(ssh: bool, display: bool) -> bool {
    ssh && !display
}

/// Load cached credentials, refresh the access token, and return
/// session-ready [`AuthResult`]. If no cache exists, refresh fails,
/// or scopes are insufficient, runs the interactive PKCE flow.
///
/// In headless mode (SSH without display), the OAuth URL is printed
/// with port-forward instructions instead of opening a browser.
///
/// The very first invocation shows a TOS consent prompt; consent
/// is persisted in the credential file so it only appears once.
///
/// **Client-side only.** Never call this from the daemon — use
/// [`load_credentials`] instead.
pub fn load_or_authenticate(cred_path: &Path) -> Result<AuthResult> {
    // Try cached credentials first (with scope check).
    if let Some(cached) = load_cached(cred_path)? {
        if scopes_sufficient(&cached.scopes) {
            match refresh_access_token(&cached.refresh_token) {
                Ok(token) => {
                    info!("Spotify token refreshed successfully");
                    save_cached(
                        cred_path,
                        &CachedCredentials {
                            refresh_token: token.refresh_token.clone(),
                            consent_given: true,
                            scopes: required_scopes(),
                        },
                    )?;
                    return Ok(AuthResult {
                        credentials: Credentials::with_access_token(token.access_token.clone()),
                        token,
                    });
                }
                Err(e) => {
                    warn!("Spotify token refresh failed, re-authenticating: {e}");
                }
            }
        } else {
            info!("cached Spotify scopes insufficient, re-authenticating for expanded permissions");
        }
    }

    // No valid cached credentials or scopes insufficient — need interactive auth.
    ensure_consent(cred_path)?;

    let headless = is_headless();
    if headless {
        print_headless_instructions();
    }

    let token = run_oauth_flow(!headless).context("Spotify OAuth PKCE flow failed")?;
    info!("Spotify OAuth flow completed");

    save_cached(
        cred_path,
        &CachedCredentials {
            refresh_token: token.refresh_token.clone(),
            consent_given: true,
            scopes: required_scopes(),
        },
    )?;

    Ok(AuthResult {
        credentials: Credentials::with_access_token(token.access_token.clone()),
        token,
    })
}

/// Check whether the user has already consented to using librespot.
/// If not, print the consent prompt and require confirmation.
///
/// Returns `Ok(())` if consent is given (now or previously).
fn ensure_consent(cred_path: &Path) -> Result<()> {
    // If cached creds exist with consent_given, we're good.
    if let Some(cached) = load_cached(cred_path)? {
        if cached.consent_given {
            return Ok(());
        }
    }

    // Print the consent prompt.
    eprintln!(
        "\n\x1b[1;33mNotice:\x1b[0m librespot is an unofficial Spotify client that \
         reverse-engineers\nSpotify's proprietary protocol. Using it may affect your \
         Spotify account.\n"
    );
    eprint!("Continue? [y/N] ");
    std::io::stderr().flush().ok();

    let mut response = String::new();
    std::io::stdin()
        .read_line(&mut response)
        .context("reading consent response from stdin")?;

    if !response.trim().eq_ignore_ascii_case("y") {
        bail!(
            "Spotify authentication cancelled. \
             Use a Spotify Premium account and re-run to authenticate."
        );
    }

    Ok(())
}

/// Print headless-mode instructions to stderr before the OAuth flow starts.
fn print_headless_instructions() {
    eprintln!(
        "\n\x1b[1;36mHeadless mode detected\x1b[0m (SSH session, no display)\n\
         \n\
         The OAuth URL will be printed below. Open it in a browser on a\n\
         machine that can reach this host on port 8898.\n\
         \n\
         If your terminal is remote, set up a port forward first:\n\
         \n\
         \x1b[1m  ssh -L 8898:127.0.0.1:8898 <this-host>\x1b[0m\n\
         \n\
         Then open the URL in your local browser. The callback will\n\
         route through the tunnel to complete authentication.\n"
    );
}

/// Run the OAuth2 PKCE flow via librespot-oauth. Listens on
/// `127.0.0.1:8898` for the redirect callback. When `open_browser`
/// is true, also opens the auth URL in the default browser.
fn run_oauth_flow(open_browser: bool) -> Result<OAuthToken, OAuthError> {
    let client_id = spotify_client_id();
    let mut builder =
        OAuthClientBuilder::new(&client_id, SPOTIFY_REDIRECT_URI, SPOTIFY_SCOPES.to_vec())
            .with_custom_message(REDIRECT_RESPONSE);

    if open_browser {
        builder = builder.open_in_browser();
    }

    builder.build()?.get_access_token()
}

/// Refresh an existing access token using a cached refresh token.
fn refresh_access_token(refresh_token: &str) -> Result<OAuthToken, OAuthError> {
    let client_id = spotify_client_id();
    let client = OAuthClientBuilder::new(&client_id, SPOTIFY_REDIRECT_URI, SPOTIFY_SCOPES.to_vec())
        .build()?;

    client.refresh_token(refresh_token)
}

// ── Scope helpers ─────────────────────────────────────────────────

/// The canonical list of required scopes as owned strings.
fn required_scopes() -> Vec<String> {
    SPOTIFY_SCOPES.iter().map(|s| (*s).to_owned()).collect()
}

/// Returns `true` if `granted` contains every scope in [`SPOTIFY_SCOPES`].
/// An empty `granted` (pre-v1.2 credential files) always fails.
fn scopes_sufficient(granted: &[String]) -> bool {
    SPOTIFY_SCOPES
        .iter()
        .all(|required| granted.iter().any(|g| g == required))
}

/// Snapshot of on-disk auth state for display surfaces. Computed by
/// inspecting the credential file without touching the network —
/// callers that want a live session should use [`load_credentials`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStatus {
    /// No credential file on disk. `clitunes auth` has never succeeded.
    LoggedOut,
    /// Credential file present but unreadable (I/O error, corrupt JSON,
    /// or parse failure). Stores a short human-readable reason.
    Unreadable(String),
    /// Credential file present but the granted scopes are missing one
    /// or more entries in [`SPOTIFY_SCOPES`] — typically a pre-v1.2
    /// credential file that needs re-auth for the expanded permissions.
    ScopesInsufficient,
    /// Credential file present with all required scopes. This is the
    /// happy path: the daemon can refresh a token from this file.
    LoggedIn,
}

/// Inspect the on-disk credential file at `path` and report its
/// current state. Never opens the network; safe to call on the
/// daemon's hot path (e.g. from a verb dispatcher).
pub fn cached_auth_status(path: &Path) -> AuthStatus {
    match load_cached(path) {
        Ok(None) => AuthStatus::LoggedOut,
        Ok(Some(creds)) => {
            if scopes_sufficient(&creds.scopes) {
                AuthStatus::LoggedIn
            } else {
                AuthStatus::ScopesInsufficient
            }
        }
        Err(e) => AuthStatus::Unreadable(e.to_string()),
    }
}

// ── Credential persistence ─────────────────────────────────────────

/// Load credentials from `path`. Returns `Ok(None)` if the file doesn't
/// exist, `Err` only on I/O or deserialization failures.
fn load_cached(path: &Path) -> Result<Option<CachedCredentials>> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("reading Spotify credentials from {}", path.display()));
        }
    };
    let creds: CachedCredentials = serde_json::from_str(&raw)
        .with_context(|| format!("parsing Spotify credentials at {}", path.display()))?;
    Ok(Some(creds))
}

/// Atomically write credentials to `path` with 0600 perms. Creates
/// parent directories with 0700 if they don't exist.
///
/// Follows the same atomic-write pattern as `tui::persistence::save_state`.
fn save_cached(path: &Path, creds: &CachedCredentials) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("credential path has no parent: {}", path.display()))?;
    ensure_parent_dir(parent)?;

    let serialized =
        serde_json::to_string_pretty(creds).context("serialize Spotify credentials")?;

    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temp file in {}", parent.display()))?;

    // Tighten perms before writing anything sensitive.
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o600))
        .context("chmod 0600 credential tempfile")?;

    tmp.write_all(serialized.as_bytes())
        .context("write credential bytes")?;
    tmp.as_file()
        .sync_all()
        .context("fsync credential tempfile")?;

    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("persist credential file: {e}"))?;

    // Belt-and-braces: reassert 0600 after rename.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .context("chmod 0600 credential file")?;

    Ok(())
}

/// Create `dir` (and parents) with mode 0700. If it already exists,
/// tighten to 0700 only when the current mode is more permissive.
fn ensure_parent_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        fs::create_dir_all(dir)
            .with_context(|| format!("creating credential parent dir {}", dir.display()))?;
    }
    let meta =
        fs::metadata(dir).with_context(|| format!("stat credential dir {}", dir.display()))?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 0700 {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn credential_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join("spotify")
            .join("nested")
            .join("credentials.json");

        let creds = CachedCredentials {
            refresh_token: "test-refresh-token-abc123".into(),
            consent_given: true,
            scopes: required_scopes(),
        };
        save_cached(&path, &creds).unwrap();

        let loaded = load_cached(&path).unwrap().unwrap();
        assert_eq!(loaded.refresh_token, "test-refresh-token-abc123");
        assert!(loaded.consent_given);
        assert_eq!(loaded.scopes, required_scopes());
    }

    #[test]
    fn credential_file_permissions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credentials.json");

        let creds = CachedCredentials {
            refresh_token: "tok".into(),
            consent_given: true,
            scopes: required_scopes(),
        };
        save_cached(&path, &creds).unwrap();

        let meta = fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "credential file should be mode 0600");
    }

    #[test]
    fn parent_dir_permissions() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("spotify");
        let path = nested.join("credentials.json");

        let creds = CachedCredentials {
            refresh_token: "tok".into(),
            consent_given: true,
            scopes: required_scopes(),
        };
        save_cached(&path, &creds).unwrap();

        // The immediate parent dir should be tightened to 0700.
        let meta = fs::metadata(&nested).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "parent dir should be mode 0700");
    }

    #[test]
    fn missing_credentials_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result = load_cached(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn consent_flag_persists() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credentials.json");

        // Save with consent given.
        let creds = CachedCredentials {
            refresh_token: "tok".into(),
            consent_given: true,
            scopes: required_scopes(),
        };
        save_cached(&path, &creds).unwrap();

        // Load and verify consent is persisted.
        let loaded = load_cached(&path).unwrap().unwrap();
        assert!(loaded.consent_given);
    }

    #[test]
    fn corrupt_credentials_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        fs::write(&path, "not valid json {{{").unwrap();

        let result = load_cached(&path);
        assert!(result.is_err());
    }

    #[test]
    fn default_path_contains_spotify_dir() {
        if let Some(path) = default_credentials_path() {
            assert!(path.to_string_lossy().contains("spotify"));
            assert!(path.to_string_lossy().contains("clitunes"));
            assert!(path.to_string_lossy().ends_with("credentials.json"));
        }
        // If dirs::config_dir() returns None (e.g., in CI), this is fine.
    }

    #[test]
    fn overwrite_credentials() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credentials.json");

        // Write initial credentials.
        save_cached(
            &path,
            &CachedCredentials {
                refresh_token: "old-token".into(),
                consent_given: false,
                scopes: vec!["streaming".into()],
            },
        )
        .unwrap();

        // Overwrite with new credentials.
        save_cached(
            &path,
            &CachedCredentials {
                refresh_token: "new-token".into(),
                consent_given: true,
                scopes: required_scopes(),
            },
        )
        .unwrap();

        let loaded = load_cached(&path).unwrap().unwrap();
        assert_eq!(loaded.refresh_token, "new-token");
        assert!(loaded.consent_given);
    }

    // ── Scope-checking tests ─────────────────────────────────────────

    #[test]
    fn scopes_sufficient_with_all_required() {
        let granted = required_scopes();
        assert!(scopes_sufficient(&granted));
    }

    #[test]
    fn scopes_sufficient_with_superset() {
        let mut granted = required_scopes();
        granted.push("user-modify-playback-state".into());
        assert!(scopes_sufficient(&granted));
    }

    #[test]
    fn scopes_insufficient_with_empty() {
        assert!(!scopes_sufficient(&[]));
    }

    #[test]
    fn scopes_insufficient_with_old_streaming_only() {
        let granted = vec!["streaming".into()];
        assert!(!scopes_sufficient(&granted));
    }

    #[test]
    fn scopes_insufficient_missing_one() {
        // All except user-read-recently-played.
        let granted = vec![
            "streaming".into(),
            "user-library-read".into(),
            "playlist-read-private".into(),
        ];
        assert!(!scopes_sufficient(&granted));
    }

    #[test]
    fn pre_v12_credentials_deserialize_with_empty_scopes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credentials.json");

        // Simulate a pre-v1.2 credential file (no scopes field).
        let json = r#"{"refresh_token":"old-tok","consent_given":true}"#;
        fs::write(&path, json).unwrap();

        let loaded = load_cached(&path).unwrap().unwrap();
        assert!(loaded.scopes.is_empty());
        assert!(!scopes_sufficient(&loaded.scopes));
    }

    #[test]
    fn credential_roundtrip_preserves_scopes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credentials.json");

        let scopes = required_scopes();
        save_cached(
            &path,
            &CachedCredentials {
                refresh_token: "tok".into(),
                consent_given: true,
                scopes: scopes.clone(),
            },
        )
        .unwrap();

        let loaded = load_cached(&path).unwrap().unwrap();
        assert_eq!(loaded.scopes, scopes);
    }

    // ── Headless detection tests ─────────────────────────────────────

    #[test]
    fn headless_ssh_no_display() {
        assert!(detect_headless(true, false));
    }

    #[test]
    fn not_headless_no_ssh() {
        assert!(!detect_headless(false, false));
    }

    #[test]
    fn not_headless_ssh_with_x11_forwarding() {
        assert!(!detect_headless(true, true));
    }

    #[test]
    fn not_headless_local_desktop() {
        assert!(!detect_headless(false, true));
    }

    // ── client_id resolution tests ───────────────────────────────────
    //
    // `std::env` is process-global; these tests serialise through a
    // `Mutex` so they never race with each other or with the doctest
    // that also reads `$CLITUNES_SPOTIFY_CLIENT_ID`. The `EnvGuard`
    // clears the var on drop so a panicking assertion still leaves a
    // clean env for the next test.

    use std::sync::{Mutex, MutexGuard};
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Lock the env mutex and guarantee `$CLITUNES_SPOTIFY_CLIENT_ID`
    /// is cleared when the test returns — success or panic.
    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn acquire() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::remove_var(CLIENT_ID_ENV);
            Self { _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(CLIENT_ID_ENV);
        }
    }

    #[test]
    fn client_id_defaults_to_shared_when_env_unset() {
        let _g = EnvGuard::acquire();
        assert_eq!(spotify_client_id(), LIBRESPOT_SHARED_CLIENT_ID);
    }

    #[test]
    fn client_id_uses_env_override() {
        let _g = EnvGuard::acquire();
        std::env::set_var(CLIENT_ID_ENV, "my-own-app-id-32chars-exactly-ok");
        assert_eq!(spotify_client_id(), "my-own-app-id-32chars-exactly-ok");
    }

    #[test]
    fn client_id_trims_whitespace_from_env() {
        let _g = EnvGuard::acquire();
        std::env::set_var(CLIENT_ID_ENV, "  spaced-id  ");
        assert_eq!(spotify_client_id(), "spaced-id");
    }

    #[test]
    fn client_id_falls_back_on_empty_env() {
        // Treat an empty or all-whitespace override as "not set" so a
        // stray `export CLITUNES_SPOTIFY_CLIENT_ID=` in a shell profile
        // doesn't break auth.
        let _g = EnvGuard::acquire();
        std::env::set_var(CLIENT_ID_ENV, "   ");
        assert_eq!(spotify_client_id(), LIBRESPOT_SHARED_CLIENT_ID);
    }
}
