//! Metaballs — "blob lamp" demoscene staple. A handful of moving charges
//! each emit a `r²/d²` potential field; summing the potentials and drawing
//! an iso-surface gives organic, merging blobs. We don't threshold to a
//! binary boundary — instead the field value drives intensity and hue, so
//! every cell carries continuous gradient + an iso-line rim for definition.

use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::density_ramp::DensityRamp;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

const NUM_BALLS: usize = 6;

#[derive(Clone, Copy)]
struct Ball {
    freq_x: f32,
    freq_y: f32,
    amp_x: f32,
    amp_y: f32,
    phase_x: f32,
    phase_y: f32,
    radius: f32,
}

pub struct Metaballs {
    start: Instant,
    balls: [Ball; NUM_BALLS],
    ramp: DensityRamp,
    energy: EnergyTracker,
}

impl Metaballs {
    pub fn new() -> Self {
        // Hand-picked orbits. The point isn't randomness, it's that the
        // ratios are mutually irrational so the pattern never loops.
        let balls = [
            Ball {
                freq_x: 0.23,
                freq_y: 0.31,
                amp_x: 0.34,
                amp_y: 0.30,
                phase_x: 0.0,
                phase_y: 0.6,
                radius: 0.22,
            },
            Ball {
                freq_x: 0.19,
                freq_y: 0.27,
                amp_x: 0.30,
                amp_y: 0.36,
                phase_x: 1.7,
                phase_y: 2.2,
                radius: 0.18,
            },
            Ball {
                freq_x: 0.33,
                freq_y: 0.22,
                amp_x: 0.28,
                amp_y: 0.32,
                phase_x: 3.1,
                phase_y: 0.9,
                radius: 0.20,
            },
            Ball {
                freq_x: 0.41,
                freq_y: 0.17,
                amp_x: 0.25,
                amp_y: 0.28,
                phase_x: 4.4,
                phase_y: 1.5,
                radius: 0.16,
            },
            Ball {
                freq_x: 0.13,
                freq_y: 0.39,
                amp_x: 0.36,
                amp_y: 0.25,
                phase_x: 2.2,
                phase_y: 5.0,
                radius: 0.24,
            },
            Ball {
                freq_x: 0.29,
                freq_y: 0.11,
                amp_x: 0.22,
                amp_y: 0.34,
                phase_x: 0.9,
                phase_y: 3.7,
                radius: 0.19,
            },
        ];
        Self {
            start: Instant::now(),
            balls,
            ramp: DensityRamp::midrange(),
            energy: EnergyTracker::new(0.6, 0.92, 500.0),
        }
    }

}

impl Default for Metaballs {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Metaballs {
    fn id(&self) -> VisualiserId {
        VisualiserId::Metaballs
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.energy.update(fft);
        let t = self.start.elapsed().as_secs_f32() * (1.0 + 0.5 * self.energy.energy());

        let grid: &mut CellGrid = &mut *ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        const ASPECT: f32 = 2.0;
        let w_f = w as f32;
        let h_f = h as f32 * ASPECT;

        // Resolve ball positions in normalised [0, 1]² space so we can
        // scale them to any terminal size uniformly. Then convert to
        // virtual-pixel coordinates for the distance math.
        let mut pos = [(0.0f32, 0.0f32); NUM_BALLS];
        let mut radii2 = [0.0f32; NUM_BALLS];
        let diag = (w_f * w_f + h_f * h_f).sqrt();
        for (i, ball) in self.balls.iter().enumerate() {
            let nx = 0.5 + ball.amp_x * (t * ball.freq_x + ball.phase_x).sin();
            let ny = 0.5 + ball.amp_y * (t * ball.freq_y + ball.phase_y).cos();
            pos[i] = (nx * w_f, ny * h_f);
            // Radius in virtual-pixel units. Breathes with energy.
            let r = ball.radius * diag * (0.85 + 0.3 * self.energy.energy());
            radii2[i] = r * r;
        }

        for y in 0..h {
            let yf = y as f32 * ASPECT;
            for x in 0..w {
                let xf = x as f32;

                let mut field = 0.0f32;
                for i in 0..NUM_BALLS {
                    let dx = xf - pos[i].0;
                    let dy = yf - pos[i].1;
                    let d2 = dx * dx + dy * dy + 1e-3;
                    field += radii2[i] / d2;
                }

                // field = 1.0 is the canonical iso-surface; we let it
                // climb higher inside blobs and fade outside.
                let inside = (field - 1.0).max(0.0);
                let core = (inside * 0.6).min(1.0);
                // Soft falloff outside (so the space around blobs glows).
                let halo = (field * 0.9).clamp(0.0, 1.0);
                let intensity = (core * 0.7 + halo * 0.5).clamp(0.0, 1.0);

                // Iso-ring: brighter lines where `field` crosses integer
                // boundaries. Gives each blob a clean rim without us
                // having to do edge detection.
                let iso = (1.0 - (field.fract() - 0.5).abs() * 2.0).clamp(0.0, 1.0);
                let iso_boost = iso.powi(4);

                // Hue: rainbow keyed to field value + slow time drift.
                let hue = (field * 0.18 + t * 0.07).fract();
                let sat = 0.78;
                let val = lerp(0.08, 1.0, intensity) + 0.25 * iso_boost;
                let (r, g, b) = hsv_to_rgb(hue, sat, val.min(1.3));
                let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

                // Darker complementary bg so the cell has depth.
                let (br, bg_g, bb) = hsv_to_rgb((hue + 0.5).fract(), 0.7, 0.04 + 0.06 * intensity);
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
        let mut m = Metaballs::new();
        let fft = FftSnapshot::new(vec![300.0; 64], 48_000, 128);
        let mut grid = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            m.render_tui(&mut ctx, &fft);
        }
        let non_black = grid
            .cells()
            .iter()
            .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
            .count();
        assert_eq!(non_black, 30 * 10, "all cells should be painted");
    }
}
