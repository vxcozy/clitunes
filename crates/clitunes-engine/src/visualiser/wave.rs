//! Wave — braille oscilloscope visualiser. Draws a connected waveform trace
//! across the terminal by resampling the time-domain samples onto the
//! sub-pixel braille grid and connecting consecutive points with Bresenham
//! lines. An `EnergyTracker` modulates trace brightness so quiet passages
//! dim and transients flash.

use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::f32_to_u8;
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

pub struct Wave {
    braille: BrailleBuffer,
    energy: EnergyTracker,
}

impl Wave {
    pub fn new() -> Self {
        Self {
            braille: BrailleBuffer::new(1, 1),
            energy: EnergyTracker::new(0.5, 0.88, 500.0),
        }
    }

    fn ensure_buf(&mut self, w: u16, h: u16) {
        if self.braille.cell_w() != w || self.braille.cell_h() != h {
            self.braille.resize(w, h);
        }
    }
}

impl Default for Wave {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Wave {
    fn id(&self) -> VisualiserId {
        VisualiserId::Wave
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

        self.ensure_buf(w, h);
        self.braille.clear();

        let sub_w = self.braille.width() as i32;
        let sub_h = self.braille.height() as i32;
        let center_y = sub_h / 2;

        let samples = &fft.samples;
        if samples.is_empty() {
            // Nothing to draw — compose blank braille and return.
            self.braille.compose(grid, |_, _, _| (Rgb::BLACK, Rgb::BLACK));
            return;
        }

        // Resample samples across the horizontal sub-pixel width.
        let mut prev: Option<(i32, i32)> = None;
        for x in 0..sub_w {
            // Map x to a sample index.
            let si = (x as f64 * (samples.len() - 1) as f64 / (sub_w - 1).max(1) as f64) as usize;
            let sample = samples[si.min(samples.len() - 1)];

            // Map sample value to vertical sub-pixel coordinate.
            // Samples are roughly in [-1, 1] after windowing; scale to half-height.
            let amplitude = (sub_h / 2) as f32;
            let y = center_y - (sample * amplitude).round() as i32;
            let y = y.clamp(0, sub_h - 1);

            if let Some((px, py)) = prev {
                self.braille.line(px, py, x, y);
            }
            prev = Some((x, y));
        }

        // Compose braille into cell grid with cool blue/cyan colouring.
        // Energy modulates brightness: base brightness + energy boost.
        let base = 0.3_f32;
        let brightness = (base + energy * 0.7).min(1.0);

        self.braille.compose(grid, |_cx, _cy, dot_count| {
            if dot_count > 0 {
                // Brighter where more dots (trace peaks).
                let peak_boost = (dot_count as f32 / 8.0).min(1.0);
                let val = brightness * (0.6 + 0.4 * peak_boost);
                let r = f32_to_u8(val * 0.2);
                let g = f32_to_u8(val * 0.7);
                let b = f32_to_u8(val * 1.0);
                (Rgb::new(r, g, b), Rgb::BLACK)
            } else {
                (Rgb::BLACK, Rgb::BLACK)
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fft_with_samples(samples: Vec<f32>) -> FftSnapshot {
        let len = samples.len();
        FftSnapshot {
            magnitudes: vec![100.0; len / 2],
            sample_rate: 48_000,
            fft_size: len,
            samples,
        }
    }

    #[test]
    fn render_with_nonzero_fft_produces_braille() {
        let mut wave = Wave::new();
        let samples: Vec<f32> = (0..1024)
            .map(|i| (i as f32 * 0.05).sin() * 0.5)
            .collect();
        let fft = fft_with_samples(samples);
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            wave.render_tui(&mut ctx, &fft);
        }
        let braille_count = grid
            .cells()
            .iter()
            .filter(|c| c.ch != '\u{2800}' && c.ch != ' ')
            .count();
        assert!(
            braille_count > 0,
            "should have non-empty braille cells, got {braille_count}"
        );
    }

    #[test]
    fn output_changes_between_frames() {
        let mut wave = Wave::new();

        let samples_a: Vec<f32> = (0..1024)
            .map(|i| (i as f32 * 0.05).sin() * 0.5)
            .collect();
        let fft_a = fft_with_samples(samples_a);
        let mut grid_a = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            wave.render_tui(&mut ctx, &fft_a);
        }

        let samples_b: Vec<f32> = (0..1024)
            .map(|i| (i as f32 * 0.15).cos() * 0.8)
            .collect();
        let fft_b = fft_with_samples(samples_b);
        let mut grid_b = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            wave.render_tui(&mut ctx, &fft_b);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(diff > 0, "different inputs should produce different output");
    }

    #[test]
    fn resize_does_not_panic() {
        let mut wave = Wave::new();
        let fft = fft_with_samples(vec![0.0; 256]);
        for (w, h) in [(10, 5), (80, 24), (1, 1), (200, 50)] {
            let mut grid = CellGrid::new(w, h);
            let mut ctx = TuiContext { grid: &mut grid };
            wave.render_tui(&mut ctx, &fft);
        }
    }
}
