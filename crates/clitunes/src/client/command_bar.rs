//! `:` command bar for the full TUI.
//!
//! A bottom-row overlay that opens on `:`, accepts a visualiser name (with
//! an optional `viz ` verb prefix), fuzzy-matches against the 23-mode
//! catalogue, and dispatches `Verb::Viz { name }` when the user confirms
//! with Enter.
//!
//! This module is the pure state machine — it holds no render dependencies
//! and does not talk to the control bus directly. The outer render loop is
//! responsible for:
//!
//! - translating raw input bytes → [`PickerKey`] via the shared picker
//!   translator;
//! - gating on `:` to call [`CommandBarState::open`] (exact gating predicate
//!   mirrors the picker's: picker hidden OR picker visible + tab != Search);
//! - routing every subsequent key to [`CommandBarState::handle_key`] while
//!   the bar is active;
//! - translating a returned [`CommandBarAction::Submit`] into
//!   `verb_tx.try_send(Verb::Viz { name })`, and surfacing
//!   send-failure as [`CommandBarState::set_error`];
//! - calling [`CommandBarState::tick`] each frame to drive the 250 ms
//!   pending-submit timeout;
//! - calling [`CommandBarState::on_viz_changed`] in the `Event::VizChanged`
//!   handler to close the bar once the daemon confirms the switch.
//!
//! All submit-path semantics (unambiguous vs tied matches, unknown-verb
//! handling, error-clear timing) are enforced here so they are trivially
//! unit-testable without spinning up a terminal.

use std::time::{Duration, Instant};

use clitunes_engine::tui::picker::state::PickerKey;

use super::fuzzy::fuzzy_match;

/// Ack deadline for a dispatched `Verb::Viz`. If no matching `VizChanged`
/// event arrives within this window, the bar surfaces a "daemon not
/// responding" error instead of closing silently.
const ACK_TIMEOUT: Duration = Duration::from_millis(250);

/// A submitted visualiser name the bar is waiting for the daemon to ack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSubmit {
    pub submitted_name: String,
    pub deadline: Instant,
}

/// Return value of [`CommandBarState::handle_key`]. Tells the outer render
/// loop what, if anything, to do in response to the keystroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandBarAction {
    /// The user confirmed a fuzzy-matched name. The outer loop should
    /// dispatch `Verb::Viz { name }` to the daemon. The bar stays active
    /// in a dimmed "submitting..." state until `on_viz_changed` fires or
    /// the ack deadline expires.
    Submit(String),
    /// The user pressed Esc. The bar has already cleared its buffer and
    /// deactivated; the outer loop should remove the overlay.
    Cancel,
    /// No externally observable change; internal state (buffer, error,
    /// pending_submit) may have updated.
    Still,
}

/// Command-bar state machine. Zero render dependencies; fully
/// unit-testable without a terminal.
#[derive(Clone, Debug)]
pub struct CommandBarState {
    active: bool,
    buffer: String,
    cursor: usize, // byte index into buffer
    last_error: Option<String>,
    pending_submit: Option<PendingSubmit>,
    /// Visualiser catalogue the fuzzy matcher searches. Injected at
    /// construction so the state machine has no compile-time coupling to
    /// the carousel's registration order — tests can supply their own
    /// catalogue.
    catalogue: &'static [&'static str],
}

impl CommandBarState {
    pub fn new(catalogue: &'static [&'static str]) -> Self {
        Self {
            active: false,
            buffer: String::new(),
            cursor: 0,
            last_error: None,
            pending_submit: None,
            catalogue,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn pending_submit(&self) -> Option<&PendingSubmit> {
        self.pending_submit.as_ref()
    }

    /// Open the bar. Called by the outer loop when `:` is pressed and the
    /// gating predicate is satisfied. Clears any leftover buffer/error
    /// from a previous session.
    pub fn open(&mut self) {
        self.active = true;
        self.buffer.clear();
        self.cursor = 0;
        self.last_error = None;
        self.pending_submit = None;
    }

    /// Force-set an error string. Called by the outer loop on
    /// `verb_tx.try_send` failure (e.g. channel full).
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.last_error = Some(message.into());
        self.pending_submit = None;
    }

    /// Called from the `Event::VizChanged` handler. If the bar had a
    /// pending submit matching `name`, the bar closes.
    pub fn on_viz_changed(&mut self, name: &str) {
        let matches = matches!(&self.pending_submit, Some(p) if p.submitted_name == name);
        if matches {
            self.pending_submit = None;
            self.active = false;
            self.buffer.clear();
            self.cursor = 0;
            self.last_error = None;
        }
    }

    /// Drive the pending-submit deadline. The outer loop calls this once
    /// per frame with the current `Instant`. If a pending submit has
    /// expired, the bar stays active and surfaces a "daemon not
    /// responding" error so the user can retry or cancel.
    pub fn tick(&mut self, now: Instant) {
        let expired = matches!(&self.pending_submit, Some(p) if p.deadline <= now);
        if expired {
            self.last_error = Some("daemon not responding".into());
            self.pending_submit = None;
        }
    }

    /// Consume one [`PickerKey`] and return a [`CommandBarAction`].
    /// Must only be called while `is_active()` returns `true`.
    pub fn handle_key(&mut self, key: PickerKey, now: Instant) -> CommandBarAction {
        match key {
            PickerKey::Escape => {
                self.cancel();
                CommandBarAction::Cancel
            }
            PickerKey::Enter => self.submit(now),
            PickerKey::Backspace => {
                if self.cursor > 0 {
                    // cursor is a byte index; walk back one char boundary.
                    let prev = self.buffer[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.buffer.drain(prev..self.cursor);
                    self.cursor = prev;
                }
                self.last_error = None;
                CommandBarAction::Still
            }
            PickerKey::Char(c) => {
                let s = c.to_string();
                self.buffer.insert_str(self.cursor, &s);
                self.cursor += s.len();
                // User has started typing new intent — stale errors clear.
                self.last_error = None;
                CommandBarAction::Still
            }
            // Keys that collide with picker-native semantics when the bar
            // is active are treated as literal chars so the user can still
            // type names containing them. 's'/'q' fall into ToggleVisibility
            // / Quit respectively but while the bar is active we want them
            // as characters.
            PickerKey::ToggleVisibility => {
                self.insert_char('s');
                CommandBarAction::Still
            }
            PickerKey::Quit => {
                self.insert_char('q');
                CommandBarAction::Still
            }
            // Up/Down/Tab/Other don't do anything in the bar today. A
            // future version might use Tab for autocomplete or Up/Down to
            // page through match candidates, but out of v1.3 scope.
            PickerKey::Up | PickerKey::Down | PickerKey::Tab | PickerKey::Other => {
                CommandBarAction::Still
            }
        }
    }

    fn cancel(&mut self) {
        self.active = false;
        self.buffer.clear();
        self.cursor = 0;
        self.last_error = None;
        self.pending_submit = None;
    }

    fn insert_char(&mut self, c: char) {
        let s = c.to_string();
        self.buffer.insert_str(self.cursor, &s);
        self.cursor += s.len();
        self.last_error = None;
    }

    fn submit(&mut self, now: Instant) -> CommandBarAction {
        // Empty / whitespace-only buffer on Enter = no-op (not an error,
        // per plan R3).
        if self.buffer.trim().is_empty() {
            return CommandBarAction::Still;
        }

        // Parse: optional leading "viz " verb. Any other leading word +
        // space counts as an unknown command. Pass the raw buffer so the
        // parser can distinguish "viz" (the lone verb, waiting for a name)
        // from "viz<something>" (a name starting with those letters — not
        // possible today but keeps the grammar simple).
        let query = match parse_command(&self.buffer) {
            Ok(q) => q,
            Err(verb) => {
                self.last_error = Some(format!("unknown command: {verb}"));
                return CommandBarAction::Still;
            }
        };

        // Query might still be empty after stripping the "viz " prefix
        // (e.g. buffer = "viz "). Treat as empty-buffer case.
        if query.is_empty() {
            return CommandBarAction::Still;
        }

        let matches = fuzzy_match(query, self.catalogue);
        if matches.is_empty() {
            self.last_error = Some("(no match)".into());
            return CommandBarAction::Still;
        }

        // Unambiguous top: single candidate, or top score strictly beats
        // 2nd. Otherwise require the user to narrow the query.
        let top_score = matches[0].1;
        let unambiguous =
            matches.len() == 1 || matches.get(1).map(|(_, s)| *s < top_score).unwrap_or(true);
        if !unambiguous {
            let hint: Vec<&str> = matches
                .iter()
                .take(3)
                .filter(|(_, s)| *s == top_score)
                .map(|(n, _)| *n)
                .collect();
            self.last_error = Some(format!("did you mean: {}?", hint.join(" ")));
            return CommandBarAction::Still;
        }

        let name = matches[0].0.to_string();
        self.pending_submit = Some(PendingSubmit {
            submitted_name: name.clone(),
            deadline: now + ACK_TIMEOUT,
        });
        // Submit keeps the bar visible dimmed — `active` stays true until
        // `on_viz_changed` or `tick` past deadline closes it.
        CommandBarAction::Submit(name)
    }
}

/// Parse a command-bar buffer.
///
/// Syntax is `:viz <name>` or the shorthand `:<name>`. Any other verb
/// (e.g. `source xyz`, `volume 50`) is rejected as unknown — v1.3 keeps
/// the grammar closed to leave room for future verbs to add cleanly.
///
/// On success, returns the (possibly prefix-stripped) query to
/// fuzzy-match. On unknown verb, returns `Err(verb)`.
fn parse_command(buffer: &str) -> Result<&str, &str> {
    let head = buffer.trim_start();
    match head.find(' ') {
        // No space = bare word. "viz" on its own is the lone verb with
        // no arg — treat as empty query so Enter is a no-op (user is
        // mid-typing). Any other bare word is a name query.
        None => {
            if head == "viz" {
                Ok("")
            } else {
                Ok(head.trim_end())
            }
        }
        Some(i) => {
            let (verb, rest) = head.split_at(i);
            if verb == "viz" {
                Ok(rest.trim())
            } else {
                Err(verb)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOGUE: &[&str] = &[
        "plasma",
        "ripples",
        "tunnel",
        "metaballs",
        "vortex",
        "fire",
        "matrix",
        "moire",
        "wave",
        "scope",
        "heartbeat",
        "classicpeak",
        "barsdot",
        "barsoutline",
        "binary",
        "scatter",
        "terrain",
        "butterfly",
        "pulse",
        "rain",
        "sakura",
        "firework",
        "retro",
    ];

    fn new_bar() -> CommandBarState {
        CommandBarState::new(CATALOGUE)
    }

    fn typed(bar: &mut CommandBarState, chars: &str, now: Instant) {
        for c in chars.chars() {
            bar.handle_key(PickerKey::Char(c), now);
        }
    }

    #[test]
    fn open_activates_and_clears() {
        let mut bar = new_bar();
        assert!(!bar.is_active());
        bar.open();
        assert!(bar.is_active());
        assert_eq!(bar.buffer(), "");
        assert_eq!(bar.cursor(), 0);
        assert!(bar.last_error().is_none());
    }

    #[test]
    fn chars_append_and_clear_errors() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        bar.set_error("stale");
        assert_eq!(bar.last_error(), Some("stale"));
        typed(&mut bar, "sak", now);
        assert_eq!(bar.buffer(), "sak");
        assert_eq!(bar.cursor(), 3);
        assert!(
            bar.last_error().is_none(),
            "char input should clear last_error"
        );
    }

    #[test]
    fn backspace_deletes_one_char() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "sak", now);
        bar.handle_key(PickerKey::Backspace, now);
        assert_eq!(bar.buffer(), "sa");
        assert_eq!(bar.cursor(), 2);
    }

    #[test]
    fn exact_name_submits_cleanly() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "sakura", now);
        let action = bar.handle_key(PickerKey::Enter, now);
        assert_eq!(action, CommandBarAction::Submit("sakura".into()));
        assert!(bar.is_active(), "bar stays open until ack");
        let pending = bar.pending_submit().expect("pending_submit set");
        assert_eq!(pending.submitted_name, "sakura");
    }

    #[test]
    fn unambiguous_partial_submits_top_match() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "sak", now);
        let action = bar.handle_key(PickerKey::Enter, now);
        assert_eq!(action, CommandBarAction::Submit("sakura".into()));
    }

    #[test]
    fn tied_top_refuses_submit_and_shows_candidates() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "b", now);
        let action = bar.handle_key(PickerKey::Enter, now);
        assert_eq!(action, CommandBarAction::Still);
        let err = bar.last_error().expect("should have error hint");
        assert!(
            err.starts_with("did you mean:"),
            "expected disambiguation hint, got {err:?}"
        );
        // 4 candidates tied at score 4 (barsdot / barsoutline / binary /
        // butterfly) — top 3 shown.
        for cand in ["barsdot", "barsoutline", "binary"] {
            assert!(err.contains(cand), "hint should list {cand}: {err}");
        }
        // Buffer preserved so user can refine.
        assert_eq!(bar.buffer(), "b");
    }

    #[test]
    fn no_match_shows_error_preserves_buffer() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "xyz", now);
        let action = bar.handle_key(PickerKey::Enter, now);
        assert_eq!(action, CommandBarAction::Still);
        assert_eq!(bar.last_error(), Some("(no match)"));
        assert_eq!(bar.buffer(), "xyz");
    }

    #[test]
    fn unknown_verb_rejected() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "source foo", now);
        let action = bar.handle_key(PickerKey::Enter, now);
        assert_eq!(action, CommandBarAction::Still);
        assert_eq!(bar.last_error(), Some("unknown command: source"));
    }

    #[test]
    fn viz_prefix_accepted() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "viz sakura", now);
        let action = bar.handle_key(PickerKey::Enter, now);
        assert_eq!(action, CommandBarAction::Submit("sakura".into()));
    }

    #[test]
    fn empty_buffer_enter_is_noop_not_error() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        let action = bar.handle_key(PickerKey::Enter, now);
        assert_eq!(action, CommandBarAction::Still);
        assert!(bar.last_error().is_none());
        assert!(bar.pending_submit().is_none());
    }

    #[test]
    fn viz_with_no_arg_is_noop() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "viz ", now);
        let action = bar.handle_key(PickerKey::Enter, now);
        assert_eq!(action, CommandBarAction::Still);
        assert!(bar.last_error().is_none());
    }

    #[test]
    fn on_viz_changed_matching_closes_bar() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "sakura", now);
        bar.handle_key(PickerKey::Enter, now);
        assert!(bar.is_active());
        bar.on_viz_changed("sakura");
        assert!(!bar.is_active());
        assert!(bar.pending_submit().is_none());
        assert_eq!(bar.buffer(), "");
    }

    #[test]
    fn on_viz_changed_mismatched_does_not_close() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "sakura", now);
        bar.handle_key(PickerKey::Enter, now);
        bar.on_viz_changed("matrix"); // different mode
        assert!(bar.is_active(), "should stay open on unrelated VizChanged");
        assert!(bar.pending_submit().is_some());
    }

    #[test]
    fn tick_past_deadline_surfaces_error() {
        let start = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "sakura", start);
        bar.handle_key(PickerKey::Enter, start);
        assert!(bar.pending_submit().is_some());
        // Simulate time passing beyond the 250ms window.
        let later = start + Duration::from_millis(500);
        bar.tick(later);
        assert_eq!(bar.last_error(), Some("daemon not responding"));
        assert!(bar.pending_submit().is_none());
        // Bar stays open so user can retry / cancel.
        assert!(bar.is_active());
    }

    #[test]
    fn escape_cancels_and_clears_everything() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "sakura", now);
        bar.handle_key(PickerKey::Enter, now);
        let action = bar.handle_key(PickerKey::Escape, now);
        assert_eq!(action, CommandBarAction::Cancel);
        assert!(!bar.is_active());
        assert!(bar.pending_submit().is_none());
        assert_eq!(bar.buffer(), "");
    }

    #[test]
    fn set_error_from_outer_loop() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        typed(&mut bar, "sakura", now);
        bar.handle_key(PickerKey::Enter, now);
        bar.set_error("queue full — try again");
        assert_eq!(bar.last_error(), Some("queue full — try again"));
        // set_error should clear the pending submit so tick doesn't also
        // fire a "daemon not responding" on top of the queue error.
        assert!(bar.pending_submit().is_none());
    }

    #[test]
    fn toggle_visibility_and_quit_type_as_chars_while_active() {
        let now = Instant::now();
        let mut bar = new_bar();
        bar.open();
        // Picker-level 's' / 'q' mappings should be treated as literal
        // characters so the user can type words like "scope" or "sakura"
        // or a hypothetical mode starting with 'q'.
        bar.handle_key(PickerKey::ToggleVisibility, now);
        bar.handle_key(PickerKey::Char('c'), now);
        bar.handle_key(PickerKey::Char('o'), now);
        bar.handle_key(PickerKey::Char('p'), now);
        bar.handle_key(PickerKey::Char('e'), now);
        assert_eq!(bar.buffer(), "scope");
        let action = bar.handle_key(PickerKey::Enter, now);
        assert_eq!(action, CommandBarAction::Submit("scope".into()));
    }
}
