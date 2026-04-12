//! Spotify playback via librespot (v1.1).
//!
//! Provides [`SpotifySource`] — an implementation of the [`Source`](super::Source) trait
//! that bridges librespot's decoded PCM output to the daemon's audio pipeline
//! with 44100→48000 Hz resampling via rubato.

pub mod auth;
pub mod sink;
