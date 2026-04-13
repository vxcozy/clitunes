//! Loading shimmer: a bright highlight sweeps across a dim bar.
//!
//! Used in the status line during source loading (radio connect,
//! Spotify auth, local file read).

use crate::visualiser::cell_grid::Rgb;

/// Width of the bright band in cells.
const BAND_WIDTH: u16 = 3;
/// Speed in cells per tick (~20 cells/second at 30fps).
const SPEED: u16 = 1;

/// Shimmer animation state.
#[derive(Clone, Debug, Default)]
pub struct ShimmerAnimation {
    position: u16,
    bar_width: u16,
    active: bool,
}

impl ShimmerAnimation {
    /// Start the shimmer for a bar of the given width.
    pub fn start(&mut self, bar_width: u16) {
        self.position = 0;
        self.bar_width = bar_width;
        self.active = true;
    }

    /// Stop the shimmer.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Whether the shimmer is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Advance the shimmer by one frame.
    pub fn tick(&mut self) {
        if !self.active || self.bar_width == 0 {
            return;
        }
        self.position = (self.position + SPEED) % (self.bar_width + BAND_WIDTH);
    }

    /// Get the brightness multiplier for a cell at position `x` in the bar.
    /// Returns a value in [0.0, 1.0] where 1.0 is the brightest.
    pub fn brightness_at(&self, x: u16) -> f32 {
        if !self.active {
            return 0.3; // dim baseline
        }
        let center = self.position as f32;
        let dx = (x as f32 - center).abs();
        let half_band = BAND_WIDTH as f32 / 2.0;
        if dx <= half_band {
            // Gaussian-ish falloff from center.
            let t = dx / half_band;
            1.0 - t * t * 0.7
        } else {
            0.3 // dim baseline
        }
    }

    /// Apply shimmer to an accent color for the cell at position `x`.
    pub fn color_at(&self, x: u16, accent: Rgb) -> Rgb {
        let b = self.brightness_at(x);
        Rgb::new(
            (accent.r as f32 * b).round() as u8,
            (accent.g as f32 * b).round() as u8,
            (accent.b as f32 * b).round() as u8,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shimmer_position_advances() {
        let mut s = ShimmerAnimation::default();
        s.start(20);
        let mut positions = Vec::new();
        for _ in 0..20 {
            positions.push(s.position);
            s.tick();
        }
        // Positions should increase monotonically (before wrap).
        for w in positions.windows(2) {
            assert!(w[1] >= w[0] || w[1] == 0, "should advance or wrap");
        }
    }

    #[test]
    fn shimmer_wraps_at_bar_width() {
        let mut s = ShimmerAnimation::default();
        s.start(10);
        for _ in 0..20 {
            s.tick();
        }
        assert!(s.position < 10 + BAND_WIDTH);
    }

    #[test]
    fn shimmer_gaussian_falloff() {
        let mut s = ShimmerAnimation::default();
        s.start(20);
        s.position = 10;
        let center = s.brightness_at(10);
        let near = s.brightness_at(11);
        let far = s.brightness_at(15);
        assert!(center > near, "center should be brightest");
        assert!(near > far, "brightness should fall off");
    }
}
