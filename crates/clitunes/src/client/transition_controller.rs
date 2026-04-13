//! Maps application events to visual transitions.
//!
//! The controller sits between the event stream and the render pipeline,
//! translating state changes into Transition instances with appropriate
//! modes and easing curves.

use clitunes_engine::tui::transition::easing;
use clitunes_engine::tui::transition::{Transition, TransitionMode};
use clitunes_engine::visualiser::cell_grid::{Cell, CellGrid, Rgb};

/// Active transition state with the source grid snapshot.
pub struct ActiveTransition {
    pub transition: Transition,
    /// Snapshot of the grid before the state change.
    pub source_grid: CellGrid,
}

/// Brightness multiplier for paused state. 60% is dim enough to
/// clearly signal "paused" while keeping the visualiser artwork
/// readable (matches `breathing::CENTER`).
const PAUSE_BRIGHTNESS: f32 = 0.6;

/// Deterministic seed for dissolve noise masks. Chosen arbitrarily;
/// the exact value doesn't matter as long as it's consistent so the
/// dissolve pattern is reproducible across runs.
const DISSOLVE_SEED: u64 = 0xCAFE;

/// Controller that manages visual transitions for state changes.
#[derive(Default)]
pub struct TransitionController {
    active: Option<ActiveTransition>,
    /// Whether the visualiser is paused (dimmed).
    paused: bool,
    /// Whether the first frame has been rendered (for first-launch fade).
    first_frame_done: bool,
}

impl TransitionController {
    pub fn new() -> Self {
        Self {
            active: None,
            paused: false,
            first_frame_done: false,
        }
    }

    /// Start a first-launch fade from black (15 frames, ease_out_cubic).
    pub fn start_first_launch(&mut self, width: u16, height: u16) {
        if self.first_frame_done {
            return;
        }
        self.first_frame_done = true;
        let source = CellGrid::new(width, height); // all-black
        self.active = Some(ActiveTransition {
            transition: Transition::new(TransitionMode::Fade, easing::ease_out_cubic, 15),
            source_grid: source,
        });
    }

    /// Start a source-switch dissolve (12 frames, ease_out_cubic).
    pub fn start_source_switch(&mut self, current_grid: &CellGrid) {
        self.active = Some(ActiveTransition {
            transition: Transition::dissolve(
                easing::ease_out_cubic,
                12,
                current_grid.width(),
                current_grid.height(),
                DISSOLVE_SEED,
            ),
            source_grid: current_grid.snapshot(),
        });
    }

    /// Start a viz-switch wipe (10 frames, ease_in_out_cubic).
    /// `forward` = true for next (wipe left), false for prev (wipe right).
    pub fn start_viz_switch(&mut self, current_grid: &CellGrid, forward: bool) {
        use clitunes_engine::tui::transition::blend::WipeDirection;
        let dir = if forward {
            WipeDirection::LeftToRight
        } else {
            WipeDirection::RightToLeft
        };
        self.active = Some(ActiveTransition {
            transition: Transition::new(TransitionMode::Wipe(dir), easing::ease_in_out_cubic, 10),
            source_grid: current_grid.snapshot(),
        });
    }

    /// Handle play/pause state change.
    /// Returns true if the paused state changed.
    pub fn set_paused(&mut self, paused: bool, current_grid: &CellGrid) -> bool {
        if self.paused == paused {
            return false;
        }
        self.paused = paused;
        self.active = Some(ActiveTransition {
            transition: Transition::new(TransitionMode::Fade, easing::ease_out_cubic, 6),
            source_grid: current_grid.snapshot(),
        });
        true
    }

    /// Whether the visualiser is currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Apply the active transition and dim effect to the grid.
    /// Call this after the visualiser renders but before ANSI emission.
    pub fn apply(&mut self, grid: &mut CellGrid) {
        // Apply active transition if any.
        if let Some(ref mut at) = self.active {
            let mut dst = CellGrid::new(grid.width(), grid.height());
            at.transition.apply(&mut dst, &at.source_grid, grid);
            grid.copy_from(&dst);
            at.transition.tick();
            if at.transition.is_done() {
                self.active = None;
            }
        }

        // Apply paused dimming.
        if self.paused && self.active.is_none() {
            dim_grid(grid, PAUSE_BRIGHTNESS);
        }
    }

    /// Whether a transition is currently in progress.
    pub fn is_transitioning(&self) -> bool {
        self.active.is_some()
    }
}

/// Multiply every cell's fg and bg by a brightness factor.
fn dim_grid(grid: &mut CellGrid, factor: f32) {
    let w = grid.width();
    let h = grid.height();
    for y in 0..h {
        for x in 0..w {
            let idx = (y as usize) * (w as usize) + (x as usize);
            let cell = grid.cells()[idx];
            grid.set(
                x,
                y,
                Cell {
                    ch: cell.ch,
                    fg: dim_rgb(cell.fg, factor),
                    bg: dim_rgb(cell.bg, factor),
                },
            );
        }
    }
}

fn dim_rgb(c: Rgb, factor: f32) -> Rgb {
    Rgb::new(
        (c.r as f32 * factor).round() as u8,
        (c.g as f32 * factor).round() as u8,
        (c.b as f32 * factor).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_grid() -> CellGrid {
        let mut g = CellGrid::new(10, 5);
        g.fill(Cell {
            ch: '*',
            fg: Rgb::new(200, 100, 50),
            bg: Rgb::new(40, 20, 10),
        });
        g
    }

    #[test]
    fn first_launch_triggers_fade_from_black() {
        let mut ctrl = TransitionController::new();
        ctrl.start_first_launch(10, 5);
        assert!(ctrl.is_transitioning());
    }

    #[test]
    fn first_launch_only_fires_once() {
        let mut ctrl = TransitionController::new();
        ctrl.start_first_launch(10, 5);
        assert!(ctrl.is_transitioning());
        // Exhaust the transition.
        let mut grid = test_grid();
        for _ in 0..20 {
            ctrl.apply(&mut grid);
        }
        assert!(!ctrl.is_transitioning());
        // Second call should do nothing.
        ctrl.start_first_launch(10, 5);
        assert!(!ctrl.is_transitioning());
    }

    #[test]
    fn source_switch_triggers_dissolve() {
        let mut ctrl = TransitionController::new();
        ctrl.first_frame_done = true;
        let grid = test_grid();
        ctrl.start_source_switch(&grid);
        assert!(ctrl.is_transitioning());
    }

    #[test]
    fn viz_switch_triggers_wipe() {
        let mut ctrl = TransitionController::new();
        ctrl.first_frame_done = true;
        let grid = test_grid();
        ctrl.start_viz_switch(&grid, true);
        assert!(ctrl.is_transitioning());
    }

    #[test]
    fn pause_dims_grid() {
        let mut ctrl = TransitionController::new();
        ctrl.first_frame_done = true;
        let grid = test_grid();
        ctrl.set_paused(true, &grid);
        // Exhaust the fade transition.
        let mut g = test_grid();
        for _ in 0..10 {
            ctrl.apply(&mut g);
        }
        // Now the grid should be dimmed.
        let mut g = test_grid();
        ctrl.apply(&mut g);
        let cell = g.cells()[0];
        assert_eq!(cell.fg, Rgb::new(120, 60, 30));
    }

    #[test]
    fn overlapping_transition_cancels_previous() {
        let mut ctrl = TransitionController::new();
        ctrl.first_frame_done = true;
        let grid = test_grid();
        ctrl.start_source_switch(&grid);
        // Advance a few frames.
        let mut g = test_grid();
        for _ in 0..3 {
            ctrl.apply(&mut g);
        }
        assert!(ctrl.is_transitioning());
        // Start another transition — replaces the first.
        ctrl.start_viz_switch(&grid, false);
        assert!(ctrl.is_transitioning());
    }

    #[test]
    fn dim_rgb_correctness() {
        assert_eq!(dim_rgb(Rgb::new(200, 100, 50), 0.6), Rgb::new(120, 60, 30));
        assert_eq!(dim_rgb(Rgb::new(0, 0, 0), 0.6), Rgb::new(0, 0, 0));
        assert_eq!(
            dim_rgb(Rgb::new(255, 255, 255), 1.0),
            Rgb::new(255, 255, 255)
        );
    }
}
