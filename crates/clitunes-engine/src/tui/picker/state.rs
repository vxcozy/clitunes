//! Picker state machine — tabbed picker for Radio / Search / Library /
//! Settings.
//!
//! The picker is a modal with four tabs:
//!
//! - **Radio** — curated-station list (v1.0 behaviour preserved).
//! - **Search** — free-text Spotify search with paginated track results.
//! - **Library** — user's saved tracks, saved albums, playlists, recently played.
//! - **Settings** — read-only snapshot of the daemon's Spotify / Connect
//!   config + on-disk auth state. No write path in this slice.
//!
//! Like the v1.0 picker, this is a pure state machine: keystrokes in,
//! [`PickerAction`]s out. The render loop owns all I/O (verb dispatch,
//! debounce timers, result-event plumbing) — the picker just describes
//! what the user asked for.
//!
//! # Tabs and focus
//!
//! The active tab owns keyboard focus. Up/Down move that tab's cursor,
//! Enter confirms that tab's selection, printable characters append to
//! the Search tab's query when it's focused. `Tab` cycles
//! Radio → Search → Library → Settings → Radio. Number keys `1`..`4`
//! jump directly to a tab (outside the Search tab, where they are
//! printable).
//!
//! # Why this lives in the state machine
//!
//! A textbook alternative would be three independent components stitched
//! together by the render loop. We keep everything in one state struct
//! so that:
//!
//! - The picker is testable end-to-end with pure data (no channels).
//! - Tab switches preserve per-tab scroll cursors and query strings.
//! - The momentum-scrolling logic from v1.0 is reused unchanged for
//!   each tab's cursor.
//!
//! # Why `PickerAction::Pick(u8)` is retained
//!
//! Radio selection still uses slot indices so
//! `crates/clitunes/src/client/render_loop.rs` can look up the
//! `CuratedList` entry. Spotify selections use
//! [`PickerAction::PickSpotify`] with the URI, because the picker
//! doesn't need to know what verb the caller sends.

use std::time::Instant;

use clitunes_core::{BrowseItem, LibraryCategory};

use crate::tui::picker::curated_seed::CuratedList;

/// Maximum time between keypresses to count as "rapid" for momentum
/// (150 ms). Tuned so that holding an arrow key at typical repeat
/// rates (~80–100 ms) always counts as rapid, while deliberate
/// pauses between individual presses (~200+ ms) always reset.
const MOMENTUM_THRESHOLD_MS: u128 = 150;
/// Rapid-press count before accelerating to speed 2 (move 2 items
/// per keypress). Five presses is roughly 0.5 s of held key.
const ACCEL_TIER_1: usize = 5;
/// Rapid-press count before accelerating to speed 3 (move 3 items
/// per keypress). Ten presses is roughly 1 s of held key.
const ACCEL_TIER_2: usize = 10;

/// Map a rapid-press count to a scroll speed (1, 2, or 3).
fn speed_for_count(count: usize) -> usize {
    if count >= ACCEL_TIER_2 {
        3
    } else if count >= ACCEL_TIER_1 {
        2
    } else {
        1
    }
}

/// Which tab currently owns keyboard focus.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PickerTab {
    Radio,
    Search,
    Library,
    /// Read-only Spotify / daemon config surface. No write path yet —
    /// shows the auth state, device name, and the shell command the
    /// user should run to log in.
    Settings,
}

impl PickerTab {
    /// Cycle Radio → Search → Library → Settings → Radio.
    pub fn next(self) -> Self {
        match self {
            Self::Radio => Self::Search,
            Self::Search => Self::Library,
            Self::Library => Self::Settings,
            Self::Settings => Self::Radio,
        }
    }
}

/// Result of handling a single key press.
#[derive(Debug, PartialEq, Eq)]
pub enum PickerAction {
    /// Selection / cursor / query changed. Caller should repaint.
    Moved,
    /// User confirmed a curated-station slot (Radio tab).
    Pick(u8),
    /// User confirmed a Spotify item (Search or Library tab). Carries
    /// the provider URI — the caller decides whether to play it
    /// directly or drill into it (e.g. playlists).
    PickSpotify(String),
    /// Search query changed. Caller should debounce and send
    /// `Verb::Search`. Empty queries still surface so callers can
    /// clear any in-flight request.
    SearchDirty(String),
    /// User selected a library category. Caller should send
    /// `Verb::BrowseLibrary`.
    BrowseLibrary(LibraryCategory),
    /// User drilled into a playlist. Caller should send
    /// `Verb::BrowsePlaylist` with this id/uri.
    BrowsePlaylist(String),
    /// User opened or refreshed the Settings tab. Caller should send
    /// `Verb::ReadConfig` so the daemon echoes a fresh snapshot.
    ReadConfig,
    /// User pressed `s` or `esc`. Caller should hide the picker.
    Hide,
    /// User pressed `q`. Caller should shut clitunes down.
    Quit,
    /// Key had no meaning here — caller may route it elsewhere.
    Ignored,
}

/// Category rows shown on the Library tab, in display order. Kept
/// alongside the enum so tests and paint stay in sync.
pub const LIBRARY_CATEGORIES: &[LibraryCategory] = &[
    LibraryCategory::SavedTracks,
    LibraryCategory::SavedAlbums,
    LibraryCategory::Playlists,
    LibraryCategory::RecentlyPlayed,
];

/// What the Library tab is currently showing: the category picker, or
/// the flat list of items inside a selected category.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryView {
    /// User is choosing a category (SavedTracks / SavedAlbums / …).
    Categories { cursor: usize },
    /// User is browsing items inside a category. `cursor` indexes
    /// into `items`.
    Items {
        category: LibraryCategory,
        items: Vec<BrowseItem>,
        cursor: usize,
    },
}

impl Default for LibraryView {
    fn default() -> Self {
        Self::Categories { cursor: 0 }
    }
}

/// On-disk auth status the Settings tab renders as a human label.
/// Mirrors `AuthStatusKind` on the wire, but deliberately decoupled so
/// the TUI state machine has no dependency on the proto module.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SettingsAuthStatus {
    LoggedIn,
    LoggedOut,
    ScopesInsufficient,
    Unreadable,
}

/// Read-only snapshot of the daemon config the Settings tab displays.
/// Populated from the `ConfigSnapshot` event the daemon sends back in
/// response to `Verb::ReadConfig`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsSnapshot {
    pub device_name: String,
    pub connect_enabled: bool,
    pub config_path: Option<String>,
    pub credentials_path: Option<String>,
    pub auth_status: SettingsAuthStatus,
    pub auth_detail: Option<String>,
}

/// Live picker state. Owned by the main binary; one instance per
/// session. Pure data — mutated by [`PickerState::handle_key`] and
/// by result-feed setters the render loop calls when events arrive.
#[derive(Clone, Debug)]
pub struct PickerState {
    pub visible: bool,
    pub active_tab: PickerTab,

    /// Radio tab — curated list selection (slot index).
    pub radio_selected: usize,
    pub radio_total: usize,

    /// Search tab — live query + paginated results.
    pub search_query: String,
    pub search_results: Vec<BrowseItem>,
    pub search_cursor: usize,

    /// Library tab — either category chooser or item list.
    pub library: LibraryView,

    /// Settings tab — last config snapshot the daemon sent. `None`
    /// until the first `Verb::ReadConfig` round-trips; the paint code
    /// renders a "loading" line in that case.
    pub settings: Option<SettingsSnapshot>,

    /// Banner shown in the header when the user's last station has
    /// gone missing. `None` on the happy path.
    pub banner: Option<String>,

    /// Momentum scrolling: count of rapid consecutive direction presses.
    rapid_count: usize,
    /// Timestamp of the last direction keypress.
    last_nav_at: Option<Instant>,
}

impl PickerState {
    pub fn new(list: &CuratedList, initial_selection: usize) -> Self {
        let total = list.stations.len();
        Self {
            visible: true,
            active_tab: PickerTab::Radio,
            radio_selected: initial_selection.min(total.saturating_sub(1)),
            radio_total: total,
            search_query: String::new(),
            search_results: Vec::new(),
            search_cursor: 0,
            library: LibraryView::default(),
            settings: None,
            banner: None,
            rapid_count: 0,
            last_nav_at: None,
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn show(&mut self) {
        self.visible = true;
    }

    /// Replace search results (called when `SearchResults` event arrives).
    /// Clamps the cursor to the new length so stale cursors don't point
    /// past the end.
    pub fn set_search_results(&mut self, items: Vec<BrowseItem>) {
        self.search_cursor = self.search_cursor.min(items.len().saturating_sub(1));
        if items.is_empty() {
            self.search_cursor = 0;
        }
        self.search_results = items;
    }

    /// Replace library items for a category. Switches the library view
    /// into the item list if we were still on the category chooser.
    pub fn set_library_items(&mut self, category: LibraryCategory, items: Vec<BrowseItem>) {
        let cursor = 0;
        self.library = LibraryView::Items {
            category,
            items,
            cursor,
        };
    }

    /// Return to the category chooser on the Library tab.
    pub fn reset_library_to_categories(&mut self) {
        self.library = LibraryView::default();
    }

    /// Store a fresh daemon config snapshot on the Settings tab. Called
    /// by the render loop when a `ConfigSnapshot` event arrives.
    pub fn set_settings(&mut self, snapshot: SettingsSnapshot) {
        self.settings = Some(snapshot);
    }

    /// Handle a key. Public entry point used by the render loop.
    pub fn handle_key(&mut self, key: PickerKey) -> PickerAction {
        self.handle_key_at(key, Instant::now())
    }

    fn handle_key_at(&mut self, key: PickerKey, now: Instant) -> PickerAction {
        if !self.visible {
            // Only `s` (ToggleVisibility) reopens the picker when hidden.
            return if matches!(key, PickerKey::ToggleVisibility) {
                self.show();
                PickerAction::Moved
            } else {
                PickerAction::Ignored
            };
        }

        // Global keys — same behaviour on every tab.
        match key {
            PickerKey::Quit => return PickerAction::Quit,
            PickerKey::Tab => {
                self.active_tab = self.active_tab.next();
                if matches!(self.active_tab, PickerTab::Settings) {
                    return PickerAction::ReadConfig;
                }
                return PickerAction::Moved;
            }
            // Numeric keys switch tabs directly. Only active outside the
            // Search input; inside Search these are printable characters
            // routed via `Char(c)` below.
            PickerKey::Char(c @ ('1' | '2' | '3' | '4'))
                if !matches!(self.active_tab, PickerTab::Search) =>
            {
                let target = match c {
                    '1' => PickerTab::Radio,
                    '2' => PickerTab::Search,
                    '3' => PickerTab::Library,
                    '4' => PickerTab::Settings,
                    _ => unreachable!(),
                };
                if self.active_tab != target {
                    self.active_tab = target;
                    if matches!(target, PickerTab::Settings) {
                        return PickerAction::ReadConfig;
                    }
                }
                return PickerAction::Moved;
            }
            PickerKey::ToggleVisibility => {
                // `s` hides from any tab EXCEPT when the user is
                // typing into the Search query — there `s` is a
                // printable character.
                if matches!(self.active_tab, PickerTab::Search) {
                    // Fall through to per-tab handling below.
                } else {
                    self.hide();
                    return PickerAction::Hide;
                }
            }
            PickerKey::Escape => {
                // Escape with a non-empty search query clears the
                // query; otherwise it hides the picker.
                if matches!(self.active_tab, PickerTab::Search) && !self.search_query.is_empty() {
                    self.search_query.clear();
                    self.search_cursor = 0;
                    let snapshot = self.search_query.clone();
                    return PickerAction::SearchDirty(snapshot);
                }
                // Library items view: Escape pops back to the category chooser.
                if matches!(self.active_tab, PickerTab::Library)
                    && matches!(self.library, LibraryView::Items { .. })
                {
                    self.reset_library_to_categories();
                    return PickerAction::Moved;
                }
                self.hide();
                return PickerAction::Hide;
            }
            _ => {}
        }

        match self.active_tab {
            PickerTab::Radio => self.handle_radio(key, now),
            PickerTab::Search => self.handle_search(key, now),
            PickerTab::Library => self.handle_library(key, now),
            PickerTab::Settings => self.handle_settings(key),
        }
    }

    /// Settings tab is read-only in this slice — the only useful key
    /// is `Enter`, which re-requests the snapshot so users can see
    /// auth state updates after running `clitunes auth` elsewhere.
    fn handle_settings(&mut self, key: PickerKey) -> PickerAction {
        match key {
            PickerKey::Enter => PickerAction::ReadConfig,
            _ => PickerAction::Ignored,
        }
    }

    fn handle_radio(&mut self, key: PickerKey, now: Instant) -> PickerAction {
        match key {
            PickerKey::Up => {
                self.move_radio_up(now);
                PickerAction::Moved
            }
            PickerKey::Down => {
                self.move_radio_down(now);
                PickerAction::Moved
            }
            PickerKey::Enter => {
                if self.radio_total == 0 {
                    PickerAction::Ignored
                } else {
                    PickerAction::Pick(self.radio_selected as u8)
                }
            }
            _ => PickerAction::Ignored,
        }
    }

    fn handle_search(&mut self, key: PickerKey, _now: Instant) -> PickerAction {
        match key {
            PickerKey::Up => {
                self.move_search_up();
                PickerAction::Moved
            }
            PickerKey::Down => {
                self.move_search_down();
                PickerAction::Moved
            }
            PickerKey::Enter => match self.search_results.get(self.search_cursor) {
                Some(item) => PickerAction::PickSpotify(item.uri.clone()),
                None => PickerAction::Ignored,
            },
            PickerKey::Char(c) => {
                self.search_query.push(c);
                PickerAction::SearchDirty(self.search_query.clone())
            }
            PickerKey::Backspace => {
                if self.search_query.pop().is_some() {
                    PickerAction::SearchDirty(self.search_query.clone())
                } else {
                    PickerAction::Ignored
                }
            }
            // `s` on Search tab is a printable character, routed above.
            PickerKey::ToggleVisibility => {
                self.search_query.push('s');
                PickerAction::SearchDirty(self.search_query.clone())
            }
            _ => PickerAction::Ignored,
        }
    }

    fn handle_library(&mut self, key: PickerKey, _now: Instant) -> PickerAction {
        match &self.library {
            LibraryView::Categories { cursor } => {
                let cursor = *cursor;
                match key {
                    PickerKey::Up => {
                        let new_cursor = if cursor == 0 {
                            LIBRARY_CATEGORIES.len() - 1
                        } else {
                            cursor - 1
                        };
                        self.library = LibraryView::Categories { cursor: new_cursor };
                        PickerAction::Moved
                    }
                    PickerKey::Down => {
                        let new_cursor = (cursor + 1) % LIBRARY_CATEGORIES.len();
                        self.library = LibraryView::Categories { cursor: new_cursor };
                        PickerAction::Moved
                    }
                    PickerKey::Enter => PickerAction::BrowseLibrary(LIBRARY_CATEGORIES[cursor]),
                    _ => PickerAction::Ignored,
                }
            }
            LibraryView::Items {
                category,
                items,
                cursor,
            } => {
                let category = *category;
                let cursor = *cursor;
                let len = items.len();
                match key {
                    PickerKey::Up => {
                        if let LibraryView::Items { cursor: c, .. } = &mut self.library {
                            if len > 0 {
                                *c = if cursor == 0 { len - 1 } else { cursor - 1 };
                            }
                        }
                        PickerAction::Moved
                    }
                    PickerKey::Down => {
                        if let LibraryView::Items { cursor: c, .. } = &mut self.library {
                            if len > 0 {
                                *c = (cursor + 1) % len;
                            }
                        }
                        PickerAction::Moved
                    }
                    PickerKey::Enter => match items.get(cursor) {
                        Some(item) => {
                            // Playlists drill in; everything else plays.
                            if item.uri.starts_with("spotify:playlist:")
                                && category == LibraryCategory::Playlists
                            {
                                PickerAction::BrowsePlaylist(item.uri.clone())
                            } else {
                                PickerAction::PickSpotify(item.uri.clone())
                            }
                        }
                        None => PickerAction::Ignored,
                    },
                    _ => PickerAction::Ignored,
                }
            }
        }
    }

    fn move_radio_up(&mut self, now: Instant) {
        if self.radio_total == 0 {
            return;
        }
        let steps = self.update_momentum(now);
        for _ in 0..steps {
            if self.radio_selected == 0 {
                self.radio_selected = self.radio_total - 1;
            } else {
                self.radio_selected -= 1;
            }
        }
    }

    fn move_radio_down(&mut self, now: Instant) {
        if self.radio_total == 0 {
            return;
        }
        let steps = self.update_momentum(now);
        for _ in 0..steps {
            self.radio_selected = (self.radio_selected + 1) % self.radio_total;
        }
    }

    /// Move the search cursor up. Unlike Radio, the Search tab doesn't
    /// use momentum — long result lists + fuzzy typing make
    /// acceleration feel unpredictable.
    fn move_search_up(&mut self) {
        let len = self.search_results.len();
        if len == 0 {
            return;
        }
        self.search_cursor = if self.search_cursor == 0 {
            len - 1
        } else {
            self.search_cursor - 1
        };
    }

    fn move_search_down(&mut self) {
        let len = self.search_results.len();
        if len == 0 {
            return;
        }
        self.search_cursor = (self.search_cursor + 1) % len;
    }

    /// Track rapid keypresses and return the number of items to move.
    fn update_momentum(&mut self, now: Instant) -> usize {
        let is_rapid = self
            .last_nav_at
            .map(|prev| now.duration_since(prev).as_millis() < MOMENTUM_THRESHOLD_MS)
            .unwrap_or(false);

        if is_rapid {
            self.rapid_count += 1;
        } else {
            self.rapid_count = 1;
        }
        self.last_nav_at = Some(now);

        speed_for_count(self.rapid_count)
    }

    /// Current momentum scroll speed (1, 2, or 3). For testing.
    #[cfg(test)]
    fn scroll_speed(&self) -> usize {
        speed_for_count(self.rapid_count)
    }
}

/// Canonical key identifier the picker cares about. The main binary's
/// keypress thread translates raw stdin bytes / escape sequences into
/// this enum via [`key_from_bytes`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PickerKey {
    Up,
    Down,
    Enter,
    ToggleVisibility, // `s` hotkey
    Escape,
    Quit,
    /// Tab key — cycle between Radio / Search / Library.
    Tab,
    /// Backspace — delete the last character of the Search query.
    Backspace,
    /// Printable character — appended to the Search query when
    /// the Search tab is focused; otherwise ignored.
    Char(char),
    Other,
}

/// Translate a raw-stdin byte slice into a [`PickerKey`].
///
/// Returns `Other` for sequences that don't map to a meaningful key.
/// Printable ASCII (including `s`/`q`) still maps to the dedicated
/// `ToggleVisibility` / `Quit` variants — the state machine decides
/// whether to treat them as printable based on the active tab.
pub fn key_from_bytes(buf: &[u8]) -> PickerKey {
    match buf {
        [b'\r'] | [b'\n'] => PickerKey::Enter,
        [b'\t'] => PickerKey::Tab,
        [0x7f] | [0x08] => PickerKey::Backspace,
        [b's'] | [b'S'] => PickerKey::ToggleVisibility,
        [b'q'] | [b'Q'] | [0x03] => PickerKey::Quit,
        [0x1b] => PickerKey::Escape,
        // Arrow keys: ESC [ A/B/C/D.
        [0x1b, b'[', b'A'] => PickerKey::Up,
        [0x1b, b'[', b'B'] => PickerKey::Down,
        // Vim-style navigation — kept for Radio tab. The Search tab
        // treats them as printable via `Char(c)` in `classify_pending`.
        [b'k'] | [b'K'] => PickerKey::Up,
        [b'j'] | [b'J'] => PickerKey::Down,
        // Any single printable byte we didn't already claim.
        [b] if (*b >= 0x20 && *b < 0x7f) => PickerKey::Char(*b as char),
        _ => PickerKey::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::picker::curated_seed::baked_list;
    use std::time::Duration;

    fn browse_item(uri: &str, title: &str) -> BrowseItem {
        BrowseItem {
            title: title.into(),
            artist: None,
            album: None,
            uri: uri.into(),
            art_url: None,
            duration_ms: None,
        }
    }

    #[test]
    fn new_clamps_initial_selection() {
        let list = baked_list();
        let st = PickerState::new(&list, 999);
        assert!(st.radio_selected < list.stations.len());
    }

    #[test]
    fn radio_tab_down_wraps() {
        let list = baked_list();
        let mut st = PickerState::new(&list, list.stations.len() - 1);
        assert_eq!(st.handle_key(PickerKey::Down), PickerAction::Moved);
        assert_eq!(st.radio_selected, 0);
    }

    #[test]
    fn radio_tab_enter_returns_pick() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 4);
        assert_eq!(st.handle_key(PickerKey::Enter), PickerAction::Pick(4));
    }

    #[test]
    fn tab_cycles_radio_search_library_settings() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        assert_eq!(st.active_tab, PickerTab::Radio);
        assert_eq!(st.handle_key(PickerKey::Tab), PickerAction::Moved);
        assert_eq!(st.active_tab, PickerTab::Search);
        assert_eq!(st.handle_key(PickerKey::Tab), PickerAction::Moved);
        assert_eq!(st.active_tab, PickerTab::Library);
        // Settings tab is special: entering it asks the caller to fetch
        // a fresh config snapshot.
        assert_eq!(st.handle_key(PickerKey::Tab), PickerAction::ReadConfig);
        assert_eq!(st.active_tab, PickerTab::Settings);
        assert_eq!(st.handle_key(PickerKey::Tab), PickerAction::Moved);
        assert_eq!(st.active_tab, PickerTab::Radio);
    }

    #[test]
    fn numeric_keys_switch_tabs_outside_search() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        assert_eq!(st.handle_key(PickerKey::Char('3')), PickerAction::Moved);
        assert_eq!(st.active_tab, PickerTab::Library);
        assert_eq!(
            st.handle_key(PickerKey::Char('4')),
            PickerAction::ReadConfig
        );
        assert_eq!(st.active_tab, PickerTab::Settings);
        // Re-pressing the same number refreshes nothing and does not
        // double-fire ReadConfig.
        assert_eq!(st.handle_key(PickerKey::Char('4')), PickerAction::Moved);
        assert_eq!(st.handle_key(PickerKey::Char('1')), PickerAction::Moved);
        assert_eq!(st.active_tab, PickerTab::Radio);
    }

    #[test]
    fn numeric_keys_on_search_tab_are_printable() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab); // Search
        assert_eq!(
            st.handle_key(PickerKey::Char('4')),
            PickerAction::SearchDirty("4".into())
        );
        assert_eq!(st.active_tab, PickerTab::Search);
    }

    #[test]
    fn settings_tab_enter_refreshes_snapshot() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.active_tab = PickerTab::Settings;
        assert_eq!(st.handle_key(PickerKey::Enter), PickerAction::ReadConfig);
    }

    #[test]
    fn set_settings_populates_snapshot() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        assert!(st.settings.is_none());
        let snap = SettingsSnapshot {
            device_name: "clitunes".into(),
            connect_enabled: false,
            config_path: Some("/tmp/daemon.toml".into()),
            credentials_path: None,
            auth_status: SettingsAuthStatus::LoggedOut,
            auth_detail: None,
        };
        st.set_settings(snap.clone());
        assert_eq!(st.settings.as_ref(), Some(&snap));
    }

    #[test]
    fn search_tab_char_appends_and_marks_dirty() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab); // Search
        assert_eq!(
            st.handle_key(PickerKey::Char('d')),
            PickerAction::SearchDirty("d".into())
        );
        assert_eq!(
            st.handle_key(PickerKey::Char('p')),
            PickerAction::SearchDirty("dp".into())
        );
        assert_eq!(st.search_query, "dp");
    }

    #[test]
    fn search_tab_s_is_printable_not_hide() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab); // Search
        assert_eq!(
            st.handle_key(PickerKey::ToggleVisibility),
            PickerAction::SearchDirty("s".into())
        );
        assert!(st.visible);
    }

    #[test]
    fn search_tab_backspace_pops_query() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab); // Search
        st.handle_key(PickerKey::Char('a'));
        st.handle_key(PickerKey::Char('b'));
        assert_eq!(
            st.handle_key(PickerKey::Backspace),
            PickerAction::SearchDirty("a".into())
        );
        assert_eq!(st.search_query, "a");
    }

    #[test]
    fn search_tab_backspace_on_empty_is_ignored() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab);
        assert_eq!(st.handle_key(PickerKey::Backspace), PickerAction::Ignored);
    }

    #[test]
    fn search_tab_escape_clears_query_first() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab); // Search
        st.handle_key(PickerKey::Char('x'));
        assert_eq!(
            st.handle_key(PickerKey::Escape),
            PickerAction::SearchDirty(String::new())
        );
        assert!(st.search_query.is_empty());
        assert!(st.visible);
    }

    #[test]
    fn search_tab_escape_on_empty_hides() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab);
        assert_eq!(st.handle_key(PickerKey::Escape), PickerAction::Hide);
        assert!(!st.visible);
    }

    #[test]
    fn search_tab_enter_picks_spotify_uri() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab); // Search
        st.set_search_results(vec![
            browse_item("spotify:track:a", "A"),
            browse_item("spotify:track:b", "B"),
        ]);
        st.handle_key(PickerKey::Down);
        assert_eq!(
            st.handle_key(PickerKey::Enter),
            PickerAction::PickSpotify("spotify:track:b".into())
        );
    }

    #[test]
    fn library_categories_enter_emits_browse_library() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab);
        st.handle_key(PickerKey::Tab); // Library
        assert_eq!(
            st.handle_key(PickerKey::Enter),
            PickerAction::BrowseLibrary(LibraryCategory::SavedTracks)
        );
    }

    #[test]
    fn library_items_enter_picks_or_drills() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab);
        st.handle_key(PickerKey::Tab); // Library
        st.set_library_items(
            LibraryCategory::SavedTracks,
            vec![browse_item("spotify:track:a", "A")],
        );
        assert_eq!(
            st.handle_key(PickerKey::Enter),
            PickerAction::PickSpotify("spotify:track:a".into())
        );

        st.set_library_items(
            LibraryCategory::Playlists,
            vec![browse_item("spotify:playlist:x", "P")],
        );
        assert_eq!(
            st.handle_key(PickerKey::Enter),
            PickerAction::BrowsePlaylist("spotify:playlist:x".into())
        );
    }

    #[test]
    fn library_escape_from_items_returns_to_categories() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab);
        st.handle_key(PickerKey::Tab); // Library
        st.set_library_items(
            LibraryCategory::SavedTracks,
            vec![browse_item("spotify:track:a", "A")],
        );
        assert_eq!(st.handle_key(PickerKey::Escape), PickerAction::Moved);
        assert!(matches!(st.library, LibraryView::Categories { .. }));
        assert!(st.visible);
    }

    #[test]
    fn tab_switch_preserves_radio_and_search_state() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Down);
        let r_sel = st.radio_selected;
        st.handle_key(PickerKey::Tab); // Search
        st.handle_key(PickerKey::Char('h'));
        st.handle_key(PickerKey::Tab); // Library
        st.handle_key(PickerKey::Tab); // Settings
        st.handle_key(PickerKey::Tab); // Radio
        assert_eq!(st.radio_selected, r_sel);
        st.handle_key(PickerKey::Tab); // Search
        assert_eq!(st.search_query, "h");
    }

    #[test]
    fn set_search_results_clamps_cursor() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.search_cursor = 99;
        st.set_search_results(vec![browse_item("spotify:track:a", "A")]);
        assert_eq!(st.search_cursor, 0);
    }

    #[test]
    fn quit_returns_quit_from_any_tab() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.handle_key(PickerKey::Tab); // Search — `q` is printable here in v1.0 logic?
                                       // NOTE: we deliberately treat `q` as Quit even on Search so
                                       // there is always an unambiguous way out. Users can type "q"
                                       // via the Char variant if they need a literal query char
                                       // (rare — they can edit after sending a search).
        assert_eq!(st.handle_key(PickerKey::Quit), PickerAction::Quit);
    }

    #[test]
    fn hidden_picker_only_reopens_on_s() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.hide();
        assert_eq!(st.handle_key(PickerKey::Up), PickerAction::Ignored);
        assert_eq!(st.handle_key(PickerKey::Enter), PickerAction::Ignored);
        assert_eq!(
            st.handle_key(PickerKey::ToggleVisibility),
            PickerAction::Moved
        );
        assert!(st.visible);
    }

    #[test]
    fn key_from_bytes_recognises_tab_and_backspace() {
        assert_eq!(key_from_bytes(b"\t"), PickerKey::Tab);
        assert_eq!(key_from_bytes(&[0x7f]), PickerKey::Backspace);
        assert_eq!(key_from_bytes(&[0x08]), PickerKey::Backspace);
    }

    #[test]
    fn key_from_bytes_recognises_printables() {
        assert_eq!(key_from_bytes(b"a"), PickerKey::Char('a'));
        assert_eq!(key_from_bytes(b"Z"), PickerKey::Char('Z'));
        assert_eq!(key_from_bytes(b" "), PickerKey::Char(' '));
        // `s`/`q`/`j`/`k` keep their legacy meanings for the Radio tab.
        assert_eq!(key_from_bytes(b"s"), PickerKey::ToggleVisibility);
        assert_eq!(key_from_bytes(b"q"), PickerKey::Quit);
        assert_eq!(key_from_bytes(b"j"), PickerKey::Down);
        assert_eq!(key_from_bytes(b"k"), PickerKey::Up);
    }

    #[test]
    fn key_from_bytes_recognises_arrows_and_enter() {
        assert_eq!(key_from_bytes(&[0x1b, b'[', b'A']), PickerKey::Up);
        assert_eq!(key_from_bytes(&[0x1b, b'[', b'B']), PickerKey::Down);
        assert_eq!(key_from_bytes(b"\r"), PickerKey::Enter);
    }

    #[test]
    fn momentum_still_accelerates_on_radio_tab() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        let start = Instant::now();
        for i in 0..5 {
            let t = start + Duration::from_millis(i as u64 * 100);
            st.handle_key_at(PickerKey::Down, t);
        }
        assert_eq!(st.scroll_speed(), 2);
    }

    #[test]
    fn momentum_accelerates_after_5_rapid_presses() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        let start = Instant::now();
        // Simulate 5 rapid keypresses at 100ms intervals.
        for i in 0..5 {
            let t = start + Duration::from_millis(i as u64 * 100);
            st.handle_key_at(PickerKey::Down, t);
        }
        assert_eq!(st.scroll_speed(), 2);
    }

    #[test]
    fn momentum_accelerates_after_10_rapid_presses() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        let start = Instant::now();
        for i in 0..10 {
            let t = start + Duration::from_millis(i as u64 * 100);
            st.handle_key_at(PickerKey::Down, t);
        }
        assert_eq!(st.scroll_speed(), 3);
    }

    #[test]
    fn momentum_resets_on_pause() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        let start = Instant::now();
        // 5 rapid presses → speed 2.
        for i in 0..5 {
            let t = start + Duration::from_millis(i as u64 * 100);
            st.handle_key_at(PickerKey::Down, t);
        }
        assert_eq!(st.scroll_speed(), 2);
        // Pause 200ms then press again → speed resets to 1.
        let pause_t = start + Duration::from_millis(5 * 100 + 200);
        st.handle_key_at(PickerKey::Down, pause_t);
        assert_eq!(st.scroll_speed(), 1);
    }

    #[test]
    fn momentum_moves_multiple_items() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        let start = Instant::now();
        // 5 rapid presses to reach speed 2 on the Radio tab.
        for i in 0..5 {
            let t = start + Duration::from_millis(i as u64 * 100);
            st.handle_key_at(PickerKey::Down, t);
        }
        // Press 1–4 at speed 1 (each moves 1 = 4). Press 5 triggers speed 2
        // (moves 2 = 6 total). Clamped to radio_total.saturating_sub(1) if
        // the curated list is shorter than 6.
        let expected = 6usize.min(st.radio_total.saturating_sub(1));
        assert_eq!(st.radio_selected, expected);
    }
}
