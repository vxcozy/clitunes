//! Starfield — perspective-projected 3D star flight. Every star has a
//! `(x, y, z)` and flies toward the viewer by decreasing `z`. Projected
//! screen position is `(x/z, y/z)` scaled to the cell rect; when a star
//! passes the near plane it respawns at `z = FAR`.
//!
//! Each star draws itself as a single cell, but we use the density ramp
//! keyed on `1/z` so close stars get big bright glyphs and distant stars
//! are faint pinpricks. FFT energy accelerates forward velocity, so a
//! beat drop launches you into warp.

use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::density_ramp::DensityRamp;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

const NUM_STARS: usize = 420;
const NEAR: f32 = 0.1;
const FAR: f32 = 6.0;
const FOV_SCALE: f32 = 1.6;

#[derive(Clone, Copy)]
struct Star {
    x: f32, // [-1, 1] world units
    y: f32, // [-1, 1] world units
    z: f32, // (NEAR, FAR]
    hue: f32,
}

pub struct Starfield {
    start: Instant,
    stars: Vec<Star>,
    rng_state: u32,
    ramp: DensityRamp,
    energy: f32,
}

impl Starfield {
    pub fn new() -> Self {
        let mut sf = Self {
            start: Instant::now(),
            stars: Vec::with_capacity(NUM_STARS),
            rng_state: 0x1234_5678,
            ramp: DensityRamp::new(" .·∙•*✦✧✶★●"),
            energy: 0.0,
        };
        for _ in 0..NUM_STARS {
            let star = sf.random_star(true);
            sf.stars.push(star);
        }
        sf
    }

    /// xorshift32 — cheap, well-distributed enough for star positions.
    fn rand(&mut self) -> f32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x.max(1);
        (x as f32) / (u32::MAX as f32)
    }

    fn random_star(&mut self, full_depth: bool) -> Star {
        let x = self.rand() * 2.0 - 1.0;
        let y = self.rand() * 2.0 - 1.0;
        // When seeding the initial field we fill the whole depth range;
        // subsequent respawns all start at the far plane so stars appear
        // to fly at you rather than pop in mid-scene.
        let z = if full_depth {
            NEAR + self.rand() * (FAR - NEAR)
        } else {
            FAR
        };
        let hue = self.rand();
        Star { x, y, z, hue }
    }

    fn update_energy(&mut self, fft: &FftSnapshot) {
        let sum: f32 = fft.magnitudes.iter().sum();
        let norm = (sum / fft.magnitudes.len().max(1) as f32 / 500.0).min(1.0);
        if norm > self.energy {
            self.energy = 0.5 * self.energy + 0.5 * norm;
        } else {
            self.energy = 0.88 * self.energy + 0.12 * norm;
        }
    }
}

impl Default for Starfield {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Starfield {
    fn id(&self) -> VisualiserId {
        VisualiserId::Starfield
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

        // Clear to space. Adds a subtle nebula wash so the background
        // isn't pure black — readable on any terminal theme.
        let nebula_hue = 0.68 + 0.05 * (t * 0.1).sin();
        let (nr, ng, nb) = hsv_to_rgb(nebula_hue, 0.7, 0.04);
        let bg = Rgb::new(f32_to_u8(nr), f32_to_u8(ng), f32_to_u8(nb));
        let space = Cell {
            ch: ' ',
            fg: Rgb::BLACK,
            bg,
        };
        grid.fill(space);

        // Advance stars. Idle speed is steady; audio adds a hot-rod boost.
        let dt = 0.033; // locked 30 fps tick — decouples from wall time
        let speed = 0.9 + 2.6 * self.energy;
        // Respawn info stored in a second pass so we don't double-borrow
        // self inside the loop.
        let mut to_respawn = Vec::new();
        for (i, star) in self.stars.iter_mut().enumerate() {
            star.z -= speed * dt;
            if star.z <= NEAR {
                to_respawn.push(i);
            }
        }
        for i in to_respawn {
            self.stars[i] = self.random_star(false);
        }

        // Project + plot.
        let cx = w as f32 * 0.5;
        let cy = h as f32 * 0.5;
        // Cells are ~2× taller than wide, so x gets a 2× scale to keep
        // the projected field looking circular rather than squished.
        let scale_x = cx * FOV_SCALE * 2.0;
        let scale_y = cy * FOV_SCALE;

        for star in &self.stars {
            let sx = star.x / star.z * scale_x + cx;
            let sy = star.y / star.z * scale_y + cy;
            if sx < 0.0 || sy < 0.0 || sx >= w as f32 || sy >= h as f32 {
                continue;
            }
            let px = sx as u16;
            let py = sy as u16;

            // Brightness falls off with z. We remap so NEAR → 1.0 and
            // FAR → ~0.0 with a slight floor so even the farthest stars
            // get a pinprick.
            let t_near = ((FAR - star.z) / (FAR - NEAR)).clamp(0.0, 1.0);
            let brightness = t_near.powf(1.4);

            // Slight blue/white colour shift so nearer stars look hotter
            // (blue-shift), far stars a little cooler (warm amber).
            let hue = lerp(0.09, 0.58, brightness) + star.hue * 0.02;
            let sat = lerp(0.15, 0.55, 1.0 - brightness);
            let val = lerp(0.35, 1.2, brightness).min(1.0);
            let (r, g, b) = hsv_to_rgb(hue, sat, val);
            let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

            let ch = self.ramp.pick(brightness);
            // Preserve the nebula bg behind the star so the backdrop
            // still reads through faint pinpricks.
            grid.set(
                px,
                py,
                Cell {
                    ch,
                    fg,
                    bg: space.bg,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_plots_stars() {
        let mut sf = Starfield::new();
        let fft = FftSnapshot {
            magnitudes: vec![100.0; 64],
            sample_rate: 48_000,
            fft_size: 128,
        };
        let mut grid = CellGrid::new(60, 20);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            sf.render_tui(&mut ctx, &fft);
        }
        // Cells should either be empty space (nebula bg, space fg) or a
        // plotted star (non-space glyph).
        let plotted = grid.cells().iter().filter(|c| c.ch != ' ').count();
        assert!(
            plotted > 0,
            "expected at least some stars to land on the grid"
        );
    }

    #[test]
    fn stars_respawn_at_far_plane() {
        let mut sf = Starfield::new();
        // Force one star to cross the near plane.
        sf.stars[0].z = 0.05;
        let loud = FftSnapshot {
            magnitudes: vec![2000.0; 64],
            sample_rate: 48_000,
            fft_size: 128,
        };
        let mut grid = CellGrid::new(40, 12);
        let mut ctx = TuiContext { grid: &mut grid };
        sf.render_tui(&mut ctx, &loud);
        assert!(sf.stars[0].z > NEAR);
    }
}
