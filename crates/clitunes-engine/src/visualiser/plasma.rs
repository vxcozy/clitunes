//! Plasma — the demoscene classic. Four layered sine fields interfere to
//! make a shimmering, endlessly-morphing intensity field; the field is
//! mapped through a density ramp for per-cell glyph weight and through a
//! rotating HSV palette for hue. No audio input required — it looks alive
//! before you press play.
//!
//! Cell aspect: terminal cells are roughly 2× taller than wide, so we
//! multiply the y coordinate by 2 inside all distance/trig functions.
//! Otherwise the round "plasma cells" come out visually squashed.
//!
//! FFT coupling: we take the total energy of the snapshot and use it to
//! gently accelerate the time-varying phases, so the plasma breathes with
//! the music when there is any, and idles gracefully when there isn't.

use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::density_ramp::DensityRamp;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

/// Horizontal wavelength of the x-aligned sine, in cells. Smaller = more bands.
const WAVELENGTH_X: f32 = 8.0;
/// Vertical wavelength of the y-aligned sine, in virtual pixels.
const WAVELENGTH_Y: f32 = 8.0;
/// Wavelength of the diagonal sine.
const WAVELENGTH_DIAG: f32 = 16.0;
/// Wavelength of the radial ripple.
const WAVELENGTH_RADIAL: f32 = 10.0;

pub struct Plasma {
    start: Instant,
    ramp: DensityRamp,
    /// Smoothed audio energy used to modulate plasma speed.
    energy: EnergyTracker,
}

impl Plasma {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            ramp: DensityRamp::detailed(),
            energy: EnergyTracker::new(0.6, 0.9, 500.0),
        }
    }
}

impl Default for Plasma {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Plasma {
    fn id(&self) -> VisualiserId {
        VisualiserId::Plasma
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.energy.update(fft);

        // Base time + an energy-modulated acceleration. Idle plasma drifts
        // at real time; a loud passage speeds it up to ~1.6× for a beat.
        let t = self.start.elapsed().as_secs_f32() * (1.0 + 0.6 * self.energy.energy());
        let grid: &mut CellGrid = &mut *ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        // Cell aspect compensation: 1 row ≈ 2 columns in screen pixels.
        const ASPECT: f32 = 2.0;
        let w_f = w as f32;
        let h_f = h as f32 * ASPECT;

        // Roving radial source — gives the plasma a wandering hotspot.
        let cx = w_f * 0.5 + (t * 0.37).sin() * w_f * 0.3;
        let cy = h_f * 0.5 + (t * 0.53).cos() * h_f * 0.3;

        for y in 0..h {
            let yf = y as f32 * ASPECT;
            for x in 0..w {
                let xf = x as f32;

                let v = (xf / WAVELENGTH_X + t).sin()
                    + (yf / WAVELENGTH_Y + t * 1.3).sin()
                    + ((xf + yf) / WAVELENGTH_DIAG + t * 0.7).sin()
                    + {
                        let dx = xf - cx;
                        let dy = yf - cy;
                        let d = (dx * dx + dy * dy).sqrt();
                        (d / WAVELENGTH_RADIAL - t * 1.1).sin()
                    };
                // v ∈ [-4, 4] → [0, 1]
                let intensity = ((v + 4.0) / 8.0).clamp(0.0, 1.0);

                // Glyph: density ramp keyed on intensity, with a gentle
                // bias so the sparse end isn't *completely* empty.
                let glyph_intensity = lerp(0.08, 1.0, intensity);
                let ch = self.ramp.pick(glyph_intensity);

                // Hue: intensity plus a slow global rotation. This is
                // what gives plasma its classic rainbow shimmer.
                let hue = (intensity * 0.7 + t * 0.08).fract();
                let sat = 0.75 + 0.25 * intensity;
                let val = lerp(0.45, 1.0, intensity);
                let (r, g, b) = hsv_to_rgb(hue, sat, val);
                let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

                // Background: complementary hue at very low value so the
                // glyph always has contrast without the screen looking flat.
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
        let mut plasma = Plasma::new();
        let fft = FftSnapshot::new(vec![100.0; 128], 48_000, 256);
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            plasma.render_tui(&mut ctx, &fft);
        }
        // All cells should be painted with some glyph (not the default empty
        // black space).
        let non_empty = grid
            .cells()
            .iter()
            .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
            .count();
        assert_eq!(non_empty, grid.cells().len());
    }

    #[test]
    fn energy_smoothing_responds_to_input() {
        let mut plasma = Plasma::new();
        assert_eq!(plasma.energy.energy(), 0.0);
        let loud = FftSnapshot::new(vec![5000.0; 64], 48_000, 128);
        for _ in 0..20 {
            plasma.energy.update(&loud);
        }
        assert!(plasma.energy.energy() > 0.5);
    }
}
