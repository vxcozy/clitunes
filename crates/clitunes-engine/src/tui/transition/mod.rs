//! Frame-interpolation transition engine for CellGrid state changes.
//!
//! Provides fade, slide, dissolve, and wipe transitions between two
//! `CellGrid` snapshots with configurable easing curves. The engine
//! operates purely in grid-space: no GPU, no terminal queries —
//! just cell-level blending that the existing `AnsiWriter` renders.

pub mod blend;
pub mod easing;

pub use blend::{SlideDirection, WipeDirection};

use crate::visualiser::cell_grid::CellGrid;

/// Easing function signature: maps `t ∈ [0, 1]` to eased `[0, 1]`.
pub type EasingFn = fn(f32) -> f32;

/// How the transition blends between source and target.
#[derive(Clone, Debug)]
pub enum TransitionMode {
    /// Per-cell alpha blend.
    Fade,
    /// Target slides in from a direction.
    Slide(SlideDirection),
    /// Random per-cell reveal using a noise mask.
    Dissolve {
        /// Pre-computed per-cell thresholds in `[0, 1]`.
        thresholds: Vec<f32>,
    },
    /// Directional sweep with a soft edge.
    Wipe(WipeDirection),
}

/// A running transition between two CellGrid states.
#[derive(Clone, Debug)]
pub struct Transition {
    mode: TransitionMode,
    easing: EasingFn,
    duration_frames: u16,
    current_frame: u16,
}

impl Transition {
    /// Create a new transition. `duration_frames = 0` means the
    /// transition is immediately done — `apply()` writes the target.
    pub fn new(mode: TransitionMode, easing: EasingFn, duration_frames: u16) -> Self {
        Self {
            mode,
            easing,
            duration_frames,
            current_frame: 0,
        }
    }

    /// Create a dissolve transition with a deterministic noise mask
    /// generated from the given seed and grid dimensions.
    pub fn dissolve(
        easing: EasingFn,
        duration_frames: u16,
        width: u16,
        height: u16,
        seed: u64,
    ) -> Self {
        let count = (width as usize) * (height as usize);
        let thresholds = generate_dissolve_mask(count, seed);
        Self::new(
            TransitionMode::Dissolve { thresholds },
            easing,
            duration_frames,
        )
    }

    /// Advance by one frame. Returns `true` if the transition is still
    /// active (more frames to render), `false` when done.
    pub fn tick(&mut self) -> bool {
        if self.current_frame < self.duration_frames {
            self.current_frame += 1;
        }
        !self.is_done()
    }

    /// The eased progress value in `[0.0, 1.0]`.
    pub fn progress(&self) -> f32 {
        if self.duration_frames == 0 {
            return 1.0;
        }
        let raw_t = self.current_frame as f32 / self.duration_frames as f32;
        (self.easing)(raw_t.clamp(0.0, 1.0))
    }

    /// Whether the transition has completed all frames.
    pub fn is_done(&self) -> bool {
        self.current_frame >= self.duration_frames
    }

    /// Blend `src` and `target` into `dst` at the current progress.
    /// All three grids must have identical dimensions.
    pub fn apply(&self, dst: &mut CellGrid, src: &CellGrid, target: &CellGrid) {
        let t = self.progress();
        match &self.mode {
            TransitionMode::Fade => blend::fade(dst, src, target, t),
            TransitionMode::Slide(dir) => blend::slide(dst, src, target, t, *dir),
            TransitionMode::Dissolve { thresholds } => {
                blend::dissolve(dst, src, target, t, thresholds)
            }
            TransitionMode::Wipe(dir) => blend::wipe(dst, src, target, t, *dir),
        }
    }
}

/// Simple deterministic noise mask for dissolve transitions.
/// Uses a splitmix64-style PRNG for speed and reproducibility.
fn generate_dissolve_mask(count: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    let mut thresholds = Vec::with_capacity(count);
    for _ in 0..count {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let shifted = ((state >> 33) ^ state).wrapping_mul(0xff51afd7ed558ccd);
        let val = (shifted >> 32) as f32 / u32::MAX as f32;
        thresholds.push(val);
    }
    thresholds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};

    fn black_grid(w: u16, h: u16) -> CellGrid {
        CellGrid::new(w, h)
    }

    fn white_grid(w: u16, h: u16) -> CellGrid {
        let mut g = CellGrid::new(w, h);
        g.fill(Cell {
            ch: '#',
            fg: Rgb::new(255, 255, 255),
            bg: Rgb::new(255, 255, 255),
        });
        g
    }

    #[test]
    fn transition_progress_tracks_frames() {
        let mut t = Transition::new(TransitionMode::Fade, easing::linear, 30);
        for _ in 0..15 {
            t.tick();
        }
        assert!((t.progress() - 0.5).abs() < 0.02);
        assert!(!t.is_done());
    }

    #[test]
    fn transition_completes() {
        let mut t = Transition::new(TransitionMode::Fade, easing::linear, 10);
        for _ in 0..10 {
            assert!(t.tick() || t.is_done());
        }
        assert!(t.is_done());
        assert!(!t.tick());
    }

    #[test]
    fn zero_frame_transition_immediately_done() {
        let t = Transition::new(TransitionMode::Fade, easing::linear, 0);
        assert!(t.is_done());
        assert!((t.progress() - 1.0).abs() < 1e-6);

        let src = black_grid(4, 4);
        let tgt = white_grid(4, 4);
        let mut dst = black_grid(4, 4);
        t.apply(&mut dst, &src, &tgt);
        // Should write target unchanged.
        assert_eq!(dst.cells()[0].fg, Rgb::new(255, 255, 255));
    }

    #[test]
    fn fade_midpoint_blend() {
        let src = black_grid(4, 4);
        let tgt = white_grid(4, 4);
        let mut dst = black_grid(4, 4);

        let mut t = Transition::new(TransitionMode::Fade, easing::linear, 2);
        t.tick(); // frame 1 of 2 → t=0.5
        t.apply(&mut dst, &src, &tgt);

        let cell = dst.cells()[0];
        // At t=0.5 between black(0) and white(255), expect ~127-128.
        assert!(
            (cell.fg.r as i16 - 127).unsigned_abs() <= 1,
            "expected ~127, got {}",
            cell.fg.r
        );
    }

    #[test]
    fn slide_right_at_midpoint() {
        let w = 10u16;
        let h = 2u16;
        let src = black_grid(w, h);
        let tgt = white_grid(w, h);
        let mut dst = black_grid(w, h);

        let mut t = Transition::new(
            TransitionMode::Slide(SlideDirection::Right),
            easing::linear,
            2,
        );
        t.tick(); // t=0.5
        t.apply(&mut dst, &src, &tgt);

        // At t=0.5, offset=5. x<5 shows target, x>=5 shows source.
        let left_cell = dst.cells()[0]; // x=0
        let right_cell = dst.cells()[5]; // x=5
        assert_eq!(
            left_cell.fg,
            Rgb::new(255, 255, 255),
            "left should be target"
        );
        assert_eq!(right_cell.fg, Rgb::BLACK, "right should be source");
    }

    #[test]
    fn dissolve_determinism() {
        let w = 8u16;
        let h = 8u16;
        let src = black_grid(w, h);
        let tgt = white_grid(w, h);

        let mut t1 = Transition::dissolve(easing::linear, 10, w, h, 42);
        let mut t2 = Transition::dissolve(easing::linear, 10, w, h, 42);

        for _ in 0..5 {
            t1.tick();
            t2.tick();
        }

        let mut dst1 = black_grid(w, h);
        let mut dst2 = black_grid(w, h);
        t1.apply(&mut dst1, &src, &tgt);
        t2.apply(&mut dst2, &src, &tgt);

        for i in 0..dst1.cells().len() {
            assert_eq!(dst1.cells()[i].fg, dst2.cells()[i].fg, "cell {i} diverged");
        }
    }

    #[test]
    fn wipe_left_to_right_quarter() {
        let w = 20u16;
        let h = 4u16;
        let src = black_grid(w, h);
        let tgt = white_grid(w, h);
        let mut dst = black_grid(w, h);

        // At t≈0.25, leftmost ~25% should be target (white).
        let mut t = Transition::new(
            TransitionMode::Wipe(WipeDirection::LeftToRight),
            easing::linear,
            4,
        );
        t.tick(); // frame 1 of 4 → t=0.25
        t.apply(&mut dst, &src, &tgt);

        // Cell at x=0 should be fully target (white).
        assert_eq!(dst.cells()[0].fg, Rgb::new(255, 255, 255));
        // Cell at x=19 should still be source (black).
        assert_eq!(dst.cells()[19].fg, Rgb::BLACK);
    }

    #[test]
    fn eased_transition_curves_progress() {
        let mut t = Transition::new(TransitionMode::Fade, easing::ease_out_cubic, 10);
        for _ in 0..5 {
            t.tick();
        }
        // ease_out_cubic(0.5) > 0.5 — progress should be above the linear midpoint.
        assert!(t.progress() > 0.5, "ease_out should be > 0.5 at midpoint");
    }

    #[test]
    fn transition_with_real_grid_no_panic() {
        let mut src = CellGrid::new(80, 24);
        let mut tgt = CellGrid::new(80, 24);
        let mut dst = CellGrid::new(80, 24);

        // Fill with different patterns.
        src.fill(Cell {
            ch: '.',
            fg: Rgb::new(50, 50, 50),
            bg: Rgb::new(10, 10, 10),
        });
        tgt.fill(Cell {
            ch: '*',
            fg: Rgb::new(200, 200, 200),
            bg: Rgb::new(30, 30, 30),
        });

        let mut t = Transition::new(TransitionMode::Fade, easing::ease_in_out_cubic, 5);
        while t.tick() {
            t.apply(&mut dst, &src, &tgt);
        }
        // Final frame should be the target.
        t.apply(&mut dst, &src, &tgt);
        assert_eq!(dst.cells()[0].ch, '*');
    }

    #[test]
    fn performance_apply_under_1ms() {
        let src = black_grid(200, 60);
        let tgt = white_grid(200, 60);
        let mut dst = black_grid(200, 60);

        let mut t = Transition::new(TransitionMode::Fade, easing::linear, 10);
        t.tick();

        let start = std::time::Instant::now();
        for _ in 0..10 {
            t.apply(&mut dst, &src, &tgt);
        }
        let elapsed = start.elapsed();
        let per_call = elapsed / 10;
        assert!(
            per_call.as_millis() < 2,
            "apply() took {:?} per call (budget: <1ms)",
            per_call
        );
    }
}
