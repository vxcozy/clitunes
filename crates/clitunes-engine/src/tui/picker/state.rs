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

use crate::tui::picker::curated_seed::CuratedList;

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
}

impl PickerState {
    pub fn new(list: &CuratedList, initial_selection: usize) -> Self {
        let total = list.stations.len();
        Self {
            visible: true,
            selected: initial_selection.min(total.saturating_sub(1)),
            total,
            banner: None,
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn show(&mut self) {
        self.visible = true;
    }

    pub fn move_up(&mut self) {
        if self.total == 0 {
            return;
        }
        if self.selected == 0 {
            self.selected = self.total - 1; // wrap to bottom
        } else {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.total == 0 {
            return;
        }
        self.selected = (self.selected + 1) % self.total;
    }

    /// Handle a key byte from the raw-stdin reader. Arrow keys are
    /// reported as 3-byte escape sequences (`\x1b[A`..`\x1b[D`) but
    /// the caller already deconstructs those into a
    /// [`PickerKey::Up`] / [`PickerKey::Down`] before calling us —
    /// see the `key_from_bytes` helper for mapping.
    pub fn handle_key(&mut self, key: PickerKey) -> PickerAction {
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
                self.move_up();
                PickerAction::Moved
            }
            PickerKey::Down => {
                self.move_down();
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
}
