//! Deterministic calibration tone source. First-run audio before any
//! real source has been picked. Designed to be visually distinguishable
//! from real music: narrow-band spectrum + slow amplitude envelope so
//! Auralis reads as "placeholder, not song".

use clitunes_core::{PcmFormat, StereoFrame};

pub struct CalibrationTone {
    format: PcmFormat,
    phase: f32,
    time_frames: u64,
    /// Base frequency in Hz. A pleasant-ish ~ A3.
    freq: f32,
}

impl CalibrationTone {
    pub fn new(format: PcmFormat) -> Self {
        Self {
            format,
            phase: 0.0,
            time_frames: 0,
            freq: 220.0,
        }
    }

    /// Fill `out` with the next block. Writes stereo with a small L/R phase
    /// offset so the panorama isn't dead-centre (which would look like a
    /// single point in Tideline).
    pub fn fill(&mut self, out: &mut [StereoFrame]) {
        let sr = self.format.sample_rate as f32;
        for slot in out.iter_mut() {
            // Slow amplitude envelope: breathing at 0.3 Hz, gentle.
            let env_phase = (self.time_frames as f32 / sr) * std::f32::consts::TAU * 0.3;
            let envelope = 0.18 + 0.05 * env_phase.sin();

            // Base sine + a quiet fifth so the spectrum isn't a single peak.
            let base = (self.phase).sin();
            let fifth = (self.phase * 1.5).sin() * 0.35;
            let triad = (self.phase * 2.0).sin() * 0.12;
            let sample = envelope * (base + fifth + triad) / 1.47;

            slot.l = sample;
            slot.r = sample * 0.92; // tiny stereo spread via gain
            self.phase += std::f32::consts::TAU * self.freq / sr;
            if self.phase > std::f32::consts::TAU * 16.0 {
                self.phase -= std::f32::consts::TAU * 16.0;
            }
            self.time_frames = self.time_frames.wrapping_add(1);
        }
    }

    pub fn format(&self) -> PcmFormat {
        self.format
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_produces_non_silent_output() {
        let mut tone = CalibrationTone::new(PcmFormat::STUDIO);
        let mut out = vec![StereoFrame::SILENCE; 4096];
        tone.fill(&mut out);
        // Not silence.
        let energy: f32 = out.iter().map(|f| f.l * f.l + f.r * f.r).sum();
        assert!(energy > 1.0, "tone energy should be meaningful: {energy}");
    }

    #[test]
    fn fill_is_bounded() {
        let mut tone = CalibrationTone::new(PcmFormat::STUDIO);
        let mut out = vec![StereoFrame::SILENCE; 4096];
        tone.fill(&mut out);
        for f in &out {
            assert!(f.l.abs() <= 1.0, "L should not clip: {}", f.l);
            assert!(f.r.abs() <= 1.0, "R should not clip: {}", f.r);
        }
    }
}
