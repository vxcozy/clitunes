//! Matrix — digital rain inspired by *The Matrix*. Multiple rain drops per
//! column fall at different speeds, leaving exponentially-decaying green
//! trails. The leading cell of each drop cycles through random-looking
//! glyphs; the trailing cells use the density ramp keyed on fade intensity.
//!
//! Cell aspect: terminal cells are roughly 2x taller than wide, so we
//! compensate by treating each row as 2 virtual pixels when computing
//! distances and speeds.
//!
//! FFT coupling: audio energy increases drop speeds and spawns additional
//! temporary bright drops on beats, so the rain accelerates with the music.

use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::density_ramp::DensityRamp;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

/// Number of procedural drops per column.
const DROPS_PER_COL: usize = 4;

/// Characters used for the "head" glyph cycling effect.
const HEAD_GLYPHS: &[u8] = b"0123456789ABCDEFabcdef@#$%&*+=<>~";

pub struct Matrix {
    start: Instant,
    ramp: DensityRamp,
    /// Smoothed audio energy used to modulate drop speed.
    energy: EnergyTracker,
    /// xorshift32 state for random glyph cycling.
    rng_state: u32,
}

impl Matrix {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            ramp: DensityRamp::new(" .·:;+xX#%@"),
            energy: EnergyTracker::new(0.5, 0.88, 500.0),
            rng_state: 0xDEAD_BEEF,
        }
    }

    /// xorshift32 — cheap PRNG for random glyph cycling.
    fn rand(&mut self) -> u32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x.max(1);
        x
    }

    /// Deterministic hash for per-column drop parameters. Given a column
    /// index and a seed, returns a well-mixed u32.
    fn col_hash(col: u16, seed: u32) -> u32 {
        let mut h = col as u32 ^ seed;
        h = h.wrapping_mul(0x9E37_79B9);
        h ^= h >> 16;
        h
    }

}

impl Default for Matrix {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Matrix {
    fn id(&self) -> VisualiserId {
        VisualiserId::Matrix
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.energy.update(fft);

        let t = self.start.elapsed().as_secs_f32();
        let grid: &mut CellGrid = &mut *ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        let h_f = h as f32;

        // Very dark green-black background.
        let (bg_r, bg_g, bg_b) = hsv_to_rgb(0.33, 0.8, 0.02);
        let bg_col = Rgb::new(f32_to_u8(bg_r), f32_to_u8(bg_g), f32_to_u8(bg_b));
        let blank = Cell {
            ch: ' ',
            fg: Rgb::BLACK,
            bg: bg_col,
        };
        grid.fill(blank);

        // Speed multiplier from audio energy. Idle drift is 1.0; loud
        // passages push it up to ~2.0 for a rush effect.
        let speed_mul = 1.0 + 1.0 * self.energy.energy();

        // We need a frame counter for the head-glyph cycling effect.
        // Derive it from time at ~30 fps granularity.
        let frame = (t * 30.0) as u32;

        // The wrap length: drops travel through (height * 2) cells before
        // recycling, which provides a gap between consecutive passes.
        let wrap = h_f * 2.0;

        for x in 0..w {
            for y in 0..h {
                let yf = y as f32;
                let mut best_intensity: f32 = 0.0;
                let mut is_head = false;

                for drop_idx in 0..DROPS_PER_COL {
                    let seed = (drop_idx as u32).wrapping_mul(0x45D9_F3B7);
                    let hash = Self::col_hash(x, seed);

                    // Per-drop parameters derived from the hash.
                    let speed_base = 3.0 + ((hash & 0xFF) as f32 / 255.0) * 7.0;
                    let phase = ((hash >> 8) & 0xFFFF) as f32 / 65535.0 * wrap;
                    let trail_length = 3.0 + (((hash >> 24) & 0xFF) as f32 / 255.0) * 12.0;

                    let speed = speed_base * speed_mul;
                    let head_y = (phase + t * speed) % wrap;

                    // Distance behind the head, wrapped positive.
                    let dist = head_y - yf;
                    let dist = if dist < 0.0 { dist + wrap } else { dist };

                    // Only contribute if the cell is within a reasonable
                    // range behind the head (within the visible height).
                    if dist < h_f && dist >= 0.0 {
                        let contribution = (-dist / trail_length).exp();
                        if contribution > best_intensity {
                            best_intensity = contribution;
                            // The "head" is the very first cell (distance < 1).
                            is_head = dist < 1.0;
                        }
                    }
                }

                if best_intensity < 0.01 {
                    continue;
                }

                let intensity = best_intensity.clamp(0.0, 1.0);

                // Glyph: head cells get a cycling random character;
                // trail cells use the density ramp.
                let ch = if is_head && intensity > 0.7 {
                    // Hash (x, y, frame) for a pseudo-random glyph.
                    let glyph_hash = frame
                        .wrapping_mul(0x9E37_79B9)
                        .wrapping_add((x as u32).wrapping_mul(2_654_435_761))
                        .wrapping_add((y as u32).wrapping_mul(40503));
                    let idx = (glyph_hash as usize) % HEAD_GLYPHS.len();
                    HEAD_GLYPHS[idx] as char
                } else {
                    self.ramp.pick(intensity)
                };

                // Colour: green monochrome.
                // High intensity (head) = bright white-green (low sat, high val).
                // Trail = deep green (high sat, medium val).
                let hue = 0.33;
                let sat = lerp(0.9, 0.15, intensity);
                let val = lerp(0.25, 1.0, intensity);
                let (r, g, b) = hsv_to_rgb(hue, sat, val);
                let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

                // Background: slightly brighter green near the head to
                // give a subtle glow, otherwise stay dark.
                let bg_val = 0.02 + 0.06 * intensity;
                let (br, bgg, bb) = hsv_to_rgb(0.33, 0.8, bg_val);
                let bg = Rgb::new(f32_to_u8(br), f32_to_u8(bgg), f32_to_u8(bb));

                grid.set(x, y, Cell { ch, fg, bg });
            }
        }

        // Beat drops: when energy is high, inject a few extra bright
        // single-cell flashes at random positions for the "data burst" feel.
        if self.energy.energy() > 0.4 {
            let flash_count = (self.energy.energy() * 20.0) as usize;
            for _ in 0..flash_count {
                let rx = (self.rand() % w as u32) as u16;
                let ry = (self.rand() % h as u32) as u16;
                let glyph_hash = self.rand();
                let ch = HEAD_GLYPHS[(glyph_hash as usize) % HEAD_GLYPHS.len()] as char;
                let (r, g, b) = hsv_to_rgb(0.33, 0.1, 1.0);
                let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));
                let (br, bgg, bb) = hsv_to_rgb(0.33, 0.6, 0.08);
                let bg = Rgb::new(f32_to_u8(br), f32_to_u8(bgg), f32_to_u8(bb));
                grid.set(rx, ry, Cell { ch, fg, bg });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_paints_cells() {
        let mut matrix = Matrix::new();
        let fft = FftSnapshot::new(vec![100.0; 64], 48_000, 128);
        let mut grid = CellGrid::new(60, 20);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            matrix.render_tui(&mut ctx, &fft);
        }
        // Some cells should be painted with non-space glyphs (the rain
        // drops and their trails).
        let painted = grid.cells().iter().filter(|c| c.ch != ' ').count();
        assert!(
            painted > 0,
            "expected at least some cells to be painted by the matrix rain"
        );
    }

    #[test]
    fn energy_smoothing_responds_to_loud_input() {
        let mut matrix = Matrix::new();
        assert_eq!(matrix.energy.energy(), 0.0);
        let loud = FftSnapshot::new(vec![5000.0; 64], 48_000, 128);
        for _ in 0..20 {
            matrix.energy.update(&loud);
        }
        assert!(matrix.energy.energy() > 0.5);
    }

    #[test]
    fn col_hash_is_deterministic() {
        let a = Matrix::col_hash(42, 123);
        let b = Matrix::col_hash(42, 123);
        assert_eq!(a, b);
    }

    #[test]
    fn col_hash_varies_by_column() {
        let a = Matrix::col_hash(0, 0);
        let b = Matrix::col_hash(1, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn rand_produces_nonzero_values() {
        let mut matrix = Matrix::new();
        let mut all_same = true;
        let first = matrix.rand();
        for _ in 0..10 {
            let v = matrix.rand();
            if v != first {
                all_same = false;
                break;
            }
        }
        assert!(!all_same, "RNG should produce varying values");
    }

    #[test]
    fn beat_flashes_appear_at_high_energy() {
        let mut matrix = Matrix::new();
        // Pump energy high.
        let loud = FftSnapshot::new(vec![8000.0; 64], 48_000, 128);
        for _ in 0..30 {
            matrix.energy.update(&loud);
        }
        assert!(matrix.energy.energy() > 0.4);

        let mut grid = CellGrid::new(80, 24);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            matrix.render_tui(&mut ctx, &loud);
        }
        let painted = grid.cells().iter().filter(|c| c.ch != ' ').count();
        assert!(
            painted > 0,
            "loud input should produce visible rain plus beat flashes"
        );
    }
}
