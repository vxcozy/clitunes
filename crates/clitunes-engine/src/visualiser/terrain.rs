//! Terrain — side-view scrolling mountain range shaped by audio spectrum using braille rendering.

use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

pub struct Terrain {
    braille: BrailleBuffer,
    energy: EnergyTracker,
    heights: Vec<f32>,
    last_w: u16,
    last_h: u16,
}

impl Terrain {
    pub fn new() -> Self {
        Self {
            braille: BrailleBuffer::new(1, 1),
            // Release tau ~115 ms: mountain heights track current loudness
            // without a trailing second of inertia behind the spectrum.
            energy: EnergyTracker::new(0.5, 0.75, 500.0),
            heights: Vec::new(),
            last_w: 0,
            last_h: 0,
        }
    }

    fn ensure_buffer(&mut self, w: u16, h: u16) {
        if self.last_w != w || self.last_h != h {
            self.braille.resize(w, h);
            self.last_w = w;
            self.last_h = h;
            let sub_w = self.braille.width() as usize;
            self.heights.clear();
            self.heights.resize(sub_w, 0.0);
        }
    }
}

impl Default for Terrain {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple deterministic hash for procedural noise, producing a value in [0, 1].
fn hash_noise(x: usize) -> f32 {
    let mut h = x as u32;
    h = h.wrapping_mul(2654435761);
    h ^= h >> 16;
    h = h.wrapping_mul(2246822519);
    h ^= h >> 13;
    (h & 0xFFFF) as f32 / 65535.0
}

impl Visualiser for Terrain {
    fn id(&self) -> VisualiserId {
        VisualiserId::Terrain
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        let energy = self.energy.update(fft);

        let grid: &mut CellGrid = ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        self.ensure_buffer(w, h);
        self.braille.clear();

        let sub_w = self.braille.width();
        let sub_h = self.braille.height();
        if sub_w == 0 || sub_h == 0 {
            return;
        }

        let len = self.heights.len();

        // Compute new height from FFT: sample multiple frequency bands for varied terrain.
        let new_height = if fft.magnitudes.is_empty() {
            0.2
        } else {
            let avg: f32 = fft.magnitudes.iter().sum::<f32>() / fft.magnitudes.len() as f32;
            let compressed = (1.0 + avg / 500.0).ln().min(1.0);
            let noise = hash_noise(len.wrapping_add((energy * 10000.0) as usize));
            (compressed * 0.5 + noise * 0.3 + 0.2).clamp(0.0, 1.0)
        };

        // Scroll heights left by 1 and push new height.
        if len > 1 {
            self.heights.copy_within(1.., 0);
        }
        if len > 0 {
            self.heights[len - 1] = new_height;
        }

        // Draw filled terrain: for each sub-pixel column, fill from terrain_y down.
        let sub_h_f = sub_h as f32;
        for x in 0..sub_w {
            let h_val = if (x as usize) < self.heights.len() {
                self.heights[x as usize]
            } else {
                0.0
            };
            // Map height [0, 1] to screen: 0.0 → near bottom (0.9), 1.0 → near top (0.1).
            let scaled = lerp(0.1, 0.9, h_val);
            let terrain_y = ((1.0 - scaled) * sub_h_f) as u16;
            let terrain_y = terrain_y.min(sub_h.saturating_sub(1));

            for y in terrain_y..sub_h {
                self.braille.set(x, y, true);
            }
        }

        // Compose with altitude-based colour.
        let cell_h = h as f32;
        let bg = Rgb::new(2, 2, 8);

        self.braille.compose(grid, |_cx, cy, dot_count| {
            if dot_count == 0 {
                return (bg, bg);
            }

            let altitude = 1.0 - (cy as f32 / cell_h);
            let altitude = altitude.clamp(0.0, 1.0);

            // High altitude: lighter green; low altitude: darker brown.
            let (fr, fg_c, fb) = if altitude > 0.5 {
                // Upper terrain — green mountain peaks.
                let t = (altitude - 0.5) * 2.0;
                hsv_to_rgb(0.28, lerp(0.4, 0.8, t), lerp(0.7, 0.3, t))
            } else {
                // Lower terrain — brown earth.
                let t = altitude * 2.0;
                hsv_to_rgb(0.08, lerp(0.6, 0.5, t), lerp(0.3, 0.5, t))
            };

            let fg = Rgb::new(f32_to_u8(fr), f32_to_u8(fg_c), f32_to_u8(fb));
            (fg, bg)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_fft() -> FftSnapshot {
        FftSnapshot {
            magnitudes: vec![5000.0; 64],
            sample_rate: 48_000,
            fft_size: 128,
            samples: vec![0.5; 128],
        }
    }

    #[test]
    fn render_paints_cells() {
        let mut terrain = Terrain::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(40, 12);
        // Feed several frames so the terrain scrolls across.
        for _ in 0..50 {
            let mut ctx = TuiContext { grid: &mut grid };
            terrain.render_tui(&mut ctx, &fft);
        }
        let braille_count = grid
            .cells()
            .iter()
            .filter(|c| c.ch != '\u{2800}' && c.ch != ' ')
            .count();
        assert!(
            braille_count > 0,
            "loud FFT should produce non-blank braille cells, got {braille_count}"
        );
    }

    #[test]
    fn output_changes_between_frames() {
        let mut terrain = Terrain::new();
        let fft = loud_fft();

        let mut grid_a = CellGrid::new(30, 10);
        for _ in 0..20 {
            let mut ctx = TuiContext { grid: &mut grid_a };
            terrain.render_tui(&mut ctx, &fft);
        }

        let mut grid_b = CellGrid::new(30, 10);
        for _ in 0..5 {
            let mut ctx = TuiContext { grid: &mut grid_b };
            terrain.render_tui(&mut ctx, &fft);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(
            diff > 0,
            "different frame counts should produce different output due to scrolling"
        );
    }

    #[test]
    fn resize_no_panic() {
        let mut terrain = Terrain::new();
        let fft = loud_fft();
        for (w, h) in [(80, 24), (40, 12), (1, 1), (200, 50)] {
            let mut grid = CellGrid::new(w, h);
            let mut ctx = TuiContext { grid: &mut grid };
            terrain.render_tui(&mut ctx, &fft);
        }
    }
}
