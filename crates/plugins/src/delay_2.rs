//! Fruity Delay 2: a stereo echo that keeps time with the tempo, as
//! `docs/plugins/fruity-delay-2.md` has it.
//!
//! The state is eight little-endian i32, the plugin's eight parameters:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Input panning | -128 (left) to 128 (right) | 0 |
//! | 1 Input volume | 0 to 160 | 128 |
//! | 2 Dry level | 0 to 160 | 128 |
//! | 3 Feedback level | 0 to 127 | 64 |
//! | 4 Time | 1 to 768 | 144 |
//! | 5 Time stereo offset | -512 (left late) to 512 (right late) | 0 |
//! | 6 Feedback mode | 0 normal, 1 invert, 2 ping-pong | 0 |
//! | 7 Cutoff freq | 0 to 128 | 128 |
//!
//! Time is in 192ths of a beat, so the echo comes
//! `D = round(time * rate * 0.3125 / tempo)` frames after its input: 144 is
//! a dotted eighth. The tempo is the host's to give, through
//! [`Effect::set_tempo`], and is 140 until it does.
//!
//! What goes into the line is the input through a pan and a volume. With
//! `w` what the line holds, `fb` the feedback and `LP` a one pole low pass,
//!
//! ```text
//! w[n] = LP(in[n] + fb * w[n - D])
//! y[n] = dry * x[n] + w[n - D]
//! ```
//!
//! Invert swaps the sides of `w[n - D]` on its way out. Ping-pong swaps
//! them on the way back into the line as well, so the echoes change sides.
//!
//! One thing here is a guess. FL scales the low pass's coefficient by a
//! number its host hands the plugin, which the doc takes to be
//! `44100 / rate`. That is what this does. It leaves the low pass out of
//! the way at the default cutoff at 44.1 kHz and under, and makes it a
//! gentle one at the default above that.

use crate::parts::{Matrix, offset_frames};
use crate::{Effect, StateError, knob_gain, knob_value, read_state};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Delay 2";

/// The range of each parameter's control.
const RANGES: [(i32, i32); 8] = [
    (-128, 128),
    (0, 160),
    (0, 160),
    (0, 127),
    (1, 768),
    (-512, 512),
    (0, 2),
    (0, 128),
];
const DEFAULTS: [i32; 8] = [0, 128, 128, 64, 144, 0, 0, 128];

/// The tempo before the host has given one.
const TEMPO: f64 = 140.0;

/// The slowest and the fastest tempo FL has. A tempo outside them is held
/// to the nearer one, which keeps the line a size that can be had.
const TEMPO_RANGE: (f64, f64) = (10.0, 522.0);

/// What the line holds under this is 0. FL has a floor here too, at a
/// number that was not read. This one is the smallest a float holds in
/// full.
const QUIET: f64 = f32::MIN_POSITIVE as f64;

/// The gain of the input volume or the dry level. FL has 128 down as 1
/// without working it out.
fn gain(value: i32) -> f32 {
    if value == 128 {
        1.0
    } else {
        knob_gain(f64::from(value) / 128.0) as f32
    }
}

/// The gains on the way into the line for a pan and a volume, as
/// `[LL, RR, RL, LR]`: left to left, right to right, right into left, left
/// into right.
///
/// The side the pan leans from loses its share and gives half of it to the
/// other side, so hard left is both inputs on the left at half the volume.
pub fn input_gains(pan: i32, volume: i32) -> [f32; 4] {
    let pan = pan as f32 / 128.0;
    let volume = gain(volume);
    if pan < 0.0 {
        let kept = pan + 1.0;
        let crossed = (1.0 - kept) * 0.5 * volume;
        [volume - crossed, kept * volume, crossed, 0.0]
    } else {
        let kept = 1.0 - pan;
        let crossed = pan * 0.5 * volume;
        [kept * volume, volume - crossed, 0.0, crossed]
    }
}

/// How many frames after its input an echo comes.
fn delay_frames(time: i32, hz: f64, tempo: f64) -> usize {
    let frames = f64::from(time) * hz * 0.3125 / tempo;
    (frames.round_ties_even() as usize).max(1)
}

/// Fruity Delay 2. A new one has FL's defaults at 44.1 kHz and 140 BPM: a
/// dotted eighth at half feedback under the dry signal.
#[derive(Debug, Clone, PartialEq)]
pub struct Delay2 {
    params: [i32; 8],
    hz: f64,
    tempo: f64,
    /// The gains on the way into the line.
    input: Matrix,
    dry: f32,
    /// The feedback the parameter asks for and the one the last block
    /// ended on.
    feedback: f32,
    feedback_now: f32,
    /// The low pass's coefficient, 1 where it passes everything.
    cutoff: f32,
    /// The line, a ring of stereo frames: the delay, the offset and one
    /// more.
    line: Vec<[f32; 2]>,
    /// Where the oldest frame of the line is: the echo of this frame.
    oldest: usize,
    /// `D`, the frames from an input to its echo.
    delay: usize,
    /// How many frames more each side's input waits.
    offset: [usize; 2],
    /// The low pass of each side.
    pole: [f64; 2],
}

impl Default for Delay2 {
    fn default() -> Self {
        let mut plugin = Delay2 {
            params: DEFAULTS,
            hz: 44_100.0,
            tempo: TEMPO,
            input: Matrix::new([0.0; 4]),
            dry: 0.0,
            feedback: 0.0,
            feedback_now: 0.0,
            cutoff: 0.0,
            line: Vec::new(),
            oldest: 0,
            delay: 0,
            offset: [0; 2],
            pole: [0.0; 2],
        };
        plugin.make_room();
        plugin.set(DEFAULTS);
        plugin.settle();
        plugin
    }
}

impl Delay2 {
    /// Have the memory for the longest line the wheels can ask for at this
    /// rate and tempo, so that no wheel has to ask for memory.
    fn make_room(&mut self) {
        let longest =
            delay_frames(RANGES[4].1, self.hz, self.tempo) + offset_frames(512, self.hz) + 1;
        self.line
            .reserve_exact(longest.saturating_sub(self.line.len()));
    }

    /// Take parameters that are inside their ranges, or a new rate or
    /// tempo.
    ///
    /// A line that has to be another length starts empty, as FL's does. One
    /// that stays as long keeps its echoes.
    fn set(&mut self, params: [i32; 8]) {
        self.params = params;
        self.input.aim(input_gains(params[0], params[1]));
        self.dry = gain(params[2]);
        self.feedback = params[3] as f32 / 128.0;
        let open = (21f64.powf(f64::from(params[7]) / 128.0) - 1.0) * 0.05;
        self.cutoff = (open * 44_100.0 / self.hz).min(1.0) as f32;

        let delay = delay_frames(params[4], self.hz, self.tempo);
        let late = offset_frames(params[5], self.hz);
        // The right side is late for an offset above 0, the left for one
        // under it.
        let offset = if params[5] > 0 { [0, late] } else { [late, 0] };
        if (delay, offset) != (self.delay, self.offset) {
            (self.delay, self.offset) = (delay, offset);
            self.line.clear();
            self.line.resize(delay + late + 1, [0.0; 2]);
            self.oldest = 0;
        }
    }

    /// End the ramps that are under way.
    fn settle(&mut self) {
        self.input.settle();
        self.feedback_now = self.feedback;
    }
}

impl Effect for Delay2 {
    /// The gains and the feedback are the state's from the first sample
    /// on, with no ramp.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.set(read_state(bytes, &RANGES)?);
        self.settle();
        Ok(())
    }

    /// Asks for memory when the line may have to be longer than before. A
    /// rate that is not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
            self.make_room();
            self.set(self.params);
        }
    }

    /// Asks for memory when the line may have to be longer than before. A
    /// tempo that is not a number is not taken.
    fn set_tempo(&mut self, bpm: f64) {
        if !bpm.is_nan() {
            self.tempo = bpm.clamp(TEMPO_RANGE.0, TEMPO_RANGE.1);
            self.make_room();
            self.set(self.params);
        }
    }

    /// After a parameter has moved, the gains into the line and the
    /// feedback go to where they were put in a straight line over this
    /// block. The dry level, the mode and the cutoff are the parameters'
    /// at once.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let (mut gains, gains_step) = self.input.ramp(len);
        let mut feedback = self.feedback_now;
        let feedback_step = (self.feedback - feedback) / len as f32;
        self.feedback_now = self.feedback;

        let mode = self.params[6];
        let cutoff = f64::from(self.cutoff);
        let frames = self.line.len();
        let wrap = |index: usize| {
            if index < frames {
                index
            } else {
                index - frames
            }
        };

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            // Where this frame's input goes, `D` frames ahead of the echo.
            let newest = wrap(self.oldest + self.delay);
            let (in_left, in_right) = Matrix::mix(&gains, *l, *r);
            self.line[wrap(newest + self.offset[0])][0] += in_left;
            self.line[wrap(newest + self.offset[1])][1] += in_right;

            let echo = self.line[self.oldest];
            let swapped = [echo[1], echo[0]];
            let (out, back) = match mode {
                0 => (echo, echo),
                1 => (swapped, echo),
                _ => (swapped, swapped),
            };
            let slots = self.line[newest].iter_mut().zip(&mut self.pole);
            for ((slot, pole), back) in slots.zip(back) {
                let fed = f64::from(*slot) + f64::from(feedback) * f64::from(back);
                *pole += (fed - *pole) * cutoff;
                *slot = if pole.abs() <= QUIET {
                    0.0
                } else {
                    *pole as f32
                };
            }
            self.line[self.oldest] = [0.0; 2];
            self.oldest = wrap(self.oldest + 1);

            // FL writes the echoes straight to the output when there is no
            // dry signal to add them to.
            if self.dry == 0.0 {
                (*l, *r) = (out[0], out[1]);
            } else {
                (*l, *r) = (*l * self.dry + out[0], *r * self.dry + out[1]);
            }
            for (gain, step) in gains.iter_mut().zip(gains_step) {
                *gain += step;
            }
            feedback += feedback_step;
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// Moving the time or the offset empties the line, as in FL.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    /// Empties the line and the low pass and ends the ramps.
    fn reset(&mut self) {
        self.line.fill([0.0; 2]);
        self.oldest = 0;
        self.pole = [0.0; 2];
        self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{presets, state};

    /// A plugin at a rate and a tempo with a state.
    fn at(params: [i32; 8], hz: f32, bpm: f64) -> Delay2 {
        let mut plugin = Delay2::default();
        plugin.set_sample_rate(hz);
        plugin.set_tempo(bpm);
        plugin.set_state(&state(&params)).unwrap();
        plugin
    }

    /// A plugin as FL has it before a tempo comes: 44.1 kHz and 140 BPM.
    fn with(params: [i32; 8]) -> Delay2 {
        at(params, 44_100.0, 140.0)
    }

    /// The doc's delay length.
    fn d(time: i32, hz: f64, bpm: f64) -> usize {
        (f64::from(time) * hz * 0.3125 / bpm).round() as usize
    }

    /// The doc's stereo offset in frames.
    fn late(value: i32, hz: f64) -> usize {
        let units = (1001f64.powf(f64::from(value.abs()) / 512.0) - 1.0) * 50.0;
        (units * hz / 100_000.0).round() as usize
    }

    /// The doc's gain curve.
    fn g(value: i32) -> f64 {
        if value == 128 {
            1.0
        } else {
            (11f64.powf(f64::from(value) / 128.0) - 1.0) / 10.0
        }
    }

    /// The doc's cutoff coefficient, with its guess at the host's factor.
    fn c(value: i32, hz: f64) -> f64 {
        ((21f64.powf(f64::from(value) / 128.0) - 1.0) * 0.05 * 44_100.0 / hz).min(1.0)
    }

    /// The doc's pan law, as `[LL, RR, RL, LR]`.
    fn pan_law(pan: i32, volume: i32) -> [f64; 4] {
        let (pan, vol) = (f64::from(pan) / 128.0, g(volume));
        if pan < 0.0 {
            let a = pan + 1.0;
            let rl = (1.0 - a) * 0.5 * vol;
            [vol - rl, a * vol, rl, 0.0]
        } else {
            let a = 1.0 - pan;
            let lr = pan * 0.5 * vol;
            [a * vol, vol - lr, 0.0, lr]
        }
    }

    /// The doc's maths with no ring: every frame the line was given is
    /// kept, and a frame reads the one `D` before it.
    fn by_the_doc(
        params: [i32; 8],
        hz: f64,
        bpm: f64,
        left: &[f32],
        right: &[f32],
    ) -> (Vec<f32>, Vec<f32>) {
        let [pan, volume, dry, feedback, time, offset, mode, cutoff] = params;
        let [ll, rr, rl, lr] = pan_law(pan, volume);
        let (dry, fb, c) = (g(dry), f64::from(feedback) / 128.0, c(cutoff, hz));
        let delay = d(time, hz, bpm);
        let late = if offset > 0 {
            [0, late(offset, hz)]
        } else {
            [late(offset, hz), 0]
        };
        // The input of a side as it gets to the line, its offset late.
        let input = |side: usize, n: usize| {
            let Some(n) = n.checked_sub(late[side]) else {
                return 0.0;
            };
            let (l, r) = (f64::from(left[n]), f64::from(right[n]));
            // The line takes it as a float.
            f64::from(if side == 0 {
                l * ll + r * rl
            } else {
                r * rr + l * lr
            } as f32)
        };

        let mut w = vec![[0f32; 2]; left.len()];
        let mut lp = [0f64; 2];
        let mut out = (Vec::new(), Vec::new());
        for n in 0..left.len() {
            let echo = if n >= delay { w[n - delay] } else { [0.0; 2] };
            let swapped = [echo[1], echo[0]];
            let (y, back) = match mode {
                0 => (echo, echo),
                1 => (swapped, echo),
                _ => (swapped, swapped),
            };
            for (side, back) in back.into_iter().enumerate() {
                lp[side] += (input(side, n) + fb * f64::from(back) - lp[side]) * c;
                w[n][side] = lp[side] as f32;
            }
            out.0
                .push((f64::from(left[n]) * dry + f64::from(y[0])) as f32);
            out.1
                .push((f64::from(right[n]) * dry + f64::from(y[1])) as f32);
        }
        out
    }

    /// Run a signal through the plugin in blocks of `block` frames.
    fn run(plugin: &mut Delay2, left: &[f32], right: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (left.to_vec(), right.to_vec());
        for (l, r) in left.chunks_mut(block).zip(right.chunks_mut(block)) {
            plugin.process(l, r);
        }
        (left, right)
    }

    /// What comes out of `len` frames that start with one frame of
    /// `(left, right)`.
    fn impulse(plugin: &mut Delay2, left: f32, right: f32, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; len], vec![0.0; len]);
        (l[0], r[0]) = (left, right);
        run(plugin, &l, &r, 512)
    }

    /// The frames of a side that are not 0, with what they are.
    fn hits(side: &[f32]) -> Vec<(usize, f32)> {
        let frames = side.iter().copied().enumerate();
        frames.filter(|(_, value)| *value != 0.0).collect()
    }

    /// A signal with nothing regular about it, in -1 to 1.
    fn noise(len: usize, seed: u32) -> Vec<f32> {
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / 8_388_608.0 - 1.0
        };
        (0..len).map(|_| next()).collect()
    }

    fn assert_close(got: &[f32], want: &[f32], what: &str) {
        assert_eq!(got.len(), want.len());
        for (n, (got, want)) in got.iter().zip(want).enumerate() {
            let within = 1e-4 * (1.0 + want.abs());
            assert!(
                (got - want).abs() < within,
                "{what}, frame {n}: {got} and not {want}"
            );
        }
    }

    #[test]
    fn at_120_bpm_a_dotted_eighth_echoes_an_impulse_once_d_samples_on() {
        let params = [0, 128, 128, 0, 144, 0, 0, 128];
        // 144 * 44100 * 0.3125 / 120 is 16537.5, three eighths of a
        // second, and rounds to 16538.
        let delay = d(144, 44_100.0, 120.0);
        assert_eq!(delay, 16_538);
        let mut plugin = at(params, 44_100.0, 120.0);
        let (left, right) = impulse(&mut plugin, 1.0, 0.0, 3 * delay);
        assert_eq!(hits(&left), [(0, 1.0), (delay, 1.0)]);
        assert_eq!(hits(&right), []);

        // The right side has its own echo.
        let mut plugin = at(params, 44_100.0, 120.0);
        let (left, right) = impulse(&mut plugin, 0.5, -0.25, 3 * delay);
        assert_eq!(hits(&left), [(0, 0.5), (delay, 0.5)]);
        assert_eq!(hits(&right), [(0, -0.25), (delay, -0.25)]);

        // The size of the blocks does not move it.
        for block in [1, 7, 4096, 3 * delay] {
            let mut plugin = at(params, 44_100.0, 120.0);
            let mut input = vec![0.0; 3 * delay];
            input[0] = 1.0;
            let (left, _) = run(&mut plugin, &input, &input, block);
            assert_eq!(hits(&left), [(0, 1.0), (delay, 1.0)], "blocks of {block}");
        }
    }

    #[test]
    fn the_delay_goes_with_the_rate_and_the_tempo() {
        let params = [0, 128, 0, 0, 144, 0, 0, 128];
        for (hz, bpm) in [(44_100.0, 140.0), (44_100.0, 60.0), (22_050.0, 174.0)] {
            let delay = d(144, hz, bpm);
            let (left, _) = impulse(&mut at(params, hz as f32, bpm), 1.0, 1.0, 2 * delay);
            assert_eq!(hits(&left), [(delay, 1.0)], "{hz} Hz at {bpm}");
        }
        // 192 is a beat and 48 a sixteenth: half a second and an eighth of
        // one at 120.
        assert_eq!(d(192, 48_000.0, 120.0), 24_000);
        assert_eq!(d(48, 48_000.0, 120.0), 6000);
        for time in [1, 48, 192, 768] {
            let plugin = at([0, 128, 0, 0, time, 0, 0, 128], 48_000.0, 120.0);
            assert_eq!(plugin.delay, d(time, 48_000.0, 120.0));
        }

        // Above 44.1 kHz the doc's guess at the host's factor makes the
        // default cutoff a low pass: at 48 kHz the echo starts at D with
        // 0.92 of the impulse, and the rest of it trails.
        let (delay, c) = (d(144, 48_000.0, 120.0), c(128, 48_000.0));
        assert_eq!(delay, 18_000);
        assert!((c - 0.91875).abs() < 1e-12);
        let (left, _) = impulse(&mut at(params, 48_000.0, 120.0), 1.0, 1.0, 2 * delay);
        let echo = hits(&left);
        assert_eq!(echo[0].0, delay);
        for (k, (frame, value)) in echo.iter().take(5).enumerate() {
            let want = c * (1.0 - c).powi(k as i32);
            assert!(*frame == delay + k && (f64::from(*value) - want).abs() < 1e-6);
        }
    }

    #[test]
    fn the_default_is_a_dotted_eighth_at_half_feedback_under_the_dry_signal() {
        let mut plugin = Delay2::default();
        assert_eq!(plugin, with(DEFAULTS));
        assert_eq!(
            [0, 1, 2, 3, 4, 5, 6, 7].map(|i| plugin.param(i)),
            DEFAULTS.map(|v| v as f32)
        );
        // Before a tempo comes, FL sizes the line for 140 BPM.
        let delay = d(144, 44_100.0, 140.0);
        assert_eq!(delay, 14_175);
        let mut left = vec![0.0; 4 * delay];
        let mut right = left.clone();
        (left[0], left[1]) = (0.5, -0.25);
        (right[0], right[1]) = (0.125, 1.0);
        let (out_left, out_right) = run(&mut plugin, &left, &right, 512);
        // The dry signal as it is, and each side's echoes under it: all of
        // it after D, half after 2 D, a quarter after 3 D.
        let echoes = |first: f32, second: f32| {
            let mut want = Vec::new();
            for (pass, level) in [1.0, 1.0, 0.5, 0.25].into_iter().enumerate() {
                want.push((pass * delay, first * level));
                want.push((pass * delay + 1, second * level));
            }
            want
        };
        assert_eq!(hits(&out_left), echoes(0.5, -0.25));
        assert_eq!(hits(&out_right), echoes(0.125, 1.0));
        let (want_left, want_right) = by_the_doc(DEFAULTS, 44_100.0, 140.0, &left, &right);
        assert_eq!((out_left, out_right), (want_left, want_right));
    }

    #[test]
    fn ping_pong_alternates_the_sides_and_invert_swaps_them() {
        let delay = d(48, 44_100.0, 140.0);
        let echoes = |mode: i32| {
            let mut plugin = with([0, 128, 0, 64, 48, 0, mode, 128]);
            let (left, right) = impulse(&mut plugin, 1.0, 0.0, 4 * delay + 10);
            (hits(&left), hits(&right))
        };
        let on = |passes: &[usize]| -> Vec<(usize, f32)> {
            let level = |pass: &usize| 0.5f32.powi(*pass as i32 - 1);
            passes.iter().map(|p| (p * delay, level(p))).collect()
        };
        // Normal: every echo of the left side is on the left.
        assert_eq!(echoes(0), (on(&[1, 2, 3, 4]), vec![]));
        // Invert: every one is on the right.
        assert_eq!(echoes(1), (vec![], on(&[1, 2, 3, 4])));
        // Ping-pong: the first is on the right, the next on the left.
        assert_eq!(echoes(2), (on(&[2, 4]), on(&[1, 3])));
    }

    #[test]
    fn the_offset_holds_one_side_s_input_back() {
        let delay = d(48, 44_100.0, 140.0);
        // 128 is 2.3 ms, 102 frames at 44.1 kHz.
        let frames = late(128, 44_100.0);
        assert_eq!(frames, 102);
        // Above 0 it is the right side that is late, on every pass.
        let mut plugin = with([0, 128, 0, 64, 48, 128, 0, 128]);
        let (left, right) = impulse(&mut plugin, 1.0, 1.0, 3 * delay);
        assert_eq!(hits(&left), [(delay, 1.0), (2 * delay, 0.5)]);
        assert_eq!(
            hits(&right),
            [(delay + frames, 1.0), (2 * delay + frames, 0.5)]
        );
        // Under 0 the left side.
        let mut plugin = with([0, 128, 0, 64, 48, -128, 0, 128]);
        let (left, right) = impulse(&mut plugin, 1.0, 1.0, 3 * delay);
        assert_eq!(
            hits(&left),
            [(delay + frames, 1.0), (2 * delay + frames, 0.5)]
        );
        assert_eq!(hits(&right), [(delay, 1.0), (2 * delay, 0.5)]);
        // The wheel ends at half a second.
        assert_eq!(with([0, 128, 0, 64, 48, -512, 0, 128]).offset, [22_050, 0]);
    }

    #[test]
    fn the_pan_is_on_the_way_into_the_line_only() {
        // The doc's law next to the plugin's gains.
        for (pan, volume) in [
            (0, 128),
            (-128, 128),
            (128, 100),
            (66, 128),
            (-84, 43),
            (106, 103),
        ] {
            let (got, want) = (input_gains(pan, volume), pan_law(pan, volume));
            for (got, want) in got.iter().zip(want) {
                assert!((f64::from(*got) - want).abs() < 1e-6, "pan {pan}");
            }
        }
        assert_eq!(input_gains(0, 128), [1.0, 1.0, 0.0, 0.0]);
        // Hard left is both inputs on the left at half the volume.
        assert_eq!(input_gains(-128, 128), [0.5, 0.0, 0.5, 0.0]);
        assert_eq!(input_gains(128, 128), [0.0, 0.5, 0.0, 0.5]);
        // The volume wheel ends at 1.90.
        assert!((input_gains(0, 160)[0] - 1.9).abs() < 0.005);

        let delay = d(48, 44_100.0, 140.0);
        let mut plugin = with([-128, 128, 128, 0, 48, 0, 0, 128]);
        let (left, right) = impulse(&mut plugin, 0.0, 1.0, 2 * delay);
        // The dry signal stays on the right, and its echo is on the left.
        assert_eq!(hits(&left), [(delay, 0.5)]);
        assert_eq!(hits(&right), [(0, 1.0)]);
        // With the volume down nothing gets into the line.
        let mut plugin = with([0, 0, 128, 64, 48, 0, 0, 128]);
        let (left, right) = impulse(&mut plugin, 1.0, 1.0, 2 * delay);
        assert_eq!(
            (hits(&left), hits(&right)),
            (vec![(0, 1.0)], vec![(0, 1.0)])
        );
    }

    #[test]
    fn the_dry_level_is_the_knob_curve_and_at_0_only_the_echoes_are_left() {
        let delay = d(48, 44_100.0, 140.0);
        for (dry, want) in [
            (0, vec![]),
            (128, vec![(0, 1.0)]),
            (160, vec![(0, g(160) as f32)]),
        ] {
            let mut plugin = with([0, 128, dry, 0, 48, 0, 0, 128]);
            let (left, right) = impulse(&mut plugin, 1.0, 1.0, 2 * delay);
            let mut want = want.clone();
            want.push((delay, 1.0));
            assert_eq!(hits(&left), want, "dry {dry}");
            assert_eq!(left, right);
        }
        assert!((g(160) - 1.9).abs() < 0.005);
    }

    #[test]
    fn the_cutoff_darkens_every_pass_around_the_line() {
        // (21^(v / 128) - 1) * 0.05: 1 at the top, 0.80 at 119 and 0.37 at
        // 89, the doc's readings of the presets.
        assert_eq!(c(128, 44_100.0), 1.0);
        assert!((c(119, 44_100.0) - 0.80).abs() < 0.005);
        assert!((c(89, 44_100.0) - 0.37).abs() < 0.005);
        assert_eq!(c(0, 44_100.0), 0.0);

        let delay = d(48, 44_100.0, 140.0);
        let cut = c(64, 44_100.0);
        let mut plugin = with([0, 128, 0, 64, 48, 0, 0, 64]);
        let (left, _) = impulse(&mut plugin, 1.0, 1.0, 3 * delay);
        // The first pass is the low pass's answer to the impulse.
        for k in 0..50 {
            let want = cut * (1.0 - cut).powi(k);
            assert!((f64::from(left[delay + k as usize]) - want).abs() < 1e-6);
        }
        // The second is that through the low pass again, at half the
        // level: (k + 1) c^2 (1 - c)^k.
        for k in 0..50 {
            let want = 0.5 * f64::from(k + 1) * cut * cut * (1.0 - cut).powi(k);
            assert!((f64::from(left[2 * delay + k as usize]) - want).abs() < 1e-6);
        }
        assert_eq!(left[delay - 1], 0.0);
        // All the way down nothing gets through.
        let mut plugin = with([0, 128, 0, 64, 48, 0, 0, 0]);
        let (left, right) = impulse(&mut plugin, 1.0, 1.0, 3 * delay);
        assert_eq!((hits(&left), hits(&right)), (vec![], vec![]));
    }

    #[test]
    fn a_signal_comes_out_as_the_doc_s_maths_has_it() {
        let (left, right) = (noise(12_000, 1), noise(12_000, 2));
        for (params, hz, bpm) in [
            ([0, 128, 128, 64, 6, 0, 0, 128], 44_100.0, 140.0),
            ([-84, 100, 128, 100, 6, 200, 0, 90], 44_100.0, 140.0),
            ([66, 128, 128, 64, 15, -128, 2, 119], 44_100.0, 140.0),
            ([28, 128, 0, 107, 15, -358, 2, 89], 44_100.0, 174.0),
            ([106, 103, 128, 23, 6, -394, 2, 128], 44_100.0, 140.0),
            ([-40, 160, 160, 127, 9, 301, 1, 12], 48_000.0, 100.0),
            ([128, 43, 60, 90, 1, 0, 1, 128], 96_000.0, 522.0),
        ] {
            let mut plugin = at(params, hz as f32, bpm);
            let got = run(&mut plugin, &left, &right, 333);
            let want = by_the_doc(params, hz, bpm, &left, &right);
            assert_close(&got.0, &want.0, &format!("{params:?} left"));
            assert_close(&got.1, &want.1, &format!("{params:?} right"));
        }
    }

    #[test]
    fn a_moved_gain_or_feedback_ramps_over_one_block() {
        let delay = d(1, 44_100.0, 140.0);
        assert_eq!(delay, 98);
        // The input volume on its way down, heard in the echo.
        let mut plugin = with([0, 128, 0, 0, 1, 0, 0, 128]);
        plugin.set_param(1, 0.0);
        let (left, right) = run(&mut plugin, &[1.0; 102], &[1.0; 102], 4);
        assert_eq!(left[delay..], [1.0, 0.75, 0.5, 0.25]);
        assert_eq!(left, right);

        // The feedback on its way up. A block of ones fills the line, and
        // over the next one the feedback goes from 0 to a half.
        let mut plugin = with([0, 128, 0, 0, 1, 0, 0, 128]);
        plugin.process(&mut [1.0; 98], &mut [1.0; 98]);
        plugin.set_param(3, 64.0);
        let mut first = ([0.0; 98], [0.0; 98]);
        plugin.process(&mut first.0, &mut first.1);
        assert_eq!(first.0, [1.0; 98]);
        let mut second = ([0.0; 98], [0.0; 98]);
        plugin.process(&mut second.0, &mut second.1);
        for (k, got) in second.0.iter().enumerate() {
            assert!(
                (got - 0.5 * k as f32 / 98.0).abs() < 1e-6,
                "frame {k}: {got}"
            );
        }
        assert_eq!(second.0, second.1);
        // From there on it is a half.
        let mut third = ([0.0; 98], [0.0; 98]);
        plugin.process(&mut third.0, &mut third.1);
        for (got, before) in third.0.iter().zip(second.0) {
            assert_eq!(*got, 0.5 * before);
        }
    }

    #[test]
    fn a_line_of_another_length_starts_empty() {
        let delay = d(48, 44_100.0, 140.0);
        let params = [0, 128, 0, 64, 48, 0, 0, 128];
        let tail = |change: &dyn Fn(&mut Delay2)| {
            let mut plugin = with(params);
            impulse(&mut plugin, 1.0, 1.0, 100);
            change(&mut plugin);
            let (left, _) = impulse(&mut plugin, 0.0, 0.0, delay);
            hits(&left)
        };
        // Left alone, the echo comes D after the impulse.
        assert_eq!(tail(&|_| {}), [(delay - 100, 1.0)]);
        // The same tempo, rate or state again does not touch the line.
        assert_eq!(tail(&|p| p.set_tempo(140.0)), [(delay - 100, 1.0)]);
        assert_eq!(tail(&|p| p.set_sample_rate(44_100.0)), [(delay - 100, 1.0)]);
        assert_eq!(
            tail(&|p| p.set_state(&state(&params)).unwrap()),
            [(delay - 100, 1.0)]
        );
        assert_eq!(tail(&|p| p.set_param(7, 128.0)), [(delay - 100, 1.0)]);
        // Another tempo, time or offset wipes it, and so does a reset.
        assert_eq!(tail(&|p| p.set_tempo(141.0)), []);
        assert_eq!(tail(&|p| p.set_param(4, 49.0)), []);
        assert_eq!(tail(&|p| p.set_param(5, 100.0)), []);
        assert_eq!(tail(&|p| p.reset()), []);

        // A tempo FL does not have is held to the nearest it has.
        let mut plugin = with(params);
        plugin.set_tempo(1.0);
        assert_eq!(plugin.delay, d(48, 44_100.0, 10.0));
        plugin.set_tempo(f64::INFINITY);
        assert_eq!(plugin.delay, d(48, 44_100.0, 522.0));
        plugin.set_tempo(f64::NAN);
        plugin.set_sample_rate(0.0);
        plugin.set_sample_rate(f32::NAN);
        assert_eq!(plugin.delay, d(48, 44_100.0, 522.0));
    }

    #[test]
    fn no_wheel_asks_for_memory() {
        let mut plugin = Delay2::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_tempo(120.0);
        let memory = (plugin.line.as_ptr(), plugin.line.capacity());
        // The longest line the wheels can ask for.
        plugin.set_param(4, 768.0);
        plugin.set_param(5, -512.0);
        assert_eq!(plugin.line.len(), d(768, 48_000.0, 120.0) + 24_000 + 1);
        plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
        plugin.set_param(5, 512.0);
        plugin
            .set_state(&state(&[0, 128, 128, 127, 768, 512, 2, 0]))
            .unwrap();
        plugin.set_state(&state(&DEFAULTS)).unwrap();
        plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
        plugin.reset();
        assert_eq!((plugin.line.as_ptr(), plugin.line.capacity()), memory);
        // A slower tempo does.
        plugin.set_tempo(60.0);
        plugin.set_param(4, 768.0);
        assert!(plugin.line.capacity() > memory.1);
        assert_eq!(plugin.line.len(), d(768, 48_000.0, 60.0) + 1);
    }

    #[test]
    fn a_moved_parameter_is_the_same_plugin_as_its_state() {
        let mut plugin = Delay2::default();
        let moves = [66.2, 102.7, 0.0, 500.0, 0.0, -358.0, 2.0, 89.0];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
        }
        plugin.set_param(8, 1.0);
        assert_eq!(plugin.param(8), 0.0);
        plugin.process(&mut [0.0; 16], &mut [0.0; 16]);
        plugin.reset();
        assert_eq!(plugin, with([66, 103, 0, 127, 1, -358, 2, 89]));
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([28, 128, 0, 107, 48, -358, 2, 89]);
        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&[0; 28]),
            Err(StateError::Length {
                expected: 32,
                found: 28
            })
        );
        for (param, value) in [
            (0, -129),
            (0, 129),
            (1, 161),
            (2, -1),
            (3, 128),
            (4, 0),
            (4, 769),
            (5, 513),
            (6, 3),
            (7, 129),
        ] {
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
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(presets) = presets(NAME) else { return };
        // Four of them have the name in lower case.
        let want = [
            (
                "Default (send)",
                "Fruity delay 2",
                [0, 128, 0, 64, 144, 0, 0, 128],
            ),
            ("Default", "Fruity delay 2", DEFAULTS),
            (
                "Stereo effect (send)",
                "Fruity delay 2",
                [66, 128, 0, 64, 144, -128, 2, 119],
            ),
            (
                "Stereo effect",
                "Fruity Delay 2",
                [66, 128, 128, 64, 144, -128, 2, 119],
            ),
            (
                "Weird (send)",
                "Fruity delay 2",
                [28, 128, 0, 107, 48, -358, 2, 89],
            ),
            (
                "Widen",
                "Fruity Delay 2",
                [106, 103, 128, 23, 6, -394, 2, 128],
            ),
        ];
        assert_eq!(presets.len(), want.len());
        for (preset, (file, name, params)) in presets.iter().zip(want) {
            assert_eq!((preset.file.as_str(), preset.plugin.as_str()), (file, name));
            assert_eq!(preset.state, state(&params), "{file}");
            let mut plugin = with([0, 0, 0, 0, 1, 0, 0, 0]);
            plugin.set_state(&preset.state).unwrap();
            assert_eq!(plugin, with(params), "{file}");
        }
        // The doc's readings: 144 is 3 steps, 107 a feedback of 0.84, and
        // the offsets 2.3, 62 and 101 ms. Widen's echo is 13 ms at 140 BPM.
        assert_eq!(144 / 48, 3);
        assert!((107.0f64 / 128.0 - 0.84).abs() < 0.005);
        for (offset, ms) in [(-128, 2.3), (-358, 62.0), (-394, 101.0)] {
            let got = late(offset, 44_100.0) as f64 / 44.1;
            assert!((got - ms).abs() < 0.5, "offset {offset}: {got}");
        }
        assert!((d(6, 44_100.0, 140.0) as f64 / 44.1 - 13.0).abs() < 0.5);
        assert!((g(103) - 0.59).abs() < 0.005);
    }
}
