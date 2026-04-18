//! ClassicPeak — Winamp-style spectrum with fractional block bars and falling peak caps.

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_classic};
use crate::visualiser::scaling::SpectrumScaler;
use crate::visualiser::{SurfaceKind, TuiContext, Visualiser, VisualiserId};

const GRAVITY: f32 = 0.003;
const BLOCK_CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const PEAK_CHAR: char = '▔';
const BG: Rgb = Rgb { r: 4, g: 4, b: 8 };
const PEAK_COLOR: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};

pub struct ClassicPeak {
    energy: EnergyTracker,
    scaler: SpectrumScaler,
    bar_heights: Vec<f32>,
    peak_heights: Vec<f32>,
    peak_velocities: Vec<f32>,
    last_band_count: usize,
}

impl ClassicPeak {
    pub fn new() -> Self {
        Self {
            // Release tau ~115 ms so the classic peak-cap accent tracks
            // song-level dynamics instead of trailing by a quarter second.
            energy: EnergyTracker::new(0.5, 0.75, 500.0),
            scaler: SpectrumScaler::new(),
            bar_heights: Vec::new(),
            peak_heights: Vec::new(),
            peak_velocities: Vec::new(),
            last_band_count: 0,
        }
    }

    fn ensure_bands(&mut self, count: usize) {
        if count != self.last_band_count {
            self.bar_heights = vec![0.0; count];
            self.peak_heights = vec![0.0; count];
            self.peak_velocities = vec![0.0; count];
            self.last_band_count = count;
        }
    }

    fn bin_fft(&mut self, fft: &FftSnapshot, band_count: usize) -> Vec<f32> {
        let bin_count = fft.magnitudes.len().max(1);
        let max_log = ((bin_count - 1) as f32).ln().max(1.0);
        let mut raw = vec![0.0_f32; band_count];
        for (band, slot) in raw.iter_mut().enumerate() {
            let lo_log = (band as f32 / band_count as f32) * max_log;
            let hi_log = ((band + 1) as f32 / band_count as f32) * max_log;
            let lo = (lo_log.exp().round() as usize).min(bin_count - 1);
            let hi = (hi_log.exp().round() as usize).clamp(lo + 1, bin_count);
            *slot = fft.magnitudes[lo..hi]
                .iter()
                .cloned()
                .fold(0.0_f32, f32::max);
        }
        self.scaler.update(&raw);
        raw.iter().map(|&m| self.scaler.normalise(m)).collect()
    }

    fn smooth_and_update_peaks(&mut self, raw: &[f32]) {
        for (i, &new) in raw.iter().enumerate() {
            let old = self.bar_heights[i];
            self.bar_heights[i] = if new > old {
                0.4 * old + 0.6 * new
            } else {
                0.75 * old + 0.25 * new
            };

            let bar_h = self.bar_heights[i];
            if bar_h >= self.peak_heights[i] {
                self.peak_heights[i] = bar_h;
                self.peak_velocities[i] = 0.0;
            } else {
                self.peak_heights[i] -= self.peak_velocities[i];
                self.peak_velocities[i] += GRAVITY;
            }
            self.peak_heights[i] = self.peak_heights[i].clamp(0.0, 1.0);
        }
    }
}

impl Default for ClassicPeak {
    fn default() -> Self {
        Self::new()
    }
}

/// Row colour based on height fraction (0 = bottom, 1 = top).
fn bar_color(frac: f32) -> Rgb {
    let (h, s, v) = if frac < 1.0 / 3.0 {
        (0.33, 0.9, 0.8)
    } else if frac < 2.0 / 3.0 {
        (0.16, 0.9, 0.9)
    } else {
        (0.0, 0.9, 1.0)
    };
    let (r, g, b) = hsv_classic(h, s, v);
    Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b))
}

impl Visualiser for ClassicPeak {
    fn id(&self) -> VisualiserId {
        VisualiserId::ClassicPeak
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.energy.update(fft);

        let grid: &mut CellGrid = ctx.grid;
        let w = grid.width() as usize;
        let h = grid.height() as usize;
        if w == 0 || h == 0 {
            return;
        }

        // Decide band count and bar layout.
        let band_count = (w / 2).clamp(1, 64);
        let cell_per_bar = if band_count * 3 <= w { 3 } else { 2 };
        let bar_width = cell_per_bar - 1;
        let total = band_count * cell_per_bar;
        let x_offset = if total < w { (w - total) / 2 } else { 0 };

        self.ensure_bands(band_count);

        let raw = self.bin_fft(fft, band_count);
        self.smooth_and_update_peaks(&raw);

        // Fill background.
        grid.fill(Cell {
            ch: ' ',
            fg: BG,
            bg: BG,
        });

        let h_f = h as f32;

        for band in 0..band_count {
            let bar_h = self.bar_heights[band];
            let peak_h = self.peak_heights[band];

            // bar_top is in virtual rows from bottom (0.0 .. h)
            let bar_top = bar_h * h_f;
            let peak_row_from_bottom = peak_h * h_f;

            let base_x = x_offset + band * cell_per_bar;

            for col in 0..bar_width {
                let cx = base_x + col;
                if cx >= w {
                    break;
                }

                for row in 0..h {
                    // row 0 is top of screen, row h-1 is bottom.
                    // rows_from_bottom = h - 1 - row
                    let rows_from_bottom = (h - 1 - row) as f32;

                    // Determine what to draw in this cell.
                    let peak_row_int = peak_row_from_bottom as usize;
                    let is_peak_row = (h - 1 - row) == peak_row_int && peak_h > 0.001;

                    if rows_from_bottom + 1.0 <= bar_top {
                        // Fully filled cell.
                        let frac = (rows_from_bottom + 0.5) / h_f;
                        let color = bar_color(frac);
                        grid.set(
                            cx as u16,
                            row as u16,
                            Cell {
                                ch: '█',
                                fg: color,
                                bg: BG,
                            },
                        );
                    } else if rows_from_bottom < bar_top {
                        // Fractional top cell.
                        let frac_part = bar_top - rows_from_bottom;
                        let block_idx = ((frac_part * 8.0) as usize).min(7);
                        let frac = (rows_from_bottom + 0.5) / h_f;
                        let color = bar_color(frac);
                        grid.set(
                            cx as u16,
                            row as u16,
                            Cell {
                                ch: BLOCK_CHARS[block_idx],
                                fg: color,
                                bg: BG,
                            },
                        );
                    } else if is_peak_row {
                        // Peak indicator.
                        grid.set(
                            cx as u16,
                            row as u16,
                            Cell {
                                ch: PEAK_CHAR,
                                fg: PEAK_COLOR,
                                bg: BG,
                            },
                        );
                    }
                    // else: already filled with background
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_fft() -> FftSnapshot {
        FftSnapshot::new(vec![5000.0; 128], 48_000, 256)
    }

    fn silent_fft() -> FftSnapshot {
        FftSnapshot::new(vec![0.0; 128], 48_000, 256)
    }

    #[test]
    fn render_paints_cells() {
        let mut viz = ClassicPeak::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(80, 24);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            // Pump several frames so smoothing catches up.
            for _ in 0..10 {
                viz.render_tui(&mut ctx, &fft);
            }
        }
        let interesting = grid
            .cells()
            .iter()
            .filter(|c| c.ch != ' ' && (c.fg != BG || c.bg != BG))
            .count();
        assert!(
            interesting > 0,
            "loud FFT should paint non-background cells, got {interesting}"
        );
    }

    #[test]
    fn output_changes_between_frames() {
        let mut viz = ClassicPeak::new();
        let fft_a = loud_fft();
        let fft_b = FftSnapshot::new(vec![200.0; 128], 48_000, 256);

        let mut grid_a = CellGrid::new(60, 20);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            for _ in 0..10 {
                viz.render_tui(&mut ctx, &fft_a);
            }
        }
        let snap_a = grid_a.snapshot();

        let mut grid_b = CellGrid::new(60, 20);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            for _ in 0..10 {
                viz.render_tui(&mut ctx, &fft_b);
            }
        }

        let diff = snap_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch || a.fg != b.fg || a.bg != b.bg)
            .count();
        assert!(diff > 0, "different inputs should produce different output");
    }

    #[test]
    fn resize_no_panic() {
        let mut viz = ClassicPeak::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(80, 24);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &fft);
        }
        let mut grid2 = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid2 };
            viz.render_tui(&mut ctx, &fft);
        }
    }

    /// Typical-listening input (peak FFT bin magnitude ≈ 6.4) should
    /// push bar_heights above 60% once the AGC envelope converges.
    #[test]
    fn typical_listening_fills_pane() {
        let mut mags = vec![0.5_f32; 128];
        mags[3] = 6.4;
        mags[4] = 5.0;
        let fft = FftSnapshot::new(mags, 48_000, 256);
        let mut viz = ClassicPeak::new();
        let mut grid = CellGrid::new(120, 40);
        for _ in 0..240 {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &fft);
        }
        let tallest = viz.bar_heights.iter().cloned().fold(0.0_f32, f32::max);
        assert!(
            tallest >= 0.6,
            "classic_peak bar should reach ≥60% at typical listening volume, got {tallest}"
        );
    }

    #[test]
    fn peaks_fall_after_silence() {
        let mut viz = ClassicPeak::new();
        let loud = loud_fft();
        let silent = silent_fft();

        let mut grid = CellGrid::new(80, 24);
        for _ in 0..20 {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &loud);
        }
        let peaks_after_loud: Vec<f32> = viz.peak_heights.clone();

        for _ in 0..30 {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &silent);
        }

        let any_fell = viz
            .peak_heights
            .iter()
            .zip(peaks_after_loud.iter())
            .any(|(now, before)| *now < *before);
        assert!(any_fell, "peaks should fall after silence");
    }

    #[test]
    fn peak_tracks_rising_bar() {
        let mut viz = ClassicPeak::new();
        let loud = loud_fft();
        let mut grid = CellGrid::new(80, 24);
        for _ in 0..20 {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &loud);
        }
        for (i, (&peak, &bar)) in viz
            .peak_heights
            .iter()
            .zip(viz.bar_heights.iter())
            .enumerate()
        {
            assert!(
                peak >= bar - 1e-6,
                "band {i}: peak ({peak}) should be >= bar ({bar})"
            );
        }
    }
}
