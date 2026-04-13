//! Breathing animation: subtle brightness oscillation during pause.
//!
//! Brightness oscillates between 55% and 65% on a 4-second sine
//! cycle (120 frames at 30fps). Makes the paused visualiser feel
//! alive without being distracting.

/// Breathing animation state.
#[derive(Clone, Debug, Default)]
pub struct BreathingAnimation {
    frame: u64,
    active: bool,
}

/// Full sine cycle in frames (120 frames = 4 s at 30 fps). Matches a
/// relaxed human breathing cadence — slow enough to feel organic, fast
/// enough to notice the visualiser is still alive.
const CYCLE_FRAMES: f32 = 120.0;
/// Center brightness (60 %). Dim enough to clearly signal "paused",
/// bright enough that the visualiser artwork stays readable.
const CENTER: f32 = 0.60;
/// Amplitude of the sine oscillation (±5 %). Just enough motion to
/// avoid looking frozen without being distracting.
const AMPLITUDE: f32 = 0.05;

impl BreathingAnimation {
    /// Start the breathing animation.
    pub fn start(&mut self) {
        self.active = true;
        self.frame = 0;
    }

    /// Stop the breathing animation.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Whether the animation is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Advance by one frame.
    pub fn tick(&mut self) {
        if self.active {
            self.frame += 1;
        }
    }

    /// Get the current brightness multiplier (0.55–0.65).
    pub fn brightness(&self) -> f32 {
        if !self.active {
            return 1.0;
        }
        let phase = (self.frame as f32 / CYCLE_FRAMES) * std::f32::consts::TAU;
        CENTER + AMPLITUDE * phase.sin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breathing_sine_cycle() {
        let mut b = BreathingAnimation::default();
        b.start();
        let mut values = Vec::new();
        for _ in 0..120 {
            values.push(b.brightness());
            b.tick();
        }
        let min = values.iter().cloned().fold(f32::MAX, f32::min);
        let max = values.iter().cloned().fold(f32::MIN, f32::max);
        // Should oscillate between ~0.55 and ~0.65.
        assert!(min >= 0.54, "min brightness {min} too low");
        assert!(max <= 0.66, "max brightness {max} too high");
        assert!((max - min) > 0.08, "amplitude too small: {}", max - min);
    }

    #[test]
    fn breathing_amplitude_bounds() {
        let mut b = BreathingAnimation::default();
        b.start();
        for _ in 0..1000 {
            let v = b.brightness();
            assert!(v >= 0.54, "brightness {v} below floor");
            assert!(v <= 0.66, "brightness {v} above ceiling");
            b.tick();
        }
    }

    #[test]
    fn inactive_returns_full_brightness() {
        let b = BreathingAnimation::default();
        assert!((b.brightness() - 1.0).abs() < 0.001);
    }
}
