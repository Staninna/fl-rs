//! Transient Processor: a gain that follows how fast the level rises and
//! how slowly it falls, on the part of the sound above a split frequency,
//! as `docs/plugins/transient-processor.md` has it.
//!
//! The state is 49 bytes: a version of 1, nine little-endian i32, a byte
//! and two more i32. The nine are the plugin's parameters:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Attack | 0 to 160 | 80 |
//! | 1 Drive | 0 to 160 | 0 |
//! | 2 Release | 0 to 160 | 80 |
//! | 3 Gain | 0 to 160 | 80 |
//! | 4 Attack shape | 0 to 2 | 1 |
//! | 5 Release shape | 0 to 2 | 1 |
//! | 6 Split frequency | 0 to 160 | 80 |
//! | 7 Split balance | 0 to 160 | 80 |
//! | 8 Effect | 0 or 1 | 1 |
//!
//! The byte and the two i32 after it are what the plugin's window shows:
//! which of its three graphs are on, whether the graph is frozen, and how
//! fast it scrolls. They change no sample.
//!
//! FL's plugin is a shell around `engine_x64.dll`, which takes the first
//! eight as numbers from 0 to 1. A one pole low pass and its high pass
//! split each side at `5 + 995 * p^2` Hz. The high part gets one gain for
//! both sides, from five followers of the level, and then a soft clip
//! whose ceiling the drive brings down. The low part passes as it is.

use crate::{Effect, StateError, knob_value, read_state, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Transient Processor";

/// The range of each parameter's control.
const RANGES: [(i32, i32); 9] = [
    (0, 160),
    (0, 160),
    (0, 160),
    (0, 160),
    (0, 2),
    (0, 2),
    (0, 160),
    (0, 160),
    (0, 1),
];
const DEFAULTS: [i32; 9] = [80, 0, 80, 80, 1, 1, 80, 80, 1];

/// How long the state is, and where its parameters end.
const LEN: usize = 49;
const PARAMS_END: usize = 40;

/// How many of the parameters the engine takes. The ninth is the shell's.
const ENGINE: usize = 8;
/// The parameter that switches the effect on.
const EFFECT: usize = 8;

/// The engine moves its smoothed parameters once in this many frames.
const SMOOTH_EVERY: u32 = 8;
/// How near a smoothed parameter gets before it is put on its target.
const SMOOTH_NEAR: f32 = 1e-4;

/// What the shell counts as silence, and what its count of silent frames
/// is put at when the plugin sleeps.
const SILENT: f32 = 1e-6;
const ASLEEP: i64 = -0x3fff_ffff;

/// What a number too small for a float to hold in full becomes in the
/// engine, which runs with those switched off.
fn whole(value: f32) -> f32 {
    if value.is_subnormal() { 0.0 } else { value }
}

/// The engine's soft clip, `v * (v^2 + 27) / (9 * v^2 + 27)`, which is at
/// 1 from 3 on.
fn soft(v: f32) -> f32 {
    if v < -3.0 {
        -1.0
    } else if v > 3.0 {
        1.0
    } else {
        (v * v + 27.0) * v / (v * 9.0 * v + 27.0)
    }
}

/// What a follower keeps of itself a frame, so that it falls to a tenth in
/// `seconds`.
fn keep(seconds: f64, rate: f64) -> f32 {
    0.1f64.powf(1.0 / (seconds * rate)) as f32
}

/// The attack or the release wheel as an amount: the square of where the
/// wheel is from its middle, with the wheel's sign.
fn amount(p: f32) -> f32 {
    let t = ((p - 0.5) + p) - 0.5;
    let square = t * t;
    if t < 0.0 { -square } else { square }
}

/// A shape wheel as the factor on its times: Sharp, Mid and Slow.
fn shape(p: f32) -> f32 {
    let p = f64::from(p);
    if p < 0.333 {
        0.5
    } else if p < 0.666 {
        1.0
    } else {
        2.0
    }
}

/// One pole filters at the split: the low pass and the high pass that
/// add up to their input.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Split {
    /// The low pass on its input, on the input before and on its own
    /// output before.
    low: [f32; 3],
    /// The same three of the high pass.
    high: [f32; 3],
}

impl Split {
    /// For the split wheel at `p` and one over the rate.
    fn new(p: f32, per_rate: f32) -> Self {
        let square = p * p;
        // Pi times the frequency over the rate, the frequency being
        // `5 + 995 * p^2` Hz.
        let turn = square * per_rate * 3_125.884_8 + per_rate * 15.707_964;
        let k = turn.tan();
        let scale = 1.0 / (k + 1.0);
        let back = -((k - 1.0) * scale);
        Split {
            low: [scale * k, scale * k, back],
            high: [scale, -scale, back],
        }
    }
}

/// What one side of the split keeps: the sample before, and the two
/// outputs before.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Side {
    before: f32,
    low: f32,
    high: f32,
}

impl Side {
    /// One sample in, the low part and the high part out.
    fn split(&mut self, x: f32, split: &Split) -> (f32, f32) {
        let [a, b, c] = split.low;
        let low = whole(a * x + b * self.before + c * self.low);
        let [a, b, c] = split.high;
        let high = whole(a * x + b * self.before + c * self.high);
        self.before = x;
        self.low = low;
        self.high = high;
        (low, high)
    }
}

/// The engine's shaper: the followers and the gain they make.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shaper {
    /// The drive, the attack amount and the release amount in use, and
    /// what the wheels ask for. The ones in use get there a frame late.
    drive: f32,
    drive_to: f32,
    attack: f32,
    attack_to: f32,
    release: f32,
    release_to: f32,
    /// The factor of each shape wheel on its times.
    attack_shape: f32,
    release_shape: f32,
    /// The attack's two followers: one that jumps to a new peak and one
    /// that takes the attack time to get there.
    peak: f32,
    slow: f32,
    /// The release's three: a peak that falls over 4 s, one that falls
    /// over the release time, and a follower of that one.
    held: f32,
    tail: f32,
    follower: f32,
    /// What the followers keep or take a frame.
    fall: f32,
    rise: f32,
    follow: f32,
    follow_more: f32,
    tail_fall: f32,
    held_fall: f32,
}

impl Shaper {
    /// As the engine makes it, before any parameter is set.
    fn new() -> Self {
        Shaper {
            drive: 0.01,
            drive_to: 0.1,
            attack: 0.0,
            attack_to: 0.0,
            release: 0.0,
            release_to: 0.0,
            attack_shape: 1.0,
            release_shape: 1.0,
            peak: 0.0,
            slow: 0.0,
            held: 0.0,
            tail: 0.0,
            follower: 0.0,
            fall: 0.0,
            rise: 0.0,
            follow: 0.0,
            follow_more: 0.0,
            tail_fall: 0.0,
            held_fall: 0.0,
        }
    }

    /// Work the followers' paces out for the rate and the two shapes.
    fn tune(&mut self, rate: f32) {
        let hz = f64::from(rate);
        self.fall = keep(0.2, hz);
        self.rise = 1.0 - keep(f64::from(self.attack_shape) * 0.01, hz);
        self.follow_more = keep(f64::from(self.release_shape) * 0.02, hz);
        self.follow = 1.0 - self.follow_more;
        self.tail_fall = keep(f64::from(self.release_shape) * 0.1, hz);
        self.held_fall = 0.1f64.powf(1.0 / f64::from(rate * 4.0)) as f32;
    }

    /// The gain for one frame of the high part.
    fn gain(&mut self, left: f32, right: f32) -> f32 {
        let level = (right.abs() + left.abs()) * 0.5;

        // The attack: how far the peak is above the slow follower.
        self.peak = if level > self.peak {
            level
        } else {
            whole(self.peak * self.fall)
        };
        self.slow = whole(if level > self.slow {
            (level - self.slow) * self.rise + self.slow
        } else {
            self.slow * self.fall
        });
        let mut rising = 1.0;
        if self.slow > 0.0 {
            let ratio = self.peak / self.slow;
            if ratio >= 1.0 {
                rising = if self.attack < 0.0 {
                    ratio
                } else if ratio > 10.0 {
                    6.6
                } else {
                    ratio * (0.96 - ratio * 0.003 * ratio)
                };
            }
        }

        // The release: how far the held peak is above the follower of the
        // tail, which goes faster the further the tail is from it.
        self.held = if level >= self.held {
            level
        } else {
            whole(self.held * self.held_fall)
        };
        self.tail = if level < self.tail {
            whole(self.tail * self.tail_fall)
        } else {
            level
        };
        let apart = self.tail - self.follower;
        let hurry = if self.release >= 0.0 && self.tail > 0.0 {
            let tail = self.tail + 1e-4;
            (tail - self.follower) / tail
        } else {
            1.0
        };
        let hurry = (hurry * 3.0).clamp(0.0, 1.0);
        self.follower = whole((hurry * self.follow_more + self.follow) * apart + self.follower);
        let mut falling = 1.0;
        if self.follower > 1e-6 {
            let ratio = self.held / self.follower;
            if ratio >= 1.0 {
                falling = if self.release < 0.0 {
                    ratio
                } else if ratio > 20.0 {
                    12.0
                } else {
                    ratio * 0.84 - ratio * ratio * (ratio * 6e-4)
                };
            }
        }

        let gain = (falling - 1.0) * self.release + (rising - 1.0) * self.attack;
        if gain >= 0.0 {
            gain + 1.0
        } else {
            1.0 / (1.0 - gain)
        }
    }

    /// One frame of the high part through the gain and the soft clip.
    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        // What the wheels have moved by is added after the frame.
        let drive_step = self.drive_to - self.drive;
        let attack_step = self.attack_to - self.attack;
        let release_step = self.release_to - self.release;

        let gain = self.gain(left, right);
        let out_left = soft(self.drive * (left * gain)) / self.drive;
        let out_right = soft(self.drive * (right * gain)) / self.drive;

        self.attack += attack_step;
        self.release += release_step;
        self.drive += drive_step;
        (out_left, out_right)
    }
}

/// Transient Processor. A new one has FL's defaults: no gain on the
/// attack or the release, the drive at its least, the split at 254 Hz.
#[derive(Debug, Clone, PartialEq)]
pub struct TransientProcessor {
    params: [i32; 9],
    rate: f32,
    /// The engine's parameters, from 0 to 1: what the wheels ask for, and
    /// what is in use while a wheel that moved is smoothed.
    target: [f32; ENGINE],
    now: [f32; ENGINE],
    /// The parameters that are on their way, and the frames since they
    /// last moved.
    moving: [bool; ENGINE],
    since: u32,
    /// What the smoothing covers of the way in one step.
    smooth: f32,
    shaper: Shaper,
    split: Split,
    sides: [Side; 2],
    /// The gains of the low part and of the high part, and the output's.
    low: f32,
    high: f32,
    gain: f32,
    /// How many frames of silence the shell still runs the engine for,
    /// and how many it does after sound.
    awake: i64,
    tail: i64,
}

impl Default for TransientProcessor {
    fn default() -> Self {
        let mut plugin = TransientProcessor {
            params: DEFAULTS,
            rate: 44_100.0,
            target: [0.0; ENGINE],
            now: [0.0; ENGINE],
            moving: [false; ENGINE],
            since: 0,
            smooth: 0.0,
            shaper: Shaper::new(),
            split: Split::new(0.5, 1.0 / 44_100.0),
            sides: [Side::default(); 2],
            low: 1.0,
            high: 1.0,
            gain: 1.0,
            awake: 0,
            tail: 0,
        };
        plugin.set_rate(44_100.0);
        plugin.load(DEFAULTS);
        plugin
    }
}

impl TransientProcessor {
    /// A wheel's whole number as the engine's 0 to 1.
    fn normal(index: usize, value: i32) -> f32 {
        let (min, max) = RANGES[index];
        let p = f64::from(value - min) * (1.0 / f64::from(max - min));
        p.clamp(0.0, 1.0) as f32
    }

    /// Put one of the engine's parameters in use at `p`.
    fn apply(&mut self, index: usize, p: f32) {
        self.now[index] = p;
        match index {
            0 => {
                let amount = amount(p);
                let reach = if amount >= 0.0 { 0.4 } else { 0.6 };
                self.shaper.attack_to = reach * amount;
            }
            1 => {
                self.shaper.drive_to = if p < 0.1 {
                    p * 9.0 + 0.1
                } else {
                    15f32.powf((p - 0.1) * 1.111_111)
                };
            }
            2 => {
                let amount = amount(p);
                self.shaper.release_to = if amount >= 0.0 { amount * 0.6 } else { amount };
            }
            3 => self.gain = 10f32.powf((p - 0.5) * 0.9),
            4 => {
                self.shaper.attack_shape = shape(p);
                self.shaper.tune(self.rate);
            }
            5 => {
                self.shaper.release_shape = shape(p);
                self.shaper.tune(self.rate);
            }
            6 => self.split = Split::new(p, 1.0 / self.rate),
            7 => {
                self.low = ((1.0 - p) + (1.0 - p)).min(1.0);
                self.high = (p + p).min(1.0);
            }
            _ => {}
        }
    }

    /// Take a whole state's parameters, which are inside their ranges.
    /// After a state FL has the engine put every one in use at once.
    fn load(&mut self, params: [i32; 9]) {
        self.params = params;
        for (index, value) in params.into_iter().enumerate().take(ENGINE) {
            self.target[index] = Self::normal(index, value);
            self.apply(index, self.target[index]);
        }
        self.moving = [false; ENGINE];
    }

    /// The engine's part of a new rate.
    fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
        self.smooth = 1.0 - 0.1f64.powf(200.0 / f64::from(rate)) as f32;
        self.shaper.tune(rate);
        // The split is worked out anew from what its wheel asks for.
        self.apply(6, self.target[6]);
        self.tail = (f64::from(rate) * 1.5).round_ties_even() as i64 + 88;
    }

    /// Move the parameters that are on their way one step.
    fn smooth(&mut self) {
        for index in 0..ENGINE {
            if !self.moving[index] {
                continue;
            }
            let target = self.target[index];
            let next = self.now[index] + (target - self.now[index]) * self.smooth;
            if (next - target).abs() < SMOOTH_NEAR {
                self.apply(index, target);
                self.moving[index] = false;
            } else {
                self.apply(index, next);
            }
        }
    }

    /// One frame through the engine.
    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.since += 1;
        if self.since == SMOOTH_EVERY {
            self.since = 0;
            self.smooth();
        }
        let split = self.split;
        let (low_right, high_right) = self.sides[1].split(right, &split);
        let (low_left, high_left) = self.sides[0].split(left, &split);
        let (shaped_left, shaped_right) = self.shaper.frame(high_left, high_right);
        (
            (self.low * low_left + self.high * shaped_left) * self.gain,
            (self.low * low_right + self.high * shaped_right) * self.gain,
        )
    }
}

impl Effect for TransientProcessor {
    /// FL reads a version under 2, and leaves the plugin alone for
    /// another. Its window's part of the state changes no sample and is
    /// not kept.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > 1 {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        self.load(read_state(&bytes[4..PARAMS_END], &RANGES)?);
        Ok(())
    }

    /// The followers, the split and the smoothing are worked out for the
    /// rate, and what the followers hold stays. A rate that is not a
    /// number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.set_rate(hz);
        }
    }

    /// FL's shell runs the engine while the effect is on and there is
    /// sound, or was in the last second and a half. Otherwise the block
    /// passes as it is and the engine stands still.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len()) as i64;
        let quiet = |side: &[f32]| side.iter().all(|sample| sample.abs() <= SILENT);
        if quiet(left) && quiet(right) {
            self.awake = self.awake.saturating_sub(len);
        } else {
            self.awake = self.tail;
        }
        if self.awake <= -len {
            self.awake = ASLEEP;
            return;
        }
        if self.params[EFFECT] == 0 {
            return;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            (*l, *r) = self.frame(*l, *r);
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// The engine smooths the attack, the drive, the release, the gain and
    /// the two split wheels: a step every 8 frames, each a part of what is
    /// left. The shapes and the switch move at once.
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(range) = RANGES.get(index) else {
            return;
        };
        let value = knob_value(value, *range);
        self.params[index] = value;
        if index >= ENGINE {
            return;
        }
        self.target[index] = Self::normal(index, value);
        if index == 4 || index == 5 {
            self.apply(index, self.target[index]);
        } else {
            self.moving[index] = true;
        }
    }

    /// The engine has nothing FL's flush clears, so its followers live on
    /// through a stop. This starts them over, as a plugin FL has just
    /// loaded has them, with every parameter where its wheel is.
    fn reset(&mut self) {
        let mut shaper = Shaper::new();
        shaper.attack_shape = self.shaper.attack_shape;
        shaper.release_shape = self.shaper.release_shape;
        self.shaper = shaper;
        self.shaper.tune(self.rate);
        self.sides = [Side::default(); 2];
        self.since = 0;
        self.awake = 0;
        self.load(self.params);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, presets, sine, state};

    /// A state as FL writes it: the version, the nine, the three graphs
    /// on, not frozen, scrolling at 50.
    fn saved(params: [i32; 9]) -> Vec<u8> {
        let mut bytes = state(&[1]);
        bytes.extend(state(&params));
        bytes.push(7);
        bytes.extend(state(&[0, 50]));
        bytes
    }

    fn with(moves: &[(usize, i32)]) -> TransientProcessor {
        let mut params = DEFAULTS;
        for (index, value) in moves {
            params[*index] = *value;
        }
        let mut plugin = TransientProcessor::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_state(&saved(params)).unwrap();
        plugin
    }

    /// Both sides through the plugin, in blocks of 512.
    fn run(plugin: &mut TransientProcessor, input: &[f32]) -> Vec<f32> {
        let mut left = input.to_vec();
        let mut right = input.to_vec();
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        assert_eq!(bits(&left), bits(&right));
        left
    }

    /// A tone at 1 kHz that starts after half a second of nothing and
    /// stops half a second before the end.
    fn burst(size: f32, len: usize) -> Vec<f32> {
        let tone = sine(1000.0 / 48_000.0, len);
        let sound = 24_000..len - 24_000;
        let shaped = tone.iter().enumerate();
        shaped
            .map(|(n, v)| if sound.contains(&n) { v * size } else { 0.0 })
            .collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |peak, v| peak.max(v.abs()))
    }

    #[test]
    fn the_defaults_are_the_wheels_of_fl_s_window() {
        let plugin = TransientProcessor::default();
        let params: Vec<f32> = (0..9).map(|index| plugin.param(index)).collect();
        assert_eq!(params, [80.0, 0.0, 80.0, 80.0, 1.0, 1.0, 80.0, 80.0, 1.0]);
        assert_eq!(plugin.param(9), 0.0);
        assert_eq!(plugin.param_count(), 9);
        assert_eq!(plugin.latency(), 0);
        // No gain on the attack or the release, the least drive, 0 dB, and
        // both parts of the split in full.
        assert_eq!(plugin.shaper.attack_to, 0.0);
        assert_eq!(plugin.shaper.release_to, 0.0);
        assert_eq!(plugin.shaper.drive_to, 0.1);
        assert_eq!((plugin.low, plugin.high, plugin.gain), (1.0, 1.0, 1.0));
    }

    #[test]
    fn the_wheels_are_the_doc_s_curves() {
        // The attack reaches 0.4 up and 0.6 down, the release 0.6 and 1.
        let plugin = with(&[(0, 160), (2, 160)]);
        assert!((plugin.shaper.attack_to - 0.4).abs() < 1e-6);
        assert!((plugin.shaper.release_to - 0.6).abs() < 1e-6);
        let plugin = with(&[(0, 0), (2, 0)]);
        assert!((plugin.shaper.attack_to + 0.6).abs() < 1e-6);
        assert_eq!(plugin.shaper.release_to, -1.0);
        // A quarter of the way from the middle is a sixteenth of the way.
        let plugin = with(&[(0, 100)]);
        assert!((plugin.shaper.attack_to - 0.4 / 16.0).abs() < 1e-6);

        // The drive is a line up to 1 at a tenth of the wheel and a power
        // of 15 from there.
        let root = 15f32.sqrt();
        for (wheel, drive) in [(0, 0.1), (8, 0.55), (16, 1.0), (88, root), (160, 15.0)] {
            let plugin = with(&[(1, wheel)]);
            assert!((plugin.shaper.drive_to - drive).abs() < 1e-4, "{wheel}");
        }
        // The gain is 9 dB either way.
        for (wheel, db) in [(0, -9.0), (80, 0.0), (120, 4.5), (160, 9.0)] {
            let plugin = with(&[(3, wheel)]);
            assert!((20.0 * plugin.gain.log10() - db).abs() < 1e-4, "{wheel}");
        }
        // The balance takes the low part away above the middle and the
        // high part under it.
        let plugin = with(&[(7, 120)]);
        assert_eq!((plugin.low, plugin.high), (0.5, 1.0));
        let plugin = with(&[(7, 0)]);
        assert_eq!((plugin.low, plugin.high), (1.0, 0.0));
        // The shapes halve and double the times.
        let plugin = with(&[(4, 0), (5, 2)]);
        assert_eq!(plugin.shaper.attack_shape, 0.5);
        assert_eq!(plugin.shaper.release_shape, 2.0);
    }

    #[test]
    fn the_followers_fall_to_a_tenth_in_their_times() {
        let plugin = with(&[]);
        let frames = |keep: f32| (0.1f64.ln() / f64::from(keep).ln()) / 48_000.0;
        let shaper = plugin.shaper;
        assert!((frames(shaper.fall) - 0.2).abs() < 1e-3);
        assert!((frames(1.0 - shaper.rise) - 0.01).abs() < 1e-4);
        assert!((frames(shaper.follow_more) - 0.02).abs() < 1e-4);
        assert!((frames(shaper.tail_fall) - 0.1).abs() < 1e-3);
        assert!((frames(shaper.held_fall) - 4.0).abs() < 0.05);
        // Slow doubles the attack's time and Sharp halves the release's.
        let shaper = with(&[(4, 2), (5, 0)]).shaper;
        assert!((frames(1.0 - shaper.rise) - 0.02).abs() < 1e-4);
        assert!((frames(shaper.tail_fall) - 0.05).abs() < 1e-3);
    }

    #[test]
    fn the_two_parts_of_the_split_add_up_to_the_input() {
        // 5 Hz at the bottom of the wheel, 254 in the middle, 1 kHz at
        // the top.
        for (wheel, hz) in [(0, 5.0), (80, 253.75), (160, 1000.0)] {
            let plugin = with(&[(6, wheel)]);
            let k = (std::f64::consts::PI * hz / 48_000.0).tan();
            assert!((f64::from(plugin.split.high[0]) - 1.0 / (k + 1.0)).abs() < 1e-6);
            assert!((f64::from(plugin.split.low[0]) - k / (k + 1.0)).abs() < 1e-6);
            // What the rounding leaves over stays in the two filters for
            // as long as they ring, which is longest at 5 Hz.
            let mut side = Side::default();
            for x in sine(0.013, 2000) {
                let (low, high) = side.split(x, &plugin.split);
                assert!((low + high - x).abs() < 2e-5, "{wheel}: {}", low + high - x);
            }
        }
    }

    #[test]
    fn no_gain_on_the_attack_or_the_release_passes_the_input() {
        // The defaults: both amounts at 0, so the gain is 1, and the soft
        // clip at its least drive is a line this far under full scale.
        let mut plugin = with(&[]);
        let input: Vec<f32> = burst(0.1, 96_000);
        let output = run(&mut plugin, &input);
        for (n, (x, y)) in input.iter().zip(&output).enumerate() {
            assert!((x - y).abs() < 2e-5, "{n}: {x} {y}");
        }
        // Louder, the clip bends a little: 0.3 % at full scale.
        let mut plugin = with(&[]);
        let input = burst(1.0, 96_000);
        let output = run(&mut plugin, &input);
        let bent = peak(&output[40_000..60_000]);
        assert!(bent < 1.0 && bent > 0.996, "{bent}");
    }

    #[test]
    fn the_attack_lifts_or_softens_the_start_of_a_sound() {
        let input = burst(0.25, 96_000);
        // How loud the first 5 ms of the sound are, and a stretch a
        // quarter second in, over what went in.
        let gains = |output: &[f32]| {
            let start = peak(&output[24_000..24_240]) / peak(&input[24_000..24_240]);
            let later = peak(&output[36_000..38_000]) / peak(&input[36_000..38_000]);
            (start, later)
        };
        let (start, later) = gains(&run(&mut with(&[(0, 160)]), &input));
        assert!(start > 1.5 && start < 3.5, "{start}");
        // Once the slow follower has caught up, the peak of a sine stays a
        // little above it, and so does the gain.
        assert!(later > 0.95 && later < 1.1, "{later}");
        let (start, later) = gains(&run(&mut with(&[(0, 0)]), &input));
        assert!(start < 0.6, "{start}");
        assert!(later > 0.85 && later < 1.01, "{later}");
    }

    #[test]
    fn the_release_lifts_or_cuts_what_follows_a_peak() {
        // A tone that falls 60 dB in half a second, after a loud start.
        let decay: Vec<f32> = sine(1000.0 / 48_000.0, 48_000)
            .iter()
            .enumerate()
            .map(|(n, v)| v * 0.5 * 0.001f32.powf(n as f32 / 24_000.0))
            .collect();
        let tail = |output: &[f32]| peak(&output[12_000..14_000]);
        let plain = tail(&run(&mut with(&[]), &decay));
        let more = tail(&run(&mut with(&[(2, 160)]), &decay));
        let less = tail(&run(&mut with(&[(2, 0)]), &decay));
        assert!(more > plain * 1.5, "{more} {plain}");
        assert!(less < plain * 0.5, "{less} {plain}");
    }

    #[test]
    fn the_drive_brings_the_ceiling_of_the_soft_clip_down() {
        // The clip is at 1 from 3 on, and what leaves it is over the
        // drive: a ceiling of 1 at a tenth of the wheel and 1/15 at its
        // top. The balance leaves the high part alone, and the split is at
        // its lowest, so that is all of the tone.
        for (wheel, ceiling) in [(16, 1.0), (160, 1.0 / 15.0)] {
            let mut plugin = with(&[(1, wheel), (6, 0), (7, 160)]);
            let output = run(&mut plugin, &burst(4.0, 96_000));
            let top = peak(&output[30_000..70_000]);
            assert!((top - ceiling).abs() < 0.01 * ceiling, "{wheel}: {top}");
        }
        assert_eq!(soft(3.5), 1.0);
        assert_eq!(soft(-3.5), -1.0);
        assert_eq!(soft(3.0), 1.0);
        assert!((soft(0.5) - 0.5 * 27.25 / 29.25).abs() < 1e-6);
    }

    #[test]
    fn the_effect_switched_off_and_silence_leave_the_block_alone() {
        let input = burst(0.5, 96_000);
        let mut plugin = with(&[(0, 160), (8, 0)]);
        assert_eq!(bits(&run(&mut plugin, &input)), bits(&input));

        // A block of nothing after nothing does not run the engine.
        let mut plugin = with(&[(0, 160)]);
        let quiet = vec![5e-7; 512];
        assert_eq!(bits(&run(&mut plugin, &quiet)), bits(&quiet));
        assert_eq!(plugin.since, 0);
        // After sound it runs on for a second and a half and 88 frames.
        run(&mut plugin, &[0.5; 512]);
        assert_eq!(plugin.awake, 72_088);
        run(&mut plugin, &vec![0.0; 72_088]);
        assert_eq!(plugin.awake, 0);
        // The block that brings the count under 0 by its own length is
        // the first the engine does not see.
        plugin.since = 3;
        run(&mut plugin, &[0.0; 511]);
        assert_eq!((plugin.awake, plugin.since), (ASLEEP, 3));
    }

    #[test]
    fn a_moved_wheel_gets_there_in_steps_of_8_frames() {
        let mut plugin = with(&[]);
        plugin.set_param(3, 160.0);
        assert_eq!(plugin.param(3), 160.0);
        // Nothing moves before the 8th frame.
        run(&mut plugin, &[0.5; 7]);
        assert_eq!(plugin.gain, 1.0);
        run(&mut plugin, &[0.5; 1]);
        let step = plugin.smooth;
        assert_eq!(plugin.now[3], 0.5 + 0.5 * step);
        // A tenth of the way is left after a 200th of the rate in steps.
        assert!((f64::from(1.0 - step).powf(48_000.0 / 200.0) - 0.1).abs() < 1e-3);
        run(&mut plugin, &vec![0.5; 48_000]);
        assert_eq!(plugin.now[3], 1.0);
        assert!(!plugin.moving[3]);
        assert_eq!(plugin.gain, with(&[(3, 160)]).gain);
        // A shape moves at once, and the switch is the shell's.
        plugin.set_param(4, 2.0);
        assert_eq!(plugin.shaper.attack_shape, 2.0);
        plugin.set_param(8, 0.0);
        plugin.set_param(9, 1.0);
        assert_eq!(plugin.param(8), 0.0);
    }

    #[test]
    fn a_reset_starts_the_followers_over() {
        let input = burst(0.5, 60_000);
        let mut plugin = with(&[(0, 160), (2, 30), (1, 40)]);
        let first = run(&mut plugin, &input);
        plugin.reset();
        let again = run(&mut plugin, &input);
        // The first frame of all has the engine's first drive, and the
        // tone starts later than that.
        assert_eq!(bits(&first), bits(&again));
    }

    #[test]
    fn a_state_of_another_version_length_or_range_is_turned_down() {
        let mut plugin = TransientProcessor::default();
        let good = saved([160, 38, 80, 80, 2, 1, 0, 80, 1]);
        assert_eq!(plugin.set_state(&good), Ok(()));
        assert_eq!(plugin.param(0), 160.0);
        // A version of 0 loads as 1 does.
        let mut old = good.clone();
        old[0] = 0;
        assert_eq!(plugin.set_state(&old), Ok(()));
        let mut newer = good.clone();
        newer[0] = 2;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
        assert_eq!(
            plugin.set_state(&good[..48]),
            Err(StateError::Length {
                expected: 49,
                found: 48
            })
        );
        assert!(matches!(
            plugin.set_state(&[1, 0]),
            Err(StateError::Length { .. })
        ));
        let wrong = saved([161, 0, 80, 80, 1, 1, 80, 80, 1]);
        assert_eq!(
            plugin.set_state(&wrong),
            Err(StateError::Range {
                param: 0,
                value: 161
            })
        );
        assert_eq!(plugin.param(0), 160.0);
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(presets) = presets(NAME) else {
            return;
        };
        assert_eq!(presets.len(), 10);
        let wanted: [(&str, [i32; 9]); 4] = [
            ("Default", DEFAULTS),
            ("Expander", [80, 0, 36, 80, 1, 1, 80, 80, 1]),
            ("Max attack", [160, 38, 80, 80, 2, 1, 0, 80, 1]),
            ("Overdiven", [120, 110, 160, 53, 2, 1, 50, 80, 1]),
        ];
        for (name, params) in wanted {
            let preset = presets.iter().find(|preset| preset.file == name).unwrap();
            assert_eq!(preset.plugin, NAME);
            let mut plugin = TransientProcessor::default();
            plugin.set_state(&preset.state).unwrap();
            let read: Vec<f32> = (0..9).map(|index| plugin.param(index)).collect();
            assert_eq!(read, params.map(|v| v as f32), "{name}");
        }
        for preset in &presets {
            let mut plugin = TransientProcessor::default();
            plugin.set_state(&preset.state).unwrap();
            let output = run(&mut plugin, &burst(0.5, 60_000));
            assert!(output.iter().all(|v| v.is_finite() && v.abs() < 4.0));
        }
    }
}
