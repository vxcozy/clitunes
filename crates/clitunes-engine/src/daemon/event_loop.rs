use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clitunes_core::{PcmFormat, Station};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::audio::{CpalOutput, CpalOutputConfig, PcmRing};
use crate::pcm::cross_process_api::{PcmBridge, DEFAULT_CAPACITY};
use crate::pcm::spmc_ring::ShmRegion;
use crate::proto::events::{Event, PlayState};
use crate::proto::server::{ControlServer, VerbReceiver};
use crate::proto::verbs::{SourceArg, Verb};
#[cfg(feature = "local")]
use crate::sources::local::LocalSource;
use crate::sources::radio::{RadioConfig, RadioSource};
#[cfg(feature = "spotify")]
use crate::sources::spotify::SpotifySource;
use crate::sources::tone_source::ToneSource;
use crate::sources::Source;

use super::config::DaemonConfig;
use super::tee_writer::TeeWriter;
use super::IdleTimer;

const TONE_BLOCK: usize = 1024;

pub struct DaemonEventLoop {
    socket_path: std::path::PathBuf,
    idle: Arc<IdleTimer>,
    stop: Arc<AtomicBool>,
    /// Daemon config consumed by the Connect receiver and exposed to
    /// clients via `Verb::ReadConfig`.
    config: DaemonConfig,
    /// Resolved `daemon.toml` path the daemon loaded from, if any.
    /// Surfaced to TUI clients so users can see where to edit config.
    config_path: Option<std::path::PathBuf>,
}

impl DaemonEventLoop {
    pub fn new(
        socket_path: std::path::PathBuf,
        idle: Arc<IdleTimer>,
        stop: Arc<AtomicBool>,
        config: DaemonConfig,
    ) -> Self {
        Self::with_config_path(socket_path, idle, stop, config, None)
    }

    /// Same as [`Self::new`] but also records the resolved path of the
    /// config file. Set by the `clitunesd` binary from the same resolver
    /// `DaemonConfig::load` used, so `Verb::ReadConfig` can echo it back
    /// to the TUI.
    pub fn with_config_path(
        socket_path: std::path::PathBuf,
        idle: Arc<IdleTimer>,
        stop: Arc<AtomicBool>,
        config: DaemonConfig,
        config_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            socket_path,
            idle,
            stop,
            config,
            config_path,
        }
    }

    pub async fn run(self) -> Result<()> {
        let capabilities = vec![
            "play".into(),
            "pause".into(),
            "source".into(),
            "viz".into(),
            "volume".into(),
            "picker".into(),
            "status".into(),
        ];

        let (mut server, mut verb_rx) =
            ControlServer::bind(&self.socket_path, capabilities).context("bind control socket")?;

        let idle_conn = Arc::clone(&self.idle);
        server.on_connect(move || {
            idle_conn.on_client_connected();
        });
        let idle_disc = Arc::clone(&self.idle);
        server.on_disconnect(move || {
            idle_disc.on_client_disconnected();
        });

        let event_tx = server.event_sender();

        // Probe the audio device's native rate so the entire pipeline
        // (ring, decoder, SPMC) runs at that rate. This eliminates the
        // double-resampling artefacts that occur when the ring is at
        // 48 kHz but the device negotiates 44.1 kHz.
        let cpal_cfg = CpalOutputConfig::default();
        let device_rate = CpalOutput::probe_device_rate(&cpal_cfg);
        let format = PcmFormat {
            sample_rate: device_rate,
            channels: 2,
        };
        let ring_frames = device_rate as usize; // ~1 second buffer

        let (region, spmc_producer) =
            <ShmRegion as PcmBridge>::create(DEFAULT_CAPACITY, format.sample_rate)
                .context("create SPMC PCM ring")?;
        let shm_name = region.shm_name().to_owned();
        tracing::info!(
            target: "clitunesd",
            shm_name = %shm_name,
            capacity = DEFAULT_CAPACITY,
            "SPMC PCM ring created"
        );

        let ring = PcmRing::new(format, ring_frames);
        let tee = TeeWriter::new(ring.writer(), Box::new(spmc_producer));

        let _audio_out = match CpalOutput::start(ring.reader(), cpal_cfg, device_rate) {
            Ok(out) => {
                let neg = out.negotiated();
                tracing::info!(
                    target: "clitunesd",
                    device = %neg.device_name,
                    rate = neg.sample_rate,
                    channels = neg.channels,
                    "audio output opened"
                );
                Some(out)
            }
            Err(e) => {
                tracing::warn!(
                    target: "clitunesd",
                    error = %e,
                    "audio output disabled (device open failed)"
                );
                None
            }
        };

        let pcm_tap_event = Event::PcmTap {
            shm_name: shm_name.clone(),
            sample_rate: format.sample_rate,
            channels: 2,
            capacity: DEFAULT_CAPACITY,
        };

        let (source_cmd_tx, source_cmd_rx) = std::sync::mpsc::channel::<SourceCommand>();

        let last_state: Arc<Mutex<Option<Event>>> = Arc::new(Mutex::new(None));

        // One shared Spotify handle per daemon: owned by the source pipeline
        // (for playback sessions) and the Web API cache (for token providers).
        // Funnels both paths through a single `load_credentials` call so the
        // on-disk refresh_token can't get rotated twice concurrently.
        #[cfg(feature = "spotify")]
        let spotify_handle = {
            let cred_path = crate::sources::spotify::default_credentials_path()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp/clitunes-spotify-creds.json"));
            Arc::new(crate::sources::spotify::SpotifyHandle::new(
                cred_path,
                tokio::runtime::Handle::current(),
            ))
        };
        #[cfg(feature = "spotify")]
        let source_spotify_handle = Arc::clone(&spotify_handle);

        // Shared slot so ConnectRuntime (producer) and ConnectSource
        // (consumer, running in the source pipeline) can hand off the
        // current sink handle. ConnectRuntime rebuilds the Session +
        // Player + sink on every Discovery event; each rebuild replaces
        // the slot contents so a freshly-activated ConnectSource always
        // binds to the right sink.
        #[cfg(feature = "connect")]
        let connect_sink_slot = crate::sources::spotify::ConnectSinkSlot::new();
        #[cfg(feature = "connect")]
        let source_connect_sink_slot = connect_sink_slot.clone();

        let source_stop = Arc::clone(&self.stop);
        let source_event_tx = event_tx.clone();
        let source_last_state = Arc::clone(&last_state);
        let source_thread = thread::Builder::new()
            .name("clitunesd-source".into())
            .spawn(move || {
                run_source_pipeline(
                    tee,
                    source_cmd_rx,
                    source_stop,
                    source_event_tx,
                    source_last_state,
                    format,
                    #[cfg(feature = "spotify")]
                    source_spotify_handle,
                    #[cfg(feature = "connect")]
                    source_connect_sink_slot,
                );
            })
            .context("spawn source pipeline")?;

        // Spawn the Spotify Connect receiver if configured. ConnectRuntime
        // survives source-pipeline transitions (phone re-picks reuse the
        // same mDNS advertisement); the source_cmd_tx clone lets it push
        // `PlayConnect` into the pipeline when credentials first arrive.
        #[cfg(feature = "connect")]
        let connect_runtime = if self.config.connect.enabled {
            let connect_source_cmd_tx = source_cmd_tx.clone();
            let connect_event_tx = event_tx.clone();
            match crate::sources::spotify::ConnectRuntime::spawn(
                self.config.connect.clone(),
                connect_sink_slot.clone(),
                device_rate,
                connect_source_cmd_tx,
                connect_event_tx,
                tokio::runtime::Handle::current(),
            ) {
                Ok(rt) => Some(rt),
                Err(e) => {
                    // Connect receiver failed to advertise (commonly: mDNS
                    // port collision or privileged-port bind). The daemon
                    // stays up so local/radio/spotify-URI playback still
                    // work; only the Connect receiver is off for this run.
                    tracing::error!(
                        target: "clitunesd",
                        error = %e,
                        "connect: receiver startup failed; daemon continuing without it"
                    );
                    let _ = event_tx
                        .send(Event::SourceError {
                            source: "connect".into(),
                            error: format!("connect startup failed: {e}"),
                            error_code: None,
                        })
                        .await;
                    None
                }
            }
        } else {
            None
        };

        let pcm_tap = pcm_tap_event.clone();
        let verb_stop = Arc::clone(&self.stop);
        let verb_ev_tx = event_tx.clone();
        let verb_last_state = Arc::clone(&last_state);
        #[cfg(feature = "webapi")]
        let webapi_cache = Arc::new(WebApiCache::new(Arc::clone(&spotify_handle)));
        #[cfg(feature = "webapi")]
        let verb_webapi = Arc::clone(&webapi_cache);
        #[cfg(feature = "connect")]
        let verb_disconnect_tx = connect_runtime.as_ref().map(|rt| rt.disconnect_sender());
        let verb_config = self.config.clone();
        let verb_config_path = self.config_path.clone();
        tokio::spawn(async move {
            dispatch_verbs(
                &mut verb_rx,
                &source_cmd_tx,
                &verb_ev_tx,
                &pcm_tap,
                &verb_stop,
                &verb_last_state,
                #[cfg(feature = "webapi")]
                &verb_webapi,
                #[cfg(feature = "connect")]
                verb_disconnect_tx,
                verb_config,
                verb_config_path,
            )
            .await;
        });

        let idle_check_stop = Arc::clone(&self.stop);
        let idle_ref = Arc::clone(&self.idle);
        let idle_shutdown = async move {
            loop {
                if idle_check_stop.load(Ordering::Relaxed) {
                    return;
                }
                if let super::Tick::Expired = idle_ref.tick() {
                    tracing::info!(target: "clitunesd", "idle window elapsed; requesting shutdown");
                    idle_check_stop.store(true, Ordering::SeqCst);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        };

        tokio::select! {
            _ = server.run() => {}
            _ = idle_shutdown => {
                tracing::info!(target: "clitunesd", "idle shutdown complete");
            }
        }

        self.stop.store(true, Ordering::SeqCst);

        // Shut down the Connect receiver before the source thread so
        // Spirc's outbound traffic stops before the sink is torn down.
        #[cfg(feature = "connect")]
        if let Some(rt) = connect_runtime {
            if let Err(e) = rt.shutdown().await {
                tracing::warn!(target: "clitunesd", error = %e, "connect: runtime shutdown error");
            }
        }

        let _ = source_thread.join();
        drop(region);
        Ok(())
    }
}

#[allow(dead_code, clippy::enum_variant_names)]
pub(crate) enum SourceCommand {
    PlayTone,
    PlayRadio {
        station: Station,
    },
    #[cfg(feature = "local")]
    PlayLocal {
        paths: Vec<std::path::PathBuf>,
    },
    #[cfg(feature = "spotify")]
    PlaySpotify {
        uri: String,
    },
    /// Switch the source pipeline into passive Connect mode. Carries no
    /// payload because Spotify Connect drives the Player externally
    /// (Spirc owns track lifecycle); the source pipeline only binds the
    /// shared sink and pumps PCM until interrupted by a stop or another
    /// `Play*` command. Emitted by `ConnectRuntime` on first credential
    /// yield, never directly by a client verb.
    #[cfg(feature = "connect")]
    PlayConnect,
}

#[allow(clippy::too_many_arguments)]
fn run_source_pipeline(
    mut tee: TeeWriter,
    cmd_rx: std::sync::mpsc::Receiver<SourceCommand>,
    stop: Arc<AtomicBool>,
    event_tx: mpsc::Sender<Event>,
    last_state: Arc<Mutex<Option<Event>>>,
    format: PcmFormat,
    #[cfg(feature = "spotify")] spotify_handle: Arc<crate::sources::spotify::SpotifyHandle>,
    #[cfg(feature = "connect")] connect_sink_slot: crate::sources::spotify::ConnectSinkSlot,
) {
    let pending: Arc<Mutex<Option<SourceCommand>>> = Arc::new(Mutex::new(None));
    let source_stop = Arc::new(AtomicBool::new(false));

    let watcher_pending = Arc::clone(&pending);
    let watcher_stop = Arc::clone(&source_stop);
    let watcher_global = Arc::clone(&stop);
    thread::Builder::new()
        .name("clitunesd-cmd-watcher".into())
        .spawn(move || loop {
            if watcher_global.load(Ordering::Relaxed) {
                watcher_stop.store(true, Ordering::SeqCst);
                return;
            }
            match cmd_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(cmd) => {
                    *watcher_pending.lock() = Some(cmd);
                    watcher_stop.store(true, Ordering::SeqCst);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    watcher_stop.store(true, Ordering::SeqCst);
                    return;
                }
            }
        })
        .expect("spawn cmd watcher");

    let mut current = SourceKind::Idle;

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        source_stop.store(false, Ordering::SeqCst);

        let send_state = |evt: Event| {
            *last_state.lock() = Some(evt.clone());
            let _ = event_tx.blocking_send(evt);
        };

        match &current {
            SourceKind::Idle => {
                // No source — wait for a command without generating audio.
                loop {
                    if stop.load(Ordering::Relaxed) || source_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            SourceKind::Tone => {
                send_state(Event::StateChanged {
                    state: PlayState::Playing,
                    source: Some("tone".into()),
                    station_or_path: None,
                    position_secs: None,
                    duration_secs: None,
                });
                let mut tone = ToneSource::new(format, TONE_BLOCK);
                tone.run(&mut tee, &source_stop);
            }
            SourceKind::Radio(station) => {
                let config = RadioConfig::new(station.clone(), format);
                match RadioSource::new(config) {
                    Ok(mut radio) => {
                        send_state(Event::StateChanged {
                            state: PlayState::Playing,
                            source: Some("radio".into()),
                            station_or_path: Some(station.name.clone()),
                            position_secs: None,
                            duration_secs: None,
                        });
                        radio.run(&mut tee, &source_stop);
                    }
                    Err(e) => {
                        tracing::error!(target: "clitunesd", error = %e, "radio source failed");
                        let _ = event_tx.blocking_send(Event::SourceError {
                            source: "radio".into(),
                            error: e.to_string(),
                            error_code: None,
                        });
                        current = SourceKind::Tone;
                        continue;
                    }
                }
            }
            #[cfg(feature = "local")]
            SourceKind::Local(paths) => match LocalSource::new(paths.clone(), format.sample_rate) {
                Ok(mut local) => {
                    let display_path = paths
                        .first()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    send_state(Event::StateChanged {
                        state: PlayState::Playing,
                        source: Some("local".into()),
                        station_or_path: Some(display_path),
                        position_secs: None,
                        duration_secs: None,
                    });
                    local.run(&mut tee, &source_stop);
                }
                Err(e) => {
                    tracing::error!(target: "clitunesd", error = %e, "local source failed");
                    let _ = event_tx.blocking_send(Event::SourceError {
                        source: "local".into(),
                        error: e.to_string(),
                        error_code: None,
                    });
                    current = SourceKind::Tone;
                    continue;
                }
            },
            #[cfg(feature = "spotify")]
            SourceKind::Spotify(ref uri) => {
                let mut spotify = build_spotify_source(
                    uri.clone(),
                    Arc::clone(&spotify_handle),
                    event_tx.clone(),
                    &format,
                );
                send_state(Event::StateChanged {
                    state: PlayState::Playing,
                    source: Some("spotify".into()),
                    station_or_path: Some(uri.clone()),
                    position_secs: None,
                    duration_secs: None,
                });
                spotify.run(&mut tee, &source_stop);
            }
            #[cfg(feature = "connect")]
            SourceKind::Connect => {
                send_state(Event::StateChanged {
                    state: PlayState::Playing,
                    source: Some("connect".into()),
                    station_or_path: None,
                    position_secs: None,
                    duration_secs: None,
                });
                let mut connect = crate::sources::spotify::ConnectSource::new(
                    connect_sink_slot.clone(),
                    format.sample_rate,
                );
                connect.run(&mut tee, &source_stop);
            }
        }

        if stop.load(Ordering::Relaxed) {
            return;
        }

        if let Some(cmd) = pending.lock().take() {
            match cmd {
                SourceCommand::PlayTone => current = SourceKind::Tone,
                SourceCommand::PlayRadio { station } => current = SourceKind::Radio(station),
                #[cfg(feature = "local")]
                SourceCommand::PlayLocal { paths } => current = SourceKind::Local(paths),
                #[cfg(feature = "spotify")]
                SourceCommand::PlaySpotify { uri } => current = SourceKind::Spotify(uri),
                #[cfg(feature = "connect")]
                SourceCommand::PlayConnect => current = SourceKind::Connect,
            }
        }
    }
}

enum SourceKind {
    /// No source selected — write silence until a source command arrives.
    Idle,
    Tone,
    Radio(Station),
    #[cfg(feature = "local")]
    Local(Vec<std::path::PathBuf>),
    #[cfg(feature = "spotify")]
    Spotify(String),
    /// Spotify Connect: the daemon is the receiver, Spirc owns the
    /// Player (track lifecycle, play/pause/seek), and the source pipeline
    /// only binds the shared sink and forwards PCM. Carries no payload —
    /// the active credentials live inside `ConnectRuntime`.
    #[cfg(feature = "connect")]
    Connect,
}

/// Construct a [`SpotifySource`] whose resampler target matches the
/// daemon's probed device rate.
///
/// Pinned as a standalone helper (rather than inlined) so the wiring
/// test in `spotify_wiring_tests` can assert the rate threads through
/// without a full pipeline boot. Regression guard for the 48kHz
/// hardcode that previously pitch-shifted Spotify on 44.1kHz hardware.
#[cfg(feature = "spotify")]
fn build_spotify_source(
    uri: String,
    handle: Arc<crate::sources::spotify::SpotifyHandle>,
    event_tx: mpsc::Sender<Event>,
    format: &PcmFormat,
) -> SpotifySource {
    SpotifySource::new(uri, handle, event_tx, format.sample_rate)
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_verbs(
    verb_rx: &mut VerbReceiver,
    source_cmd_tx: &std::sync::mpsc::Sender<SourceCommand>,
    event_tx: &mpsc::Sender<Event>,
    pcm_tap: &Event,
    stop: &Arc<AtomicBool>,
    last_state: &Arc<Mutex<Option<Event>>>,
    #[cfg(feature = "webapi")] webapi_cache: &Arc<WebApiCache>,
    #[cfg(feature = "connect")] connect_disconnect_tx: Option<mpsc::UnboundedSender<()>>,
    config: DaemonConfig,
    config_path: Option<std::path::PathBuf>,
) {
    // Serialises overlapping `Verb::StartAuth` calls: once a flow is
    // running the next verb gets a CommandResult ok but no events,
    // matching the "idempotent — return current status" contract.
    #[cfg(feature = "spotify")]
    let auth_in_progress = Arc::new(AtomicBool::new(false));
    while let Some((envelope, reply_tx)) = verb_rx.recv().await {
        if stop.load(Ordering::Relaxed) {
            let _ = reply_tx.try_send(Event::command_err(&envelope.cmd_id, "daemon shutting down"));
            return;
        }

        let cmd_id = &envelope.cmd_id;

        match &envelope.verb {
            Verb::Play => {
                tracing::info!(target: "clitunes_engine", "play: state → Playing");
                let evt = {
                    let mut guard = last_state.lock();
                    match guard.as_mut() {
                        Some(Event::StateChanged { state, .. }) => {
                            *state = PlayState::Playing;
                        }
                        _ => {
                            *guard = Some(Event::StateChanged {
                                state: PlayState::Playing,
                                source: None,
                                station_or_path: None,
                                position_secs: None,
                                duration_secs: None,
                            });
                        }
                    }
                    guard.clone()
                };
                if let Some(evt) = evt {
                    let _ = event_tx.send(evt).await;
                }
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Pause => {
                tracing::info!(target: "clitunes_engine", "pause: state → Paused");
                let evt = {
                    let mut guard = last_state.lock();
                    match guard.as_mut() {
                        Some(Event::StateChanged { state, .. }) => {
                            *state = PlayState::Paused;
                        }
                        _ => {
                            *guard = Some(Event::StateChanged {
                                state: PlayState::Paused,
                                source: None,
                                station_or_path: None,
                                position_secs: None,
                                duration_secs: None,
                            });
                        }
                    }
                    guard.clone()
                };
                if let Some(evt) = evt {
                    let _ = event_tx.send(evt).await;
                }
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Source(SourceArg::Radio { uuid }) => {
                match crate::sources::radio::resolve_station(uuid).await {
                    Ok(station) => {
                        tracing::info!(
                            target: "clitunesd",
                            station = %station.name,
                            url = %station.url_resolved,
                            "resolved radio station"
                        );
                        let _ = source_cmd_tx.send(SourceCommand::PlayRadio { station });
                        let _ = reply_tx.try_send(Event::command_ok(cmd_id));
                    }
                    Err(e) => {
                        tracing::error!(
                            target: "clitunesd",
                            error = %e,
                            %uuid,
                            "failed to resolve radio station"
                        );
                        let _ = reply_tx
                            .try_send(Event::command_err(cmd_id, format!("resolve station: {e}")));
                    }
                }
            }
            #[cfg(feature = "local")]
            Verb::Source(SourceArg::Local { path }) => {
                let paths = vec![std::path::PathBuf::from(path)];
                let _ = source_cmd_tx.send(SourceCommand::PlayLocal { paths });
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            #[cfg(not(feature = "local"))]
            Verb::Source(SourceArg::Local { .. }) => {
                let _ = reply_tx.try_send(Event::command_err(
                    cmd_id,
                    "local file playback not enabled in this build",
                ));
            }
            #[cfg(feature = "spotify")]
            Verb::Source(SourceArg::Spotify { uri }) => {
                let _ = source_cmd_tx.send(SourceCommand::PlaySpotify { uri: uri.clone() });
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            #[cfg(not(feature = "spotify"))]
            Verb::Source(SourceArg::Spotify { .. }) => {
                let _ = reply_tx.try_send(Event::command_err(
                    cmd_id,
                    "Spotify playback not enabled in this build",
                ));
            }
            Verb::Status => {
                if let Some(state_event) = last_state.lock().clone() {
                    let _ = reply_tx.try_send(state_event);
                }
                let _ = reply_tx.try_send(pcm_tap.clone());
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Volume { level } => {
                let _ = event_tx.send(Event::VolumeChanged { volume: *level }).await;
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Viz { name } => {
                let _ = event_tx
                    .send(Event::VizChanged { name: name.clone() })
                    .await;
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Layout { name } => {
                let _ = event_tx
                    .send(Event::LayoutChanged { name: name.clone() })
                    .await;
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Picker => {
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Next | Verb::Prev => {
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Quit | Verb::Subscribe { .. } | Verb::Unsubscribe { .. } | Verb::Capabilities => {
            }
            #[cfg(feature = "webapi")]
            Verb::Search { query, limit } => {
                dispatch_search(query, *limit, cmd_id, event_tx, &reply_tx, webapi_cache).await;
            }
            #[cfg(feature = "webapi")]
            Verb::BrowseLibrary { category, limit } => {
                dispatch_browse_library(
                    *category,
                    *limit,
                    cmd_id,
                    event_tx,
                    &reply_tx,
                    webapi_cache,
                )
                .await;
            }
            #[cfg(feature = "webapi")]
            Verb::BrowsePlaylist { id, limit } => {
                dispatch_browse_playlist(id, *limit, cmd_id, event_tx, &reply_tx, webapi_cache)
                    .await;
            }
            #[cfg(not(feature = "webapi"))]
            Verb::Search { .. } | Verb::BrowseLibrary { .. } | Verb::BrowsePlaylist { .. } => {
                let _ = reply_tx.try_send(Event::command_err(
                    cmd_id,
                    "browse/search not enabled in this build",
                ));
            }
            #[cfg(feature = "connect")]
            Verb::ConnectDisconnect => {
                if let Some(ref tx) = connect_disconnect_tx {
                    let _ = tx.send(());
                }
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            #[cfg(not(feature = "connect"))]
            Verb::ConnectDisconnect => {
                let _ = reply_tx.try_send(Event::command_err(
                    cmd_id,
                    "Spotify Connect not enabled in this build",
                ));
            }
            Verb::ReadConfig => {
                let snapshot = build_config_snapshot(&config, config_path.as_deref());
                // The event stream is the single source of truth — the
                // reply channel is only for the ack, so we push the
                // snapshot into the broadcast first, then ack.
                let _ = event_tx.send(snapshot).await;
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            #[cfg(feature = "spotify")]
            Verb::StartAuth => {
                // Ack first so the client's CommandResult round-trip
                // isn't blocked by the OAuth flow. Progress arrives via
                // the `auth` event topic.
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
                dispatch_start_auth(&auth_in_progress, event_tx.clone()).await;
            }
            #[cfg(not(feature = "spotify"))]
            Verb::StartAuth => {
                let _ = reply_tx.try_send(Event::command_err(
                    cmd_id,
                    "Spotify not enabled in this build",
                ));
            }
        }
    }
}

/// Kick off a Spotify OAuth flow on behalf of a TUI client. Bails
/// early when a flow is already running so repeated `a` presses on
/// the Settings tab don't open a second browser window. Emits
/// [`Event::AuthStarted`] before handing control to librespot-oauth
/// and [`Event::AuthCompleted`] / [`Event::AuthFailed`] when the
/// spawned task terminates.
#[cfg(feature = "spotify")]
async fn dispatch_start_auth(auth_in_progress: &Arc<AtomicBool>, event_tx: mpsc::Sender<Event>) {
    // `compare_exchange` guarantees a single winner on concurrent
    // presses; subsequent verbs observe `true` and skip silently.
    if auth_in_progress
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        tracing::debug!(target: "clitunesd", "start_auth: flow already in progress, ignoring");
        return;
    }

    let Some(cred_path) = crate::sources::spotify::default_credentials_path() else {
        let _ = event_tx
            .send(Event::AuthFailed {
                reason: "cannot determine config directory".into(),
            })
            .await;
        auth_in_progress.store(false, Ordering::SeqCst);
        return;
    };

    // Announce before blocking so the TUI can flip to "pending" before
    // the browser-open / listener-bind latency.
    let _ = event_tx.send(Event::AuthStarted { url: None }).await;

    let in_progress = Arc::clone(auth_in_progress);
    tokio::spawn(async move {
        // 5-minute ceiling: users who walked away get a clean failure
        // instead of a zombie flow holding the in-progress flag forever.
        const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

        let result = tokio::time::timeout(
            AUTH_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                crate::sources::spotify::authenticate_from_daemon(&cred_path)
            }),
        )
        .await;

        let event = match result {
            Ok(Ok(Ok(_))) => Event::AuthCompleted,
            Ok(Ok(Err(e))) => Event::AuthFailed {
                reason: e.to_string(),
            },
            Ok(Err(join_err)) => Event::AuthFailed {
                reason: format!("auth task panicked: {join_err}"),
            },
            Err(_elapsed) => Event::AuthFailed {
                reason: "timeout".into(),
            },
        };
        let _ = event_tx.send(event).await;
        in_progress.store(false, Ordering::SeqCst);
    });
}

/// Build the `ConfigSnapshot` event the daemon sends in reply to
/// `Verb::ReadConfig`. Inspects the Spotify credential cache at its
/// default location so users see the same auth state the daemon itself
/// would see on its next source switch.
fn build_config_snapshot(config: &DaemonConfig, config_path: Option<&std::path::Path>) -> Event {
    use crate::proto::events::AuthStatusKind;

    #[cfg(feature = "spotify")]
    let (credentials_path, auth_status, auth_detail) = {
        use crate::sources::spotify::{cached_auth_status, default_credentials_path, AuthStatus};
        match default_credentials_path() {
            Some(path) => {
                let status = cached_auth_status(&path);
                let (kind, detail) = match status {
                    AuthStatus::LoggedIn => (AuthStatusKind::LoggedIn, None),
                    AuthStatus::LoggedOut => (AuthStatusKind::LoggedOut, None),
                    AuthStatus::ScopesInsufficient => (AuthStatusKind::ScopesInsufficient, None),
                    AuthStatus::Unreadable(reason) => (AuthStatusKind::Unreadable, Some(reason)),
                };
                (Some(path.to_string_lossy().into_owned()), kind, detail)
            }
            None => (None, AuthStatusKind::LoggedOut, None),
        }
    };
    #[cfg(not(feature = "spotify"))]
    let (credentials_path, auth_status, auth_detail) =
        (None::<String>, AuthStatusKind::LoggedOut, None::<String>);

    Event::ConfigSnapshot {
        device_name: config.connect.name.clone(),
        connect_enabled: config.connect.enabled,
        config_path: config_path.map(|p| p.to_string_lossy().into_owned()),
        credentials_path,
        auth_status,
        auth_detail,
    }
}

/// Lazy cache for the daemon-side [`SpotifyWebApi`] client.
///
/// rspotify's `AuthCodePkceSpotify` owns its own token and refreshes it
/// internally on 401 responses, so we only need to build the client
/// once — on the first verb that needs it. Subsequent verbs share the
/// same `Arc<SpotifyWebApi>` (the underlying reqwest connection pool
/// and the in-memory token state come with it).
///
/// When an API call returns an error that indicates a terminal auth
/// failure ([`is_auth_shaped`] returns true) the dispatcher calls
/// [`WebApiCache::invalidate`] so the next verb rebuilds from disk.
/// rspotify handles transient 401s itself via the cached refresh
/// token; we only evict on hard failures.
///
/// # Concurrency model
///
/// [`get`](Self::get) holds a `tokio::sync::Mutex` across the first
/// build's `.await`. This is load-bearing: it guarantees that parallel
/// callers on an empty cache coalesce into a single build rather than
/// each running their own. Today the daemon dispatches verbs serially
/// via `while let Some(..) = verb_rx.recv().await`, so the lock never
/// has contention in practice — but if verb dispatch is ever
/// parallelised, this pattern still produces the correct result (one
/// build, N readers) without adding a separate build-barrier.
#[cfg(feature = "webapi")]
pub(crate) struct WebApiCache {
    /// Shared Spotify auth state. The cache asks the handle for a fresh
    /// [`SharedTokenProvider`] on first build — by reusing the handle's
    /// cached `AuthResult`, the Web API build path no longer races the
    /// source pipeline on `credentials.json` rotation.
    handle: Arc<crate::sources::spotify::SpotifyHandle>,
    client: tokio::sync::Mutex<Option<Arc<crate::sources::spotify::webapi::SpotifyWebApi>>>,
}

#[cfg(feature = "webapi")]
impl WebApiCache {
    pub(crate) fn new(handle: Arc<crate::sources::spotify::SpotifyHandle>) -> Self {
        Self {
            handle,
            client: tokio::sync::Mutex::new(None),
        }
    }

    /// Return the cached client, building it on first demand. The build
    /// step reads `credentials.json` on the blocking pool and does an
    /// OAuth refresh, so it isn't free — but it runs at most once per
    /// daemon lifetime (unless `invalidate` is called).
    pub(crate) async fn get(&self) -> Result<Arc<crate::sources::spotify::webapi::SpotifyWebApi>> {
        let mut guard = self.client.lock().await;
        if let Some(existing) = guard.as_ref() {
            return Ok(Arc::clone(existing));
        }
        let api = Arc::new(build_webapi(&self.handle).await?);
        *guard = Some(Arc::clone(&api));
        Ok(api)
    }

    /// Drop the cached client. The next call to [`WebApiCache::get`]
    /// will rebuild from disk. Call this when an API error indicates
    /// the cached token/client state is no longer recoverable.
    pub(crate) async fn invalidate(&self) {
        *self.client.lock().await = None;
    }

    /// Test-only: seed the cache with a pre-built client so invalidation
    /// behaviour can be asserted without building a real rspotify client.
    #[cfg(test)]
    pub(crate) async fn seed_for_test(
        &self,
        api: Arc<crate::sources::spotify::webapi::SpotifyWebApi>,
    ) {
        *self.client.lock().await = Some(api);
    }

    /// Test-only: whether the cache currently holds a client. Used to
    /// assert the effect of [`invalidate`](Self::invalidate).
    #[cfg(test)]
    pub(crate) async fn is_cached(&self) -> bool {
        self.client.lock().await.is_some()
    }
}

/// Build a fresh [`SpotifyWebApi`] client from the shared Spotify handle.
/// Asks the handle for a token provider — the handle's first call lazily
/// loads `credentials.json` on the blocking pool; subsequent calls reuse
/// the cached auth. Invoked at most once per `WebApiCache` lifetime unless
/// invalidated by a hard auth failure.
#[cfg(feature = "webapi")]
async fn build_webapi(
    handle: &crate::sources::spotify::SpotifyHandle,
) -> Result<crate::sources::spotify::webapi::SpotifyWebApi> {
    use crate::sources::spotify::webapi::SpotifyWebApi;
    let provider = handle
        .token_provider()
        .await
        .context("build spotify token provider")?;
    Ok(SpotifyWebApi::from_provider(&provider))
}

/// Does an HTTP status code indicate a terminal auth failure? Exposed
/// as a standalone function so it is unit-testable without constructing
/// a real `reqwest::Response`.
#[cfg(feature = "webapi")]
fn is_auth_status(status: u16) -> bool {
    matches!(status, 401 | 403)
}

/// Does this error indicate a terminal auth failure that should evict
/// the cached client?
///
/// Walks the `anyhow::Error` chain for an [`rspotify::ClientError`] and
/// matches on structured variants:
/// - `InvalidToken` — rspotify rejected the token before sending
/// - `Http(StatusCode(resp))` with `resp.status()` 401 or 403
///
/// Errors that never reached rspotify (our own credential-cache failures
/// from [`build_webapi`]) are matched on their typed context strings —
/// still not a raw string search of the whole chain, but narrow enough
/// that a log-format change upstream can't silently break invalidation.
#[cfg(feature = "webapi")]
fn is_auth_shaped(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(client_err) = cause.downcast_ref::<rspotify::ClientError>() {
            return match client_err {
                rspotify::ClientError::InvalidToken => true,
                rspotify::ClientError::Http(http) => match http.as_ref() {
                    rspotify::http::HttpError::StatusCode(resp) => {
                        is_auth_status(resp.status().as_u16())
                    }
                    _ => false,
                },
                _ => false,
            };
        }
    }

    // Not an rspotify error. `build_webapi` attaches one of two typed
    // context strings when the on-disk credential cache is unusable;
    // either is an auth failure from our perspective. We only look at
    // the outermost error's top-level message so an unrelated cause
    // anywhere deeper in the chain can't flip the decision.
    let outer = err.to_string();
    outer == "load spotify credentials" || outer == "resolve spotify credentials path (set $HOME?)"
}

/// Map any auth/webapi error to a `CommandResult { ok: false, .. }`.
/// Keeps dispatch handlers terse.
///
/// `context` is the dispatcher-level verb ("spotify search", "spotify
/// library", …). It is the **only** layer that should carry that verb
/// — the per-call `.context()` strings in `sources/spotify/webapi.rs`
/// deliberately add narrower segments ("saved tracks", "fetch playlist
/// items") so the chain reads as `"spotify library: saved tracks:
/// <cause>"` rather than a doubled prefix.
///
/// Uses `{err:#}` (anyhow's alt formatter) so the full cause chain is
/// rendered in both the daemon log and the CLI-facing message. A plain
/// `%err` would print only the outermost segment, hiding the real
/// reason behind whichever context is attached last.
#[cfg(feature = "webapi")]
fn webapi_err(cmd_id: &str, context: &str, err: anyhow::Error) -> Event {
    tracing::warn!(
        target: "clitunesd",
        error = format!("{err:#}"),
        context,
        "spotify webapi call failed"
    );
    Event::command_err(cmd_id, format!("{context}: {err:#}"))
}

#[cfg(feature = "webapi")]
async fn dispatch_search(
    query: &str,
    limit: Option<u32>,
    cmd_id: &str,
    event_tx: &mpsc::Sender<Event>,
    reply_tx: &mpsc::Sender<Event>,
    webapi_cache: &Arc<WebApiCache>,
) {
    let api = match webapi_cache.get().await {
        Ok(api) => api,
        Err(e) => {
            if is_auth_shaped(&e) {
                webapi_cache.invalidate().await;
            }
            let _ = reply_tx.try_send(webapi_err(cmd_id, "spotify auth", e));
            return;
        }
    };
    match api.search(query, limit).await {
        Ok((items, total)) => {
            let _ = event_tx
                .send(Event::SearchResults {
                    query: query.to_string(),
                    items,
                    total,
                })
                .await;
            let _ = reply_tx.try_send(Event::command_ok(cmd_id));
        }
        Err(e) => {
            if is_auth_shaped(&e) {
                webapi_cache.invalidate().await;
            }
            let _ = reply_tx.try_send(webapi_err(cmd_id, "spotify search", e));
        }
    }
}

#[cfg(feature = "webapi")]
async fn dispatch_browse_library(
    category: clitunes_core::LibraryCategory,
    limit: Option<u32>,
    cmd_id: &str,
    event_tx: &mpsc::Sender<Event>,
    reply_tx: &mpsc::Sender<Event>,
    webapi_cache: &Arc<WebApiCache>,
) {
    use clitunes_core::LibraryCategory;
    let api = match webapi_cache.get().await {
        Ok(api) => api,
        Err(e) => {
            if is_auth_shaped(&e) {
                webapi_cache.invalidate().await;
            }
            let _ = reply_tx.try_send(webapi_err(cmd_id, "spotify auth", e));
            return;
        }
    };
    let result = match category {
        LibraryCategory::SavedTracks => api.saved_tracks(limit).await,
        LibraryCategory::SavedAlbums => api.saved_albums(limit).await,
        LibraryCategory::Playlists => api.playlists(limit).await,
        LibraryCategory::RecentlyPlayed => api.recently_played(limit).await,
    };
    match result {
        Ok((items, total)) => {
            let _ = event_tx
                .send(Event::LibraryResults {
                    category,
                    items,
                    total,
                })
                .await;
            let _ = reply_tx.try_send(Event::command_ok(cmd_id));
        }
        Err(e) => {
            if is_auth_shaped(&e) {
                webapi_cache.invalidate().await;
            }
            let _ = reply_tx.try_send(webapi_err(cmd_id, "spotify library", e));
        }
    }
}

#[cfg(feature = "webapi")]
async fn dispatch_browse_playlist(
    id: &str,
    limit: Option<u32>,
    cmd_id: &str,
    event_tx: &mpsc::Sender<Event>,
    reply_tx: &mpsc::Sender<Event>,
    webapi_cache: &Arc<WebApiCache>,
) {
    let api = match webapi_cache.get().await {
        Ok(api) => api,
        Err(e) => {
            if is_auth_shaped(&e) {
                webapi_cache.invalidate().await;
            }
            let _ = reply_tx.try_send(webapi_err(cmd_id, "spotify auth", e));
            return;
        }
    };
    match api.playlist_tracks(id, limit).await {
        Ok((name, items, total)) => {
            let _ = event_tx
                .send(Event::PlaylistResults {
                    playlist_id: id.to_string(),
                    playlist_name: Some(name),
                    items,
                    total,
                })
                .await;
            let _ = reply_tx.try_send(Event::command_ok(cmd_id));
        }
        Err(e) => {
            if is_auth_shaped(&e) {
                webapi_cache.invalidate().await;
            }
            let _ = reply_tx.try_send(webapi_err(cmd_id, "spotify playlist", e));
        }
    }
}

#[cfg(test)]
#[cfg(feature = "webapi")]
mod webapi_cache_tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn is_auth_status_matches_401_and_403_only() {
        assert!(is_auth_status(401));
        assert!(is_auth_status(403));
        assert!(!is_auth_status(400));
        assert!(!is_auth_status(404));
        assert!(!is_auth_status(429));
        assert!(!is_auth_status(500));
        assert!(!is_auth_status(503));
    }

    #[test]
    fn is_auth_shaped_matches_rspotify_invalid_token() {
        // Structured variant — the dispatcher should evict on this.
        let err: anyhow::Error =
            anyhow::Error::from(rspotify::ClientError::InvalidToken).context("spotify search");
        assert!(is_auth_shaped(&err));
    }

    #[test]
    fn is_auth_shaped_matches_our_credential_load_failure() {
        // `build_webapi` attaches this exact context on load_credentials
        // failure — matches the current `.context(...)` call sites.
        let err: anyhow::Error =
            anyhow!("no cached Spotify credentials").context("load spotify credentials");
        assert!(is_auth_shaped(&err));

        let err: anyhow::Error = anyhow!("dirs::config_dir returned None")
            .context("resolve spotify credentials path (set $HOME?)");
        assert!(is_auth_shaped(&err));
    }

    #[test]
    fn is_auth_shaped_rejects_transient_and_unrelated_errors() {
        // Plain messages that *used* to match the old string heuristic
        // no longer do — we only trust structured evidence now.
        assert!(!is_auth_shaped(&anyhow!("connection reset")));
        assert!(!is_auth_shaped(&anyhow!("dns lookup failed")));
        assert!(!is_auth_shaped(&anyhow!("timed out after 10s")));
        assert!(!is_auth_shaped(&anyhow!("got 500 Internal Server Error")));
        // A bare string that happens to contain "401" or "unauthorized"
        // no longer triggers invalidation — it must come through
        // rspotify or our own typed contexts.
        assert!(!is_auth_shaped(&anyhow!(
            "got 401 from some unrelated service"
        )));
        // `credential task panicked` is a JoinError wrapper, not an
        // auth failure.
        let err: anyhow::Error = anyhow!("runtime shut down").context("credential task panicked");
        assert!(!is_auth_shaped(&err));
    }

    /// Build a [`SpotifyWebApi`] with a synthetic token. The client
    /// won't successfully hit Spotify, but it is real-typed — enough to
    /// let us test the cache container's seed/invalidate semantics
    /// without touching the network.
    fn synthetic_webapi() -> Arc<crate::sources::spotify::webapi::SpotifyWebApi> {
        use crate::sources::spotify::{token::SharedTokenProvider, webapi::SpotifyWebApi};
        use librespot_oauth::OAuthToken;
        use std::time::{Duration, Instant};
        let token = OAuthToken {
            access_token: "synthetic".into(),
            refresh_token: "synthetic".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
            token_type: "Bearer".into(),
            scopes: vec!["streaming".into()],
        };
        let provider = SharedTokenProvider::new(token, "/tmp/ignored".into());
        Arc::new(SpotifyWebApi::from_provider(&provider))
    }

    #[tokio::test]
    async fn invalidate_drops_a_seeded_client() {
        let cache = WebApiCache::new(Arc::new(crate::sources::spotify::SpotifyHandle::new(
            std::path::PathBuf::from("/tmp/clitunes-test-webapi-handle.json"),
            tokio::runtime::Handle::current(),
        )));
        assert!(!cache.is_cached().await, "fresh cache should be empty");

        cache.seed_for_test(synthetic_webapi()).await;
        assert!(cache.is_cached().await, "seed should populate the cache");

        cache.invalidate().await;
        assert!(
            !cache.is_cached().await,
            "invalidate should drop the client"
        );
    }

    #[tokio::test]
    async fn invalidate_on_empty_cache_is_a_noop() {
        // Regression guard: invalidate must not deadlock or panic when
        // the cache is already empty.
        let cache = WebApiCache::new(Arc::new(crate::sources::spotify::SpotifyHandle::new(
            std::path::PathBuf::from("/tmp/clitunes-test-webapi-handle.json"),
            tokio::runtime::Handle::current(),
        )));
        cache.invalidate().await;
        cache.invalidate().await;
        assert!(!cache.is_cached().await);
    }
}

#[cfg(test)]
#[cfg(feature = "spotify")]
mod spotify_wiring_tests {
    //! Pins the rate-threading wiring between `run_source_pipeline`
    //! (which owns the probed device `PcmFormat`) and `SpotifySource`
    //! (which passes the rate down to the resampler target).
    //!
    //! The `sources::spotify::sink::target_rate_44100_is_identity_pass`
    //! test proves the sink does the right thing *when given* 44.1kHz.
    //! These tests prove the daemon actually *gives* it the device rate,
    //! not a hardcoded 48kHz. Without this pair, a regression that
    //! reintroduced `SpotifySource::new(..., 48_000)` at the call site
    //! would slip through both test suites.
    use super::*;
    use clitunes_core::PcmFormat;

    #[tokio::test]
    async fn threads_44100_format_rate_to_source() {
        let (tx, _rx) = mpsc::channel::<Event>(1);
        let format = PcmFormat {
            sample_rate: 44_100,
            channels: 2,
        };
        let handle = Arc::new(crate::sources::spotify::SpotifyHandle::new(
            std::path::PathBuf::from("/tmp/clitunes-test-creds.json"),
            tokio::runtime::Handle::current(),
        ));
        let source =
            build_spotify_source("spotify:track:doesnt:matter".into(), handle, tx, &format);
        assert_eq!(
            source.target_sample_rate(),
            44_100,
            "44.1kHz device rate must reach the Spotify resampler target"
        );
    }

    #[tokio::test]
    async fn threads_48000_format_rate_to_source() {
        let (tx, _rx) = mpsc::channel::<Event>(1);
        let format = PcmFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        let handle = Arc::new(crate::sources::spotify::SpotifyHandle::new(
            std::path::PathBuf::from("/tmp/clitunes-test-creds.json"),
            tokio::runtime::Handle::current(),
        ));
        let source =
            build_spotify_source("spotify:track:doesnt:matter".into(), handle, tx, &format);
        assert_eq!(source.target_sample_rate(), 48_000);
    }

    #[tokio::test]
    async fn threads_96000_format_rate_to_source() {
        // Exotic but legal hi-fi rate — proves the helper isn't
        // clamped/capped and genuinely passes the format field.
        let (tx, _rx) = mpsc::channel::<Event>(1);
        let format = PcmFormat {
            sample_rate: 96_000,
            channels: 2,
        };
        let handle = Arc::new(crate::sources::spotify::SpotifyHandle::new(
            std::path::PathBuf::from("/tmp/clitunes-test-creds.json"),
            tokio::runtime::Handle::current(),
        ));
        let source =
            build_spotify_source("spotify:track:doesnt:matter".into(), handle, tx, &format);
        assert_eq!(source.target_sample_rate(), 96_000);
    }
}
