//! Signal building blocks the engine and the plugins compose: gain and pan
//! maths, a biquad filter, and fractional sample readers in [`resample`].
//! Everything works on `f32` samples and allocates nothing in its hot path.
//! The double biquad takes `f32` samples too and keeps its state in `f64`.
//! FL's pan law lives in the engine.
//!
//! Three parts are FL's own, read from its binaries: [`curve`], the
//! envelope curve that both the engine and the plugins draw; [`interpolator`],
//! the sample interpolator FL's plugins share; and [`additive`], the sine
//! bank Image-Line's additive synths build their voices on.

pub mod additive;
pub mod biquad;
pub mod curve;
pub mod interpolator;
pub mod resample;

/// Decibels to linear gain. `-inf` and below -200 dB give 0.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -200.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Linear gain to decibels. 0 gives `-inf`.
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * gain.log10()
    }
}

/// Constant-power pan. `pan` runs -1 (left) to 1 (right); centre gives
/// 1/sqrt(2) on both sides so a mono source keeps its power.
pub fn pan_constant_power(pan: f32) -> (f32, f32) {
    let t = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * std::f32::consts::PI;
    (t.cos(), t.sin())
}

/// Linear pan. Centre gives 1 on both sides; full left mutes the right.
/// This is the "no law" option, useful as a reference.
pub fn pan_linear(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_round_trips() {
        for db in [-60.0, -6.0, 0.0, 6.0] {
            assert!((gain_to_db(db_to_gain(db)) - db).abs() < 1e-4);
        }
        assert_eq!(db_to_gain(-300.0), 0.0);
        assert_eq!(gain_to_db(0.0), f32::NEG_INFINITY);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn constant_power_keeps_power() {
        for pan in [-1.0, -0.5, 0.0, 0.3, 1.0] {
            let (l, r) = pan_constant_power(pan);
            assert!((l * l + r * r - 1.0).abs() < 1e-6, "pan {pan}");
        }
        let (l, r) = pan_constant_power(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
    }

    #[test]
    fn linear_pan_centre_is_unity() {
        assert_eq!(pan_linear(0.0), (1.0, 1.0));
        assert_eq!(pan_linear(1.0), (0.0, 1.0));
    }
}
