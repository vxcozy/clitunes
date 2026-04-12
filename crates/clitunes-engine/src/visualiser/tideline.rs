//! Tideline — monochrome waveform visualiser.
//!
//! The opposite of Auralis: minimal, contemplative, fluid. A single morphing
//! waveform line breathes with the audio against a cool dark blue-grey
//! background. No bars, no glow, no particles — just the line and a soft
//! vignette.

use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};
use crate::visualiser::palette::{f32_to_u8, lerp};
use crate::visualiser::{SurfaceKind, TuiContext, Visualiser, VisualiserId};

const NUM_COLUMNS: usize = 200;

pub struct Tideline {
    start: Instant,
    column_heights: [f32; NUM_COLUMNS],
    prev_rms: f32,
}

impl Tideline {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            column_heights: [0.0; NUM_COLUMNS],
            prev_rms: 0.0,
        }
    }
}

impl Default for Tideline {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Tideline {
    fn id(&self) -> VisualiserId {
        VisualiserId::Tideline
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        let t = self.start.elapsed().as_secs_f32();
        let grid: &mut CellGrid = &mut *ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        let w_f = w as f32;
        let h_f = h as f32;
        let virt_h = h_f * 2.0;
        let bin_count = fft.magnitudes.len().max(1);

        // --- Per-column envelope from FFT ---
        for x in 0..(w as usize).min(NUM_COLUMNS) {
            let lo = x * bin_count / (w as usize).max(1);
            let hi = ((x + 1) * bin_count / (w as usize).max(1))
                .max(lo + 1)
                .min(bin_count);
            let slice = &fft.magnitudes[lo..hi];
            let avg_mag = slice.iter().sum::<f32>() / slice.len().max(1) as f32;
            let compressed = (1.0 + avg_mag / 500.0).ln().min(1.0);

            let old = self.column_heights[x];
            if compressed > old {
                self.column_heights[x] = 0.5 * old + 0.5 * compressed; // attack
            } else {
                self.column_heights[x] = 0.92 * old + 0.08 * compressed; // release
            }
        }

        // --- RMS for overall energy ---
        let sum_sq: f32 = fft.magnitudes.iter().map(|m| m * m).sum();
        let rms = (sum_sq / fft.magnitudes.len().max(1) as f32).sqrt() / 1000.0;
        self.prev_rms = 0.9 * self.prev_rms + 0.1 * rms;

        // --- Breathing offset ---
        let breath = (t * 0.4).sin() * 0.03;

        // --- Sharpness for line rendering ---
        let sharpness = 800.0;

        // --- Render each cell ---
        for y in 0..h {
            for x in 0..w {
                let x_idx = (x as usize).min(NUM_COLUMNS - 1);

                // Virtual pixel positions (0.0 at top, 1.0 at bottom).
                let vy_top = y as f32 * 2.0 + 0.5;
                let vy_bot = y as f32 * 2.0 + 1.5;
                let vv_top = vy_top / virt_h;
                let vv_bot = vy_bot / virt_h;

                // Waveform geometry.
                let center = 0.5 + breath;
                let disp = self.column_heights[x_idx] * 0.35 * (1.0 + self.prev_rms);
                let wave_top = center - disp;
                let wave_bot = center + disp;

                let fg = sample_pixel(
                    vv_top,
                    x as f32,
                    w_f,
                    wave_top,
                    wave_bot,
                    sharpness,
                    self.column_heights[x_idx],
                    self.prev_rms,
                );
                let bg = sample_pixel(
                    vv_bot,
                    x as f32,
                    w_f,
                    wave_top,
                    wave_bot,
                    sharpness,
                    self.column_heights[x_idx],
                    self.prev_rms,
                );

                grid.set(
                    x,
                    y,
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

/// Sample one virtual pixel at normalised vertical position `vv`.
fn sample_pixel(
    vv: f32,
    x: f32,
    w: f32,
    wave_top: f32,
    wave_bot: f32,
    sharpness: f32,
    col_height: f32,
    prev_rms: f32,
) -> Rgb {
    // Distance to nearest waveform edge.
    let dist_top = (vv - wave_top).abs();
    let dist_bot = (vv - wave_bot).abs();
    let dist = dist_top.min(dist_bot);

    // Line brightness: sharp Gaussian falloff from the edge.
    let line_bright = (-dist * dist * sharpness).exp();

    // Inner fill glow: if between the two edges, add a soft fill.
    let fill = if vv > wave_top && vv < wave_bot {
        let dist_to_edge = dist_top.min(dist_bot);
        (-dist_to_edge * 20.0).exp() * 0.15
    } else {
        0.0
    };

    // Background: dark blue-grey with vignette.
    let bg_r = 6.0 / 255.0;
    let bg_g = 8.0 / 255.0;
    let bg_b = 14.0 / 255.0;
    let cx = (x / w) - 0.5;
    let cy = vv - 0.5;
    let vignette = (1.0 - (cx * cx + cy * cy).sqrt() * 0.7).max(0.0);

    // Line colour: teal/cyan keyed to energy.
    let energy = col_height.max(prev_rms);
    let line_r = lerp(30.0, 100.0, energy) / 255.0;
    let line_g = lerp(70.0, 190.0, energy) / 255.0;
    let line_b = lerp(90.0, 210.0, energy) / 255.0;

    // Composite: lerp background toward line colour by brightness.
    let t = (line_bright + fill).min(1.0);
    let r = lerp(bg_r * vignette, line_r, t);
    let g = lerp(bg_g * vignette, line_g, t);
    let b = lerp(bg_b * vignette, line_b, t);

    Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_paints_whole_grid() {
        let mut vis = Tideline::new();
        let fft = FftSnapshot {
            magnitudes: vec![500.0; 128],
            sample_rate: 48_000,
            fft_size: 256,
        };
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }
        for c in grid.cells() {
            assert_eq!(c.ch, Cell::UPPER_BLOCK);
        }
    }

    #[test]
    fn silent_input_still_renders_background() {
        let mut vis = Tideline::new();
        let fft = FftSnapshot {
            magnitudes: vec![0.0; 128],
            sample_rate: 48_000,
            fft_size: 256,
        };
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }
        // Background is dark blue-grey, not pure black.
        let non_black = grid
            .cells()
            .iter()
            .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
            .count();
        assert!(
            non_black > 0,
            "silent input should still render dark background"
        );
    }

    #[test]
    fn non_silent_has_brighter_center() {
        let mut vis = Tideline::new();
        let fft = FftSnapshot {
            magnitudes: vec![2000.0; 128],
            sample_rate: 48_000,
            fft_size: 256,
        };
        let mut grid = CellGrid::new(40, 20);
        // Render a few frames so smoothing converges.
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }

        let h = grid.height() as usize;
        let w = grid.width() as usize;
        let center_row = h / 2;
        let edge_row = 0;

        // Sum brightness of cells in the center row.
        let brightness = |row: usize| -> u32 {
            let start = row * w;
            grid.cells()[start..start + w]
                .iter()
                .map(|c| {
                    (c.fg.r as u32 + c.fg.g as u32 + c.fg.b as u32)
                        + (c.bg.r as u32 + c.bg.g as u32 + c.bg.b as u32)
                })
                .sum::<u32>()
        };

        let center_brightness = brightness(center_row);
        let edge_brightness = brightness(edge_row);
        assert!(
            center_brightness > edge_brightness,
            "center row ({center_brightness}) should be brighter than edge row ({edge_brightness})"
        );
    }
}
