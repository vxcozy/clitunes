//! Vortex — logarithmic spiral visualiser inspired by M.C. Escher's
//! "Whirlpools" and infinite regression prints. Two spiral fields with
//! opposing twist directions interfere to produce impossible-looking
//! patterns that rotate inward under the `log(dist)` mapping, which
//! compresses the arms near the center for an Escher-esque infinite
//! regression effect.
//!
//! Cell aspect: terminal cells are roughly 2x taller than wide, so we
//! multiply the y coordinate by 2 inside all distance/trig functions.
//!
//! FFT coupling: audio energy accelerates rotation speed and tightens
//! the twist factor slightly, so the spiral breathes and contracts on
//! beat. A subtle radial pulsation tied to energy makes the center
//! bloom on loud passages.

use std::f32::consts::PI;
use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::density_ramp::DensityRamp;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

/// Number of arms in the primary spiral.
const NUM_ARMS_1: f32 = 5.0;
/// Number of arms in the secondary, counter-rotating spiral.
const NUM_ARMS_2: f32 = 3.0;
/// How tightly the primary arms wrap (higher = more turns near center).
const TWIST_1: f32 = 2.5;
/// Opposite twist direction for the secondary spiral.
const TWIST_2: f32 = -1.8;
/// Primary rotation speed (rad/s).
const ROT_SPEED_1: f32 = 0.4;
/// Secondary rotation speed (rad/s), counter-rotating.
const ROT_SPEED_2: f32 = -0.25;

pub struct Vortex {
    start: Instant,
    ramp: DensityRamp,
    /// Smoothed audio energy used to modulate spiral dynamics.
    energy: EnergyTracker,
}

impl Vortex {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            ramp: DensityRamp::detailed(),
            energy: EnergyTracker::new(0.5, 0.9, 500.0),
        }
    }
}

impl Default for Vortex {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Vortex {
    fn id(&self) -> VisualiserId {
        VisualiserId::Vortex
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.energy.update(fft);

        let grid: &mut CellGrid = &mut *ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        // Cell aspect compensation: 1 row ~ 2 columns in screen pixels.
        const ASPECT: f32 = 2.0;
        let w_f = w as f32;
        let h_f = h as f32 * ASPECT;
        let cx = w_f * 0.5;
        let cy = h_f * 0.5;
        // Normalise distances so the vortex scales with terminal size.
        let half_diag = (cx * cx + cy * cy).sqrt().max(1.0);

        let t = self.start.elapsed().as_secs_f32();
        let energy = self.energy.energy();

        // Energy-modulated twist: spirals tighten slightly on beat.
        let twist_1 = TWIST_1 + energy * 0.5;
        let twist_2 = TWIST_2 - energy * 0.3;

        // Energy-accelerated rotation.
        let rot_1 = t * (ROT_SPEED_1 + energy * 0.3);
        let rot_2 = t * (ROT_SPEED_2 - energy * 0.2);

        // Subtle radial pulsation tied to energy.
        let pulse = 1.0 + energy * 0.15;

        for y in 0..h {
            let yf = y as f32 * ASPECT - cy;
            for x in 0..w {
                let xf = x as f32 - cx;

                let dist = (xf * xf + yf * yf).sqrt() / half_diag;
                let angle = yf.atan2(xf);

                // Logarithmic spiral field: log(dist) causes arms to wrap
                // more tightly near the center -- Escher's infinite
                // regression effect. The +0.1 avoids log(0) at center.
                let log_dist = (dist + 0.1).ln();

                let spiral_1 = (NUM_ARMS_1 * (angle - log_dist * twist_1) + rot_1).sin();
                let spiral_2 = (NUM_ARMS_2 * (angle + log_dist * twist_2) + rot_2).sin();

                // Blend the two spirals; the interference creates
                // impossible-looking patterns.
                let field = (spiral_1 * 0.6 + spiral_2 * 0.4 + 1.0) / 2.0;

                // Radial brightness falloff: center is the bright eye
                // of the vortex, edges fade out. Pulse with energy.
                let center_brightness = (1.0 - dist * 0.7 * (1.0 / pulse)).clamp(0.2, 1.0);

                let intensity = (field * center_brightness).clamp(0.0, 1.0);

                // Glyph from density ramp, biased slightly so sparse
                // regions still have texture.
                let glyph_intensity = lerp(0.05, 1.0, intensity);
                let ch = self.ramp.pick(glyph_intensity);

                // Deep jewel-tone palette. Hue rotates with the spiral
                // angle for a rainbow-through-the-spiral effect, drifts
                // with distance and time for depth.
                let hue = (angle / (2.0 * PI) + dist * 0.15 + t * 0.06).fract();
                let sat = lerp(0.7, 0.95, intensity);
                let val = lerp(0.1, 1.0, intensity * center_brightness);
                let (r, g, b) = hsv_to_rgb(hue, sat, val);
                let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

                // Background: dark complementary hue at very low value
                // so glyphs always have contrast.
                let bg_hue = (hue + 0.5).fract();
                let (br, bg_g, bb) = hsv_to_rgb(bg_hue, 0.8, 0.04 + 0.04 * intensity);
                let bg = Rgb::new(f32_to_u8(br), f32_to_u8(bg_g), f32_to_u8(bb));

                grid.set(x, y, Cell { ch, fg, bg });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_paints_whole_grid() {
        let mut vortex = Vortex::new();
        let fft = FftSnapshot::new(vec![100.0; 128], 48_000, 256);
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            vortex.render_tui(&mut ctx, &fft);
        }
        // All cells should be painted with some colour (not default black).
        let non_empty = grid
            .cells()
            .iter()
            .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
            .count();
        assert_eq!(non_empty, grid.cells().len());
    }

    #[test]
    fn energy_smoothing_responds_to_input() {
        let mut vortex = Vortex::new();
        assert_eq!(vortex.energy.energy(), 0.0);
        let loud = FftSnapshot::new(vec![5000.0; 64], 48_000, 128);
        for _ in 0..20 {
            vortex.energy.update(&loud);
        }
        assert!(vortex.energy.energy() > 0.5);
    }
}
