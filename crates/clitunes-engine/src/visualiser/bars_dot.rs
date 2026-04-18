//! BarsDot — braille-stippled spectrum bar visualiser.

use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::f32_to_u8;
use crate::visualiser::scaling::SpectrumScaler;
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

pub struct BarsDot {
    energy: EnergyTracker,
    scaler: SpectrumScaler,
    braille: BrailleBuffer,
    bar_heights: Vec<f32>,
    raw_bands: Vec<f32>,
    last_band_count: usize,
    last_w: u16,
    last_h: u16,
    frame: u32,
}

impl BarsDot {
    pub fn new() -> Self {
        Self {
            // Release tau ~115 ms so overlay brightness tracks the beat.
            energy: EnergyTracker::new(0.5, 0.75, 500.0),
            scaler: SpectrumScaler::new(),
            braille: BrailleBuffer::new(1, 1),
            bar_heights: Vec::new(),
            raw_bands: Vec::new(),
            last_band_count: 0,
            last_w: 0,
            last_h: 0,
            frame: 0,
        }
    }

    fn ensure_buf(&mut self, w: u16, h: u16) {
        if self.last_w != w || self.last_h != h {
            self.braille.resize(w, h);
            self.last_w = w;
            self.last_h = h;
        }
    }

    fn bands_from_fft(&self, fft: &FftSnapshot, num_bands: usize) -> Vec<f32> {
        let bin_count = fft.magnitudes.len().max(1);
        let max_log = ((bin_count - 1).max(1) as f32).ln().max(1.0);
        let mut out = vec![0.0_f32; num_bands];
        for (band, slot) in out.iter_mut().enumerate() {
            let lo_log = (band as f32 / num_bands as f32) * max_log;
            let hi_log = ((band + 1) as f32 / num_bands as f32) * max_log;
            let lo = (lo_log.exp().round() as usize).min(bin_count - 1);
            let hi = (hi_log.exp().round() as usize).clamp(lo + 1, bin_count);
            let slice = &fft.magnitudes[lo..hi];
            *slot = slice.iter().cloned().fold(0.0_f32, f32::max);
        }
        out
    }

    fn smooth_bars(&mut self, raw: &[f32]) {
        if self.bar_heights.len() != raw.len() {
            self.bar_heights.resize(raw.len(), 0.0);
            self.last_band_count = raw.len();
        }
        for (i, &r) in raw.iter().enumerate() {
            let prev = self.bar_heights[i];
            if r > prev {
                self.bar_heights[i] = 0.5 * prev + 0.5 * r; // attack
            } else {
                self.bar_heights[i] = 0.8 * prev + 0.2 * r; // release
            }
        }
    }

    /// 3-tier gradient: bottom green, mid yellow, top red.
    fn color_for_row(cy: u16, h: u16) -> Rgb {
        if h == 0 {
            return Rgb::BLACK;
        }
        // t=0 at top of grid, t=1 at bottom.
        let t = cy as f32 / (h - 1).max(1) as f32;
        // Invert: bar grows from bottom, so bottom rows (high t) are green,
        // top rows (low t) are red.
        let inv = 1.0 - t;
        if inv < 0.5 {
            // Bottom half: green to yellow.
            let local = inv / 0.5;
            let r = f32_to_u8(local);
            let g = 255;
            Rgb::new(r, g, 0)
        } else {
            // Top half: yellow to red.
            let local = (inv - 0.5) / 0.5;
            let r = 255;
            let g = f32_to_u8(1.0 - local);
            Rgb::new(r, g, 0)
        }
    }
}

impl Default for BarsDot {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for BarsDot {
    fn id(&self) -> VisualiserId {
        VisualiserId::BarsDot
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.energy.update(fft);
        self.frame = self.frame.wrapping_add(1);

        let grid: &mut CellGrid = ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        self.ensure_buf(w, h);
        self.braille.clear();

        let num_bands = 64.min(w as usize);
        if num_bands == 0 {
            return;
        }

        self.raw_bands = self.bands_from_fft(fft, num_bands);
        self.scaler.update(&self.raw_bands);
        let normalised: Vec<f32> = self
            .raw_bands
            .iter()
            .map(|&m| self.scaler.normalise(m))
            .collect();
        self.smooth_bars(&normalised);

        let bw = self.braille.width() as usize;
        let bh = self.braille.height() as usize;

        for (band, &height) in self.bar_heights.iter().enumerate() {
            let col_start = band * bw / num_bands;
            let col_end = (band + 1) * bw / num_bands;
            let fill_h = (height * bh as f32).round().min(bh as f32) as usize;
            if fill_h == 0 || col_start >= col_end {
                continue;
            }
            for x in col_start..col_end {
                for dy in 0..fill_h {
                    let y = bh - 1 - dy;
                    self.braille.set(x as u16, y as u16, true);
                }
            }
        }

        // Subtle warm gutter so empty air above the bars reads as a muted
        // backdrop rather than a dead pane.
        let gutter = Rgb::new(4, 2, 0);
        let grid_h = h;
        self.braille.compose(grid, |_cx, cy, dot_count| {
            if dot_count > 0 {
                let fg = Self::color_for_row(cy, grid_h);
                (fg, gutter)
            } else {
                (gutter, gutter)
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_fft() -> FftSnapshot {
        FftSnapshot::new(vec![5000.0; 128], 48_000, 256)
    }

    #[test]
    fn render_paints_cells() {
        let mut viz = BarsDot::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(80, 24);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &fft);
        }
        let non_blank = grid
            .cells()
            .iter()
            .filter(|c| c.ch != '\u{2800}' && c.ch != ' ')
            .count();
        assert!(
            non_blank > 0,
            "loud FFT should produce non-blank braille cells, got {non_blank}"
        );
    }

    #[test]
    fn output_changes_between_frames() {
        let mut viz = BarsDot::new();
        let fft = loud_fft();

        let mut grid_a = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            viz.render_tui(&mut ctx, &fft);
        }

        // Feed a different FFT so smoothing changes output.
        let fft_b = FftSnapshot::new(vec![100.0; 128], 48_000, 256);
        let mut grid_b = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            viz.render_tui(&mut ctx, &fft_b);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch || a.fg != b.fg)
            .count();
        assert!(
            diff > 0,
            "consecutive frames with different input should differ"
        );
    }

    #[test]
    fn gutter_is_tinted_not_black() {
        // Even when the bars are short, the air above them must carry a
        // palette-consistent tint rather than raw black.
        let mut viz = BarsDot::new();
        let fft = FftSnapshot::new(vec![0.0; 128], 48_000, 256);
        let mut grid = CellGrid::new(120, 40);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &fft);
        }

        let edge_rows = [0u16, 39];
        let edge_cols = [0u16, 119];

        for row in edge_rows {
            let any_tinted = (0..120u16).any(|x| {
                let cell = grid.cells()[(row as usize) * 120 + x as usize];
                cell.bg != Rgb::BLACK || cell.fg != Rgb::BLACK
            });
            assert!(any_tinted, "row {row} must have palette-tinted cells");
        }
        for col in edge_cols {
            let any_tinted = (0..40u16).any(|y| {
                let cell = grid.cells()[(y as usize) * 120 + col as usize];
                cell.bg != Rgb::BLACK || cell.fg != Rgb::BLACK
            });
            assert!(any_tinted, "col {col} must have palette-tinted cells");
        }
    }

    /// After the AGC envelope has converged on a typical-listening input
    /// (peak FFT bin magnitude ≈ 6.4, modeling peak_sample ≈ 0.05 at
    /// fft_size=256), the loudest bar must fill the majority of the pane.
    /// This is the whole point of the scaling fix — pre-fix, the bar would
    /// sit at ≈1% of the pane height.
    #[test]
    fn typical_listening_fills_pane() {
        let mut mags = vec![0.5_f32; 128];
        mags[3] = 6.4;
        mags[4] = 5.0;
        mags[5] = 3.0;
        let fft = FftSnapshot::new(mags, 48_000, 256);
        let mut viz = BarsDot::new();
        let mut grid = CellGrid::new(120, 40);
        for _ in 0..240 {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &fft);
        }
        let tall = viz.bar_heights.iter().cloned().fold(0.0_f32, f32::max);
        assert!(
            tall >= 0.6,
            "loudest bar should fill ≥60% at typical listening volume, got {tall}"
        );
    }

    #[test]
    fn resize_no_panic() {
        let mut viz = BarsDot::new();
        let fft = loud_fft();
        for (w, h) in [(80, 24), (40, 12), (1, 1), (200, 50)] {
            let mut grid = CellGrid::new(w, h);
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &fft);
        }
    }
}
