/// Twinkling braille particle field where dot density follows audio energy.
use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

pub struct Scatter {
    energy: EnergyTracker,
    braille: BrailleBuffer,
    frame: u32,
    last_w: u16,
    last_h: u16,
}

impl Scatter {
    pub fn new() -> Self {
        Self {
            energy: EnergyTracker::new(0.5, 0.88, 500.0),
            braille: BrailleBuffer::new(1, 1),
            frame: 0,
            last_w: 0,
            last_h: 0,
        }
    }

    #[inline]
    fn hash(x: u32, y: u32, frame: u32) -> u32 {
        let mut h = x
            .wrapping_mul(374761393)
            .wrapping_add(y.wrapping_mul(668265263))
            .wrapping_add(frame.wrapping_mul(1013904223));
        h = (h ^ (h >> 13)).wrapping_mul(1274126177);
        h ^= h >> 16;
        h
    }
}

impl Default for Scatter {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Scatter {
    fn id(&self) -> VisualiserId {
        VisualiserId::Scatter
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.energy.update(fft);
        self.frame = self.frame.wrapping_add(1);

        let grid: &mut CellGrid = ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        if w != self.last_w || h != self.last_h {
            self.braille.resize(w, h);
            self.last_w = w;
            self.last_h = h;
        }
        self.braille.clear();

        let bw = self.braille.width();
        let bh = self.braille.height();
        let bh_f = bh as f32;

        let mags = &fft.magnitudes;
        let num_bands = mags.len().min(64).max(1);

        for y in 0..bh {
            for x in 0..bw {
                let band_idx = (x as usize) * num_bands / (bw as usize).max(1);
                let band_idx = band_idx.min(num_bands - 1);

                let band_energy = (1.0 + mags[band_idx] / 1000.0).ln().min(1.0);
                let vertical_factor = 1.0 - (y as f32 / bh_f) * 0.7;
                let probability = band_energy * band_energy * vertical_factor;

                let h_val = Self::hash(x as u32, y as u32, self.frame);
                let noise = (h_val % 1000) as f32 / 1000.0;

                if noise < probability {
                    self.braille.set(x, y, true);
                }
            }
        }

        let bg = Rgb::new(4, 2, 0);
        self.braille.compose(grid, |_cx, _cy, dot_count| {
            let intensity = dot_count as f32 / 8.0;
            let fg = {
                let sat = lerp(0.5, 0.9, intensity);
                let val = lerp(0.3, 1.0, intensity);
                let (r, g, b) = hsv_to_rgb(0.08, sat, val);
                Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b))
            };
            (fg, bg)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_fft() -> FftSnapshot {
        FftSnapshot::new(vec![5000.0; 128], 48_000, 256)
    }

    #[test]
    fn render_paints_cells() {
        let mut scatter = Scatter::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(40, 12);
        for _ in 0..5 {
            let mut ctx = TuiContext { grid: &mut grid };
            scatter.render_tui(&mut ctx, &fft);
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
        let mut scatter = Scatter::new();
        let fft = loud_fft();

        let mut grid_a = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            scatter.render_tui(&mut ctx, &fft);
        }

        let mut grid_b = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            scatter.render_tui(&mut ctx, &fft);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(
            diff > 0,
            "consecutive frames should differ due to frame counter"
        );
    }

    #[test]
    fn resize_no_panic() {
        let mut scatter = Scatter::new();
        let fft = loud_fft();

        let mut grid = CellGrid::new(80, 24);
        let mut ctx = TuiContext { grid: &mut grid };
        scatter.render_tui(&mut ctx, &fft);

        let mut grid = CellGrid::new(40, 12);
        let mut ctx = TuiContext { grid: &mut grid };
        scatter.render_tui(&mut ctx, &fft);
    }
}
