//! The easing curves, one table for every front end. Each curve has its own
//! name here; a format maps the names it has always offered onto them (styles
//! and scenes both say `ease_in_out`, and have never meant the same curve).

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Linear,
    EaseIn,
    EaseOut,
    /// Two parabolas joined at the middle: the styles' `ease_in_out`.
    EaseInOut,
    /// 3x² − 2x³ (smoothstep): the scenes' `ease_in_out`.
    Smooth,
    Back,
    /// The power curves x³ and 1 − (1 − x)³ ("easeInCubic" / "easeOutCubic" in the
    /// usual easing tables). Not the CSS ease-in / ease-out keywords (cubic-bezier).
    EaseInCubic,
    EaseOutCubic,
}

/// `x` (clamped to 0..1) through the curve.
pub fn ease(c: Curve, x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    match c {
        Curve::Linear => x,
        Curve::EaseIn => x * x,
        Curve::EaseOut => 1.0 - (1.0 - x) * (1.0 - x),
        Curve::EaseInOut => {
            if x < 0.5 {
                2.0 * x * x
            } else {
                1.0 - (-2.0 * x + 2.0).powi(2) / 2.0
            }
        }
        Curve::Smooth => x * x * (3.0 - 2.0 * x),
        Curve::Back => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
        }
        Curve::EaseInCubic => x * x * x,
        Curve::EaseOutCubic => 1.0 - (1.0 - x).powi(3),
    }
}

#[cfg(test)]
mod tests {
    use super::{Curve, ease};

    /// The two curves both formats call `ease_in_out` stay two curves.
    #[test]
    fn the_two_ease_in_out_curves_differ_and_meet_at_the_ends_and_the_middle() {
        for x in [0.0, 0.5, 1.0] {
            assert_eq!(ease(Curve::EaseInOut, x), ease(Curve::Smooth, x));
        }
        assert_eq!(ease(Curve::EaseInOut, 0.25), 0.125);
        assert_eq!(ease(Curve::Smooth, 0.25), 0.15625);
    }
}
