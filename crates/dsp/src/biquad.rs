//! Direct-form-I biquad with the Audio EQ Cookbook coefficient formulas, in
//! single precision and in double.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefficients {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Coefficients {
    /// Pass everything through unchanged.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    pub fn low_pass(sample_rate: f32, cutoff: f32, q: f32) -> Self {
        let (w0, alpha) = w0_alpha(sample_rate, cutoff, q);
        let c = w0.cos();
        Self::normalised(
            (1.0 - c) * 0.5,
            1.0 - c,
            (1.0 - c) * 0.5,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        )
    }

    pub fn high_pass(sample_rate: f32, cutoff: f32, q: f32) -> Self {
        let (w0, alpha) = w0_alpha(sample_rate, cutoff, q);
        let c = w0.cos();
        Self::normalised(
            (1.0 + c) * 0.5,
            -(1.0 + c),
            (1.0 + c) * 0.5,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        )
    }

    pub fn band_pass(sample_rate: f32, centre: f32, q: f32) -> Self {
        let (w0, alpha) = w0_alpha(sample_rate, centre, q);
        let c = w0.cos();
        Self::normalised(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    /// Peaking EQ with `gain_db` boost or cut at `centre`.
    pub fn peak(sample_rate: f32, centre: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (w0, alpha) = w0_alpha(sample_rate, centre, q);
        let c = w0.cos();
        Self::normalised(
            1.0 + alpha * a,
            -2.0 * c,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * c,
            1.0 - alpha / a,
        )
    }

    fn normalised(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
}

fn w0_alpha(sample_rate: f32, freq: f32, q: f32) -> (f32, f32) {
    let w0 = 2.0 * std::f32::consts::PI * (freq / sample_rate).clamp(1e-5, 0.49999);
    (w0, w0.sin() / (2.0 * q.max(1e-3)))
}

/// One channel of filter state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[inline]
    pub fn process(&mut self, c: &Coefficients, x: f32) -> f32 {
        let y = c.b0 * x + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    pub fn process_block(&mut self, c: &Coefficients, buf: &mut [f32]) {
        for x in buf {
            *x = self.process(c, *x);
        }
    }
}

/// Cookbook coefficients in double precision, for a filter that has to stay
/// exact where its poles sit close to the unit circle: a corner of a few Hz,
/// or a high Q.
///
/// The cookbook works every filter out from `w0`, the corner in radians a
/// sample, and `alpha`, which sets the width. The constructors take the
/// cosine of `w0` and `alpha` as they are, so the caller picks how a knob
/// becomes a width: a Q is `sin(w0) / (2 Q)`, and [`bandwidth_alpha`] is the
/// cookbook's form for a width in octaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefficients64 {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefficients64 {
    /// Pass everything through unchanged.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    pub fn low_pass(cos: f64, alpha: f64) -> Self {
        let inv = 1.0 / (alpha + 1.0);
        let b1 = inv * (1.0 - cos);
        Self {
            b0: b1 * 0.5,
            b1,
            b2: b1 * 0.5,
            a1: inv * cos * -2.0,
            a2: inv * (1.0 - alpha),
        }
    }

    pub fn high_pass(cos: f64, alpha: f64) -> Self {
        let inv = 1.0 / (alpha + 1.0);
        let b1 = inv * -(cos + 1.0);
        Self {
            b0: -b1 * 0.5,
            b1,
            b2: -b1 * 0.5,
            a1: inv * cos * -2.0,
            a2: inv * (1.0 - alpha),
        }
    }

    /// The band pass that is at unity in its centre.
    pub fn band_pass(cos: f64, alpha: f64) -> Self {
        let inv = 1.0 / (alpha + 1.0);
        Self {
            b0: inv * alpha,
            b1: 0.0,
            b2: inv * -alpha,
            a1: inv * cos * -2.0,
            a2: inv * (1.0 - alpha),
        }
    }

    pub fn notch(cos: f64, alpha: f64) -> Self {
        let inv = 1.0 / (alpha + 1.0);
        let a1 = inv * cos * -2.0;
        Self {
            b0: inv,
            b1: a1,
            b2: inv,
            a1,
            a2: inv * (1.0 - alpha),
        }
    }

    /// The allpass that turns the phase by 180 degrees at `w0`.
    pub fn all_pass(cos: f64, alpha: f64) -> Self {
        let inv = 1.0 / (alpha + 1.0);
        let a1 = inv * cos * -2.0;
        let a2 = inv * (1.0 - alpha);
        Self {
            b0: a2,
            b1: a1,
            b2: 1.0,
            a1,
            a2,
        }
    }

    /// Peaking EQ. `a` is the square root of the gain at the centre:
    /// `10^(dB / 40)`.
    pub fn peak(cos: f64, alpha: f64, a: f64) -> Self {
        let inv = 1.0 / (alpha / a + 1.0);
        let a1 = inv * cos * -2.0;
        Self {
            b0: inv * (alpha * a + 1.0),
            b1: a1,
            b2: inv * (1.0 - alpha * a),
            a1,
            a2: inv * (1.0 - alpha / a),
        }
    }

    /// Low shelf. `a` is the square root of the shelf's gain, and
    /// `beta_sin` is the cookbook's `2 sqrt(A) alpha`: [`shelf_beta`] times
    /// the sine of `w0`.
    pub fn low_shelf(cos: f64, beta_sin: f64, a: f64) -> Self {
        let tilt = (a - 1.0) * cos;
        let inv = 1.0 / (a + 1.0 + tilt + beta_sin);
        Self {
            b0: inv * a * (((a + 1.0) - tilt) + beta_sin),
            b1: inv * a * 2.0 * ((a - 1.0) - (a + 1.0) * cos),
            b2: inv * a * (((a + 1.0) - tilt) - beta_sin),
            a1: inv * ((a - 1.0) + (a + 1.0) * cos) * -2.0,
            a2: inv * ((a + 1.0 + tilt) - beta_sin),
        }
    }

    /// High shelf, with `a` and `beta_sin` as for [`Self::low_shelf`].
    pub fn high_shelf(cos: f64, beta_sin: f64, a: f64) -> Self {
        let tilt = (a - 1.0) * cos;
        let inv = 1.0 / (((a + 1.0) - tilt) + beta_sin);
        Self {
            b0: inv * a * (a + 1.0 + tilt + beta_sin),
            b1: inv * a * -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            b2: inv * a * ((a + 1.0 + tilt) - beta_sin),
            a1: inv * ((a - 1.0) - (a + 1.0) * cos) * 2.0,
            a2: inv * (((a + 1.0) - tilt) - beta_sin),
        }
    }
}

/// The cookbook's `alpha` for a width in octaves between the half-gain
/// points: `sin(w0) sinh(ln 2 / 2 * octaves * w0 / sin(w0))`.
pub fn bandwidth_alpha(w0: f64, octaves: f64) -> f64 {
    let sin = w0.sin();
    sin * (std::f64::consts::LN_2 / 2.0 * octaves * w0 / sin).sinh()
}

/// What a shelf of slope `S` has for `beta`, with `a` the square root of
/// its gain: `sqrt(A) sqrt((A + 1/A) (1/S - 1) + 2)`. A slope of 1 is the
/// steepest shelf that does not overshoot.
pub fn shelf_beta(a: f64, slope: f64) -> f64 {
    a.sqrt() * ((a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0).sqrt()
}

/// One channel of direct-form-I state in double precision.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Biquad64 {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad64 {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[inline]
    pub fn process(&mut self, c: &Coefficients64, x: f64) -> f64 {
        let y = x * c.b0 + (((self.x1 * c.b1 - self.y2 * c.a2) + self.x2 * c.b2) - self.y1 * c.a1);
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    /// Run `f32` samples through in place. The state keeps each output in
    /// full, before it is cut down to an `f32`.
    pub fn process_block(&mut self, c: &Coefficients64, buf: &mut [f32]) {
        for x in buf {
            *x = self.process(c, f64::from(*x)) as f32;
        }
    }

    /// Zero whatever of the state is too small for a double to hold in
    /// full, before it slows the sums down. For the end of a block.
    pub fn flush(&mut self) {
        for state in [&mut self.x1, &mut self.x2, &mut self.y1, &mut self.y2] {
            if state.is_subnormal() {
                *state = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms_of_sine(c: &Coefficients, freq: f32, sr: f32) -> f32 {
        let mut f = Biquad::default();
        let n = (sr as usize) / 2;
        let mut acc = 0.0f64;
        for i in 0..n * 2 {
            let x = (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin();
            let y = f.process(c, x);
            if i >= n {
                acc += (y * y) as f64;
            }
        }
        ((acc / n as f64).sqrt() * std::f64::consts::SQRT_2) as f32
    }

    #[test]
    fn identity_passes_through() {
        let mut f = Biquad::default();
        assert_eq!(f.process(&Coefficients::IDENTITY, 0.7), 0.7);
    }

    #[test]
    fn low_pass_keeps_lows_and_cuts_highs() {
        let c = Coefficients::low_pass(48_000.0, 1_000.0, std::f32::consts::FRAC_1_SQRT_2);
        assert!((rms_of_sine(&c, 50.0, 48_000.0) - 1.0).abs() < 0.02);
        assert!(rms_of_sine(&c, 10_000.0, 48_000.0) < 0.02);
    }

    #[test]
    fn peak_boosts_by_requested_db() {
        let c = Coefficients::peak(48_000.0, 1_000.0, 1.0, 6.0);
        let g = rms_of_sine(&c, 1_000.0, 48_000.0);
        assert!((crate::gain_to_db(g) - 6.0).abs() < 0.1, "got {g}");
    }

    /// The gain of `c` at `turns` a sample, from its transfer function.
    fn gain(c: &Coefficients64, turns: f64) -> f64 {
        let w = std::f64::consts::TAU * turns;
        let size = |k: [f64; 3]| {
            let re = k[0] + k[1] * w.cos() + k[2] * (2.0 * w).cos();
            let im = k[1] * w.sin() + k[2] * (2.0 * w).sin();
            re.hypot(im)
        };
        size([c.b0, c.b1, c.b2]) / size([1.0, c.a1, c.a2])
    }

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    /// `cos(w0)` and the `alpha` of a Q for a corner at `turns` a sample.
    fn corner(turns: f64, q: f64) -> (f64, f64) {
        let (sin, cos) = (std::f64::consts::TAU * turns).sin_cos();
        (cos, sin / (2.0 * q))
    }

    #[test]
    fn the_double_filters_have_the_cookbook_s_gains() {
        use std::f64::consts::TAU;
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let (cos, alpha) = corner(0.05, q);
        // A low pass and a high pass have the gain Q at the corner.
        let low = Coefficients64::low_pass(cos, alpha);
        assert!((gain(&low, 0.05) - q).abs() < 1e-12);
        assert!((gain(&low, 0.0) - 1.0).abs() < 1e-12 && gain(&low, 0.5) < 1e-12);
        let high = Coefficients64::high_pass(cos, alpha);
        assert!((gain(&high, 0.05) - q).abs() < 1e-12);
        assert!((gain(&high, 0.5) - 1.0).abs() < 1e-12 && gain(&high, 0.0) < 1e-12);
        // A band pass is at unity in its centre and a notch at nothing.
        let band = Coefficients64::band_pass(cos, alpha);
        assert!((gain(&band, 0.05) - 1.0).abs() < 1e-12 && gain(&band, 0.0) < 1e-12);
        let notch = Coefficients64::notch(cos, alpha);
        assert!(gain(&notch, 0.05) < 1e-12 && (gain(&notch, 0.0) - 1.0).abs() < 1e-12);
        // An allpass is at unity everywhere, and upside down at the corner.
        let all = Coefficients64::all_pass(cos, alpha);
        for turns in [0.0, 0.01, 0.05, 0.3, 0.5] {
            assert!((gain(&all, turns) - 1.0).abs() < 1e-12, "{turns}");
        }
        let (sin, cos_2) = (TAU * 0.05).sin_cos();
        let (sin_2, cos_4) = (TAU * 0.1).sin_cos();
        let num = (
            all.b0 + all.b1 * cos_2 + all.b2 * cos_4,
            -all.b1 * sin - all.b2 * sin_2,
        );
        let den = (
            1.0 + all.a1 * cos_2 + all.a2 * cos_4,
            -all.a1 * sin - all.a2 * sin_2,
        );
        // num / den = -1: the two are each other's opposite.
        assert!((num.0 + den.0).abs() < 1e-12 && (num.1 + den.1).abs() < 1e-12);
        // A peak has its gain in the centre and none at either end.
        let a = 10f64.powf(6.0 / 40.0);
        let peak = Coefficients64::peak(cos, alpha, a);
        assert!((db(gain(&peak, 0.05)) - 6.0).abs() < 1e-9);
        assert!(db(gain(&peak, 0.0)).abs() < 1e-9 && db(gain(&peak, 0.5)).abs() < 1e-9);
        // A shelf has its gain at its own end and half of it, in dB, at
        // the corner.
        let beta_sin = shelf_beta(a, 1.0) * (std::f64::consts::TAU * 0.05).sin();
        let low = Coefficients64::low_shelf(cos, beta_sin, a);
        assert!((db(gain(&low, 0.0)) - 6.0).abs() < 1e-9 && db(gain(&low, 0.5)).abs() < 1e-9);
        assert!((db(gain(&low, 0.05)) - 3.0).abs() < 1e-9);
        let high = Coefficients64::high_shelf(cos, beta_sin, a);
        assert!((db(gain(&high, 0.5)) - 6.0).abs() < 1e-9 && db(gain(&high, 0.0)).abs() < 1e-9);
        assert!((db(gain(&high, 0.05)) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_width_in_octaves_is_between_the_half_gain_points() {
        // Low in the band, where the bilinear warp is small: a peak of
        // +12 dB two octaves wide is at +6 dB one octave either side.
        let w0 = std::f64::consts::TAU * 0.002;
        let peak =
            Coefficients64::peak(w0.cos(), bandwidth_alpha(w0, 2.0), 10f64.powf(12.0 / 40.0));
        for turns in [0.001, 0.004] {
            assert!((db(gain(&peak, turns)) - 6.0).abs() < 0.01, "{turns}");
        }
        // At a slope of 1 the shelf's beta is sqrt(2 A).
        assert!((shelf_beta(4.0, 1.0) - 8f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn the_double_biquad_is_the_single_one_with_more_bits() {
        let single = Coefficients::peak(48_000.0, 1_000.0, 1.0, 6.0);
        let w0 = std::f64::consts::TAU * 1_000.0 / 48_000.0;
        let double = Coefficients64::peak(w0.cos(), w0.sin() / 2.0, 10f64.powf(6.0 / 40.0));
        let (mut f, mut d) = (Biquad::default(), Biquad64::default());
        let mut block: Vec<f32> = (0..256)
            .map(|n| ((n * 37 % 101) as f32 - 50.0) / 50.0)
            .collect();
        let want: Vec<f32> = block.iter().map(|x| f.process(&single, *x)).collect();
        d.process_block(&double, &mut block);
        for (got, want) in block.iter().zip(want) {
            assert!((got - want).abs() < 1e-5, "{got} and not {want}");
        }

        let mut d = Biquad64::default();
        assert_eq!(d.process(&Coefficients64::IDENTITY, 0.7), 0.7);
        // A state too small to hold in full is gone after the flush, and
        // any other stays.
        d.y1 = f64::MIN_POSITIVE / 8.0;
        d.flush();
        assert_eq!((d.x1, d.y1), (0.7, 0.0));
        d.reset();
        assert_eq!(d, Biquad64::default());
    }
}
