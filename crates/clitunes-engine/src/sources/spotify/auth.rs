//! Spotify OAuth2 PKCE authentication with credential caching.
//!
//! Wraps librespot-oauth for the browser-based PKCE flow and caches
//! credentials at `~/.config/clitunes/spotify/credentials.json` (mode 0600).

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

/// Spotify's embedded PKCE client ID (same as spotifyd, ncspot, etc.).
const SPOTIFY_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";

/// Redirect URI registered for the PKCE client. Must use this exact port.
const SPOTIFY_REDIRECT_URI: &str = "http://127.0.0.1:8898/login";

/// Minimum scopes needed for playback.
const SPOTIFY_SCOPES: &[&str] = &["streaming"];

/// HTML response shown in the browser after successful OAuth callback.
const REDIRECT_RESPONSE: &str = r#"<!doctype html>
<html><body><h1>Authenticated — return to your terminal.</h1></body></html>"#;

/// On-disk credential cache.
#[derive(Serialize, Deserialize)]
struct CachedCredentials {
    refresh_token: String,
    consent_given: bool,
}

/// Default path: `~/.config/clitunes/spotify/credentials.json`.
pub fn default_credentials_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| {
        base.join("clitunes")
            .join("spotify")
            .join("credentials.json")
    })
}

/// Load cached credentials, refresh the access token, and return
/// session-ready `Credentials`. If no cache exists or refresh fails,
/// runs the interactive PKCE flow (which opens a browser).
///
/// The very first invocation shows a TOS consent prompt; consent
/// is persisted in the credential file so it only appears once.
pub fn load_or_authenticate(cred_path: &Path) -> Result<Credentials> {
    // Try cached credentials first.
    if let Some(cached) = load_cached(cred_path)? {
        match refresh_access_token(&cached.refresh_token) {
            Ok(token) => {
                info!("Spotify token refreshed successfully");
                // Update the cache with the new refresh token (Spotify may rotate it).
                save_cached(
                    cred_path,
                    &CachedCredentials {
                        refresh_token: token.refresh_token,
                        consent_given: true,
                    },
                )?;
                return Ok(Credentials::with_access_token(token.access_token));
            }
            Err(e) => {
                warn!("Spotify token refresh failed, re-authenticating: {e}");
            }
        }
    }

    // No valid cached credentials — need interactive auth.
    ensure_consent(cred_path)?;

    let token = run_oauth_flow().context("Spotify OAuth PKCE flow failed")?;
    info!("Spotify OAuth flow completed");

    save_cached(
        cred_path,
        &CachedCredentials {
            refresh_token: token.refresh_token,
            consent_given: true,
        },
    )?;

    Ok(Credentials::with_access_token(token.access_token))
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

/// Run the OAuth2 PKCE flow via librespot-oauth. Opens a browser and
/// listens on `127.0.0.1:8898` for the redirect callback.
fn run_oauth_flow() -> Result<OAuthToken, OAuthError> {
    let client = OAuthClientBuilder::new(
        SPOTIFY_CLIENT_ID,
        SPOTIFY_REDIRECT_URI,
        SPOTIFY_SCOPES.to_vec(),
    )
    .open_in_browser()
    .with_custom_message(REDIRECT_RESPONSE)
    .build()?;

    client.get_access_token()
}

/// Refresh an existing access token using a cached refresh token.
fn refresh_access_token(refresh_token: &str) -> Result<OAuthToken, OAuthError> {
    let client = OAuthClientBuilder::new(
        SPOTIFY_CLIENT_ID,
        SPOTIFY_REDIRECT_URI,
        SPOTIFY_SCOPES.to_vec(),
    )
    .build()?;

    client.refresh_token(refresh_token)
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
        };
        save_cached(&path, &creds).unwrap();

        let loaded = load_cached(&path).unwrap().unwrap();
        assert_eq!(loaded.refresh_token, "test-refresh-token-abc123");
        assert!(loaded.consent_given);
    }

    #[test]
    fn credential_file_permissions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("credentials.json");

        let creds = CachedCredentials {
            refresh_token: "tok".into(),
            consent_given: true,
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
            },
        )
        .unwrap();

        // Overwrite with new credentials.
        save_cached(
            &path,
            &CachedCredentials {
                refresh_token: "new-token".into(),
                consent_given: true,
            },
        )
        .unwrap();

        let loaded = load_cached(&path).unwrap().unwrap();
        assert_eq!(loaded.refresh_token, "new-token");
        assert!(loaded.consent_given);
    }
}
