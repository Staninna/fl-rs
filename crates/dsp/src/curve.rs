//! The curve FL's envelopes draw through their points: an automation
//! clip's, and the one a Fruity WaveShaper bakes into its table.
//!
//! Each [`Point`] ends a segment and says how the segment gets from the
//! point before to itself: one of 13 modes and a tension. [`Curve`] is those
//! segments as a function of `x`. `docs/automation-234.md` is the source for
//! the formulas, and `docs/plugins/fruity-waveshaper.md` has the same class
//! in another DLL.
//!
//! This is a copy of the curve in the engine's `automation`, which was
//! there first and reads its points from `flp`. The engine should move onto
//! this one, so that the formulas live in one place.

use std::f64::consts::{FRAC_PI_2, TAU};

/// One point of an envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// How far the point is after the one before it. An automation clip
    /// counts in beats, and a WaveShaper in parts of its whole curve.
    pub x: f64,
    /// The value at the point, 0 to 1.
    pub y: f64,
    /// How far the segment that ends here bends, -1 to 1.
    pub tension: f32,
    /// The shape of the segment that ends here, 0 to 12.
    pub mode: u8,
}

impl Point {
    /// A point that a straight line leads to.
    pub fn new(x: f64, y: f64) -> Self {
        Point {
            x,
            y,
            tension: 0.0,
            mode: 0,
        }
    }
}

/// The points of an envelope, as a value for every `x`.
///
/// A segment runs from the point before up to, and not including, the point
/// that ends it, which is where the next one takes over. So a hold segment
/// jumps at its end point.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Curve {
    segments: Vec<Segment>,
    /// The last point's value, and 0 for a curve without points.
    last: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    /// Where the segment starts and ends, from the start of the envelope.
    start: f64,
    end: f64,
    /// The values of the point before and of the point that ends it.
    from: f64,
    to: f64,
    shape: Shape,
}

impl Curve {
    /// The curve through these points. A point's mode and tension shape
    /// the segment that ends at it.
    ///
    /// FL's loader reads a negative `x` as 0 and a `y` that is not a number
    /// as the `y` before it, and so does this.
    pub fn new(points: &[Point]) -> Self {
        let mut clean: Vec<Point> = Vec::with_capacity(points.len());
        for point in points {
            let before = clean.last().map_or(0.0, |p| p.y);
            clean.push(Point {
                x: if point.x > 0.0 { point.x } else { 0.0 },
                y: if point.y.is_finite() { point.y } else { before },
                ..*point
            });
        }
        let mut at = clean.first().map_or(0.0, |p| p.x);
        let mut segments = Vec::with_capacity(clean.len().saturating_sub(1));
        for (i, point) in clean.iter().enumerate().skip(1) {
            segments.push(Segment {
                start: at,
                end: at + point.x,
                from: clean[i - 1].y,
                to: point.y,
                shape: Shape::new(&clean, i),
            });
            at += point.x;
        }
        Curve {
            segments,
            last: clean.last().map_or(0.0, |p| p.y),
        }
    }

    /// The curve's value at `x`: the first point's value before the first
    /// point, and the last point's from the last point on. This allocates
    /// nothing.
    pub fn value_at(&self, x: f64) -> f64 {
        let over = self.segments.partition_point(|s| s.end <= x);
        let Some(segment) = self.segments.get(over) else {
            return self.last;
        };
        // A segment that ends after `x` and has started is longer than
        // nothing.
        let t = (x - segment.start) / (segment.end - segment.start);
        if t > 0.0 {
            segment.from + segment.shape.at(t) * (segment.to - segment.from)
        } else {
            segment.from
        }
    }

    /// How far along its rise the segment that ends at point `end` is when
    /// `t` of its length is over: 0 at its start and 1 at its end, whatever
    /// the two values are. 0 for a point that ends no segment, which the
    /// first is.
    ///
    /// FL's envelopes that loop run segment by segment: a loop jumps from
    /// the point that ends it into the segment after the point that starts
    /// it, and takes that segment's shape from the value it has to the
    /// value the segment ends on. This allocates nothing.
    pub fn progress(&self, end: usize, t: f64) -> f64 {
        let Some(segment) = end.checked_sub(1).and_then(|i| self.segments.get(i)) else {
            return 0.0;
        };
        if t >= 1.0 {
            1.0
        } else if t > 0.0 {
            segment.shape.at(t)
        } else {
            0.0
        }
    }
}

/// ln 1000001, the exponent FL's curves are built on.
const L: f64 = 13.815_511_557_963_774;
/// 2^-24, FL's "too small to divide by".
const EPSILON: f64 = 5.960_464_477_539_063e-8;

/// How a segment gets from 0 at its start to 1 at its end, with what FL
/// works out per point after loading (`FUN_00b9ba60`) worked out already.
/// The names are FL's.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    /// A straight line. FL draws one for a tension of exactly 0 in every
    /// mode that only bends, and for a smooth segment between two equal
    /// values.
    Line,
    /// Stays at the start. Mode 2, and any mode past 12.
    Hold,
    /// Modes 0 and 1. `k` is the tension's size times [`L`].
    SingleCurve {
        k: f64,
        up: bool,
    },
    DoubleCurve {
        k: f64,
        up: bool,
    },
    /// Mode 3: `n` treads, and `side` is 1 for a tension from 0 up and -1
    /// below.
    Stairs {
        n: f64,
        side: f64,
    },
    /// Mode 4.
    SmoothStairs {
        n: f64,
        rounded: bool,
    },
    /// Mode 5.
    Pulse {
        n: f64,
        side: f64,
    },
    /// Mode 6: `periods` of a sine for a tension below 0 and of a triangle
    /// from 0 up.
    Wave {
        periods: f64,
        sine: bool,
    },
    /// Modes 7 and 8.
    SingleCurve2 {
        tension: f64,
    },
    DoubleCurve2 {
        tension: f64,
    },
    /// Mode 9.
    HalfSine {
        tension: f64,
    },
    /// Mode 10: a cubic with the slope `into` at its start and `out` at
    /// its end, mixed with the straight line by `amount`.
    Smooth {
        into: f64,
        out: f64,
        amount: f64,
    },
    /// Modes 11 and 12.
    SingleCurve3 {
        a: f64,
        b: f64,
        c: f64,
        up: bool,
    },
    DoubleCurve3 {
        a: f64,
        b: f64,
        c: f64,
        up: bool,
    },
}

/// `(e^(ku) - 1) / (e^k - 1)`, the bend of modes 0 and 1.
fn bend(k: f64, u: f64) -> f64 {
    (k * u).exp_m1() / k.exp_m1()
}

/// The bend of modes 0 and 1 for either sign of the tension.
fn single_curve(k: f64, up: bool, t: f64) -> f64 {
    if up {
        1.0 - bend(k, 1.0 - t)
    } else {
        bend(k, t)
    }
}

/// Mode 7.
fn single_curve_2(tension: f64, t: f64) -> f64 {
    let u = if tension > 0.0 { 1.0 - t } else { t };
    t + tension * (u - u.powi(6))
}

/// Mode 11.
fn single_curve_3(a: f64, b: f64, c: f64, up: bool, t: f64) -> f64 {
    b / (1.0 - a * if up { 1.0 - t } else { t }) - c
}

/// A double curve: `half` over the first half of the segment and its
/// mirror image over the second.
fn doubled(t: f64, half: impl Fn(f64) -> f64) -> f64 {
    if t < 0.5 {
        0.5 * half(2.0 * t)
    } else {
        1.0 - 0.5 * half(2.0 * (1.0 - t))
    }
}

/// FL's step count for modes 3 to 6 (`FUN_00b9d450`): 1000 at no tension,
/// down to 0 at full tension either way.
fn steps(tension: f64) -> f64 {
    let scaled = (L * (1.0 - 0.6 * tension.abs())).exp();
    ((scaled - 1.0) / 1000.0).round_ties_even()
}

/// The slope a smooth segment has where it meets a neighbour: the weighted
/// harmonic mean of the two segments' slopes, which keeps the curve from
/// overshooting. The segment itself has slope 1 and length 1, and `slope`
/// and `len` are the neighbour's on that scale. Flat where the two slopes
/// do not point the same way.
fn tangent(slope: f64, len: f64) -> f64 {
    if slope > 0.0 {
        let (theirs, ours) = (2.0 * len + 1.0, len + 2.0);
        slope * (theirs + ours) / (theirs * slope + ours)
    } else {
        0.0
    }
}

impl Shape {
    /// The shape of the segment that ends at `points[i]`, for `i` from 1.
    fn new(points: &[Point], i: usize) -> Self {
        let point = &points[i];
        let tension = f64::from(point.tension);
        let (size, up) = (tension.abs(), tension > 0.0);
        let side = if tension >= 0.0 { 1.0 } else { -1.0 };
        if tension == 0.0 && matches!(point.mode, 0 | 1 | 7 | 8 | 9 | 11 | 12) {
            return Shape::Line;
        }
        match point.mode {
            0 => Shape::SingleCurve { k: size * L, up },
            1 => Shape::DoubleCurve { k: size * L, up },
            3 => Shape::Stairs {
                n: steps(tension) + if tension >= 0.0 { 2.0 } else { 1.0 },
                side,
            },
            4 => Shape::SmoothStairs {
                n: steps(tension) + 1.0,
                rounded: tension < 0.0,
            },
            5 => Shape::Pulse {
                n: 2.0 * (steps(tension) + 1.0),
                side,
            },
            6 if up && steps(tension) == 0.0 => Shape::Line,
            6 => Shape::Wave {
                periods: steps(tension) + 0.5,
                sine: tension < 0.0,
            },
            7 => Shape::SingleCurve2 { tension },
            8 => Shape::DoubleCurve2 { tension },
            9 => Shape::HalfSine { tension },
            10 => Self::smooth(points, i),
            11 | 12 => {
                let reach = (0.9 * size).max(1.525_878_906_25e-5);
                let a = 1.0 - (1.0 - reach).powi(3);
                let b = (1.0 - a) / a;
                let (b, c) = if up { (-b, -b - 1.0) } else { (b, b) };
                if point.mode == 11 {
                    Shape::SingleCurve3 { a, b, c, up }
                } else {
                    Shape::DoubleCurve3 { a, b, c, up }
                }
            }
            _ => Shape::Hold,
        }
    }

    /// Mode 10, from the loader's precompute at 0xb9be64. The slopes at
    /// the two ends come from the neighbouring segments when those are
    /// smooth as well, and are flat when they are not.
    fn smooth(points: &[Point], i: usize) -> Self {
        let (from, to) = (points[i - 1].y, points[i].y);
        let rise = to - from;
        if rise.abs() < EPSILON {
            return Shape::Line;
        }
        let len = points[i].x.max(EPSILON);
        // A neighbour's length and its rise, both in units of this segment.
        let (before_len, before_rise) = if points[i - 1].mode == 10 {
            let start = i.checked_sub(2).map_or(from, |j| points[j].y);
            (points[i - 1].x / len, (from - start) / rise)
        } else {
            (1.0, 0.0)
        };
        let (after_len, after_rise) = match points.get(i + 1) {
            Some(next) if next.mode == 10 => (next.x / len, (next.y - to) / rise),
            _ => (1.0, 0.0),
        };
        let (before_len, after_len) = (before_len.max(EPSILON), after_len.max(EPSILON));
        Shape::Smooth {
            into: tangent(before_rise / before_len, before_len),
            out: tangent(after_rise / after_len, after_len),
            amount: 1.0 - f64::from(points[i].tension).abs(),
        }
    }

    /// How far along the segment's rise it is at `t`, for `t` above 0 and
    /// below 1.
    fn at(self, t: f64) -> f64 {
        match self {
            Shape::Line => t,
            Shape::Hold => 0.0,
            Shape::SingleCurve { k, up } => single_curve(k, up, t),
            Shape::DoubleCurve { k, up } => doubled(t, |u| single_curve(k, up, u)),
            Shape::Stairs { n, side } => (t * n - 0.5 * side).round_ties_even() / (n - side),
            Shape::SmoothStairs { n, rounded } => {
                let s = t * n - 0.5;
                let tread = (s - 0.5).round_ties_even();
                let u = 2.0 * (s - tread) - 1.0;
                let w = if rounded {
                    u.signum() * (1.0 - (1.0 - u.abs()).powi(3))
                } else {
                    u.powi(3)
                };
                (tread + 0.5 * w + 1.0) / n
            }
            Shape::Pulse { n, side } => (t * n - 0.5 * side).round_ties_even().rem_euclid(2.0),
            Shape::Wave {
                periods,
                sine: true,
            } => ((TAU * (t * periods + 0.75)).sin() + 1.0) / 2.0,
            Shape::Wave { periods, .. } => {
                let s = t * periods - 0.5;
                (2.0 * (s - (s - 0.5).round_ties_even()) - 1.0).abs()
            }
            Shape::SingleCurve2 { tension } => single_curve_2(tension, t),
            Shape::DoubleCurve2 { tension } => doubled(t, |u| single_curve_2(tension, u)),
            Shape::HalfSine { tension } => {
                let quarter = if tension > 0.0 {
                    (t * FRAC_PI_2).sin()
                } else {
                    1.0 - (t * FRAC_PI_2).cos()
                };
                (1.0 - tension.abs()) * t + tension.abs() * quarter
            }
            Shape::Smooth { into, out, amount } => {
                let cubic = 3.0 * t * t - 2.0 * t.powi(3)
                    + out * t * t * (t - 1.0)
                    + into * t * (t - 1.0) * (t - 1.0);
                t + amount * (cubic - t)
            }
            Shape::SingleCurve3 { a, b, c, up } => single_curve_3(a, b, c, up, t),
            Shape::DoubleCurve3 { a, b, c, up } => doubled(t, |u| single_curve_3(a, b, c, up, u)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f64, y: f64, mode: u8, tension: f32) -> Point {
        Point {
            mode,
            tension,
            ..Point::new(x, y)
        }
    }

    /// One segment of this mode and tension, from 0 at 0 to 1 at 1, read
    /// at `t`.
    fn shape(mode: u8, tension: f32, t: f64) -> f64 {
        Curve::new(&[Point::new(0.0, 0.0), point(1.0, 1.0, mode, tension)]).value_at(t)
    }

    fn close(got: f64, want: f64) -> bool {
        (got - want).abs() < 1e-9
    }

    #[test]
    fn every_mode_starts_at_the_point_before_and_ends_at_its_own() {
        for mode in 0..=13 {
            for tension in [-1.0, -0.5, 0.0, 0.3, 1.0] {
                let at = format!("mode {mode} tension {tension}");
                assert_eq!(shape(mode, tension, 0.0), 0.0, "{at}");
                assert_eq!(shape(mode, tension, 1.0), 1.0, "{at}");
                assert_eq!(shape(mode, tension, 7.0), 1.0, "{at}");
                assert_eq!(shape(mode, tension, -1.0), 0.0, "{at}");
                // And it stays a number in between.
                for step in 1..200 {
                    let value = shape(mode, tension, f64::from(step) / 200.0);
                    assert!((-0.01..=1.01).contains(&value), "{at}: {value} at {step}");
                }
            }
        }
    }

    #[test]
    fn every_mode_half_way_at_no_tension_and_at_half_tension() {
        // Per mode: the value at t = 0.5 for tension 0, 0.5 and -0.5, worked
        // out by hand from the formulas in docs/automation-234.md.
        let g = |k: f64| (0.5 * k).exp_m1() / k.exp_m1();
        let bent = g(0.5 * L);
        // Half tension leaves 16 steps: 1000001^0.7 is 15849. Stairs that
        // go up have 18 treads over 17 rises, and 8.5 rounds to 8. Below 0
        // they are round(17t + 0.5) / 18.
        let stairs = (8.0 / 17.0, 9.0 / 18.0);
        let seventh = 0.5 * (0.5 - 0.5f64.powi(6));
        let a = 1.0 - 0.55f64.powi(3);
        let third = (1.0 - a) / a / (1.0 - 0.5 * a) - (1.0 - a) / a;
        let want = [
            (0, 0.5, 1.0 - bent, bent),
            (1, 0.5, 0.5, 0.5),
            (2, 0.0, 0.0, 0.0),
            (3, 500.0 / 1001.0, stairs.0, stairs.1),
            (4, 0.5, 0.5, 0.5),
            (5, 0.0, 0.0, 0.0),
            // 16.5 periods put both the triangle and the sine half-way up.
            (6, 0.5, 0.5, 0.5),
            (7, 0.5, 0.5 + seventh, 0.5 - seventh),
            (8, 0.5, 0.5, 0.5),
            (
                9,
                0.5,
                0.25 + 0.5 * 0.5f64.sqrt(),
                0.75 - 0.5 * 0.5f64.sqrt(),
            ),
            (10, 0.5, 0.5, 0.5),
            (11, 0.5, 1.0 - third, third),
            (12, 0.5, 0.5, 0.5),
        ];
        for (mode, none, up, down) in want {
            let got = [0.0, 0.5, -0.5].map(|tension| shape(mode, tension, 0.5));
            for (got, want) in got.into_iter().zip([none, up, down]) {
                assert!(close(got, want), "mode {mode}: {got}, wanted {want}");
            }
        }
    }

    #[test]
    fn a_line_and_every_bending_mode_without_tension_are_straight() {
        for mode in [0, 1, 7, 8, 9, 11, 12] {
            for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                assert_eq!(shape(mode, 0.0, t), t, "mode {mode} at {t}");
            }
        }
    }

    #[test]
    fn hold_stays_at_the_point_before_until_the_end() {
        for tension in [-1.0, 0.0, 1.0] {
            assert_eq!(shape(2, tension, 0.0), 0.0);
            assert_eq!(shape(2, tension, 0.5), 0.0);
            assert_eq!(shape(2, tension, 0.999_999), 0.0);
            assert_eq!(shape(2, tension, 1.0), 1.0);
        }
        // A mode FL has no curve for holds as well.
        assert_eq!(shape(13, 0.0, 0.5), 0.0);
        assert_eq!(shape(200, 0.0, 0.5), 0.0);
    }

    #[test]
    fn a_single_curve_bends_up_for_a_tension_above_0_and_down_below() {
        // g(u) = (e^(ku) - 1) / (e^k - 1) with k = ln 1000001 at full
        // tension, so g(0.5) is close to 1000 / 1000000.
        let half = (1_000_001f64.sqrt() - 1.0) / 1e6;
        assert!(close(shape(0, -1.0, 0.5), half));
        assert!(close(shape(0, 1.0, 0.5), 1.0 - half));
        assert!(shape(0, -0.2, 0.5) < 0.5 && shape(0, 0.2, 0.5) > 0.5);
        // The two signs mirror each other.
        for t in [0.1, 0.5, 0.8] {
            assert!(close(shape(0, 0.4, t), 1.0 - shape(0, -0.4, 1.0 - t)));
        }
        // A tension close to 0 is close to the line.
        assert!((shape(0, 1e-6, 0.5) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn the_double_curves_are_two_halves_that_meet_in_the_middle() {
        for (double, single) in [(1, 0), (8, 7), (12, 11)] {
            for tension in [-1.0, -0.3, 0.6, 1.0] {
                let at = format!("mode {double} tension {tension}");
                assert!(close(shape(double, tension, 0.5), 0.5), "{at}");
                let half = shape(single, tension, 0.5);
                assert!(close(shape(double, tension, 0.25), 0.5 * half), "{at}");
                assert!(
                    close(shape(double, tension, 0.75), 1.0 - 0.5 * half),
                    "{at}"
                );
            }
        }
    }

    #[test]
    fn the_stairs_and_the_pulse_step() {
        // Full tension leaves 0 steps. Stairs that go up get one more:
        // round(2t - 0.5) / 1, a single step half-way.
        assert_eq!(shape(3, 1.0, 0.4), 0.0);
        assert_eq!(shape(3, 1.0, 0.6), 1.0);
        // Below 0 it is round(t + 0.5) / 2: half-way up from the start on.
        assert_eq!(shape(3, -1.0, 0.01), 0.5);
        assert_eq!(shape(3, -1.0, 0.99), 0.5);
        // No tension is 1000 steps, which is the line to a thousandth.
        for t in [0.123, 0.5, 0.877] {
            assert!((shape(3, 0.0, t) - t).abs() < 0.001);
            assert!((shape(4, 0.0, t) - t).abs() < 0.001);
            assert!((shape(3, -1e-6, t) - t).abs() < 0.001);
        }
        // Smooth stairs with one step: flat at both ends, and through the
        // middle on a cube (tension from 0 up) or on its mirror image.
        assert!(close(shape(4, 1.0, 0.5), 0.5));
        assert!(close(shape(4, 1.0, 0.25), 0.5 * 0.125));
        assert!(close(shape(4, 1.0, 0.75), 1.0 - 0.5 * 0.125));
        assert!(close(shape(4, -1.0, 0.25), 0.5 * 0.875));
        assert!(close(shape(4, -1.0, 0.75), 1.0 - 0.5 * 0.875));
        // A pulse is 0 or 1 and nothing else. At full tension it has two
        // halves: low then high from 0 up, high then low below.
        let levels = |tension| [0.1, 0.3, 0.6, 0.9].map(|t| shape(5, tension, t));
        assert_eq!(levels(1.0), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(levels(-1.0), [1.0, 1.0, 0.0, 0.0]);
        // 1000 steps are 1001 pulses, each a little under 0.001 long.
        let levels = [0.0004, 0.0006, 0.0012, 0.0016].map(|t| shape(5, 0.0, t));
        assert_eq!(levels, [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn a_wave_swings_between_the_two_points() {
        // Full negative tension is half a period of a sine, from its
        // trough to its crest.
        assert!(close(shape(6, -1.0, 0.5), 0.5));
        assert!(close(shape(6, -1.0, 0.25), (1.0 - 0.5f64.sqrt()) / 2.0));
        // Full positive tension has no steps, and FL draws a line.
        assert_eq!(shape(6, 1.0, 0.3), 0.3);
        // No tension is 1000.5 periods: up and down again within a
        // thousandth of the segment.
        assert!(shape(6, 0.0, 0.0005) > 0.9);
        assert!(shape(6, 0.0, 0.001) < 0.1);
        assert!(shape(6, -0.0001, 0.0005) > 0.9);
    }

    #[test]
    fn the_second_and_third_single_curves_and_the_half_sine() {
        // Mode 7 adds tension * (u - u^6) to the line.
        let bulge = 0.5 - 0.5f64.powi(6);
        assert!(close(shape(7, 1.0, 0.5), 0.5 + bulge));
        assert!(close(shape(7, -1.0, 0.5), 0.5 - bulge));
        assert!(close(shape(7, 0.5, 0.5), 0.5 + 0.5 * bulge));
        // Mode 9 mixes the line with a quarter of a sine.
        assert!(close(shape(9, 1.0, 0.5), 0.5f64.sqrt()));
        assert!(close(shape(9, -1.0, 0.5), 1.0 - 0.5f64.sqrt()));
        assert!(close(shape(9, 0.5, 0.5), 0.25 + 0.5 * 0.5f64.sqrt()));
        // Mode 11 at full tension: A = 1 - 0.1^3, so the curve is at
        // 0.001 / 0.5005 - 0.001 / 0.999 half-way.
        let a = 0.999;
        let low = (1.0 - a) / a / (1.0 - a * 0.5) - (1.0 - a) / a;
        assert!(close(shape(11, -1.0, 0.5), low));
        assert!(close(shape(11, 1.0, 0.5), 1.0 - low));
        assert!(shape(11, -0.2, 0.5) < 0.5 && shape(11, 0.2, 0.5) > 0.5);
    }

    #[test]
    fn a_smooth_segment_alone_eases_in_and_out() {
        // Flat at both ends: 3t^2 - 2t^3.
        for t in [0.1f64, 0.25, 0.5, 0.9] {
            assert!(close(shape(10, 0.0, t), 3.0 * t * t - 2.0 * t.powi(3)));
        }
        // Tension takes it towards the line, either sign alike.
        let eased = 3.0 * 0.0625 - 2.0 * 0.015_625;
        for tension in [0.5, -0.5] {
            assert!(close(shape(10, tension, 0.25), 0.25 + 0.5 * (eased - 0.25)));
        }
        assert_eq!(shape(10, 1.0, 0.25), 0.25);
    }

    #[test]
    fn smooth_neighbours_pass_through_a_point_at_one_slope() {
        // Three points on a line, all smooth: the middle keeps the line's
        // slope, and the ends are flat.
        let points = [
            point(0.0, 0.0, 10, 0.0),
            point(1.0, 0.25, 10, 0.0),
            point(1.0, 0.5, 10, 0.0),
            point(1.0, 0.75, 10, 0.0),
        ];
        let curve = Curve::new(&points);
        let slope = |at: f64| (curve.value_at(at + 1e-6) - curve.value_at(at - 1e-6)) / 2e-6;
        assert!((slope(1.0) - 0.25).abs() < 1e-5);
        assert!((slope(2.0) - 0.25).abs() < 1e-5);
        assert!(slope(1e-5).abs() < 1e-4 && slope(3.0 - 1e-5).abs() < 1e-4);
        // The middle segment has slope 1 at both ends, so it is the line.
        assert!(close(curve.value_at(1.5), 0.375));

        // At a peak the slopes point opposite ways, and the curve is flat.
        let peak = [
            point(0.0, 0.0, 10, 0.0),
            point(1.0, 1.0, 10, 0.0),
            point(1.0, 0.0, 10, 0.0),
        ];
        let curve = Curve::new(&peak);
        assert!(close(curve.value_at(0.5), 0.5));
        assert!(curve.value_at(0.99) < 1.0 && curve.value_at(0.99) > 0.999);

        // A neighbour of another mode leaves the end flat too.
        let mixed = [
            point(0.0, 0.0, 0, 0.0),
            point(1.0, 0.5, 0, 0.0),
            point(1.0, 1.0, 10, 0.0),
        ];
        assert!(close(
            Curve::new(&mixed).value_at(1.25),
            0.5 + 0.5 * 0.15625
        ));
        // And between two equal values there is nothing to shape.
        let level = [point(0.0, 0.5, 10, 0.0), point(1.0, 0.5, 10, 0.0)];
        assert_eq!(Curve::new(&level).value_at(0.3), 0.5);
    }

    #[test]
    fn a_curve_is_its_segments_end_to_end() {
        let curve = Curve::new(&[
            Point::new(0.0, 0.25),
            Point::new(2.0, 1.0),
            point(1.0, 0.5, 2, 0.0),
            // No length: a jump.
            Point::new(0.0, 0.0),
            Point::new(4.0, 1.0),
        ]);
        let values: Vec<f64> = [-1.0, 0.0, 1.0, 2.0, 2.5, 3.0, 5.0, 7.0, 99.0]
            .map(|x| curve.value_at(x))
            .to_vec();
        assert_eq!(values, [0.25, 0.25, 0.625, 1.0, 1.0, 0.0, 0.5, 1.0, 1.0]);
        assert_eq!(curve.value_at(f64::NAN), 0.25);
        assert_eq!(curve.value_at(f64::INFINITY), 1.0);

        assert_eq!(Curve::new(&[]).value_at(1.0), 0.0);
        assert_eq!(Curve::new(&[Point::new(3.0, 0.7)]).value_at(1.0), 0.7);
        // A first point that is not at 0 moves the whole curve.
        let late = Curve::new(&[Point::new(1.0, 0.0), Point::new(2.0, 1.0)]);
        assert_eq!(late.value_at(0.5), 0.0);
        assert_eq!(late.value_at(2.0), 0.5);
    }

    #[test]
    fn a_broken_point_reads_as_fl_s_loader_reads_it() {
        let curve = Curve::new(&[
            Point::new(0.0, f64::NAN),
            Point::new(1.0, 0.5),
            Point::new(-3.0, f64::NAN),
            Point::new(f64::NAN, f64::INFINITY),
            Point::new(1.0, 1.0),
        ]);
        // 0, a line to 0.5, two points on the spot that keep 0.5, a line
        // to 1.
        assert_eq!(curve.value_at(0.0), 0.0);
        assert_eq!(curve.value_at(0.5), 0.25);
        assert_eq!(curve.value_at(1.0), 0.5);
        assert_eq!(curve.value_at(1.5), 0.75);
    }
}
