//! Shared token provider bridging librespot-oauth tokens to rspotify and
//! librespot-core consumers.
//!
//! The daemon creates one [`SharedTokenProvider`] at startup. The source
//! pipeline calls [`SharedTokenProvider::librespot_credentials`] for
//! session creation, and the verb dispatcher calls
//! [`SharedTokenProvider::rspotify_token`] for Web API calls. Only
//! rspotify (daemon-side) refreshes tokens; librespot receives fresh
//! `Credentials::with_access_token()` on each session and never refreshes
//! independently.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use librespot_core::authentication::Credentials;
use librespot_oauth::OAuthToken;
use rspotify::Token;

use super::auth;

/// Bridges a librespot-oauth [`OAuthToken`] to both rspotify and
/// librespot consumers. Holds the latest token state; wrapped in
/// `Arc<Mutex<SharedTokenProvider>>` by the daemon event loop.
pub struct SharedTokenProvider {
    token: OAuthToken,
    cred_path: PathBuf,
}

impl SharedTokenProvider {
    /// Create a provider from a freshly obtained or refreshed token.
    pub fn new(token: OAuthToken, cred_path: PathBuf) -> Self {
        Self { token, cred_path }
    }

    /// Build an rspotify [`Token`] from the current OAuth state.
    ///
    /// The returned token carries the access token, scopes, and an
    /// estimated `expires_at` derived from the librespot-oauth
    /// `Instant`-based expiry.
    pub fn rspotify_token(&self) -> Token {
        let expires_in = self
            .token
            .expires_at
            .checked_duration_since(std::time::Instant::now())
            .unwrap_or(Duration::ZERO);

        let scopes: HashSet<String> = self.token.scopes.iter().cloned().collect();

        Token {
            access_token: self.token.access_token.clone(),
            expires_in: chrono::Duration::from_std(expires_in).unwrap_or(chrono::Duration::zero()),
            expires_at: Some(chrono::Utc::now() + expires_in),
            refresh_token: Some(self.token.refresh_token.clone()),
            scopes,
        }
    }

    /// Build librespot [`Credentials`] from the current access token.
    pub fn librespot_credentials(&self) -> Credentials {
        Credentials::with_access_token(&self.token.access_token)
    }

    /// Replace the held token (e.g. after a refresh cycle).
    pub fn update_token(&mut self, token: OAuthToken) {
        self.token = token;
    }

    /// Refresh the token via the credential cache and update internal
    /// state. Returns `Err` if the refresh fails — the caller should
    /// surface an `auth_required` error to the client.
    pub fn refresh(&mut self) -> Result<()> {
        let auth_result = auth::load_credentials(&self.cred_path)?;
        self.token = auth_result.token;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn fake_token() -> OAuthToken {
        OAuthToken {
            access_token: "test-access-token".into(),
            refresh_token: "test-refresh-token".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
            token_type: "Bearer".into(),
            scopes: vec![
                "streaming".into(),
                "user-library-read".into(),
                "playlist-read-private".into(),
                "user-read-recently-played".into(),
            ],
        }
    }

    #[test]
    fn rspotify_token_has_access_token() {
        let provider = SharedTokenProvider::new(fake_token(), "/tmp/test".into());
        let tok = provider.rspotify_token();
        assert_eq!(tok.access_token, "test-access-token");
    }

    #[test]
    fn rspotify_token_has_scopes() {
        let provider = SharedTokenProvider::new(fake_token(), "/tmp/test".into());
        let tok = provider.rspotify_token();
        assert!(tok.scopes.contains("streaming"));
        assert!(tok.scopes.contains("user-library-read"));
        assert!(tok.scopes.contains("playlist-read-private"));
        assert!(tok.scopes.contains("user-read-recently-played"));
        assert_eq!(tok.scopes.len(), 4);
    }

    #[test]
    fn rspotify_token_has_refresh_token() {
        let provider = SharedTokenProvider::new(fake_token(), "/tmp/test".into());
        let tok = provider.rspotify_token();
        assert_eq!(tok.refresh_token.as_deref(), Some("test-refresh-token"));
    }

    #[test]
    fn rspotify_token_expires_in_future() {
        let provider = SharedTokenProvider::new(fake_token(), "/tmp/test".into());
        let tok = provider.rspotify_token();
        assert!(tok.expires_at.unwrap() > chrono::Utc::now());
    }

    #[test]
    fn librespot_credentials_has_access_token() {
        let provider = SharedTokenProvider::new(fake_token(), "/tmp/test".into());
        let creds = provider.librespot_credentials();
        // Credentials::with_access_token stores the token in auth_data
        // as bytes, and sets auth_type to AUTHENTICATION_SPOTIFY_TOKEN.
        assert_eq!(creds.auth_data, b"test-access-token");
    }

    #[test]
    fn update_token_replaces_state() {
        let mut provider = SharedTokenProvider::new(fake_token(), "/tmp/test".into());
        assert_eq!(provider.rspotify_token().access_token, "test-access-token");

        let mut new_tok = fake_token();
        new_tok.access_token = "refreshed-token".into();
        provider.update_token(new_tok);

        assert_eq!(provider.rspotify_token().access_token, "refreshed-token");
    }

    #[test]
    fn rspotify_token_expired_token_gets_zero_duration() {
        let mut tok = fake_token();
        // Set expiry in the past.
        tok.expires_at = Instant::now() - Duration::from_secs(60);
        let provider = SharedTokenProvider::new(tok, "/tmp/test".into());
        let rsp_tok = provider.rspotify_token();
        // expires_in should be zero (clamped, not negative).
        assert_eq!(rsp_tok.expires_in, chrono::Duration::zero());
    }
}
