use std::io::{self, BufWriter};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clitunes_core::{LibraryCategory, StereoFrame};
use clitunes_engine::audio::FftTap;
use clitunes_engine::pcm::cross_process_api::PcmConsumer;
use clitunes_engine::proto::events::Event;
use clitunes_engine::proto::verbs::{SourceArg, Verb};
use clitunes_engine::tui::album_art::AlbumArtState;
use clitunes_engine::tui::components::now_playing::{render_now_playing, NowPlayingState};
use clitunes_engine::tui::micro::{BreathingAnimation, ErrorPulse, QuitFade, VolumeOverlay};
use clitunes_engine::tui::picker::{
    key_from_bytes, load_curated, paint_picker, CuratedList, CuratedLoadOutcome, PickerAction,
    PickerKey, PickerState, PickerTab, PickerTransition,
};
use clitunes_engine::tui::theme::Theme;

use crate::client::transition_controller::TransitionController;
use clitunes_engine::visualiser::{
    AnsiWriter, CellGrid, Fire, Heartbeat, Matrix, Metaballs, Moire, Plasma, Ripples, Scope,
    TuiContext, Tunnel, Visualiser, Vortex, Wave,
};

/// FFT window size. 2048 samples at 48 kHz gives ~43 ms windows and
/// 1024 frequency bins — standard for music visualisation, balancing
/// frequency resolution against temporal responsiveness.
const FFT_SIZE: usize = 2048;
/// Target frame duration (~30 fps). 33 ms rather than 33.3 ms because
/// `Duration::from_millis` truncates, and the 0.3 ms drift is
/// invisible at terminal refresh rates.
const TARGET_FRAME: Duration = Duration::from_millis(33);
/// Fallback terminal dimensions when `TIOCGWINSZ` is unavailable
/// (piped output, CI). 80x24 is the POSIX minimum.
const FALLBACK_COLS: u16 = 80;
const FALLBACK_ROWS: u16 = 24;
/// Longest ANSI escape sequence we recognise (ESC [ A = 3 bytes).
/// Any pending buffer longer than this can't be a valid escape.
const MAX_ESCAPE_LEN: usize = 3;
/// Debounce window between a Search query edit and the outgoing
/// `Verb::Search`. 300 ms is short enough that users don't notice the
/// gap but long enough to coalesce fast typing into a single request.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

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
    pub measure_startup: bool,
}

/// TUI state for the render loop. Bundles visualiser carousel,
/// picker, overlays, and transition state into a single struct
/// so `handle_event` and `handle_key` are clean methods instead
/// of 9-parameter functions.
struct AppState {
    grid: CellGrid,
    visualisers: Vec<Box<dyn Visualiser>>,
    active_idx: usize,
    theme: Theme,
    transition_ctrl: TransitionController,
    curated: CuratedList,
    picker_state: PickerState,
    picker_transition: PickerTransition,
    picker_snap: CellGrid,
    volume_overlay: VolumeOverlay,
    error_pulse: ErrorPulse,
    quit_fade: QuitFade,
    breathing: BreathingAnimation,
    frame_idx: u64,
    /// Pending debounced search query. Cleared after the `Verb::Search`
    /// is actually dispatched. Holds `(query, dirty_at)`.
    pending_search: Option<(String, Instant)>,
    /// Album art state. Updated from `NowPlayingChanged.art_url`;
    /// painted in the top-right of the grid when a cover is loaded.
    album_art: AlbumArtState,
    now_playing: NowPlayingState,
}

impl AppState {
    fn new(cells_w: u16, cells_h: u16, curated: CuratedList) -> Self {
        let mut picker_state = PickerState::new(&curated, 0);
        picker_state.show();

        let visualisers: Vec<Box<dyn Visualiser>> = vec![
            Box::new(Plasma::new()),
            Box::new(Ripples::new()),
            Box::new(Tunnel::new()),
            Box::new(Metaballs::new()),
            Box::new(Vortex::new()),
            Box::new(Fire::new()),
            Box::new(Matrix::new()),
            Box::new(Moire::new()),
            Box::new(Wave::new()),
            Box::new(Scope::new()),
            Box::new(Heartbeat::new()),
        ];
        let active_idx = 0; // Plasma — the strongest first impression.

        Self {
            grid: CellGrid::new(cells_w, cells_h),
            visualisers,
            active_idx,
            theme: Theme::default(),
            transition_ctrl: TransitionController::new(),
            curated,
            picker_state,
            picker_transition: PickerTransition::start_fade_in(),
            picker_snap: CellGrid::new(cells_w, cells_h),
            volume_overlay: VolumeOverlay::default(),
            error_pulse: ErrorPulse::default(),
            quit_fade: QuitFade::default(),
            breathing: BreathingAnimation::default(),
            frame_idx: 0,
            pending_search: None,
            album_art: AlbumArtState::new(),
            now_playing: NowPlayingState::default(),
        }
    }

    /// Resize grid and picker snapshot if terminal dimensions changed.
    /// Returns `true` if a resize occurred (caller should clear screen).
    fn maybe_resize(&mut self, new_w: u16, new_h: u16) -> bool {
        let (cur_w, cur_h) = (self.grid.width(), self.grid.height());
        if cur_w == new_w && cur_h == new_h {
            return false;
        }
        self.grid = CellGrid::new(new_w, new_h);
        self.picker_snap = CellGrid::new(new_w, new_h);
        true
    }

    fn handle_event(&mut self, event: &Event, stop: &AtomicBool) {
        match event {
            Event::VizChanged { name } => {
                if let Some(idx) = self
                    .visualisers
                    .iter()
                    .position(|v| v.id().as_str() == name)
                {
                    self.transition_ctrl
                        .start_viz_switch(&self.grid, idx > self.active_idx);
                    self.active_idx = idx;
                }
            }
            Event::StateChanged {
                source: Some(source),
                state,
                ..
            } if source == "radio" => {
                self.transition_ctrl.start_source_switch(&self.grid);
                self.picker_state.hide();
                let paused = matches!(state, clitunes_engine::proto::events::PlayState::Paused);
                self.transition_ctrl.set_paused(paused, &self.grid);
                if paused {
                    self.breathing.start();
                } else {
                    self.breathing.stop();
                }
            }
            Event::StateChanged { state, .. } => {
                let paused = matches!(state, clitunes_engine::proto::events::PlayState::Paused);
                self.transition_ctrl.set_paused(paused, &self.grid);
                if paused {
                    self.breathing.start();
                } else {
                    self.breathing.stop();
                }
            }
            Event::VolumeChanged { volume } => {
                self.volume_overlay.show(*volume);
            }
            Event::NowPlayingChanged {
                artist,
                title,
                album,
                art_url,
                ..
            } => {
                self.now_playing.artist = artist.clone();
                self.now_playing.title = title.clone();
                self.now_playing.album = album.clone();
                match art_url {
                    Some(url) => self.album_art.request(url),
                    None => self.album_art.clear(),
                }
            }
            Event::ConnectDeviceConnected { remote_name } => {
                self.now_playing.connect_device = Some(remote_name.clone().unwrap_or_default());
            }
            Event::ConnectDeviceDisconnected => {
                self.now_playing.connect_device = None;
            }

            Event::SourceError {
                error, error_code, ..
            } => {
                tracing::warn!(target: "clitunes", %error, "source error from daemon");
                let (pulse_msg, banner_msg) = if error_code.as_deref() == Some("premium_required") {
                    (
                        "Spotify Premium is required for playback".into(),
                        "Spotify Premium required — visit spotify.com/premium".into(),
                    )
                } else {
                    (
                        format!("Source error: {error}"),
                        format!("Source error — pick another station. ({error})"),
                    )
                };
                self.error_pulse.trigger(pulse_msg);
                self.picker_state.banner = Some(banner_msg);
                self.picker_state.show();
            }
            Event::DaemonShuttingDown { reason } => {
                tracing::info!(target: "clitunes", %reason, "daemon shutting down");
                stop.store(true, Ordering::SeqCst);
            }
            Event::SearchResults { query, items, .. } => {
                // Ignore stale results — user has moved on.
                if *query == self.picker_state.search_query {
                    self.picker_state.set_search_results(items.clone());
                }
            }
            Event::LibraryResults {
                category, items, ..
            } => {
                self.picker_state
                    .set_library_items(*category, items.clone());
            }
            Event::PlaylistResults { items, .. } => {
                // Playlists drill-in: show the playlist's tracks in the
                // Library items view under the Playlists category.
                self.picker_state
                    .set_library_items(LibraryCategory::Playlists, items.clone());
            }
            _ => {}
        }
    }

    /// Handle a keypress. Returns `true` if the terminal screen should
    /// be cleared (viz switch).
    fn handle_key(&mut self, key: AppKey, verb_tx: &tokio::sync::mpsc::Sender<Verb>) -> bool {
        if self.quit_fade.is_input_blocked() {
            return false;
        }
        // `n`/`p` as viz nav collide with typing into the Search tab.
        // When the Search tab has focus, the `Char` goes to the picker;
        // otherwise we translate them back into viz-nav events.
        let key = match key {
            AppKey::Picker(PickerKey::Char(c))
                if (c == 'n' || c == 'N' || c == 'p' || c == 'P')
                    && (!self.picker_state.visible
                        || self.picker_state.active_tab != PickerTab::Search) =>
            {
                if c == 'n' || c == 'N' {
                    AppKey::VizNext
                } else {
                    AppKey::VizPrev
                }
            }
            other => other,
        };
        match key {
            AppKey::Picker(pk) => {
                let was_visible = self.picker_state.visible;
                let action = self.picker_state.handle_key(pk);
                match action {
                    PickerAction::Pick(slot) => {
                        if let Some(station) = self.curated.stations.iter().find(|s| s.slot == slot)
                        {
                            self.picker_state.hide();
                            self.picker_transition = PickerTransition::start_fade_out();
                            if let Some(uuid) = station.url.strip_prefix("radiobrowser:") {
                                tracing::info!(
                                    target: "clitunes",
                                    station = %station.name,
                                    %uuid,
                                    "picker: station selected"
                                );
                                if let Err(e) = verb_tx.try_send(Verb::Source(
                                    clitunes_engine::proto::verbs::SourceArg::Radio {
                                        uuid: uuid.to_owned(),
                                    },
                                )) {
                                    tracing::error!(
                                        target: "clitunes",
                                        error = %e,
                                        "picker: failed to send source verb"
                                    );
                                }
                            } else {
                                tracing::warn!(
                                    target: "clitunes",
                                    url = %station.url,
                                    "picker: station URL is not a radiobrowser: URI"
                                );
                            }
                        }
                    }
                    PickerAction::PickSpotify(uri) => {
                        self.picker_state.hide();
                        self.picker_transition = PickerTransition::start_fade_out();
                        tracing::info!(
                            target: "clitunes",
                            %uri,
                            "picker: spotify item selected"
                        );
                        if let Err(e) = verb_tx.try_send(Verb::Source(SourceArg::Spotify { uri })) {
                            tracing::error!(
                                target: "clitunes",
                                error = %e,
                                "picker: failed to send spotify source verb"
                            );
                        }
                    }
                    PickerAction::SearchDirty(query) => {
                        if query.trim().is_empty() {
                            // Clear any in-flight search and wipe results.
                            self.pending_search = None;
                            self.picker_state.set_search_results(Vec::new());
                        } else {
                            self.pending_search = Some((query, Instant::now()));
                        }
                    }
                    PickerAction::BrowseLibrary(category) => {
                        if let Err(e) = verb_tx.try_send(Verb::BrowseLibrary {
                            category,
                            limit: None,
                        }) {
                            tracing::error!(
                                target: "clitunes",
                                error = %e,
                                ?category,
                                "picker: failed to send browse_library verb"
                            );
                        }
                    }
                    PickerAction::BrowsePlaylist(id) => {
                        if let Err(e) = verb_tx.try_send(Verb::BrowsePlaylist {
                            id: id.clone(),
                            limit: None,
                        }) {
                            tracing::error!(
                                target: "clitunes",
                                error = %e,
                                %id,
                                "picker: failed to send browse_playlist verb"
                            );
                        }
                    }
                    PickerAction::Quit => {
                        self.quit_fade.start();
                    }
                    PickerAction::Hide => {
                        self.picker_transition = PickerTransition::start_fade_out();
                    }
                    PickerAction::Moved if !was_visible && self.picker_state.visible => {
                        // Reopened via 's' — fade in.
                        self.picker_transition = PickerTransition::start_fade_in();
                    }
                    PickerAction::Moved | PickerAction::Ignored => {}
                }
                false
            }
            AppKey::VizNext => {
                self.transition_ctrl.start_viz_switch(&self.grid, true);
                self.active_idx = (self.active_idx + 1) % self.visualisers.len();
                true
            }
            AppKey::VizPrev => {
                self.transition_ctrl.start_viz_switch(&self.grid, false);
                self.active_idx =
                    (self.active_idx + self.visualisers.len() - 1) % self.visualisers.len();
                true
            }
        }
    }
}

pub struct RenderLoop {
    consumer: Box<dyn PcmConsumer>,
    pcm_buf: Vec<StereoFrame>,
    fft: FftTap,
    sample_rate: u32,
    event_rx: std::sync::mpsc::Receiver<Event>,
    verb_tx: tokio::sync::mpsc::Sender<Verb>,
    stop: Arc<AtomicBool>,
    measure_startup: bool,
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
            measure_startup: config.measure_startup,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        let (mut cells_w, mut cells_h) = visualiser_cell_rect();
        tracing::info!(
            target: "clitunes",
            cells_w,
            cells_h,
            "boot: daemon client → visualiser carousel → ansi"
        );

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

        let mut state = AppState::new(cells_w, cells_h, curated);

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

        while !self.stop.load(Ordering::Relaxed) {
            let frame_start = Instant::now();

            // Process daemon events.
            while let Ok(ev) = self.event_rx.try_recv() {
                state.handle_event(&ev, &self.stop);
            }

            // Process user input.
            while let Ok(key) = key_rx.try_recv() {
                if state.handle_key(key, &self.verb_tx) {
                    let _ = writer.clear_screen();
                }
            }

            // Flush any debounced search query whose window has elapsed.
            if let Some((query, dirty_at)) = state.pending_search.as_ref() {
                if dirty_at.elapsed() >= SEARCH_DEBOUNCE {
                    let query = query.clone();
                    state.pending_search = None;
                    if let Err(e) = self.verb_tx.try_send(Verb::Search {
                        query: query.clone(),
                        limit: None,
                    }) {
                        tracing::error!(
                            target: "clitunes",
                            error = %e,
                            %query,
                            "picker: failed to send search verb"
                        );
                    }
                }
            }

            // Check for terminal resize.
            let (new_w, new_h) = visualiser_cell_rect();
            if state.maybe_resize(new_w, new_h) {
                cells_w = new_w;
                cells_h = new_h;
                let _ = writer.clear_screen();
            }

            // PCM → FFT snapshot.
            let n = self.consumer.read_frames(&mut self.pcm_buf).unwrap_or(0);
            let snapshot = self
                .fft
                .snapshot_from(&self.pcm_buf[..n.max(1)], self.sample_rate);

            // Render active visualiser.
            {
                let mut ctx = TuiContext {
                    grid: &mut state.grid,
                };
                state.visualisers[state.active_idx].render_tui(&mut ctx, &snapshot);
            }

            // First-launch fade from black.
            if state.frame_idx == 0 {
                state.transition_ctrl.start_first_launch(cells_w, cells_h);
            }

            // Apply state transitions (source switch, viz switch, play/pause, first launch).
            state.transition_ctrl.apply(&mut state.grid);

            // Album art: drain any completed fetch, then paint into the
            // top-right corner (below any picker overlay). 20×10 cells
            // at roughly a 2:1 cell aspect ratio gives a near-square
            // cover. Skipped for tiny terminals.
            state.album_art.poll_ready();
            const ART_W: u16 = 20;
            const ART_H: u16 = 10;
            const ART_PAD: u16 = 1;
            if cells_w >= ART_W + ART_PAD * 2 && cells_h >= ART_H + ART_PAD * 2 {
                let x0 = cells_w - ART_W - ART_PAD;
                let y0 = ART_PAD;
                state.album_art.paint(&mut state.grid, x0, y0, ART_W, ART_H);
            }

            // Now-playing strip: bottom 2 rows, only when we have metadata.
            if state.now_playing.artist.is_some() || state.now_playing.title.is_some() {
                let np_y = cells_h.saturating_sub(2);
                render_now_playing(
                    &mut state.grid,
                    np_y,
                    0,
                    cells_w,
                    &state.now_playing,
                    &state.theme,
                );
            }

            // Picker overlay with fade transitions.
            if state
                .picker_transition
                .should_paint_picker(state.picker_state.visible)
            {
                // Snapshot the visualiser-only frame before painting the picker.
                state.picker_snap.copy_from(&state.grid);
                let _ = paint_picker(
                    &mut state.grid,
                    &state.curated,
                    &state.picker_state,
                    &state.theme,
                );

                if state.picker_transition.is_active() {
                    let mut blended = CellGrid::new(cells_w, cells_h);
                    if let Some(t) = state.picker_transition.transition() {
                        if state.picker_transition.is_fading_out() {
                            // Fade out: source=picker overlay, target=viz only.
                            t.apply(&mut blended, &state.grid, &state.picker_snap);
                        } else {
                            // Fade in: source=viz only, target=picker overlay.
                            t.apply(&mut blended, &state.picker_snap, &state.grid);
                        }
                        state.grid.copy_from(&blended);
                    }
                    state.picker_transition.tick();
                }
            }

            // Micro-interactions: volume overlay, breathing, error pulse.
            state.volume_overlay.render(&mut state.grid, &state.theme);
            state.volume_overlay.tick();
            state.error_pulse.tick();
            state.breathing.tick();

            // Quit fade (last — overrides everything).
            if state.quit_fade.is_active() {
                state.quit_fade.apply(&mut state.grid);
                state.quit_fade.tick();
                if state.quit_fade.is_done() {
                    self.stop.store(true, Ordering::SeqCst);
                }
            }

            writer.write_frame(&state.grid)?;
            writer.flush()?;

            state.frame_idx += 1;

            if self.measure_startup {
                self.stop.store(true, Ordering::SeqCst);
                break;
            }

            if state.frame_idx.is_multiple_of(60) {
                tracing::debug!(target: "clitunes", frame_idx = state.frame_idx, "frame stats");
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
            frames = state.frame_idx,
            "shutdown"
        );

        Ok(())
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
                            } else if pending.len() >= MAX_ESCAPE_LEN {
                                // Unrecognised sequence — discard.
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
    // `n`/`p` used to be viz-nav shortcuts here, but that stole the
    // letters from anyone trying to type them on the Search tab. They
    // now flow through as `PickerKey::Char` — `AppState::handle_key`
    // translates them to viz-nav when the picker isn't typing.
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
