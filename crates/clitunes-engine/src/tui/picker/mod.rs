//! Curated taste-neutral station picker (Unit 8).
//!
//! The picker is the first thing a new user sees: a modal overlay
//! painted over the Auralis calibration tone, listing 8–15 curated
//! stations that span genres without reflecting the engineer's taste.
//!
//! # Why "taste-neutral"
//!
//! Auto-memory rule `feedback_no_taste_imposition.md` is explicit:
//! **never hardcode defaults that reflect developer taste**. The
//! curated list is therefore documented per slot in
//! `docs/curation/2026-04-11-curated-stations.md` with a rationale
//! explaining why that slot exists (genre coverage, regional balance,
//! stream stability) — not "I like this station."
//!
//! # Override path
//!
//! Users who want a different seed can drop a file at
//! `~/.config/clitunes/curated_stations.toml` and [`load_curated`]
//! will prefer it over the baked list. The override is strict: if the
//! file exists but is empty or malformed, we fall back to the baked
//! list and log a warning — this matches the plan's edge case and
//! means "I messed up my override file" never leaves the user staring
//! at a blank picker.
//!
//! # Submodules
//!
//! - [`curated_seed`] — baked-in slot list + override loader
//! - `paint` / `state` — the picker UI itself

pub mod curated_seed;
pub mod paint;
pub mod state;

pub use curated_seed::{load_curated, CuratedList, CuratedLoadOutcome, CURATED_SLOT_COUNT};
pub use paint::{paint_picker, Rect};
pub use state::{
    key_from_bytes, LibraryView, PickerAction, PickerKey, PickerState, PickerTab,
    LIBRARY_CATEGORIES,
};

use crate::tui::transition::easing;
use crate::tui::transition::{Transition, TransitionMode};

/// Fade-in duration (8 frames = 267 ms at 30 fps). Slightly longer
/// than fade-out so the picker feels like it settles in gently.
const PICKER_FADE_IN_FRAMES: u16 = 8;
/// Fade-out duration (6 frames = 200 ms at 30 fps). Shorter than
/// fade-in so dismissal feels snappy and doesn't delay the user.
const PICKER_FADE_OUT_FRAMES: u16 = 6;

/// Picker transition state for fade-in and fade-out.
#[derive(Clone, Debug, Default)]
pub enum PickerTransition {
    /// No transition in progress — picker is fully visible or fully hidden.
    #[default]
    Idle,
    /// Fading in (`PICKER_FADE_IN_FRAMES`, ease_out_cubic).
    FadingIn(Transition),
    /// Fading out (`PICKER_FADE_OUT_FRAMES`, ease_in_cubic). Picker
    /// should remain painted until the fade completes.
    FadingOut(Transition),
}

impl PickerTransition {
    /// Start a fade-in transition.
    pub fn start_fade_in() -> Self {
        Self::FadingIn(Transition::new(
            TransitionMode::Fade,
            easing::ease_out_cubic,
            PICKER_FADE_IN_FRAMES,
        ))
    }

    /// Start a fade-out transition.
    pub fn start_fade_out() -> Self {
        Self::FadingOut(Transition::new(
            TransitionMode::Fade,
            easing::ease_in_cubic,
            PICKER_FADE_OUT_FRAMES,
        ))
    }

    /// Advance by one frame. Returns `true` if the transition is still active.
    pub fn tick(&mut self) -> bool {
        match self {
            Self::Idle => false,
            Self::FadingIn(t) | Self::FadingOut(t) => {
                let still_active = t.tick();
                if !still_active {
                    *self = Self::Idle;
                }
                still_active
            }
        }
    }

    /// Whether the picker should be painted this frame (visible or fading out).
    pub fn should_paint_picker(&self, picker_visible: bool) -> bool {
        match self {
            Self::Idle => picker_visible,
            Self::FadingIn(_) => true,
            Self::FadingOut(_) => true,
        }
    }

    /// Whether a fade is currently running.
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// Get the underlying transition for applying blending, if active.
    pub fn transition(&self) -> Option<&Transition> {
        match self {
            Self::Idle => None,
            Self::FadingIn(t) | Self::FadingOut(t) => Some(t),
        }
    }

    /// Whether this is a fade-out (for inversion of blend direction).
    pub fn is_fading_out(&self) -> bool {
        matches!(self, Self::FadingOut(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_in_completes_in_expected_frames() {
        let mut pt = PickerTransition::start_fade_in();
        let mut active_ticks = 0;
        while pt.tick() {
            active_ticks += 1;
        }
        // tick() advances one frame and returns false when the
        // transition finishes, so we see PICKER_FADE_IN_FRAMES - 1
        // active ticks before the final tick returns false.
        assert_eq!(active_ticks, PICKER_FADE_IN_FRAMES - 1);
        assert!(!pt.is_active());
    }

    #[test]
    fn fade_out_completes_in_expected_frames() {
        let mut pt = PickerTransition::start_fade_out();
        let mut active_ticks = 0;
        while pt.tick() {
            active_ticks += 1;
        }
        assert_eq!(active_ticks, PICKER_FADE_OUT_FRAMES - 1);
        assert!(!pt.is_active());
    }

    #[test]
    fn should_paint_during_fade_out() {
        let pt = PickerTransition::start_fade_out();
        assert!(pt.should_paint_picker(false));
    }

    #[test]
    fn idle_defers_to_visibility() {
        let pt = PickerTransition::Idle;
        assert!(pt.should_paint_picker(true));
        assert!(!pt.should_paint_picker(false));
    }
}
