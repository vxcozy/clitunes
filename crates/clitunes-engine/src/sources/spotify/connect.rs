//! Spotify Connect receiver wiring (v1.2 Unit 4).
//!
//! Two cooperating pieces:
//!
//! - [`ConnectRuntime`] is a daemon-lifetime task that owns
//!   `librespot_discovery::Discovery` and the active `Spirc`. It runs
//!   the canonical `tokio::select!` loop from `librespot/src/main.rs`:
//!   discovery yields credentials → shut down any old Spirc → ensure
//!   Player+Session singletons are up → rebind session credentials →
//!   spawn a fresh Spirc task. When credentials first arrive it pushes
//!   `SourceCommand::PlayConnect` into the source pipeline.
//!
//! - [`ConnectSource`] is a `Source` impl. It owns no Spotify state of
//!   its own; it simply binds a fresh PCM channel onto the daemon's
//!   shared sink (via `SpotifyHandle::start_playback`) and pumps frames
//!   into the daemon's `PcmWriter` until the source pipeline tells it
//!   to stop. Spirc — running in `ConnectRuntime` — drives `Player`
//!   independently (load track, play, pause, seek), and the resulting
//!   PCM lands in the same shared sink that `ConnectSource` is bound to.
//!
//! The two halves are deliberately decoupled: ConnectRuntime can outlive
//! any number of ConnectSource activations (e.g. if a local verb
//! interrupts and the user later picks the device again from their
//! phone, the same Spirc task picks up where it left off without
//! re-advertising on mDNS).
//!
//! ## Why ConnectSource doesn't subscribe to player events
//!
//! `librespot_playback::Player::get_player_event_channel` returns a
//! fresh receiver each call, so multiple subscribers do work — but
//! responsibility belongs in one place. ConnectRuntime is the
//! authoritative subscriber and bridges `TrackChanged` /
//! `Playing` / `Paused` to the daemon's `NowPlayingChanged` /
//! `StateChanged` events. ConnectSource is intentionally dumb: it
//! transports PCM and nothing else.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use librespot_connect::{ConnectConfig as LibrespotConnectConfig, Spirc};
use librespot_discovery::Discovery;
use librespot_playback::mixer::softmixer::SoftMixer;
use librespot_playback::mixer::{Mixer, MixerConfig};
use librespot_playback::player::PlayerEvent;
use tokio::runtime::{Builder, Handle as RuntimeHandle};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use futures_util::StreamExt;

use clitunes_core::sanitize;

use crate::audio::ring::PcmWriter;
use crate::daemon::config::{BindMode, ConnectConfig};
use crate::proto::events::Event;
use crate::sources::Source;

use super::handle::SpotifyHandle;

/// `Source` impl active while the daemon is in `SourceKind::Connect`.
///
/// Binds a PCM channel onto the shared Spotify sink and forwards frames
/// to the daemon's writer until the pipeline interrupts. Owns no
/// Spirc/discovery state — that all lives in [`ConnectRuntime`].
pub struct ConnectSource {
    handle: Arc<SpotifyHandle>,
    target_sample_rate: u32,
}

impl ConnectSource {
    pub fn new(handle: Arc<SpotifyHandle>, target_sample_rate: u32) -> Self {
        Self {
            handle,
            target_sample_rate,
        }
    }
}

impl Source for ConnectSource {
    fn name(&self) -> &str {
        "connect"
    }

    fn run(&mut self, writer: &mut dyn PcmWriter, stop: &AtomicBool) {
        // Mirror outer stop → inner stop, same as SpotifySource. Lets the
        // playback thread observe an `AtomicBool` it owns rather than
        // borrowing the pipeline's stop flag across thread boundaries.
        let inner_stop = Arc::new(AtomicBool::new(false));
        let handle = Arc::clone(&self.handle);
        let target_rate = self.target_sample_rate;

        std::thread::scope(|scope| {
            let mirror = Arc::clone(&inner_stop);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                mirror.store(true, Ordering::SeqCst);
            });

            let playback_stop = Arc::clone(&inner_stop);
            scope.spawn(move || {
                let rt = match Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        error!(error = %e, "connect: failed to build tokio runtime");
                        return;
                    }
                };
                rt.block_on(async {
                    if let Err(e) = drain_pcm(&handle, writer, &playback_stop, target_rate).await {
                        error!(error = %e, "connect: PCM drain ended with error");
                    }
                });
            });
        });
    }
}

/// Bind the shared sink and push frames into `writer` until `stop`.
///
/// Mirrors the wall-clock pacing pattern from `run_spotify_playback`:
/// librespot decodes well ahead of realtime, so we sleep when the
/// downstream ring is `MAX_AHEAD` ahead of the wall clock to give cpal
/// time to consume. Without this the ring overruns and the output
/// sounds chopped.
async fn drain_pcm(
    handle: &Arc<SpotifyHandle>,
    writer: &mut dyn PcmWriter,
    stop: &AtomicBool,
    target_rate: u32,
) -> Result<()> {
    let guard = handle.start_playback(target_rate).await?;
    info!("connect: source bound to shared sink, pumping PCM");

    const MAX_AHEAD: Duration = Duration::from_millis(400);
    const PACING_MARGIN: Duration = Duration::from_millis(150);
    let mut pace_start: Option<Instant> = None;
    let mut pace_frames: u64 = 0;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        let notified = guard.pcm_notify().notified();

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
                    debug!("connect: PCM channel disconnected");
                    return Ok(());
                }
            }
        }

        if let Some(start) = pace_start {
            let played = Duration::from_secs_f64(pace_frames as f64 / f64::from(target_rate));
            let real = start.elapsed();
            if played > real + MAX_AHEAD {
                let sleep_for = played - real - PACING_MARGIN;
                tokio::time::sleep(sleep_for).await;
                continue;
            }
        }

        tokio::select! {
            _ = notified => {}
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                // Periodic stop-flag check, also covers the silent-gap
                // case between Spirc tracks where no PCM is flowing.
            }
        }
    }

    info!("connect: source draining stopped");
    Ok(())
}

/// Daemon-lifetime task that owns Discovery + the active Spirc and
/// drives the canonical librespot connect loop. Spawned at daemon boot
/// when `config.connect.enabled = true`; survives source-pipeline
/// transitions so re-picking the device from the phone reuses the
/// existing mDNS advertisement.
pub struct ConnectRuntime {
    shutdown_tx: mpsc::UnboundedSender<()>,
    task: JoinHandle<()>,
}

impl ConnectRuntime {
    /// Build Discovery and spawn the runtime task on the daemon runtime.
    /// Returns immediately; the task runs until [`shutdown`](Self::shutdown).
    pub(crate) fn spawn(
        connect_config: ConnectConfig,
        handle: Arc<SpotifyHandle>,
        target_rate: u32,
        source_cmd_tx: std::sync::mpsc::Sender<crate::daemon::event_loop::SourceCommand>,
        event_tx: mpsc::Sender<Event>,
        runtime: RuntimeHandle,
    ) -> Result<Self> {
        let device_id = device_id_from_name(&connect_config.name);
        let client_id = super::auth::spotify_client_id();

        let mut discovery_builder = Discovery::builder(device_id, client_id)
            .name(connect_config.name.clone())
            .port(connect_config.port);

        if let BindMode::Loopback = connect_config.bind {
            // Restrict mDNS responder to localhost. Useful for SSH-tunnel
            // setups where the user wants Connect handshake to traverse
            // a forwarded port instead of the LAN broadcast domain.
            discovery_builder = discovery_builder
                .zeroconf_ip(vec![std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)]);
        }

        let discovery = discovery_builder
            .launch()
            .map_err(|e| anyhow::anyhow!("connect: discovery launch failed: {e}"))?;

        info!(
            name = %connect_config.name,
            port = connect_config.port,
            bind = ?connect_config.bind,
            "connect: discovery advertising"
        );

        let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();
        let task = runtime.spawn(run_connect_loop(
            discovery,
            connect_config,
            handle,
            target_rate,
            source_cmd_tx,
            event_tx,
            shutdown_rx,
        ));

        Ok(Self { shutdown_tx, task })
    }

    /// Signal the runtime to stop and await the task. Idempotent — a
    /// second call is a no-op once the task has finished.
    pub async fn shutdown(self) -> Result<()> {
        let _ = self.shutdown_tx.send(());
        match self.task.await {
            Ok(()) => Ok(()),
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(anyhow::anyhow!("connect: runtime task panicked: {e}")),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_connect_loop(
    mut discovery: Discovery,
    connect_config: ConnectConfig,
    handle: Arc<SpotifyHandle>,
    target_rate: u32,
    source_cmd_tx: std::sync::mpsc::Sender<crate::daemon::event_loop::SourceCommand>,
    event_tx: mpsc::Sender<Event>,
    mut shutdown_rx: mpsc::UnboundedReceiver<()>,
) {
    let librespot_config = LibrespotConnectConfig {
        name: connect_config.name.clone(),
        initial_volume: scaled_volume(connect_config.initial_volume),
        ..LibrespotConnectConfig::default()
    };

    let mut current_spirc: Option<Spirc> = None;
    let mut current_task: Option<JoinHandle<()>> = None;
    let mut event_subscription: Option<librespot_playback::player::PlayerEventChannel> = None;

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("connect: shutdown signal received");
                if let Some(spirc) = current_spirc.take() {
                    if let Err(e) = Spirc::shutdown(&spirc) {
                        warn!(error = %e, "connect: spirc shutdown error");
                    }
                }
                if let Some(task) = current_task.take() {
                    let _ = task.await;
                }
                break;
            }

            Some(credentials) = discovery.next() => {
                info!("connect: discovery yielded credentials, (re)starting spirc");

                if let Some(spirc) = current_spirc.take() {
                    if let Err(e) = Spirc::shutdown(&spirc) {
                        warn!(error = %e, "connect: failed to shut down previous spirc");
                    }
                }
                if let Some(task) = current_task.take() {
                    let _ = task.await;
                }

                let (player, session) = match handle
                    .ensure_player_and_session(target_rate)
                    .await
                {
                    Ok(pair) => pair,
                    Err(e) => {
                        error!(error = %e, "connect: failed to initialise player+session");
                        let _ = event_tx
                            .send(Event::SourceError {
                                source: "connect".into(),
                                error: format!("connect init failed: {e}"),
                                error_code: None,
                            })
                            .await;
                        continue;
                    }
                };

                if let Err(e) = session.connect(credentials.clone(), true).await {
                    error!(error = %e, "connect: session.connect failed");
                    let _ = event_tx
                        .send(Event::SourceError {
                            source: "connect".into(),
                            error: format!("session connect failed: {e}"),
                            error_code: None,
                        })
                        .await;
                    continue;
                }

                // Subscribe to player events *before* Spirc::new so we
                // never miss the initial TrackChanged that follows a
                // phone client picking us. Only one subscription needs
                // to live across Spirc rebuilds — the underlying
                // Player is the daemon's singleton.
                if event_subscription.is_none() {
                    event_subscription = Some(player.get_player_event_channel());
                }

                let mixer: Arc<dyn Mixer> = match SoftMixer::open(MixerConfig::default()) {
                    Ok(m) => Arc::new(m),
                    Err(e) => {
                        error!(error = %e, "connect: SoftMixer::open failed");
                        let _ = event_tx
                            .send(Event::SourceError {
                                source: "connect".into(),
                                error: format!("mixer init failed: {e}"),
                                error_code: None,
                            })
                            .await;
                        continue;
                    }
                };
                let new_spirc = Spirc::new(
                    librespot_config.clone(),
                    session,
                    credentials,
                    Arc::clone(&player),
                    mixer,
                )
                .await;

                let (spirc, spirc_task) = match new_spirc {
                    Ok(pair) => pair,
                    Err(e) => {
                        error!(error = %e, "connect: Spirc::new failed");
                        let _ = event_tx
                            .send(Event::SourceError {
                                source: "connect".into(),
                                error: format!("spirc init failed: {e}"),
                                error_code: None,
                            })
                            .await;
                        continue;
                    }
                };

                current_spirc = Some(spirc);
                current_task = Some(tokio::spawn(spirc_task));

                let _ = event_tx
                    .send(Event::ConnectDeviceConnected { remote_name: None })
                    .await;

                // Tell the source pipeline to enter passive Connect mode
                // if it isn't already. Best-effort: if the channel is
                // closed the daemon is shutting down anyway.
                let _ = source_cmd_tx.send(
                    crate::daemon::event_loop::SourceCommand::PlayConnect,
                );
            }

            // Bridge librespot player events to daemon NowPlayingChanged
            // while a Spirc is running.
            Some(event) = next_player_event(&mut event_subscription) => {
                bridge_player_event(&event, &event_tx).await;
            }

            // Detect when the active Spirc task ends so we can emit
            // ConnectDeviceDisconnected without polling.
            res = wait_task(&mut current_task), if current_task.is_some() => {
                if let Err(e) = res {
                    if !e.is_cancelled() {
                        warn!(error = %e, "connect: spirc task ended with error");
                    }
                }
                current_task = None;
                current_spirc = None;
                let _ = event_tx
                    .send(Event::ConnectDeviceDisconnected)
                    .await;
                info!("connect: spirc task ended, awaiting next discovery");
            }
        }
    }

    info!("connect: runtime loop exited");
}

/// Pull the next player event, returning `None` when the subscription
/// is absent or the channel has closed. Used inside the runtime's
/// `tokio::select!` to participate cleanly when no Spirc is active.
async fn next_player_event(
    subscription: &mut Option<librespot_playback::player::PlayerEventChannel>,
) -> Option<PlayerEvent> {
    match subscription {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Await the active spirc task to completion. Used by the runtime's
/// `tokio::select!` only when `current_task.is_some()` — see the
/// guard expression on the corresponding arm.
async fn wait_task(
    task: &mut Option<JoinHandle<()>>,
) -> std::result::Result<(), tokio::task::JoinError> {
    match task {
        Some(handle) => handle.await,
        // Unreachable in practice — the select arm guards on `is_some()`.
        None => std::future::pending().await,
    }
}

/// Translate the librespot events ConnectRuntime cares about into the
/// daemon's `NowPlayingChanged` events. Skips Spirc-internal events the
/// daemon doesn't surface (TimeToPreloadNextTrack, AudioQuality, etc.).
async fn bridge_player_event(event: &PlayerEvent, event_tx: &mpsc::Sender<Event>) {
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
            let art_url = audio_item.covers.first().map(|c| sanitize(&c.url));
            let _ = event_tx
                .send(Event::NowPlayingChanged {
                    artist,
                    title: Some(title),
                    album,
                    station: None,
                    raw_stream_title: Some(sanitize(&audio_item.uri)),
                    art_url,
                })
                .await;
            info!(
                title = %audio_item.name,
                duration_ms = audio_item.duration_ms,
                "connect: now playing"
            );
        }
        PlayerEvent::Playing { position_ms, .. } => {
            debug!(position_ms, "connect: playing");
        }
        PlayerEvent::Paused { position_ms, .. } => {
            debug!(position_ms, "connect: paused");
        }
        _ => {}
    }
}

/// Map the daemon's `0..=100` initial-volume config to the `u16` range
/// librespot's `ConnectConfig` expects (`0..=u16::MAX`). The daemon's
/// loader has already validated the input range — see
/// `DaemonConfig::validate`.
fn scaled_volume(initial: u8) -> u16 {
    ((u32::from(initial) * u32::from(u16::MAX)) / 100) as u16
}

/// Derive a stable device ID from the configured device name. The
/// Spotify Connect protocol identifies devices by an opaque ID; using
/// a hash of the user's chosen name keeps it stable across daemon
/// restarts without persisting anything.
fn device_id_from_name(name: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_volume_endpoints() {
        assert_eq!(scaled_volume(0), 0);
        assert_eq!(scaled_volume(100), u16::MAX);
        assert!(scaled_volume(50) > 32_000 && scaled_volume(50) < 33_000);
    }

    #[test]
    fn device_id_is_stable_for_same_name() {
        let id_a = device_id_from_name("Living Room");
        let id_b = device_id_from_name("Living Room");
        let id_c = device_id_from_name("Bedroom");
        assert_eq!(id_a, id_b);
        assert_ne!(id_a, id_c);
        assert_eq!(id_a.len(), 16, "device ID must be 16 hex chars");
    }

    #[test]
    fn connect_source_name() {
        let handle = Arc::new(SpotifyHandle::new(
            std::path::PathBuf::from("/tmp/clitunes-test-creds.json"),
            tokio::runtime::Handle::try_current().unwrap_or_else(|_| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let h = rt.handle().clone();
                std::mem::forget(rt);
                h
            }),
        ));
        let source = ConnectSource::new(handle, 48_000);
        assert_eq!(source.name(), "connect");
    }
}
