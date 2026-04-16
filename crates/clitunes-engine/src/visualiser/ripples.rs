//! Ripples — raindrops on water. A pool of `Drop` sources each radiate an
//! expanding cosine ring; rings from different drops interfere, so the
//! surface turns into a moire of overlapping waves. FFT energy seeds new
//! drops, so loud passages "rain harder".
//!
//! The field value at a cell is the sum of
//! `cos(2π(dist - age*SPEED)/WAVELENGTH) * amp * decay(age)` across all
//! live drops, windowed by a ring gate so energy only contributes near the
//! actual wavefront rather than everywhere in the disc.

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::time::Instant;

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::density_ramp::DensityRamp;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

const MAX_DROPS: usize = 14;
/// Wave propagation speed in virtual-pixel units per second.
const DROP_SPEED: f32 = 14.0;
/// Time in seconds for a drop's amplitude to decay by 1/e.
const DROP_DECAY: f32 = 2.8;
/// Wavelength in virtual pixels.
const WAVELENGTH: f32 = 4.5;
/// Half-width of the annular window around the leading wavefront.
const RING_HALF_WIDTH: f32 = 6.0;
/// Seconds between guaranteed ambient drops when there is no audio.
const BASE_SPAWN_INTERVAL: f32 = 1.1;

#[derive(Clone, Copy)]
struct Drop {
    x: f32,
    y: f32,
    spawned_at: f32,
    amp: f32,
    hue: f32,
}

pub struct Ripples {
    start: Instant,
    drops: VecDeque<Drop>,
    next_ambient: f32,
    rng_state: u32,
    ramp: DensityRamp,
    energy: EnergyTracker,
    energy_cooldown: f32,
}

impl Ripples {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            drops: VecDeque::with_capacity(MAX_DROPS),
            next_ambient: 0.0,
            rng_state: 0xDEAD_BEEF,
            ramp: DensityRamp::new(" .·∙•○◎◉●◐◑◒◓◔◕"),
            energy: EnergyTracker::new(0.5, 0.88, 600.0),
            energy_cooldown: 0.0,
        }
    }

    /// 32-bit xorshift — good enough for drop positions and pocket change.
    fn rand(&mut self) -> f32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x.max(1);
        (x as f32) / (u32::MAX as f32)
    }

    fn spawn_drop(&mut self, t: f32, w_f: f32, h_f: f32, amp: f32) {
        if self.drops.len() >= MAX_DROPS {
            self.drops.pop_front();
        }
        let x = self.rand() * w_f;
        let y = self.rand() * h_f;
        let hue = self.rand();
        self.drops.push_back(Drop {
            x,
            y,
            spawned_at: t,
            amp,
            hue,
        });
    }

}

impl Default for Ripples {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Ripples {
    fn id(&self) -> VisualiserId {
        VisualiserId::Ripples
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

        const ASPECT: f32 = 2.0;
        let w_f = w as f32;
        let h_f = h as f32 * ASPECT;

        // Ambient rain — guarantees life even in silence.
        if t >= self.next_ambient {
            self.spawn_drop(t, w_f, h_f, 0.7);
            self.next_ambient = t + BASE_SPAWN_INTERVAL;
        }
        // Audio-driven drops: a loud beat splats a big, bright drop. The
        // cooldown keeps a sustained tone from spamming new sources.
        self.energy_cooldown = (self.energy_cooldown - 0.033).max(0.0);
        if self.energy.energy() > 0.35 && self.energy_cooldown <= 0.0 {
            self.spawn_drop(t, w_f, h_f, 0.9 + self.energy.energy() * 0.8);
            self.energy_cooldown = 0.15;
        }

        // Retire drops whose envelope has fallen below perceptible.
        while let Some(front) = self.drops.front() {
            if (t - front.spawned_at) / DROP_DECAY > 4.0 {
                self.drops.pop_front();
            } else {
                break;
            }
        }

        for y in 0..h {
            let yf = y as f32 * ASPECT;
            for x in 0..w {
                let xf = x as f32;

                // Superposition across active drops.
                let mut field = 0.0f32;
                let mut hue_acc = 0.0f32;
                let mut weight_acc = 1e-6f32;
                for drop in self.drops.iter() {
                    let age = t - drop.spawned_at;
                    if age < 0.0 {
                        continue;
                    }
                    let dx = xf - drop.x;
                    let dy = yf - drop.y;
                    let dist = (dx * dx + dy * dy).sqrt();
                    let front = age * DROP_SPEED;
                    let ring = (-((dist - front).powi(2) / (2.0 * RING_HALF_WIDTH.powi(2)))).exp();
                    let decay = (-age / DROP_DECAY).exp();
                    let wave = (TAU * (dist - front) / WAVELENGTH).cos();
                    let contrib = wave * drop.amp * decay * ring;
                    field += contrib;
                    let w = contrib.abs();
                    hue_acc += drop.hue * w;
                    weight_acc += w;
                }

                let intensity = ((field * 0.5 + 0.5) * 0.9 + 0.05).clamp(0.0, 1.0);

                // Watery blue/teal base palette, modulated by drop hue
                // contribution so bright ringfronts pick up individuality.
                let drop_hue = hue_acc / weight_acc;
                let hue = 0.52 + 0.10 * (drop_hue - 0.5) + 0.02 * (t * 0.1).sin();
                let sat = lerp(0.45, 0.85, intensity);
                let val = lerp(0.10, 0.95, intensity);
                let (r, g, b) = hsv_to_rgb(hue, sat, val);
                let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));

                // Background: dark complementary tint so the surface feels
                // like it has depth rather than sitting on pure black.
                let bg_val = 0.03 + 0.04 * intensity;
                let (br, bg_g, bb) = hsv_to_rgb(hue + 0.05, 0.9, bg_val);
                let bg = Rgb::new(f32_to_u8(br), f32_to_u8(bg_g), f32_to_u8(bb));

                let glyph_intensity = lerp(0.05, 1.0, intensity);
                let ch = self.ramp.pick(glyph_intensity);

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
        let mut r = Ripples::new();
        let fft = FftSnapshot::new(vec![100.0; 64], 48_000, 128);
        let mut grid = CellGrid::new(32, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            r.render_tui(&mut ctx, &fft);
        }
        let non_black = grid
            .cells()
            .iter()
            .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
            .count();
        assert!(non_black > 0, "at least the ambient drop should paint");
    }

    #[test]
    fn loud_audio_spawns_drops() {
        let mut r = Ripples::new();
        let loud = FftSnapshot::new(vec![5000.0; 64], 48_000, 128);
        let mut grid = CellGrid::new(32, 10);
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            r.render_tui(&mut ctx, &loud);
        }
        assert!(!r.drops.is_empty(), "loud audio should spawn drops");
    }
}
