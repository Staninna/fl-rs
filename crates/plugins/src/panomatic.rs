//! Fruity PanOMatic: a pan and a volume that an LFO can move, as
//! `docs/plugins/fruity-panomatic.md` has them.
//!
//! The state is six little-endian i32, the plugin's six parameters:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Pan | 0 (left) to 256 (right) | 128 |
//! | 1 Volume | 0 to 320 | 256 |
//! | 2 LFO shape | 0 sine, 1 triangle, 2 square | 0 |
//! | 3 LFO target | 0 off, 1 pan, 2 volume | 0 |
//! | 4 LFO amount | 0 to 128 | 64 |
//! | 5 LFO speed | 0 to 55000 | 27500 |
//!
//! Pan and volume make a 2x2 matrix of gains, which [`gains`] works out.
//! Volume is `(11^(v / 256) - 1) / 10`: silent at 0, unity at 256 and +5.6 dB
//! at 320. The pan takes the side it leans away from down in a straight
//! line and folds what it takes off into the other side at half gain, so
//! hard left is `(L + R) / 2` on the left and nothing on the right. At the
//! defaults the matrix is unity and a block comes out as it went in.
//!
//! The LFO reads one of the host's tables of 16384 points. It moves once a
//! block, by the block's length, and the matrix goes to where it puts the
//! pan or the volume in a straight line over the block. At full amount it
//! swings the pan by all of its range either way, kept inside it, or the
//! gain of the volume by 0.5 either way, kept from going under 0.
//!
//! # The LFO's rate depends on the sample rate squared
//!
//! [`lfo_period`] is the LFO's period in samples, and FL's formula for it
//! has the sample rate in the denominator: `FPD_SetSampleRate` keeps
//! `2048 / rate` (`0x006c5e2e`) and the speed routine multiplies by it. A
//! period that is shorter in samples at a higher rate is shorter twice
//! over in seconds: the LFO of one project runs `(48 / 44.1)^2`, 18 %,
//! faster at 48 kHz than at 44.1 kHz. That is what FL does, and so does
//! this.
//!
//! Each gain of the matrix moves by no more than `0.001 * 44100 / rate` a
//! frame, the cap FL's renderer hands the plugin, so a fast LFO or a moved
//! knob may take more than a block to get there.

use crate::parts::Matrix;
use crate::three_osc::host_table;
use crate::{Effect, StateError, knob_gain, knob_value, read_state};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity PanOMatic";

/// The range of each parameter's control.
const RANGES: [(i32, i32); 6] = [(0, 256), (0, 320), (0, 2), (0, 2), (0, 128), (0, 55_000)];
const DEFAULTS: [i32; 6] = [128, 256, 0, 0, 64, 27_500];

/// The rate the plugin is made at, 44.1 kHz until the host says otherwise.
const BASE_RATE: f32 = 44_100.0;

/// The matrix for a pan from -1 to 1 and a linear gain: left to left, right
/// to right, right into left, left into right.
///
/// The side the pan leans to keeps `1 - |pan| / 2` of its own input and
/// takes `|pan| / 2` of the other, and the far side keeps `1 - |pan|`.
pub fn gains(pan: f32, gain: f32) -> [f32; 4] {
    let lean = pan.abs();
    let far = (1.0 - lean) * gain;
    let folded = lean * gain / 2.0;
    let near = gain - folded;
    if pan < 0.0 {
        [near, far, folded, 0.0]
    } else {
        [far, near, 0.0, folded]
    }
}

/// The LFO's period in samples for a speed and a sample rate in Hz
/// (`FUN_006c64b0`): `round((50001^(i / 65536) - 1) * i * 2048 / rate)`
/// with `i = 65536 - speed`. At 44.1 kHz that is an hour at speed 0, 21 s
/// at the default 27500, 0.54 s at 45614 and 52 ms at 55000.
///
/// The rate divides the count of samples, so the period in seconds goes
/// with one over the rate squared. See the module's note.
pub fn lfo_period(speed: i32, hz: f32) -> u32 {
    let i = 65_536 - speed;
    // `Exp(float(i / 65536) * Ln(50001))` in doubles, less 1, as a single.
    let x = f64::from((f64::from(i) / 65_536.0) as f32);
    let depth = ((x * 50_001f64.ln()).exp() - 1.0) as f32;
    // `0x180`, worked out in doubles; FL multiplies the three as singles
    // and rounds half to even (`CVTSD2SI`).
    let per_rate = (2048.0 / f64::from(hz)) as f32;
    let samples = depth * i as f32 * per_rate;
    (samples.round_ties_even() as u32).max(1)
}

/// Fruity PanOMatic. A new one has FL's defaults, which pass a block through
/// untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct PanOMatic {
    params: [i32; 6],
    matrix: Matrix,
    /// The pan knob, -1 to 1, and the gain of the volume knob.
    pan: f32,
    gain: f32,
    /// How far the LFO moves its target at the top of its table.
    amount: f32,
    /// The LFO's place in its cycle, 2^32 to a turn, and how far it moves
    /// in a sample.
    phase: u32,
    step: u32,
    hz: f32,
    /// The most a gain moves in a frame.
    cap: f32,
}

/// The most a gain of the matrix moves in a frame: `TAudioRenderer`'s
/// `0.001 * 44100 / rate` (engine info `+0x34`).
fn ramp_cap(hz: f32) -> f32 {
    (44_100.0 / f64::from(hz) * 0.001) as f32
}

impl Default for PanOMatic {
    fn default() -> Self {
        // The tables are made here, where asking for memory is allowed.
        host_table(0);
        let mut plugin = PanOMatic {
            params: DEFAULTS,
            matrix: Matrix::new([1.0, 1.0, 0.0, 0.0]),
            pan: 0.0,
            gain: 1.0,
            amount: 0.0,
            phase: 0,
            step: 0,
            hz: BASE_RATE,
            cap: ramp_cap(BASE_RATE),
        };
        plugin.update();
        plugin.matrix.settle();
        plugin
    }
}

impl PanOMatic {
    /// Work out what the parameters come to and aim the matrix at the pan
    /// and the volume as the knobs have them.
    fn update(&mut self) {
        let [pan, volume, _, target, amount, speed] = self.params;
        self.pan = (pan - 128) as f32 / 128.0;
        self.gain = match volume {
            256 => 1.0,
            _ => knob_gain(f64::from(volume) / 256.0) as f32,
        };
        self.amount = amount as f32 / if target == 1 { 128.0 } else { 256.0 };
        let period = u64::from(lfo_period(speed, self.hz));
        self.step = ((1u64 << 32) / period).min(u64::from(u32::MAX)) as u32;
        self.matrix.aim(gains(self.pan, self.gain));
    }
}

impl Effect for PanOMatic {
    /// The matrix is the state's from the first sample on, with no ramp,
    /// and the LFO starts its cycle.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.params = read_state(bytes, &RANGES)?;
        self.update();
        self.matrix.settle();
        self.phase = 0;
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.hz = hz;
        self.cap = ramp_cap(hz);
        self.update();
    }

    /// With the LFO on, it moves by the length of the block and the matrix
    /// follows it over the block. A long block therefore sees little of a
    /// fast LFO, as in FL: 512 samples at 44.1 kHz catch the fastest one
    /// four times a cycle.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let [_, _, shape, target, ..] = self.params;
        if target != 0 && len > 0 {
            let moved = self.step.wrapping_mul(len as u32);
            self.phase = self.phase.wrapping_add(moved);
            let table = host_table(shape as usize);
            let lfo = table[(self.phase >> 18) as usize] * self.amount;
            self.matrix.aim(match target {
                1 => gains((self.pan + lfo).clamp(-1.0, 1.0), self.gain),
                _ => gains(self.pan, (self.gain + lfo).max(0.0)),
            });
        }
        self.matrix.apply_capped(left, right, self.cap);
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
            self.update();
        }
    }

    /// Ends a ramp that is under way and puts the LFO at the start of its
    /// cycle, as FL's flush does.
    fn reset(&mut self) {
        self.matrix.settle();
        self.phase = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fidelity;
    use crate::testing::{self, bits, state};
    use dsp::gain_to_db;

    fn with(params: [i32; 6]) -> PanOMatic {
        let mut plugin = PanOMatic::default();
        plugin.set_state(&state(&params)).unwrap();
        plugin
    }

    /// The same with no cap on a ramp, so that every block gets where it
    /// is aimed, for the tests of where that is.
    fn uncapped(params: [i32; 6]) -> PanOMatic {
        let mut plugin = with(params);
        plugin.cap = f32::MAX;
        plugin
    }

    /// A plugin with this pan and volume and the LFO off.
    fn still(pan: i32, volume: i32) -> PanOMatic {
        with([pan, volume, 0, 0, 64, 27_500])
    }

    /// What one frame comes out as.
    fn frame(plugin: &mut PanOMatic, left: f32, right: f32) -> (f32, f32) {
        let (mut l, mut r) = ([left], [right]);
        plugin.process(&mut l, &mut r);
        (l[0], r[0])
    }

    fn close(got: f32, want: f32, within: f32) -> bool {
        (got - want).abs() < within
    }

    #[test]
    fn the_defaults_are_unity_to_the_bit() {
        assert_eq!(gains(0.0, 1.0), [1.0, 1.0, 0.0, 0.0]);
        let input = [0.0, -0.0, 1.0, -1.0, 0.3, -1e-30, 7.5, f32::MIN_POSITIVE];
        for mut plugin in [PanOMatic::default(), still(128, 256)] {
            assert_eq!(plugin.matrix, Matrix::new([1.0, 1.0, 0.0, 0.0]));
            let (mut left, mut right) = (input, input);
            right.reverse();
            plugin.process(&mut left, &mut right);
            assert_eq!(bits(&left), bits(&input));
            right.reverse();
            assert_eq!(bits(&right), bits(&input));
            assert_eq!(plugin.fidelity(), Fidelity::Exact);
        }
        let params: Vec<f32> = (0..7).map(|i| PanOMatic::default().param(i)).collect();
        assert_eq!(params, [128.0, 256.0, 0.0, 0.0, 64.0, 27_500.0, 0.0]);
    }

    #[test]
    fn the_volume_is_the_knob_curve() {
        assert_eq!(frame(&mut still(128, 0), 1.0, -1.0), (0.0, -0.0));
        // The doc's numbers: -12.7 dB at 128 and +5.6 dB at 320.
        let (left, right) = frame(&mut still(128, 128), 1.0, 1.0);
        assert_eq!(left, right);
        assert!(close(gain_to_db(left), -12.7, 0.05));
        let (left, _) = frame(&mut still(128, 320), 1.0, 1.0);
        assert!(close(left, (11f32.powf(1.25) - 1.0) / 10.0, 1e-6));
        assert!(close(gain_to_db(left), 5.6, 0.02));
        for volume in [1, 60, 222, 239, 255, 257] {
            let want = (11f32.powf(volume as f32 / 256.0) - 1.0) / 10.0;
            let (left, right) = frame(&mut still(128, volume), 1.0, 1.0);
            assert!(close(left, want, 1e-6), "volume {volume}: {left}");
            assert_eq!(left, right);
        }
    }

    #[test]
    fn hard_left_folds_the_right_side_into_the_left_at_half_gain() {
        assert_eq!(gains(-1.0, 1.0), [0.5, 0.0, 0.5, 0.0]);
        // Left alone, right alone and both: L' = (L + R) / 2, R' = 0.
        assert_eq!(frame(&mut still(0, 256), 1.0, 0.0), (0.5, 0.0));
        assert_eq!(frame(&mut still(0, 256), 0.0, 1.0), (0.5, 0.0));
        assert_eq!(frame(&mut still(0, 256), 0.5, 0.25), (0.375, 0.0));
        // A mono source keeps its level.
        assert_eq!(frame(&mut still(0, 256), 0.8, 0.8), (0.8, 0.0));
        // Hard right is the mirror image.
        assert_eq!(gains(1.0, 1.0), [0.0, 0.5, 0.0, 0.5]);
        assert_eq!(frame(&mut still(256, 256), 1.0, 0.0), (0.0, 0.5));
        assert_eq!(frame(&mut still(256, 256), 0.25, 0.5), (0.0, 0.375));
        // The volume goes on all of it.
        let half = (11f32.sqrt() - 1.0) / 10.0;
        let (left, right) = frame(&mut still(0, 128), 1.0, 1.0);
        assert!(close(left, half, 1e-6) && right == 0.0);
    }

    #[test]
    fn half_left_takes_the_right_side_half_way_down() {
        // The doc's law at p = -0.5: outL = 0.75 L + 0.25 R, outR = 0.5 R.
        assert_eq!(gains(-0.5, 1.0), [0.75, 0.5, 0.25, 0.0]);
        assert_eq!(frame(&mut still(64, 256), 1.0, 0.0), (0.75, 0.0));
        assert_eq!(frame(&mut still(64, 256), 0.0, 1.0), (0.25, 0.5));
        assert_eq!(gains(0.5, 2.0), [1.0, 1.5, 0.0, 0.5]);
        // The corpus's 119 is a hair left.
        let [ll, rr, rl, lr] = gains(-9.0 / 128.0, 1.0);
        assert!(close(ll, 1.0 - 4.5 / 128.0, 1e-6) && close(rr, 1.0 - 9.0 / 128.0, 1e-6));
        assert!(close(rl, 4.5 / 128.0, 1e-6) && lr == 0.0);
    }

    #[test]
    fn the_lfo_s_period_is_the_doc_s_formula() {
        let hz = |speed: i32| 44_100.0 / f64::from(lfo_period(speed, 44_100.0));
        // The doc's 0.29 mHz, 0.047 Hz and 19.2 Hz. For 45614, the speed
        // most corpus states have, the doc says 1.1 Hz and its formula
        // gives 1.85.
        assert!((hz(0) * 1000.0 - 0.29).abs() < 0.005, "{}", hz(0));
        assert!((hz(27_500) - 0.047).abs() < 0.0005, "{}", hz(27_500));
        assert!((hz(45_614) - 1.85).abs() < 0.01, "{}", hz(45_614));
        assert!((hz(55_000) - 19.2).abs() < 0.05, "{}", hz(55_000));
        // Worked out here in doubles, for the fastest and the favourite.
        for speed in [55_000, 45_614, 40_490] {
            let i = f64::from(65_536 - speed);
            let want = (50_001f64.powf(i / 65_536.0) - 1.0) * i * 2048.0 / 44_100.0;
            let got = f64::from(lfo_period(speed, 44_100.0));
            assert!((got - want).abs() <= 1.0, "speed {speed}: {got}, {want}");
        }
        // The rate is in the period, so the LFO is faster by the rates'
        // ratio squared.
        let ratio =
            f64::from(lfo_period(45_614, 44_100.0)) / f64::from(lfo_period(45_614, 48_000.0));
        assert!((ratio - 48_000.0 / 44_100.0).abs() < 1e-3, "{ratio}");
    }

    #[test]
    fn an_lfo_on_the_volume_has_the_formula_s_period_at_44_1_khz() {
        // A sine at full amount on the volume: the gain is 1 + 0.5 sin.
        let speed = 55_000;
        let mut plugin = with([128, 256, 0, 2, 128, speed]);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        let period = lfo_period(speed, 44_100.0);
        assert_eq!(period, 2297);
        // Forty turns in blocks of 4 frames of 1.0, so the output is the
        // gain.
        let frames = 40 * period as usize;
        let (mut left, mut right) = (vec![1.0f32; frames], vec![1.0f32; frames]);
        for (l, r) in left.chunks_mut(4).zip(right.chunks_mut(4)) {
            plugin.process(l, r);
        }
        assert_eq!(left, right);
        let (low, high) = left.iter().fold((f32::MAX, f32::MIN), |(low, high), v| {
            (low.min(*v), high.max(*v))
        });
        assert!(
            close(low, 0.5, 1e-3) && close(high, 1.5, 1e-3),
            "{low} {high}"
        );
        // Where the gain comes up through 1.
        let up: Vec<usize> = (1..frames)
            .filter(|n| left[n - 1] < 1.0 && left[*n] >= 1.0)
            .collect();
        assert!((39..=40).contains(&up.len()), "{}", up.len());
        let turns = (up.len() - 1) as f64;
        let measured = (up[up.len() - 1] - up[0]) as f64 / turns;
        assert!(
            (measured - f64::from(period)).abs() < 0.5,
            "{measured} samples a turn, wanted {period}"
        );
    }

    #[test]
    fn the_lfo_moves_once_a_block_and_the_matrix_ramps_to_it() {
        // A square on the volume at the fastest speed: 2297 samples a
        // turn. The first block of 100 is in the high half.
        let mut plugin = uncapped([128, 256, 2, 2, 128, 55_000]);
        let (mut left, mut right) = ([1.0f32; 100], [1.0f32; 100]);
        plugin.process(&mut left, &mut right);
        // From unity towards 1.5 in a straight line over the block.
        for (n, v) in left.iter().enumerate() {
            assert!(close(*v, 1.0 + 0.5 * n as f32 / 100.0, 1e-5), "{n}: {v}");
        }
        assert_eq!(left, right);
        // The next block starts where the ramp ended and stays.
        let (mut left, mut right) = ([1.0f32; 100], [1.0f32; 100]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [1.5; 100]);
        // Nine more blocks end 48 samples short of half a turn. The next
        // one is past it, where the square is low, and ramps from 1.5 to
        // 0.5.
        for _ in 0..9 {
            plugin.process(&mut [1.0; 100], &mut [1.0; 100]);
        }
        let (mut left, mut right) = ([1.0f32; 100], [1.0f32; 100]);
        plugin.process(&mut left, &mut right);
        assert!(close(left[0], 1.5, 1e-5) && close(left[50], 1.0, 1e-5));
        assert!(close(frame(&mut plugin, 1.0, 1.0).0, 0.5, 1e-5));
        // An empty block moves nothing.
        let before = plugin.clone();
        plugin.process(&mut [], &mut []);
        assert_eq!(plugin, before);
        // A reset puts the LFO back at the start and ends the ramp.
        plugin.reset();
        assert_eq!(plugin.phase, 0);
        assert!(close(frame(&mut plugin, 1.0, 1.0).0, 0.5, 1e-5));
    }

    #[test]
    fn an_lfo_on_the_pan_swings_it_and_stays_inside_its_range() {
        // A square at full amount from the centre: hard right, then hard
        // left. Blocks of one frame, so every frame is where the LFO is.
        let mut plugin = uncapped([128, 256, 2, 1, 128, 55_000]);
        plugin.process(&mut [0.0], &mut [0.0]);
        assert_eq!(frame(&mut plugin, 1.0, 0.0), (0.0, 0.5));
        for _ in 0..1200 {
            plugin.process(&mut [0.0], &mut [0.0]);
        }
        assert_eq!(frame(&mut plugin, 0.0, 1.0), (0.5, 0.0));
        // From hard right the high half has nowhere to go, and the low
        // half comes back to the centre.
        let mut plugin = uncapped([256, 256, 2, 1, 128, 55_000]);
        plugin.process(&mut [0.0], &mut [0.0]);
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (0.0, 1.0));
        for _ in 0..1200 {
            plugin.process(&mut [0.0], &mut [0.0]);
        }
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (1.0, 1.0));
        // Half the amount on the volume is a quarter of the gain, and a
        // volume at 0 does not go under it.
        let mut plugin = uncapped([128, 256, 2, 2, 64, 55_000]);
        plugin.process(&mut [0.0], &mut [0.0]);
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (1.25, 1.25));
        let mut plugin = uncapped([128, 0, 2, 2, 128, 55_000]);
        for _ in 0..1200 {
            plugin.process(&mut [0.0], &mut [0.0]);
        }
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (0.0, 0.0));
    }

    /// `FPD_SetSampleRate` keeps `2048 / rate` and the period in samples
    /// is multiplied by it, so the LFO turns faster by the rates' ratio
    /// squared. That is FL's, at any rate.
    #[test]
    fn the_lfo_s_rate_goes_with_the_sample_rate_squared() {
        let mut plugin = with([128, 256, 1, 1, 87, 45_614]);
        plugin.set_sample_rate(48_000.0);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        let step = |hz: f32| ((1u64 << 32) / u64::from(lfo_period(45_614, hz))) as u32;
        assert_eq!(plugin.step, step(48_000.0));
        let turns = |hz: f32| f64::from(hz) / f64::from(lfo_period(45_614, hz));
        let ratio = turns(48_000.0) / turns(44_100.0);
        assert!((ratio - (48.0f64 / 44.1).powi(2)).abs() < 2e-3, "{ratio}");
        // The cap on a gain's step goes with one over the rate.
        assert_eq!(plugin.cap, (44_100.0f64 / 48_000.0 * 0.001) as f32);
    }

    /// A gain moves by no more than the cap a frame, so a moved knob takes
    /// as long as it needs, whatever the blocks.
    #[test]
    fn a_ramp_is_held_to_the_cap() {
        let mut plugin = PanOMatic::default();
        plugin.set_param(1, 0.0);
        let mut left = vec![1.0f32; 1500];
        let mut right = left.clone();
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        // From 1 at 0.001 a frame over the first block, then the rest of
        // the way, under the cap, over the second.
        assert!((left[500] - 0.5).abs() < 1e-5, "{}", left[500]);
        let rest = 1.0 - 0.512f32;
        assert!((left[1023] - rest / 512.0).abs() < 1e-5, "{}", left[1023]);
        assert_eq!(&left[1024..], &[0.0; 476][..]);
        assert_eq!(left, right);
    }

    #[test]
    fn a_moved_parameter_ramps_over_one_block() {
        let mut plugin = uncapped(DEFAULTS);
        plugin.set_param(1, 0.0);
        let (mut left, mut right) = ([1.0; 4], [-1.0; 4]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [1.0, 0.75, 0.5, 0.25]);
        assert_eq!(right, [-1.0, -0.75, -0.5, -0.25]);
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (0.0, 0.0));
        // Switching the LFO off goes back to where the knobs are.
        let mut plugin = uncapped([128, 256, 2, 2, 128, 55_000]);
        plugin.process(&mut [0.0], &mut [0.0]);
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (1.5, 1.5));
        plugin.set_param(3, 0.0);
        plugin.process(&mut [0.0], &mut [0.0]);
        assert_eq!(frame(&mut plugin, 1.0, 1.0), (1.0, 1.0));
    }

    #[test]
    fn parameters_are_whole_numbers_inside_their_ranges() {
        let mut plugin = PanOMatic::default();
        assert_eq!(plugin.param_count(), 6);
        for (index, (low, high)) in RANGES.into_iter().enumerate() {
            plugin.set_param(index, -1e9);
            assert_eq!(plugin.param(index), low as f32);
            plugin.set_param(index, 1e9);
            assert_eq!(plugin.param(index), high as f32);
        }
        plugin.set_param(0, 119.4);
        plugin.set_param(6, 1.0);
        assert_eq!((plugin.param(0), plugin.param(6)), (119.0, 0.0));
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([119, 100, 1, 1, 87, 45_614]);
        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&[0; 23]),
            Err(StateError::Length {
                expected: 24,
                found: 23
            })
        );
        for (param, value) in [(0, 257), (1, 321), (2, 3), (3, -1), (4, 129), (5, 55_001)] {
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
    fn every_state_fl_ships_loads_and_runs() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states of {NAME} skipped");
            return;
        };
        let named = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects: Vec<_> = testing::projects(&root).into_iter().filter(named).collect();
        let presets = testing::every_preset(&root, NAME);
        // The doc's counts: 193 uses in the projects, and the Default
        // preset with the three templates.
        assert_eq!((projects.len(), presets.len()), (193, 4));
        let mut moving = [0; 3];
        for saved in projects.iter().chain(&presets) {
            let mut plugin = PanOMatic::default();
            plugin.set_sample_rate(48_000.0);
            assert_eq!(saved.state.len(), 24, "{}", saved.file);
            if let Err(error) = plugin.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            moving[plugin.param(3) as usize] += 1;
            let (mut left, mut right) = (testing::sine(0.01, 1024), testing::sine(0.037, 1024));
            for (l, r) in left.chunks_mut(300).zip(right.chunks_mut(300)) {
                plugin.process(l, r);
            }
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 4.0),
                "{}",
                saved.file
            );
        }
        // The LFO is on the pan in 26 and on the volume in 4. The doc's
        // table has 14 and 3.
        assert_eq!(moving, [167, 26, 4]);
        let default = presets
            .iter()
            .find(|saved| saved.file == "Default")
            .unwrap();
        assert_eq!(default.state, state(&[127, 239, 0, 0, 64, 27_500]));
    }
}
