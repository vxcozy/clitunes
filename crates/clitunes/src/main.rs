//! clitunes — slice-2 driver.
//!
//! Pipeline: `SourceManager` drives either `ToneSource` or `RadioSource`
//! into `PcmRing`; the main thread pulls PCM through `FftTap`, paints
//! the active `Visualiser` into a `CellGrid`, optionally overlays the
//! curated-station picker, and flushes truecolor ANSI to stdout.
//!
//! Unit 8 added:
//! - **Persistence**: `~/.config/clitunes/state.toml` (chmod 0600,
//!   parent dir 0700, atomic write). Loaded on boot for auto-resume,
//!   saved whenever the user picks a station.
//! - **Curated picker**: first-run modal listing 12 taste-neutral
//!   stations (filled during slice-2 polish). Painted directly into
//!   the cell grid — no ratatui dependency.
//! - **Source manager**: can hot-swap between tone and radio without
//!   tearing down the render loop, so auto-resume and `s`-initiated
//!   re-picks are seamless.

use std::io::{self, BufWriter, Read};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use clitunes_core::{PcmFormat, Station};
use clitunes_engine::audio::{FftTap, PcmRing};
use clitunes_engine::observability;
use clitunes_engine::sources::radio::{
    resolve_station_blocking, RadioConfig, RadioSource,
};
use clitunes_engine::sources::{tone_source::ToneSource, Source};
use clitunes_engine::tui::persistence::{
    default_state_path, load_state, save_state, Recovery, State, SOURCE_RADIO,
};
use clitunes_engine::tui::picker::{
    key_from_bytes, load_curated, paint_picker, CuratedLoadOutcome, PickerAction, PickerKey,
    PickerState,
};
use clitunes_engine::visualiser::{
    AnsiWriter, Auralis, CellGrid, Metaballs, Plasma, Ripples, Starfield, TuiContext, Tunnel,
    Visualiser,
};

const FFT_SIZE: usize = 2048;
const RING_FRAMES: usize = 48_000; // one second @ 48 kHz
const TONE_BLOCK: usize = 1024;
/// ~30 fps. Bars are smoothed so the eye doesn't need 60 Hz, and halving
/// the frame rate halves everything downstream (FFT, paint, stdout).
const TARGET_FRAME: Duration = Duration::from_millis(33);

/// Fallback cell rect when TIOCGWINSZ fails (e.g. piped output) or the
/// terminal reports zero dims. 80×24 is the classic VT100 default.
const FALLBACK_COLS: u16 = 80;
const FALLBACK_ROWS: u16 = 24;

fn main() -> Result<()> {
    observability::init_tracing("clitunes")?;

    let cli = CliArgs::parse_from_env()?;

    let (cells_w, cells_h) = visualiser_cell_rect();
    tracing::info!(
        target: "clitunes",
        cells_w,
        cells_h,
        source = cli.source.as_str(),
        "boot: source → visualiser carousel → ansi"
    );

    let app_stop = Arc::new(AtomicBool::new(false));
    install_signal_handler(Arc::clone(&app_stop))?;

    const VISUALISER_COUNT: usize = 6;
    let viz_index = Arc::new(AtomicUsize::new(0));

    // Load persisted state early so we can decide on first-run picker
    // vs auto-resume before any audio starts. A missing or corrupt
    // file routes us into the picker flow — never fatal.
    let state_path = default_state_path();
    let loaded_state = state_path
        .as_ref()
        .map(|p| load_state(p))
        .transpose()?
        .unwrap_or(Recovery::Missing);
    let previous_state = match &loaded_state {
        Recovery::Loaded(s) => Some(s.clone()),
        Recovery::Missing => None,
        Recovery::Corrupt(reason) => {
            tracing::warn!(target: "clitunes", %reason, "state.toml corrupt; starting fresh");
            if let Some(p) = state_path.as_ref() {
                let _ = std::fs::remove_file(p);
            }
            None
        }
    };

    // Curated seed list for the picker. Override file is optional.
    let (curated, curated_outcome) = load_curated(None);
    match &curated_outcome {
        CuratedLoadOutcome::BakedNoOverride => {}
        CuratedLoadOutcome::OverrideLoaded(p) => {
            tracing::info!(target: "clitunes", path = %p.display(), "curated stations: override loaded");
        }
        CuratedLoadOutcome::OverrideRejected { path, reason } => {
            tracing::warn!(
                target: "clitunes",
                path = %path.display(),
                %reason,
                "curated stations: override rejected, using baked list"
            );
        }
    }

    // Raw mode + stdin reader for keypress handling. Restored on Drop
    // so the user doesn't end up with a wedged terminal if the loop
    // panics.
    let _raw = RawStdin::enable().context("enable raw stdin")?;
    let (key_tx, key_rx) = mpsc::channel::<AppKey>();
    spawn_keypress_thread(Arc::clone(&app_stop), key_tx);

    let format = PcmFormat::STUDIO;
    let ring = PcmRing::new(format, RING_FRAMES);

    // Resolve-worker → main loop channel. Used for both the CLI
    // `--station <uuid>` path and the auto-resume path from state.toml.
    let (resolve_tx, resolve_rx) = mpsc::channel::<ResolveResult>();

    let mut manager = SourceManager::new(ring.clone(), format);

    // Boot strategy:
    // 1. Always start the calibration tone so the user hears something
    //    within the first frame even if station resolution is slow.
    // 2. If CLI forced a source or state.toml carries a last station,
    //    kick off resolve in the background and swap when it lands.
    // 3. Otherwise show the picker overlay floating over Auralis.
    manager.start_tone(Arc::clone(&app_stop))?;

    let mut picker_state = PickerState::new(&curated, 0);

    let boot = BootIntent::from_cli_and_state(&cli, previous_state.as_ref());
    match &boot {
        BootIntent::FirstRunShowPicker => {
            tracing::info!(target: "clitunes", "boot: first run — showing picker");
            picker_state.show();
        }
        BootIntent::AutoResume { uuid, name } => {
            tracing::info!(
                target: "clitunes",
                %uuid,
                name = name.as_deref().unwrap_or("?"),
                "boot: auto-resuming last station"
            );
            picker_state.hide();
            spawn_resolve(uuid.clone(), resolve_tx.clone());
        }
        BootIntent::CliStation(uuid) => {
            tracing::info!(target: "clitunes", %uuid, "boot: resolving CLI-supplied station");
            picker_state.hide();
            spawn_resolve(uuid.clone(), resolve_tx.clone());
        }
        BootIntent::CliTone => {
            tracing::info!(target: "clitunes", "boot: tone only (--source tone)");
            picker_state.hide();
        }
    }

    let mut visualisers: Vec<Box<dyn Visualiser>> = vec![
        Box::new(Plasma::new()),
        Box::new(Ripples::new()),
        Box::new(Tunnel::new()),
        Box::new(Metaballs::new()),
        Box::new(Starfield::new()),
        Box::new(Auralis::new()),
    ];
    debug_assert_eq!(visualisers.len(), VISUALISER_COUNT);
    let mut active_idx: usize = 0;
    tracing::info!(
        target: "clitunes",
        visualiser = visualisers[active_idx].id().as_str(),
        "starting visualiser"
    );

    let mut fft = FftTap::new(FFT_SIZE);
    let reader = ring.reader();
    let mut grid = CellGrid::new(cells_w, cells_h);

    // 64 KiB stdout buffer → one full frame ships in a single write syscall.
    let stdout = io::stdout();
    let mut writer = AnsiWriter::new(BufWriter::with_capacity(64 * 1024, stdout.lock()));
    writer.clear_screen()?;
    writer.hide_cursor()?;
    writer.flush()?;

    let mut frame_idx: u64 = 0;
    let loop_start = Instant::now();

    while !app_stop.load(Ordering::Relaxed) {
        let frame_start = Instant::now();

        // Drain any resolved stations from background workers first so
        // a pending auto-resume can swap in as soon as it's ready.
        while let Ok(result) = resolve_rx.try_recv() {
            match result {
                ResolveResult::Ok(station) => {
                    if let Err(e) = manager.start_radio(station.clone(), Arc::clone(&app_stop)) {
                        tracing::error!(target: "clitunes", error = %e, "failed to start radio source");
                    } else {
                        let new_state = State {
                            last_station_uuid: Some(station.uuid.as_str().to_string()),
                            last_station_name: Some(station.name.clone()),
                            last_source: Some(SOURCE_RADIO.to_string()),
                            last_visualiser: Some(
                                visualisers[active_idx].id().as_str().to_string(),
                            ),
                            last_layout: None,
                        };
                        persist_state_best_effort(&new_state, state_path.as_deref());
                    }
                }
                ResolveResult::Err(uuid, err) => {
                    tracing::warn!(target: "clitunes", %uuid, error = %err, "station resolve failed; showing picker");
                    picker_state.banner = Some(format!(
                        "Last station ({uuid}) could not be loaded — pick another."
                    ));
                    picker_state.show();
                }
            }
        }

        // Drain keypresses and dispatch.
        while let Ok(key) = key_rx.try_recv() {
            match key {
                AppKey::Picker(pk) => {
                    let action = picker_state.handle_key(pk);
                    match action {
                        PickerAction::Pick(slot) => {
                            if let Some(station) = curated.stations.iter().find(|s| s.slot == slot)
                            {
                                picker_state.hide();
                                spawn_resolve_from_url(station.url.clone(), resolve_tx.clone());
                            }
                        }
                        PickerAction::Quit => {
                            app_stop.store(true, Ordering::SeqCst);
                        }
                        PickerAction::Moved | PickerAction::Hide | PickerAction::Ignored => {}
                    }
                }
                AppKey::VizNext => {
                    let cur = viz_index.load(Ordering::Relaxed);
                    viz_index.store((cur + 1) % visualisers.len(), Ordering::Relaxed);
                }
                AppKey::VizPrev => {
                    let cur = viz_index.load(Ordering::Relaxed);
                    viz_index.store(
                        (cur + visualisers.len() - 1) % visualisers.len(),
                        Ordering::Relaxed,
                    );
                }
            }
        }

        let requested = viz_index.load(Ordering::Relaxed) % visualisers.len();
        if requested != active_idx {
            active_idx = requested;
            let _ = writer.clear_screen();
            tracing::info!(
                target: "clitunes",
                visualiser = visualisers[active_idx].id().as_str(),
                "switched visualiser"
            );
        }

        let snapshot = fft.snapshot(&reader, format.sample_rate);

        {
            let mut ctx = TuiContext { grid: &mut grid };
            visualisers[active_idx].render_tui(&mut ctx, &snapshot);
        }

        if picker_state.visible {
            let _ = paint_picker(&mut grid, &curated, picker_state.selected);
        }

        writer.write_frame(&grid)?;
        writer.flush()?;

        frame_idx += 1;
        if frame_idx.is_multiple_of(60) {
            tracing::debug!(target: "clitunes", frame_idx, "frame stats");
        }

        let elapsed = frame_start.elapsed();
        if elapsed < TARGET_FRAME {
            thread::sleep(TARGET_FRAME - elapsed);
        }
    }

    // Leave the terminal grid clean on shutdown so the user's prompt isn't
    // sitting on top of half-painted cells.
    let _ = writer.reset();
    let _ = writer.clear_screen();
    let _ = writer.show_cursor();
    let _ = writer.flush();

    tracing::info!(
        target: "clitunes",
        frames = frame_idx,
        uptime_secs = loop_start.elapsed().as_secs_f32(),
        "shutdown"
    );

    manager.shutdown();
    Ok(())
}

/// Persist state.toml, swallowing errors with a warning. State is
/// best-effort: a write failure (disk full, permission denied) should
/// never break playback, just a warning in the log.
fn persist_state_best_effort(state: &State, path: Option<&std::path::Path>) {
    let Some(path) = path else { return };
    if let Err(e) = save_state(state, path) {
        tracing::warn!(target: "clitunes", error = %e, path = %path.display(), "save state failed");
    }
}

/// Background worker that resolves a station UUID via `resolve_station_blocking`
/// and reports back to the main loop.
fn spawn_resolve(uuid: String, tx: mpsc::Sender<ResolveResult>) {
    thread::Builder::new()
        .name("clitunes-resolve".into())
        .spawn(move || {
            let result = match resolve_station_blocking(&uuid) {
                Ok(station) => ResolveResult::Ok(station),
                Err(e) => ResolveResult::Err(uuid, e.to_string()),
            };
            let _ = tx.send(result);
        })
        .expect("spawn resolve thread");
}

/// Resolve a picker slot's URL into a `Station`. Curated URLs are
/// either direct stream URLs (future) or `radiobrowser:<uuid>`
/// sentinels (normal). Placeholders resolved via the sentinel path.
fn spawn_resolve_from_url(url: String, tx: mpsc::Sender<ResolveResult>) {
    if let Some(uuid) = url.strip_prefix("radiobrowser:") {
        spawn_resolve(uuid.to_string(), tx);
        return;
    }
    // Direct URL path — for now we can't do much with it until the
    // curated list ships real radio-browser UUIDs. Report a failure
    // so the picker stays visible and logs a hint.
    thread::spawn(move || {
        let _ = tx.send(ResolveResult::Err(
            url.clone(),
            "curated slot is a placeholder; fill during slice-2 polish".into(),
        ));
    });
}

/// Outcome of a background resolve worker.
enum ResolveResult {
    Ok(Station),
    Err(String, String),
}

/// Owner of the active source thread. Lets the main loop hot-swap
/// between tone and radio without tearing down the ring or the
/// render loop. One instance per process; not `Send`.
struct SourceManager {
    ring: PcmRing,
    format: PcmFormat,
    current: Option<ActiveSource>,
}

struct ActiveSource {
    /// Flag flipped on swap/shutdown. The underlying source's `run`
    /// loop polls this (or a mirrored inner flag, for the radio
    /// source) and exits cleanly.
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
    kind: ActiveKind,
}

#[derive(Clone, Debug)]
enum ActiveKind {
    Tone,
    #[allow(dead_code)] // captured for tracing/debug, not read at runtime
    Radio {
        uuid: String,
        name: String,
    },
}

impl SourceManager {
    fn new(ring: PcmRing, format: PcmFormat) -> Self {
        Self {
            ring,
            format,
            current: None,
        }
    }

    fn start_tone(&mut self, app_stop: Arc<AtomicBool>) -> Result<()> {
        self.stop_current();
        let source_stop = merged_stop(&app_stop);
        let watcher = spawn_stop_mirror(Arc::clone(&app_stop), Arc::clone(&source_stop));
        let mut writer = self.ring.writer();
        let format = self.format;
        let stop_for_thread = Arc::clone(&source_stop);
        let handle = thread::Builder::new()
            .name("clitunes-tone".into())
            .spawn(move || {
                let mut source = ToneSource::new(format, TONE_BLOCK);
                source.run(&mut writer, &stop_for_thread);
                drop(watcher);
            })?;
        self.current = Some(ActiveSource {
            stop: source_stop,
            handle,
            kind: ActiveKind::Tone,
        });
        Ok(())
    }

    fn start_radio(&mut self, station: Station, app_stop: Arc<AtomicBool>) -> Result<()> {
        self.stop_current();
        let source_stop = merged_stop(&app_stop);
        let watcher = spawn_stop_mirror(Arc::clone(&app_stop), Arc::clone(&source_stop));
        let mut writer = self.ring.writer();
        let format = self.format;
        let radio_config = RadioConfig::new(station.clone(), format);
        let mut radio = RadioSource::new(radio_config).context("build radio source")?;
        let stop_for_thread = Arc::clone(&source_stop);
        let handle = thread::Builder::new()
            .name("clitunes-radio".into())
            .spawn(move || {
                radio.run(&mut writer, &stop_for_thread);
                drop(watcher);
            })?;
        self.current = Some(ActiveSource {
            stop: source_stop,
            handle,
            kind: ActiveKind::Radio {
                uuid: station.uuid.as_str().to_string(),
                name: station.name.clone(),
            },
        });
        Ok(())
    }

    fn stop_current(&mut self) {
        if let Some(active) = self.current.take() {
            tracing::info!(target: "clitunes", kind = ?active.kind, "stopping active source");
            active.stop.store(true, Ordering::SeqCst);
            let _ = active.handle.join();
        }
    }

    fn shutdown(&mut self) {
        self.stop_current();
    }
}

/// Build a fresh per-source stop flag that starts `false`. The
/// `app_stop` is mirrored into it via [`spawn_stop_mirror`] so a quit
/// keystroke reaches any running source within one poll tick.
fn merged_stop(_app_stop: &Arc<AtomicBool>) -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Mirror thread: polls `app_stop` every 50ms and flips `source_stop`
/// when it trips. Returned handle is dropped by the source thread on
/// exit so the watcher goes away naturally.
fn spawn_stop_mirror(app_stop: Arc<AtomicBool>, source_stop: Arc<AtomicBool>) -> StopMirror {
    let local_stop = Arc::new(AtomicBool::new(false));
    let watcher_stop = Arc::clone(&local_stop);
    let handle = thread::Builder::new()
        .name("clitunes-stop-mirror".into())
        .spawn(move || {
            while !watcher_stop.load(Ordering::Relaxed) {
                if app_stop.load(Ordering::Relaxed) {
                    source_stop.store(true, Ordering::SeqCst);
                    return;
                }
                if source_stop.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        })
        .expect("spawn stop mirror");
    StopMirror {
        stop: local_stop,
        handle: Some(handle),
    }
}

/// RAII wrapper so the mirror thread is torn down when the source
/// thread finishes. Flipping `stop` lets the mirror's poll loop exit
/// on its next tick.
struct StopMirror {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for StopMirror {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Query the controlling terminal for its size in cells via TIOCGWINSZ.
/// Returns a sane fallback when stdout isn't a tty.
fn visualiser_cell_rect() -> (u16, u16) {
    let (term_cols, term_rows) = terminal_size().unwrap_or((FALLBACK_COLS, FALLBACK_ROWS));
    // Leave a one-cell margin on the right and two rows at the bottom so
    // the prompt returns cleanly after shutdown.
    let cols = term_cols.saturating_sub(1).max(20);
    let rows = term_rows.saturating_sub(2).max(10);
    (cols, rows)
}

fn terminal_size() -> Option<(u16, u16)> {
    let stdout = io::stdout();
    let fd = stdout.as_raw_fd();
    unsafe {
        let mut ws: MaybeUninit<libc::winsize> = MaybeUninit::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ, ws.as_mut_ptr()) != 0 {
            return None;
        }
        let ws = ws.assume_init();
        if ws.ws_col == 0 || ws.ws_row == 0 {
            return None;
        }
        Some((ws.ws_col, ws.ws_row))
    }
}

/// Stdin raw-mode guard. Puts the controlling tty into cbreak so we can
/// read individual keypresses without echoing them, and restores the prior
/// termios settings on Drop. The Drop guarantee means the user's terminal
/// stays usable even if the render loop panics.
struct RawStdin {
    fd: libc::c_int,
    saved: libc::termios,
}

impl RawStdin {
    fn enable() -> Result<Self> {
        let fd = io::stdin().as_raw_fd();
        unsafe {
            let mut saved: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut saved) != 0 {
                return Err(anyhow::anyhow!("tcgetattr failed: stdin is not a tty"));
            }
            let mut raw = saved;
            // cbreak: no echo, no canonical line buffering, but keep signals
            // (so Ctrl-C still works as an emergency exit).
            raw.c_lflag &= !(libc::ECHO | libc::ICANON);
            raw.c_iflag &= !(libc::IXON | libc::ICRNL);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 1; // 100ms read timeout
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
                return Err(anyhow::anyhow!("tcsetattr failed"));
            }
            Ok(Self { fd, saved })
        }
    }
}

impl Drop for RawStdin {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
        }
    }
}

/// Event type on the keypress channel. Picker keys are forwarded to
/// the picker state machine; viz keys cycle the visualiser ring even
/// when the picker is hidden.
enum AppKey {
    Picker(PickerKey),
    VizNext,
    VizPrev,
}

/// Background thread that reads stdin one byte at a time, assembles
/// escape sequences, and pushes recognised events onto `tx`. Also
/// flips the app-level stop flag on a recognised quit key so the main
/// loop exits promptly even if the channel receiver is lagging.
fn spawn_keypress_thread(stop: Arc<AtomicBool>, tx: mpsc::Sender<AppKey>) {
    thread::Builder::new()
        .name("clitunes-keypress".into())
        .spawn(move || {
            let mut stdin = io::stdin();
            let mut buf = [0u8; 8];
            let mut pending: Vec<u8> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                match stdin.read(&mut buf) {
                    Ok(0) => {
                        // VTIME=1 → read returns 0 after the timeout
                        // with no bytes available. If we were mid-escape,
                        // treat a bare ESC as an escape key.
                        if pending == [0x1b] {
                            let _ = tx.send(AppKey::Picker(PickerKey::Escape));
                            pending.clear();
                        }
                    }
                    Ok(n) => {
                        for &b in &buf[..n] {
                            pending.push(b);
                            if let Some(ev) = classify_pending(&pending) {
                                match ev {
                                    AppKey::Picker(PickerKey::Quit) => {
                                        stop.store(true, Ordering::SeqCst);
                                        let _ = tx.send(ev);
                                        return;
                                    }
                                    _ => {
                                        let _ = tx.send(ev);
                                    }
                                }
                                pending.clear();
                            } else if pending.len() >= 3 {
                                // Unknown 3-byte sequence — drop it and
                                // keep going rather than stalling.
                                pending.clear();
                            }
                        }
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        })
        .expect("spawn keypress thread");
}

/// Inspect the byte accumulator and decide whether it's a complete
/// recognisable key. Returns `None` if the bytes might still grow
/// into something meaningful (partial escape sequence).
fn classify_pending(pending: &[u8]) -> Option<AppKey> {
    match pending {
        // Bare ESC could still grow into an arrow key — the caller
        // handles the timeout-flush case explicitly.
        [0x1b] => None,
        [0x1b, b'['] => None,
        [0x1b, b'[', _] => {
            let k = key_from_bytes(pending);
            if k == PickerKey::Other {
                None
            } else {
                Some(AppKey::Picker(k))
            }
        }
        [b'n'] | [b'N'] => Some(AppKey::VizNext),
        [b'p'] | [b'P'] => Some(AppKey::VizPrev),
        single if single.len() == 1 => {
            let k = key_from_bytes(single);
            if k == PickerKey::Other {
                None
            } else {
                Some(AppKey::Picker(k))
            }
        }
        _ => None,
    }
}

/// Which source feeds the PCM ring. `Tone` is the slice-1 calibration
/// tone; `Radio` drives the symphonia pipeline against an internet
/// radio station selected by UUID or via the curated picker.
#[derive(Copy, Clone, Debug)]
enum SourceChoice {
    Tone,
    Radio,
    Auto,
}

impl SourceChoice {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tone => "tone",
            Self::Radio => "radio",
            Self::Auto => "auto",
        }
    }
}

/// Resolved boot strategy once CLI + state have both been consulted.
enum BootIntent {
    /// No CLI override, no saved state → show the picker modal.
    FirstRunShowPicker,
    /// Saved state carries a last station → resume it silently.
    AutoResume { uuid: String, name: Option<String> },
    /// CLI forced `--source radio --station <uuid>`.
    CliStation(String),
    /// CLI forced `--source tone`.
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

/// Tiny hand-rolled CLI: no clap, no structopt. Supports exactly the
/// flags Slice 2 needs today. If we grow more surface (local files,
/// spotify, scrobbling) this becomes the right time to pull in clap, but
/// for three flags the manual parser keeps compile time and the
/// dependency graph small.
#[derive(Clone, Debug)]
struct CliArgs {
    source: SourceChoice,
    station: Option<String>,
}

impl CliArgs {
    fn parse_from_env() -> Result<Self> {
        let mut source = SourceChoice::Auto;
        let mut station: Option<String> = None;

        let mut args = std::env::args().skip(1).peekable();
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
    // Minimal SIGINT handler via libc. No signal-hook dep for slice 1.
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

    // Watcher thread flips the shared AtomicBool so the main loop can exit.
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

