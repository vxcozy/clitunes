//! Spotify playback via librespot (v1.1).
//!
//! Provides [`SpotifySource`] — an implementation of the [`Source`](super::Source) trait
//! that bridges librespot's decoded PCM output to the daemon's audio pipeline
//! with 44100→48000 Hz resampling via rubato.

pub mod auth;
pub mod sink;

pub use auth::{default_credentials_path, load_credentials, load_or_authenticate};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_core::spotify_uri::SpotifyUri;
use librespot_playback::config::PlayerConfig;
use librespot_playback::mixer::NoOpVolume;
use librespot_playback::player::{Player, PlayerEvent};
use tokio::runtime::Builder;
use tracing::{debug, error, info, warn};

use crate::audio::ring::PcmWriter;
use crate::proto::events::Event;

use clitunes_core::sanitize;

/// A Spotify playback source. Bridges librespot's Player to the daemon's
/// audio pipeline via [`SpotifySink`](sink::SpotifySink) and rubato resampling.
pub struct SpotifySource {
    /// Spotify track URI to play (e.g. `spotify:track:4PTG3Z6ehGkBFwjybzWkR8`).
    uri: String,
    /// Path to cached credentials file.
    credentials_path: PathBuf,
    /// Channel to emit events (NowPlayingChanged, SourceError) to the daemon event loop.
    event_tx: tokio::sync::mpsc::Sender<Event>,
}

impl SpotifySource {
    /// Create a new SpotifySource. Does not start playback; call `run()` for that.
    ///
    /// - `uri`: Spotify URI (e.g. `spotify:track:...`)
    /// - `credentials_path`: path to cached OAuth credentials file
    /// - `event_tx`: sender for daemon events (NowPlaying updates, errors)
    pub fn new(
        uri: String,
        credentials_path: PathBuf,
        event_tx: tokio::sync::mpsc::Sender<Event>,
    ) -> Self {
        Self {
            uri,
            credentials_path,
            event_tx,
        }
    }
}

impl super::Source for SpotifySource {
    fn name(&self) -> &str {
        "spotify"
    }

    fn run(&mut self, writer: &mut dyn PcmWriter, stop: &AtomicBool) {
        let uri_str = self.uri.clone();
        let cred_path = self.credentials_path.clone();
        let event_tx = self.event_tx.clone();

        // Mirror outer stop → inner stop (same pattern as RadioSource).
        let inner_stop = Arc::new(AtomicBool::new(false));

        std::thread::scope(|scope| {
            // Watcher thread: polls outer stop every 50ms.
            let mirror = Arc::clone(&inner_stop);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                mirror.store(true, Ordering::SeqCst);
            });

            // Main playback thread with its own tokio runtime.
            let playback_stop = Arc::clone(&inner_stop);
            scope.spawn(move || {
                let rt = match Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        error!(error = %e, "spotify: failed to build tokio runtime");
                        return;
                    }
                };

                rt.block_on(async {
                    if let Err(e) = run_spotify_playback(
                        &uri_str,
                        &cred_path,
                        &event_tx,
                        writer,
                        &playback_stop,
                    )
                    .await
                    {
                        error!(error = %e, "spotify playback error");
                        let _ = event_tx
                            .send(Event::SourceError {
                                source: "spotify".into(),
                                error: e.to_string(),
                            })
                            .await;
                    }
                });
            });
        });
    }
}

/// Core async playback loop. Connects to Spotify, loads the track,
/// and bridges PCM from the sink receiver to the PcmWriter.
async fn run_spotify_playback(
    uri_str: &str,
    cred_path: &std::path::Path,
    event_tx: &tokio::sync::mpsc::Sender<Event>,
    writer: &mut dyn PcmWriter,
    stop: &AtomicBool,
) -> Result<()> {
    // 1. Authenticate (daemon-safe: refresh only, never interactive).
    //    Runs on a blocking thread to avoid starving the async runtime
    //    during the HTTP token-refresh round-trip.
    let cred_path_owned = cred_path.to_path_buf();
    let credentials = tokio::task::spawn_blocking(move || auth::load_credentials(&cred_path_owned))
        .await
        .context("credential task panicked")?
        .context("Spotify authentication failed")?;

    // 2. Connect session.
    let session_config = SessionConfig::default();
    let session = Session::new(session_config, None);
    session
        .connect(credentials, false)
        .await
        .map_err(|e| anyhow::anyhow!("Spotify session connect failed: {e}"))?;
    info!("spotify: session connected");

    // 3. Parse the track URI.
    let spotify_uri = SpotifyUri::from_uri(uri_str)
        .map_err(|e| anyhow::anyhow!("invalid Spotify URI '{uri_str}': {e}"))?;

    // 4. Create our custom sink with the PCM channel.
    let (sink, pcm_rx, pcm_notify) = sink::channel();

    // 5. Create the librespot Player with our sink as the backend.
    let player_config = PlayerConfig::default();
    let player = Player::new(
        player_config,
        session.clone(),
        Box::new(NoOpVolume),
        move || Box::new(sink),
    );

    // 6. Subscribe to player events.
    let mut player_events = player.get_player_event_channel();

    // 7. Load the track.
    player.load(spotify_uri, true, 0);
    info!(uri = %uri_str, "spotify: track loaded");

    // 8. Track last known position for resume after reconnect.
    let mut last_position_ms: u32 = 0;

    // 9. PCM drain + event monitoring loop.
    // `writer` isn't Send so we can't move it into a spawned task —
    // everything runs in the same task, woken by `tokio::select!`.
    loop {
        if stop.load(Ordering::Relaxed) {
            player.stop();
            break;
        }

        // Register the notify future BEFORE draining so we don't miss
        // notifications that fire between the last try_recv and select!.
        let notified = pcm_notify.notified();

        // Drain all available PCM frames (non-blocking).
        loop {
            match pcm_rx.try_recv() {
                Ok(frames) => {
                    writer.write(&frames);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    debug!("spotify: PCM channel disconnected");
                    break;
                }
            }
        }

        // Wait for the next wake-up: PCM data, player event, or stop check.
        tokio::select! {
            _ = notified => {
                // PCM data available — loop back to drain.
            }
            event = player_events.recv() => {
                match event {
                    Some(event) => {
                        handle_player_event(&event, event_tx, uri_str).await;
                        match event {
                            PlayerEvent::Playing { position_ms, .. } => {
                                last_position_ms = position_ms;
                            }
                            PlayerEvent::Paused { position_ms, .. } => {
                                last_position_ms = position_ms;
                            }
                            PlayerEvent::EndOfTrack { .. } => {
                                info!("spotify: end of track");
                                break;
                            }
                            PlayerEvent::Stopped { .. } => {
                                info!("spotify: player stopped");
                                break;
                            }
                            PlayerEvent::Unavailable { .. } => {
                                warn!("spotify: track unavailable");
                                let _ = event_tx
                                    .send(Event::SourceError {
                                        source: "spotify".into(),
                                        error: format!("track unavailable: {uri_str}"),
                                    })
                                    .await;
                                break;
                            }
                            PlayerEvent::SessionDisconnected { .. } => {
                                warn!("spotify: session disconnected, attempting reconnect");
                                if let Err(e) = attempt_reconnect(&session, cred_path).await {
                                    error!(error = %e, "spotify: reconnect failed");
                                    let _ = event_tx
                                        .send(Event::SourceError {
                                            source: "spotify".into(),
                                            error: format!("session lost: {e}"),
                                        })
                                        .await;
                                    break;
                                }
                                info!(
                                    position_ms = last_position_ms,
                                    "spotify: reconnected, resuming track"
                                );
                                match SpotifyUri::from_uri(uri_str) {
                                    Ok(uri) => player.load(uri, true, last_position_ms),
                                    Err(e) => {
                                        error!(error = %e, "spotify: failed to re-parse URI after reconnect");
                                        break;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    None => {
                        debug!("spotify: player event channel closed");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                // Periodic stop-flag check.
            }
        }
    }

    info!("spotify: playback ended");
    Ok(())
}

/// Map librespot PlayerEvents to daemon NowPlayingChanged events.
async fn handle_player_event(
    event: &PlayerEvent,
    event_tx: &tokio::sync::mpsc::Sender<Event>,
    uri: &str,
) {
    match event {
        PlayerEvent::TrackChanged { audio_item } => {
            let (artist, album) = match &audio_item.unique_fields {
                librespot_metadata::audio::UniqueFields::Track { artists, album, .. } => {
                    let artist_names: Vec<&str> =
                        artists.0.iter().map(|a| a.name.as_str()).collect();
                    (
                        Some(sanitize(&artist_names.join(", "))),
                        Some(sanitize(album)),
                    )
                }
                _ => (None, None),
            };

            let title = sanitize(&audio_item.name);

            let _ = event_tx
                .send(Event::NowPlayingChanged {
                    artist,
                    title: Some(title),
                    album,
                    station: None,
                    raw_stream_title: Some(sanitize(uri)),
                })
                .await;

            info!(
                title = %audio_item.name,
                duration_ms = audio_item.duration_ms,
                "spotify: now playing"
            );
        }
        PlayerEvent::Playing { position_ms, .. } => {
            debug!(position_ms, "spotify: playing");
        }
        PlayerEvent::Paused { position_ms, .. } => {
            debug!(position_ms, "spotify: paused");
        }
        _ => {}
    }
}

/// Attempt session reconnect with exponential backoff (1s, 2s, 4s).
async fn attempt_reconnect(session: &Session, cred_path: &std::path::Path) -> Result<()> {
    let delays = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
    ];

    for (i, delay) in delays.iter().enumerate() {
        info!(attempt = i + 1, "spotify: reconnect attempt");
        tokio::time::sleep(*delay).await;

        // Re-load credentials on a blocking thread (daemon-safe, no interactive auth).
        let cred_path_owned = cred_path.to_path_buf();
        let credentials =
            match tokio::task::spawn_blocking(move || auth::load_credentials(&cred_path_owned))
                .await
            {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    warn!(error = %e, "spotify: credential reload failed during reconnect");
                    continue;
                }
                Err(e) => {
                    warn!(error = %e, "spotify: credential task panicked during reconnect");
                    continue;
                }
            };

        match session.connect(credentials, false).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!(
                    attempt = i + 1,
                    error = %e,
                    "spotify: reconnect attempt failed"
                );
            }
        }
    }

    anyhow::bail!("reconnect failed after 3 attempts")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Source;

    #[test]
    fn source_name() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let source = SpotifySource::new(
            "spotify:track:test".into(),
            PathBuf::from("/tmp/test-creds.json"),
            tx,
        );
        assert_eq!(source.name(), "spotify");
    }

    #[test]
    fn spotify_uri_parse_valid() {
        let uri = SpotifyUri::from_uri("spotify:track:4PTG3Z6ehGkBFwjybzWkR8");
        assert!(uri.is_ok());
    }

    #[test]
    fn spotify_uri_parse_invalid() {
        let uri = SpotifyUri::from_uri("not-a-uri");
        assert!(uri.is_err());
    }

    #[tokio::test]
    async fn handle_player_event_playing_no_emit() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let event = PlayerEvent::Playing {
            play_request_id: 1,
            track_id: SpotifyUri::from_uri("spotify:track:4PTG3Z6ehGkBFwjybzWkR8").unwrap(),
            position_ms: 5000,
        };
        handle_player_event(&event, &tx, "spotify:track:4PTG3Z6ehGkBFwjybzWkR8").await;

        // Playing events are debug-logged but don't emit daemon events.
        assert!(rx.try_recv().is_err(), "Playing should not emit an event");
    }

    #[tokio::test]
    async fn handle_player_event_paused_no_emit() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let event = PlayerEvent::Paused {
            play_request_id: 1,
            track_id: SpotifyUri::from_uri("spotify:track:4PTG3Z6ehGkBFwjybzWkR8").unwrap(),
            position_ms: 10_000,
        };
        handle_player_event(&event, &tx, "spotify:track:4PTG3Z6ehGkBFwjybzWkR8").await;

        assert!(rx.try_recv().is_err(), "Paused should not emit an event");
    }

    #[test]
    fn sink_channel_produces_notify() {
        let (_sink, _rx, notify) = sink::channel();
        // Verify the notify is functional (doesn't panic).
        notify.notify_one();
    }

    #[tokio::test]
    async fn sink_notify_fires_on_pcm_send() {
        use librespot_playback::audio_backend::Sink;
        use librespot_playback::convert::Converter;
        use librespot_playback::decoder::AudioPacket;

        let (mut sink_inst, _rx, notify) = sink::channel();
        sink_inst.start().expect("start should succeed");

        // Send enough data to fill at least one chunk.
        let num_frames: usize = 2048;
        let samples: Vec<f64> = vec![0.0; num_frames * 2];
        let mut converter = Converter::new(None);
        sink_inst
            .write(AudioPacket::Samples(samples), &mut converter)
            .expect("write should succeed");

        // The notify should have been triggered when frames were sent.
        // Use a timeout to verify it resolves quickly.
        let result = tokio::time::timeout(Duration::from_millis(100), notify.notified()).await;
        assert!(result.is_ok(), "notify should fire after PCM send");
    }

    #[test]
    fn source_construction_preserves_fields() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let source = SpotifySource::new(
            "spotify:track:4PTG3Z6ehGkBFwjybzWkR8".into(),
            PathBuf::from("/home/user/.config/clitunes/spotify-creds.json"),
            tx,
        );
        assert_eq!(source.uri, "spotify:track:4PTG3Z6ehGkBFwjybzWkR8");
        assert_eq!(
            source.credentials_path,
            PathBuf::from("/home/user/.config/clitunes/spotify-creds.json")
        );
    }
}
