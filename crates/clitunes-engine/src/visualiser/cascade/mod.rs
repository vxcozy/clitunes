//! Cascade — spectrogram waterfall visualiser.
//!
//! Renders a scrolling spectrogram where time flows downward and
//! frequency runs left-to-right on a log scale. Each frame's FFT
//! magnitudes are rebinned to the grid width, log-compressed, pushed
//! into a rolling [`History`], and then painted into the [`CellGrid`]
//! using viridis colouring and upper-half blocks for 2x vertical
//! resolution.

mod colormap;
mod history;

use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};
use crate::visualiser::{SurfaceKind, TuiContext, Visualiser, VisualiserId};

use self::colormap::viridis;
use self::history::History;

pub struct Cascade {
    history: History,
    #[allow(dead_code)]
    start: Instant,
}

impl Cascade {
    pub fn new() -> Self {
        Self {
            history: History::new(),
            start: Instant::now(),
        }
    }
}

impl Default for Cascade {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Cascade {
    fn id(&self) -> VisualiserId {
        VisualiserId::Cascade
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        let grid: &mut CellGrid = &mut *ctx.grid;
        let w = grid.width() as usize;
        let h = grid.height() as usize;
        if w == 0 || h == 0 {
            return;
        }

        // 1. Rebin FFT magnitudes into `w` bins using log-scale bucketing.
        let rebinned = rebin_log(&fft.magnitudes, w);

        // 2. Log-compress each bin into [0, 1].
        let compressed: Vec<f32> = rebinned
            .iter()
            .map(|&mag| (1.0 + mag / 1000.0).ln().min(1.0))
            .collect();

        // 3. Push into history.
        self.history.push_row(compressed);

        // 4-6. Render the grid.
        // Each grid row covers two virtual pixels (upper-half block).
        // Total virtual rows needed = h * 2.
        let virt_rows = h * 2;
        let hist_len = self.history.len();

        // The newest history row maps to the bottom-most virtual row
        // (virt_rows - 1). When history is shorter than the grid, the
        // top virtual rows are black. `first_hist_virt` is the virtual
        // row index at which history row 0 appears.
        let first_hist_virt = virt_rows.saturating_sub(hist_len);
        // When history exceeds the grid, skip the oldest rows.
        let hist_skip = hist_len.saturating_sub(virt_rows);

        for y in 0..h {
            for x in 0..w {
                let top_virt = y * 2;
                let bot_virt = y * 2 + 1;

                let fg = virt_to_color(&self.history, top_virt, first_hist_virt, hist_skip, x);
                let bg = virt_to_color(&self.history, bot_virt, first_hist_virt, hist_skip, x);

                grid.set(
                    x as u16,
                    y as u16,
                    Cell {
                        ch: Cell::UPPER_BLOCK,
                        fg,
                        bg,
                    },
                );
            }
        }
    }
}

/// Map a virtual-pixel row to a history row and return its colour.
/// `first_hist_virt` is the virtual row at which history row 0 appears
/// (rows above it are black). `hist_skip` is the number of oldest
/// history rows to skip when the history is taller than the grid.
fn virt_to_color(
    history: &History,
    virt_row: usize,
    first_hist_virt: usize,
    hist_skip: usize,
    x: usize,
) -> Rgb {
    if virt_row < first_hist_virt {
        return Rgb::BLACK;
    }
    let hist_idx = hist_skip + (virt_row - first_hist_virt);
    match history.get(hist_idx) {
        Some(row) if x < row.len() => viridis(row[x]),
        _ => Rgb::BLACK,
    }
}

/// Rebin FFT magnitudes into `num_bins` buckets using log-scale spacing.
/// Low frequencies get fewer (narrower) bins; high frequencies get wider
/// bins, matching human pitch perception.
fn rebin_log(magnitudes: &[f32], num_bins: usize) -> Vec<f32> {
    if magnitudes.is_empty() {
        return vec![0.0; num_bins];
    }
    let bin_count = magnitudes.len();
    let max_log = ((bin_count - 1) as f32).ln().max(1.0);
    let mut out = vec![0.0; num_bins];
    for (bar, slot) in out.iter_mut().enumerate() {
        let lo_log = (bar as f32 / num_bins as f32) * max_log;
        let hi_log = ((bar + 1) as f32 / num_bins as f32) * max_log;
        let lo = (lo_log.exp().round() as usize).min(bin_count - 1);
        let hi = (hi_log.exp().round() as usize).clamp(lo + 1, bin_count);
        *slot = magnitudes[lo..hi].iter().cloned().fold(0.0_f32, f32::max);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_paints_whole_grid() {
        let mut cascade = Cascade::new();
        let fft = FftSnapshot::new(vec![500.0; 128], 48_000, 256);
        let mut grid = CellGrid::new(40, 12);

        // Push enough frames to fill the grid (12 rows * 2 virtual = 24).
        for _ in 0..30 {
            let mut ctx = TuiContext { grid: &mut grid };
            cascade.render_tui(&mut ctx, &fft);
        }

        // Every cell should have the upper-block glyph.
        for c in grid.cells() {
            assert_eq!(c.ch, Cell::UPPER_BLOCK);
        }

        // With non-zero FFT data, cells should have non-black colours.
        let non_black = grid
            .cells()
            .iter()
            .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
            .count();
        assert!(
            non_black > 0,
            "expected coloured cells from non-zero FFT data"
        );
    }

    #[test]
    fn silent_input_produces_dark_output() {
        let mut cascade = Cascade::new();
        let fft = FftSnapshot::new(vec![0.0; 128], 48_000, 256);
        let mut grid = CellGrid::new(40, 12);

        for _ in 0..30 {
            let mut ctx = TuiContext { grid: &mut grid };
            cascade.render_tui(&mut ctx, &fft);
        }

        // All colours should be dark (viridis(0) = dark purple, so allow
        // small values but no bright pixels).
        for c in grid.cells() {
            let max_component =
                c.fg.r
                    .max(c.fg.g)
                    .max(c.fg.b)
                    .max(c.bg.r)
                    .max(c.bg.g)
                    .max(c.bg.b);
            assert!(
                max_component < 100,
                "silent input should produce dark cells, got max component {max_component}"
            );
        }
    }

    #[test]
    fn rebin_log_produces_correct_length() {
        let mags = vec![1.0; 256];
        let out = rebin_log(&mags, 40);
        assert_eq!(out.len(), 40);
    }

    #[test]
    fn empty_magnitudes_do_not_panic() {
        let mut cascade = Cascade::new();
        let fft = FftSnapshot::new(vec![], 48_000, 0);
        let mut grid = CellGrid::new(10, 5);
        let mut ctx = TuiContext { grid: &mut grid };
        cascade.render_tui(&mut ctx, &fft);
    }
}
