//! clitunes — slice-1 driver.
//!
//! Pipeline: `ToneSource` → `PcmRing` → `FftTap` → active `Visualiser`
//! (CPU cell grid) → `AnsiWriter` to stdout. The source thread produces
//! PCM, the main thread renders at ~30 fps by painting a `CellGrid` and
//! emitting truecolor ANSI.
//!
//! Plasma is the first visualiser in the carousel because it self-animates
//! and looks alive even when only a calibration tone is playing. `n` / `p`
//! cycle through the ring so every visualiser gets eyeballed in one run.
//! Once Unit 8 lands the picker will replace this raw cycling.

use std::io::{self, BufWriter, Read};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use clitunes_core::PcmFormat;
use clitunes_engine::audio::{FftTap, PcmRing};
use clitunes_engine::observability;
use clitunes_engine::sources::radio::{
    resolve_station_blocking, RadioConfig, RadioSource,
};
use clitunes_engine::sources::{tone_source::ToneSource, Source};
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

    let stop = Arc::new(AtomicBool::new(false));
    install_signal_handler(Arc::clone(&stop))?;

    const VISUALISER_COUNT: usize = 6;
    let viz_index = Arc::new(AtomicUsize::new(0));

    // Raw mode + stdin reader for `q` quit and `n`/`p` visualiser cycling.
    // Restored on Drop so the user doesn't end up with a wedged terminal
    // if the loop panics.
    let _raw = RawStdin::enable().context("enable raw stdin")?;
    spawn_keypress_thread(
        Arc::clone(&stop),
        Arc::clone(&viz_index),
        VISUALISER_COUNT,
    );

    let format = PcmFormat::STUDIO;
    let ring = PcmRing::new(format, RING_FRAMES);

    // Source thread: either the calibration tone (default) or the radio
    // pipeline (`--source radio --station <uuid>`). Both implement the
    // same `Source` trait so the main render loop is unaware of which
    // producer is actually filling the ring.
    let source_stop = Arc::clone(&stop);
    let mut source_writer = ring.writer();
    let source_handle = match cli.source {
        SourceChoice::Tone => thread::Builder::new()
            .name("clitunes-tone".into())
            .spawn(move || {
                let mut source = ToneSource::new(format, TONE_BLOCK);
                source.run(&mut source_writer, &source_stop);
            })?,
        SourceChoice::Radio => {
            let uuid = cli.station.clone().expect(
                "CliArgs::parse_from_env guarantees station is Some when source is Radio",
            );
            tracing::info!(target: "clitunes", %uuid, "resolving station");
            let station = resolve_station_blocking(&uuid)
                .context("resolve station")?;
            tracing::info!(
                target: "clitunes",
                name = %station.name,
                url = %station.url_resolved,
                "station resolved"
            );
            let radio_config = RadioConfig::new(station, format);
            let mut radio_source =
                RadioSource::new(radio_config).context("build radio source")?;
            thread::Builder::new()
                .name("clitunes-radio".into())
                .spawn(move || {
                    radio_source.run(&mut source_writer, &source_stop);
                })?
        }
    };

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

    while !stop.load(Ordering::Relaxed) {
        let frame_start = Instant::now();

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

    let _ = source_handle.join();
    Ok(())
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

/// Background thread that reads stdin one byte at a time. Trips the
/// shared stop flag on `q`, `Q`, ESC, or Ctrl-C, and advances the shared
/// visualiser index on `n` (next) or `p` (prev).
fn spawn_keypress_thread(stop: Arc<AtomicBool>, viz_index: Arc<AtomicUsize>, viz_count: usize) {
    thread::Builder::new()
        .name("clitunes-keypress".into())
        .spawn(move || {
            let mut stdin = io::stdin();
            let mut buf = [0u8; 8];
            while !stop.load(Ordering::Relaxed) {
                match stdin.read(&mut buf) {
                    Ok(0) => {
                        // VTIME=1 → read returns 0 after the timeout with no
                        // bytes available. Loop and re-poll.
                    }
                    Ok(n) => {
                        for &b in &buf[..n] {
                            match b {
                                b'q' | b'Q' | 0x1b | 0x03 => {
                                    stop.store(true, Ordering::SeqCst);
                                    return;
                                }
                                b'n' | b'N' => {
                                    let cur = viz_index.load(Ordering::Relaxed);
                                    viz_index.store((cur + 1) % viz_count, Ordering::Relaxed);
                                }
                                b'p' | b'P' => {
                                    let cur = viz_index.load(Ordering::Relaxed);
                                    viz_index.store(
                                        (cur + viz_count - 1) % viz_count,
                                        Ordering::Relaxed,
                                    );
                                }
                                _ => {}
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

/// Which source feeds the PCM ring. `Tone` is the slice-1 calibration
/// tone; `Radio` drives Unit 7's symphonia pipeline against an internet
/// radio station selected by UUID.
#[derive(Copy, Clone, Debug)]
enum SourceChoice {
    Tone,
    Radio,
}

impl SourceChoice {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tone => "tone",
            Self::Radio => "radio",
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
        let mut source = SourceChoice::Tone;
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
                        anyhow::anyhow!("--source requires a value (tone|radio)")
                    })?;
                    source = match value.as_str() {
                        "tone" => SourceChoice::Tone,
                        "radio" => SourceChoice::Radio,
                        other => {
                            anyhow::bail!("unknown --source '{}': expected tone or radio", other);
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

        if matches!(source, SourceChoice::Radio) && station.is_none() {
            anyhow::bail!("--source radio requires --station <uuid>");
        }

        Ok(Self { source, station })
    }
}

fn print_help() {
    println!(
        "\
clitunes — the Ghostty of TUI music apps

USAGE:
    clitunes [--source tone|radio] [--station <uuid>]

OPTIONS:
    --source <tone|radio>    Audio source (default: tone)
    --station <uuid>         Radio station UUID (required with --source radio)
    -h, --help               Show this help

KEYS:
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
