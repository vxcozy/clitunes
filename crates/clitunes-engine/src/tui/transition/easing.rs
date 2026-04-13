//! Easing curves for transition interpolation.
//!
//! Each function maps `t ∈ [0, 1]` to an eased value in `[0, 1]`.
//! The caller is responsible for clamping `t` before calling.

/// Linear — constant velocity, no easing.
#[inline]
pub fn linear(t: f32) -> f32 {
    t
}

/// Cubic ease-out — fast start, gentle landing. Default for opens/appears.
#[inline]
pub fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// Cubic ease-in — gentle start, fast exit. Default for closes/dismissals.
#[inline]
pub fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}

/// Cubic ease-in-out — symmetric S-curve for balanced transitions.
#[inline]
pub fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - u * u * u / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type NamedEasing = (&'static str, fn(f32) -> f32);

    const ALL_EASINGS: &[NamedEasing] = &[
        ("linear", linear),
        ("ease_out_cubic", ease_out_cubic),
        ("ease_in_cubic", ease_in_cubic),
        ("ease_in_out_cubic", ease_in_out_cubic),
    ];

    /// All easing functions must satisfy f(0) = 0 and f(1) = 1.
    #[test]
    fn boundary_conditions() {
        for (name, f) in ALL_EASINGS {
            assert!(
                (f(0.0)).abs() < 1e-6,
                "{name}(0.0) should be 0.0, got {}",
                f(0.0)
            );
            assert!(
                (f(1.0) - 1.0).abs() < 1e-6,
                "{name}(1.0) should be 1.0, got {}",
                f(1.0)
            );
        }
    }

    /// All easing functions must be monotonically non-decreasing.
    #[test]
    fn monotonic() {
        for (name, f) in ALL_EASINGS {
            let mut prev = 0.0f32;
            for i in 1..=100 {
                let t = i as f32 / 100.0;
                let v = f(t);
                assert!(
                    v >= prev - 1e-6,
                    "{name}({t}) = {v} < prev {prev} — not monotonic"
                );
                prev = v;
            }
        }
    }

    /// ease_out_cubic is front-loaded (above the diagonal at midpoint).
    #[test]
    fn ease_out_shape() {
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    /// ease_in_cubic is back-loaded (below the diagonal at midpoint).
    #[test]
    fn ease_in_shape() {
        assert!(ease_in_cubic(0.5) < 0.5);
    }

    /// ease_in_out_cubic passes through (0.5, 0.5).
    #[test]
    fn ease_in_out_symmetric() {
        assert!((ease_in_out_cubic(0.5) - 0.5).abs() < 1e-6);
    }
}
