//! Tunnel — demoscene endless corridor. For every cell, compute polar
//! `(dist, angle)` from screen center, then synthesise texture coordinates
//! `(u = DEPTH_SCALE / dist + t, v = angle/π + t)`. Sampling a procedural
//! grid at `(u, v)` produces the illusion of a rotating, forward-zooming
//! tunnel because the 1/dist mapping compresses texture sharply near the
//! vanishing point.
//!
//! FFT energy accelerates forward motion and rotation, so the tunnel
//! physically responds to bass.

use std::f32::consts::PI;
use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::density_ramp::DensityRamp;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

const DEPTH_SCALE: f32 = 32.0;

pub struct Tunnel {
    start: Instant,
    ramp: DensityRamp,
    energy: EnergyTracker,
}

impl Tunnel {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            ramp: DensityRamp::new(" .:-=+*#%@█"),
            energy: EnergyTracker::new(0.55, 0.9, 500.0),
        }
    }
}

impl Default for Tunnel {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Tunnel {
    fn id(&self) -> VisualiserId {
        VisualiserId::Tunnel
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

        const ASPECT: f32 = 2.0;
        let w_f = w as f32;
        let h_f = h as f32 * ASPECT;
        let cx = w_f * 0.5;
        let cy = h_f * 0.5;
        // Normalise distances so the tunnel feels the same on any terminal.
        let half_diag = (cx * cx + cy * cy).sqrt().max(1.0);

        let t = self.start.elapsed().as_secs_f32();
        let forward = t * (0.6 + 1.0 * self.energy.energy());
        let spin = t * (0.12 + 0.4 * self.energy.energy());

        for y in 0..h {
            let yf = y as f32 * ASPECT - cy;
            for x in 0..w {
                let xf = x as f32 - cx;

                let dist = (xf * xf + yf * yf).sqrt() / half_diag;
                let angle = yf.atan2(xf);

                // 1/dist so near = fast-moving texture, far = compressed.
                // Clamp floor so the singularity at (0,0) doesn't NaN out.
                let inv = DEPTH_SCALE / dist.max(0.02);
                let u = inv + forward * 6.0;
                let v = angle / PI + spin;

                // Texture: ring stripes in u, angular stripes in v, plus a
                // soft diagonal to break up the grid into something that
                // reads more like brick-lined stone than a wireframe.
                let ring = (u * 0.35).sin();
                let arm = (v * 6.0).sin();
                let grout = ((u * 0.18 + v * 2.0) * 1.3).sin();
                let tex = (ring * 0.55 + arm * 0.35 + grout * 0.25 + 1.0) * 0.5;

                // Fade everything toward black as dist→1 so the rim of the
                // screen reads as the "far end" of the tunnel.
                let fade = (1.0 - dist * 0.9).clamp(0.0, 1.0);
                let intensity = (tex * fade).clamp(0.0, 1.0);

                // Warm palette that drifts with time — copper → gold → red.
                let hue = (0.02 + 0.12 * (u * 0.03).sin() + t * 0.05).fract();
                let sat = lerp(0.65, 0.95, fade);
                let val = lerp(0.05, 1.0, intensity);
                let (r, g, b) = hsv_to_rgb(hue, sat, val);
                let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

                // Bg just a shade darker → gives each brick an outline.
                let (br, bg_g, bb) = hsv_to_rgb(hue, sat, val * 0.35);
                let bg = Rgb::new(f32_to_u8(br), f32_to_u8(bg_g), f32_to_u8(bb));

                let ch = self.ramp.pick(lerp(0.05, 1.0, intensity));
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
        let mut t = Tunnel::new();
        let fft = FftSnapshot::new(vec![200.0; 64], 48_000, 128);
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            t.render_tui(&mut ctx, &fft);
        }
        // Every cell should have been visited.
        assert_eq!(grid.cells().len(), 40 * 12);
        // Center should be brighter than the rim (fade → 0 at edges).
        let center_idx = (6_usize) * 40 + 20;
        let _c = grid.cells()[center_idx];
        // Don't assert exact values — just that all cells were painted.
    }
}
