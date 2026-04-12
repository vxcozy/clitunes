use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clitunes_core::{PcmFormat, Station};
use tokio::sync::mpsc;

use crate::audio::ring::PcmWriter;
use crate::audio::{CpalOutput, CpalOutputConfig, FftTap, PcmRing};
use crate::pcm::cross_process_api::{PcmBridge, DEFAULT_CAPACITY};
use crate::pcm::spmc_ring::ShmRegion;
use crate::proto::events::{Event, PlayState};
use crate::proto::server::{ControlServer, VerbReceiver};
use crate::proto::verbs::{SourceArg, Verb};
use crate::sources::radio::{RadioConfig, RadioSource};
use crate::sources::tone_source::ToneSource;
use crate::sources::Source;

use super::tee_writer::TeeWriter;
use super::IdleTimer;

const RING_FRAMES: usize = 48_000;
const TONE_BLOCK: usize = 1024;
const FORMAT: PcmFormat = PcmFormat::STUDIO;

pub struct DaemonEventLoop {
    socket_path: std::path::PathBuf,
    idle: Arc<IdleTimer>,
    stop: Arc<AtomicBool>,
}

impl DaemonEventLoop {
    pub fn new(socket_path: std::path::PathBuf, idle: Arc<IdleTimer>, stop: Arc<AtomicBool>) -> Self {
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

        let (region, spmc_producer) =
            <ShmRegion as PcmBridge>::create(DEFAULT_CAPACITY, FORMAT.sample_rate)
                .context("create SPMC PCM ring")?;
        let shm_name = region.shm_name().to_owned();
        tracing::info!(
            target: "clitunesd",
            shm_name = %shm_name,
            capacity = DEFAULT_CAPACITY,
            "SPMC PCM ring created"
        );

        let ring = PcmRing::new(FORMAT, RING_FRAMES);
        let tee = TeeWriter::new(ring.writer(), Box::new(spmc_producer));

        let _audio_out = match CpalOutput::start(ring.reader(), CpalOutputConfig::default()) {
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
            sample_rate: FORMAT.sample_rate,
            channels: 2,
            capacity: DEFAULT_CAPACITY,
        };

        let (source_cmd_tx, source_cmd_rx) = std::sync::mpsc::channel::<SourceCommand>();

        let source_stop = Arc::clone(&self.stop);
        let source_event_tx = event_tx.clone();
        let source_thread = thread::Builder::new()
            .name("clitunesd-source".into())
            .spawn(move || {
                run_source_pipeline(tee, source_cmd_rx, source_stop, source_event_tx);
            })
            .context("spawn source pipeline")?;

        let stop = Arc::clone(&self.stop);
        let idle = Arc::clone(&self.idle);
        let pcm_tap = pcm_tap_event.clone();
        let ev_tx = event_tx.clone();

        tokio::spawn(async move {
            dispatch_verbs(&mut verb_rx, &source_cmd_tx, &ev_tx, &pcm_tap, &stop).await;
        });

        let idle_check_stop = Arc::clone(&self.stop);
        let idle_ref = Arc::clone(&self.idle);
        tokio::spawn(async move {
            loop {
                if idle_check_stop.load(Ordering::Relaxed) {
                    return;
                }
                match idle_ref.tick() {
                    super::Tick::Expired => {
                        tracing::info!(
                            target: "clitunesd",
                            "idle window elapsed; requesting shutdown"
                        );
                        idle_check_stop.store(true, Ordering::SeqCst);
                        return;
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });

        server.run().await;

        self.stop.store(true, Ordering::SeqCst);
        let _ = source_thread.join();
        drop(region);
        Ok(())
    }
}

enum SourceCommand {
    PlayTone,
    PlayRadio { station: Station },
    Stop,
}

fn run_source_pipeline(
    mut tee: TeeWriter,
    cmd_rx: std::sync::mpsc::Receiver<SourceCommand>,
    stop: Arc<AtomicBool>,
    event_tx: mpsc::Sender<Event>,
) {
    let mut tone = ToneSource::new(FORMAT, TONE_BLOCK);
    tone.run(&mut tee, &stop);
    let _ = event_tx.blocking_send(Event::StateChanged {
        state: PlayState::Playing,
        source: Some("tone".into()),
        station_or_path: None,
        position_secs: None,
        duration_secs: None,
    });

    while !stop.load(Ordering::Relaxed) {
        match cmd_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(SourceCommand::PlayTone) => {
                let mut tone = ToneSource::new(FORMAT, TONE_BLOCK);
                tone.run(&mut tee, &stop);
            }
            Ok(SourceCommand::PlayRadio { station }) => {
                let config = RadioConfig::new(station.clone(), FORMAT);
                match RadioSource::new(config) {
                    Ok(mut radio) => {
                        let _ = event_tx.blocking_send(Event::StateChanged {
                            state: PlayState::Playing,
                            source: Some("radio".into()),
                            station_or_path: Some(station.name.clone()),
                            position_secs: None,
                            duration_secs: None,
                        });
                        radio.run(&mut tee, &stop);
                    }
                    Err(e) => {
                        tracing::error!(
                            target: "clitunesd",
                            error = %e,
                            "failed to create radio source"
                        );
                        let _ = event_tx.blocking_send(Event::SourceError {
                            source: "radio".into(),
                            error: e.to_string(),
                        });
                    }
                }
            }
            Ok(SourceCommand::Stop) => {
                let _ = event_tx.blocking_send(Event::StateChanged {
                    state: PlayState::Stopped,
                    source: None,
                    station_or_path: None,
                    position_secs: None,
                    duration_secs: None,
                });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

async fn dispatch_verbs(
    verb_rx: &mut VerbReceiver,
    source_cmd_tx: &std::sync::mpsc::Sender<SourceCommand>,
    event_tx: &mpsc::Sender<Event>,
    pcm_tap: &Event,
    stop: &Arc<AtomicBool>,
) {
    while let Some((envelope, reply_tx)) = verb_rx.recv().await {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        let cmd_id = &envelope.cmd_id;

        match &envelope.verb {
            Verb::Play => {
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Pause => {
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Source(SourceArg::Radio { uuid }) => {
                match crate::sources::radio::resolve_station_blocking(uuid) {
                    Ok(station) => {
                        let _ = source_cmd_tx.send(SourceCommand::PlayRadio { station });
                        let _ = reply_tx.try_send(Event::command_ok(cmd_id));
                    }
                    Err(e) => {
                        let _ = reply_tx.try_send(Event::command_err(
                            cmd_id,
                            format!("resolve station: {e}"),
                        ));
                    }
                }
            }
            Verb::Source(SourceArg::Local { .. }) => {
                let _ = reply_tx.try_send(Event::command_err(
                    cmd_id,
                    "local file playback not yet implemented",
                ));
            }
            Verb::Status => {
                let _ = reply_tx.try_send(pcm_tap.clone());
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Volume { level } => {
                let _ = event_tx.send(Event::VolumeChanged { volume: *level }).await;
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Viz { name } => {
                let _ = event_tx
                    .send(Event::VizChanged {
                        name: name.clone(),
                    })
                    .await;
                let _ = reply_tx.try_send(Event::command_ok(cmd_id));
            }
            Verb::Layout { name } => {
                let _ = event_tx
                    .send(Event::LayoutChanged {
                        name: name.clone(),
                    })
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
                // Handled by ControlServer directly
            }
        }
    }
}
