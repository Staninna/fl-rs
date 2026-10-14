//! Fruity Filter: a state variable filter with a gain on each of its three
//! outputs, as `docs/plugins/fruity-filter.md` has it.
//!
//! The state is a little-endian i32 version, the plugin's six parameters
//! as i32, and from version 2 on a byte for Center:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Cutoff frequency | 0 to 1024 | 1024 |
//! | 1 Resonance | 0 to 1024 | 205 |
//! | 2 Low pass | 0 to 1024 | 1024 |
//! | 3 Band pass | 0 to 1024 | 0 |
//! | 4 High pass | 0 to 1024 | 512 |
//! | 5 X 2 | 0 or 1 | 0 |
//!
//! Version 1 is 28 bytes and loads with Center off. Version 2 is 29, and
//! FL writes it with Center on unless it was switched off. Center is not a
//! parameter, so only a state sets it.
//!
//! The filter is Chamberlin's. Its corner is at `(rate / pi) * asin(f / 2)`
//! for a coefficient `f` that the cutoff wheel gives with no look at the
//! rate, so the same wheel is a higher corner at a higher rate: wide open
//! it is a sixth of the rate, 7.35 kHz at 44.1 kHz and 16 kHz at 96 kHz.
//! That is what FL does, and what is here.

use crate::parts::{Follower, slew};
use crate::{Effect, StateError, knob_gain, knob_value, read_state, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Filter";

/// The range of each parameter's control.
const RANGES: [(i32, i32); 6] = [
    (0, 1024),
    (0, 1024),
    (0, 1024),
    (0, 1024),
    (0, 1024),
    (0, 1),
];
const DEFAULTS: [i32; 6] = [1024, 205, 1024, 0, 512, 0];

/// How long each version of the state is.
const V1_LEN: usize = 28;
const V2_LEN: usize = 29;

/// What Center gives its follower: `1.0070235 / rate` to go after the
/// input with, and 0.96 of its speed kept. Together they are a corner of
/// 4 Hz at any rate.
const CENTER_PACE: f64 = 1.007_023_5;
const CENTER_KEEP: f32 = 0.96;

/// A filter state smaller than this is 0 after the block.
const QUIET: f32 = 1.0 / 16_777_216.0;

/// What the parameters make the filter's five coefficients: `f`, which
/// sets the corner, `q`, the damping that the resonance wheel lowers, and
/// the gains of the low, band and high outputs.
///
/// `f` is `(26^x - 1) * 0.04` and ends on 1. `q` is `1 - 0.8 x`, from 1
/// down to 0.2. The gains are the knob curve `(11^x - 1) / 10`.
pub fn coefficients(params: &[i32; 6]) -> [f32; 5] {
    let x = |index: usize| f64::from(params[index]) / 1024.0;
    [
        (26f64.powf(x(0)) - 1.0) * 0.04,
        1.0 - 0.8 * x(1),
        knob_gain(x(2)),
        knob_gain(x(3)),
        knob_gain(x(4)),
    ]
    .map(|coefficient| coefficient as f32)
}

/// One filter of one side.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Stage {
    low: f32,
    band: f32,
}

impl Stage {
    /// Take one sample and give the three outputs at their gains.
    fn step(&mut self, x: f32, f: f32, q: f32, [low, band, high]: [f32; 3]) -> f32 {
        self.low += f * self.band;
        let out = x - self.low - q * self.band;
        self.band += f * out;
        self.low * low + self.band * band + out * high
    }

    fn flush(&mut self) {
        for state in [&mut self.low, &mut self.band] {
            if state.abs() < QUIET {
                *state = 0.0;
            }
        }
    }
}

/// Fruity Filter. A new one has FL's defaults: wide open, with a quarter
/// of the high output mixed in and Center on.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    params: [i32; 6],
    /// Whether the offset is taken off the output.
    center: bool,
    /// The coefficients the parameters ask for.
    target: [f32; 5],
    /// The coefficients the last block ended on.
    current: [f32; 5],
    /// Each side's filter and the second one that X 2 runs after it.
    stages: [[Stage; 2]; 2],
    /// Each side's offset, as Center follows it.
    offset: [Follower; 2],
    /// [`CENTER_PACE`] over the rate.
    pace: f32,
}

impl Default for Filter {
    fn default() -> Self {
        let target = coefficients(&DEFAULTS);
        Filter {
            params: DEFAULTS,
            center: true,
            target,
            current: target,
            stages: Default::default(),
            offset: Default::default(),
            pace: (CENTER_PACE / 44_100.0) as f32,
        }
    }
}

impl Filter {
    /// Take parameters that are inside their ranges. Switching X 2 empties
    /// the filters.
    fn set(&mut self, params: [i32; 6]) {
        if params[5] != self.params[5] {
            self.stages = Default::default();
        }
        self.params = params;
        self.target = coefficients(&params);
    }
}

impl Effect for Filter {
    /// The coefficients are the state's from the first sample on, with no
    /// slew.
    ///
    /// FL reads a version under 2 as 28 bytes, and so does this. It never
    /// writes one under 1.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, V2_LEN)?;
        if version > 2 {
            return Err(StateError::Version { found: version });
        }
        let expected = if version == 2 { V2_LEN } else { V1_LEN };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        self.set(read_state(&bytes[4..V1_LEN], &RANGES)?);
        self.current = self.target;
        self.center = bytes.get(V1_LEN).is_some_and(|byte| byte & 1 == 1);
        Ok(())
    }

    /// Only Center looks at the rate. As in FL, giving the rate starts its
    /// follower from nothing. A rate that is not a number above 0 is not
    /// taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.pace = (CENTER_PACE / f64::from(hz)) as f32;
            self.offset = Default::default();
        }
    }

    /// After a parameter has moved, each coefficient slews to where it was
    /// put: over this block when it is 128 frames or longer, and at the
    /// pace of 128 frames when it is shorter.
    ///
    /// The second filter of X 2 has the first one's `f` and gains and a
    /// `q` of 1, so it steepens the slope and leaves the resonance alone.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let mut now = [0.0; 5];
        let mut step = [0.0; 5];
        for k in 0..5 {
            (now[k], step[k]) = slew(&mut self.current[k], self.target[k], len);
        }
        let moving = step != [0.0; 5];
        let twice = self.params[5] == 1;

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if moving {
                for (coefficient, step) in now.iter_mut().zip(step) {
                    *coefficient += step;
                }
            }
            let [f, q, gains @ ..] = now;
            for (sample, [first, second]) in [l, r].into_iter().zip(&mut self.stages) {
                *sample = first.step(*sample, f, q, gains);
                if twice {
                    *sample = second.step(*sample, f, 1.0, gains);
                }
            }
        }
        for stage in self.stages.as_flattened_mut() {
            stage.flush();
        }

        if self.center {
            let (pace, keep) = (f64::from(self.pace), f64::from(CENTER_KEEP));
            for (side, offset) in [left, right].into_iter().zip(&mut self.offset) {
                for sample in &mut side[..len] {
                    let x = f64::from(*sample);
                    *sample = (x - offset.step(x, pace, keep)) as f32;
                }
                offset.flush();
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

    /// Empties the filters and Center's follower and ends a slew. FL
    /// itself keeps all three when the song stops, so that a filter rings
    /// on from where it was.
    fn reset(&mut self) {
        self.stages = Default::default();
        self.offset = Default::default();
        self.current = self.target;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{partial, presets, response, sine, state};
    use dsp::gain_to_db;
    use std::f64::consts::PI;

    const OPEN_LOW_PASS: [i32; 6] = [1024, 205, 1024, 0, 0, 0];

    /// A version 1 state: no Center.
    fn v1(params: [i32; 6]) -> Vec<u8> {
        let mut bytes = state(&[1]);
        bytes.extend(state(&params));
        bytes
    }

    /// A version 2 state.
    fn v2(params: [i32; 6], center: u8) -> Vec<u8> {
        let mut bytes = state(&[2]);
        bytes.extend(state(&params));
        bytes.push(center);
        bytes
    }

    fn with(bytes: &[u8]) -> Filter {
        let mut plugin = Filter::default();
        plugin.set_state(bytes).unwrap();
        plugin
    }

    /// The doc's three curves.
    fn f(value: i32) -> f64 {
        (26f64.powf(f64::from(value) / 1024.0) - 1.0) * 0.04
    }

    fn q(value: i32) -> f64 {
        1.0 - 0.8 * f64::from(value) / 1024.0
    }

    fn mix(value: i32) -> f64 {
        (11f64.powf(f64::from(value) / 1024.0) - 1.0) / 10.0
    }

    /// The doc's corner for a cutoff, in turns a sample: `asin(f / 2) / pi`.
    fn corner(cutoff: i32) -> f64 {
        (f(cutoff) / 2.0).asin() / PI
    }

    /// The gain of one filter at `turns` a sample, from the doc's loop
    /// written as a transfer function. With `d = z^-1`:
    ///
    /// ```text
    /// low  = f^2 d       / den
    /// band = f (1 - d)   / den        den = 1 - (2 - f q - f^2) d + (1 - f q) d^2
    /// high = (1 - d)^2   / den
    /// ```
    fn stage(params: [i32; 6], q: f64, turns: f64) -> f64 {
        let f = f(params[0]);
        let (low, band, high) = (mix(params[2]), mix(params[3]), mix(params[4]));
        let num = [band * f + high, low * f * f - band * f - 2.0 * high, high];
        let den = [1.0, -(2.0 - f * q - f * f), 1.0 - f * q];
        response(&num, &den, turns)
    }

    /// The gain the doc gives the whole plugin, Center aside.
    fn by_the_doc(params: [i32; 6], turns: f64) -> f64 {
        let first = stage(params, q(params[1]), turns);
        if params[5] == 1 {
            first * stage(params, 1.0, turns)
        } else {
            first
        }
    }

    /// The gain the doc gives Center at `turns` a sample, with
    /// `A = 1.0070235 / rate` and `B = 0.96`:
    /// `(1 - z^-1) (1 - B z^-1) / (1 - (1 + B - A) z^-1 + B z^-2)`.
    fn center(turns: f64, hz: f64) -> f64 {
        let (a, b) = (1.007_023_5 / hz, 0.96);
        response(&[1.0, -(1.0 + b), b], &[1.0, -(1.0 + b - a), b], turns)
    }

    /// The gain the plugin has for a tone that turns `cycles` times in
    /// `window` samples, once it has settled. Both sides get the tone.
    fn measured(plugin: &mut Filter, cycles: usize, window: usize) -> f64 {
        let turns = cycles as f64 / window as f64;
        let settle = window * 40_000usize.div_ceil(window);
        let mut left = sine(turns, settle + window);
        let mut right = left.clone();
        plugin.process(&mut left, &mut right);
        assert!(left == right);
        partial(&left[settle..], turns)
    }

    fn close(got: f64, want: f64, part: f64) -> bool {
        (got - want).abs() <= part * want.abs()
    }

    #[test]
    fn the_coefficients_are_the_doc_s_curves() {
        for params in [
            DEFAULTS,
            [0, 0, 0, 0, 0, 0],
            [160, 1024, 864, 0, 0, 0],
            [480, 1024, 0, 0, 1024, 1],
            [544, 0, 0, 1024, 464, 1],
            [1, 1023, 37, 512, 999, 0],
        ] {
            let got = coefficients(&params);
            let want = [
                f(params[0]),
                q(params[1]),
                mix(params[2]),
                mix(params[3]),
                mix(params[4]),
            ];
            for (got, want) in got.iter().zip(want) {
                assert!((f64::from(*got) - want).abs() < 1e-7, "{params:?}");
            }
        }
        // f is 0 at the bottom, 0.164 half way and 1 to the bit at the
        // top. q is 1 with no resonance, 0.84 at the default and 0.2 at
        // the top. A mix is 0.23 half way and 1 at the top.
        assert_eq!(coefficients(&[0; 6]), [0.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(coefficients(&[1024; 6]), [1.0, 0.2, 1.0, 1.0, 1.0]);
        let half = coefficients(&[512, 205, 512, 0, 0, 0]);
        assert!((half[0] - 0.164).abs() < 5e-4);
        assert!((half[1] - 0.84).abs() < 5e-4);
        assert!((half[2] - 0.23).abs() < 5e-3);
        // The doc's table of presets: f and the corner at 44.1 kHz.
        for (cutoff, f_want, hz) in [
            (1024, 1.0, 7350.0),
            (160, 0.0266, 187.0),
            (480, 0.144, 1010.0),
            (544, 0.186, 1310.0),
        ] {
            assert!((f(cutoff) - f_want).abs() < 5e-4, "cutoff {cutoff}");
            assert!(
                close(corner(cutoff) * 44_100.0, hz, 0.005),
                "cutoff {cutoff}"
            );
        }
    }

    #[test]
    fn wide_open_the_corner_is_a_sixth_of_the_rate_whatever_the_rate() {
        // asin(1 / 2) / pi is a sixth of a turn a sample.
        assert!((corner(1024) - 1.0 / 6.0).abs() < 1e-15);
        assert!(close(corner(1024) * 44_100.0, 7350.0, 1e-9));
        assert!(close(corner(1024) * 96_000.0, 16_000.0, 1e-9));

        let mut heard = Vec::new();
        for hz in [44_100.0, 48_000.0, 96_000.0] {
            let mut plugin = with(&v1(OPEN_LOW_PASS));
            plugin.set_sample_rate(hz);
            // At its corner a low pass has the gain 1 / q.
            let at_corner = measured(&mut plugin, 1, 6);
            assert!(close(at_corner, 1.0 / q(205), 1e-4), "{at_corner}");
            assert!(close(at_corner, by_the_doc(OPEN_LOW_PASS, 1.0 / 6.0), 1e-4));
            // Under it the tone passes, and above it there is less of it.
            for (cycles, window) in [(1, 60), (1, 12), (1, 4), (9, 20)] {
                let got = measured(&mut plugin, cycles, window);
                let want = by_the_doc(OPEN_LOW_PASS, cycles as f64 / window as f64);
                assert!(close(got, want, 1e-4), "{cycles}/{window}: {got}");
            }
            assert!(close(measured(&mut plugin, 1, 60), 1.0, 0.01));
            assert!(measured(&mut plugin, 9, 20) < 0.8);

            // The same samples come out at every rate.
            let (mut left, mut right) = (sine(0.11, 500), sine(0.31, 500));
            plugin.reset();
            plugin.process(&mut left, &mut right);
            heard.push((left, right));
        }
        assert!(heard[0] == heard[1] && heard[0] == heard[2]);
    }

    #[test]
    fn the_default_is_a_low_pass_that_takes_the_top_down() {
        let mut plugin = Filter::default();
        assert_eq!(plugin, with(&v2(DEFAULTS, 1)));
        assert_eq!(
            [0, 1, 2, 3, 4, 5].map(|i| plugin.param(i)),
            [1024.0, 205.0, 1024.0, 0.0, 512.0, 0.0]
        );

        // A low tone on the left and a high one on the right: 735 Hz and
        // 19.8 kHz at 44.1 kHz.
        let (low, high) = (1.0 / 60.0, 9.0 / 20.0);
        let (mut left, mut right) = (sine(low, 42_000), sine(high, 42_000));
        plugin.process(&mut left, &mut right);
        let (low_gain, high_gain) = (
            partial(&left[36_000..], low),
            partial(&right[36_000..], high),
        );
        // The default has Center on, which is in the way of neither.
        for (got, turns) in [(low_gain, low), (high_gain, high)] {
            let want = by_the_doc(DEFAULTS, turns) * center(turns, 44_100.0);
            assert!(close(got, want, 1e-4), "{got} and not {want}");
            assert!(close(center(turns, 44_100.0), 1.0, 2e-3));
        }
        assert!(gain_to_db(low_gain as f32).abs() < 0.02, "{low_gain}");
        assert!(gain_to_db(high_gain as f32) < -20.0, "{high_gain}");

        // At the corner, a sixth of the rate, the high output is the low
        // one upside down, so the gain there is the low mix less the high
        // mix, over q.
        plugin.reset();
        let at_corner = measured(&mut plugin, 1, 6);
        let want = (1.0 - mix(512)) / q(205) * center(1.0 / 6.0, 44_100.0);
        assert!(close(at_corner, want, 1e-4), "{at_corner}");

        // It is the high mix that takes the top down. The low output on
        // its own, with f at 1, loses 2 dB there.
        let alone = measured(&mut with(&v2(OPEN_LOW_PASS, 1)), 9, 20);
        assert!(gain_to_db(alone as f32) > -2.5, "{alone}");
    }

    #[test]
    fn a_lower_cutoff_moves_the_corner_and_takes_a_high_tone_down() {
        let window = 44_100;
        for cutoff in [160, 480, 544, 768] {
            let params = [cutoff, 205, 1024, 0, 0, 0];
            let mut plugin = with(&v1(params));
            // The tone with a whole number of periods that is nearest to
            // the corner.
            let cycles = (corner(cutoff) * window as f64).round() as usize;
            let got = measured(&mut plugin, cycles, window);
            assert!(close(
                got,
                by_the_doc(params, cycles as f64 / window as f64),
                1e-3
            ));
            assert!(close(got, 1.0 / q(205), 0.01), "cutoff {cutoff}: {got}");

            // Two octaves up there is a fifteenth left under a low corner
            // and a tenth under a high one, and with X 2 that much again.
            let turns = 4.0 * cycles as f64 / window as f64;
            let got = measured(&mut plugin, 4 * cycles, window);
            assert!(close(got, by_the_doc(params, turns), 1e-3));
            assert!(got < 1.0 / 9.0, "cutoff {cutoff}: {got}");

            let params = [cutoff, 205, 1024, 0, 0, 1];
            let twice = measured(&mut with(&v1(params)), 4 * cycles, window);
            assert!(close(twice, by_the_doc(params, turns), 1e-3));
            assert!(twice < got / 9.0, "cutoff {cutoff}: {twice}");
        }
    }

    #[test]
    fn the_three_outputs_mix_as_the_doc_s_loop_has_them() {
        for params in [
            // The presets: Party next door, Retro sound, Tiny speakers.
            [160, 1024, 864, 0, 0, 0],
            [480, 1024, 0, 0, 1024, 1],
            [544, 0, 0, 1024, 464, 1],
            [600, 300, 1024, 1024, 1024, 0],
            [900, 700, 300, 1024, 0, 1],
        ] {
            let mut plugin = with(&v1(params));
            for (cycles, window) in [(1, 300), (1, 43), (1, 12), (1, 5), (3, 7)] {
                let got = measured(&mut plugin, cycles, window);
                let want = by_the_doc(params, cycles as f64 / window as f64);
                assert!(
                    (got - want).abs() < 1e-3 * want + 1e-6,
                    "{params:?} at {cycles}/{window}: {got} and not {want}"
                );
            }
        }
        // The high output has nothing of a level that stands still, and
        // the low output has all of it.
        for (params, want) in [
            ([480, 0, 0, 0, 1024, 0], 0.0),
            ([480, 0, 1024, 0, 0, 0], 0.5),
        ] {
            let (mut left, mut right) = ([0.5; 4000], [0.5; 4000]);
            with(&v1(params)).process(&mut left, &mut right);
            assert!((left[3999] - want).abs() < 1e-6 && left == right);
        }
        // Party next door's low mix is 0.66.
        assert!((mix(864) - 0.66).abs() < 0.005);
    }

    #[test]
    fn center_takes_an_offset_off_with_a_corner_of_4_hz_at_any_rate() {
        let params = [1024, 0, 1024, 0, 0, 0];
        for hz in [44_100usize, 48_000, 96_000] {
            let mut plugin = with(&v2(params, 1));
            plugin.set_sample_rate(hz as f32);
            // A level that stands still is gone within a second.
            let (mut left, mut right) = (vec![0.5; hz], vec![-0.25; hz]);
            plugin.process(&mut left, &mut right);
            assert!(left[hz - 1].abs() < 1e-6 && right[hz - 1].abs() < 1e-6);
            assert!(left[10] > 0.4 && right[10] < -0.2);

            // The doc's transfer function, and its reading of it: a high
            // pass of one pole with its corner at 4 Hz. That is right to
            // 2 %. The other pole and the zero next to it do not cancel in
            // full, and lift what is between the corner and a few hundred
            // Hz by up to 1.4 %.
            for tone in [4, 40, 400] {
                let turns = tone as f64 / hz as f64;
                let mut left = sine(turns, 3 * hz);
                let mut right = left.clone();
                plugin.reset();
                plugin.process(&mut left, &mut right);
                let got = partial(&left[2 * hz..], turns);
                let want = center(turns, hz as f64);
                assert!(close(got, want, 1e-3), "{tone} Hz at {hz}: {got}");
                let one_pole = tone as f64 / (tone as f64).hypot(4.0);
                assert!(close(got, one_pole, 0.02), "{tone} Hz at {hz}: {got}");
            }
        }

        // A rate that is not one leaves Center as it is.
        let mut plugin = with(&v2(params, 1));
        let pace = plugin.pace;
        plugin.set_sample_rate(0.0);
        plugin.set_sample_rate(f32::NAN);
        assert_eq!(plugin.pace, pace);

        // A version 1 state has no Center, and the level stays.
        let mut plugin = with(&v1(params));
        let (mut left, mut right) = (vec![0.5; 44_100], vec![-0.25; 44_100]);
        plugin.process(&mut left, &mut right);
        assert!((left[44_099] - 0.5).abs() < 1e-6 && (right[44_099] + 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_moved_coefficient_slews_over_128_samples() {
        let mut plugin = with(&v1([0, 0, 1024, 0, 0, 0]));
        assert_eq!(plugin.current, [0.0, 1.0, 1.0, 0.0, 0.0]);
        plugin.set_param(0, 1024.0);
        plugin.set_param(2, 0.0);
        assert_eq!(plugin.target, [1.0, 1.0, 0.0, 0.0, 0.0]);
        // A block of 32 goes a quarter of the way.
        plugin.process(&mut [0.0; 32], &mut [0.0; 32]);
        assert_eq!(plugin.current, [0.25, 1.0, 0.75, 0.0, 0.0]);
        // The next one a quarter of what is left.
        plugin.process(&mut [0.0; 32], &mut [0.0; 32]);
        assert_eq!(plugin.current, [0.4375, 1.0, 0.5625, 0.0, 0.0]);
        // An empty one nowhere, and one of 128 all the way.
        plugin.process(&mut [], &mut []);
        assert_eq!(plugin.current, [0.4375, 1.0, 0.5625, 0.0, 0.0]);
        plugin.process(&mut [0.0; 128], &mut [0.0; 128]);
        assert_eq!(plugin.current, plugin.target);

        // The gain of the low output on its way down, heard on a level
        // that the filter passes as it is: every frame has the gain of
        // the frame before less 1/128.
        let mut plugin = with(&v1([1024, 0, 1024, 0, 0, 0]));
        let (mut left, mut right) = ([1.0; 64], [1.0; 64]);
        plugin.process(&mut left, &mut right);
        assert!((left[63] - 1.0).abs() < 1e-6);
        plugin.set_param(2, 0.0);
        let (mut left, mut right) = ([1.0; 64], [1.0; 64]);
        plugin.process(&mut left, &mut right);
        for (n, got) in left.iter().enumerate() {
            let want = 1.0 - (n + 1) as f32 / 128.0;
            assert!((got - want).abs() < 1e-6, "frame {n}: {got}");
        }
        assert_eq!(left, right);

        // A state does not slew.
        plugin.set_state(&v1([512, 512, 0, 1024, 0, 0])).unwrap();
        assert_eq!(plugin.current, plugin.target);
    }

    #[test]
    fn switching_x_2_and_a_reset_empty_the_filters() {
        let ringing = || {
            let mut plugin = with(&v1([300, 1024, 1024, 0, 0, 0]));
            plugin.process(&mut [1.0; 100], &mut [-1.0; 100]);
            plugin
        };
        let tail = |plugin: &mut Filter| {
            let (mut left, mut right) = ([0.0; 16], [0.0; 16]);
            plugin.process(&mut left, &mut right);
            (left, right)
        };
        let (left, right) = tail(&mut ringing());
        assert!(left.iter().all(|v| *v > 0.1) && right.iter().all(|v| *v < -0.1));

        let mut plugin = ringing();
        plugin.set_param(5, 1.0);
        assert_eq!(tail(&mut plugin), ([0.0; 16], [0.0; 16]));
        // Any other parameter leaves them as they are.
        let mut plugin = ringing();
        plugin.set_param(3, 0.0);
        plugin.set_param(5, 0.0);
        assert_eq!(tail(&mut plugin), (left, right));
        let mut plugin = ringing();
        plugin.reset();
        assert_eq!(tail(&mut plugin), ([0.0; 16], [0.0; 16]));
    }

    #[test]
    fn a_moved_parameter_is_the_same_plugin_as_its_state() {
        let mut plugin = Filter::default();
        for (index, value) in [(0, 480.2), (1, 1023.6), (2, -3.0), (4, 2000.0), (5, 1.0)] {
            plugin.set_param(index, value);
        }
        plugin.set_param(6, 1.0);
        assert_eq!(plugin.param(6), 0.0);
        plugin.process(&mut [0.0; 128], &mut [0.0; 128]);
        assert_eq!(plugin, with(&v2([480, 1024, 0, 0, 1024, 1], 1)));
    }

    #[test]
    fn both_versions_of_the_state_load() {
        let params = [134, 235, 1024, 0, 0, 1];
        // 28 bytes of version 1, with Center off.
        let old = with(&v1(params));
        assert_eq!(v1(params).len(), 28);
        assert!(!old.center);
        // 29 bytes of version 2, with Center as its byte's lowest bit.
        assert_eq!(v2(params, 1).len(), 29);
        assert!(with(&v2(params, 1)).center && with(&v2(params, 3)).center);
        assert!(!with(&v2(params, 0)).center && !with(&v2(params, 2)).center);
        assert_eq!(with(&v2(params, 0)), old);
        assert_eq!(with(&v2(params, 1)).params, params);
        // FL reads any version under 2 as version 1.
        let mut ancient = v1(params);
        ancient[..4].copy_from_slice(&0i32.to_le_bytes());
        assert_eq!(with(&ancient), old);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with(&v2([368, 118, 0, 0, 1024, 1], 0));
        let before = plugin.clone();
        // FL does not load a version it does not know.
        let mut newer = v2(DEFAULTS, 1);
        newer[..4].copy_from_slice(&3i32.to_le_bytes());
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 3 })
        );
        // Each version has its one length.
        for (bytes, expected) in [
            (&v1(DEFAULTS)[..27], 28),
            (&v2(DEFAULTS, 1)[..], 28),
            (&v2(DEFAULTS, 1)[..28], 29),
            (&[2, 0, 0][..], 29),
            (&[][..], 29),
        ] {
            let mut bytes = bytes.to_vec();
            if expected == 28 && bytes.len() >= 4 {
                bytes[..4].copy_from_slice(&1i32.to_le_bytes());
            }
            assert_eq!(
                plugin.set_state(&bytes),
                Err(StateError::Length {
                    expected,
                    found: bytes.len()
                })
            );
        }
        for (param, value) in [
            (0, -1),
            (0, 1025),
            (1, 1025),
            (2, -1),
            (3, 1025),
            (4, 4096),
            (5, 2),
        ] {
            let mut params = DEFAULTS;
            params[param] = value;
            for bytes in [v1(params), v2(params, 1)] {
                assert_eq!(
                    plugin.set_state(&bytes),
                    Err(StateError::Range { param, value })
                );
            }
        }
        assert_eq!(plugin, before);
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(presets) = presets(NAME) else { return };
        let want = [
            ("Default", DEFAULTS),
            ("Party next door", [160, 1024, 864, 0, 0, 0]),
            ("Retro sound", [480, 1024, 0, 0, 1024, 1]),
            ("Tiny speakers", [544, 0, 0, 1024, 464, 1]),
        ];
        assert_eq!(presets.len(), want.len());
        for (preset, (file, params)) in presets.iter().zip(want) {
            assert_eq!((preset.file.as_str(), preset.plugin.as_str()), (file, NAME));
            // Every one is version 2 with Center on.
            assert_eq!(preset.state, v2(params, 1), "{file}");
            let mut plugin = with(&v1([0; 6]));
            plugin.set_state(&preset.state).unwrap();
            assert!(plugin.center && plugin.params == params, "{file}");
            assert_eq!(plugin.target, coefficients(&params));
        }
        let mut plugin = with(&v1([0; 6]));
        plugin.set_state(&presets[0].state).unwrap();
        assert_eq!(plugin, Filter::default());
    }
}
