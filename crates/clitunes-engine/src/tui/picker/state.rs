//! Picker state machine.
//!
//! The picker is a small modal with five meaningful inputs:
//!
//! - `↑` / `k` — move selection up
//! - `↓` / `j` — move selection down
//! - `enter`   — confirm the current selection
//! - `s` / esc — hide the picker without picking (keeping the current source)
//! - `q`       — quit clitunes entirely
//!
//! The state machine is deliberately dumb: it holds a selected index
//! and a visibility flag, and [`PickerState::handle_key`] returns a
//! [`PickerAction`] describing what the caller should do. The main
//! binary is responsible for actually starting/stopping sources and
//! persisting state — the picker just tells it what the user asked
//! for. That keeps this module testable without any threads, channels,
//! or I/O.

use std::time::Instant;

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

/// Result of handling a single key press.
#[derive(Debug, PartialEq, Eq)]
pub enum PickerAction {
    /// Selection moved (or was a no-op at the edge). Caller should
    /// repaint on the next frame.
    Moved,
    /// User pressed enter. Caller should switch to the station at the
    /// given slot, save state, and hide the picker.
    Pick(u8),
    /// User pressed `s` or `esc`. Caller should hide the picker.
    Hide,
    /// User pressed `q`. Caller should shut clitunes down.
    Quit,
    /// Key had no meaning here — caller may route it elsewhere.
    Ignored,
}

/// Live picker state. Held inside the main binary; one instance per
/// clitunes session.
#[derive(Clone, Debug)]
pub struct PickerState {
    pub visible: bool,
    pub selected: usize,
    pub total: usize,
    /// The banner shown in the header when the user's last station
    /// has gone missing (from the "state.toml references a station
    /// that no longer exists" edge case). `None` for the happy path.
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
            selected: initial_selection.min(total.saturating_sub(1)),
            total,
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

    pub fn move_up(&mut self) {
        self.move_up_at(Instant::now());
    }

    pub fn move_down(&mut self) {
        self.move_down_at(Instant::now());
    }

    fn move_up_at(&mut self, now: Instant) {
        if self.total == 0 {
            return;
        }
        let steps = self.update_momentum(now);
        for _ in 0..steps {
            if self.selected == 0 {
                self.selected = self.total - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    fn move_down_at(&mut self, now: Instant) {
        if self.total == 0 {
            return;
        }
        let steps = self.update_momentum(now);
        for _ in 0..steps {
            self.selected = (self.selected + 1) % self.total;
        }
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

    /// Handle a key byte from the raw-stdin reader. Arrow keys are
    /// reported as 3-byte escape sequences (`\x1b[A`..`\x1b[D`) but
    /// the caller already deconstructs those into a
    /// [`PickerKey::Up`] / [`PickerKey::Down`] before calling us —
    /// see the `key_from_bytes` helper for mapping.
    pub fn handle_key(&mut self, key: PickerKey) -> PickerAction {
        self.handle_key_at(key, Instant::now())
    }

    fn handle_key_at(&mut self, key: PickerKey, now: Instant) -> PickerAction {
        if !self.visible {
            // Only `s` reopens the picker when hidden.
            return if matches!(key, PickerKey::ToggleVisibility) {
                self.show();
                PickerAction::Moved
            } else {
                PickerAction::Ignored
            };
        }
        match key {
            PickerKey::Up => {
                self.move_up_at(now);
                PickerAction::Moved
            }
            PickerKey::Down => {
                self.move_down_at(now);
                PickerAction::Moved
            }
            PickerKey::Enter => {
                if self.total == 0 {
                    PickerAction::Ignored
                } else {
                    PickerAction::Pick(self.selected as u8)
                }
            }
            PickerKey::ToggleVisibility | PickerKey::Escape => {
                self.hide();
                PickerAction::Hide
            }
            PickerKey::Quit => PickerAction::Quit,
            PickerKey::Other => PickerAction::Ignored,
        }
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
    Other,
}

/// Translate a raw-stdin byte slice into a [`PickerKey`]. Handles the
/// common ANSI arrow-key escapes (`ESC [ A/B/C/D`), `\r` / `\n` for
/// enter, `s` / `S` for toggle, `q` / `Q` / `\x03` (Ctrl-C) for quit,
/// and bare `ESC` for hide. Bytes we don't recognise return
/// [`PickerKey::Other`].
pub fn key_from_bytes(buf: &[u8]) -> PickerKey {
    match buf {
        [b'\r'] | [b'\n'] => PickerKey::Enter,
        [b's'] | [b'S'] => PickerKey::ToggleVisibility,
        [b'q'] | [b'Q'] | [0x03] => PickerKey::Quit,
        [0x1b] => PickerKey::Escape,
        // Arrow keys: ESC [ A/B/C/D.
        [0x1b, b'[', b'A'] => PickerKey::Up,
        [0x1b, b'[', b'B'] => PickerKey::Down,
        // Vim-style navigation.
        [b'k'] | [b'K'] => PickerKey::Up,
        [b'j'] | [b'J'] => PickerKey::Down,
        _ => PickerKey::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::picker::curated_seed::baked_list;
    use std::time::Duration;

    #[test]
    fn new_clamps_initial_selection() {
        let list = baked_list();
        let st = PickerState::new(&list, 999);
        assert!(st.selected < list.stations.len());
    }

    #[test]
    fn move_down_wraps() {
        let list = baked_list();
        let mut st = PickerState::new(&list, list.stations.len() - 1);
        st.move_down();
        assert_eq!(st.selected, 0);
    }

    #[test]
    fn move_up_wraps_from_top() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.move_up();
        assert_eq!(st.selected, list.stations.len() - 1);
    }

    #[test]
    fn enter_returns_pick_with_current_slot() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 4);
        assert_eq!(st.handle_key(PickerKey::Enter), PickerAction::Pick(4));
    }

    #[test]
    fn s_hides_and_reshows() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        assert_eq!(
            st.handle_key(PickerKey::ToggleVisibility),
            PickerAction::Hide
        );
        assert!(!st.visible);
        assert_eq!(
            st.handle_key(PickerKey::ToggleVisibility),
            PickerAction::Moved
        );
        assert!(st.visible);
    }

    #[test]
    fn escape_hides() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        assert_eq!(st.handle_key(PickerKey::Escape), PickerAction::Hide);
        assert!(!st.visible);
    }

    #[test]
    fn quit_returns_quit() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        assert_eq!(st.handle_key(PickerKey::Quit), PickerAction::Quit);
    }

    #[test]
    fn ignored_when_hidden_except_s() {
        let list = baked_list();
        let mut st = PickerState::new(&list, 0);
        st.hide();
        assert_eq!(st.handle_key(PickerKey::Up), PickerAction::Ignored);
        assert_eq!(st.handle_key(PickerKey::Enter), PickerAction::Ignored);
        assert_eq!(
            st.handle_key(PickerKey::ToggleVisibility),
            PickerAction::Moved
        );
    }

    #[test]
    fn key_from_bytes_recognises_arrows() {
        assert_eq!(key_from_bytes(&[0x1b, b'[', b'A']), PickerKey::Up);
        assert_eq!(key_from_bytes(&[0x1b, b'[', b'B']), PickerKey::Down);
    }

    #[test]
    fn key_from_bytes_recognises_vim_keys() {
        assert_eq!(key_from_bytes(b"j"), PickerKey::Down);
        assert_eq!(key_from_bytes(b"k"), PickerKey::Up);
    }

    #[test]
    fn key_from_bytes_maps_enter_and_ctrl_c() {
        assert_eq!(key_from_bytes(b"\r"), PickerKey::Enter);
        assert_eq!(key_from_bytes(b"\n"), PickerKey::Enter);
        assert_eq!(key_from_bytes(&[0x03]), PickerKey::Quit);
    }

    #[test]
    fn key_from_bytes_unknown_is_other() {
        assert_eq!(key_from_bytes(b"z"), PickerKey::Other);
        assert_eq!(key_from_bytes(b""), PickerKey::Other);
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
        // 5 rapid presses to reach speed 2.
        for i in 0..5 {
            let t = start + Duration::from_millis(i as u64 * 100);
            st.move_down_at(t);
        }
        // After 5 presses at speed 1 then last at speed 2:
        // Press 1-4: move 1 each = 4. Press 5: speed becomes 2, move 2 = 6 total.
        assert_eq!(st.selected, 6);
    }
}
