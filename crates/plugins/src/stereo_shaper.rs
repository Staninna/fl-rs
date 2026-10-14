//! Fruity Stereo Shaper: a 2x2 mix with a gain for every way through it, a
//! delay on one side and a phase turn on one side, as
//! `docs/plugins/fruity-stereo-shaper.md` has it.
//!
//! The state is 36 bytes: a little-endian i32 version that FL does not
//! read, the plugin's six parameters as i32, an i32 that numbers the insert
//! the difference goes to, and an i32 that is 0 for the delay and the phase
//! turn before the mix and 1 for after it.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Right into left | -25600 to 25600 | 0 |
//! | 1 Left | -25600 to 25600 | 12800 |
//! | 2 Right | -25600 to 25600 | 12800 |
//! | 3 Left into right | -25600 to 25600 | 0 |
//! | 4 Delay | -4096 (left late) to 4096 (right late) | 0 |
//! | 5 Dephasing | -4096 (left) to 4096 (right) | 0 |
//!
//! A gain is `(3^x - 1) / 2` with `x` the wheel over 12800, upside down for
//! a wheel under 0: 1 at 12800, a half at 8076 and 4 at 25600. The delay is
//! `(1001^x - 1) * 0.5` ms with `x` the wheel's size over 4096, half a
//! second at most. The dephasing is a second-order allpass that turns the
//! phase by 180 degrees at `(5001^(1 - x) - 1) * 4.398 + 10` Hz: 22 kHz for
//! a wheel just off 0, down to 10 Hz at its end.
//!
//! With a number for the difference, the plugin also makes its input less
//! its output: [`Effect::aux_output`] has it after every block. The number
//! counts the inserts the plugin's own insert sends to. FL's host
//! (`FUN_0132bb50`) hands the plugin that insert's own buffer, the one its
//! generators play into, and the plugin adds the difference to it. So the
//! difference is heard there, whatever the level of the send.
//!
//! The doc has the allpass's `a1` with no sign and its angle as the
//! frequency times a number of the host's. Both are taken as the cookbook
//! has them, which is the one way the phase turns by 180 degrees at the
//! frequency the doc names.

use crate::parts::Matrix;
use crate::{Effect, StateError, knob_value, read_state};
use dsp::biquad::{Biquad64, Coefficients64};
use std::f64::consts::TAU;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Stereo Shaper";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 6] = [
    (-25600, 25600),
    (-25600, 25600),
    (-25600, 25600),
    (-25600, 25600),
    (-4096, 4096),
    (-4096, 4096),
];
const DEFAULTS: [i32; 6] = [0, 12800, 12800, 0, 0, 0];

/// How long the state is, and where the parameters end in it.
const LEN: usize = 36;
const PARAMS_END: usize = 28;

/// How many frames of the difference there is room for from the start. A
/// longer block asks for more, once.
const AUX_ROOM: usize = 4096;

/// What a gain wheel comes to: `(3^x - 1) / 2` with the wheel's sign, and 1
/// at 12800 without working it out, as in FL.
pub fn gain(value: i32) -> f32 {
    let x = f64::from(value.abs()) / 12800.0;
    let size = if x == 1.0 {
        1.0
    } else {
        (3f64.powf(x) - 1.0) / 2.0
    };
    (size * f64::from(value.signum())) as f32
}

/// How many frames the delay wheel holds one side back by:
/// `(1001^x - 1) * 50` units of 10 microseconds.
pub fn delay_frames(value: i32, hz: f64) -> usize {
    let units = (1001f64.powf(f64::from(value.abs()) / 4096.0) - 1.0) * 50.0;
    (units * hz / 100_000.0).round_ties_even() as usize
}

/// Where the dephasing wheel puts the allpass's 180 degrees, in Hz.
pub fn dephase_hz(value: i32) -> f64 {
    (5001f64.powf(1.0 - f64::from(value.abs()) / 4096.0) - 1.0) * 4.398 + 10.0
}

/// The mix for the first four parameters, as `[LL, RR, RL, LR]`.
fn gains(params: &[i32; 6]) -> [f32; 4] {
    [
        gain(params[1]),
        gain(params[2]),
        gain(params[0]),
        gain(params[3]),
    ]
}

/// The allpass for a dephasing wheel that is not at 0.
///
/// The doc has nothing on a corner above half the rate, where this filter
/// would blow up. The corner stops just under it here.
fn allpass(value: i32, hz: f64) -> Coefficients64 {
    let (sin, cos) = (TAU * dephase_hz(value).min(0.499 * hz) / hz).sin_cos();
    Coefficients64::all_pass(cos, sin)
}

/// Fruity Stereo Shaper. A new one has FL's defaults, which pass a block
/// through untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct StereoShaper {
    params: [i32; 6],
    /// Which of the inserts its own sends to gets the difference, 0 for
    /// none.
    sidechain: i32,
    /// Whether the delay and the phase turn come after the mix.
    after: bool,
    hz: f64,
    matrix: Matrix,
    /// What the late side has yet to play, in a ring, and where the next
    /// sample goes.
    held: Vec<f32>,
    at: usize,
    turn: Coefficients64,
    turned: Biquad64,
    /// The last block's input less its output, when a sidechain is chosen.
    aux: [Vec<f32>; 2],
}

impl Default for StereoShaper {
    fn default() -> Self {
        let mut plugin = StereoShaper {
            params: DEFAULTS,
            sidechain: 0,
            after: true,
            hz: 44_100.0,
            matrix: Matrix::new(gains(&DEFAULTS)),
            held: Vec::new(),
            at: 0,
            turn: Coefficients64::IDENTITY,
            turned: Biquad64::default(),
            aux: [Vec::with_capacity(AUX_ROOM), Vec::with_capacity(AUX_ROOM)],
        };
        plugin.make_room();
        plugin
    }
}

impl StereoShaper {
    /// Have the memory for the longest delay the wheel can ask for at this
    /// rate, half a second, so that the wheel does not have to ask for it.
    fn make_room(&mut self) {
        let longest = delay_frames(RANGES[4].1, self.hz);
        self.held
            .reserve_exact(longest.saturating_sub(self.held.len()));
    }

    /// Take parameters that are inside their ranges, or a new rate.
    ///
    /// A delay of another length or on the other side starts with nothing
    /// held. A dephasing wheel put at 0 forgets what the allpass held, as
    /// FL's does, and one moved elsewhere keeps it.
    fn set(&mut self, params: [i32; 6]) {
        let late = delay_frames(params[4], self.hz);
        let moved_over = (params[4] > 0) != (self.params[4] > 0);
        if late != self.held.len() || moved_over {
            self.held.clear();
            self.held.resize(late, 0.0);
            self.at = 0;
        }
        if params[5] == 0 {
            self.turn = Coefficients64::IDENTITY;
            self.turned.reset();
        } else {
            self.turn = allpass(params[5], self.hz);
        }
        self.params = params;
        self.matrix.aim(gains(&params));
    }

    /// The delay, then the phase turn, each on the side its wheel names:
    /// the right for a wheel above 0 and the left for one below.
    fn effects(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.held.is_empty() {
            let late = if self.params[4] > 0 {
                &mut *right
            } else {
                &mut *left
            };
            for sample in late {
                std::mem::swap(sample, &mut self.held[self.at]);
                self.at += 1;
                if self.at == self.held.len() {
                    self.at = 0;
                }
            }
        }
        if self.params[5] != 0 {
            let side = if self.params[5] > 0 { right } else { left };
            self.turned.process_block(&self.turn, side);
            self.turned.flush();
        }
    }
}

impl Effect for StereoShaper {
    /// The mix is the state's from the first sample on, with no ramp. FL
    /// reads whatever version the state starts with, and so does this.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        let params = read_state(&bytes[4..PARAMS_END], &RANGES)?;
        let [sidechain, position] = read_state(&bytes[PARAMS_END..], &[(0, i32::MAX), (0, 1)])
            .map_err(|error| match error {
                StateError::Range { param, value } => StateError::Range {
                    param: param + RANGES.len(),
                    value,
                },
                other => other,
            })?;
        self.sidechain = sidechain;
        self.after = position == 1;
        self.set(params);
        self.matrix.settle();
        Ok(())
    }

    /// Asks for memory when the rate is higher than before. A rate that is
    /// not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
            self.make_room();
            self.set(self.params);
        }
    }

    /// After a gain has moved, the mix goes from where the last block ended
    /// to where the parameters put it, in a straight line over this block.
    ///
    /// With a sidechain chosen, a block longer than any before it and
    /// longer than 4096 frames asks for memory to keep the difference in.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let aux = self.sidechain > 0;
        if aux {
            for (kept, side) in self.aux.iter_mut().zip([&*left, &*right]) {
                kept.clear();
                kept.extend_from_slice(side);
            }
        }
        if !self.after {
            self.effects(left, right);
        }
        self.matrix.apply(left, right);
        if self.after {
            self.effects(left, right);
        }
        if aux {
            for (kept, side) in self.aux.iter_mut().zip([&*left, &*right]) {
                for (kept, out) in kept.iter_mut().zip(side) {
                    *kept -= *out;
                }
            }
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// Moving the delay drops what the late side had yet to play, as in
    /// FL.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    /// Drops what the late side had yet to play and what the allpass held,
    /// and ends a ramp.
    fn reset(&mut self) {
        self.held.fill(0.0);
        self.at = 0;
        self.turned.reset();
        self.matrix.settle();
        for kept in &mut self.aux {
            kept.clear();
        }
    }

    /// The input of the last block less its output, when the state has a
    /// number for it.
    fn aux_output(&self) -> Option<(&[f32], &[f32])> {
        (self.sidechain > 0).then(|| (self.aux[0].as_slice(), self.aux[1].as_slice()))
    }

    fn sidechain_output(&self) -> usize {
        self.sidechain.max(0) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, corpus, every_preset, partial, presets, sine, state};

    /// A state with these parameters, this sidechain track and the effects
    /// before (0) or after (1) the mix.
    fn bytes(params: [i32; 6], sidechain: i32, position: i32) -> Vec<u8> {
        let mut bytes = state(&[0]);
        bytes.extend(state(&params));
        bytes.extend(state(&[sidechain, position]));
        bytes
    }

    fn with(params: [i32; 6]) -> StereoShaper {
        let mut plugin = StereoShaper::default();
        plugin.set_state(&bytes(params, 0, 1)).unwrap();
        plugin
    }

    /// What one frame comes out as.
    fn frame(params: [i32; 6], left: f32, right: f32) -> (f32, f32) {
        let (mut l, mut r) = ([left], [right]);
        with(params).process(&mut l, &mut r);
        (l[0], r[0])
    }

    /// The frames of a side that are not 0, with what they are.
    fn hits(side: &[f32]) -> Vec<(usize, f32)> {
        let frames = side.iter().copied().enumerate();
        frames.filter(|(_, value)| *value != 0.0).collect()
    }

    /// The doc's delay in samples.
    fn late(value: i32, hz: f64) -> usize {
        let ms = (1001f64.powf(f64::from(value.abs()) / 4096.0) - 1.0) * 0.5;
        (ms * hz / 1000.0).round() as usize
    }

    #[test]
    fn the_defaults_pass_a_stereo_signal_to_the_bit() {
        assert_eq!(gains(&DEFAULTS), [1.0, 1.0, 0.0, 0.0]);
        let left = [0.0, -0.0, 1.0, -1.0, 0.3, -1e-30, 7.5, f32::MIN_POSITIVE];
        let right = [0.25, 1e-20, -3.0, 0.1, -0.0, 1e30, -1e-38, 0.5];
        for mut plugin in [StereoShaper::default(), with(DEFAULTS)] {
            for _ in 0..2 {
                let (mut l, mut r) = (left, right);
                plugin.process(&mut l, &mut r);
                assert_eq!(bits(&l), bits(&left));
                assert_eq!(bits(&r), bits(&right));
            }
            assert_eq!(plugin.aux_output(), None);
        }
        assert_eq!(StereoShaper::default(), with(DEFAULTS));
    }

    #[test]
    fn a_gain_is_the_doc_s_curve_with_the_wheel_s_sign() {
        assert_eq!((gain(12800), gain(-12800), gain(0)), (1.0, -1.0, 0.0));
        // 8076 is a half to five places: 3^(8076 / 12800) is 2.000016.
        assert!((gain(8076) - 0.5).abs() < 1e-5 && gain(8076) != 0.5);
        assert!((gain(-8076) + 0.5).abs() < 1e-5);
        // The wheel's end is (9 - 1) / 2. The doc's prose has 2 there,
        // which its own formula does not give.
        assert_eq!(gain(25600), 4.0);
        for value in [1, 300, 6400, 12799, 12801, 20000] {
            let want = (3f64.powf(f64::from(value) / 12800.0) - 1.0) / 2.0;
            assert!((f64::from(gain(value)) - want).abs() < 1e-6, "{value}");
            assert_eq!(gain(-value), -gain(value));
        }
    }

    #[test]
    fn the_four_gains_are_the_four_ways_through_the_mix() {
        // L' = L * left + R * (right into left), and the same for R'.
        let params = [3000, 9000, -12800, 20000, 0, 0];
        let (left, right) = frame(params, 0.5, -0.25);
        let want_left = 0.5 * gain(9000) - 0.25 * gain(3000);
        let want_right = -0.25 * gain(-12800) + 0.5 * gain(20000);
        assert!((left - want_left).abs() < 1e-6 && (right - want_right).abs() < 1e-6);
        // The left all the way up and the right at nothing mutes the
        // right: the doc's "Left - Right splitter".
        let splitter = [0, 12800, 0, 0, 0, 0];
        assert_eq!(frame(splitter, 0.7, -0.4), (0.7, 0.0));
        assert_eq!(frame(splitter, 0.0, 1.0), (0.0, 0.0));
        // "Invert Left", "Mid" and "MS to LR".
        assert_eq!(frame([0, -12800, 12800, 0, 0, 0], 0.5, 0.25), (-0.5, 0.25));
        let (mid, also_mid) = frame([8076, 8076, 8076, 8076, 0, 0], 1.0, 0.5);
        assert!((mid - 0.75).abs() < 1e-4 && mid == also_mid);
        assert_eq!(
            frame([12800, 12800, 12800, -12800, 0, 0], 0.5, 0.25),
            (0.75, -0.25)
        );
    }

    #[test]
    fn the_delay_holds_one_side_back_by_the_formula_s_samples() {
        // The doc's "Delay": 2416 is 28.9 ms.
        let ms = (1001f64.powf(2416.0 / 4096.0) - 1.0) * 0.5;
        assert!((ms - 28.9).abs() < 0.05, "{ms}");
        assert_eq!(late(2416, 48_000.0), (ms * 48.0).round() as usize);
        assert_eq!(late(4096, 44_100.0), 22_050);

        for (value, hz) in [
            (2416, 48_000.0),
            (-2416, 48_000.0),
            (1408, 44_100.0),
            (-300, 96_000.0),
        ] {
            let n = late(value, f64::from(hz));
            assert_eq!(delay_frames(value, f64::from(hz)), n);
            let mut plugin = with([0, 12800, 12800, 0, value, 0]);
            plugin.set_sample_rate(hz);
            let (mut left, mut right) = (vec![0.0; n + 250], vec![0.0; n + 250]);
            (left[0], right[0]) = (1.0, 1.0);
            for (l, r) in left.chunks_mut(100).zip(right.chunks_mut(100)) {
                plugin.process(l, r);
            }
            // Above 0 it is the right side that is late, under 0 the left.
            let (on_time, held) = if value > 0 {
                (left, right)
            } else {
                (right, left)
            };
            assert_eq!(hits(&on_time), [(0, 1.0)], "delay {value}");
            assert_eq!(hits(&held), [(n, 1.0)], "delay {value}");
        }
        assert!(late(-300, 96_000.0) > 20);
    }

    #[test]
    fn the_dephasing_turns_one_side_upside_down_at_the_formula_s_frequency() {
        // The wheel's two ends, and the doc's "Stereoize" at 297 Hz.
        assert!((dephase_hz(0) - 22_000.0).abs() < 1e-6);
        assert!((dephase_hz(4096) - 10.0).abs() < 1e-9);
        assert!((dephase_hz(2079) - 297.0).abs() < 1.0);

        for value in [2079i32, -2079, 1500, -3000] {
            let hz = 48_000.0;
            let wanted = (5001f64.powf(1.0 - f64::from(value.abs()) / 4096.0) - 1.0) * 4.398 + 10.0;
            let turns = wanted / hz;
            let mut plugin = with([0, 12800, 12800, 0, 0, value]);
            plugin.set_sample_rate(hz as f32);
            let tone = sine(turns, 96_000);
            let (mut left, mut right) = (tone.clone(), tone.clone());
            plugin.process(&mut left, &mut right);
            let (turned, plain) = if value > 0 {
                (right, left)
            } else {
                (left, right)
            };
            // The other side is as it came.
            assert!(plain == tone, "dephasing {value}");
            // 180 degrees: once it has settled, the tone with its sign
            // turned.
            let worst = turned[90_000..]
                .iter()
                .zip(&tone[90_000..])
                .map(|(got, sent)| (got + sent).abs())
                .fold(0.0, f32::max);
            assert!(worst < 1e-4, "dephasing {value}: {worst}");
        }

        // An allpass keeps every tone's size: an octave under its corner
        // the tone is as loud as it came, and neither as it was nor upside
        // down.
        let mut plugin = with([0, 12800, 12800, 0, 0, 2079]);
        let turns = dephase_hz(2079) / 2.0 / 44_100.0;
        let window = (20.0 / turns).round() as usize;
        let turns = 20.0 / window as f64;
        let tone = sine(turns, 40_000 + window);
        let (mut left, mut right) = (tone.clone(), tone.clone());
        plugin.process(&mut left, &mut right);
        assert!((partial(&right[40_000..], turns) - 1.0).abs() < 1e-4);
        let apart: Vec<f32> = right.iter().zip(&tone).map(|(a, b)| a - b).collect();
        let size = partial(&apart[40_000..], turns);
        assert!(size > 0.5 && size < 1.9, "{size}");
    }

    #[test]
    fn the_effects_are_before_or_after_the_mix() {
        // The left into both outputs at a half, and the right 3 frames
        // late at 44.1 kHz.
        let value = (1..4096).find(|v| delay_frames(*v, 44_100.0) == 3).unwrap();
        let run = |position: i32| {
            let mut plugin = StereoShaper::default();
            let params = [0, 8076, 12800, 8076, value, 0];
            plugin.set_state(&bytes(params, 0, position)).unwrap();
            let (mut left, mut right) = (vec![0.0; 8], vec![0.0; 8]);
            (left[0], right[0]) = (1.0, 1.0);
            plugin.process(&mut left, &mut right);
            let round = |side: &[f32]| -> Vec<(usize, i32)> {
                let hits = hits(side).into_iter();
                hits.map(|(n, v)| (n, (v * 1000.0).round() as i32))
                    .collect()
            };
            (round(&left), round(&right))
        };
        // After the mix, all of the right output is late.
        assert_eq!(run(1), (vec![(0, 500)], vec![(3, 1500)]));
        // Before it, only the right input is, and the left's part of the
        // right output is on time.
        assert_eq!(run(0), (vec![(0, 500)], vec![(0, 500), (3, 1000)]));
    }

    #[test]
    fn the_second_output_is_the_input_less_the_output() {
        // The splitter: the right muted, and the difference to the second
        // insert its own sends to.
        let mut plugin = StereoShaper::default();
        let splitter = [0, 12800, 0, 0, 0, 0];
        plugin.set_state(&bytes(splitter, 2, 1)).unwrap();
        assert_eq!(plugin.sidechain_output(), 2);
        assert_eq!(plugin.aux_output(), Some((&[][..], &[][..])));
        let (left, right) = ([0.5, -0.25, 0.125], [0.75, 0.5, -1.0]);
        let (mut l, mut r) = (left, right);
        plugin.process(&mut l, &mut r);
        assert_eq!((l, r), (left, [0.0; 3]));
        // What the left output kept is gone from the difference, and what
        // the right output lost is in it.
        assert_eq!(plugin.aux_output(), Some((&[0.0; 3][..], &right[..])));

        // With a delay and a phase turn it is still the one less the
        // other, sample by sample, and as long as the block.
        let params = [3000, 9000, -12800, 20000, 700, -2000];
        for position in [0, 1] {
            let mut plugin = StereoShaper::default();
            plugin.set_state(&bytes(params, 1, position)).unwrap();
            let mut quiet = StereoShaper::default();
            quiet.set_state(&bytes(params, 0, position)).unwrap();
            for len in [64, 1, 300] {
                let (left, right) = (sine(0.013, len), sine(0.031, len));
                let (mut l, mut r) = (left.clone(), right.clone());
                plugin.process(&mut l, &mut r);
                let (aux_left, aux_right) = plugin.aux_output().unwrap();
                assert_eq!((aux_left.len(), aux_right.len()), (len, len));
                for n in 0..len {
                    assert_eq!(aux_left[n], left[n] - l[n]);
                    assert_eq!(aux_right[n], right[n] - r[n]);
                }
                // The sidechain changes nothing in the output.
                let (mut ql, mut qr) = (left.clone(), right.clone());
                quiet.process(&mut ql, &mut qr);
                assert!(ql == l && qr == r);
                assert_eq!(quiet.aux_output(), None);
            }
        }
    }

    #[test]
    fn a_block_within_the_room_asks_for_no_memory() {
        let mut plugin = StereoShaper::default();
        plugin.set_sample_rate(48_000.0);
        plugin
            .set_state(&bytes([0, 12800, 0, 0, 4096, 0], 3, 1))
            .unwrap();
        let memory = |plugin: &StereoShaper| {
            (
                plugin.held.as_ptr(),
                plugin.aux[0].as_ptr(),
                plugin.aux[1].as_ptr(),
            )
        };
        let before = memory(&plugin);
        assert_eq!(plugin.held.len(), 24_000);
        for (len, delay) in [(512, -4096.0), (AUX_ROOM, 17.0), (1, 4096.0), (64, 0.0)] {
            plugin.set_param(4, delay);
            plugin.process(&mut vec![0.5; len], &mut vec![0.5; len]);
            assert_eq!(plugin.aux_output().unwrap().0.len(), len);
        }
        assert_eq!(memory(&plugin), before);
        // A rate that is not one leaves the plugin as it is.
        plugin.set_param(4, 4096.0);
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(0.0);
        assert_eq!(plugin.held.len(), 24_000);
    }

    #[test]
    fn a_moved_parameter_ramps_the_mix_and_is_the_same_plugin_as_its_state() {
        let mut plugin = StereoShaper::default();
        plugin.set_param(1, 0.0);
        let (mut left, mut right) = ([1.0; 4], [-1.0; 4]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [1.0, 0.75, 0.5, 0.25]);
        assert_eq!(right, [-1.0; 4]);

        let moves = [-8076.2, 99_999.0, -12800.0, 20000.4, -700.0, 2079.0];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
        }
        plugin.set_param(6, 1.0);
        assert_eq!((plugin.param(6), plugin.param_count()), (0.0, 6));
        assert_eq!(
            [0, 1, 2, 3, 4, 5].map(|i| plugin.param(i)),
            [-8076.0, 25600.0, -12800.0, 20000.0, -700.0, 2079.0]
        );
        plugin.reset();
        assert_eq!(plugin, with([-8076, 25600, -12800, 20000, -700, 2079]));

        // The dephasing wheel back at 0 forgets what the allpass held.
        plugin.process(&mut [1.0; 16], &mut [1.0; 16]);
        assert_ne!(plugin.turned, Biquad64::default());
        plugin.set_param(5, 0.0);
        assert_eq!(plugin.turned, Biquad64::default());
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([3000, 9000, -12800, 20000, 700, -2000]);
        let before = plugin.clone();
        for len in [0, 3, 32, 40] {
            assert_eq!(
                plugin.set_state(&vec![0; len]),
                Err(StateError::Length {
                    expected: 36,
                    found: len
                })
            );
        }
        for (param, value) in [(0, 25601), (2, -25601), (4, 4097), (5, -4097)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(params, 0, 1)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(
            plugin.set_state(&bytes(DEFAULTS, -1, 1)),
            Err(StateError::Range {
                param: 6,
                value: -1
            })
        );
        assert_eq!(
            plugin.set_state(&bytes(DEFAULTS, 0, 2)),
            Err(StateError::Range { param: 7, value: 2 })
        );
        assert_eq!(plugin, before);
        // FL does not read the version, so any will do.
        let mut versioned = bytes(DEFAULTS, 0, 1);
        versioned[0] = 9;
        assert_eq!(plugin.set_state(&versioned), Ok(()));
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(own) = presets(NAME) else { return };
        // 25 of the plugin's own, 15 of them under Mixing, and the three
        // of the plugin database.
        let presets = every_preset(&corpus().unwrap(), NAME);
        assert_eq!((own.len(), presets.len()), (25, 28));
        let named = |file: &str| {
            let preset = presets.iter().find(|p| p.file == file);
            let preset = preset.unwrap_or_else(|| panic!("no preset {file}"));
            let mut plugin = StereoShaper::default();
            plugin.set_state(&preset.state).unwrap();
            plugin
        };
        for preset in &presets {
            assert_eq!(preset.state.len(), 36, "{}", preset.file);
            assert!(preset.plugin.eq_ignore_ascii_case(NAME), "{}", preset.file);
            named(&preset.file);
        }
        assert_eq!(named("Default"), StereoShaper::default());
        // The doc's table.
        let splitter = named("Left - Right splitter");
        assert_eq!(splitter.params, [0, 12800, 0, 0, 0, 0]);
        assert_eq!((splitter.sidechain_output(), splitter.after), (2, true));
        let delay = named("Delay");
        assert!(delay.params[4] > 0);
        let ms = delay.held.len() as f64 / 44.1;
        assert!((ms - 28.9).abs() < 0.05, "{ms}");
        let stereoize = named("Stereoize");
        assert!(stereoize.params[5] > 0);
        assert!((dephase_hz(stereoize.params[5]) - 297.0).abs() < 0.5);
        let phaser = named("Phaser");
        assert_eq!(phaser.params[..4], [12800; 4]);
        assert!(!phaser.after && (dephase_hz(phaser.params[5]) - 591.0).abs() < 0.5);
        let side = named("Side");
        assert_eq!(side.params[..4], [-8076, 8076, -8076, 8076]);
    }
}
