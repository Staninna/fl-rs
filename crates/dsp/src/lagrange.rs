//! The four-point Lagrange read Image-Line's grain shifters share: Pitch
//! Shifter's two engines, and Luxeverb's predelay and pitch shifters, read
//! their delay lines with it. It is FL's own arithmetic, read from the
//! binaries: the constants are FL's `1/3` and `1/6` as they sit in its
//! data, a few ulps off the nearest `f32`, and the sums run in FL's order,
//! so it rounds as FL's code does.

/// FL's `1/3` (`0x3eaaaa9f`).
const THIRD: f32 = f32::from_bits(0x3eaa_aa9f);
/// FL's `1/6` (`0x3e2aaac1`).
const SIXTH: f32 = f32::from_bits(0x3e2a_aac1);

/// The cubic through four samples, read `frac` (0 to 1) of the way from
/// `x0` to `x1`. `before` lies one sample on the other side of `x0`, and
/// `x2` one past `x1`, so `before, x0, x1, x2` are four samples in a row.
/// Which way the row runs in the buffer is the caller's: Pitch Shifter
/// reads forwards, Luxeverb from the write head back. At `frac` 0 it
/// gives `x0`.
#[inline]
pub fn read(before: f32, x0: f32, x1: f32, x2: f32, frac: f32) -> f32 {
    let f2 = frac * frac;
    let x2 = x2 * SIXTH;
    (((x1 - (before * THIRD + x0 * 0.5)) - x2) + ((x2 - before * SIXTH) + (x0 - x1) * 0.5) * f2)
        * frac
        + x0
        + ((x1 * 0.5 + before * 0.5) - x0) * f2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_passes_through_its_points() {
        assert_eq!(read(1.0, 2.0, 3.0, 4.0, 0.0), 2.0);
        let at_one = read(0.5, -1.0, 0.25, 2.0, 1.0);
        assert!((at_one - 0.25).abs() < 1e-6, "{at_one}");
    }

    #[test]
    fn it_follows_a_cubic() {
        // Four points of a cubic, read between the middle two, give the
        // cubic to within FL's constants.
        let p = |t: f32| 0.3 * t * t * t - t * t + 0.5 * t + 0.2;
        for k in 0..=10 {
            let frac = k as f32 / 10.0;
            let got = read(p(-1.0), p(0.0), p(1.0), p(2.0), frac);
            assert!((got - p(frac)).abs() < 1e-5, "{frac}: {got} {}", p(frac));
        }
    }

    #[test]
    fn it_rounds_as_fl_does() {
        // FL's constants are not the nearest `f32`s, and the order of the
        // sums is FL's: this value is pinned to the bit.
        assert_ne!(THIRD, 1.0 / 3.0);
        assert_ne!(SIXTH, 1.0 / 6.0);
        assert_eq!(read(0.1, -0.7, 0.9, 0.3, 0.37).to_bits(), 0xbe12_510b);
    }
}
