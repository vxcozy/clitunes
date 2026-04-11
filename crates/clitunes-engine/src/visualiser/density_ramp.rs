//! ASCII density ramps for glyph-based visualisers.
//!
//! A density ramp is a sequence of characters ordered from visually sparse
//! (space) to visually dense (`@`, `#`, `M`). Mapping a normalized intensity
//! in `[0, 1]` through a ramp gives per-cell "pixel → glyph" translation
//! — the core of the demoscene ASCII look. Combined with 24-bit truecolor
//! fg/bg, each cell carries three orthogonal axes of information: hue,
//! value, and glyph weight.
//!
//! Two flavours ship out of the box:
//! - [`DensityRamp::simple`]  — Paul Bourke's 10-level ramp (`" .:-=+*#%@"`),
//!   clean and obvious.
//! - [`DensityRamp::detailed`] — 70-level ramp for fine gradation and the
//!   classic ASCII-art-photograph look.
//!
//! Visualisers are free to build their own ramp from any string; the only
//! constraint is that the characters are ordered from dark to bright.

pub struct DensityRamp {
    chars: Vec<char>,
}

impl DensityRamp {
    pub fn new(s: &str) -> Self {
        let chars: Vec<char> = s.chars().collect();
        Self { chars }
    }

    /// 10-level ramp: `" .:-=+*#%@"`. Big obvious steps.
    pub fn simple() -> Self {
        Self::new(" .:-=+*#%@")
    }

    /// 70-level ramp (Paul Bourke): fine gradation, classic ASCII-art look.
    pub fn detailed() -> Self {
        Self::new(" .'`^\",:;Il!i><~+_-?][}{1)(|\\/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$")
    }

    /// 23-level ramp, tuned so the mid-range glyphs (`x`, `X`, `%`) read
    /// clearly at typical terminal sizes. Good sweet spot for plasma and
    /// tunnel effects.
    pub fn midrange() -> Self {
        Self::new(" .,:;+*xX%#@M&$W")
    }

    /// Pick a glyph for `intensity` in `[0, 1]`. Clamps out-of-range inputs.
    pub fn pick(&self, intensity: f32) -> char {
        if self.chars.is_empty() {
            return ' ';
        }
        let max = self.chars.len() - 1;
        let idx = (intensity.clamp(0.0, 1.0) * max as f32).round() as usize;
        self.chars[idx]
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_ramp_endpoints() {
        let r = DensityRamp::simple();
        assert_eq!(r.pick(0.0), ' ');
        assert_eq!(r.pick(1.0), '@');
    }

    #[test]
    fn pick_is_monotonic() {
        let r = DensityRamp::detailed();
        let mut prev = 0;
        for step in 0..=10 {
            let t = step as f32 / 10.0;
            let ch = r.pick(t);
            let idx = r.chars.iter().position(|&c| c == ch).unwrap();
            assert!(idx >= prev, "ramp must be non-decreasing");
            prev = idx;
        }
    }

    #[test]
    fn pick_clamps_out_of_range() {
        let r = DensityRamp::simple();
        assert_eq!(r.pick(-1.0), ' ');
        assert_eq!(r.pick(99.0), '@');
    }

    #[test]
    fn empty_ramp_returns_space() {
        let r = DensityRamp::new("");
        assert_eq!(r.pick(0.5), ' ');
    }
}
