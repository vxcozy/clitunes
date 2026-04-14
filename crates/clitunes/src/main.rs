//! clitunes — daemon-client TUI music player.
//!
//! Connects to `clitunesd` over a Unix socket control bus, reads PCM via
//! shared-memory SPMC ring, and renders visualisers at 30 fps.
//!
//! Dispatch modes:
//! - `clitunes` (no subcommand)        → full TUI with picker + carousel
//! - `clitunes --pane <name>`          → standalone single-component pane
//! - `clitunes play|pause|next|prev`   → headless one-shot verb
//! - `clitunes volume <0-100>`         → headless volume
//! - `clitunes viz <name>`             → headless viz switch
//! - `clitunes source radio <uuid>`    → headless source switch
//! - `clitunes source local <path>`    → headless source switch
//! - `clitunes status [--json]`        → one-shot status query
//! - `clitunes auth`                   → interactive Spotify auth (headless-safe)

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use clitunes::auto_spawn;
use clitunes::client::reconnect::ReconnectingSession;
use clitunes::client::render_loop::{RenderLoop, RenderLoopConfig};
use clitunes_core::LibraryCategory;
use clitunes_engine::observability;
use clitunes_engine::pcm::spmc_ring::ShmRegion;
use clitunes_engine::proto::events::Event;
use clitunes_engine::proto::verbs::{SourceArg, Verb};
use clitunes_engine::tui::persistence::{
    default_state_path, load_state, save_state, Recovery, State, SOURCE_RADIO,
};

fn main() -> Result<()> {
    let t0 = Instant::now();
    let mode = CliMode::parse_from_env()?;

    if let CliMode::Help = mode {
        print_help();
        return Ok(());
    }

    if let CliMode::Auth = mode {
        return run_auth();
    }

    // TUI modes must log to a file — stderr shares the terminal and
    // would corrupt the visualiser output. When stdout is not a terminal
    // (CI / piped), stderr is safe because there's no terminal to corrupt.
    let tui_mode = matches!(mode, CliMode::FullTui(_) | CliMode::Pane { .. });
    if tui_mode && std::io::stdout().is_terminal() {
        let log_path = std::env::temp_dir().join("clitunes-tui.log");
        observability::init_tracing_to_file("clitunes", &log_path)?;
    } else {
        observability::init_tracing("clitunes")?;
    }

    let app_stop = Arc::new(AtomicBool::new(false));
    install_signal_handler(Arc::clone(&app_stop))?;

    let connected = auto_spawn::connect_or_spawn().context("connect to daemon")?;
    let t_daemon = t0.elapsed();
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

    match mode {
        CliMode::FullTui(cli) => {
            let startup = if cli.measure_startup {
                Some(StartupTimings {
                    t0,
                    daemon_connected: t_daemon,
                })
            } else {
                None
            };
            rt.block_on(run_full_tui(cli, socket_path, app_stop, startup))
        }
        CliMode::Pane { name, viz } => rt.block_on(run_pane(name, viz, socket_path, app_stop)),
        CliMode::Headless(verb) => rt.block_on(run_headless(verb, &socket_path)),
        CliMode::HeadlessBrowse(verb) => rt.block_on(run_headless_browse(verb, &socket_path)),
        CliMode::StatusJson => rt.block_on(run_status_json(&socket_path)),
        CliMode::Help | CliMode::Auth => unreachable!(),
    }
}

struct StartupTimings {
    t0: Instant,
    daemon_connected: Duration,
}

// ─── dispatch: auth ───────────────────────────────────────────────

fn run_auth() -> Result<()> {
    use clitunes_engine::sources::spotify::{default_credentials_path, load_or_authenticate};

    let cred_path = default_credentials_path()
        .ok_or_else(|| anyhow::anyhow!("cannot determine config directory"))?;

    match load_or_authenticate(&cred_path) {
        Ok(_) => {
            eprintln!("Spotify credentials saved to {}", cred_path.display());
            Ok(())
        }
        Err(e) => {
            eprintln!("Spotify authentication failed: {e}");
            Err(e)
        }
    }
}

// ─── dispatch: headless verb ───────────────────────────────────────

async fn run_headless(verb: Verb, socket_path: &std::path::Path) -> Result<()> {
    clitunes::client::headless::dispatch(socket_path, verb).await
}

async fn run_headless_browse(verb: Verb, socket_path: &std::path::Path) -> Result<()> {
    clitunes::client::headless::dispatch_browse(socket_path, verb).await
}

// ─── dispatch: status --json ───────────────────────────────────────

async fn run_status_json(socket_path: &std::path::Path) -> Result<()> {
    clitunes::client::status_json::run(socket_path).await
}

// ─── dispatch: pane mode ───────────────────────────────────────────

async fn run_pane(
    pane_name: String,
    viz_name: Option<String>,
    socket_path: PathBuf,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut session = ReconnectingSession::connect(socket_path)
        .await
        .context("connect control session")?;

    session.request_status().await?;

    let (shm_name, sample_rate) = wait_pcm_tap(&mut session).await?;

    let (_region, consumer) = ShmRegion::open_consumer(&shm_name)
        .map_err(|e| anyhow::anyhow!("open SPMC consumer '{shm_name}': {e}"))?;

    let (event_tx, event_rx) = std::sync::mpsc::channel::<Event>();
    let bridge_stop = Arc::clone(&stop);
    tokio::spawn(async move {
        loop {
            match session.recv_event().await {
                Some(ev) => {
                    if event_tx.send(ev).is_err() {
                        break;
                    }
                }
                None => break,
            }
            if bridge_stop.load(Ordering::Relaxed) {
                break;
            }
        }
    });

    let pane_stop = Arc::clone(&stop);
    let config = clitunes::client::pane_mode::PaneModeConfig {
        pane_name,
        viz_name,
        consumer: Box::new(consumer),
        sample_rate,
        event_rx,
        stop: pane_stop,
    };

    let result =
        tokio::task::spawn_blocking(move || clitunes::client::pane_mode::run_pane(config)).await?;
    drop(_region);
    result
}

// ─── dispatch: full TUI ────────────────────────────────────────────

async fn run_full_tui(
    cli: TuiArgs,
    socket_path: PathBuf,
    stop: Arc<AtomicBool>,
    startup: Option<StartupTimings>,
) -> Result<()> {
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

    let mut session = ReconnectingSession::connect(socket_path)
        .await
        .context("connect control session")?;

    session.request_status().await?;

    let (shm_name, sample_rate) = wait_pcm_tap(&mut session).await?;
    let t_pcm_tap = startup.as_ref().map(|s| s.t0.elapsed());

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
        tracing::debug!(target: "clitunes", "bridge task started");
        loop {
            tokio::select! {
                event = session.recv_event() => {
                    match event {
                        Some(ev) => {
                            if event_tx.send(ev).is_err() {
                                tracing::warn!(target: "clitunes", "bridge: event_tx closed");
                                break;
                            }
                        }
                        None => {
                            tracing::warn!(target: "clitunes", "bridge: session returned None (daemon disconnected)");
                            break;
                        }
                    }
                }
                verb = verb_rx.recv() => {
                    match verb {
                        Some(v) => {
                            tracing::info!(target: "clitunes", verb = ?v, "bridge: forwarding verb to daemon");
                            if let Verb::Source(SourceArg::Radio { ref uuid }) = v {
                                persist_state_best_effort(
                                    uuid,
                                    bridge_state_path.as_deref(),
                                );
                            }
                            if let Err(e) = session.send_verb(v).await {
                                tracing::error!(target: "clitunes", error = %e, "bridge: send_verb failed");
                            }
                        }
                        None => {
                            tracing::info!(target: "clitunes", "bridge: verb_rx closed");
                            break;
                        }
                    }
                }
            }
            if bridge_stop.load(Ordering::Relaxed) {
                break;
            }
        }
        tracing::info!(target: "clitunes", "bridge task exited");
    });

    if let Some(ref s) = startup {
        let t_render_ready = s.t0.elapsed();
        eprintln!("startup\tdaemon_ms\tpcm_tap_ms\trender_ready_ms");
        eprintln!(
            "clitunes\t{}\t{}\t{}",
            s.daemon_connected.as_millis(),
            t_pcm_tap.unwrap_or_default().as_millis(),
            t_render_ready.as_millis(),
        );
    }

    let render_stop = Arc::clone(&stop);
    let render_handle = tokio::task::spawn_blocking(move || {
        let mut render = RenderLoop::new(RenderLoopConfig {
            consumer: Box::new(consumer),
            sample_rate,
            event_rx,
            verb_tx,
            stop: render_stop,
            measure_startup: startup.is_some(),
        });
        render.run()
    });

    let result = render_handle.await?;
    drop(_region);
    result
}

async fn wait_pcm_tap(session: &mut ReconnectingSession) -> Result<(String, u32)> {
    for _ in 0..50 {
        if let Some(Event::PcmTap {
            shm_name,
            sample_rate,
            ..
        }) = session.recv_event_timeout(Duration::from_millis(100)).await
        {
            return Ok((shm_name, sample_rate));
        }
    }
    anyhow::bail!("daemon did not send PcmTap within 5s")
}

fn persist_state_best_effort(uuid: &str, path: Option<&std::path::Path>) {
    let Some(path) = path else { return };
    let state = State {
        last_station_uuid: Some(uuid.to_string()),
        last_station_name: None,
        last_source: Some(SOURCE_RADIO.to_string()),
        last_visualiser: None,
        last_layout: None,
        last_spotify_uri: None,
    };
    if let Err(e) = save_state(&state, path) {
        tracing::warn!(target: "clitunes", error = %e, "save state failed");
    }
}

// ─── boot intent (full TUI only) ──────────────────────────────────

enum BootIntent {
    FirstRunShowPicker,
    AutoResume { uuid: String, name: Option<String> },
    CliStation(String),
    CliTone,
}

impl BootIntent {
    fn from_cli_and_state(cli: &TuiArgs, state: Option<&State>) -> Self {
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

// ─── CLI parsing ───────────────────────────────────────────────────

#[derive(Copy, Clone)]
enum SourceChoice {
    Tone,
    Radio,
    Auto,
}

#[derive(Clone)]
struct TuiArgs {
    source: SourceChoice,
    station: Option<String>,
    measure_startup: bool,
}

enum CliMode {
    Help,
    FullTui(TuiArgs),
    Pane {
        name: String,
        viz: Option<String>,
    },
    Headless(Verb),
    /// Browse verbs (search / browse-library / browse-playlist). Uses a
    /// different dispatcher that prints the result event as JSON before
    /// the CommandResult.
    HeadlessBrowse(Verb),
    StatusJson,
    Auth,
}

impl CliMode {
    fn parse_from_env() -> Result<Self> {
        let args: Vec<String> = std::env::args().skip(1).collect();

        if args.is_empty() {
            return Ok(CliMode::FullTui(TuiArgs {
                source: SourceChoice::Auto,
                station: None,
                measure_startup: false,
            }));
        }

        // Check for --measure-startup anywhere (consumed before subcommand dispatch).
        let measure_startup = args.iter().any(|a| a == "--measure-startup");
        let args: Vec<String> = args
            .into_iter()
            .filter(|a| a != "--measure-startup")
            .collect();

        if args.is_empty() {
            return Ok(CliMode::FullTui(TuiArgs {
                source: SourceChoice::Auto,
                station: None,
                measure_startup,
            }));
        }

        // Check for --help anywhere
        if args.iter().any(|a| a == "-h" || a == "--help") {
            return Ok(CliMode::Help);
        }

        // Check for --pane
        if let Some(pos) = args.iter().position(|a| a == "--pane") {
            let name = args
                .get(pos + 1)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "--pane requires a name: {}",
                        clitunes::client::pane_mode::PANE_NAMES.join(", ")
                    )
                })?
                .clone();
            clitunes::client::pane_mode::validate_pane_name(&name)?;

            let mut viz = None;
            if let Some(vpos) = args.iter().position(|a| a == "--viz") {
                viz = Some(
                    args.get(vpos + 1)
                        .ok_or_else(|| anyhow::anyhow!("--viz requires a visualiser name"))?
                        .clone(),
                );
            }

            return Ok(CliMode::Pane { name, viz });
        }

        // Subcommand dispatch
        match args[0].as_str() {
            "auth" => Ok(CliMode::Auth),
            "play" => Ok(CliMode::Headless(Verb::Play)),
            "pause" => Ok(CliMode::Headless(Verb::Pause)),
            "next" => Ok(CliMode::Headless(Verb::Next)),
            "prev" => Ok(CliMode::Headless(Verb::Prev)),
            "volume" => {
                let level_str = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("volume requires a level (0-100)"))?;
                let level: u8 = level_str
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid volume level: {level_str}"))?;
                if level > 100 {
                    anyhow::bail!("volume must be 0-100, got {level}");
                }
                Ok(CliMode::Headless(Verb::Volume { level }))
            }
            "viz" => {
                let name = args
                    .get(1)
                    .ok_or_else(|| {
                        anyhow::anyhow!("viz requires a name (e.g. auralis, cascade, tideline)")
                    })?
                    .clone();
                Ok(CliMode::Headless(Verb::Viz { name }))
            }
            "source" => {
                let kind = args.get(1).ok_or_else(|| {
                    anyhow::anyhow!("source requires: radio <uuid> | local <path>")
                })?;
                match kind.as_str() {
                    "radio" => {
                        let uuid = args
                            .get(2)
                            .ok_or_else(|| anyhow::anyhow!("source radio requires a station UUID"))?
                            .clone();
                        Ok(CliMode::Headless(Verb::Source(SourceArg::Radio { uuid })))
                    }
                    "local" => {
                        let path = args
                            .get(2)
                            .ok_or_else(|| anyhow::anyhow!("source local requires a file path"))?
                            .clone();
                        Ok(CliMode::Headless(Verb::Source(SourceArg::Local { path })))
                    }
                    other if other.starts_with("spotify:") => {
                        Ok(CliMode::Headless(Verb::Source(SourceArg::Spotify {
                            uri: other.to_string(),
                        })))
                    }
                    other => anyhow::bail!(
                        "unknown source type: {other}. Expected: radio, local, spotify:<uri>"
                    ),
                }
            }
            "status" => {
                if args.get(1).map(|a| a.as_str()) == Some("--json") || args.len() == 1 {
                    Ok(CliMode::StatusJson)
                } else {
                    anyhow::bail!("usage: clitunes status [--json]")
                }
            }
            "search" => {
                let query = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("search requires a query string"))?
                    .clone();
                let limit = args.get(2).and_then(|s| s.parse::<u32>().ok());
                Ok(CliMode::HeadlessBrowse(Verb::Search { query, limit }))
            }
            "browse" => {
                let category = args.get(1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "browse requires a category: saved_tracks | saved_albums | playlists | recently_played"
                    )
                })?;
                let category = match category.as_str() {
                    "saved_tracks" => LibraryCategory::SavedTracks,
                    "saved_albums" => LibraryCategory::SavedAlbums,
                    "playlists" => LibraryCategory::Playlists,
                    "recently_played" => LibraryCategory::RecentlyPlayed,
                    other => anyhow::bail!(
                        "unknown browse category: {other}. Expected: saved_tracks, saved_albums, playlists, recently_played"
                    ),
                };
                let limit = args.get(2).and_then(|s| s.parse::<u32>().ok());
                Ok(CliMode::HeadlessBrowse(Verb::BrowseLibrary {
                    category,
                    limit,
                }))
            }
            "browse-playlist" => {
                let id = args
                    .get(1)
                    .ok_or_else(|| {
                        anyhow::anyhow!("browse-playlist requires a playlist id or URI")
                    })?
                    .clone();
                let limit = args.get(2).and_then(|s| s.parse::<u32>().ok());
                Ok(CliMode::HeadlessBrowse(Verb::BrowsePlaylist { id, limit }))
            }
            // Legacy full-TUI flags
            _ => {
                let mut source = SourceChoice::Auto;
                let mut station: Option<String> = None;
                let mut i = 0;
                while i < args.len() {
                    match args[i].as_str() {
                        "--source" => {
                            let value = args.get(i + 1).ok_or_else(|| {
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
                            i += 2;
                        }
                        "--station" => {
                            station = Some(
                                args.get(i + 1)
                                    .ok_or_else(|| anyhow::anyhow!("--station requires a UUID"))?
                                    .clone(),
                            );
                            i += 2;
                        }
                        other => anyhow::bail!("unknown argument: {other}"),
                    }
                }
                Ok(CliMode::FullTui(TuiArgs {
                    source,
                    station,
                    measure_startup,
                }))
            }
        }
    }
}

fn print_help() {
    println!(
        "\
clitunes — the Ghostty of TUI music apps

USAGE:
    clitunes                                Full TUI with picker + visualiser carousel
    clitunes --pane <name> [--viz <viz>]    Standalone pane (visualiser, now-playing, mini-spectrum)
    clitunes play|pause|next|prev           Headless playback control
    clitunes volume <0-100>                 Set volume
    clitunes viz <name>                     Switch visualiser
    clitunes source radio <uuid>            Switch to radio station
    clitunes source local <path>            Play local file/directory
    clitunes search <query> [limit]         Search Spotify; prints SearchResults JSON
    clitunes browse <category> [limit]      List saved library (saved_tracks | saved_albums | playlists | recently_played)
    clitunes browse-playlist <id> [limit]   List tracks in a Spotify playlist
    clitunes status [--json]                Print current status as JSON
    clitunes auth                           Authenticate with Spotify (headless-safe)

PANE NAMES:
    visualiser      Fullscreen visualiser (default: auralis, override with --viz)
    now-playing     Track info strip (1-3 rows)
    mini-spectrum   Unicode block spectrum bars (1 row, for status lines)

FULL TUI OPTIONS:
    --source <auto|tone|radio>  Audio source (default: auto — resume last or show picker)
    --station <uuid>            Radio station UUID (used with --source radio)
    --measure-startup           Print startup timing breakdown to stderr and exit after first frame

KEYS (full TUI and visualiser pane):
    \u{2191} / \u{2193}       move picker selection (or j / k)
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
