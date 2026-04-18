/// Horizontally mirrored Rorschach inkblot spectrum using braille rendering.
use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

pub struct Butterfly {
    energy: EnergyTracker,
    braille: BrailleBuffer,
    frame: u32,
    last_w: u16,
    last_h: u16,
}

impl Butterfly {
    pub fn new() -> Self {
        Self {
            // Release tau ~115 ms: wings flap with the beat envelope
            // rather than coasting through quiet sections.
            energy: EnergyTracker::new(0.5, 0.75, 500.0),
            braille: BrailleBuffer::new(1, 1),
            frame: 0,
            last_w: 0,
            last_h: 0,
        }
    }

    fn ensure_buf(&mut self, w: u16, h: u16) {
        if self.last_w != w || self.last_h != h {
            self.braille.resize(w, h);
            self.last_w = w;
            self.last_h = h;
        }
    }
}

impl Default for Butterfly {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Butterfly {
    fn id(&self) -> VisualiserId {
        VisualiserId::Butterfly
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

        self.ensure_buf(w, h);
        self.braille.clear();

        let bw = self.braille.width();
        let bh = self.braille.height();
        if bw == 0 || bh == 0 {
            return;
        }

        let num_bands = fft.magnitudes.len().min(64);
        if num_bands == 0 {
            self.braille
                .compose(grid, |_, _, _| (Rgb::BLACK, Rgb::new(4, 0, 6)));
            return;
        }

        let half_width = bw / 2;
        let center_x = half_width;

        // Map bands to vertical rows in braille space.
        let rows_per_band = (bh as f32 / num_bands as f32).max(1.0);

        for band in 0..num_bands {
            let band_idx = band * fft.magnitudes.len() / num_bands;
            let mag = fft.magnitudes[band_idx.min(fft.magnitudes.len() - 1)];
            let energy = (1.0 + mag / 1000.0).ln().min(1.0);

            let spread = (energy * half_width as f32) as u16;

            let wobble = (self.frame as f32 * 0.1 + band as f32 * 0.5).sin() * 3.0;

            let y_start = (band as f32 * rows_per_band) as u16;
            let y_end = (((band + 1) as f32 * rows_per_band) as u16).min(bh);

            for y in y_start..y_end {
                let effective_spread = (spread as f32 + wobble).max(0.0) as u16;

                // Left wing: center - effective_spread .. center
                let left_start = center_x.saturating_sub(effective_spread);
                for x in left_start..center_x {
                    self.braille.set(x, y, true);
                }

                // Right wing: center .. center + effective_spread
                let right_end = (center_x + effective_spread).min(bw);
                for x in center_x..right_end {
                    self.braille.set(x, y, true);
                }
            }
        }

        // Central spine: always-on vertical column at center_x.
        for y in 0..bh {
            self.braille.set(center_x, y, true);
        }

        let cell_h = h as f32;
        self.braille.compose(grid, |_cx, cy, dot_count| {
            let hue = lerp(0.75, 0.85, cy as f32 / cell_h);
            let sat = 0.8;
            let val = lerp(0.3, 1.0, dot_count as f32 / 8.0);
            let (r, g, b) = hsv_to_rgb(hue, sat, val);
            let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));
            let bg = Rgb::new(4, 0, 6);
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
            fft_size: 1024,
            samples: vec![0.5; 1024],
        }
    }

    #[test]
    fn render_paints_cells() {
        let mut viz = Butterfly::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &fft);
        }
        let non_blank = grid
            .cells()
            .iter()
            .filter(|c| c.ch != '\u{2800}' && c.ch != ' ')
            .count();
        assert!(
            non_blank > 0,
            "loud FFT should produce non-blank braille cells, got {non_blank}"
        );
    }

    #[test]
    fn output_changes_between_frames() {
        let mut viz = Butterfly::new();
        let fft = loud_fft();

        let mut grid_a = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            viz.render_tui(&mut ctx, &fft);
        }

        // Advance frame counter significantly so wobble changes.
        for _ in 0..60 {
            let mut grid_tmp = CellGrid::new(30, 10);
            let mut ctx = TuiContext {
                grid: &mut grid_tmp,
            };
            viz.render_tui(&mut ctx, &fft);
        }

        let mut grid_b = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            viz.render_tui(&mut ctx, &fft);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(diff > 0, "different frames should produce different output");
    }

    #[test]
    fn resize_no_panic() {
        let mut viz = Butterfly::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(80, 24);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            viz.render_tui(&mut ctx, &fft);
        }
        let mut grid2 = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid2 };
            viz.render_tui(&mut ctx, &fft);
        }
    }
}
