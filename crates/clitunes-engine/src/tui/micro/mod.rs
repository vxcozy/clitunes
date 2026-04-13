//! Micro-interactions: shimmer, volume overlay, error pulse, quit fade, breathing.
//!
//! All grid-painting micro-interactions implement [`Overlay`] so the
//! render loop can tick and apply them uniformly. Modifier-only
//! animations (shimmer, breathing, error pulse) expose their own
//! accessors because they feed values into other render stages rather
//! than painting pixels themselves.

pub mod breathing;
pub mod error_pulse;
pub mod quit_fade;
pub mod shimmer;
pub mod volume_overlay;

pub use breathing::BreathingAnimation;
pub use error_pulse::ErrorPulse;
pub use quit_fade::QuitFade;
pub use shimmer::ShimmerAnimation;
pub use volume_overlay::VolumeOverlay;

use crate::tui::theme::Theme;
use crate::visualiser::cell_grid::CellGrid;

/// A micro-interaction that paints directly into the cell grid.
///
/// Implementors are ticked once per frame and, when active, render
/// their effect on top of the current grid contents. The render loop
/// iterates a `Vec<Box<dyn Overlay>>` instead of calling each type
/// by name — adding a new overlay doesn't require changing the loop.
pub trait Overlay {
    /// Advance internal timers by one frame.
    fn tick(&mut self);

    /// Whether this overlay is currently visible / active.
    fn is_active(&self) -> bool;

    /// Paint the overlay into `grid`. Only called when `is_active()`
    /// returns `true`, but implementations should be safe to call
    /// at any time (no-op when inactive).
    fn apply(&mut self, grid: &mut CellGrid, theme: &Theme);
}
