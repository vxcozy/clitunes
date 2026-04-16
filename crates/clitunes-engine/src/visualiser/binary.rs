//! Binary — streaming columns of binary digits (0s and 1s) with scroll
//! speed proportional to audio energy. Each column maps to a frequency
//! band; louder bands scroll faster and glow brighter, producing a cyber
//! rain of data that surges with the music.

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::Cell;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

/// Base scroll speed in cells per frame (even in silence).
const BASE_SPEED: f32 = 0.3;
/// Maximum additional scroll speed contributed by audio energy.
const SPEED_BOOST: f32 = 2.0;

pub struct Binary {
    energy: EnergyTracker,
    scroll_offsets: Vec<f32>,
    last_w: u16,
}

impl Binary {
    pub fn new() -> Self {
        Self {
            energy: EnergyTracker::new(0.5, 0.88, 500.0),
            scroll_offsets: Vec::new(),
            last_w: 0,
        }
    }

    /// Resize scroll_offsets if the grid width changed.
    fn ensure_columns(&mut self, w: u16) {
        if w != self.last_w {
            self.scroll_offsets.clear();
            self.scroll_offsets.resize(w as usize, 0.0);
            self.last_w = w;
        }
    }

    /// Cheap integer hash for deterministic digit generation.
    #[inline]
    fn hash(x: u32, y: u32) -> u32 {
        let mut h = x
            .wrapping_mul(374761393)
            .wrapping_add(y.wrapping_mul(668265263));
        h = (h ^ (h >> 13)).wrapping_mul(1274126177);
        h ^= h >> 16;
        h
    }

    /// Per-column energy from FFT magnitudes, log-compressed to [0, 1].
    fn column_energy(col: u16, width: u16, fft: &FftSnapshot) -> f32 {
        let num_bins = fft.magnitudes.len();
        if num_bins == 0 || width == 0 {
            return 0.0;
        }
        let band = (col as usize) * num_bins / (width as usize);
        let band = band.min(num_bins - 1);
        let mag = fft.magnitudes[band];
        // Log-compress: ln(1 + mag) / ln(1 + 500) keeps the range sane.
        let compressed = (1.0 + mag).ln() / (1.0 + 500.0_f32).ln();
        compressed.clamp(0.0, 1.0)
    }
}

impl Default for Binary {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Binary {
    fn id(&self) -> VisualiserId {
        VisualiserId::Binary
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.energy.update(fft);

        let grid = &mut *ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        self.ensure_columns(w);

        let bg = Rgb::new(0, 4, 0);

        for x in 0..w {
            let col_e = Self::column_energy(x, w, fft);

            // Advance scroll for this column.
            self.scroll_offsets[x as usize] += BASE_SPEED + col_e * SPEED_BOOST;

            let scroll_int = self.scroll_offsets[x as usize] as u32;

            // Brightness from column energy.
            let brightness = col_e;
            let hue = 0.33;
            let sat = lerp(0.6, 0.9, 1.0 - brightness);
            let val = lerp(0.15, 0.9, brightness);
            let (r, g, b) = hsv_to_rgb(hue, sat, val);
            let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

            for y in 0..h {
                let hash_y = (y as u32).wrapping_add(scroll_int);
                let hv = Self::hash(x as u32, hash_y);
                let ch = if hv & 1 == 0 { '0' } else { '1' };
                grid.set(x, y, Cell { ch, fg, bg });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visualiser::cell_grid::CellGrid;

    #[test]
    fn render_paints_binary_digits() {
        let mut vis = Binary::new();
        let fft = FftSnapshot::new(vec![500.0; 64], 48_000, 128);
        let mut grid = CellGrid::new(40, 12);
        let mut ctx = TuiContext { grid: &mut grid };
        vis.render_tui(&mut ctx, &fft);

        for cell in grid.cells() {
            assert!(
                cell.ch == '0' || cell.ch == '1',
                "expected binary digit, got {:?}",
                cell.ch,
            );
        }
    }

    #[test]
    fn output_changes_between_frames() {
        let mut vis = Binary::new();
        let fft = FftSnapshot::new(vec![500.0; 64], 48_000, 128);

        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }
        let snap1 = grid.snapshot();

        {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }
        let snap2 = grid.snapshot();

        let differs = snap1
            .cells()
            .iter()
            .zip(snap2.cells().iter())
            .any(|(a, b)| a.ch != b.ch);
        assert!(
            differs,
            "two consecutive frames should differ due to scroll"
        );
    }

    #[test]
    fn resize_no_panic() {
        let mut vis = Binary::new();
        let fft = FftSnapshot::new(vec![100.0; 64], 48_000, 128);

        let mut grid = CellGrid::new(80, 24);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }

        let mut grid2 = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid2 };
            vis.render_tui(&mut ctx, &fft);
        }
    }

    #[test]
    fn silent_input_still_scrolls() {
        let mut vis = Binary::new();
        let silent = FftSnapshot::new(vec![0.0; 64], 48_000, 128);

        let mut grid = CellGrid::new(20, 10);

        // Capture initial frame.
        {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &silent);
        }
        let snap_initial = grid.snapshot();

        // Run many frames so base_speed accumulates enough to shift by
        // at least one integer cell (0.3 * 10 = 3.0 > 1).
        for _ in 0..9 {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &silent);
        }
        let snap_later = grid.snapshot();

        let differs = snap_initial
            .cells()
            .iter()
            .zip(snap_later.cells().iter())
            .any(|(a, b)| a.ch != b.ch);
        assert!(
            differs,
            "base_speed should cause scrolling even with silent input"
        );
    }
}
