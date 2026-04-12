use std::io::{self, BufWriter};
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
use clitunes_engine::proto::verbs::Verb;
use clitunes_engine::tui::picker::{
    key_from_bytes, load_curated, paint_picker, CuratedLoadOutcome, PickerAction, PickerKey,
    PickerState,
};
use clitunes_engine::visualiser::{
    AnsiWriter, Auralis, CellGrid, Metaballs, Plasma, Ripples, Starfield, TuiContext, Tunnel,
    Visualiser,
};

const FFT_SIZE: usize = 2048;
const TARGET_FRAME: Duration = Duration::from_millis(33);
const FALLBACK_COLS: u16 = 80;
const FALLBACK_ROWS: u16 = 24;

enum AppKey {
    Picker(PickerKey),
    VizNext,
    VizPrev,
}

pub struct RenderLoopConfig {
    pub consumer: Box<dyn PcmConsumer>,
    pub sample_rate: u32,
    pub event_rx: std::sync::mpsc::Receiver<Event>,
    pub verb_tx: tokio::sync::mpsc::Sender<Verb>,
    pub stop: Arc<AtomicBool>,
}

pub struct RenderLoop {
    consumer: Box<dyn PcmConsumer>,
    pcm_buf: Vec<StereoFrame>,
    fft: FftTap,
    sample_rate: u32,
    event_rx: std::sync::mpsc::Receiver<Event>,
    verb_tx: tokio::sync::mpsc::Sender<Verb>,
    stop: Arc<AtomicBool>,
}

impl RenderLoop {
    pub fn new(config: RenderLoopConfig) -> Self {
        Self {
            consumer: config.consumer,
            pcm_buf: vec![StereoFrame::SILENCE; FFT_SIZE],
            fft: FftTap::new(FFT_SIZE),
            sample_rate: config.sample_rate,
            event_rx: config.event_rx,
            verb_tx: config.verb_tx,
            stop: config.stop,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        let (cells_w, cells_h) = visualiser_cell_rect();
        tracing::info!(
            target: "clitunes",
            cells_w,
            cells_h,
            "boot: daemon client → visualiser carousel → ansi"
        );

        let mut grid = CellGrid::new(cells_w, cells_h);

        let mut visualisers: Vec<Box<dyn Visualiser>> = vec![
            Box::new(Plasma::new()),
            Box::new(Ripples::new()),
            Box::new(Tunnel::new()),
            Box::new(Metaballs::new()),
            Box::new(Starfield::new()),
            Box::new(Auralis::new()),
        ];
        let mut active_idx: usize = 0;

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
        let mut picker_state = PickerState::new(&curated, 0);
        picker_state.show();

        let _raw = RawStdin::enable().context("enable raw stdin")?;
        let interactive = _raw.is_some();
        let (key_tx, key_rx) = std::sync::mpsc::channel::<AppKey>();
        if interactive {
            spawn_keypress_thread(Arc::clone(&self.stop), key_tx);
        } else {
            drop(key_tx);
            tracing::info!(
                target: "clitunes",
                "stdin is not a tty — running non-interactively (piped output / CI)"
            );
        }

        let stdout = io::stdout();
        let mut writer = AnsiWriter::new(BufWriter::with_capacity(64 * 1024, stdout.lock()));
        writer.clear_screen()?;
        writer.hide_cursor()?;
        writer.flush()?;

        let mut frame_idx: u64 = 0;

        while !self.stop.load(Ordering::Relaxed) {
            let frame_start = Instant::now();

            while let Ok(ev) = self.event_rx.try_recv() {
                self.handle_event(&ev, &mut active_idx, &visualisers, &mut picker_state);
            }

            while let Ok(key) = key_rx.try_recv() {
                match key {
                    AppKey::Picker(pk) => {
                        let action = picker_state.handle_key(pk);
                        match action {
                            PickerAction::Pick(slot) => {
                                if let Some(station) =
                                    curated.stations.iter().find(|s| s.slot == slot)
                                {
                                    picker_state.hide();
                                    if let Some(uuid) = station.url.strip_prefix("radiobrowser:") {
                                        let _ = self.verb_tx.try_send(Verb::Source(
                                            clitunes_engine::proto::verbs::SourceArg::Radio {
                                                uuid: uuid.to_owned(),
                                            },
                                        ));
                                    }
                                }
                            }
                            PickerAction::Quit => {
                                self.stop.store(true, Ordering::SeqCst);
                            }
                            PickerAction::Moved | PickerAction::Hide | PickerAction::Ignored => {}
                        }
                    }
                    AppKey::VizNext => {
                        active_idx = (active_idx + 1) % visualisers.len();
                        let _ = writer.clear_screen();
                    }
                    AppKey::VizPrev => {
                        active_idx = (active_idx + visualisers.len() - 1) % visualisers.len();
                        let _ = writer.clear_screen();
                    }
                }
            }

            let n = self.consumer.read_frames(&mut self.pcm_buf).unwrap_or(0);
            let snapshot = self
                .fft
                .snapshot_from(&self.pcm_buf[..n.max(1)], self.sample_rate);

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

        let _ = writer.reset();
        let _ = writer.clear_screen();
        let _ = writer.show_cursor();
        let _ = writer.flush();

        tracing::info!(
            target: "clitunes",
            frames = frame_idx,
            "shutdown"
        );

        Ok(())
    }

    fn handle_event(
        &self,
        event: &Event,
        active_idx: &mut usize,
        visualisers: &[Box<dyn Visualiser>],
        picker_state: &mut PickerState,
    ) {
        match event {
            Event::VizChanged { name } => {
                if let Some(idx) = visualisers.iter().position(|v| v.id().as_str() == name) {
                    *active_idx = idx;
                }
            }
            Event::StateChanged {
                source: Some(source),
                ..
            } if source == "radio" => {
                picker_state.hide();
            }
            Event::SourceError { error, .. } => {
                tracing::warn!(target: "clitunes", %error, "source error from daemon");
                picker_state.banner =
                    Some(format!("Source error — pick another station. ({error})"));
                picker_state.show();
            }
            Event::DaemonShuttingDown { reason } => {
                tracing::info!(target: "clitunes", %reason, "daemon shutting down");
                self.stop.store(true, Ordering::SeqCst);
            }
            _ => {}
        }
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

fn spawn_keypress_thread(stop: Arc<AtomicBool>, tx: std::sync::mpsc::Sender<AppKey>) {
    thread::Builder::new()
        .name("clitunes-keypress".into())
        .spawn(move || {
            use io::Read;
            let mut stdin = io::stdin();
            let mut buf = [0u8; 8];
            let mut pending: Vec<u8> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                match stdin.read(&mut buf) {
                    Ok(0) => {
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

fn classify_pending(pending: &[u8]) -> Option<AppKey> {
    match pending {
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

fn visualiser_cell_rect() -> (u16, u16) {
    let (term_cols, term_rows) = terminal_size().unwrap_or((FALLBACK_COLS, FALLBACK_ROWS));
    let cols = term_cols.saturating_sub(1).max(20);
    let rows = term_rows.saturating_sub(2).max(10);
    (cols, rows)
}

fn terminal_size() -> Option<(u16, u16)> {
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd;
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
