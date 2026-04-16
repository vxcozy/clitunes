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
    pub samples: Vec<f32>,
}

impl FftSnapshot {
    pub fn new(magnitudes: Vec<f32>, sample_rate: u32, fft_size: usize) -> Self {
        Self {
            magnitudes,
            sample_rate,
            fft_size,
            samples: vec![],
        }
    }
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
        let samples = self.scratch_in.clone();
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
            samples,
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
        let samples = self.scratch_in.clone();
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
            samples,
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

    #[test]
    fn snapshot_contains_samples_of_correct_length() {
        let mut tap = FftTap::new(1024);
        let frames = vec![StereoFrame::SILENCE; 1024];
        let snap = tap.snapshot_from(&frames, PcmFormat::STUDIO.sample_rate);
        assert_eq!(snap.samples.len(), 1024);
    }

    #[test]
    fn samples_reflect_input_waveform() {
        let mut tap = FftTap::new(1024);
        let sr = 48_000_f32;
        let freq = 440.0_f32;
        let frames: Vec<_> = (0..1024)
            .map(|i| {
                let s = (std::f32::consts::TAU * freq * i as f32 / sr).sin() * 0.8;
                StereoFrame { l: s, r: s }
            })
            .collect();
        let snap = tap.snapshot_from(&frames, sr as u32);
        assert_eq!(snap.samples.len(), 1024);
        let peak = snap.samples.iter().copied().fold(0.0_f32, f32::max);
        assert!(peak > 0.1, "sine input should produce non-trivial samples, peak={peak}");
    }

    #[test]
    fn silent_input_gives_near_zero_samples() {
        let mut tap = FftTap::new(1024);
        let frames = vec![StereoFrame::SILENCE; 1024];
        let snap = tap.snapshot_from(&frames, PcmFormat::STUDIO.sample_rate);
        let max_abs: f32 = snap.samples.iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(max_abs < 1e-6, "silence should give near-zero samples, got {max_abs}");
    }

    #[test]
    fn new_constructor_defaults_empty_samples() {
        let snap = FftSnapshot::new(vec![1.0, 2.0], 44_100, 4);
        assert!(snap.samples.is_empty());
        assert_eq!(snap.magnitudes, vec![1.0, 2.0]);
        assert_eq!(snap.sample_rate, 44_100);
        assert_eq!(snap.fft_size, 4);
    }
}
