//! Fruity Blood Overdrive: a band of the input made louder, split in
//! three, each part bent through a quarter of a sine, and two one-pole
//! low-passes after it, as `docs/plugins/fruity-blood-overdrive.md` has it.
//!
//! The state is 36 bytes: a little-endian i32 version, 3, the plugin's six
//! parameters as i32, the number of the editor's colour scheme, and a word
//! that is 0. FL reads a version under 3 as 32 bytes with no colour scheme,
//! and leaves a newer one alone.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Pre band | 0 to 10000 | 0 |
//! | 1 Color | 0 to 10000 | 5000 |
//! | 2 Pre amp | 0 to 10000 | 0 |
//! | 3 x100 | 0 (off) or 1 (on) | 0 |
//! | 4 Post filter | 0 to 10000 | 0 |
//! | 5 Post gain | 0 to 10000 | 10000 |
//!
//! With every wheel over 10000 as `x`, each side goes through
//!
//! ```text
//! low  += F * band;  high = in - low - band;  band += F * high      F = color^3
//! x     = ((high + low) * B + band) * G                             B = 1 - band wheel, G = 1 + 25 amp^3
//! low2 += band2 / 4; high2 = x - low2 - band2; band2 += high2 / 4
//! y     = clamp((shape(low2) + shape(band2) + shape(high2)) * P)    P = gain^2, to ±0.99995
//! lp1   = y * (1 - a) + lp1 * a;  out = lp2 = lp1 * (1 - a) + lp2 * a     a = 0.2 + 0.7 filter
//! ```
//!
//! where `shape(v)` is `sin(v * pi / 2)` out of a table of 65536 steps,
//! held at its ends, and x100 makes the 25 a 2500. `G`, `B`, `P` and `F`
//! each follow their wheel through a one-pole that is 99 % there after
//! 18,400 samples. Nothing here follows the sample rate, as nothing does in
//! FL: the colour band is at 878 Hz at 44.1 kHz and at 956 Hz at 48 kHz.
//!
//! The shaper is in the path with the pre amp at 0 too, so the plugin
//! never passes its input: a small level comes out `pi / 2` times as loud,
//! in steps of the table.
//!
//! A plugin FL has just made starts its amp, its blend and its post gain at
//! 1 and its colour at 0 and goes to the wheels from there. This one starts
//! where a loaded state does, on the wheels.

use crate::{Effect, StateError, knob_value, state_version};
use std::sync::LazyLock;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Blood Overdrive";

/// The range of each parameter's control.
const RANGES: [(i32, i32); 6] = [
    (0, 10_000),
    (0, 10_000),
    (0, 10_000),
    (0, 1),
    (0, 10_000),
    (0, 10_000),
];
const DEFAULTS: [i32; 6] = [0, 5000, 0, 0, 0, 10_000];

/// How long the state is, and how long with no colour scheme in it.
const LEN: usize = 36;
const OLD_LEN: usize = 32;
/// The newest version FL reads.
const VERSION: i32 = 3;

/// What a smoother keeps of itself every frame, and what it takes of the
/// value it goes to.
const KEEP: f32 = 0.99975;
const TAKE: f32 = 0.00025;

/// The drive filter's coefficient.
const DRIVE: f32 = 0.25;

/// The most the shaped sum is let through at.
const CEILING: f32 = 0.99995;

/// The shaper: entry `0x8000 + i` is `sin(i * pi / 65536)` and entry
/// `0x7fff - i` is that under 0, so a level under 0 comes out one step
/// nearer to 0 than the same level above it.
static TABLE: LazyLock<Box<[f32]>> = LazyLock::new(|| {
    let mut table = vec![0.0; 0x1_0000];
    for i in 0..0x8000 {
        let value = (i as f64 * std::f64::consts::PI / 65_536.0).sin() as f32;
        table[0x7fff - i] = -value;
        table[0x8000 + i] = value;
    }
    table.into_boxed_slice()
});

/// `sin(v * pi / 2)` on the table's grid: `v` is cut to a whole number of
/// 32767ths, towards 0, and held at ±1.
fn shape(table: &[f32], v: f32) -> f32 {
    let steps = f64::from(v * 32_767.0);
    // What FL's conversion leaves in 32 bits: nothing of a number that is
    // none or does not fit 64.
    let steps = if steps.abs() < 9.2e18 {
        steps as i64 as i32
    } else {
        0
    };
    table[steps.wrapping_add(0x8000).clamp(0, 0xffff) as usize]
}

/// What the smoothers go to, and the post filter's coefficient.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Targets {
    /// The amp gain, the band blend, the post gain and the colour
    /// coefficient.
    gain: f32,
    blend: f32,
    post: f32,
    colour: f32,
    /// `a` of the post filter.
    keep: f32,
}

impl Targets {
    fn new(params: &[i32; 6]) -> Self {
        let x = |index: usize| (f64::from(params[index]) / 10_000.0) as f32;
        let (band, colour, amp, filter, gain) = (x(0), x(1), x(2), x(4), x(5));
        let times = if params[3] == 1 { 2500.0 } else { 25.0 };
        Targets {
            gain: times * (amp * amp * amp) + 1.0,
            blend: 1.0 - band,
            post: gain * gain,
            colour: colour * colour * colour,
            keep: filter * 0.7 + 0.2,
        }
    }
}

/// The filters of one side.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Side {
    /// The colour filter's low and band, and the drive filter's.
    low: f32,
    band: f32,
    low2: f32,
    band2: f32,
    /// The two one-poles of the post filter.
    lp1: f32,
    lp2: f32,
}

impl Side {
    fn step(&mut self, input: f32, live: &Live, keep: f32, table: &[f32]) -> f32 {
        self.low += live.colour * self.band;
        let high = input - self.low - self.band;
        self.band += live.colour * high;
        let x = ((high + self.low) * live.blend + self.band) * live.gain;

        self.low2 += self.band2 * DRIVE;
        let high2 = x - self.low2 - self.band2;
        self.band2 += high2 * DRIVE;
        let shaped = shape(table, self.low2) + shape(table, self.band2) + shape(table, high2);
        let y = (shaped * live.post).clamp(-CEILING, CEILING);
        self.lp1 = y * (1.0 - keep) + self.lp1 * keep;
        self.lp2 = self.lp1 * (1.0 - keep) + self.lp2 * keep;
        self.lp2
    }

    /// A number too small for an f32 to hold in full becomes 0, before it
    /// slows the sums down. FL runs with such numbers rounded away.
    fn flush(&mut self) {
        let states = [
            &mut self.low,
            &mut self.band,
            &mut self.low2,
            &mut self.band2,
            &mut self.lp1,
            &mut self.lp2,
        ];
        for state in states {
            if state.is_subnormal() {
                *state = 0.0;
            }
        }
    }
}

/// The four smoothed values, as the last frame left them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Live {
    gain: f32,
    blend: f32,
    post: f32,
    colour: f32,
}

impl Live {
    /// One frame nearer to the targets.
    fn step(&mut self, to: &Targets) {
        self.gain = self.gain * KEEP + to.gain * TAKE;
        self.blend = self.blend * KEEP + to.blend * TAKE;
        self.post = self.post * KEEP + to.post * TAKE;
        self.colour = self.colour * KEEP + to.colour * TAKE;
    }
}

/// Fruity Blood Overdrive. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct BloodOverdrive {
    params: [i32; 6],
    /// The editor's colour scheme, which the state carries and nothing
    /// here reads.
    scheme: i32,
    hz: f64,
    targets: Targets,
    live: Live,
    sides: [Side; 2],
}

impl Default for BloodOverdrive {
    fn default() -> Self {
        // The table is made here and not by the first block.
        LazyLock::force(&TABLE);
        let targets = Targets::new(&DEFAULTS);
        let mut plugin = BloodOverdrive {
            params: DEFAULTS,
            scheme: 0,
            hz: 44_100.0,
            targets,
            live: Live {
                gain: 1.0,
                blend: 1.0,
                post: 1.0,
                colour: 0.0,
            },
            sides: [Side::default(); 2],
        };
        // Until the smoothers stay where they are, so that loading the
        // defaults changes nothing. They get there, as none of them ever
        // turns back.
        for _ in 0..64 {
            let before = plugin.live;
            plugin.settle();
            if plugin.live == before {
                break;
            }
        }
        plugin
    }
}

impl BloodOverdrive {
    /// Take parameters that are inside their ranges.
    fn set(&mut self, params: [i32; 6]) {
        self.params = params;
        self.targets = Targets::new(&params);
    }

    /// What FL does after it has read a state: run the smoothers for as
    /// many frames as a second has, which leaves them on their targets.
    fn settle(&mut self) {
        for _ in 0..self.hz as usize {
            self.live.step(&self.targets);
        }
    }
}

impl Effect for BloodOverdrive {
    /// The smoothed values are the state's from the first sample on.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        let expected = if version == VERSION { LEN } else { OLD_LEN };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let words = bytes.as_chunks::<4>().0;
        let mut params = [0; 6];
        for (param, range) in RANGES.iter().enumerate() {
            let value = i32::from_le_bytes(words[1 + param]);
            if value < range.0 || value > range.1 {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        if version == VERSION {
            self.scheme = i32::from_le_bytes(words[7]);
        }
        self.set(params);
        self.settle();
        Ok(())
    }

    /// Only how long a state takes to settle depends on the rate. A rate
    /// that is not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let table: &[f32] = &TABLE;
        let keep = self.targets.keep;
        let [left_side, right_side] = &mut self.sides;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.live.step(&self.targets);
            *l = left_side.step(*l, &self.live, keep, table);
            *r = right_side.step(*r, &self.live, keep, table);
        }
        left_side.flush();
        right_side.flush();
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// The amp, the blend, the post gain and the colour go to a moved
    /// wheel through their smoothers. The post filter is there at once.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    /// Empties the filters and puts the smoothed values on the wheels.
    /// FL's own flush leaves both as they are.
    fn reset(&mut self) {
        self.sides = [Side::default(); 2];
        self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, presets, sine, state};
    use std::f64::consts::FRAC_PI_2;

    /// A state of version 3 with these parameters.
    fn bytes(params: [i32; 6]) -> Vec<u8> {
        let mut bytes = state(&[3]);
        bytes.extend(state(&params));
        bytes.extend(state(&[0, 0]));
        bytes
    }

    fn with(params: [i32; 6]) -> BloodOverdrive {
        let mut plugin = BloodOverdrive::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin
    }

    /// What a level that does not move comes out as, once the filters have
    /// settled on it.
    fn settled(params: [i32; 6], level: f32) -> (f32, f32) {
        let mut plugin = with(params);
        let (mut l, mut r) = (vec![level; 6000], vec![-level; 6000]);
        plugin.process(&mut l, &mut r);
        (l[5999], r[5999])
    }

    /// The doc's chain for one side, in doubles and with the sine worked
    /// out and not looked up. `live` is `[G, B, P, F]`.
    fn model(input: &[f32], live: [f64; 4], a: f64) -> Vec<f64> {
        let [gain, blend, post, colour] = live;
        let fold = |v: f64| (v.clamp(-1.0, 1.0) * FRAC_PI_2).sin();
        let (mut low, mut band, mut low2, mut band2, mut lp1, mut lp2) =
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let mut out = Vec::new();
        for x in input {
            low += colour * band;
            let high = f64::from(*x) - low - band;
            band += colour * high;
            let x = ((high + low) * blend + band) * gain;
            low2 += 0.25 * band2;
            let high2 = x - low2 - band2;
            band2 += 0.25 * high2;
            let y = (fold(low2) + fold(band2) + fold(high2)) * post;
            lp1 = y.clamp(-0.99995, 0.99995) * (1.0 - a) + lp1 * a;
            lp2 = lp1 * (1.0 - a) + lp2 * a;
            out.push(lp2);
        }
        out
    }

    #[test]
    fn the_shaper_is_a_quarter_sine_on_a_16_bit_grid() {
        let table: &[f32] = &TABLE;
        assert_eq!(table.len(), 65_536);
        assert_eq!((shape(table, 0.0), shape(table, -0.0)), (0.0, 0.0));
        // Held at both ends. The last step is 1 as near as an f32 has it.
        let top = (32_767.0 * std::f64::consts::PI / 65_536.0).sin() as f32;
        assert_eq!((shape(table, 1.0), shape(table, 7.5)), (top, top));
        assert_eq!((shape(table, -1.5), shape(table, -3.0)), (-top, -top));
        assert_eq!(top, 1.0);
        // A number FL's conversion makes nothing of.
        assert_eq!((shape(table, f32::NAN), shape(table, 1e30)), (0.0, 0.0));
        assert_eq!(shape(table, -1e30), 0.0);
        // In between, the sine of the step under the level.
        for v in [0.001f32, 0.25, 0.5, 0.9, 0.999] {
            let step = f64::from(v * 32_767.0).trunc();
            let wanted = (step * std::f64::consts::PI / 65_536.0).sin() as f32;
            assert_eq!(shape(table, v), wanted, "{v}");
            assert!((f64::from(wanted) - (f64::from(v) * FRAC_PI_2).sin()).abs() < 1e-4);
            // Under 0 it is one step nearer to 0.
            let under = ((step - 1.0) * std::f64::consts::PI / 65_536.0).sin() as f32;
            assert_eq!(shape(table, -v), -under, "{v}");
        }
        // Under one step, nothing comes out.
        assert_eq!((shape(table, 2e-5), shape(table, -2e-5)), (0.0, 0.0));
    }

    #[test]
    fn the_wheels_come_to_the_doc_s_gains() {
        let targets = |params: [i32; 6]| Targets::new(&params);
        let reset = targets(DEFAULTS);
        assert_eq!((reset.gain, reset.blend, reset.post), (1.0, 1.0, 1.0));
        assert_eq!((reset.colour, reset.keep), (0.125, 0.2));
        // "Drive": 26 times, 15 % of the rest, a quarter after.
        let drive = targets([8500, 5000, 10_000, 0, 5000, 5000]);
        assert_eq!((drive.gain, drive.post), (26.0, 0.25));
        assert!((drive.blend - 0.15).abs() < 1e-6 && (drive.keep - 0.55).abs() < 1e-6);
        // The amp wheel is cubed, and x100 makes the 25 a 2500.
        assert!((targets([0, 0, 2000, 0, 0, 0]).gain - 1.2).abs() < 1e-6);
        assert!((targets([0, 0, 2578, 1, 0, 0]).gain - 43.83).abs() < 0.01);
        assert_eq!(targets([0, 0, 10_000, 1, 10_000, 0]).gain, 2501.0);
        assert!((targets([0, 0, 0, 0, 10_000, 0]).keep - 0.9).abs() < 1e-6);
    }

    #[test]
    fn a_loaded_state_is_on_its_targets_to_a_hair() {
        // The smoother stops short of where it goes: near it, what a frame
        // would add is less than an f32 can take. That is a few parts in
        // 10,000, in FL as here.
        let plugin = with([2000, 7000, 5000, 0, 0, 8000]);
        let to = plugin.targets;
        for (live, target) in [
            (plugin.live.gain, to.gain),
            (plugin.live.blend, to.blend),
            (plugin.live.post, to.post),
            (plugin.live.colour, to.colour),
        ] {
            assert!((live - target).abs() < 5e-4 * target, "{live} {target}");
        }
        assert_eq!(BloodOverdrive::default(), with(DEFAULTS));
    }

    #[test]
    fn a_level_that_stays_comes_out_as_its_sine_times_the_post_gain() {
        let table: &[f32] = &TABLE;
        // With the wheels at their defaults nothing is made louder, and
        // the shaper is still there: a quarter comes out as sin(pi / 8).
        let (left, right) = settled(DEFAULTS, 0.25);
        assert!((left - shape(table, 0.25)).abs() < 1e-4, "{left}");
        assert!((f64::from(left) - (FRAC_PI_2 * 0.25).sin()).abs() < 1e-4);
        assert!((right + left).abs() < 1e-4);
        // A small level is pi / 2 times as loud, to a step of the table.
        let (left, _) = settled(DEFAULTS, 0.01);
        assert!((f64::from(left) - 0.01 * FRAC_PI_2).abs() < 1e-4, "{left}");
        // And under one step there is nothing.
        assert_eq!(settled(DEFAULTS, 2e-5).0, 0.0);

        // The amp pushes the level over the knee, and the ceiling holds
        // what the post gain lets through.
        let (left, right) = settled([0, 5000, 5000, 0, 0, 10_000], 0.25);
        assert!((left - CEILING).abs() < 1e-5 && (right + CEILING).abs() < 1e-5);
        let (left, _) = settled([0, 5000, 5000, 0, 0, 5000], 0.25);
        assert!((left - 0.25 * shape(table, 1.0)).abs() < 1e-4, "{left}");
        // With the band wheel all the way up only the colour band is left,
        // and a level that stays has none of it.
        let (left, _) = settled([10_000, 5000, 5000, 0, 0, 10_000], 0.25);
        assert!(left.abs() < 1e-3, "{left}");
    }

    #[test]
    fn a_tone_goes_through_the_doc_s_chain() {
        // A loud and a quiet tone and a click, against the chain in
        // doubles. What is left between the two is the table's grid.
        for params in [
            DEFAULTS,
            [4000, 5000, 2000, 0, 2500, 10_000],
            [8500, 5000, 10_000, 0, 5000, 5000],
            [2775, 1586, 791, 1, 0, 2465],
            [0, 10_000, 3000, 0, 10_000, 7000],
        ] {
            let mut plugin = with(params);
            let live = plugin.live;
            let live = [live.gain, live.blend, live.post, live.colour].map(f64::from);
            let a = f64::from(plugin.targets.keep);
            let mut left: Vec<f32> = sine(0.013, 4000).iter().map(|v| v * 0.4).collect();
            let mut right: Vec<f32> = sine(0.21, 4000).iter().map(|v| v * 0.02).collect();
            right[100] = 1.0;
            let wanted = [model(&left, live, a), model(&right, live, a)];
            for (l, r) in left.chunks_mut(333).zip(right.chunks_mut(333)) {
                plugin.process(l, r);
            }
            for (side, out) in [left, right].iter().enumerate() {
                let apart = out.iter().zip(&wanted[side]);
                let worst = apart.map(|(got, want)| (f64::from(*got) - want).abs());
                let worst = worst.fold(0.0, f64::max);
                assert!(worst < 2e-3, "{params:?} side {side}: {worst}");
                assert!(out.iter().all(|v| v.abs() <= 1.0));
            }
        }
    }

    #[test]
    fn the_two_sides_do_not_meet() {
        let mut plugin = with([4000, 5000, 6000, 0, 2500, 10_000]);
        let (mut l, mut r) = (sine(0.02, 2000), vec![0.0; 2000]);
        plugin.process(&mut l, &mut r);
        assert!(r.iter().all(|v| *v == 0.0));
        assert!(l.iter().any(|v| v.abs() > 0.5));
        assert_eq!(plugin.latency(), 0);
    }

    #[test]
    fn the_post_filter_is_two_one_poles() {
        // Past the colour and the drive filter a level that stays is a
        // step for the post filter. With the wheel up, `a` is 0.9, and the
        // step is answered by 1 - (1 + n (1 - a)) a^n.
        let mut plugin = with([0, 5000, 0, 0, 10_000, 10_000]);
        let mut open = with([0, 5000, 0, 0, 0, 10_000]);
        let (mut l, mut r) = (vec![0.25; 6000], vec![0.25; 6000]);
        plugin.process(&mut l, &mut r);
        let (mut ol, mut or) = (vec![0.25; 6000], vec![0.25; 6000]);
        open.process(&mut ol, &mut or);
        // Both end on the same level, and the closed one gets there later.
        assert!((l[5999] - ol[5999]).abs() < 1e-5);
        assert!(l[20] < ol[20] - 0.05);
        // The filter's states are one-poles of what the shaper gives.
        let side = plugin.sides[0];
        assert!((side.lp1 - side.lp2).abs() < 1e-5);
        assert_eq!((plugin.targets.keep, open.targets.keep), (0.9, 0.2));
    }

    #[test]
    fn a_moved_wheel_is_followed_through_the_smoother() {
        let mut plugin = BloodOverdrive::default();
        plugin.set_param(2, 10_000.0);
        assert_eq!(plugin.targets.gain, 26.0);
        let before = plugin.live.gain;
        plugin.process(&mut [0.0; 1000], &mut [0.0; 1000]);
        // 0.99975^1000 of the way is left.
        let left = f64::from(KEEP).powi(1000);
        let wanted = 26.0 + (f64::from(before) - 26.0) * left;
        assert!(
            (f64::from(plugin.live.gain) - wanted).abs() < 0.01,
            "{wanted}"
        );
        // The post filter does not wait.
        plugin.set_param(4, 5000.0);
        assert!((plugin.targets.keep - 0.55).abs() < 1e-6);

        let moves = [10_000.4, -5.0, 2578.0, 7.0, f32::NAN, 99_999.0];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
        }
        plugin.set_param(6, 1.0);
        assert_eq!((plugin.param(6), plugin.param_count()), (0.0, 6));
        assert_eq!(
            [0, 1, 2, 3, 4, 5].map(|i| plugin.param(i)),
            [10_000.0, 0.0, 2578.0, 1.0, 0.0, 10_000.0]
        );
        plugin.reset();
        let same = with([10_000, 0, 2578, 1, 0, 10_000]);
        assert_eq!((plugin.params, plugin.sides), (same.params, same.sides));
        assert!((plugin.live.gain - same.live.gain).abs() < 5e-3);
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([4000, 5000, 2000, 0, 2500, 10_000]);
        let before = plugin.clone();
        for len in [3, 32, 40] {
            let mut short = bytes(DEFAULTS);
            short.resize(len, 0);
            assert_eq!(
                plugin.set_state(&short),
                Err(StateError::Length {
                    expected: 36,
                    found: len
                })
            );
        }
        for (param, value) in [(0, -1), (1, 10_001), (3, 2), (5, 10_001)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(params)),
                Err(StateError::Range { param, value })
            );
        }
        let mut newer = bytes(DEFAULTS);
        newer[0] = 4;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 4 })
        );
        assert_eq!(plugin, before);

        // A version under 3 has no colour scheme: 32 bytes.
        let mut older = state(&[2]);
        older.extend(state(&[0, 5000, 10_000, 1, 0, 5000, 0]));
        assert_eq!(plugin.set_state(&older), Ok(()));
        assert_eq!(plugin.params, [0, 5000, 10_000, 1, 0, 5000]);
        older.extend(state(&[0]));
        assert_eq!(
            plugin.set_state(&older),
            Err(StateError::Length {
                expected: 32,
                found: 36
            })
        );
        // The colour scheme is kept and changes nothing that is heard.
        let mut schemed = bytes(DEFAULTS);
        schemed[28] = 3;
        plugin.set_state(&schemed).unwrap();
        assert_eq!(plugin.scheme, 3);
        assert_eq!(plugin.targets, BloodOverdrive::default().targets);
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(own) = presets(NAME) else { return };
        // Three of the plugin's own and the three of the plugin database.
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert_eq!((own.len(), all.len()), (3, 6));
        let named = |file: &str| {
            let preset = own.iter().find(|p| p.file == file);
            let preset = preset.unwrap_or_else(|| panic!("no preset {file}"));
            let mut plugin = BloodOverdrive::default();
            plugin.set_state(&preset.state).unwrap();
            plugin
        };
        for preset in &all {
            assert_eq!(preset.state.len(), 36, "{}", preset.file);
            let mut plugin = BloodOverdrive::default();
            plugin.set_state(&preset.state).unwrap();
        }
        assert_eq!(named("Reset"), BloodOverdrive::default());
        assert_eq!(named("Boost").params, [4000, 5000, 2000, 0, 2500, 10_000]);
        assert_eq!(named("Drive").params, [8500, 5000, 10_000, 0, 5000, 5000]);
    }
}
