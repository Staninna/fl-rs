//! Fruity Balance: a volume and a pan, as `docs/plugins/fruity-balance.md`
//! has them.
//!
//! The state is two little-endian i32, the plugin's two parameters:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Balance | -128 (left) to 128 (right) | 0 |
//! | 1 Volume | 0 to 320 | 256 |
//!
//! They make a 2x2 matrix of gains, which [`gains`] works out. Volume is
//! `(11^(v / 256) - 1) / 10` a side at centre: silent at 0, unity at 256
//! and +5.6 dB at 320. The pan is constant power with two twists: the level
//! drops towards the sides, and the far side is folded into the near one,
//! so hard left is both inputs on the left at -3 dB each.

use crate::parts::Matrix;
use crate::{Effect, StateError, knob_gain, knob_value, read_state};
use std::f64::consts::{FRAC_PI_4, PI};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Balance";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 2] = [(-128, 128), (0, 320)];
const DEFAULTS: [i32; 2] = [0, 256];

/// The matrix for a balance and a volume, as `ProcessParam` works it out:
/// left to left, right to right, right into left, left into right.
///
/// At the defaults it is 1, 1, 0, 0 to the bit.
pub fn gains(balance: i32, volume: i32) -> [f32; 4] {
    let pos = f64::from(balance + 128) / 256.0;
    let pan = f64::from(balance) / 128.0;
    // FL keeps 1 / (cos(pi/4) * 10) for this, the factor that puts the
    // defaults at unity. The tenth is in `knob_gain` here.
    let comp = (1.0 - 0.5 * ((pos - 0.5).abs() * PI).sin()) / FRAC_PI_4.cos();
    // FL narrows the exponent to an f32 on its way into `Exp`.
    let exponent = f64::from((f64::from(volume) / 320.0 * 1.25) as f32);
    let gain = knob_gain(exponent) * comp;
    let angle = (pan + 1.0) * FRAC_PI_4;
    let (left, right) = (gain * angle.cos(), gain * angle.sin());
    // The side the pan leans to takes the other input at the difference of
    // the two gains. At centre the difference is 0, and the last bit of a
    // sine or a cosine must not make it anything else.
    let (into_left, into_right) = match balance {
        ..0 => (left - right, 0.0),
        0 => (0.0, 0.0),
        _ => (0.0, right - left),
    };
    [left, right, into_left, into_right].map(|gain| gain as f32)
}

/// Fruity Balance. A new one has FL's defaults, which pass a block through
/// untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct Balance {
    params: [i32; 2],
    matrix: Matrix,
}

impl Default for Balance {
    fn default() -> Self {
        Balance {
            params: DEFAULTS,
            matrix: Matrix::new(gains(DEFAULTS[0], DEFAULTS[1])),
        }
    }
}

impl Effect for Balance {
    /// The matrix is the state's from the first sample on, with no ramp.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.params = read_state(bytes, &RANGES)?;
        self.matrix.jump(gains(self.params[0], self.params[1]));
        Ok(())
    }

    /// Nothing here depends on the rate.
    fn set_sample_rate(&mut self, _hz: f32) {}

    /// After a parameter has moved, the matrix goes from where the last
    /// block ended to where the parameters put it, in a straight line over
    /// this block.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.matrix.apply(left, right);
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.matrix.aim(gains(self.params[0], self.params[1]));
        }
    }

    /// Ends a ramp that is under way.
    fn reset(&mut self) {
        self.matrix.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsp::gain_to_db;

    fn state(balance: i32, volume: i32) -> Vec<u8> {
        [balance, volume]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    fn with(balance: i32, volume: i32) -> Balance {
        let mut plugin = Balance::default();
        plugin.set_state(&state(balance, volume)).unwrap();
        plugin
    }

    /// What one frame comes out as.
    fn frame(plugin: &mut Balance, left: f32, right: f32) -> (f32, f32) {
        let (mut l, mut r) = ([left], [right]);
        plugin.process(&mut l, &mut r);
        (l[0], r[0])
    }

    fn close(got: f32, want: f32, within: f32) -> bool {
        (got - want).abs() < within
    }

    #[test]
    fn the_defaults_are_unity_to_the_bit() {
        assert_eq!(gains(0, 256), [1.0, 1.0, 0.0, 0.0]);
        let input = [0.0, -0.0, 1.0, -1.0, 0.3, -1e-30, 7.5, f32::MIN_POSITIVE];
        let bits = |side: &[f32]| side.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        for mut plugin in [Balance::default(), with(0, 256)] {
            let (mut left, mut right) = (input, input);
            right.reverse();
            plugin.process(&mut left, &mut right);
            assert_eq!(bits(&left), bits(&input));
            right.reverse();
            assert_eq!(bits(&right), bits(&input));
        }
    }

    #[test]
    fn the_volume_is_the_knob_curve_and_tops_out_at_5_6_db() {
        let (left, right) = frame(&mut with(0, 320), 1.0, 1.0);
        assert_eq!(left, right);
        // (11^1.25 - 1) / 10 = 1.90.
        assert!(close(left, (11f32.powf(1.25) - 1.0) / 10.0, 1e-6));
        assert!(close(gain_to_db(left), 5.6, 0.02));
        assert_eq!(frame(&mut with(0, 0), 1.0, -1.0), (0.0, -0.0));
        // Per side at centre the gain is (11^(v / 256) - 1) / 10.
        for volume in [1, 64, 191, 223, 255, 257, 290] {
            let want = (11f32.powf(volume as f32 / 256.0) - 1.0) / 10.0;
            let (left, right) = frame(&mut with(0, volume), 1.0, 1.0);
            assert!(close(left, want, 1e-6), "volume {volume}: {left}");
            assert_eq!(left, right);
        }
        // Centre has no crossfeed whatever the volume.
        assert_eq!(frame(&mut with(0, 300), 1.0, 0.0).1, 0.0);
    }

    #[test]
    fn hard_left_folds_the_right_side_into_the_left_at_minus_3_db() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        // The doc's matrix: LL 0.7071, RR 0, RL 0.7071.
        let (from_left, silent) = frame(&mut with(-128, 256), 1.0, 0.0);
        assert!(close(from_left, half, 1e-6));
        assert!(close(gain_to_db(from_left), -3.01, 0.01));
        assert_eq!(silent, 0.0);
        let (from_right, silent) = frame(&mut with(-128, 256), 0.0, 1.0);
        assert!(close(from_right, half, 1e-6));
        assert_eq!(silent, 0.0);
        // L' = 0.7071 (L + R).
        let (both, silent) = frame(&mut with(-128, 256), 0.5, 0.25);
        assert!(close(both, half * 0.75, 1e-6));
        assert_eq!(silent, 0.0);

        // Hard right is the mirror image.
        let (silent, from_left) = frame(&mut with(128, 256), 1.0, 0.0);
        assert!(close(from_left, half, 1e-6) && silent.abs() < 1e-12);
        let (silent, from_right) = frame(&mut with(128, 256), 0.0, 1.0);
        assert!(close(from_right, half, 1e-6) && silent.abs() < 1e-12);
    }

    #[test]
    fn half_left_has_the_doc_s_three_gains() {
        // LL 0.845, RR 0.350, RL 0.495, and nothing from left into right.
        let [ll, rr, rl, lr] = gains(-64, 256);
        assert!(close(ll, 0.845, 1e-3) && close(rr, 0.350, 1e-3) && close(rl, 0.495, 1e-3));
        assert_eq!(lr, 0.0);
        assert_eq!(frame(&mut with(-64, 256), 1.0, 0.0), (ll, 0.0));
        assert_eq!(frame(&mut with(-64, 256), 0.0, 1.0), (rl, rr));
        // Half right swaps the sides.
        let mirrored = gains(64, 256);
        assert!(close(mirrored[0], rr, 1e-6) && close(mirrored[1], ll, 1e-6));
        assert!(close(mirrored[3], rl, 1e-6));
        assert_eq!(mirrored[2], 0.0);
        // The level drops towards the sides: by half in the factor FL
        // calls the compensation, which leaves the near side 3 dB down.
        assert!(close(gain_to_db(gains(-128, 256)[0]), -3.01, 0.01));
    }

    #[test]
    fn a_moved_parameter_ramps_over_one_block() {
        let mut plugin = Balance::default();
        plugin.set_param(1, 0.0);
        assert_eq!(plugin.param(1), 0.0);
        let (mut left, mut right) = ([1.0; 4], [-1.0; 4]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [1.0, 0.75, 0.5, 0.25]);
        assert_eq!(right, [-1.0, -0.75, -0.5, -0.25]);
        let (mut left, mut right) = ([1.0; 4], [1.0; 4]);
        plugin.process(&mut left, &mut right);
        assert_eq!((left, right), ([0.0; 4], [0.0; 4]));

        // An empty block uses up no ramp, and a reset ends one.
        plugin.set_param(1, 256.0);
        plugin.process(&mut [], &mut []);
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (0.0, 0.0));
        plugin.set_param(1, 0.0);
        plugin.reset();
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (0.0, 0.0));
        // A state starts where it is.
        plugin.set_state(&state(0, 256)).unwrap();
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (1.0, 1.0));
    }

    #[test]
    fn the_pan_ramps_the_crossfeed_in() {
        let mut plugin = Balance::default();
        plugin.set_param(0, -128.0);
        let (mut left, mut right) = ([0.0; 2], [1.0; 2]);
        plugin.process(&mut left, &mut right);
        // Right into left from 0 towards 0.7071, right to right from 1
        // towards 0.
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(left[0], 0.0, 1e-6) && close(left[1], half / 2.0, 1e-6));
        assert!(close(right[0], 1.0, 1e-6) && close(right[1], 0.5, 1e-6));
        assert_eq!(frame(&mut plugin, 0.0, 1.0).1, 0.0);
    }

    #[test]
    fn parameters_are_whole_numbers_inside_the_wheels_ranges() {
        let mut plugin = Balance::default();
        assert_eq!((plugin.param(0), plugin.param(1)), (0.0, 256.0));
        assert_eq!(plugin.param(2), 0.0);
        plugin.set_param(0, -500.0);
        plugin.set_param(1, 290.4);
        plugin.set_param(2, 1.0);
        assert_eq!((plugin.param(0), plugin.param(1)), (-128.0, 290.0));
        plugin.set_param(1, 999.0);
        assert_eq!(plugin.param(1), 320.0);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with(-64, 100);
        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&[0; 7]),
            Err(StateError::Length {
                expected: 8,
                found: 7
            })
        );
        assert_eq!(
            plugin.set_state(&state(129, 256)),
            Err(StateError::Range {
                param: 0,
                value: 129
            })
        );
        assert_eq!(
            plugin.set_state(&state(0, 321)),
            Err(StateError::Range {
                param: 1,
                value: 321
            })
        );
        assert_eq!(plugin, before);
    }
}
