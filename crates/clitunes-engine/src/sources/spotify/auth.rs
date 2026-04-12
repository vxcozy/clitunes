//! Spotify OAuth2 PKCE authentication with credential caching.
//!
//! Wraps librespot-oauth for the browser-based PKCE flow and caches
//! credentials at `~/.config/clitunes/spotify/credentials.json` (mode 0600).
