//! Heartbeat — braille ECG-style scrolling trace. A history buffer stores
//! Y values across the sub-pixel width; each frame the history shifts left
//! and a new value derived from the current samples is pushed on the right.
//! The sample transform `sample * sample.abs()` produces the sharp spikes
//! characteristic of an ECG trace. A dashed baseline runs through the
//! centre. Green-on-black hospital monitor aesthetic.

use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::f32_to_u8;
use crate::visualiser::scaling::SampleScaler;
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

pub struct Heartbeat {
    braille: BrailleBuffer,
    energy: EnergyTracker,
    /// AGC on raw sample amplitudes. Raw samples at ~0.05 peak listening
    /// levels multiplied by `sample * |sample|` give ~0.0025 — the trace
    /// flatlines at one pixel without this (CLI-89 / CLI-97 pattern).
    sample_scaler: SampleScaler,
    history: Vec<f32>,
    last_w: u16,
    last_h: u16,
}

impl Heartbeat {
    pub fn new() -> Self {
        Self {
            braille: BrailleBuffer::new(1, 1),
            // Release tau ~115 ms (was ~258 ms): ECG amplitude tracks the
            // beat envelope instead of smearing across multiple pulses.
            energy: EnergyTracker::new(0.5, 0.75, 500.0),
            sample_scaler: SampleScaler::new(),
            history: Vec::new(),
            last_w: 0,
            last_h: 0,
        }
    }

    fn ensure_buf(&mut self, w: u16, h: u16) {
        if self.last_w != w || self.last_h != h {
            self.braille.resize(w, h);
            self.last_w = w;
            self.last_h = h;
            // Reset history to match new sub-pixel width.
            let sub_w = self.braille.width() as usize;
            self.history.clear();
            self.history.resize(sub_w, 0.0);
        }
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Heartbeat {
    fn id(&self) -> VisualiserId {
        VisualiserId::Heartbeat
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

        let sub_w = self.braille.width() as usize;
        let sub_h = self.braille.height() as i32;
        let center_y = sub_h / 2;

        // Compute the frame's signed ECG spike on AGC-normalised
        // samples. Raw amplitudes at typical listening peaks (~0.05)
        // flatlined the trace — `sample * |sample|` compressed them
        // to ~0.0025, a one-pixel Y sweep (CLI-89 pattern). The
        // SampleScaler lifts samples into [-1, 1] before the spike
        // shape preserves ECG dynamics; energy modulates overall
        // amplitude so loud beats still punch harder than quiet ones.
        self.sample_scaler.update(&fft.samples);
        let new_val = if fft.samples.is_empty() {
            0.0
        } else {
            let sum: f32 = fft
                .samples
                .iter()
                .map(|&s| {
                    let n = self.sample_scaler.normalise(s);
                    n * n.abs()
                })
                .sum();
            let avg = sum / fft.samples.len() as f32;
            let amplitude_mod = 0.5 + energy * 2.0;
            (avg * amplitude_mod).clamp(-1.0, 1.0)
        };

        // Scroll history left, push new value.
        let len = self.history.len();
        if len > 1 {
            self.history.copy_within(1.., 0);
            self.history[len - 1] = new_val;
        } else if len == 1 {
            self.history[0] = new_val;
        }

        // Draw the trace using line() between consecutive points.
        let half_h = (sub_h / 2) as f32;
        let mut prev: Option<(i32, i32)> = None;
        for (x, &val) in self.history.iter().enumerate() {
            let y = center_y - (val * half_h).round() as i32;
            let y = y.clamp(0, sub_h - 1);

            if let Some((px, py)) = prev {
                self.braille.line(px, py, x as i32, y);
            }
            prev = Some((x as i32, y));
        }

        // Draw dashed baseline at center: every other x, set dot at center_y.
        for x in (0..sub_w).step_by(2) {
            self.braille
                .set(x as u16, center_y.clamp(0, sub_h - 1) as u16, true);
        }

        // Compose into grid with green-on-black hospital monitor colour.
        let base = 0.3_f32;
        let brightness = (base + energy * 0.7).min(1.0);

        // Muted hospital-monitor green so empty cells carry the palette
        // rather than leave a raw black pane.
        let gutter = Rgb::new(0, 6, 2);
        self.braille.compose(grid, |_cx, _cy, dot_count| {
            if dot_count > 0 {
                let peak_boost = (dot_count as f32 / 8.0).min(1.0);
                let green_val = brightness * (0.5 + 0.5 * peak_boost);
                let fg = Rgb::new(0, f32_to_u8(green_val), 0);
                (fg, gutter)
            } else {
                (gutter, gutter)
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
        let mut hb = Heartbeat::new();
        let samples: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.05).sin() * 0.7).collect();
        let fft = fft_with_samples(samples);
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            hb.render_tui(&mut ctx, &fft);
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
        let mut hb = Heartbeat::new();

        // Feed several frames of silence to fill the history with zeros.
        let silent = fft_with_samples(vec![0.0; 1024]);
        let mut grid = CellGrid::new(30, 10);
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            hb.render_tui(&mut ctx, &silent);
        }
        let mut grid_a = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            hb.render_tui(&mut ctx, &silent);
        }

        // Now feed loud, asymmetric samples to push non-zero values
        // into the history. The ECG transform (s * |s|) preserves sign,
        // so a positive-biased signal will produce different trace than zeros.
        let loud: Vec<f32> = (0..1024).map(|_| 0.8).collect();
        let fft_loud = fft_with_samples(loud);
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            hb.render_tui(&mut ctx, &fft_loud);
        }
        let mut grid_b = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            hb.render_tui(&mut ctx, &fft_loud);
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
    fn gutter_is_tinted_not_black() {
        // Even with silent input the pane must carry the hospital-green
        // gutter palette instead of collapsing to raw black.
        let mut hb = Heartbeat::new();
        let fft = fft_with_samples(vec![0.0; 1024]);
        let mut grid = CellGrid::new(120, 40);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            hb.render_tui(&mut ctx, &fft);
        }

        let edge_rows = [0u16, 39];
        let edge_cols = [0u16, 119];

        for row in edge_rows {
            let any_tinted = (0..120u16).any(|x| {
                let cell = grid.cells()[(row as usize) * 120 + x as usize];
                cell.bg != Rgb::BLACK || cell.fg != Rgb::BLACK
            });
            assert!(any_tinted, "row {row} must have palette-tinted cells");
        }
        for col in edge_cols {
            let any_tinted = (0..40u16).any(|y| {
                let cell = grid.cells()[(y as usize) * 120 + col as usize];
                cell.bg != Rgb::BLACK || cell.fg != Rgb::BLACK
            });
            assert!(any_tinted, "col {col} must have palette-tinted cells");
        }
    }

    #[test]
    fn resize_does_not_panic() {
        let mut hb = Heartbeat::new();
        let fft = fft_with_samples(vec![0.3; 256]);
        for (w, h) in [(10, 5), (80, 24), (1, 1), (200, 50)] {
            let mut grid = CellGrid::new(w, h);
            let mut ctx = TuiContext { grid: &mut grid };
            hb.render_tui(&mut ctx, &fft);
        }
    }

    /// Scan the history buffer and return the maximum excursion from
    /// centre as a fraction of `half_h`. The rendered trace converts
    /// history values in `[-1, 1]` via `center_y - val * half_h`, so
    /// |val| = 1.0 corresponds to an excursion spanning 100% of the
    /// half-pane (i.e. 50% of the full pane height in rows).
    fn history_excursion(hb: &Heartbeat) -> f32 {
        hb.history.iter().fold(0.0_f32, |acc, v| acc.max(v.abs()))
    }

    #[test]
    fn quiet_listening_volume_lifts_trace_off_baseline() {
        // Regression for CLI-97: at 0.05 sample peak the old direct
        // map (`avg of s * |s|` without AGC) compressed the signal to
        // ~0.0025 and pinned the trace to one pixel above the
        // baseline. After SampleScaler AGC the signal reaches a
        // meaningful fraction of the pane height.
        let mut hb = Heartbeat::new();
        // DC-biased samples at +0.05 so each frame has a real non-zero
        // average (a sine over many cycles averages to ~0).
        let fft = fft_with_samples(vec![0.05_f32; 1024]);
        let mut grid = CellGrid::new(40, 12);
        for _ in 0..60 {
            let mut ctx = TuiContext { grid: &mut grid };
            hb.render_tui(&mut ctx, &fft);
        }
        let excursion = history_excursion(&hb);
        // The trace maps |val|=1 to 50% of pane height (half_h). So
        // ≥20% pane height = ≥0.4 excursion.
        assert!(
            excursion >= 0.4,
            "quiet-volume trace must lift ≥20% of pane height, got excursion {excursion}"
        );
    }

    #[test]
    fn loud_volume_saturates_without_overshoot() {
        let mut hb = Heartbeat::new();
        let fft = fft_with_samples(vec![0.3_f32; 1024]);
        let mut grid = CellGrid::new(40, 12);
        for _ in 0..60 {
            let mut ctx = TuiContext { grid: &mut grid };
            hb.render_tui(&mut ctx, &fft);
        }
        let excursion = history_excursion(&hb);
        // ≥80% of pane height = ≥1.6 of half_h? No — |val|=1 is 50%
        // of pane, which is the ceiling, so ≥0.8 here means the trace
        // reaches ≥40% of pane height, well above the ≥80%-of-
        // half-span intent. Bound from above at 1.0 to check clamp.
        assert!(
            excursion >= 0.8,
            "loud-volume trace must peak at ≥80% of half-span, got {excursion}"
        );
        assert!(
            excursion <= 1.0,
            "trace must never overshoot clamp, got {excursion}"
        );
    }
}
