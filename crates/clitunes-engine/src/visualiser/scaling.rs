//! Shared amplitude → bar-height scaling for spectrum bar visualisers.
//!
//! Raw FFT magnitudes span a huge dynamic range (≈0.001 at a quiet passage
//! to ≈1000+ on a loud transient) and are perceived logarithmically, so
//! mapping them linearly to row heights leaves bars pinned to the bottom
//! at typical listening volumes. This module provides a two-layer mapping:
//!
//! 1. **Log / dB compression.** Convert magnitudes to dBFS-ish
//!    (`20 * log10(mag)`) and clamp to a finite display range.
//! 2. **Envelope-follower AGC.** Track a smoothed peak across frames with a
//!    fast attack and slow release. Normalise bar heights against that
//!    peak so quiet passages stay visible without loud transients
//!    overshooting.

/// One-pole envelope-follower AGC over spectrum dB levels.
///
/// Attack ≈25 ms so loud transients snap to the new ceiling within a
/// single frame; release ≈1.2 s so the ceiling recovers briskly on the
/// other side of a drop without strobing on brief dips. Assumes a
/// ~30 fps render loop.
#[derive(Clone, Copy, Debug)]
pub struct SpectrumScaler {
    peak_db: f32,
}

/// Silence floor — anything below this dB value maps to zero height.
const FLOOR_DB: f32 = -60.0;

/// Useful dynamic range displayed on the bars.
const RANGE_DB: f32 = 40.0;

/// Lower clamp for the tracked peak. Keeps bars visibly small during
/// quiet passages instead of auto-gaining silence up to full height.
const MIN_PEAK_DB: f32 = -30.0;

/// Attack coefficient: `1 - exp(-dt / tau)` with dt ≈ 33 ms and tau ≈ 25 ms.
/// Snappy enough that a kick drum transient is reflected in the same frame.
const ATTACK: f32 = 0.735;

/// Release coefficient: dt ≈ 33 ms, tau ≈ 1200 ms. Half the previous
/// 2.5 s so the ceiling follows song-level dynamics instead of averaging
/// out the last 5 seconds of the track.
const RELEASE: f32 = 0.027;

impl SpectrumScaler {
    pub fn new() -> Self {
        Self {
            peak_db: MIN_PEAK_DB,
        }
    }

    /// Advance the envelope follower using the current frame's band
    /// magnitudes (already binned — one value per visual column).
    pub fn update(&mut self, bands: &[f32]) {
        let mut loudest_db = FLOOR_DB;
        for &mag in bands {
            let db = mag_to_db(mag);
            if db > loudest_db {
                loudest_db = db;
            }
        }
        let target = loudest_db.max(MIN_PEAK_DB);
        let coeff = if target > self.peak_db {
            ATTACK
        } else {
            RELEASE
        };
        self.peak_db += coeff * (target - self.peak_db);
    }

    /// Map one raw magnitude to `[0, 1]` using the current tracked peak.
    pub fn normalise(&self, mag: f32) -> f32 {
        let db = mag_to_db(mag);
        let floor = (self.peak_db - RANGE_DB).max(FLOOR_DB);
        let span = (self.peak_db - floor).max(1.0);
        ((db - floor) / span).clamp(0.0, 1.0)
    }

    #[cfg(test)]
    pub fn peak_db(&self) -> f32 {
        self.peak_db
    }
}

impl Default for SpectrumScaler {
    fn default() -> Self {
        Self::new()
    }
}

fn mag_to_db(mag: f32) -> f32 {
    20.0 * mag.max(1e-6).log10()
}

/// Envelope-follower AGC over raw PCM sample amplitudes.
///
/// Sibling to [`SpectrumScaler`] for viz that draw from `fft.samples`
/// (bipolar `[-1, 1]`) rather than spectrum magnitudes. Same attack /
/// release philosophy — fast attack so transients punch through, slow
/// release so the gain ceiling doesn't strobe on brief dips. Tracks the
/// peak absolute amplitude across recent frames and scales samples
/// against it so a 0.05-peak listening volume still fills most of the
/// available visual range instead of flatlining at one pixel (CLI-89 /
/// CLI-97).
#[derive(Clone, Copy, Debug)]
pub struct SampleScaler {
    peak: f32,
}

/// Lower bound on the tracked peak. A typical quiet listening level
/// sits around 0.03–0.05 on the peak sample; this floor keeps the
/// envelope from auto-gaining silence into full-scale spikes while
/// still letting the trace breathe at realistic volumes.
const SAMPLE_MIN_PEAK: f32 = 0.03;

/// Coefficient on the per-frame peak when updating the envelope.
/// `1 - exp(-dt / tau)` with dt ≈ 33 ms, tau ≈ 25 ms.
const SAMPLE_ATTACK: f32 = 0.735;

/// Release coefficient: dt ≈ 33 ms, tau ≈ 800 ms.
const SAMPLE_RELEASE: f32 = 0.04;

impl SampleScaler {
    pub fn new() -> Self {
        Self {
            peak: SAMPLE_MIN_PEAK,
        }
    }

    /// Advance the follower from a frame's worth of bipolar samples.
    /// Call once per render frame before `normalise`.
    pub fn update(&mut self, samples: &[f32]) {
        let frame_peak = samples
            .iter()
            .fold(0.0_f32, |acc, s| acc.max(s.abs()))
            .max(SAMPLE_MIN_PEAK);
        let coeff = if frame_peak > self.peak {
            SAMPLE_ATTACK
        } else {
            SAMPLE_RELEASE
        };
        self.peak += coeff * (frame_peak - self.peak);
    }

    /// Scale a single bipolar sample to `[-1, 1]` against the tracked
    /// peak. Preserves sign so trace shape survives.
    pub fn normalise(&self, sample: f32) -> f32 {
        (sample / self.peak).clamp(-1.0, 1.0)
    }

    #[cfg(test)]
    pub fn peak(&self) -> f32 {
        self.peak
    }
}

impl Default for SampleScaler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed the scaler to match what it would read off the provided
    /// magnitudes after the envelope has converged.
    fn converge(scaler: &mut SpectrumScaler, bands: &[f32]) {
        for _ in 0..240 {
            scaler.update(bands);
        }
    }

    #[test]
    fn fresh_scaler_starts_at_min_peak() {
        let s = SpectrumScaler::new();
        assert!((s.peak_db() - MIN_PEAK_DB).abs() < 1e-6);
    }

    #[test]
    fn typical_listening_peak_fills_majority_of_pane() {
        // peak_sample ≈ 0.05 → windowed FFT bin magnitude ≈ 0.05 * fft_size/2.
        // For fft_size=256 that's ~6.4 on the strongest bin. Broadband music
        // distributes energy across bands; model that here.
        let mut bands = vec![0.5_f32; 32];
        bands[3] = 6.4;
        bands[4] = 5.0;
        bands[5] = 3.0;

        let mut scaler = SpectrumScaler::new();
        converge(&mut scaler, &bands);

        let peak_band = scaler.normalise(bands[3]);
        assert!(
            peak_band >= 0.6,
            "loudest band should fill ≥60% of pane at peak≈0.05, got {peak_band}"
        );
    }

    #[test]
    fn quiet_passage_stays_visible_but_not_pinned() {
        // peak_sample ≈ 0.005 → peak bin magnitude ~0.64.
        let mut bands = vec![0.05_f32; 32];
        bands[3] = 0.64;
        bands[4] = 0.4;

        let mut scaler = SpectrumScaler::new();
        converge(&mut scaler, &bands);

        let peak_band = scaler.normalise(bands[3]);
        assert!(
            peak_band >= 0.10,
            "quiet loudest band should still reach ≥10%, got {peak_band}"
        );
        assert!(
            peak_band <= 1.0,
            "quiet band must not overshoot, got {peak_band}"
        );
    }

    #[test]
    fn loud_transient_clamps_at_one() {
        // peak_sample ≈ 0.3 → peak bin magnitude ~38.
        let mut bands = vec![2.0_f32; 32];
        bands[3] = 38.0;
        bands[4] = 25.0;

        let mut scaler = SpectrumScaler::new();
        converge(&mut scaler, &bands);

        let peak_band = scaler.normalise(bands[3]);
        assert!(
            (0.99..=1.0).contains(&peak_band),
            "loud transient should peak at ~1.0 with no overshoot, got {peak_band}"
        );
    }

    #[test]
    fn no_nan_on_zero_input() {
        let mut scaler = SpectrumScaler::new();
        let bands = vec![0.0_f32; 16];
        for _ in 0..50 {
            scaler.update(&bands);
        }
        let out = scaler.normalise(0.0);
        assert!(out.is_finite(), "zero input must not produce NaN");
        assert!(
            (0.0..=1.0).contains(&out),
            "zero input stays within [0,1], got {out}"
        );
    }

    #[test]
    fn attack_faster_than_release() {
        let mut rising = SpectrumScaler::new();
        rising.update(&[100.0]);
        let attack_step = rising.peak_db() - MIN_PEAK_DB;

        let mut falling = SpectrumScaler::new();
        // Pre-load peak high, then feed near-silence for one step.
        converge(&mut falling, &[100.0]);
        let start = falling.peak_db();
        falling.update(&[0.0001]);
        let release_step = start - falling.peak_db();

        assert!(
            attack_step > release_step * 10.0,
            "attack should be >10× faster than release; attack={attack_step} release={release_step}"
        );
    }

    #[test]
    fn silence_does_not_auto_gain() {
        // Long run of silence must not pull the peak below MIN_PEAK_DB
        // (which would make a whisper suddenly read as "loud").
        let mut scaler = SpectrumScaler::new();
        converge(&mut scaler, &[100.0; 8]);
        for _ in 0..600 {
            scaler.update(&[0.0; 8]);
        }
        assert!(
            scaler.peak_db() >= MIN_PEAK_DB - 1e-3,
            "silence must not push peak below MIN_PEAK_DB floor, got {}",
            scaler.peak_db()
        );
    }

    fn converge_samples(scaler: &mut SampleScaler, peak: f32) {
        let frame = vec![peak; 256];
        for _ in 0..240 {
            scaler.update(&frame);
        }
    }

    #[test]
    fn sample_scaler_quiet_signal_fills_majority_of_range() {
        // Typical quiet listening ≈ 0.05 peak sample. After the envelope
        // converges, that sample should scale to ≥0.8 so the trace
        // covers a healthy slice of the pane instead of one pixel.
        let mut scaler = SampleScaler::new();
        converge_samples(&mut scaler, 0.05);
        let out = scaler.normalise(0.05);
        assert!(
            out >= 0.8,
            "quiet 0.05 sample should scale ≥0.8 after AGC, got {out}"
        );
    }

    #[test]
    fn sample_scaler_preserves_sign() {
        let mut scaler = SampleScaler::new();
        converge_samples(&mut scaler, 0.3);
        let pos = scaler.normalise(0.15);
        let neg = scaler.normalise(-0.15);
        assert!(pos > 0.0 && neg < 0.0, "sign must survive: {pos}, {neg}");
        assert!(
            (pos + neg).abs() < 1e-4,
            "magnitudes should mirror: {pos} vs {neg}"
        );
    }

    #[test]
    fn sample_scaler_loud_transient_clamps_without_overshoot() {
        let mut scaler = SampleScaler::new();
        converge_samples(&mut scaler, 0.3);
        let out = scaler.normalise(0.3);
        assert!(
            (0.8..=1.0).contains(&out),
            "loud peak should reach ≥0.8 and never overshoot, got {out}"
        );
    }

    #[test]
    fn sample_scaler_silence_does_not_auto_gain() {
        let mut scaler = SampleScaler::new();
        converge_samples(&mut scaler, 0.5);
        for _ in 0..600 {
            scaler.update(&[0.0; 256]);
        }
        assert!(
            scaler.peak() >= SAMPLE_MIN_PEAK - 1e-6,
            "silence must not drop peak below floor, got {}",
            scaler.peak()
        );
    }

    #[test]
    fn sample_scaler_attack_faster_than_release() {
        let mut rising = SampleScaler::new();
        rising.update(&[0.5; 64]);
        let attack_step = rising.peak() - SAMPLE_MIN_PEAK;

        let mut falling = SampleScaler::new();
        converge_samples(&mut falling, 0.5);
        let start = falling.peak();
        falling.update(&[0.0; 64]);
        let release_step = start - falling.peak();

        assert!(
            attack_step > release_step * 3.0,
            "attack should outpace release; attack={attack_step} release={release_step}"
        );
    }
}
