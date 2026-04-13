//! Error pulse: status line background flashes danger color twice.
//!
//! Triggered on SourceError and connection failures. Two quick pulses
//! over 20 frames, then holds error message for 3 seconds.

use crate::visualiser::cell_grid::Rgb;

/// Two Gaussian peaks at frames 5 and 15 span 20 frames total
/// (~667 ms at 30 fps). Enough for two distinct flashes.
const PULSE_FRAMES: u16 = 20;
/// After the pulse, hold the error message on screen for 3 seconds
/// (90 frames at 30 fps) so the user can read it.
const HOLD_FRAMES: u16 = 90;

/// Error pulse state.
#[derive(Clone, Debug, Default)]
pub struct ErrorPulse {
    frame: u16,
    active: bool,
    message: String,
}

impl ErrorPulse {
    /// Trigger an error pulse with a message.
    pub fn trigger(&mut self, message: String) {
        self.frame = 0;
        self.active = true;
        self.message = message;
    }

    /// Whether the pulse is active (pulsing or holding message).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The error message, if in the hold phase.
    pub fn message(&self) -> Option<&str> {
        if self.active && self.frame >= PULSE_FRAMES {
            Some(&self.message)
        } else {
            None
        }
    }

    /// Advance by one frame.
    pub fn tick(&mut self) {
        if !self.active {
            return;
        }
        self.frame += 1;
        if self.frame >= PULSE_FRAMES + HOLD_FRAMES {
            self.active = false;
        }
    }

    /// Get the danger color intensity for the current frame (0.0–1.0).
    /// Two peaks at frames ~5 and ~15.
    pub fn intensity(&self) -> f32 {
        if !self.active || self.frame >= PULSE_FRAMES {
            return 0.0;
        }
        let f = self.frame as f32;
        // Two Gaussian-ish peaks at frame 5 and 15.
        let peak1 = (-((f - 5.0) * (f - 5.0)) / 8.0).exp();
        let peak2 = (-((f - 15.0) * (f - 15.0)) / 8.0).exp();
        (peak1 + peak2).min(1.0)
    }

    /// Blend the danger color with the normal bg based on pulse intensity.
    pub fn blend_bg(&self, normal_bg: Rgb, danger: Rgb) -> Rgb {
        let t = self.intensity();
        if t < 0.01 {
            return normal_bg;
        }
        Rgb::new(
            ((normal_bg.r as f32 * (1.0 - t) + danger.r as f32 * t).round()) as u8,
            ((normal_bg.g as f32 * (1.0 - t) + danger.g as f32 * t).round()) as u8,
            ((normal_bg.b as f32 * (1.0 - t) + danger.b as f32 * t).round()) as u8,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_timing() {
        let mut p = ErrorPulse::default();
        p.trigger("test error".into());
        let mut intensities = Vec::new();
        for _ in 0..PULSE_FRAMES {
            intensities.push(p.intensity());
            p.tick();
        }
        // Should have 2 peaks.
        let peaks: Vec<usize> = intensities
            .windows(3)
            .enumerate()
            .filter(|(_, w)| w[1] > w[0] && w[1] > w[2])
            .map(|(i, _)| i + 1)
            .collect();
        assert_eq!(peaks.len(), 2, "expected exactly 2 peaks, got {:?}", peaks);
    }

    #[test]
    fn pulse_returns_to_normal() {
        let mut p = ErrorPulse::default();
        p.trigger("err".into());
        for _ in 0..(PULSE_FRAMES + HOLD_FRAMES) {
            p.tick();
        }
        assert!(!p.is_active());
    }

    #[test]
    fn message_shown_after_pulse() {
        let mut p = ErrorPulse::default();
        p.trigger("connection lost".into());
        for _ in 0..PULSE_FRAMES {
            p.tick();
        }
        assert_eq!(p.message(), Some("connection lost"));
    }
}
