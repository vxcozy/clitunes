//! FFT tap that snapshots recent PCM from a ring and computes a magnitude
//! spectrum for visualisers to consume. Uses realfft for a real-to-complex
//! FFT with Hann windowing.

use clitunes_core::StereoFrame;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

use super::PcmRingReader;

pub struct FftTap {
    planner_fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    fft_size: usize,
    scratch_in: Vec<f32>,
    scratch_out: Vec<realfft::num_complex::Complex<f32>>,
}

#[derive(Clone, Debug)]
pub struct FftSnapshot {
    pub magnitudes: Vec<f32>,
    pub sample_rate: u32,
    pub fft_size: usize,
}

impl FftTap {
    pub fn new(fft_size: usize) -> Self {
        assert!(
            fft_size.is_power_of_two(),
            "fft_size must be a power of two"
        );
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let scratch_in = vec![0.0; fft_size];
        let scratch_out = fft.make_output_vec();
        // Hann window coefficients.
        let window = (0..fft_size)
            .map(|i| {
                let x = (i as f32) / (fft_size as f32 - 1.0);
                0.5 * (1.0 - (std::f32::consts::TAU * x).cos())
            })
            .collect();
        Self {
            planner_fft: fft,
            window,
            fft_size,
            scratch_in,
            scratch_out,
        }
    }

    /// Compute a magnitude spectrum from the most recent frames in `reader`.
    /// Returns silence-filled spectrum if the ring doesn't have enough data.
    pub fn snapshot(&mut self, reader: &PcmRingReader, sample_rate: u32) -> FftSnapshot {
        let frames = reader.snapshot(self.fft_size);
        let have = frames.len();
        for (i, slot) in self.scratch_in.iter_mut().enumerate() {
            let f = if i + have >= self.fft_size {
                frames[i + have - self.fft_size].mono()
            } else {
                0.0
            };
            *slot = f * self.window[i];
        }
        // realfft writes into scratch_out in place.
        let _ = self
            .planner_fft
            .process(&mut self.scratch_in, &mut self.scratch_out);

        let magnitudes = self
            .scratch_out
            .iter()
            .map(|c| (c.re * c.re + c.im * c.im).sqrt())
            .collect();

        FftSnapshot {
            magnitudes,
            sample_rate,
            fft_size: self.fft_size,
        }
    }

    /// Utility for tests and synchronous visualiser code that passes frames
    /// directly without going through a ring.
    pub fn snapshot_from(&mut self, frames: &[StereoFrame], sample_rate: u32) -> FftSnapshot {
        let have = frames.len().min(self.fft_size);
        for (i, slot) in self.scratch_in.iter_mut().enumerate() {
            let f = if i + have >= self.fft_size {
                frames[i + have - self.fft_size].mono()
            } else {
                0.0
            };
            *slot = f * self.window[i];
        }
        let _ = self
            .planner_fft
            .process(&mut self.scratch_in, &mut self.scratch_out);
        FftSnapshot {
            magnitudes: self
                .scratch_out
                .iter()
                .map(|c| (c.re * c.re + c.im * c.im).sqrt())
                .collect(),
            sample_rate,
            fft_size: self.fft_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clitunes_core::PcmFormat;

    #[test]
    fn silent_input_gives_silent_spectrum() {
        let mut tap = FftTap::new(1024);
        let frames = vec![StereoFrame::SILENCE; 1024];
        let snap = tap.snapshot_from(&frames, PcmFormat::STUDIO.sample_rate);
        let total: f32 = snap.magnitudes.iter().sum();
        assert!(
            total < 1e-3,
            "silence should give near-zero spectrum, got {total}"
        );
    }

    #[test]
    fn sine_wave_produces_peak_near_expected_bin() {
        let mut tap = FftTap::new(1024);
        let sr = 48_000_f32;
        let freq = 1000.0_f32;
        let frames: Vec<_> = (0..1024)
            .map(|i| {
                let s = (std::f32::consts::TAU * freq * i as f32 / sr).sin() * 0.5;
                StereoFrame { l: s, r: s }
            })
            .collect();
        let snap = tap.snapshot_from(&frames, sr as u32);
        let (peak_bin, _) = snap
            .magnitudes
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        let peak_freq = peak_bin as f32 * sr / snap.fft_size as f32;
        let err = (peak_freq - freq).abs();
        assert!(
            err < 100.0,
            "peak should be near 1000Hz, got {peak_freq} Hz"
        );
    }
}
