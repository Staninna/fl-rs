//! Fruity Fast Dist: a table shaper with a gain before it and a mix and a
//! gain after it, as `docs/plugins/fruity-fast-dist.md` has it.
//!
//! The state is five little-endian i32, the plugin's five parameters:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Pre amp | 64 to 192 | 128 |
//! | 1 Threshold | 1 to 10 | 10 |
//! | 2 Distortion type | 0 (A) or 1 (B) | 0 |
//! | 3 Mix level | 0 to 128 | 128 |
//! | 4 Post gain | 0 to 128 | 128 |
//!
//! Pre, mix and post are gains on the knob curve `(11^(v / 128) - 1) / 10`.
//! The threshold picks one of ten curves of its type, and 10 is the
//! heaviest: type A at 1 is a straight line up to a hard clip.
//!
//! The plugin does not shape anything itself. It hands the block to FL's
//! `DistWave_32FM`, which looks every sample up in a table of 8192 int16
//! that FL builds at startup. [`FastDist`] builds the same table for its
//! threshold and type and does the same lookup, to the nearest entry. FL
//! also has a lookup that interpolates, behind a switch nothing was seen to
//! set, and that one is not here.

use crate::{Effect, StateError, knob_gain, knob_value, read_state};
use std::f64::consts::LN_10;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Fast Dist";

/// The range of each parameter's control.
const RANGES: [(i32, i32); 5] = [(64, 192), (1, 10), (0, 1), (0, 128), (0, 128)];
const DEFAULTS: [i32; 5] = [128, 10, 0, 128, 128];

/// The last index of a curve's table, which a full-scale sample lands on.
const TOP: usize = 8191;

/// Entry `i` of a curve, as FL's generator works it out in dB: what a
/// sample at `i / 8191` of full scale comes out as, where 32767 is full
/// scale.
fn entry(i: usize, threshold: i32, kind: i32) -> i16 {
    let x = i as f64 / TOP as f64;
    let t = f64::from(threshold);
    let db = if kind == 0 {
        // A compressor above -25 dB, at a ratio that runs from 1:1 at
        // threshold 1 to 10:1 at 10, with the level it loses made up.
        let x_db = if x > 0.0 {
            x.ln() * (20.0 / LN_10)
        } else {
            -10_000.0
        };
        let ratio = (11.0 - t) / 10.0;
        let lift = 1.6 * (t - 1.0);
        let y_db = if x_db >= -25.0 {
            (x_db + 25.0) * ratio - 25.0
        } else {
            x_db
        };
        if y_db <= -lift { y_db + lift } else { y_db }
    } else {
        // The attenuation in dB to a power below 1.
        let attenuation = if x > 0.0 {
            -(x.ln() * (20.0 / LN_10))
        } else {
            1000.0
        };
        -attenuation.powf(1.0 - 0.02 * t)
    };
    ((db * LN_10 * 0.05).exp() * 32767.0).round_ties_even() as i16
}

/// The gain of a pre, mix or post knob. FL has 128 down as 1 without
/// working it out, so that the default is 1 to the bit.
fn gain(value: i32) -> f32 {
    if value == 128 {
        1.0
    } else {
        knob_gain(f64::from(value) / 128.0) as f32
    }
}

/// Fruity Fast Dist. A new one has FL's defaults: type A at threshold 10,
/// all of it wet.
#[derive(Clone)]
pub struct FastDist {
    params: [i32; 5],
    /// What the plugin hands `DistWave_32FM`: the gain on the input as it
    /// is, the gain on the table's output, and the gain on the way into
    /// the table.
    dry: f32,
    wet: f32,
    mul: f32,
    /// The curve of the threshold and the type, [`TOP`]` + 1` entries.
    table: Vec<i16>,
}

impl Default for FastDist {
    fn default() -> Self {
        let mut plugin = FastDist {
            params: DEFAULTS,
            dry: 0.0,
            wet: 0.0,
            mul: 0.0,
            table: vec![0; TOP + 1],
        };
        plugin.fill();
        plugin.set(DEFAULTS);
        plugin
    }
}

impl FastDist {
    /// Build the table for the threshold and the type.
    fn fill(&mut self) {
        let [_, threshold, kind, _, _] = self.params;
        for (i, slot) in self.table.iter_mut().enumerate() {
            *slot = entry(i, threshold, kind);
        }
    }

    /// Take parameters that are inside their ranges. The table is built
    /// again only when the threshold or the type is another one.
    fn set(&mut self, params: [i32; 5]) {
        let curve_moved = params[1..3] != self.params[1..3];
        self.params = params;
        if curve_moved {
            self.fill();
        }
        let (pre, mix, post) = (gain(params[0]), gain(params[3]), gain(params[4]));
        self.mul = pre;
        self.wet = mix * post;
        // Post twice and pre once: it is what FL's code does, and it makes
        // the dry side `pre * post^2` at mix 0.
        self.dry = (post - self.wet) * pre * post;
    }
}

impl Effect for FastDist {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.set(read_state(bytes, &RANGES)?);
        Ok(())
    }

    /// Nothing here depends on the rate.
    fn set_sample_rate(&mut self, _hz: f32) {}

    /// Every sample on its own, so the two sides do not meet. A sample
    /// past full scale after the pre gain reads the last entry, which is
    /// the hard clip.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let scale = f64::from(self.mul) * TOP as f64;
        let (dry, wet) = (f64::from(self.dry), f64::from(self.wet) / 32768.0);
        for sample in left.iter_mut().chain(right.iter_mut()) {
            let x = f64::from(*sample);
            let index = (x * scale).round_ties_even() as i32;
            let shaped = f64::from(self.table[(index.unsigned_abs() as usize).min(TOP)]);
            // The curve is the same for both signs.
            let shaped = if index < 0 { -shaped } else { shaped };
            *sample = (x * dry + shaped * wet) as f32;
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// Moving the threshold or the type builds a table of 8192 entries
    /// before this returns.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    /// The shaper keeps nothing from one block to the next.
    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsp::gain_to_db;

    const TYPE_A: [i32; 5] = [128, 10, 0, 128, 128];
    const TYPE_B: [i32; 5] = [128, 10, 1, 128, 128];

    fn state(params: [i32; 5]) -> Vec<u8> {
        params
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    fn with(params: [i32; 5]) -> FastDist {
        let mut plugin = FastDist::default();
        plugin.set_state(&state(params)).unwrap();
        plugin
    }

    /// The left side of a block run through the plugin with the same block
    /// on the right.
    fn run(params: [i32; 5], input: &[f32]) -> Vec<f32> {
        let (mut left, mut right) = (input.to_vec(), input.to_vec());
        with(params).process(&mut left, &mut right);
        assert_eq!(left, right);
        left
    }

    /// The knob curve as the doc gives it.
    fn knob(value: i32) -> f32 {
        if value == 128 {
            1.0
        } else {
            ((11f64.powf(f64::from(value) / 128.0) - 1.0) / 10.0) as f32
        }
    }

    /// One sample by the doc's formulas, with no table: the gains from the
    /// parameters, the index the sample rounds to, that index's level
    /// through the type's curve in dB, and the dry and wet sum. `nudge`
    /// moves the index by that many entries.
    fn by_the_doc(x: f32, [pre, threshold, kind, mix, post]: [i32; 5], nudge: f64) -> f32 {
        let (pre, mix, post) = (knob(pre), knob(mix), knob(post));
        let wet = mix * post;
        let dry = (post - wet) * pre * post;
        let scale = f64::from(pre) * 8191.0;
        let index = (f64::from(x) * scale).round_ties_even().abs() + nudge;
        let index = index.clamp(0.0, 8191.0);
        let level_db = 20.0 * (index / 8191.0).log10();
        let t = f64::from(threshold);
        let out_db = if kind == 0 {
            let squashed = if level_db >= -25.0 {
                (level_db + 25.0) * (11.0 - t) / 10.0 - 25.0
            } else {
                level_db
            };
            let lift = 1.6 * (t - 1.0);
            if squashed <= -lift {
                squashed + lift
            } else {
                squashed
            }
        } else {
            -(-level_db).powf(1.0 - 0.02 * t)
        };
        let entry = (10f64.powf(out_db / 20.0) * 32767.0).round();
        let shaped = entry.copysign(f64::from(x)) / 32768.0;
        (f64::from(x) * f64::from(dry) + shaped * f64::from(wet)) as f32
    }

    /// Ten periods of a sine at -6 dBFS, 480 samples each.
    fn sine() -> Vec<f32> {
        let peak = 10f64.powf(-6.0 / 20.0);
        let phase = |n: usize| std::f64::consts::TAU * n as f64 / 480.0;
        (0..4800).map(|n| (peak * phase(n).sin()) as f32).collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |max, v| max.max(v.abs()))
    }

    fn rms(samples: &[f32]) -> f32 {
        let squares: f64 = samples.iter().map(|v| f64::from(*v).powi(2)).sum();
        (squares / samples.len() as f64).sqrt() as f32
    }

    /// The level of a harmonic of the sine in `samples`, as the peak of
    /// the partial.
    fn harmonic(samples: &[f32], number: usize) -> f32 {
        let turn = std::f64::consts::TAU * number as f64 / 480.0;
        let (mut re, mut im) = (0.0, 0.0);
        for (n, value) in samples.iter().enumerate() {
            re += f64::from(*value) * (turn * n as f64).cos();
            im += f64::from(*value) * (turn * n as f64).sin();
        }
        (2.0 * re.hypot(im) / samples.len() as f64) as f32
    }

    /// One table step at full scale, with room for a rounding that fell
    /// the other way.
    const STEP: f32 = 1.5 / 32768.0;

    /// Run a block through the plugin and hold every sample against
    /// [`by_the_doc`].
    ///
    /// With the pre gain off 128, the gain worked out here and the plugin's
    /// may differ in their last bit. That moves a sample that sits on the
    /// edge between two entries to the one next door, so either will do.
    fn assert_by_the_doc(params: [i32; 5], input: &[f32]) -> Vec<f32> {
        let nudges: &[f64] = if params[0] == 128 {
            &[0.0]
        } else {
            &[0.0, -1.0, 1.0]
        };
        let out = run(params, input);
        for (n, (got, x)) in out.iter().zip(input).enumerate() {
            let want = nudges.iter().map(|nudge| by_the_doc(*x, params, *nudge));
            assert!(
                want.clone().any(|want| (got - want).abs() <= STEP),
                "{params:?} sample {n}: {x} gave {got}, the doc says {:?}",
                want.collect::<Vec<_>>()
            );
        }
        out
    }

    #[test]
    fn the_defaults_squash_a_sine_at_minus_6_db_flat() {
        let input = sine();
        let out = assert_by_the_doc(TYPE_A, &input);
        let want: Vec<f32> = input.iter().map(|x| by_the_doc(*x, TYPE_A, 0.0)).collect();

        // 19 dB above the knee at 10:1 is 1.9 dB, less the 25 and plus the
        // 14.4 of make-up: -6 dB in is -8.7 dB out.
        assert!((gain_to_db(peak(&out)) + 8.7).abs() < 0.01);
        assert!((peak(&out) - peak(&want)).abs() <= STEP);
        assert!((rms(&out) - rms(&want)).abs() < 1e-5);
        // Everything above -25 dB comes out between -10.6 and -8.7 dB, so
        // the wave is close to square: a sine's peak is 1.41 times its RMS
        // and a square's is 1.
        let crest = peak(&out) / rms(&out);
        assert!(crest < 1.15, "crest {crest}");
        assert!(gain_to_db(rms(&out)) > -10.6 && gain_to_db(rms(&out)) < -8.7);

        let fundamental = harmonic(&out, 1);
        for number in 1..=9 {
            let (got, want) = (harmonic(&out, number), harmonic(&want, number));
            assert!((got - want).abs() < 1e-5, "harmonic {number}: {got}");
            // The curve is odd, so it adds odd harmonics only, and a wave
            // this square has them a little under a square's 1/3, 1/5 and
            // so on.
            let share = got / fundamental;
            if number % 2 == 0 {
                assert!(share < 1e-3, "harmonic {number}: {share}");
            } else {
                let square = 1.0 / number as f32;
                assert!(
                    share > 0.6 * square && share <= square,
                    "harmonic {number}: {share}"
                );
            }
        }
        // The sine had none of them.
        assert!(harmonic(&input, 3) < 1e-6);
        // A square of this height would have a fundamental of 4/pi times
        // it. This one is a little short of that.
        let square = 4.0 / std::f32::consts::PI * peak(&out);
        assert!(fundamental > 0.9 * square && fundamental < square);
    }

    #[test]
    fn type_b_bends_the_same_sine_less() {
        let input = sine();
        let a = run(TYPE_A, &input);
        let b = assert_by_the_doc(TYPE_B, &input);
        let want: Vec<f32> = input.iter().map(|x| by_the_doc(*x, TYPE_B, 0.0)).collect();
        assert!((rms(&b) - rms(&want)).abs() < 1e-5);
        for number in 1..=9 {
            let (got, want) = (harmonic(&b, number), harmonic(&want, number));
            assert!((got - want).abs() < 1e-5, "harmonic {number}: {got}");
        }
        // 6 dB of attenuation to the power 0.8 is 4.19 dB.
        assert!((gain_to_db(peak(&b)) + 6f32.powf(0.8)).abs() < 0.01);
        assert!((gain_to_db(peak(&b)) + 4.19).abs() < 0.01);
        // Louder at the top than type A, with a rounder wave and less of
        // every odd harmonic next to the fundamental.
        assert!(peak(&b) > 1.5 * peak(&a));
        assert!(peak(&b) / rms(&b) > 1.2);
        for number in [3, 5, 7] {
            let share = |out: &[f32]| harmonic(out, number) / harmonic(out, 1);
            assert!(share(&b) < 0.75 * share(&a), "harmonic {number}");
            assert!(share(&b) > 0.02, "harmonic {number}");
        }
        assert!(harmonic(&b, 2) / harmonic(&b, 1) < 1e-3);
    }

    #[test]
    fn the_curves_have_the_doc_s_levels() {
        let level = |params: [i32; 5], db: f32| {
            let x = 10f32.powf(db / 20.0);
            let out = run(params, &[x, -x]);
            assert_eq!(out[0], -out[1]);
            gain_to_db(out[0])
        };
        // Type A at 10: full scale leaves at -8.1 dB and the knee at
        // -10.6. Below the knee there is the make-up alone, 14.4 dB.
        assert!((level(TYPE_A, 0.0) + 8.1).abs() < 0.01);
        assert!((level(TYPE_A, -25.0) + 10.6).abs() < 0.05);
        assert!((level(TYPE_A, -40.0) + 25.6).abs() < 0.1);
        // Type B at 10: -20 dB comes out at -11 and -40 at -19. The doc
        // has -60 at -27, where 60^0.8 is 26.5.
        assert!((level(TYPE_B, -20.0) + 11.0).abs() < 0.05);
        assert!((level(TYPE_B, -40.0) + 19.1).abs() < 0.1);
        assert!((level(TYPE_B, -60.0) + 26.5).abs() < 0.2);
        assert!((level(TYPE_B, 0.0)).abs() < 0.001);
        // Silence stays silent on both.
        assert_eq!(run(TYPE_A, &[0.0, -0.0]), [0.0, 0.0]);
        assert_eq!(run(TYPE_B, &[0.0]), [0.0]);
    }

    #[test]
    fn threshold_1_of_type_a_is_a_straight_line_up_to_the_clip() {
        // The line is quantised to the table's 8192 steps on the way in
        // and to an int16 on the way out.
        let fine = 0.5 / 8191.0 + 2.0 / 32768.0;
        let input: Vec<f32> = (-1000..=1000).map(|n| n as f32 / 1000.0).collect();
        let out = assert_by_the_doc([128, 1, 0, 128, 128], &input);
        for (got, x) in out.iter().zip(&input) {
            assert!((got - x).abs() <= fine, "{x} gave {got}");
        }

        // With the gains off their defaults the output is the input times
        // them: the dry gain, and the pre gain times the wet gain.
        let params = [100, 1, 0, 96, 110];
        let (pre, mix, post) = (knob(100), knob(96), knob(110));
        let wet = mix * post;
        let dry = (post - wet) * pre * post;
        assert!(pre < 1.0 && wet > 0.1 && dry > 0.01);
        let out = assert_by_the_doc(params, &input);
        for (got, x) in out.iter().zip(&input) {
            let want = x * (dry + pre * wet);
            assert!((got - want).abs() <= fine * wet, "{x} gave {got}");
        }

        // The pre gain all the way up is 3.55, and what it pushes past
        // full scale is clipped there.
        let driven = run([192, 1, 0, 128, 128], &[0.2, 0.5, -0.9, 1.0 / 3.548]);
        assert!((driven[0] - 0.2 * knob(192)).abs() <= fine);
        assert_eq!(driven[1], 32767.0 / 32768.0);
        assert_eq!(driven[2], -32767.0 / 32768.0);
        assert!((driven[3] - 1.0).abs() < 1e-3);

        // Threshold 1 of type B is not a line.
        let bent = run([128, 1, 1, 128, 128], &[0.1]);
        assert!((bent[0] - 0.1).abs() > 0.004);
    }

    #[test]
    fn the_table_is_the_generator_s() {
        let table = |params: [i32; 5]| with(params).table;
        // Type A at threshold 1 is the identity on 32767.
        let line = table([128, 1, 0, 128, 128]);
        assert_eq!(line.len(), 8192);
        for (i, entry) in line.iter().enumerate() {
            let want = (i as f64 * 32767.0 / 8191.0).round();
            assert_eq!(f64::from(*entry), want, "entry {i}");
        }
        // Every curve starts at 0. Type A ends 2.5 dB down a step of the
        // threshold with 1.6 dB a step made up, and type B ends at full
        // scale.
        for threshold in 1..=10 {
            let a = table([128, threshold, 0, 128, 128]);
            let b = table([128, threshold, 1, 128, 128]);
            let steps = f64::from(threshold - 1);
            let top = (10f64.powf(-0.9 * steps / 20.0) * 32767.0).round();
            assert_eq!(
                (a[0], f64::from(a[8191])),
                (0, top),
                "threshold {threshold}"
            );
            assert_eq!((b[0], b[8191]), (0, 32767), "threshold {threshold}");
            assert!(a.windows(2).all(|pair| pair[0] <= pair[1]));
            assert!(b.windows(2).all(|pair| pair[0] <= pair[1]));
        }
        // 1/8191 of full scale is -78.3 dB: plus 14.4 on type A, and to
        // the power 0.8 on type B.
        let (a, b) = (table(TYPE_A), table(TYPE_B));
        let low_db = 20.0 * (1f64 / 8191.0).log10();
        assert_eq!(
            f64::from(a[1]),
            (10f64.powf((low_db + 14.4) / 20.0) * 32767.0).round()
        );
        assert_eq!(
            f64::from(b[1]),
            (10f64.powf(-(-low_db).powf(0.8) / 20.0) * 32767.0).round()
        );
    }

    #[test]
    fn the_mix_is_fl_s_dry_and_wet_gains() {
        // All wet at the default, where the dry gain is 0.
        let plugin = FastDist::default();
        assert_eq!((plugin.dry, plugin.wet, plugin.mul), (0.0, 1.0, 1.0));
        // Mix at 0 with the gains at 1 is the input as it is.
        let input = [0.3, -0.8, 1.5];
        assert_eq!(run([128, 10, 0, 0, 128], &input), input);
        // With the gains elsewhere the dry side is pre times post squared,
        // and not the input as it is.
        let out = run([192, 10, 0, 0, 64], &[0.5]);
        assert!((out[0] - 0.5 * knob(192) * knob(64) * knob(64)).abs() < 1e-7);
        assert!((knob(192) * knob(64) * knob(64) - 0.19).abs() < 0.001);
        // In between it is both, as the doc's formula has them.
        assert_by_the_doc([150, 7, 1, 57, 98], &sine());
        assert_by_the_doc([82, 7, 0, 128, 128], &sine());
        // The post gain at 0 is silence.
        assert_eq!(run([128, 10, 0, 128, 0], &[0.5, -0.5]), [0.0, 0.0]);
    }

    #[test]
    fn a_moved_parameter_is_the_same_plugin_as_its_state() {
        let mut plugin = FastDist::default();
        for (index, value) in [(0, 192.0), (1, 6.0), (2, 1.0), (3, 57.2), (4, 97.6)] {
            plugin.set_param(index, value);
        }
        let params = [192, 6, 1, 57, 98];
        assert_eq!(
            [0, 1, 2, 3, 4].map(|i| plugin.param(i)),
            params.map(|v| v as f32)
        );
        let loaded = with(params);
        assert!(plugin.table == loaded.table);
        assert_eq!(
            (plugin.dry, plugin.wet, plugin.mul),
            (loaded.dry, loaded.wet, loaded.mul)
        );
        // A value outside the control's range stops at its end.
        plugin.set_param(1, 0.0);
        plugin.set_param(2, 9.0);
        plugin.set_param(0, 1.0);
        plugin.set_param(5, 1.0);
        assert_eq!(
            (plugin.param(0), plugin.param(1), plugin.param(2)),
            (64.0, 1.0, 1.0)
        );
        assert_eq!(plugin.param(5), 0.0);
        assert!(plugin.table == with([64, 1, 1, 57, 98]).table);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([92, 8, 0, 91, 128]);
        assert_eq!(
            plugin.set_state(&[0; 8]),
            Err(StateError::Length {
                expected: 20,
                found: 8
            })
        );
        for (param, value) in [
            (0, 63),
            (0, 193),
            (1, 0),
            (1, 11),
            (2, 2),
            (3, 129),
            (4, -1),
        ] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&state(params)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(
            [0, 1, 2, 3, 4].map(|i| plugin.param(i)),
            [92.0, 8.0, 0.0, 91.0, 128.0]
        );
        assert!(plugin.table == with([92, 8, 0, 91, 128]).table);
    }
}
