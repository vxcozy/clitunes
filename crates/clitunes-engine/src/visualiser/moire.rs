//! Moire — M.C. Escher-inspired interference pattern visualiser. Three sets
//! of concentric rings orbit slowly around the screen centre at mutually
//! irrational angular velocities. A fourth layer, a rotating line grid,
//! interferes with the circles to produce classic moire beat patterns —
//! massive low-frequency spatial oscillations that look impossible and
//! hypnotic, like an Escher tessellation in motion.
//!
//! The moire effect emerges purely from superposition: each layer is a
//! simple `sin(dist * freq)`, but when frequencies and centres are close
//! enough the waves phase-cancel and reinforce at macroscopic scales.
//!
//! FFT coupling: audio energy gently modulates ring frequency (the pattern
//! "breathes" wider/tighter) and accelerates the orbital rotation of the
//! three centres, so loud passages warp the interference zones.

use std::f32::consts::PI;
use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::density_ramp::DensityRamp;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

/// Base ring spatial frequency. Controls how tight the concentric rings are.
const RING_FREQ: f32 = 0.8;

/// Line-grid spatial frequency. Slightly different from `RING_FREQ` so the
/// circle-vs-line interference produces visible beat bands.
const LINE_FREQ: f32 = 0.6;

/// Angular velocities for the three orbiting centres (rad/s). Mutually
/// irrational ratios ensure the pattern never exactly repeats.
const CENTER_SPEEDS: [f32; 3] = [0.13, 0.21, 0.34];

/// Phase-speed offsets for the three ring waves, so each set scrolls at a
/// different temporal rate.
const PHASE_SPEEDS: [f32; 3] = [0.7, -0.5, 0.9];

/// Orbit radius as a fraction of the screen diagonal.
const ORBIT_RADIUS: f32 = 0.30;

/// Rotation speed of the line-grid layer (rad/s).
const LINE_ROTATION_SPEED: f32 = 0.09;

pub struct Moire {
    start: Instant,
    ramp: DensityRamp,
    /// Smoothed audio energy; modulates ring freq and orbital speed.
    energy: f32,
}

impl Moire {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            ramp: DensityRamp::detailed(),
            energy: 0.0,
        }
    }

    fn update_energy(&mut self, fft: &FftSnapshot) {
        let sum: f32 = fft.magnitudes.iter().sum();
        let norm = (sum / fft.magnitudes.len().max(1) as f32 / 500.0).min(1.0);
        // Attack-release smoothing.
        if norm > self.energy {
            self.energy = 0.5 * self.energy + 0.5 * norm;
        } else {
            self.energy = 0.9 * self.energy + 0.1 * norm;
        }
    }
}

impl Default for Moire {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Moire {
    fn id(&self) -> VisualiserId {
        VisualiserId::Moire
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.update_energy(fft);

        let t = self.start.elapsed().as_secs_f32();
        let grid: &mut CellGrid = &mut *ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        const ASPECT: f32 = 2.0;
        let w_f = w as f32;
        let h_f = h as f32 * ASPECT;
        let cx = w_f * 0.5;
        let cy = h_f * 0.5;
        let diag = (w_f * w_f + h_f * h_f).sqrt();
        let orbit_r = diag * ORBIT_RADIUS;

        // Audio-driven modulation: energy makes the rings breathe and the
        // centres orbit faster.
        let freq_mod = RING_FREQ + 0.15 * self.energy;
        let speed_mul = 1.0 + 0.6 * self.energy;

        // Compute the three orbiting centres.
        let mut centres = [(0.0f32, 0.0f32); 3];
        for i in 0..3 {
            let angle = t * CENTER_SPEEDS[i] * speed_mul
                + (i as f32) * 2.0 * PI / 3.0;
            centres[i] = (
                cx + orbit_r * angle.cos(),
                cy + orbit_r * angle.sin(),
            );
        }

        // Line-grid rotation angle.
        let line_angle = t * LINE_ROTATION_SPEED * speed_mul;
        let line_cos = line_angle.cos();
        let line_sin = line_angle.sin();

        for y in 0..h {
            let yf = y as f32 * ASPECT;
            for x in 0..w {
                let xf = x as f32;

                // Layer 1-3: concentric ring waves from orbiting centres.
                let mut ring_sum = 0.0f32;
                for i in 0..3 {
                    let dx = xf - centres[i].0;
                    let dy = yf - centres[i].1;
                    let dist = (dx * dx + dy * dy).sqrt();
                    let wave = (dist * freq_mod + t * PHASE_SPEEDS[i]).sin();
                    ring_sum += wave;
                }
                ring_sum /= 3.0;

                // Layer 4: rotating line grid. The interference between
                // parallel lines and concentric circles is the signature
                // moire effect.
                let line_wave = (
                    (xf * LINE_FREQ * line_cos + yf * LINE_FREQ * line_sin)
                        + t * 0.3
                ).sin();

                // Combined field: ring interference + line interference.
                // The 0.7/0.3 blend keeps rings dominant while the line
                // grid adds the classic stripy moire fringes.
                let field = ring_sum * 0.7 + line_wave * 0.3;

                // field is in [-1, 1]; map to [0, 1].
                let intensity = ((field + 1.0) * 0.5).clamp(0.0, 1.0);

                // Glyph from the 70-level detailed ramp.
                let glyph_intensity = lerp(0.05, 1.0, intensity);
                let ch = self.ramp.pick(glyph_intensity);

                // Cool blue-violet-cyan palette. Hue range 0.55-0.75,
                // modulated by the field value and a slow drift.
                let hue = lerp(0.55, 0.75, intensity) + (t * 0.03).fract();
                let hue = hue.fract();
                let sat = lerp(0.55, 0.95, intensity);
                let val = lerp(0.30, 1.0, intensity);
                let (r, g, b) = hsv_to_rgb(hue, sat, val);
                let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

                // Background: deep indigo. Slightly brighter where the
                // field is hot so the interference fringes glow.
                let bg_hue = 0.68;
                let bg_val = 0.02 + 0.04 * intensity;
                let (br, bg_g, bb) = hsv_to_rgb(bg_hue, 0.9, bg_val);
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
        let mut m = Moire::new();
        let fft = FftSnapshot {
            magnitudes: vec![100.0; 128],
            sample_rate: 48_000,
            fft_size: 256,
        };
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            m.render_tui(&mut ctx, &fft);
        }
        let non_black = grid
            .cells()
            .iter()
            .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
            .count();
        assert_eq!(non_black, 40 * 12, "all cells should be painted");
    }

    #[test]
    fn energy_smoothing_responds_to_input() {
        let mut m = Moire::new();
        assert_eq!(m.energy, 0.0);
        let loud = FftSnapshot {
            magnitudes: vec![5000.0; 64],
            sample_rate: 48_000,
            fft_size: 128,
        };
        for _ in 0..20 {
            m.update_energy(&loud);
        }
        assert!(m.energy > 0.5, "energy should ramp up with loud input");
    }

    #[test]
    fn zero_size_grid_does_not_panic() {
        let mut m = Moire::new();
        let fft = FftSnapshot {
            magnitudes: vec![0.0; 64],
            sample_rate: 48_000,
            fft_size: 128,
        };
        let mut grid = CellGrid::new(0, 0);
        let mut ctx = TuiContext { grid: &mut grid };
        m.render_tui(&mut ctx, &fft);
        // No assertion — just verifying no panic.
    }
}
