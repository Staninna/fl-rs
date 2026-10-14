//! Fruity Soft Clipper: a knee that bends what is above a threshold under
//! full scale, and a gain after it, as `docs/plugins/fruity-soft-clipper.md`
//! has it.
//!
//! The state is two little-endian i32, the plugin's two parameters:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Threshold | 1 to 127 | 100 |
//! | 1 Post gain | 0 to 160 | 128 |
//!
//! The threshold is `T = v / 128` of full scale. A sample at or under it
//! passes as it is. One above it comes out at
//!
//! ```text
//! T + (1 - T) * (1 - e^(-(|x| - T) / (1 - T)))
//! ```
//!
//! with its sign, which leaves the knee at the slope it came in with and
//! never gets to full scale. The post gain is the knob curve
//! `(11^(v / 128) - 1) / 10` on what the knee gives.

use crate::{Effect, StateError, knob_gain, knob_value, read_state};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Soft Clipper";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 2] = [(1, 127), (0, 160)];
const DEFAULTS: [i32; 2] = [100, 128];

/// Fruity Soft Clipper. A new one has FL's defaults: the knee at 0.78 of
/// full scale and no gain after it.
#[derive(Debug, Clone, PartialEq)]
pub struct SoftClipper {
    params: [i32; 2],
    /// `T`, where the knee starts.
    threshold: f32,
    /// The gain after the knee.
    post: f32,
}

impl Default for SoftClipper {
    fn default() -> Self {
        let mut plugin = SoftClipper {
            params: DEFAULTS,
            threshold: 0.0,
            post: 0.0,
        };
        plugin.set(DEFAULTS);
        plugin
    }
}

impl SoftClipper {
    /// Take parameters that are inside their ranges.
    fn set(&mut self, params: [i32; 2]) {
        self.params = params;
        self.threshold = params[0] as f32 / 128.0;
        // FL has 128 down as 1 without working it out.
        self.post = if params[1] == 128 {
            1.0
        } else {
            knob_gain(f64::from(params[1]) / 128.0) as f32
        };
    }
}

impl Effect for SoftClipper {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.set(read_state(bytes, &RANGES)?);
        Ok(())
    }

    /// Nothing here depends on the rate.
    fn set_sample_rate(&mut self, _hz: f32) {}

    /// Every sample on its own, so the two sides do not meet.
    ///
    /// FL keeps `1 - T` from being 0 with a floor of 2^-16. The wheel stops
    /// at 127, where it is 1/128, so the floor is not here.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let threshold = f64::from(self.threshold);
        let room = 1.0 - threshold;
        for sample in left.iter_mut().chain(right.iter_mut()) {
            let x = f64::from(*sample);
            if x.abs() > threshold {
                let over = (x.abs() - threshold) / room;
                let bent = threshold - room * (-over).exp_m1();
                *sample = bent.copysign(x) as f32;
            }
            if self.post != 1.0 {
                *sample *= self.post;
            }
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    /// The knee keeps nothing from one block to the next.
    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, presets, state};
    use dsp::gain_to_db;

    fn with(threshold: i32, post: i32) -> SoftClipper {
        let mut plugin = SoftClipper::default();
        plugin.set_state(&state(&[threshold, post])).unwrap();
        plugin
    }

    /// A block through the plugin, the same on both sides.
    fn run(threshold: i32, post: i32, input: &[f32]) -> Vec<f32> {
        let (mut left, mut right) = (input.to_vec(), input.to_vec());
        with(threshold, post).process(&mut left, &mut right);
        assert_eq!(bits(&left), bits(&right));
        left
    }

    /// The doc's closed form for a sample above the threshold.
    fn knee(x: f64, threshold: i32) -> f64 {
        let t = f64::from(threshold) / 128.0;
        (t + (1.0 - t) * (1.0 - (-(x.abs() - t) / (1.0 - t)).exp())).copysign(x)
    }

    /// The doc's post gain.
    fn post(value: i32) -> f64 {
        if value == 128 {
            1.0
        } else {
            (11f64.powf(f64::from(value) / 128.0) - 1.0) / 10.0
        }
    }

    #[test]
    fn the_defaults_pass_a_stereo_signal_under_the_threshold_to_the_bit() {
        // 100 / 128 is where the knee starts, and a sample right on it
        // still passes.
        let left = [0.0, -0.0, 0.78125, -0.78125, 0.3, -1e-30, 0.78, -0.5];
        let right = [0.1, 0.2, -0.7, 0.75, f32::MIN_POSITIVE, 0.0, -0.78124, 0.6];
        for mut plugin in [SoftClipper::default(), with(100, 128)] {
            let (mut l, mut r) = (left, right);
            plugin.process(&mut l, &mut r);
            assert_eq!(bits(&l), bits(&left));
            assert_eq!(bits(&r), bits(&right));
        }
    }

    #[test]
    fn above_the_threshold_the_excess_is_squashed_on_the_doc_s_curve() {
        let input: Vec<f32> = (0..=400).map(|n| (n - 200) as f32 / 50.0).collect();
        let out = run(100, 128, &input);
        for (got, x) in out.iter().zip(&input) {
            if x.abs() <= 0.78125 {
                assert_eq!(got, x);
                continue;
            }
            let want = knee(f64::from(*x), 100);
            assert!((f64::from(*got) - want).abs() < 1e-7, "{x} gave {got}");
            // Less than what came in, more than the threshold, and never
            // past full scale.
            assert!(got.abs() < x.abs() && got.abs() > 0.78125 && got.abs() <= 1.0);
            assert_eq!(got.signum(), x.signum());
        }
        // A louder sample still comes out louder.
        let rising = &out[200..];
        assert!(rising.windows(2).all(|pair| pair[0] <= pair[1]));

        // Full scale comes out at 0.92, 0.7 dB down, and +6 dB at 0.999.
        let [full, hot] = run(100, 128, &[1.0, -2.0])[..] else {
            unreachable!()
        };
        assert!((full - 0.92).abs() < 0.001 && (gain_to_db(full) + 0.7).abs() < 0.03);
        assert!((hot + 0.999).abs() < 0.0005);
        // However hot the input, the output stops at full scale.
        assert_eq!(run(100, 128, &[1e6, f32::NEG_INFINITY]), [1.0, -1.0]);
    }

    #[test]
    fn the_two_sides_are_clipped_apart() {
        let mut plugin = SoftClipper::default();
        let (mut left, mut right) = ([1.5, 0.2], [0.2, -1.5]);
        plugin.process(&mut left, &mut right);
        let bent = knee(1.5, 100) as f32;
        assert_eq!((left, right), ([bent, 0.2], [0.2, -bent]));
    }

    #[test]
    fn the_knee_starts_at_the_slope_of_the_line_under_it() {
        // Just above the threshold the curve is `T + u - u^2 / 2 (1 - T)`
        // for an excess of `u`: the line, less a little.
        for threshold in [1, 64, 100, 127] {
            let t = threshold as f32 / 128.0;
            let u = 0.05 * (1.0 - t);
            let got = run(threshold, 128, &[t + u])[0];
            let lost = f64::from(t + u) - f64::from(got);
            let want = f64::from(u).powi(2) / (2.0 * f64::from(1.0 - t));
            assert!(
                lost > 0.0 && (lost - want).abs() < 0.05 * want + 1e-7,
                "threshold {threshold}: {lost} and not {want}"
            );
        }
    }

    #[test]
    fn the_lowest_threshold_saturates_from_nearly_nothing() {
        // T = 1/128: close to `1 - e^-|x|`, and 0.635 at full scale, 4 dB
        // down.
        let out = run(1, 128, &[1.0, -0.25, 0.005]);
        assert!((out[0] - 0.635).abs() < 0.001 && (gain_to_db(out[0]) + 4.0).abs() < 0.1);
        assert!((f64::from(out[0]) - knee(1.0, 1)).abs() < 1e-7);
        assert!((f64::from(out[1]) - knee(-0.25, 1)).abs() < 1e-7);
        assert!((f64::from(out[1]) + (1.0 - (-0.25f64).exp())).abs() < 0.005);
        assert_eq!(out[2], 0.005);

        // T = 127/128 is close to a hard clip: 0.992 passes, and what is
        // above it has 1/128 left to fit in.
        let out = run(127, 128, &[0.99, 1.0, 3.0]);
        assert_eq!(out[0], 0.99);
        assert!((f64::from(out[1]) - knee(1.0, 127)).abs() < 1e-7);
        assert!(out[1] > 0.992 && out[1] < 0.9972);
        assert!(out[2] > 0.9999 && out[2] <= 1.0);
    }

    #[test]
    fn the_post_gain_is_the_knob_curve_after_the_knee() {
        // 150 is 1.56, +3.9 dB, and 160 is 1.90, +5.6 dB.
        assert!((post(150) - 1.56).abs() < 0.005 && (post(160) - 1.90).abs() < 0.005);
        for value in [1, 64, 127, 129, 150, 160] {
            let out = run(100, value, &[0.5, -1.5]);
            let gain = post(value);
            assert!(
                (f64::from(out[0]) - 0.5 * gain).abs() < 1e-6,
                "post {value}"
            );
            assert!((f64::from(out[1]) - knee(-1.5, 100) * gain).abs() < 1e-6);
        }
        assert!((gain_to_db(run(100, 160, &[0.5])[0] / 0.5) - 5.6).abs() < 0.02);
        // The knee does not see the gain: a sample under the threshold is
        // not bent however far the gain lifts it.
        assert!((f64::from(run(100, 160, &[0.75])[0]) - 0.75 * post(160)).abs() < 1e-6);
        // All the way down is silence.
        assert_eq!(run(100, 0, &[0.5, -2.0]), [0.0, -0.0]);
    }

    #[test]
    fn a_moved_parameter_is_the_same_plugin_as_its_state() {
        let mut plugin = SoftClipper::default();
        assert_eq!((plugin.param(0), plugin.param(1)), (100.0, 128.0));
        plugin.set_param(0, 31.6);
        plugin.set_param(1, 140.2);
        plugin.set_param(2, 5.0);
        assert_eq!(plugin, with(32, 140));
        assert_eq!(plugin.param(2), 0.0);
        // A value outside the wheel's range stops at its end.
        plugin.set_param(0, 0.0);
        plugin.set_param(1, 500.0);
        assert_eq!((plugin.param(0), plugin.param(1)), (1.0, 160.0));
        plugin.set_param(0, 128.0);
        assert_eq!(plugin.param(0), 127.0);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with(83, 141);
        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&[0; 12]),
            Err(StateError::Length {
                expected: 8,
                found: 12
            })
        );
        for (param, value) in [(0, 0), (0, 128), (1, -1), (1, 161)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&state(&params)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_preset_is_the_default() {
        let Some(presets) = presets(NAME) else { return };
        assert_eq!(presets.len(), 1);
        let preset = &presets[0];
        assert_eq!(preset.file, "Default");
        // The preset has the name in lower case.
        assert_eq!(preset.plugin, "Fruity soft clipper");
        assert_eq!(preset.state, state(&DEFAULTS));
        let mut plugin = with(1, 0);
        plugin.set_state(&preset.state).unwrap();
        assert_eq!(plugin, SoftClipper::default());
    }
}
