//! Auralis — maximalist instantaneous spectrum visualiser, ASCII edition.
//!
//! Renders a 64-bar log-scale spectrum into a [`CellGrid`] using upper-half
//! blocks (`▀`) for 2× vertical resolution. Warm-to-cool hue gradient per
//! bar, soft radial vignette background, top glow line per bar, and a slow
//! hue wash on the background. Pure CPU colour math — no GPU context, no
//! readback, no Kitty payload.
//!
//! The slice-1 pipeline is: `FftSnapshot → bars_from_fft → sample_pixel per
//! virtual pixel → CellGrid`. The ANSI writer walks the grid and emits
//! truecolor SGR. Typical cost on a 200×60 terminal at 30 fps is well
//! under 5% CPU on a modern laptop.

use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{SurfaceKind, TuiContext, Visualiser, VisualiserId};

const NUM_BARS: usize = 64;
const BAR_GAP: f32 = 0.15;

pub struct Auralis {
    start: Instant,
    bar_smoothing: [f32; NUM_BARS],
}

impl Auralis {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            bar_smoothing: [0.0; NUM_BARS],
        }
    }

    fn bars_from_fft(&mut self, fft: &FftSnapshot) -> [f32; NUM_BARS] {
        // Log-scale bucket the positive-frequency bins into NUM_BARS groups.
        let bin_count = fft.magnitudes.len().max(1);
        let max_log = ((bin_count - 1) as f32).ln().max(1.0);
        let mut out = [0.0; NUM_BARS];
        for (bar, slot) in out.iter_mut().enumerate() {
            let lo_log = (bar as f32 / NUM_BARS as f32) * max_log;
            let hi_log = ((bar + 1) as f32 / NUM_BARS as f32) * max_log;
            let lo = (lo_log.exp().round() as usize).min(bin_count - 1);
            let hi = (hi_log.exp().round() as usize).clamp(lo + 1, bin_count);
            let slice = &fft.magnitudes[lo..hi];
            let max_mag = slice.iter().cloned().fold(0.0_f32, f32::max);
            // Log-compress + normalise. The /1000 denominator is a slice-1
            // hack; Unit 19 adds proper AGC and per-bar headroom.
            let compressed = (1.0 + max_mag / 1000.0).ln();
            *slot = compressed.min(1.0);
        }
        // Attack-release smoothing so bars don't flicker on noise.
        for (i, slot) in self.bar_smoothing.iter_mut().enumerate() {
            if out[i] > *slot {
                *slot = 0.6 * *slot + 0.4 * out[i]; // attack
            } else {
                *slot = 0.85 * *slot + 0.15 * out[i]; // release
            }
            out[i] = *slot;
        }
        out
    }
}

impl Default for Auralis {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Auralis {
    fn id(&self) -> VisualiserId {
        VisualiserId::Auralis
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        let bars = self.bars_from_fft(fft);
        let t = self.start.elapsed().as_secs_f32();
        let grid: &mut CellGrid = &mut *ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }
        let cells_w_f = w as f32;
        // Each cell is two virtual pixels (top=fg, bottom=bg). `virt_h` is
        // the total virtual-pixel height of the frame.
        let virt_h = (h as usize) * 2;
        let virt_h_f = virt_h as f32;
        for y in 0..h {
            for x in 0..w {
                let uu = (x as f32 + 0.5) / cells_w_f;
                // Top virtual pixel of this cell lives higher on screen
                // (smaller vy), so it maps to a larger `vv` (0 at bottom,
                // 1 at top).
                let vy_top = (y as f32) * 2.0 + 0.5;
                let vy_bot = (y as f32) * 2.0 + 1.5;
                let vv_top = 1.0 - vy_top / virt_h_f;
                let vv_bot = 1.0 - vy_bot / virt_h_f;
                let fg = sample_pixel(&bars, t, uu, vv_top);
                let bg = sample_pixel(&bars, t, uu, vv_bot);
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

/// Sample one virtual pixel. `uu` is the horizontal position in [0, 1]
/// across the visualiser; `vv` is the vertical position in [0, 1] with
/// v=0 at the bottom and v=1 at the top.
fn sample_pixel(bars: &[f32; NUM_BARS], t: f32, uu: f32, vv: f32) -> Rgb {
    let n = NUM_BARS as f32;
    let bar_pos = uu * n;
    let bar_idx = (bar_pos.floor() as usize).min(NUM_BARS - 1);
    let bar_u = bar_pos - bar_pos.floor();
    let in_bar_col = bar_u > BAR_GAP * 0.5 && bar_u < 1.0 - BAR_GAP * 0.5;

    let h = bars[bar_idx];
    let bar_top = 0.08 + h * 0.85;

    // Background: soft radial vignette + subtle hue wash.
    let cx = uu - 0.5;
    let cy = vv - 0.5;
    let r = (cx * cx + cy * cy).sqrt();
    let bg_hue = 0.55 + 0.05 * (t * 0.25).sin();
    let (br, bg_, bb) = hsv_to_rgb(bg_hue, 0.6, 0.06);
    let vignette = (1.0 - r * 0.9).max(0.0);
    let mut col_r = br * vignette;
    let mut col_g = bg_ * vignette;
    let mut col_b = bb * vignette;

    // Bar fill: warm bottom → cool top, saturation and value keyed to energy.
    if in_bar_col && vv <= bar_top {
        let local_y = vv / bar_top.max(1e-3);
        let hue = lerp(0.06, 0.62, local_y);
        let sat = 0.55 + 0.45 * h;
        let val = lerp(0.45, 1.15, h);
        let (fr, fg_, fb) = hsv_to_rgb(hue, sat, val);
        col_r = fr;
        col_g = fg_;
        col_b = fb;
    }

    // Top glow: bright rim on the leading edge of each bar, scaled by energy.
    if in_bar_col {
        let dist = (bar_top - vv).abs();
        if dist < 0.02 {
            let strength = (1.0 - dist / 0.02) * h * 1.2;
            col_r += 1.00 * strength;
            col_g += 0.95 * strength;
            col_b += 0.85 * strength;
        }
    }

    Rgb::new(f32_to_u8(col_r), f32_to_u8(col_g), f32_to_u8(col_b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_paints_whole_grid() {
        let mut auralis = Auralis::new();
        let fft = FftSnapshot {
            magnitudes: vec![500.0; 128],
            sample_rate: 48_000,
            fft_size: 256,
        };
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            auralis.render_tui(&mut ctx, &fft);
        }
        // Every cell should have the upper-block glyph.
        for c in grid.cells() {
            assert_eq!(c.ch, Cell::UPPER_BLOCK);
        }
        // At least some cells should be non-black (bars + vignette).
        let non_black = grid
            .cells()
            .iter()
            .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
            .count();
        assert!(non_black > 0, "expected at least some painted cells");
    }
}
