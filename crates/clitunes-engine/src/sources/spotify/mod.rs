//! Spotify playback via librespot.
//!
//! Provides [`SpotifySource`] — an implementation of the [`Source`](super::Source)
//! trait that drives a per-track playback loop against the daemon's shared
//! [`SpotifyHandle`]. The handle owns a singleton `Player` + `Session` plus
//! a rebindable sink (see the [`handle`] and [`sink`] modules); this source
//! acquires a [`PlaybackGuard`] per track, loads the URI, pumps PCM from
//! the sink into the daemon's ring at the probed device sample rate, and
//! relies on the guard's `Drop` to tear the binding back down cleanly.

pub mod auth;
#[cfg(feature = "connect")]
pub mod connect;
pub mod handle;
pub mod sink;

pub use auth::{
    authenticate_from_daemon, cached_auth_status, default_credentials_path, load_credentials,
    load_or_authenticate, AuthResult, AuthStatus,
};
#[cfg(feature = "connect")]
pub use connect::{ConnectRuntime, ConnectSinkSlot, ConnectSource};
pub use handle::{PlaybackGuard, SpotifyHandle};
#[cfg(feature = "webapi")]
pub mod token;
#[cfg(feature = "webapi")]
pub mod webapi;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Drop guard that sets an [`AtomicBool`] to `true` on drop — including
/// during panic unwind. Used in source-runner playback threads so
/// `inner_stop` is always signalled, preventing the mirror thread from
/// spinning forever if the playback thread panics.
pub(crate) struct StopGuard(Arc<AtomicBool>);

impl Drop for StopGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

use anyhow::Result;
use librespot_core::spotify_uri::SpotifyUri;
use librespot_playback::player::PlayerEvent;
use tokio::runtime::Builder;
use tracing::{debug, error, info, warn};

use crate::audio::ring::PcmWriter;
use crate::proto::events::Event;

use clitunes_core::sanitize;

/// A Spotify playback source. Binds a fresh PCM channel to the daemon's
/// shared [`SpotifySink`](sink::SpotifySink) per track and routes resampled
/// frames into the PCM ring at the probed device rate.
pub struct SpotifySource {
    /// Spotify track URI to play (e.g. `spotify:track:4PTG3Z6ehGkBFwjybzWkR8`).
    uri: String,
    /// Shared session + auth state. Built once per daemon, shared with the
    /// Web API cache so both paths go through a single `load_credentials`
    /// call and don't race on the on-disk refresh_token rotation.
    handle: Arc<SpotifyHandle>,
    /// Channel to emit events (NowPlayingChanged, SourceError) to the daemon event loop.
    event_tx: tokio::sync::mpsc::Sender<Event>,
    /// Target sample rate the sink should resample Spotify's 44.1 kHz
    /// PCM to. Must match the daemon's PCM ring / audio device rate —
    /// on 44.1 kHz hardware this is a 1:1 identity pass; on 48 kHz
    /// hardware the sink upsamples.
    target_sample_rate: u32,
}

impl SpotifySource {
    /// Create a new SpotifySource. Does not start playback; call `run()` for that.
    ///
    /// - `uri`: Spotify URI (e.g. `spotify:track:...`)
    /// - `handle`: shared Spotify handle owning the daemon's Session + auth cache
    /// - `event_tx`: sender for daemon events (NowPlaying updates, errors)
    /// - `target_sample_rate`: PCM ring / audio-device sample rate; the sink
    ///   resamples librespot's 44.1 kHz output to this rate. Mismatch here
    ///   causes a pitch-shift regression on non-48 kHz hardware.
    pub fn new(
        uri: String,
        handle: Arc<SpotifyHandle>,
        event_tx: tokio::sync::mpsc::Sender<Event>,
        target_sample_rate: u32,
    ) -> Self {
        Self {
            uri,
            handle,
            event_tx,
            target_sample_rate,
        }
    }

    /// Resampler target rate this source will hand to the sink.
    ///
    /// Exposed to the crate so the daemon's wiring test can pin that
    /// `run_source_pipeline` threads the probed device rate through —
    /// the sink's own identity-pass test can't catch a caller-side
    /// regression that hardcodes 48 kHz.
    #[cfg(test)]
    pub(crate) fn target_sample_rate(&self) -> u32 {
        self.target_sample_rate
    }
}

impl super::Source for SpotifySource {
    fn name(&self) -> &str {
        "spotify"
    }

    fn run(&mut self, writer: &mut dyn PcmWriter, stop: &AtomicBool) {
        let uri_str = self.uri.clone();
        let handle = Arc::clone(&self.handle);
        let event_tx = self.event_tx.clone();
        let target_sample_rate = self.target_sample_rate;

        // Mirror outer stop → inner stop (same pattern as RadioSource).
        let inner_stop = Arc::new(AtomicBool::new(false));

        std::thread::scope(|scope| {
            // Watcher thread: polls outer stop every 50ms.
            let mirror = Arc::clone(&inner_stop);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) && !mirror.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                mirror.store(true, Ordering::SeqCst);
            });

            // Main playback thread with its own tokio runtime.
            let playback_stop = Arc::clone(&inner_stop);
            scope.spawn(move || {
                let _guard = StopGuard(Arc::clone(&playback_stop));
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
                        &handle,
                        &event_tx,
                        writer,
                        &playback_stop,
                        target_sample_rate,
                    )
                    .await
                    {
                        error!(error = %e, "spotify playback error");
                        let err_code = if e.to_string().contains("premium_required") {
                            Some("premium_required".into())
                        } else {
                            None
                        };
                        let _ = event_tx
                            .send(Event::SourceError {
                                source: "spotify".into(),
                                error: e.to_string(),
                                error_code: err_code,
                            })
                            .await;
                    }
                });
            });
        });
    }
}

/// Core async playback loop. Acquires a [`PlaybackGuard`] on the
/// daemon-shared Player, loads the track, and bridges PCM from the
/// sink receiver to the PcmWriter. The guard's Drop tears down the
/// sink binding and stops the Player cleanly when this function
/// returns — see [`handle`] module docs for the shared-singleton
/// rationale.
async fn run_spotify_playback(
    uri_str: &str,
    handle: &Arc<SpotifyHandle>,
    event_tx: &tokio::sync::mpsc::Sender<Event>,
    writer: &mut dyn PcmWriter,
    stop: &AtomicBool,
    target_sample_rate: u32,
) -> Result<()> {
    // 1. Acquire the shared Player + Session and bind a fresh PCM
    //    channel onto the shared sink. Lazy-init on first call;
    //    reused across tracks afterwards.
    let mut guard = handle.start_playback(target_sample_rate).await?;

    // 2. Parse the track URI.
    let spotify_uri = SpotifyUri::from_uri(uri_str)
        .map_err(|e| anyhow::anyhow!("invalid Spotify URI '{uri_str}': {e}"))?;

    // 3. Subscribe to player events (fresh receiver from the guard
    //    so no state bleeds across tracks) and kick off the load.
    let mut player_events = guard
        .take_player_events()
        .expect("fresh guard always has a player-event receiver");
    guard.player().load(spotify_uri, true, 0);
    info!(uri = %uri_str, "spotify: track loaded");

    // 4. Track last known position for resume after reconnect.
    let mut last_position_ms: u32 = 0;

    // Wall-clock pacing state. librespot downloads the encrypted track in
    // one burst and then decodes at ~40× realtime, flooding the SyncSender
    // and ring with no natural backpressure (the downstream writer drops
    // oldest on overrun — see `PcmRingWriter::write`). Without pacing the
    // cpal callback ends up reading chopped-up fragments and the output
    // sounds like heavily corrupted audio. Matching ToneSource's pattern,
    // the source self-paces: after each drain we sleep until the wall
    // clock catches up with the frames we've written, capped so the ring
    // never holds more than `MAX_AHEAD` of lookahead.
    //
    // Reset on pause/idle so resume doesn't burst — otherwise the pacing
    // clock drifts forward during the gap and we'd immediately write a
    // second's worth of frames on the next Playing event.
    const MAX_AHEAD: Duration = Duration::from_millis(400);
    const PACING_MARGIN: Duration = Duration::from_millis(150);
    let mut pace_start: Option<Instant> = None;
    let mut pace_frames: u64 = 0;

    // 5. PCM drain + event monitoring loop.
    // `writer` isn't Send so we can't move it into a spawned task —
    // everything runs in the same task, woken by `tokio::select!`.
    loop {
        if stop.load(Ordering::Relaxed) {
            guard.player().stop();
            break;
        }

        // Register the notify future BEFORE draining so we don't miss
        // notifications that fire between the last try_recv and select!.
        let notified = guard.pcm_notify().notified();

        // Drain all available PCM frames (non-blocking).
        loop {
            match guard.pcm_rx().try_recv() {
                Ok(frames) => {
                    if pace_start.is_none() {
                        pace_start = Some(Instant::now());
                        pace_frames = 0;
                    }
                    pace_frames += frames.len() as u64;
                    writer.write(&frames);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    debug!("spotify: PCM channel disconnected");
                    break;
                }
            }
        }

        // Wall-clock pace: if we've buffered more than MAX_AHEAD, sleep
        // until we're back within PACING_MARGIN. This yields, so librespot
        // can keep decoding into the (now-stalled) SyncSender — which then
        // blocks librespot's own sink.write once full, providing the
        // backpressure we actually need.
        if let Some(start) = pace_start {
            let played =
                Duration::from_secs_f64(pace_frames as f64 / f64::from(target_sample_rate));
            let real = start.elapsed();
            if played > real + MAX_AHEAD {
                let sleep_for = played - real - PACING_MARGIN;
                tokio::time::sleep(sleep_for).await;
                continue;
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
                                pace_start = None;
                                pace_frames = 0;
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
                                let catalogue = guard
                                    .session()
                                    .user_data()
                                    .attributes
                                    .get("type")
                                    .cloned()
                                    .unwrap_or_default();
                                let (error, error_code) = if catalogue != "premium" {
                                    warn!("spotify: track unavailable (non-premium account)");
                                    (
                                        "Spotify Premium is required for playback. \
                                         Visit spotify.com/premium to upgrade."
                                            .to_string(),
                                        Some("premium_required".into()),
                                    )
                                } else {
                                    warn!("spotify: track unavailable");
                                    (
                                        format!("track unavailable: {uri_str}"),
                                        None,
                                    )
                                };
                                let _ = event_tx
                                    .send(Event::SourceError {
                                        source: "spotify".into(),
                                        error,
                                        error_code,
                                    })
                                    .await;
                                break;
                            }
                            PlayerEvent::SessionDisconnected { .. } => {
                                warn!("spotify: session disconnected, attempting reconnect");
                                pace_start = None;
                                pace_frames = 0;
                                if let Err(e) = handle.reconnect().await {
                                    error!(error = %e, "spotify: reconnect failed");
                                    let _ = event_tx
                                        .send(Event::SourceError {
                                            source: "spotify".into(),
                                            error: format!("session lost: {e}"),
                                            error_code: None,
                                        })
                                        .await;
                                    break;
                                }
                                info!(
                                    position_ms = last_position_ms,
                                    "spotify: reconnected, resuming track"
                                );
                                match SpotifyUri::from_uri(uri_str) {
                                    Ok(uri) => guard.player().load(uri, true, last_position_ms),
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

    // `guard` drops here — see `PlaybackGuard::drop`. It calls
    // `player.stop()` (so the decoder goes idle for the next track),
    // unbinds the sink (so any in-flight resample gets discarded
    // silently), then the `pcm_rx` field drops and wakes any
    // `SyncSender::send` still parked in the sink with `Disconnected`.
    // This is the PR #29 shutdown-deadlock fix, now lifted into the
    // guard so every playback gets it automatically.
    drop(guard);

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

            // Covers are pre-sorted largest-first by librespot; pick the
            // first URL if present. `CoverImage.url` is already a fully-formed
            // CDN URL, so no additional processing is needed.
            let art_url = audio_item.covers.first().map(|c| sanitize(&c.url));

            let _ = event_tx
                .send(Event::NowPlayingChanged {
                    artist,
                    title: Some(title),
                    album,
                    station: None,
                    raw_stream_title: Some(sanitize(uri)),
                    art_url,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Source;
    use std::path::PathBuf;

    #[tokio::test]
    async fn source_name() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let handle = Arc::new(SpotifyHandle::new(
            PathBuf::from("/tmp/test-creds.json"),
            tokio::runtime::Handle::current(),
        ));
        let source = SpotifySource::new("spotify:track:test".into(), handle, tx, 48_000);
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
    fn sink_bind_produces_notify() {
        let (_sink, handle) = sink::new_sink(48_000);
        let (_rx, notify) = handle.bind();
        // Verify the notify is functional (doesn't panic).
        notify.notify_one();
    }

    #[tokio::test]
    async fn sink_notify_fires_on_pcm_send() {
        use librespot_playback::audio_backend::Sink;
        use librespot_playback::convert::Converter;
        use librespot_playback::decoder::AudioPacket;

        let (mut sink_inst, sink_handle) = sink::new_sink(48_000);
        let (_rx, notify) = sink_handle.bind();
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

    #[tokio::test]
    async fn source_construction_preserves_fields() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let cred_path = PathBuf::from("/home/user/.config/clitunes/spotify-creds.json");
        let handle = Arc::new(SpotifyHandle::new(
            cred_path.clone(),
            tokio::runtime::Handle::current(),
        ));
        let source = SpotifySource::new(
            "spotify:track:4PTG3Z6ehGkBFwjybzWkR8".into(),
            Arc::clone(&handle),
            tx,
            48_000,
        );
        assert_eq!(source.uri, "spotify:track:4PTG3Z6ehGkBFwjybzWkR8");
        assert_eq!(source.handle.cred_path(), cred_path);
        assert_eq!(source.target_sample_rate, 48_000);
    }
}
