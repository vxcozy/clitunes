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
/// Attack ≈50 ms so loud transients pop immediately; release ≈2.5 s so
/// the ceiling doesn't dive back down during brief quiet spots.
/// Assumes a ~30 fps render loop.
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

/// Attack coefficient: `1 - exp(-dt / tau)` with dt ≈ 33 ms and tau ≈ 50 ms.
const ATTACK: f32 = 0.48;

/// Release coefficient: dt ≈ 33 ms, tau ≈ 2500 ms.
const RELEASE: f32 = 0.013;

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
}
