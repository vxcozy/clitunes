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

use super::tee_writer::TeeWriter;
use super::IdleTimer;

const TONE_BLOCK: usize = 1024;

pub struct DaemonEventLoop {
    socket_path: std::path::PathBuf,
    idle: Arc<IdleTimer>,
    stop: Arc<AtomicBool>,
}

impl DaemonEventLoop {
    pub fn new(
        socket_path: std::path::PathBuf,
        idle: Arc<IdleTimer>,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            socket_path,
            idle,
            stop,
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
                );
            })
            .context("spawn source pipeline")?;

        let pcm_tap = pcm_tap_event.clone();
        let verb_stop = Arc::clone(&self.stop);
        let verb_ev_tx = event_tx.clone();
        let verb_last_state = Arc::clone(&last_state);
        tokio::spawn(async move {
            dispatch_verbs(
                &mut verb_rx,
                &source_cmd_tx,
                &verb_ev_tx,
                &pcm_tap,
                &verb_stop,
                &verb_last_state,
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
        let _ = source_thread.join();
        drop(region);
        Ok(())
    }
}

#[allow(dead_code, clippy::enum_variant_names)]
enum SourceCommand {
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
}

fn run_source_pipeline(
    mut tee: TeeWriter,
    cmd_rx: std::sync::mpsc::Receiver<SourceCommand>,
    stop: Arc<AtomicBool>,
    event_tx: mpsc::Sender<Event>,
    last_state: Arc<Mutex<Option<Event>>>,
    format: PcmFormat,
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
                    });
                    current = SourceKind::Tone;
                    continue;
                }
            },
            #[cfg(feature = "spotify")]
            SourceKind::Spotify(ref uri) => {
                let cred_path =
                    crate::sources::spotify::default_credentials_path().unwrap_or_else(|| {
                        std::path::PathBuf::from("/tmp/clitunes-spotify-creds.json")
                    });
                let mut spotify = SpotifySource::new(uri.clone(), cred_path, event_tx.clone());
                send_state(Event::StateChanged {
                    state: PlayState::Playing,
                    source: Some("spotify".into()),
                    station_or_path: Some(uri.clone()),
                    position_secs: None,
                    duration_secs: None,
                });
                spotify.run(&mut tee, &source_stop);
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
}

async fn dispatch_verbs(
    verb_rx: &mut VerbReceiver,
    source_cmd_tx: &std::sync::mpsc::Sender<SourceCommand>,
    event_tx: &mpsc::Sender<Event>,
    pcm_tap: &Event,
    stop: &Arc<AtomicBool>,
    last_state: &Arc<Mutex<Option<Event>>>,
) {
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
        }
    }
}
