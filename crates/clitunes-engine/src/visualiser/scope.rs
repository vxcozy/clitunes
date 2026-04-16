//! Scope — braille Lissajous XY oscilloscope. Plots sample pairs as XY
//! coordinates to produce the rotating Lissajous figures familiar from
//! analogue oscilloscopes. A slowly oscillating phase offset between the
//! X and Y channels makes the figure evolve continuously. An
//! `EnergyTracker` modulates the phosphor-green brightness.

use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::f32_to_u8;
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

pub struct Scope {
    braille: BrailleBuffer,
    energy: EnergyTracker,
    frame: u64,
}

impl Scope {
    pub fn new() -> Self {
        Self {
            braille: BrailleBuffer::new(1, 1),
            energy: EnergyTracker::new(0.5, 0.88, 500.0),
            frame: 0,
        }
    }

    fn ensure_buf(&mut self, w: u16, h: u16) {
        if self.braille.cell_w() != w || self.braille.cell_h() != h {
            self.braille.resize(w, h);
        }
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Scope {
    fn id(&self) -> VisualiserId {
        VisualiserId::Scope
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        let energy = self.energy.update(fft);
        self.frame = self.frame.wrapping_add(1);

        let grid: &mut CellGrid = ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        self.ensure_buf(w, h);
        self.braille.clear();

        let sub_w = self.braille.width() as f32;
        let sub_h = self.braille.height() as f32;

        let samples = &fft.samples;
        if samples.len() < 2 {
            self.braille
                .compose(grid, |_, _, _| (Rgb::BLACK, Rgb::BLACK));
            return;
        }

        // Phase offset oscillates slowly for evolving Lissajous figures.
        let phase_offset = (self.frame % 512) as usize;
        let mut prev: Option<(i32, i32)> = None;
        for i in 0..samples.len() {
            let x_sample = samples[i];
            let y_sample = samples[(i + phase_offset) % samples.len()];

            // Map [-1, 1] to sub-pixel coords with a small margin.
            let px = ((x_sample + 1.0) * 0.5 * (sub_w - 1.0))
                .round()
                .clamp(0.0, sub_w - 1.0) as i32;
            let py = ((y_sample + 1.0) * 0.5 * (sub_h - 1.0))
                .round()
                .clamp(0.0, sub_h - 1.0) as i32;

            if let Some((ppx, ppy)) = prev {
                self.braille.line(ppx, ppy, px, py);
            }
            prev = Some((px, py));
        }

        // Phosphor-green CRT colour, brightness modulated by energy.
        let base = 0.35_f32;
        let brightness = (base + energy * 0.65).min(1.0);

        self.braille.compose(grid, |_cx, _cy, dot_count| {
            if dot_count > 0 {
                let peak_boost = (dot_count as f32 / 8.0).min(1.0);
                let green_val = brightness * (0.6 + 0.4 * peak_boost);
                let fg = Rgb::new(0, f32_to_u8(green_val), 0);
                (fg, Rgb::BLACK)
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
        let mut scope = Scope::new();
        let samples: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.03).sin() * 0.6).collect();
        let fft = fft_with_samples(samples);
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            scope.render_tui(&mut ctx, &fft);
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
        let mut scope = Scope::new();

        let samples: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.03).sin() * 0.6).collect();
        let fft = fft_with_samples(samples.clone());
        let mut grid_a = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            scope.render_tui(&mut ctx, &fft);
        }

        // Advance many frames so phase offset changes meaningfully.
        for _ in 0..100 {
            scope.frame = scope.frame.wrapping_add(1);
        }
        let fft_b = fft_with_samples(samples);
        let mut grid_b = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            scope.render_tui(&mut ctx, &fft_b);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(
            diff > 0,
            "different frame counts should produce different output"
        );
    }

    #[test]
    fn resize_does_not_panic() {
        let mut scope = Scope::new();
        let fft = fft_with_samples(vec![0.3; 256]);
        for (w, h) in [(10, 5), (80, 24), (1, 1), (200, 50)] {
            let mut grid = CellGrid::new(w, h);
            let mut ctx = TuiContext { grid: &mut grid };
            scope.render_tui(&mut ctx, &fft);
        }
    }
}
