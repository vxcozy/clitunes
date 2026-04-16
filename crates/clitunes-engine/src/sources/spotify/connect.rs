//! Spotify Connect receiver wiring (v1.2 Unit 4).
//!
//! Two cooperating pieces:
//!
//! - [`ConnectRuntime`] is a daemon-lifetime task that owns
//!   `librespot_discovery::Discovery` and the active Session + Player +
//!   Spirc. It runs the canonical `tokio::select!` loop from
//!   `librespot/src/main.rs`: discovery yields credentials → shut down
//!   any old Spirc → **build a fresh unconnected Session + Player +
//!   sink** → call `Spirc::new`, which wires its dealer listeners and
//!   then connects the session itself. When credentials first arrive
//!   the runtime pushes `SourceCommand::PlayConnect` into the source
//!   pipeline.
//!
//!   Two constraints to keep straight:
//!
//!   - librespot-core's `Session::connect()` sets a `tx_connection`
//!     `OnceLock`, so a Session can be connected only once. Calling
//!     connect ourselves *and* letting Spirc connect would fail the
//!     second time with `SessionError::NotConnected`. `Spirc::new`
//!     owns the connect call, so we hand it an unconnected Session.
//!   - A Session whose Spirc has shut down isn't reusable as-is, so
//!     every Discovery event gets a fresh Session+Player+sink triple.
//!     This matches the reference librespot impl in `src/main.rs`.
//!
//! - [`ConnectSource`] is a `Source` impl. It owns no Spotify state of
//!   its own; it reads the current sink handle from [`ConnectSinkSlot`]
//!   (published by ConnectRuntime) and pumps PCM into the daemon's
//!   `PcmWriter` until the source pipeline tells it to stop.
//!
//! The two halves share one [`ConnectSinkSlot`] so ConnectSource can
//! always locate the current sink, even after ConnectRuntime rebuilds
//! the Session/Player on a fresh Discovery event.
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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use librespot_connect::{ConnectConfig as LibrespotConnectConfig, Spirc};
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_discovery::Discovery;
use librespot_playback::config::PlayerConfig;
use librespot_playback::mixer::softmixer::SoftMixer;
use librespot_playback::mixer::{Mixer, MixerConfig, NoOpVolume};
use librespot_playback::player::{Player, PlayerEvent};
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

use super::sink::{new_sink, SpotifySinkHandle};

/// Shared slot holding the currently-active Connect sink handle.
///
/// Cloned between [`ConnectRuntime`] (producer: replaces the handle on
/// every Discovery credential arrival) and [`ConnectSource`] (consumer:
/// reads the current handle on activation and binds a PCM receiver onto
/// it). `None` means ConnectRuntime hasn't built a Session yet, or has
/// torn the old one down ahead of building the replacement.
#[derive(Clone, Default)]
pub struct ConnectSinkSlot {
    inner: Arc<Mutex<Option<SpotifySinkHandle>>>,
}

impl ConnectSinkSlot {
    pub fn new() -> Self {
        Self::default()
    }

    fn set(&self, handle: SpotifySinkHandle) {
        *self.inner.lock().expect("connect sink slot poisoned") = Some(handle);
    }

    fn clear(&self) {
        *self.inner.lock().expect("connect sink slot poisoned") = None;
    }

    /// Snapshot the current sink handle. Returns `None` if ConnectRuntime
    /// hasn't published one yet.
    pub fn current(&self) -> Option<SpotifySinkHandle> {
        self.inner
            .lock()
            .expect("connect sink slot poisoned")
            .clone()
    }
}

/// `Source` impl active while the daemon is in `SourceKind::Connect`.
///
/// Reads the current sink handle from [`ConnectSinkSlot`] and binds a
/// PCM consumer onto it. If ConnectRuntime rebuilds the Session while
/// this source is running (new Discovery event), the slot's handle
/// rotates: the drain loop detects the new handle by `Arc` identity
/// (see [`SpotifySinkHandle::points_to_same_slot`]) and returns, so
/// the source pipeline re-enters `Connect` and builds a fresh
/// `ConnectSource` that binds to the new handle.
///
/// Identity check, not `Disconnected`: the old rx stays connected
/// because ConnectSource itself keeps a `SpotifySinkHandle` clone
/// alive through its RAII unbinder, which holds the `Arc<Mutex<…>>`
/// that owns the old tx. Waiting on `Disconnected` would hang.
pub struct ConnectSource {
    sink_slot: ConnectSinkSlot,
    target_sample_rate: u32,
}

impl ConnectSource {
    pub fn new(sink_slot: ConnectSinkSlot, target_sample_rate: u32) -> Self {
        Self {
            sink_slot,
            target_sample_rate,
        }
    }
}

impl Source for ConnectSource {
    fn name(&self) -> &str {
        "connect"
    }

    fn run(&mut self, writer: &mut dyn PcmWriter, stop: &AtomicBool) {
        // Mirror outer stop → inner stop, same as SpotifySource.
        let inner_stop = Arc::new(AtomicBool::new(false));
        let target_rate = self.target_sample_rate;
        let sink_slot = self.sink_slot.clone();

        std::thread::scope(|scope| {
            let mirror = Arc::clone(&inner_stop);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) && !mirror.load(Ordering::Relaxed) {
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
                    if let Err(e) = drain_pcm(&sink_slot, writer, &playback_stop, target_rate).await
                    {
                        error!(error = %e, "connect: PCM drain ended with error");
                    }
                });
                playback_stop.store(true, Ordering::SeqCst);
            });
        });
    }
}

/// Wait for ConnectRuntime to publish a sink, bind a PCM receiver onto
/// it, and drain frames into `writer` until `stop` fires or the slot
/// rotates (ConnectRuntime rebuilt the Session and published a new
/// handle).
///
/// ## Why rotation uses identity, not `Disconnected`
///
/// When ConnectRuntime rebuilds, the old Player and its sink drop, but
/// the tx inside the old `SinkBinding` lives inside `Arc<Mutex<…>>`
/// and this function keeps an `Arc` clone alive through its RAII
/// unbinder. So the old rx never sees `Disconnected` — we'd hang.
/// Instead we poll `sink_slot.current()` and compare by `Arc` pointer
/// identity; when it rotates to a new handle we return, the source
/// pipeline re-enters Connect, and a fresh `drain_pcm` binds to the
/// new handle.
///
/// Wall-clock pacing mirrors `run_spotify_playback`: librespot decodes
/// well ahead of realtime, so we sleep when the downstream ring is
/// `MAX_AHEAD` ahead of the wall clock. Without this the ring overruns
/// and the output sounds chopped.
async fn drain_pcm(
    sink_slot: &ConnectSinkSlot,
    writer: &mut dyn PcmWriter,
    stop: &AtomicBool,
    target_rate: u32,
) -> Result<()> {
    // Wait (briefly) for ConnectRuntime to publish a sink. PlayConnect is
    // only sent after the runtime builds the Session+Player+sink, so this
    // should return on the first iteration in practice — the poll is a
    // belt-and-braces guard for an unexpected scheduling reorder.
    let sink_handle = loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Some(h) = sink_slot.current() {
            break h;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let (pcm_rx, pcm_notify) = sink_handle.bind();
    info!("connect: source bound to runtime sink, pumping PCM");

    // RAII unbind so the sink stops routing to our channel if we bail
    // via an error or return early. Without this, a stale binding could
    // absorb frames meant for a later ConnectSource activation.
    struct Unbinder(SpotifySinkHandle);
    impl Drop for Unbinder {
        fn drop(&mut self) {
            self.0.unbind();
        }
    }
    let unbinder = Unbinder(sink_handle);

    const MAX_AHEAD: Duration = Duration::from_millis(400);
    const PACING_MARGIN: Duration = Duration::from_millis(150);
    let mut pace_start: Option<Instant> = None;
    let mut pace_frames: u64 = 0;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        // Detect sink rotation: ConnectRuntime may have dropped the old
        // Session and published a new handle. A rotated slot means the
        // old Player is gone and no more PCM will arrive on our rx —
        // return so the source pipeline can build a fresh ConnectSource
        // against the new sink. A `None` slot means the runtime is
        // mid-rebuild (cleared but not yet republished); keep looping
        // until it's set, then compare identity on the next iteration.
        if let Some(current) = sink_slot.current() {
            if !current.points_to_same_slot(&unbinder.0) {
                info!("connect: sink rotated by runtime, releasing source for rebuild");
                return Ok(());
            }
        }

        let notified = pcm_notify.notified();

        loop {
            match pcm_rx.try_recv() {
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
                    // Not expected during a runtime rebuild (see module
                    // doc), but still the right thing to do on shutdown
                    // paths where every clone has gone away.
                    debug!("connect: PCM channel fully disconnected, exiting drain");
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
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    info!("connect: source draining stopped");
    Ok(())
}

/// Daemon-lifetime task that owns Discovery + the active Session/Player/
/// Spirc and drives the canonical librespot connect loop. Spawned at
/// daemon boot when `config.connect.enabled = true`; survives
/// source-pipeline transitions so re-picking the device from the phone
/// reuses the existing mDNS advertisement.
pub struct ConnectRuntime {
    shutdown_tx: mpsc::UnboundedSender<()>,
    task: JoinHandle<()>,
}

impl ConnectRuntime {
    /// Build Discovery and spawn the runtime task on the daemon runtime.
    /// Returns immediately; the task runs until [`shutdown`](Self::shutdown).
    pub(crate) fn spawn(
        connect_config: ConnectConfig,
        sink_slot: ConnectSinkSlot,
        target_rate: u32,
        source_cmd_tx: std::sync::mpsc::Sender<crate::daemon::event_loop::SourceCommand>,
        event_tx: mpsc::Sender<Event>,
        runtime: RuntimeHandle,
    ) -> Result<Self> {
        let device_id = device_id_from_name(&connect_config.name);
        let client_id = super::auth::spotify_client_id();

        let mut discovery_builder = Discovery::builder(device_id.clone(), client_id)
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
            sink_slot,
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

/// Per-session state owned by the runtime loop. Held so the Player and
/// Session stay alive for as long as Spirc is driving them; dropped when
/// Discovery yields a fresh credential and we rebuild.
#[allow(dead_code)]
struct ActivePlayback {
    session: Session,
    player: Arc<Player>,
    sink_handle: SpotifySinkHandle,
}

// `current_playback` is a drop anchor — we hold Session+Player+sink
// alive between Discovery events and drop the old one before building
// the replacement. The binding is `#[allow(dead_code)]` on the struct;
// the assignment sites use `drop(current_playback.take())` to make the
// drop explicit at each rebuild point.
async fn run_connect_loop(
    mut discovery: Discovery,
    connect_config: ConnectConfig,
    sink_slot: ConnectSinkSlot,
    target_rate: u32,
    source_cmd_tx: std::sync::mpsc::Sender<crate::daemon::event_loop::SourceCommand>,
    event_tx: mpsc::Sender<Event>,
    mut shutdown_rx: mpsc::UnboundedReceiver<()>,
) {
    let device_id = device_id_from_name(&connect_config.name);
    let librespot_config = LibrespotConnectConfig {
        name: connect_config.name.clone(),
        initial_volume: scaled_volume(connect_config.initial_volume),
        ..LibrespotConnectConfig::default()
    };

    let mut current_spirc: Option<Spirc> = None;
    let mut current_task: Option<JoinHandle<()>> = None;
    let mut current_playback: Option<ActivePlayback> = None;
    let mut event_subscription: Option<librespot_playback::player::PlayerEventChannel> = None;
    let mut announced_connected = false;

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
                sink_slot.clear();
                break;
            }

            Some(credentials) = discovery.next() => {
                info!("connect: discovery yielded credentials, (re)starting spirc");

                // Tear down any previous spirc before building a fresh
                // Session. The old Player/Session/sink drop when we
                // replace `current_playback` below — that drop wakes any
                // ConnectSource blocked on the old PCM channel, which
                // exits cleanly and lets the source pipeline re-enter
                // Connect against the new sink.
                if let Some(spirc) = current_spirc.take() {
                    if let Err(e) = Spirc::shutdown(&spirc) {
                        warn!(error = %e, "connect: failed to shut down previous spirc");
                    }
                }
                if let Some(task) = current_task.take() {
                    let _ = task.await;
                }
                // Clear any lingering state from a prior failed build
                // (mixer or Spirc::new error on a previous iteration
                // left `event_subscription`/`sink_slot`/`current_playback`
                // pointing at orphaned resources). On the happy path
                // these were all dropped on the previous Spirc teardown.
                drop(event_subscription.take());
                sink_slot.clear();
                // Drop the old Session+Player+sink *before* building
                // replacements — otherwise two Sessions briefly compete
                // for the same Discovery credentials.
                drop(current_playback.take());

                // Fresh, unconnected Session — Spirc::new itself calls
                // session.connect() after wiring dealer listeners, and
                // the OnceLock inside Session means only one connect
                // succeeds. The device_id must match what Discovery
                // advertises or the backend can't correlate the two.
                let session = Session::new(
                    SessionConfig {
                        device_id: device_id.clone(),
                        ..SessionConfig::default()
                    },
                    None,
                );

                // Build a fresh sink + Player bound to this session. The
                // sink is single-use (`Player::new` consumes it via
                // `FnOnce`), so a new Session means a new Player means a
                // new sink — all three share a lifetime.
                let (sink, sink_handle) = new_sink(target_rate);
                let player = Player::new(
                    PlayerConfig::default(),
                    session.clone(),
                    Box::new(NoOpVolume),
                    move || Box::new(sink),
                );
                info!(target_rate, "connect: player + sink built");

                // Publish the new handle so ConnectSource (running in the
                // source pipeline) can bind a PCM receiver onto it.
                sink_slot.set(sink_handle.clone());

                event_subscription = Some(player.get_player_event_channel());

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
                    session.clone(),
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
                current_playback = Some(ActivePlayback {
                    session,
                    player,
                    sink_handle,
                });

                if !announced_connected {
                    announced_connected = true;
                    let _ = event_tx
                        .send(Event::ConnectDeviceConnected { remote_name: None })
                        .await;
                    // Ask the source pipeline to enter passive Connect
                    // mode. Best-effort: on subsequent rebuilds the
                    // pipeline is already in Connect, so we don't resend.
                    let _ = source_cmd_tx
                        .send(crate::daemon::event_loop::SourceCommand::PlayConnect);
                }
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
                drop(current_playback.take());
                event_subscription = None;
                sink_slot.clear();
                announced_connected = false;
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

/// Derive a stable device ID from the configured device name, matching
/// the reference librespot impl: SHA-1 of the name encoded as 40-char
/// lowercase hex. Must match between Discovery and SessionConfig or
/// the Spotify backend can't correlate the Zeroconf handshake with the
/// dealer registration.
fn device_id_from_name(name: &str) -> String {
    use sha1::Digest;
    let hash = sha1::Sha1::digest(name.as_bytes());
    hash.iter().fold(String::with_capacity(40), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
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
    fn device_id_is_stable_sha1_hex() {
        let id_a = device_id_from_name("Living Room");
        let id_b = device_id_from_name("Living Room");
        let id_c = device_id_from_name("Bedroom");
        assert_eq!(id_a, id_b);
        assert_ne!(id_a, id_c);
        assert_eq!(id_a.len(), 40, "device ID must be 40 hex chars (SHA-1)");
        assert!(id_a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn connect_source_name() {
        let slot = ConnectSinkSlot::new();
        let source = ConnectSource::new(slot, 48_000);
        assert_eq!(source.name(), "connect");
    }

    #[test]
    fn sink_slot_set_and_clear() {
        let slot = ConnectSinkSlot::new();
        assert!(slot.current().is_none(), "fresh slot is empty");

        let (_sink, handle) = new_sink(48_000);
        slot.set(handle);
        assert!(slot.current().is_some(), "set populates slot");

        slot.clear();
        assert!(slot.current().is_none(), "clear empties slot");
    }

    /// Test-only PcmWriter. Counts frames written so tests can assert on
    /// whether any PCM flowed through drain_pcm.
    struct CountingWriter(Arc<std::sync::atomic::AtomicUsize>);
    impl crate::audio::ring::PcmWriter for CountingWriter {
        fn write(&mut self, frames: &[clitunes_core::StereoFrame]) -> usize {
            self.0
                .fetch_add(frames.len(), std::sync::atomic::Ordering::SeqCst);
            frames.len()
        }
    }

    /// When ConnectRuntime rotates the slot to a fresh handle,
    /// `drain_pcm` must observe the rotation by Arc identity and
    /// return so the source pipeline can re-enter Connect. This is
    /// the regression guard for the bug where `drain_pcm` waited on
    /// `Disconnected` and hung — ConnectSource's own Unbinder keeps
    /// the old tx alive, so `Disconnected` never fires on rebuild.
    #[tokio::test]
    async fn drain_pcm_returns_when_slot_rotates() {
        let slot = ConnectSinkSlot::new();
        let (_sink_a, handle_a) = new_sink(48_000);
        slot.set(handle_a);

        let stop = AtomicBool::new(false);
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut writer = CountingWriter(Arc::clone(&writes));

        // Keep sink_b alive for the duration of the select so the
        // rotation's new handle points at a real sink.
        let (_sink_b, handle_b) = new_sink(48_000);
        let rotation_slot = slot.clone();

        let trigger = async move {
            // Let drain_pcm enter its loop and make at least one pass
            // against handle_a before we rotate.
            tokio::time::sleep(Duration::from_millis(80)).await;
            rotation_slot.set(handle_b);
            // Hang guard: if drain_pcm doesn't return, this wins the
            // select and panics with a clear message.
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            panic!("drain_pcm did not return within 1.5s of slot rotation");
        };

        tokio::select! {
            result = drain_pcm(&slot, &mut writer, &stop, 48_000) => {
                result.expect("drain_pcm should return Ok on rotation");
            }
            _ = trigger => unreachable!("trigger's panic branch fires only on hang"),
        }
    }

    /// `drain_pcm` should also return cleanly when the caller asserts
    /// `stop`. Covers the shutdown path that's distinct from the
    /// rotation path.
    #[tokio::test]
    async fn drain_pcm_returns_when_stop_asserted() {
        let slot = ConnectSinkSlot::new();
        let (_sink, handle) = new_sink(48_000);
        slot.set(handle);

        let stop = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut writer = CountingWriter(Arc::clone(&writes));

        let stop_trigger = Arc::clone(&stop);
        let trigger = async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            stop_trigger.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            panic!("drain_pcm did not return within 1.5s of stop");
        };

        tokio::select! {
            result = drain_pcm(&slot, &mut writer, &stop, 48_000) => {
                result.expect("drain_pcm should return Ok on stop");
            }
            _ = trigger => unreachable!("trigger's panic branch fires only on hang"),
        }
    }

    /// `drain_pcm`'s initial wait-for-publish loop must respect `stop`
    /// even when the slot is never populated — otherwise a misordered
    /// shutdown where Discovery never yielded credentials would hang
    /// the source thread.
    #[tokio::test]
    async fn drain_pcm_returns_from_initial_wait_on_stop() {
        let slot = ConnectSinkSlot::new(); // deliberately empty
        let stop = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut writer = CountingWriter(Arc::clone(&writes));

        let stop_trigger = Arc::clone(&stop);
        let trigger = async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            stop_trigger.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            panic!("drain_pcm did not exit initial wait loop within 1.5s");
        };

        tokio::select! {
            result = drain_pcm(&slot, &mut writer, &stop, 48_000) => {
                result.expect("drain_pcm should return Ok on stop");
            }
            _ = trigger => unreachable!("trigger's panic branch fires only on hang"),
        }
    }
}
