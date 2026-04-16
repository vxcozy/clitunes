//! Fire — buffer-based cellular automaton fire effect. A heat buffer
//! sized to grid dimensions is maintained across frames. Each frame:
//!
//! 1. Heat is injected at the bottom row (base intensity + audio energy
//!    boost + turbulent noise).
//! 2. Each cell above the bottom averages the three cells below it
//!    (below-left, below, below-right) minus a cooling factor.
//! 3. Procedural turbulence from a cheap hash adds per-cell flicker.
//! 4. Heat is mapped to a glyph via a custom fire density ramp and to
//!    colour via a deep ember HSV palette.
//!
//! Cell aspect: terminal cells are roughly 2× taller than wide, so we
//! multiply the y coordinate by 2 inside all distance/trig functions.
//!
//! FFT coupling: energy boosts the heat injection at the bottom row and
//! increases the turbulence amplitude, so the fire roars with loud audio
//! and smoulders in silence.

use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::density_ramp::DensityRamp;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

/// Base heat injected at the bottom row even with no audio.
const BASE_HEAT: f32 = 0.55;
/// How much audio energy can boost bottom-row heat (additive).
const ENERGY_HEAT_BOOST: f32 = 0.45;
/// Cooling subtracted per propagation step. Higher = shorter flames.
const COOLING: f32 = 0.038;
/// Amplitude of the turbulence noise added to each cell per frame.
const TURBULENCE_BASE: f32 = 0.04;
/// Extra turbulence amplitude contributed by audio energy.
const TURBULENCE_ENERGY: f32 = 0.06;

pub struct Fire {
    start: Instant,
    ramp: DensityRamp,
    /// Smoothed audio energy used to modulate fire intensity.
    energy: EnergyTracker,
    /// Heat buffer, row-major, same dimensions as grid.
    heat: Vec<f32>,
    /// Last known grid width (to detect resize).
    last_w: u16,
    /// Last known grid height (to detect resize).
    last_h: u16,
}

impl Fire {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            ramp: DensityRamp::new(" .·:;+=xX#%@"),
            energy: EnergyTracker::new(0.5, 0.88, 500.0),
            heat: Vec::new(),
            last_w: 0,
            last_h: 0,
        }
    }

    /// Cheap integer hash for procedural turbulence. Takes a cell
    /// coordinate and a frame counter, returns a float in [0, 1].
    #[inline]
    fn hash_noise(x: u32, y: u32, frame: u32) -> f32 {
        let mut h = x
            .wrapping_mul(374761393)
            .wrapping_add(y.wrapping_mul(668265263))
            .wrapping_add(frame.wrapping_mul(1013904223));
        h = (h ^ (h >> 13)).wrapping_mul(1274126177);
        h ^= h >> 16;
        (h & 0x00FF_FFFF) as f32 / 0x00FF_FFFF as f32
    }

    /// Ensure the heat buffer matches the grid dimensions. If the grid
    /// resized, we clear the buffer so there are no stale artifacts.
    fn ensure_buffer(&mut self, w: u16, h: u16) {
        if w != self.last_w || h != self.last_h {
            let len = (w as usize) * (h as usize);
            self.heat.clear();
            self.heat.resize(len, 0.0);
            self.last_w = w;
            self.last_h = h;
        }
    }
}

impl Default for Fire {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Fire {
    fn id(&self) -> VisualiserId {
        VisualiserId::Fire
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

        self.ensure_buffer(w, h);

        let t = self.start.elapsed().as_secs_f32();
        // Frame counter for noise — ~30 fps granularity.
        let frame = (t * 30.0) as u32;

        let w_us = w as usize;
        let h_us = h as usize;

        // Cell aspect compensation: 1 row ≈ 2 columns in screen pixels.
        const ASPECT: f32 = 2.0;
        let _ = ASPECT; // used conceptually; fire propagation is vertical

        // --- Step 1: Inject heat at the bottom row ---
        let bottom = h_us - 1;
        let base = BASE_HEAT + ENERGY_HEAT_BOOST * self.energy.energy();
        for x in 0..w_us {
            let noise = Self::hash_noise(x as u32, bottom as u32, frame);
            // Vary injection across the bottom to create natural flicker.
            let inject = base + 0.3 * noise;
            let idx = bottom * w_us + x;
            self.heat[idx] = inject.clamp(0.0, 1.0);
        }

        // --- Step 2: Propagate heat upward (bottom-up, skip bottom row) ---
        // We iterate from row (h-2) up to row 0.  For each cell, average
        // the three cells below (below-left, below, below-right) minus
        // cooling.
        for y in (0..h_us - 1).rev() {
            let below = y + 1;
            for x in 0..w_us {
                let bl = if x > 0 {
                    self.heat[below * w_us + (x - 1)]
                } else {
                    self.heat[below * w_us + x]
                };
                let bc = self.heat[below * w_us + x];
                let br = if x + 1 < w_us {
                    self.heat[below * w_us + (x + 1)]
                } else {
                    self.heat[below * w_us + x]
                };
                let avg = (bl + bc + br) / 3.0;

                // --- Step 3: Procedural turbulence ---
                let turb_amp = TURBULENCE_BASE + TURBULENCE_ENERGY * self.energy.energy();
                let noise = Self::hash_noise(x as u32, y as u32, frame);
                // Centre noise around 0 so it flickers both ways.
                let turb = (noise - 0.5) * turb_amp;

                let new_heat = (avg - COOLING + turb).clamp(0.0, 1.0);
                self.heat[y * w_us + x] = new_heat;
            }
        }

        // --- Step 4: Map heat to glyph + colour and paint the grid ---
        for y in 0..h {
            for x in 0..w {
                let idx = (y as usize) * w_us + (x as usize);
                let heat = self.heat[idx];

                // Glyph: density ramp keyed on heat.
                let ch = self.ramp.pick(heat);

                // --- Deep ember colour palette ---
                // Heat 0.0 = black/dark red
                // Heat 0.3 = dark red
                // Heat 0.6 = orange
                // Heat 0.8 = yellow
                // Heat 1.0 = white-hot
                //
                // hsv_to_rgb has a shifted hue wheel: h ≈ 1/6 is red,
                // h ≈ 1/4 is yellow-orange. We map the user-facing
                // palette [red..yellow-orange] into the engine's hue
                // space accordingly.
                let hue = lerp(1.0 / 6.0, 1.0 / 6.0 + 0.12, heat);
                // Saturation decreases at high heat (white-hot is desaturated).
                let sat = lerp(1.0, 0.15, heat * heat);
                // Value tracks heat, with a floor so even cool cells aren't
                // invisible (they should glow a faint dark red).
                let val = lerp(0.08, 1.0, heat);
                let (r, g, b) = hsv_to_rgb(hue, sat, val);
                let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

                // Background: very dark ember so the glyph has contrast.
                let bg_val = lerp(0.0, 0.12, heat);
                let (br, bg_g, bb) = hsv_to_rgb(1.0 / 6.0, 0.9, bg_val);
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
        let mut fire = Fire::new();
        let fft = FftSnapshot::new(vec![100.0; 128], 48_000, 256);
        let mut grid = CellGrid::new(40, 12);
        // Run a few frames so heat propagates upward.
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            fire.render_tui(&mut ctx, &fft);
        }
        // All cells should be painted (not default black-on-black space).
        let non_empty = grid
            .cells()
            .iter()
            .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
            .count();
        assert_eq!(non_empty, grid.cells().len());
    }

    #[test]
    fn energy_smoothing_responds_to_input() {
        let mut fire = Fire::new();
        assert_eq!(fire.energy.energy(), 0.0);
        let loud = FftSnapshot::new(vec![5000.0; 64], 48_000, 128);
        for _ in 0..20 {
            fire.energy.update(&loud);
        }
        assert!(fire.energy.energy() > 0.5);
    }

    #[test]
    fn resize_resets_heat_buffer() {
        let mut fire = Fire::new();
        fire.ensure_buffer(10, 5);
        assert_eq!(fire.heat.len(), 50);
        fire.heat[0] = 0.9;
        // Resize should clear.
        fire.ensure_buffer(8, 4);
        assert_eq!(fire.heat.len(), 32);
        assert_eq!(fire.heat[0], 0.0);
    }
}
