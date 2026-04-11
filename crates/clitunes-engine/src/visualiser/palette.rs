//! Colour helpers shared across CPU visualisers.
//!
//! All visualisers work in `f32` linear-ish space for colour math and
//! convert to 8-bit sRGB on the way into a `Cell`. The conversion is a
//! clamp + quantise — the terminal does the final gamma. Keeping math in
//! floats avoids the "brown mud" that 8-bit intermediate clipping causes
//! when compositing bars, glow, and vignette.

use crate::visualiser::cell_grid::Rgb;

/// Port of the IQ / Hocevar single-instruction HSV→RGB helper:
/// `v * mix(1, clamp(abs(fract(h + k/6) * 6 - 3) - 1, 0, 1), s)`. The hue
/// origin is shifted by -1/6 relative to the textbook wheel, so h=0 is
/// roughly pink rather than red. Use [`hsv_classic`] if you want the
/// textbook convention.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h - h.floor();
    let channel = |k: f32| -> f32 {
        let x = (h + k).fract();
        let x = if x < 0.0 { x + 1.0 } else { x };
        (x * 6.0 - 3.0).abs()
    };
    let px = channel(5.0 / 6.0);
    let py = channel(3.0 / 6.0);
    let pz = channel(1.0 / 6.0);
    let cx = (px - 1.0).clamp(0.0, 1.0);
    let cy = (py - 1.0).clamp(0.0, 1.0);
    let cz = (pz - 1.0).clamp(0.0, 1.0);
    let rx = 1.0 + s * (cx - 1.0);
    let ry = 1.0 + s * (cy - 1.0);
    let rz = 1.0 + s * (cz - 1.0);
    (v * rx, v * ry, v * rz)
}

/// Textbook HSV→RGB where h=0 is red, 1/3 is green, 2/3 is blue.
pub fn hsv_classic(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = (h - h.floor()) * 6.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

pub fn rgb_from_f32(r: f32, g: f32, b: f32) -> Rgb {
    Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b))
}

pub fn f32_to_u8(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_saturation_zero_is_greyscale() {
        let (r, g, b) = hsv_to_rgb(0.4, 0.0, 0.75);
        assert!((r - 0.75).abs() < 1e-3);
        assert!((g - 0.75).abs() < 1e-3);
        assert!((b - 0.75).abs() < 1e-3);
    }

    #[test]
    fn hsv_classic_primary_colours() {
        let (r, g, b) = hsv_classic(0.0, 1.0, 1.0);
        assert!((r - 1.0).abs() < 1e-3 && g.abs() < 1e-3 && b.abs() < 1e-3);
        let (r, g, b) = hsv_classic(1.0 / 3.0, 1.0, 1.0);
        assert!(r.abs() < 1e-3 && (g - 1.0).abs() < 1e-3 && b.abs() < 1e-3);
        let (r, g, b) = hsv_classic(2.0 / 3.0, 1.0, 1.0);
        assert!(r.abs() < 1e-3 && g.abs() < 1e-3 && (b - 1.0).abs() < 1e-3);
    }

    #[test]
    fn f32_to_u8_clamps() {
        assert_eq!(f32_to_u8(-0.5), 0);
        assert_eq!(f32_to_u8(0.0), 0);
        assert_eq!(f32_to_u8(0.5), 128);
        assert_eq!(f32_to_u8(1.0), 255);
        assert_eq!(f32_to_u8(2.0), 255);
    }
}
