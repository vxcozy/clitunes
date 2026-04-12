//! clitunes — daemon-client TUI music player.
//!
//! Connects to `clitunesd` over a Unix socket control bus, reads PCM via
//! shared-memory SPMC ring, and renders visualisers at 30 fps.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use clitunes::auto_spawn;
use clitunes::client::reconnect::ReconnectingSession;
use clitunes::client::render_loop::{RenderLoop, RenderLoopConfig};
use clitunes_engine::observability;
use clitunes_engine::pcm::spmc_ring::ShmRegion;
use clitunes_engine::proto::events::Event;
use clitunes_engine::proto::verbs::{SourceArg, Verb};
use clitunes_engine::tui::persistence::{
    default_state_path, load_state, save_state, Recovery, State, SOURCE_RADIO,
};

fn main() -> Result<()> {
    observability::init_tracing("clitunes")?;

    let cli = CliArgs::parse_from_env()?;

    let app_stop = Arc::new(AtomicBool::new(false));
    install_signal_handler(Arc::clone(&app_stop))?;

    let state_path = default_state_path();
    let previous_state = match state_path.as_ref().map(|p| load_state(p)).transpose()? {
        Some(Recovery::Loaded(s)) => Some(s),
        Some(Recovery::Corrupt(reason)) => {
            tracing::warn!(target: "clitunes", %reason, "state.toml corrupt; starting fresh");
            if let Some(p) = state_path.as_ref() {
                let _ = std::fs::remove_file(p);
            }
            None
        }
        Some(Recovery::Missing) | None => None,
    };

    let connected = auto_spawn::connect_or_spawn().context("connect to daemon")?;
    let socket_path = connected.socket_path.clone();
    if connected.spawned_daemon {
        tracing::info!(target: "clitunes", "spawned daemon");
    }
    drop(connected.stream);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    rt.block_on(run_client(
        cli,
        socket_path,
        previous_state,
        state_path,
        app_stop,
    ))
}

async fn run_client(
    cli: CliArgs,
    socket_path: PathBuf,
    previous_state: Option<State>,
    state_path: Option<PathBuf>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut session = ReconnectingSession::connect(socket_path)
        .await
        .context("connect control session")?;

    session.request_status().await?;

    let mut pcm_tap = None;
    for _ in 0..50 {
        match session.recv_event_timeout(Duration::from_millis(100)).await {
            Some(Event::PcmTap {
                shm_name,
                sample_rate,
                channels,
                capacity,
            }) => {
                tracing::info!(
                    target: "clitunes",
                    %shm_name, sample_rate, channels, capacity,
                    "received PcmTap"
                );
                pcm_tap = Some((shm_name, sample_rate));
                break;
            }
            Some(_) => {}
            None => {}
        }
    }

    let (shm_name, sample_rate) =
        pcm_tap.ok_or_else(|| anyhow::anyhow!("daemon did not send PcmTap within 5s"))?;

    let (_region, consumer) = ShmRegion::open_consumer(&shm_name)
        .map_err(|e| anyhow::anyhow!("open SPMC consumer '{shm_name}': {e}"))?;

    let boot = BootIntent::from_cli_and_state(&cli, previous_state.as_ref());
    match &boot {
        BootIntent::AutoResume { uuid, name } => {
            tracing::info!(
                target: "clitunes",
                %uuid,
                name = name.as_deref().unwrap_or("?"),
                "auto-resuming last station"
            );
            session
                .send_verb(Verb::Source(SourceArg::Radio { uuid: uuid.clone() }))
                .await?;
        }
        BootIntent::CliStation(uuid) => {
            tracing::info!(target: "clitunes", %uuid, "playing CLI station");
            session
                .send_verb(Verb::Source(SourceArg::Radio { uuid: uuid.clone() }))
                .await?;
        }
        BootIntent::FirstRunShowPicker => {
            tracing::info!(target: "clitunes", "first run — picker will show");
        }
        BootIntent::CliTone => {
            tracing::info!(target: "clitunes", "tone mode");
        }
    }

    let (event_tx, event_rx) = std::sync::mpsc::channel::<Event>();
    let (verb_tx, mut verb_rx) = tokio::sync::mpsc::channel::<Verb>(64);

    let bridge_stop = Arc::clone(&stop);
    let bridge_state_path = state_path.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = session.recv_event() => {
                    match event {
                        Some(ev) => {
                            if event_tx.send(ev).is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                verb = verb_rx.recv() => {
                    match verb {
                        Some(v) => {
                            if let Verb::Source(SourceArg::Radio { ref uuid }) = v {
                                persist_state_best_effort(
                                    uuid,
                                    bridge_state_path.as_deref(),
                                );
                            }
                            let _ = session.send_verb(v).await;
                        }
                        None => break,
                    }
                }
            }
            if bridge_stop.load(Ordering::Relaxed) {
                break;
            }
        }
    });

    let render_stop = Arc::clone(&stop);
    let render_handle = tokio::task::spawn_blocking(move || {
        let mut render = RenderLoop::new(RenderLoopConfig {
            consumer: Box::new(consumer),
            sample_rate,
            event_rx,
            verb_tx,
            stop: render_stop,
        });
        render.run()
    });

    let result = render_handle.await?;
    drop(_region);
    result
}

fn persist_state_best_effort(uuid: &str, path: Option<&std::path::Path>) {
    let Some(path) = path else { return };
    let state = State {
        last_station_uuid: Some(uuid.to_string()),
        last_station_name: None,
        last_source: Some(SOURCE_RADIO.to_string()),
        last_visualiser: None,
        last_layout: None,
    };
    if let Err(e) = save_state(&state, path) {
        tracing::warn!(target: "clitunes", error = %e, "save state failed");
    }
}

enum BootIntent {
    FirstRunShowPicker,
    AutoResume { uuid: String, name: Option<String> },
    CliStation(String),
    CliTone,
}

impl BootIntent {
    fn from_cli_and_state(cli: &CliArgs, state: Option<&State>) -> Self {
        match (cli.source, cli.station.as_ref()) {
            (SourceChoice::Tone, _) => BootIntent::CliTone,
            (SourceChoice::Radio, Some(uuid)) => BootIntent::CliStation(uuid.clone()),
            (SourceChoice::Radio, None) | (SourceChoice::Auto, _) => {
                if let Some(s) = state {
                    if let Some(uuid) = &s.last_station_uuid {
                        return BootIntent::AutoResume {
                            uuid: uuid.clone(),
                            name: s.last_station_name.clone(),
                        };
                    }
                }
                BootIntent::FirstRunShowPicker
            }
        }
    }
}

#[derive(Copy, Clone)]
enum SourceChoice {
    Tone,
    Radio,
    Auto,
}

#[derive(Clone)]
struct CliArgs {
    source: SourceChoice,
    station: Option<String>,
}

impl CliArgs {
    fn parse_from_env() -> Result<Self> {
        let mut source = SourceChoice::Auto;
        let mut station: Option<String> = None;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                "--source" => {
                    let value = args.next().ok_or_else(|| {
                        anyhow::anyhow!("--source requires a value (tone|radio|auto)")
                    })?;
                    source = match value.as_str() {
                        "tone" => SourceChoice::Tone,
                        "radio" => SourceChoice::Radio,
                        "auto" => SourceChoice::Auto,
                        other => {
                            anyhow::bail!(
                                "unknown --source '{}': expected tone, radio, or auto",
                                other
                            );
                        }
                    };
                }
                "--station" => {
                    let value = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--station requires a UUID"))?;
                    station = Some(value);
                }
                other => anyhow::bail!("unknown argument: {}", other),
            }
        }

        Ok(Self { source, station })
    }
}

fn print_help() {
    println!(
        "\
clitunes — the Ghostty of TUI music apps

USAGE:
    clitunes [--source auto|tone|radio] [--station <uuid>]

OPTIONS:
    --source <auto|tone|radio>  Audio source (default: auto — resume last station or show picker)
    --station <uuid>            Radio station UUID (used with --source radio)
    -h, --help                  Show this help

KEYS:
    ↑ / ↓       move picker selection (or j / k)
    enter       confirm picker selection
    s           open / close the station picker
    n / p       next / previous visualiser
    q / ESC     quit
"
    );
}

fn install_signal_handler(stop: Arc<AtomicBool>) -> Result<()> {
    extern "C" fn handler(_sig: libc::c_int) {
        HANDLED.store(true, Ordering::SeqCst);
    }
    static HANDLED: AtomicBool = AtomicBool::new(false);

    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as usize;
        sa.sa_flags = 0;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
    }

    thread::Builder::new()
        .name("clitunes-signal".into())
        .spawn(move || loop {
            if HANDLED.load(Ordering::SeqCst) {
                stop.store(true, Ordering::SeqCst);
                return;
            }
            thread::sleep(Duration::from_millis(50));
        })?;
    Ok(())
}
