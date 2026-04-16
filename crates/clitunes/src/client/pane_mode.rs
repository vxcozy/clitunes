//! `clitunes --pane <name>` — standalone single-component pane.
//!
//! Each pane type is a standalone process that connects to the daemon,
//! subscribes to relevant events + PCM, and renders ONE component
//! fullscreen with no chrome. Intended for tmux/wezterm/ghostty embedding.

use std::io::{self, BufWriter, Write as _};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clitunes_core::StereoFrame;
use clitunes_engine::audio::FftTap;
use clitunes_engine::pcm::cross_process_api::PcmConsumer;
use clitunes_engine::proto::events::Event;
use clitunes_engine::visualiser::{
    AnsiWriter, CellGrid, Fire, Matrix, Metaballs, Moire, Plasma, Ripples, TuiContext, Tunnel,
    Visualiser, Vortex,
};

const FFT_SIZE: usize = 2048;
const TARGET_FRAME: Duration = Duration::from_millis(33);
const MINI_SPECTRUM_BINS: usize = 16;

/// Available pane names that can be passed to `--pane`.
pub const PANE_NAMES: &[&str] = &["visualiser", "now-playing", "mini-spectrum"];

pub fn validate_pane_name(name: &str) -> Result<()> {
    if PANE_NAMES.contains(&name) {
        Ok(())
    } else {
        anyhow::bail!("unknown pane: {name}. Available: {}", PANE_NAMES.join(", "))
    }
}

pub struct PaneModeConfig {
    pub pane_name: String,
    pub viz_name: Option<String>,
    pub consumer: Box<dyn PcmConsumer>,
    pub sample_rate: u32,
    pub event_rx: std::sync::mpsc::Receiver<Event>,
    pub stop: Arc<AtomicBool>,
}

pub fn run_pane(config: PaneModeConfig) -> Result<()> {
    match config.pane_name.as_str() {
        "visualiser" => run_visualiser_pane(config),
        "now-playing" => run_now_playing_pane(config),
        "mini-spectrum" => run_mini_spectrum_pane(config),
        _ => unreachable!(),
    }
}

// ─── visualiser pane ───────────────────────────────────────────────

fn run_visualiser_pane(config: PaneModeConfig) -> Result<()> {
    let (mut cols, mut rows) = pane_cell_rect();
    let mut grid = CellGrid::new(cols, rows);

    let mut visualisers: Vec<Box<dyn Visualiser>> = vec![
        Box::new(Plasma::new()),
        Box::new(Ripples::new()),
        Box::new(Tunnel::new()),
        Box::new(Metaballs::new()),
        Box::new(Vortex::new()),
        Box::new(Fire::new()),
        Box::new(Matrix::new()),
        Box::new(Moire::new()),
    ];

    let mut active_idx: usize = if let Some(ref name) = config.viz_name {
        visualisers
            .iter()
            .position(|v| v.id().as_str() == name)
            .unwrap_or(0)
    } else {
        0
    };

    let _raw = RawStdin::enable().context("enable raw stdin")?;
    let interactive = _raw.is_some();
    let (key_tx, key_rx) = std::sync::mpsc::channel::<u8>();
    if interactive {
        spawn_raw_key_thread(Arc::clone(&config.stop), key_tx);
    }

    let stdout = io::stdout();
    let mut writer = AnsiWriter::new(BufWriter::with_capacity(64 * 1024, stdout.lock()));
    writer.clear_screen()?;
    writer.hide_cursor()?;
    writer.flush()?;

    let mut consumer = config.consumer;
    let mut pcm_buf = vec![StereoFrame::SILENCE; FFT_SIZE];
    let mut fft = FftTap::new(FFT_SIZE);

    while !config.stop.load(Ordering::Relaxed) {
        let frame_start = Instant::now();

        while let Ok(ev) = config.event_rx.try_recv() {
            if let Event::VizChanged { ref name } = ev {
                if let Some(idx) = visualisers.iter().position(|v| v.id().as_str() == name) {
                    active_idx = idx;
                    let _ = writer.clear_screen();
                }
            }
            if let Event::DaemonShuttingDown { .. } = ev {
                config.stop.store(true, Ordering::SeqCst);
            }
        }

        while let Ok(b) = key_rx.try_recv() {
            match b {
                b'q' | b'Q' => {
                    config.stop.store(true, Ordering::SeqCst);
                }
                b'n' | b'N' => {
                    active_idx = (active_idx + 1) % visualisers.len();
                    let _ = writer.clear_screen();
                }
                b'p' | b'P' => {
                    active_idx = (active_idx + visualisers.len() - 1) % visualisers.len();
                    let _ = writer.clear_screen();
                }
                _ => {}
            }
        }

        // Check for terminal resize.
        let (new_w, new_h) = pane_cell_rect();
        if new_w != cols || new_h != rows {
            cols = new_w;
            rows = new_h;
            grid = CellGrid::new(cols, rows);
            let _ = writer.clear_screen();
        }

        let n = consumer.read_frames(&mut pcm_buf).unwrap_or(0);
        let snapshot = fft.snapshot_from(&pcm_buf[..n.max(1)], config.sample_rate);

        {
            let mut ctx = TuiContext { grid: &mut grid };
            visualisers[active_idx].render_tui(&mut ctx, &snapshot);
        }

        writer.write_frame(&grid)?;
        writer.flush()?;

        let elapsed = frame_start.elapsed();
        if elapsed < TARGET_FRAME {
            thread::sleep(TARGET_FRAME - elapsed);
        }
    }

    let _ = writer.reset();
    let _ = writer.clear_screen();
    let _ = writer.show_cursor();
    let _ = writer.flush();
    Ok(())
}

// ─── now-playing pane ──────────────────────────────────────────────

fn run_now_playing_pane(config: PaneModeConfig) -> Result<()> {
    let _raw = RawStdin::enable().context("enable raw stdin")?;
    let interactive = _raw.is_some();
    let (key_tx, key_rx) = std::sync::mpsc::channel::<u8>();
    if interactive {
        spawn_raw_key_thread(Arc::clone(&config.stop), key_tx);
    }

    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    // Hide cursor
    write!(out, "\x1b[?25l")?;
    out.flush()?;

    let mut artist: Option<String> = None;
    let mut title: Option<String> = None;
    let mut album: Option<String> = None;
    let mut station_or_path: Option<String> = None;
    let mut source: Option<String> = None;
    let mut connect_device: Option<String> = None;

    while !config.stop.load(Ordering::Relaxed) {
        while let Ok(ev) = config.event_rx.try_recv() {
            match ev {
                Event::NowPlayingChanged {
                    artist: a,
                    title: t,
                    album: al,
                    ..
                } => {
                    artist = a;
                    title = t;
                    album = al;
                }
                Event::StateChanged {
                    source: s,
                    station_or_path: sp,
                    ..
                } => {
                    source = s;
                    station_or_path = sp;
                }
                Event::ConnectDeviceConnected { remote_name } => {
                    connect_device = Some(remote_name.unwrap_or_default());
                }
                Event::ConnectDeviceDisconnected => {
                    connect_device = None;
                }
                Event::DaemonShuttingDown { .. } => {
                    config.stop.store(true, Ordering::SeqCst);
                }
                _ => {}
            }
        }

        while let Ok(b) = key_rx.try_recv() {
            if b == b'q' || b == b'Q' {
                config.stop.store(true, Ordering::SeqCst);
            }
        }

        let (cols, rows) = terminal_size().unwrap_or((80, 3));

        // Move to top-left, clear
        write!(out, "\x1b[H\x1b[2J")?;

        let display_title = title
            .as_deref()
            .or(station_or_path.as_deref())
            .unwrap_or("—");
        let display_artist = artist.as_deref().unwrap_or("");
        let display_source = source.as_deref().unwrap_or("");

        let connect_tag = connect_device.as_ref().map(|name| {
            if name.is_empty() {
                "[Connect]".to_string()
            } else {
                format!("[Connect: {name}]")
            }
        });

        if rows >= 3 {
            // 3-line layout: source/station, artist, title [album]
            let source_info = format!(
                "{} ▸ {}",
                display_source,
                station_or_path.as_deref().unwrap_or("")
            );
            let line1 = if let Some(ref tag) = connect_tag {
                let col = cols as usize;
                if col > tag.len() + 4 {
                    let left = truncate(&source_info, col - tag.len() - 2);
                    let pad = col.saturating_sub(left.len() + tag.len());
                    format!("\x1b[2m{left}{:>pad$}{tag}\x1b[0m", "")
                } else {
                    format!("\x1b[2m{}\x1b[0m", truncate(&source_info, col))
                }
            } else {
                format!("\x1b[2m{}\x1b[0m", truncate(&source_info, cols as usize))
            };
            let line2 = if display_artist.is_empty() {
                String::new()
            } else {
                format!("\x1b[1m{}\x1b[0m", truncate(display_artist, cols as usize))
            };
            let line3 = if let Some(ref al) = album {
                truncate(
                    &format!("{display_title}  \x1b[2m({al})\x1b[0m"),
                    cols as usize,
                )
            } else {
                truncate(display_title, cols as usize)
            };
            write!(out, "{line1}\r\n{line2}\r\n{line3}")?;
        } else {
            // 1-line layout: Artist - Title
            let line = if display_artist.is_empty() {
                display_title.to_string()
            } else {
                format!("{display_artist} — {display_title}")
            };
            write!(out, "{}", truncate(&line, cols as usize))?;
        }

        out.flush()?;
        thread::sleep(Duration::from_millis(250));
    }

    // Restore cursor
    write!(out, "\x1b[?25h\x1b[H\x1b[2J")?;
    out.flush()?;
    Ok(())
}

// ─── mini-spectrum pane ────────────────────────────────────────────

/// Unicode block characters for spectrum display (8 levels).
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn run_mini_spectrum_pane(config: PaneModeConfig) -> Result<()> {
    let _raw = RawStdin::enable().context("enable raw stdin")?;
    let interactive = _raw.is_some();
    let (key_tx, key_rx) = std::sync::mpsc::channel::<u8>();
    if interactive {
        spawn_raw_key_thread(Arc::clone(&config.stop), key_tx);
    }

    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    write!(out, "\x1b[?25l")?;
    out.flush()?;

    let mut consumer = config.consumer;
    let mut pcm_buf = vec![StereoFrame::SILENCE; FFT_SIZE];
    let mut fft = FftTap::new(FFT_SIZE);
    let mut smoothed = [0.0f32; MINI_SPECTRUM_BINS];

    while !config.stop.load(Ordering::Relaxed) {
        let frame_start = Instant::now();

        while let Ok(ev) = config.event_rx.try_recv() {
            if let Event::DaemonShuttingDown { .. } = ev {
                config.stop.store(true, Ordering::SeqCst);
            }
        }

        while let Ok(b) = key_rx.try_recv() {
            if b == b'q' || b == b'Q' {
                config.stop.store(true, Ordering::SeqCst);
            }
        }

        let n = consumer.read_frames(&mut pcm_buf).unwrap_or(0);
        let snapshot = fft.snapshot_from(&pcm_buf[..n.max(1)], config.sample_rate);
        let bins = log_rebin(&snapshot.magnitudes, MINI_SPECTRUM_BINS);

        let (cols, _) = terminal_size().unwrap_or((80, 1));
        let display_bins = (cols as usize).min(bins.len());

        write!(out, "\x1b[H")?;
        for i in 0..display_bins {
            // Exponential smoothing
            smoothed[i] = smoothed[i] * 0.6 + bins[i] * 0.4;
            let level = (smoothed[i] * 8.0).clamp(0.0, 7.0) as usize;
            // Teal colour gradient based on level
            let g = 120 + (level * 17).min(135);
            let b_col = 160 + (level * 12).min(95);
            write!(out, "\x1b[38;2;40;{g};{b_col}m{}", BLOCKS[level])?;
        }
        write!(out, "\x1b[0m\x1b[K")?;
        out.flush()?;

        let elapsed = frame_start.elapsed();
        if elapsed < TARGET_FRAME {
            thread::sleep(TARGET_FRAME - elapsed);
        }
    }

    write!(out, "\x1b[?25h\x1b[H\x1b[2J")?;
    out.flush()?;
    Ok(())
}

/// Rebin FFT magnitudes into `num_bins` logarithmically-spaced bins.
fn log_rebin(magnitudes: &[f32], num_bins: usize) -> Vec<f32> {
    if magnitudes.is_empty() {
        return vec![0.0; num_bins];
    }
    // Skip DC, use first half of spectrum
    let usable = &magnitudes[1..magnitudes.len().min(magnitudes.len() / 2 + 1)];
    if usable.is_empty() {
        return vec![0.0; num_bins];
    }
    let mut bins = vec![0.0f32; num_bins];
    let n = usable.len() as f32;
    for (i, bin) in bins.iter_mut().enumerate() {
        let lo = (n.powf(i as f32 / num_bins as f32)) as usize;
        let hi = (n.powf((i + 1) as f32 / num_bins as f32) as usize).max(lo + 1);
        let lo = lo.min(usable.len());
        let hi = hi.min(usable.len());
        if hi > lo {
            let sum: f32 = usable[lo..hi].iter().sum();
            // Normalise and apply log scale
            let avg = sum / (hi - lo) as f32;
            *bin = (1.0 + avg).ln().min(4.0) / 4.0;
        }
    }
    bins
}

// ─── shared helpers ────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 1 {
        format!("{}…", &s[..max - 1])
    } else {
        String::new()
    }
}

struct RawStdin {
    fd: libc::c_int,
    saved: libc::termios,
}

impl RawStdin {
    fn enable() -> Result<Option<Self>> {
        let fd = io::stdin().as_raw_fd();
        unsafe {
            let mut saved: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut saved) != 0 {
                return Ok(None);
            }
            let mut raw = saved;
            raw.c_lflag &= !(libc::ECHO | libc::ICANON);
            raw.c_iflag &= !(libc::IXON | libc::ICRNL);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 1;
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
                return Err(anyhow::anyhow!("tcsetattr failed"));
            }
            Ok(Some(Self { fd, saved }))
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

fn spawn_raw_key_thread(stop: Arc<AtomicBool>, tx: std::sync::mpsc::Sender<u8>) {
    thread::Builder::new()
        .name("clitunes-pane-key".into())
        .spawn(move || {
            use io::Read;
            let mut stdin = io::stdin();
            let mut buf = [0u8; 8];
            while !stop.load(Ordering::Relaxed) {
                match stdin.read(&mut buf) {
                    Ok(0) => {}
                    Ok(n) => {
                        for &b in &buf[..n] {
                            let _ = tx.send(b);
                        }
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        })
        .expect("spawn pane key thread");
}

fn pane_cell_rect() -> (u16, u16) {
    let (c, r) = terminal_size().unwrap_or((80, 24));
    (c.saturating_sub(1).max(20), r.saturating_sub(1).max(10))
}

fn terminal_size() -> Option<(u16, u16)> {
    use std::mem::MaybeUninit;
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
