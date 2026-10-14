//! Fruity Delay 3: a delay with a filter, a saturator, a sample rate and
//! bit reducer and a diffuser in its feedback, as
//! `docs/plugins/fruity-delay-3.md` has it.
//!
//! The state is 108 bytes: a little-endian i32 version, which is 1, and the
//! plugin's 26 parameters as i32:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Input wet | 0 to 6000 | 6000 |
//! | 1 Delay type | 0 Mono, 1 Stereo, 2 Ping pong | 1 |
//! | 2 Tempo sync | 0, 1 | 1 |
//! | 3 Keep pitch | 0, 1 | 0 |
//! | 4 Time | 1 to 768 | 192 |
//! | 5 Time R, or the pan of Ping pong | 0 to 6000 | 3000 |
//! | 6 Time smooth, ms | 0 to 1000 | 500 |
//! | 7 Stereo spread | 0 to 6000 | 6000 |
//! | 8 Modulation rate | 0 to 6000 | 3000 |
//! | 9 Modulation time | 0 to 6000 | 0 |
//! | 10 Modulation cutoff | 0 to 6000 | 0 |
//! | 11 Diffusion level | 0 to 6000 | 0 |
//! | 12 Diffusion spread | 0 to 6000 | 0 |
//! | 13 Distortion type | 0 Limit, 1 Saturate | 1 |
//! | 14 Feedback level | 0 to 6000, which is 125 % | 3000 |
//! | 15 Filter mode | 0 low, 1 high, 2 band pass | 0 |
//! | 16 Feedback cutoff | 0 to 6000 | 6000 |
//! | 17 Feedback resonance | 0 to 6000 | 0 |
//! | 18 Distortion amount | 0 to 6000 | 0 |
//! | 19 Distortion hardness | 0 to 6000 | 0 |
//! | 20 Distortion symmetry | 0 to 6000 | 0 |
//! | 21 Rate reduction | 0 to 60000, off at the top | 60000 |
//! | 22 Bits reduction | 0 to 6000, off at the top | 6000 |
//! | 23 Output wet | 0 to 6000 | 6000 |
//! | 24 Output tone | -3000 to 3000 | 0 |
//! | 25 Output dry | 0 to 6000 | 6000 |
//!
//! With the sync on, Time is in 192ths of a beat. With it off, Time is a
//! cubic from 1 ms to 1 s. FL's Chorus preset saves a Time of 0, so a state
//! may have one, and it is the shortest time.
//!
//! An echo goes around this loop, where the two orders are the doc's:
//!
//! ```text
//! Mono              line -> 10 Hz high pass -> + input -> saturator -> filter -> diffuser -> line
//! Stereo, Ping pong line -> 10 Hz high pass -> + input -> filter -> saturator -> diffuser -> line
//! ```
//!
//! The saturator starts with the rate and bit reducer. Stereo is two such
//! loops, and Time R holds one side's input back. Ping pong writes each
//! side's loop into the other side's line. Mono is one loop that is read
//! twice, the second time further back by Time R. What is read from the
//! lines goes through the tone filter and the spread and is added to the
//! dry signal.
//!
//! The diffuser's own length comes off the line's, so the echo of a click
//! is the time after the click whatever the diffuser does to it.
//!
//! Some of this is not in the doc and is this file's choice:
//!
//! - The tempo is the host's to give, through [`Effect::set_tempo`], and is
//!   140 until it does.
//! - FL moves a wheel's value to where it was put over some milliseconds,
//!   and lets a new time glide or fade in. A state, a new rate and a reset
//!   skip all of that and so does anything set before the first block.
//! - FL's lines are 2^20 samples at 44.1 kHz. The ones here are as long as
//!   the wheels can ask for at the rate and the tempo, and no longer than
//!   FL's, which reads the same.
//! - What goes into a line under 1e-30 goes in as 0, and a filter state too
//!   small to hold in full is 0 after the block, to keep the sums fast.
//! - The doc gives the two selectors to the engine as `v / Max`, which
//!   does not land on its own three models. The saved number is taken as
//!   the model and the filter mode.

use std::f64::consts::PI;

use crate::{Effect, StateError, knob_value, read_state, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Delay 3";

const PARAMS: usize = 26;
const STATE_LEN: usize = 4 + 4 * PARAMS;

const INPUT_WET: usize = 0;
const MODEL: usize = 1;
const SYNC: usize = 2;
const KEEP_PITCH: usize = 3;
const TIME: usize = 4;
const TIME_R: usize = 5;
const TIME_SMOOTH: usize = 6;
const SPREAD: usize = 7;
const MOD_RATE: usize = 8;
const MOD_TIME: usize = 9;
const MOD_CUTOFF: usize = 10;
const DIFFUSION: usize = 11;
const DIFFUSION_SPREAD: usize = 12;
const DIST_TYPE: usize = 13;
const FEEDBACK: usize = 14;
const FILTER_MODE: usize = 15;
const CUTOFF: usize = 16;
const RESONANCE: usize = 17;
const DIST_AMOUNT: usize = 18;
const HARDNESS: usize = 19;
const SYMMETRY: usize = 20;
const RATE: usize = 21;
const BITS: usize = 22;
const OUTPUT_WET: usize = 23;
const TONE: usize = 24;
const OUTPUT_DRY: usize = 25;

/// The range of each parameter's control. Time starts at 0 for the state
/// FL's Chorus preset saves.
const RANGES: [(i32, i32); PARAMS] = [
    (0, 6000),
    (0, 2),
    (0, 1),
    (0, 1),
    (0, 768),
    (0, 6000),
    (0, 1000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 1),
    (0, 6000),
    (0, 2),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 60000),
    (0, 6000),
    (0, 6000),
    (-3000, 3000),
    (0, 6000),
];

/// What `Default.fst` has.
const DEFAULTS: [i32; PARAMS] = [
    6000, 1, 1, 0, 192, 3000, 500, 6000, 3000, 0, 0, 0, 0, 1, 3000, 0, 6000, 0, 0, 0, 0, 60000,
    6000, 6000, 0, 6000,
];

/// The parameters FL moves to where they were put over some milliseconds:
/// 0, 7, 10, 11, 14 and 16 to 25.
const GLIDES: u32 =
    0x03ff_0000 | 1 << INPUT_WET | 1 << SPREAD | 1 << MOD_CUTOFF | 1 << DIFFUSION | 1 << FEEDBACK;

const MONO: u8 = 0;
const PING_PONG: u8 = 2;

/// The tempo before the host has given one.
const TEMPO: f64 = 140.0;

/// The slowest and the fastest tempo FL has. A tempo outside them is held
/// to the nearer one.
const TEMPO_RANGE: (f64, f64) = (10.0, 522.0);

/// The LFO's period in seconds along the rate wheel.
const PERIODS: [f32; 8] = [8.872, 3.808, 1.833, 0.878, 0.398, 0.150, 0.050, 0.0];

/// The cutoffs in Hz along a low pass's wheel and a high pass's.
const LOW_PASS: [f32; 5] = [270.0, 480.0, 1300.0, 7000.0, 22000.0];
const HIGH_PASS: [f32; 5] = [20.0, 90.0, 400.0, 1300.0, 4200.0];

/// The engine tunes its feedback filter with a pi of four decimals.
#[allow(clippy::approx_constant)]
const FILTER_PI: f64 = 3.1415;

/// A sample under this is 0 to the engine.
const SILENT: f32 = 1e-13;

/// What goes into a line under this goes in as 0.
const QUIET: f32 = 1e-30;

/// The lengths a head reads at: the diffuser's, the two taps' and the
/// line's, which the doc calls A, B, C and D.
const A: usize = 0;
const B: usize = 1;
const C: usize = 2;
const D: usize = 3;

/// What the engine is given for a saved value: 0 to 1 along the wheel, and
/// the number itself for a switch or a selector.
fn unit(index: usize, value: i32) -> f32 {
    let (min, max) = RANGES[index];
    match index {
        MODEL | SYNC | KEEP_PITCH | DIST_TYPE | FILTER_MODE => value as f32,
        // The wheel runs from 1.
        TIME => ((value - 1) as f32 / 767.0).clamp(0.0, 1.0),
        _ => (value - min) as f32 / (max - min) as f32,
    }
}

/// A table read at a place between two of its numbers.
fn table(points: &[f32], at: f32) -> f32 {
    let whole = (at as usize).min(points.len() - 2);
    let part = at - whole as f32;
    points[whole] + (points[whole + 1] - points[whole]) * part
}

/// The four gains of Ping pong's input for the Time R wheel at `v`, as
/// `[LL, RR, RL, LR]`: left to left, right to right, right into left, left
/// into right. The side the wheel leans from gives half of what it loses to
/// the other.
pub fn pan_law(v: f32) -> [f32; 4] {
    let x = 2.0 * v - 1.0;
    if x >= 0.0 {
        let crossed = x / 2.0;
        [1.0 - x, 1.0 - crossed, 0.0, crossed]
    } else {
        let crossed = -x / 2.0;
        [1.0 - crossed, 1.0 + x, crossed, 0.0]
    }
}

/// How long FL's own lines are at a rate.
fn fl_ring(hz: f64) -> usize {
    if hz <= 61_440.0 {
        1 << 20
    } else if hz <= 126_976.0 {
        1 << 21
    } else {
        1 << 22
    }
}

/// A ring of samples as long as a power of two. Every ring is written at
/// the plugin's one position.
#[derive(Debug, Clone)]
struct Ring {
    samples: Vec<f32>,
    mask: usize,
}

impl Ring {
    fn new() -> Self {
        Ring {
            samples: vec![0.0],
            mask: 0,
        }
    }

    /// Be at least `len` samples long, a power of two, and keep what the
    /// ring holds behind `pos`.
    fn grow(&mut self, len: usize, pos: usize) {
        if len <= self.samples.len() {
            return;
        }
        let mut samples = vec![0.0; len];
        for back in 1..=self.samples.len() {
            let at = pos.wrapping_sub(back);
            samples[at & (len - 1)] = self.samples[at & self.mask];
        }
        self.samples = samples;
        self.mask = len - 1;
    }

    /// What was written `len` samples before `pos`, between two samples
    /// for a length that is not whole. Nothing holds the length inside the
    /// ring, in FL either.
    fn read(&self, pos: usize, len: f64) -> f32 {
        let whole = len as usize;
        let part = (len - whole as f64) as f32;
        let at = pos.wrapping_sub(whole);
        let near = self.samples[at & self.mask];
        let far = self.samples[at.wrapping_sub(1) & self.mask];
        near + (far - near) * part
    }

    fn write(&mut self, pos: usize, sample: f32) {
        self.samples[pos & self.mask] = sample;
    }
}

/// A filter of one pole and one zero: `y = b0 x + b1 x1 + a1 y1`. The DC
/// blockers and the tone are this.
#[derive(Debug, Clone, Copy, PartialEq)]
struct First {
    b0: f32,
    b1: f32,
    a1: f32,
}

impl First {
    /// The filter that leaves its input alone.
    const THROUGH: First = First {
        b0: 1.0,
        b1: 0.0,
        a1: 0.0,
    };

    /// A cutoff is held under 0.49 of the rate.
    fn k(fc: f64, hz: f64) -> f64 {
        (fc.min(0.49 * hz) * PI / hz).tan()
    }

    fn low(fc: f64, hz: f64) -> Self {
        let k = First::k(fc, hz);
        let b = (k / (1.0 + k)) as f32;
        First {
            b0: b,
            b1: b,
            a1: ((1.0 - k) / (1.0 + k)) as f32,
        }
    }

    fn high(fc: f64, hz: f64) -> Self {
        let k = First::k(fc, hz);
        let b = (1.0 / (1.0 + k)) as f32;
        First {
            b0: b,
            b1: -b,
            a1: ((1.0 - k) / (1.0 + k)) as f32,
        }
    }

    /// The tone wheel at `v`: flat at the centre, a low pass that closes
    /// to 270 Hz under it and a high pass that climbs to 4.2 kHz above it.
    fn tone(v: f32, hz: f64) -> Self {
        if v == 0.5 {
            First::THROUGH
        } else if v < 0.5 {
            First::low(f64::from(table(&LOW_PASS, 2.0 * v * 3.9999)), hz)
        } else {
            First::high(f64::from(table(&HIGH_PASS, 2.0 * (v - 0.5) * 3.9999)), hz)
        }
    }

    fn run(&self, state: &mut [f32; 2], x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * state[0] + self.a1 * state[1];
        *state = [x, y];
        y
    }
}

/// The feedback filter, Simper's state variable filter: `a1`, `a2`, `a3`
/// and how much of the input, the band and the low output to mix.
#[derive(Debug, Clone, Copy)]
struct Svf {
    a: [f32; 3],
    mix: [f32; 3],
}

impl Svf {
    fn run(&self, ic: &mut [f32; 2], x: f32) -> f32 {
        let [a1, a2, a3] = self.a;
        let v3 = x - ic[1];
        let v1 = a1 * ic[0] + a2 * v3;
        let v2 = ic[1] + a2 * ic[0] + a3 * v3;
        ic[0] = 2.0 * v1 - ic[0];
        ic[1] = 2.0 * v2 - ic[1];
        self.mix[0] * x + self.mix[1] * v1 + self.mix[2] * v2
    }
}

/// Follow a level: up at the first speed, down at the second.
fn follow(envelope: &mut f32, level: f32, speeds: [f32; 2]) {
    let speed = if level >= *envelope {
        speeds[0]
    } else {
        speeds[1]
    };
    *envelope += (level - *envelope) * speed;
}

/// The saturator with the reducer it starts with.
#[derive(Debug, Clone, Copy)]
struct Shaper {
    /// Where Limit holds a peak: the drive's gain times the make-up.
    ceiling: f32,
    makeup: f32,
    /// Where Saturate starts to bend, how wide the bend is and a quarter
    /// over that width, above 0 and under it.
    bend: [[f32; 3]; 2],
    /// How many samples the rate reducer holds one for. 0 is off.
    hold: f32,
    /// How many steps the bit reducer has in 1. 0 is off.
    steps: f32,
    /// How fast Limit's envelope rises and falls: 99 % of the way in 441
    /// and 4410 samples, whatever the rate.
    speeds: [f32; 2],
}

impl Shaper {
    /// The reducer: hold a sample for `hold` samples, then round it
    /// towards 0 to a step. The counter's reset is as the doc found it.
    fn reduce(&self, side: &mut Side, mut x: f32) -> f32 {
        if self.hold > 0.0 {
            side.count += 1.0;
            if side.count < self.hold {
                x = side.held;
            } else {
                side.held = x;
                side.count = (side.count / self.hold).fract() * side.count;
            }
        }
        if self.steps > 0.0 {
            x = (self.steps * x).trunc() / self.steps;
        }
        x
    }

    /// Limit turns down what its envelope has over the ceiling. Saturate
    /// bends a sample over the threshold along a parabola that is flat at
    /// the ceiling.
    fn run(&self, limit: bool, side: &mut Side, x: f32) -> f32 {
        let x = self.reduce(side, x);
        if limit {
            let mut x = x / self.ceiling;
            follow(&mut side.envelope, x.abs(), self.speeds);
            if side.envelope > 1.0 {
                x /= side.envelope;
            }
            x * self.ceiling
        } else {
            let [threshold, knee, quarter] = self.bend[usize::from(x < 0.0)];
            let mut size = x.abs();
            if size >= threshold {
                let over = (size - threshold).min(2.0 * knee);
                size = threshold + over - over * over * quarter;
            }
            size.copysign(x) * self.makeup
        }
    }
}

/// What one side keeps from sample to sample.
#[derive(Debug, Clone, Copy, Default)]
struct Side {
    dc: [f32; 2],
    filter: [f32; 2],
    tone: [f32; 2],
    /// The reducer's sample and its counter.
    held: f32,
    count: f32,
    /// Limit's envelope, which Ping pong has on its output with Saturate.
    envelope: f32,
}

impl Side {
    /// A state too small to hold in full becomes 0.
    fn flush(&mut self) {
        let pairs = [&mut self.dc, &mut self.filter, &mut self.tone];
        let states = pairs.into_iter().flatten();
        for state in states.chain([&mut self.envelope]) {
            if state.is_subnormal() {
                *state = 0.0;
            }
        }
    }
}

/// One of the two read heads: the lengths it is on its way to and the ones
/// it reads at now.
#[derive(Debug, Clone, Copy, Default)]
struct Head {
    target: [f64; 4],
    live: [f64; 4],
}

/// Fruity Delay 3. A new one has FL's defaults at 44.1 kHz and 140 BPM: a
/// stereo echo a beat long at 62 % feedback, under the dry signal.
#[derive(Debug, Clone)]
pub struct Delay3 {
    params: [i32; PARAMS],
    hz: f64,
    tempo: f64,
    /// What the engine has for each parameter now, and where it was put.
    now: [f32; PARAMS],
    aim: [f32; PARAMS],
    /// Whether a parameter is on its way, and the count to every eighth
    /// sample, where it takes a step of `glide` of what is left.
    gliding: bool,
    glide_at: u8,
    glide: f32,
    /// Nothing has gone through since the plugin was made or reset.
    fresh: bool,

    /// The model, kept to see it change.
    model: u8,
    /// Ping pong's input gains.
    pan: [f32; 4],
    /// How much of the way a length goes in one sample.
    time_smooth: f64,
    /// The LFO: where it is in its turn, its step a sample, and how far it
    /// moves the read heads at this rate, which is less for a fast one.
    phase: f32,
    phase_step: f32,
    reach: f32,
    /// How far the LFO moves the heads now, in samples, and the speed that
    /// goes after the wheel with.
    depth: f32,
    depth_speed: f32,
    filter: Svf,
    /// The count to every fourth sample, where the filter is tuned.
    filter_at: u8,
    dc: First,
    tone: First,
    shaper: Shaper,

    heads: [Head; 2],
    weights: [f32; 2],
    /// The head that is heard in full when no fade is under way.
    active: usize,
    fading: bool,
    fade_at: usize,
    fade_len: usize,
    /// The lengths are to be worked out again.
    retune: bool,
    /// Mono's Time R is under the centre, so the first tap is the long one.
    swapped: bool,

    /// Where every ring is written next.
    pos: usize,
    /// The rings Stereo holds its inputs back in.
    early: [Ring; 2],
    line: [Ring; 2],
    /// The diffuser's allpass rings, one a head for each side.
    diffuser: [[Ring; 2]; 2],
    sides: [Side; 2],
}

impl Default for Delay3 {
    fn default() -> Self {
        let pair = || [Ring::new(), Ring::new()];
        let mut plugin = Delay3 {
            params: DEFAULTS,
            hz: 44_100.0,
            tempo: TEMPO,
            now: [0.0; PARAMS],
            aim: std::array::from_fn(|index| unit(index, DEFAULTS[index])),
            gliding: false,
            glide_at: 0,
            glide: 1.0,
            fresh: true,
            model: DEFAULTS[MODEL] as u8,
            pan: [1.0, 1.0, 0.0, 0.0],
            time_smooth: 1.0,
            phase: 0.0,
            phase_step: 0.0,
            reach: 0.0,
            depth: 0.0,
            depth_speed: 1.0,
            filter: Svf {
                a: [0.0; 3],
                mix: [0.0; 3],
            },
            filter_at: 0,
            dc: First::THROUGH,
            tone: First::THROUGH,
            shaper: Shaper {
                ceiling: 1.0,
                makeup: 1.0,
                bend: [[0.0; 3]; 2],
                hold: 0.0,
                steps: 0.0,
                speeds: [
                    1.0 - 0.01f32.powf(1.0 / 441.0),
                    1.0 - 0.01f32.powf(1.0 / 4410.0),
                ],
            },
            heads: [Head::default(); 2],
            weights: [1.0, 0.0],
            active: 0,
            fading: false,
            fade_at: 0,
            fade_len: 1,
            retune: false,
            swapped: false,
            pos: 0,
            early: pair(),
            line: pair(),
            diffuser: [pair(), pair()],
            sides: [Side::default(); 2],
        };
        plugin.make_room();
        plugin.settle();
        plugin
    }
}

impl Delay3 {
    fn rings(&mut self) -> impl Iterator<Item = &mut Ring> {
        let diffuser = self.diffuser.iter_mut().flatten();
        self.early.iter_mut().chain(&mut self.line).chain(diffuser)
    }

    /// Have the memory for the longest lengths the wheels can ask for at
    /// this rate and tempo, so that no wheel has to ask for memory. FL's
    /// own rings are the most a ring gets, and a length over that wraps as
    /// it does in FL.
    fn make_room(&mut self) {
        let (hz, pos) = (self.hz, self.pos);
        let most = fl_ring(hz);
        let len = |samples: f64| (samples as usize).saturating_add(4).min(most);
        let len = |samples: f64| len(samples).next_power_of_two();
        // Four beats with the sync on and a second with it off.
        let longest = (240.0 / self.tempo).max(1.0) * hz;
        // Mono's second tap is half as long again, and the LFO moves a
        // read by up to 0.075 s.
        let line = len(1.5 * longest + 0.075 * hz);
        // Stereo holds an input back by up to a quarter of the time.
        let early = len(0.25 * longest);
        let diffuser = len(655.0 * hz / 44_100.0);
        self.line.iter_mut().for_each(|ring| ring.grow(line, pos));
        self.early.iter_mut().for_each(|ring| ring.grow(early, pos));
        let rings = self.diffuser.iter_mut().flatten();
        rings.for_each(|ring| ring.grow(diffuser, pos));
    }

    /// Empty the rings and the filters, as FL does for another model.
    fn wipe(&mut self) {
        self.rings().for_each(|ring| ring.samples.fill(0.0));
        self.sides = [Side::default(); 2];
    }

    /// Have everything where it was put, with no glide and no fade.
    fn settle(&mut self) {
        let hz = self.hz;
        // 99 % of the way in 5 ms of steps and in 100 ms.
        self.glide = (1.0 - 0.01f64.powf(1.0 / (0.005 * hz))) as f32;
        self.depth_speed = (1.0 - 0.01f64.powf(1.0 / (0.1 * hz))) as f32;
        self.fade_len = ((0.1 * hz) as usize).max(1);
        self.dc = First::high(10.0, hz);
        self.now = self.aim;
        for index in 0..PARAMS {
            self.apply(index);
        }
        self.gliding = false;
        self.depth = self.depth_aim();
        (self.active, self.weights, self.fading) = (0, [1.0, 0.0], false);
        let lengths = self.lengths();
        self.heads[0] = Head {
            target: lengths,
            live: lengths,
        };
        self.retune = false;
        self.filter_at = 0;
    }

    /// Do what the engine's setter does when parameter `index` has the
    /// value in `now`. The parameters that are not here are read where
    /// they are used.
    fn apply(&mut self, index: usize) {
        let v = self.now[index];
        match index {
            MODEL => {
                if v as u8 != self.model {
                    self.model = v as u8;
                    self.wipe();
                }
                self.retune = true;
            }
            // The doc's setter has no retune for the sync switch, so the
            // time follows it at the next one.
            KEEP_PITCH => self.retune |= v < 0.5,
            TIME | DIFFUSION_SPREAD => self.retune = true,
            TIME_R => {
                self.pan = pan_law(v);
                self.retune = true;
            }
            TIME_SMOOTH => {
                // 99 % of the way in `v` seconds.
                self.time_smooth = if v == 0.0 {
                    1.0
                } else {
                    1.0 - 0.01f64.powf(1.0 / (f64::from(v) * self.hz))
                };
            }
            MOD_RATE => {
                let period = table(&PERIODS, 0.99 * v * 5.9994);
                self.phase_step = (1.0 / (self.hz * f64::from(period))) as f32;
                self.reach = (period / PERIODS[0]).sqrt();
            }
            DIST_AMOUNT | HARDNESS | SYMMETRY => self.shape(),
            RATE => {
                let f = 127.0 * (1.0 - v).powi(3);
                let samples = 0.0002 * f * f * f - 0.0171 * f * f + 0.7435 * f + 1.0;
                self.shaper.hold = if f > 0.0 {
                    (samples * (self.hz / 44_100.0) as f32).max(1.0)
                } else {
                    0.0
                };
            }
            BITS => {
                self.shaper.steps = if v < 1.0 {
                    2f32.powf(2.0 + 14.0 * (1.0 - (1.0 - v).sqrt()))
                } else {
                    0.0
                };
            }
            TONE => self.tone = First::tone(v, self.hz),
            _ => {}
        }
    }

    /// The saturator's ceiling, make-up and bends for the amount, the
    /// hardness and the symmetry.
    fn shape(&mut self) {
        let amount = 0.95 * f64::from(self.now[DIST_AMOUNT]);
        let (hardness, symmetry) = (self.now[HARDNESS], self.now[SYMMETRY]);
        // The drive is 0 to -28.5 dB.
        let gain = 10f64.powf(-30.0 * amount / 20.0) as f32;
        let loud =
            1.0 + 3.7032 * amount.powi(2) - 7.5177 * amount.powi(3) + 2.9843 * amount.powi(4);
        self.shaper.makeup = (1.0 / loud).powf(0.707) as f32;
        self.shaper.ceiling = gain * self.shaper.makeup;
        for (bend, sign) in self.shaper.bend.iter_mut().zip([1.0, -1.0]) {
            let ceiling = (1.0 + sign * symmetry) * gain;
            let threshold = ceiling * hardness;
            let knee = (ceiling - threshold).max(1e-11);
            *bend = [threshold, knee, 0.25 / knee];
        }
    }

    /// How far the mod wheel asks the LFO to move the heads, in samples.
    fn depth_aim(&self) -> f32 {
        let amount = 0.1 * self.now[MOD_TIME] * self.now[MOD_TIME];
        amount * (0.5 * self.hz) as f32 * self.reach
    }

    /// The lengths A, B, C and D in samples for the time, the tempo, the
    /// diffusion spread and Time R.
    fn lengths(&mut self) -> [f64; 4] {
        let hz = self.hz;
        let seconds = if self.now[SYNC] >= 0.5 {
            // 192 steps are a beat.
            let steps = (767.0 * self.now[TIME] + 0.5) as i32 + 1;
            f64::from(steps) / 192.0 * 60.0 / self.tempo
        } else {
            let t = 0.94 * f64::from(self.now[TIME]) + 0.06;
            (t * t * t - 0.000216) * 1.000216 * 0.999 + 0.001
        };
        let total = (seconds * hz).trunc();
        // The diffuser's allpass, which is shorter under 15 ms.
        let spread = 590.0 * f64::from(self.now[DIFFUSION_SPREAD]) + 65.0;
        let short = (total / (0.015 * hz)).min(1.0);
        let diffuser = (short * (hz / 44_100.0) * spread).trunc().max(1.0);
        let rest = (total - diffuser).max(1.0);
        let time_r = f64::from(self.now[TIME_R]);
        let x = (time_r - 0.5).abs();
        if self.model == MONO {
            // The second tap is up to half the time further back.
            let far = (x * total + rest).trunc();
            self.swapped = time_r < 0.5;
            if self.swapped {
                [diffuser, far, rest, rest]
            } else {
                [diffuser, rest, far, rest]
            }
        } else {
            // One input waits up to a quarter of the time.
            let late = (x * x * total).trunc().max(1.0);
            if time_r >= 0.5 {
                [diffuser, late, 1.0, rest]
            } else {
                [diffuser, 1.0, late, rest]
            }
        }
    }

    /// Step every parameter that is on its way.
    fn step_params(&mut self) {
        self.gliding = false;
        for index in 0..PARAMS {
            let (now, aim) = (self.now[index], self.aim[index]);
            if now == aim {
                continue;
            }
            let next = now + (aim - now) * self.glide;
            if (aim - next).abs() < 1e-4 {
                self.now[index] = aim;
            } else {
                self.now[index] = next;
                self.gliding = true;
            }
            self.apply(index);
        }
    }

    /// Before a sample: without keep pitch the first head's lengths glide
    /// to where they were put. With it the idle head takes the new lengths
    /// at once and a fade to it starts.
    fn move_heads(&mut self) {
        if self.now[KEEP_PITCH] < 0.5 {
            (self.active, self.weights) = (0, [1.0, 0.0]);
            if self.retune {
                self.heads[0].target = self.lengths();
                self.retune = false;
            }
            let Head { target, live } = &mut self.heads[0];
            for (live, target) in live.iter_mut().zip(target) {
                *live += (*target - *live) * self.time_smooth;
                if (*target - *live).abs() < 0.001 {
                    *live = *target;
                }
            }
        } else if !self.fading && self.retune {
            (self.fade_at, self.fading, self.retune) = (0, true, false);
            let lengths = self.lengths();
            self.heads[1 - self.active] = Head {
                target: lengths,
                live: lengths,
            };
        }
    }

    /// After a sample with keep pitch: the fade goes one step on, over a
    /// tenth of a second, and then the heads change places.
    fn fade(&mut self) {
        if self.now[KEEP_PITCH] >= 0.5 && self.fading {
            let idle = 1 - self.active;
            let w = self.fade_at as f32 * (1.0 / self.fade_len as f32);
            self.weights[self.active] = 1.0 - w;
            self.weights[idle] = w;
            self.fade_at += 1;
            if self.fade_at == self.fade_len {
                (self.active, self.fading) = (idle, false);
            }
        }
    }

    /// Tune the feedback filter for the cutoff, which the LFO moves, the
    /// resonance and the mode.
    fn tune_filter(&mut self, lfo: f32) {
        let c = (self.now[CUTOFF] + 0.5 * self.now[MOD_CUTOFF] * lfo).clamp(0.0, 1.0);
        let res = 0.5 * self.now[RESONANCE].clamp(0.0, 1.0);
        let r2 = res * res;
        let (fc, k, mix) = match self.now[FILTER_MODE] as u8 {
            0 => {
                let k = 1.0 / (4.0 * r2 + 0.707);
                (table(&LOW_PASS, 3.999 * c), k, [0.0, 0.0, 1.0])
            }
            1 => {
                let k = 1.0 / (4.0 * r2 + 0.707);
                (table(&HIGH_PASS, 3.999 * c), k, [1.0, -k, -1.0])
            }
            _ => {
                let k = 1.0 / (4.6 * r2 + 0.107);
                (table(&LOW_PASS, 3.6 * c), k, [0.0, k, 0.0])
            }
        };
        let turn = (FILTER_PI / self.hz) as f32;
        let g = (f64::from(fc).min(0.49 * self.hz) * f64::from(turn)).tan() as f32;
        let a1 = 1.0 / (1.0 + g * (g + k));
        self.filter = Svf {
            a: [a1, a1 * g, a1 * g * g],
            mix,
        };
    }

    /// What both heads read `which` length back in a ring, each at its
    /// weight. `wobble` is what the LFO adds to the length.
    fn tap(&self, ring: &Ring, pos: usize, which: usize, wobble: f64) -> f32 {
        let [first, second] = &self.heads;
        ring.read(pos, first.live[which] + wobble) * self.weights[0]
            + ring.read(pos, second.live[which] + wobble) * self.weights[1]
    }

    /// A side's diffuser: one Schroeder allpass a head, each at its head's
    /// weight. At a level of 0 it is a delay of A samples.
    fn diffuse(&mut self, side: usize, x: f32) -> f32 {
        let g = 0.499 * self.now[DIFFUSION].clamp(0.0, 1.0);
        let mut out = 0.0;
        for (head, ring) in self.diffuser[side].iter_mut().enumerate() {
            let delayed = ring.read(self.pos, self.heads[head].live[A]);
            ring.write(self.pos, x - g * delayed);
            out += ((1.0 - g * g) * delayed + g * x) * self.weights[head];
        }
        if out.abs() < QUIET { 0.0 } else { out }
    }

    /// The spread: each side keeps half of itself and up to all of it, and
    /// has the rest from the other.
    fn spread(&self, wet: [f32; 2]) -> [f32; 2] {
        let direct = 0.5 * self.now[SPREAD] + 0.5;
        let cross = 1.0 - direct;
        [
            direct * wet[0] + cross * wet[1],
            direct * wet[1] + cross * wet[0],
        ]
    }

    /// The wet level and the dry signal.
    fn mix(&self, dry: [f32; 2], wet: [f32; 2]) -> (f32, f32) {
        let (level, dry_level) = (self.now[OUTPUT_WET], self.now[OUTPUT_DRY]);
        (
            wet[0] * level + dry[0] * dry_level,
            wet[1] * level + dry[1] * dry_level,
        )
    }

    /// Mono: one line that both sides' input goes into and that is read
    /// at two taps, the left side's and the right's.
    fn mono(&mut self, l: f32, r: f32, wobble: f64) -> (f32, f32) {
        let pos = self.pos;
        let input = (l + r) / 2.0;
        let taps = [B, C].map(|which| self.tap(&self.line[0], pos, which, wobble));
        // The feedback is the tap that is the time long.
        let base = taps[usize::from(self.swapped)];
        let back = self
            .dc
            .run(&mut self.sides[0].dc, 1.25 * self.now[FEEDBACK] * base);
        let limit = self.now[DIST_TYPE] < 0.5;
        let fed = back + input * self.now[INPUT_WET];
        let shaped = self.shaper.run(limit, &mut self.sides[0], fed);
        let filtered = self.filter.run(&mut self.sides[0].filter, shaped);
        let diffused = self.diffuse(0, filtered);
        self.line[0].write(pos, diffused);
        let wet = [
            self.tone.run(&mut self.sides[0].tone, taps[0]),
            self.tone.run(&mut self.sides[1].tone, taps[1]),
        ];
        self.mix([l, r], self.spread(wet))
    }

    /// Stereo and Ping pong: a line a side. Stereo writes a side's loop
    /// back into its own line and Ping pong into the other side's.
    fn stereo(&mut self, l: f32, r: f32, wobble: f64) -> (f32, f32) {
        let pos = self.pos;
        let ping_pong = self.model == PING_PONG;
        let limit = self.now[DIST_TYPE] < 0.5;
        let wet_in = self.now[INPUT_WET];
        let feedback = 1.25 * self.now[FEEDBACK];
        let input = if ping_pong {
            let [ll, rr, rl, lr] = self.pan;
            [(ll * l + rl * r) * wet_in, (lr * l + rr * r) * wet_in]
        } else {
            // One side's input is held back by its tap, and a tap of 1 is
            // no time at all.
            let mut early = [l, r];
            for (side, which) in [B, C].into_iter().enumerate() {
                self.early[side].write(pos, early[side]);
                let held = self.tap(&self.early[side], pos.wrapping_add(1), which, 0.0);
                early[side] = held * wet_in;
            }
            early
        };
        let outs = [0, 1].map(|side| self.tap(&self.line[side], pos, D, wobble));
        let mut wet = [0.0; 2];
        for (side, out) in outs.into_iter().enumerate() {
            let back = self.dc.run(&mut self.sides[side].dc, feedback * out);
            let fed = input[side] + back;
            let filtered = self.filter.run(&mut self.sides[side].filter, fed);
            let shaped = self.shaper.run(limit, &mut self.sides[side], filtered);
            let diffused = self.diffuse(side, shaped);
            let into = if ping_pong { 1 - side } else { side };
            self.line[into].write(pos, diffused);
            wet[side] = self.tone.run(&mut self.sides[side].tone, out);
        }
        let mut wet = self.spread(wet);
        if ping_pong && !limit {
            // With Saturate, Ping pong has Limit's envelope on its output.
            for (wet, side) in wet.iter_mut().zip(&mut self.sides) {
                follow(&mut side.envelope, wet.abs(), self.shaper.speeds);
                if side.envelope > 1.0 {
                    *wet /= side.envelope;
                }
            }
        }
        self.mix([l, r], wet)
    }

    /// One frame through the plugin.
    fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.move_heads();
        let lfo = ((std::f32::consts::TAU * self.phase).sin() + 1.0) / 2.0;
        self.phase += self.phase_step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        self.depth += (self.depth_aim() - self.depth) * self.depth_speed;
        let wobble = f64::from((lfo + 0.5) * self.depth);
        if self.filter_at == 0 {
            self.tune_filter(lfo);
        }
        self.filter_at = (self.filter_at + 1) & 3;
        let out = if self.model == MONO {
            self.mono(l, r, wobble)
        } else {
            self.stereo(l, r, wobble)
        };
        self.pos = self.pos.wrapping_add(1);
        self.fade();
        out
    }
}

impl Effect for Delay3 {
    /// FL reads a version under 2 and leaves a newer state alone. The
    /// state is in force from the first sample on, with no glide. Another
    /// model empties the lines, as in FL.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, STATE_LEN)?;
        if version > 1 {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != STATE_LEN {
            return Err(StateError::Length {
                expected: STATE_LEN,
                found: bytes.len(),
            });
        }
        // A parameter's number in an error is its place in the state.
        let params = read_state(&bytes[4..], &RANGES)?;
        self.params = params;
        self.aim = std::array::from_fn(|index| unit(index, params[index]));
        self.settle();
        Ok(())
    }

    /// As in FL, a new rate starts the plugin from nothing, with new
    /// lines. A rate that is not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
            self.rings().for_each(|ring| *ring = Ring::new());
            self.make_room();
            self.reset();
        }
    }

    /// Asks for memory when a line may have to be longer than before. A
    /// tempo that is not a number is not taken. Once sound has gone
    /// through, the time moves to the new tempo as it does for the Time
    /// wheel.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_nan() {
            return;
        }
        let tempo = bpm.clamp(TEMPO_RANGE.0, TEMPO_RANGE.1);
        if tempo != self.tempo {
            self.tempo = tempo;
            self.make_room();
            self.retune = true;
            if self.fresh {
                self.settle();
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let silence = |x: f32| if x.abs() < SILENT { 0.0 } else { x };
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if self.gliding && self.glide_at == 0 {
                self.step_params();
            }
            self.glide_at = (self.glide_at + 1) & 7;
            (*l, *r) = self.tick(silence(*l), silence(*r));
            self.fresh = false;
        }
        self.sides.iter_mut().for_each(Side::flush);
        if self.depth.is_subnormal() {
            self.depth = 0.0;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// A level, the filter, the saturator, the reducer or the tone goes to
    /// where it was put over some milliseconds, as in FL. A new time
    /// glides there at the pace of Time smooth, or fades in over a tenth
    /// of a second with keep pitch. Another model empties the lines.
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(range) = RANGES.get(index) else {
            return;
        };
        self.params[index] = knob_value(value, *range);
        self.aim[index] = unit(index, self.params[index]);
        if self.fresh {
            self.settle();
        } else if GLIDES & 1 << index != 0 {
            self.gliding = true;
        } else {
            self.now[index] = self.aim[index];
            self.apply(index);
        }
    }

    /// Empties the lines and the filters, starts the LFO over and ends
    /// every glide and fade.
    fn reset(&mut self) {
        self.wipe();
        (self.pos, self.phase, self.glide_at) = (0, 0.0, 0);
        self.heads = [Head::default(); 2];
        self.settle();
        self.fresh = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, partial, presets, projects, sine, state};

    const HZ: f64 = 44_100.0;

    /// The defaults with some parameters moved.
    fn moved(changes: &[(usize, i32)]) -> [i32; PARAMS] {
        let mut params = DEFAULTS;
        for (index, value) in changes {
            params[*index] = *value;
        }
        params
    }

    fn bytes(params: &[i32; PARAMS]) -> Vec<u8> {
        state(&[&[1], &params[..]].concat())
    }

    /// A plugin at 44.1 kHz and 120 BPM with the defaults and some
    /// parameters moved.
    fn with(changes: &[(usize, i32)]) -> Delay3 {
        let mut plugin = Delay3::default();
        plugin.set_sample_rate(HZ as f32);
        plugin.set_tempo(120.0);
        plugin.set_state(&bytes(&moved(changes))).unwrap();
        plugin
    }

    /// A plugin with nothing in its loop but the open filter: no feedback,
    /// no dry signal, and Limit, which leaves a sample under 1 alone.
    fn clean(changes: &[(usize, i32)]) -> Delay3 {
        let base = [(FEEDBACK, 0), (OUTPUT_DRY, 0), (DIST_TYPE, 0)];
        with(&[&base[..], changes].concat())
    }

    fn run(plugin: &mut Delay3, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (left.to_vec(), right.to_vec());
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        (left, right)
    }

    /// What comes out of `len` frames that start with one frame of
    /// `(left, right)`.
    fn impulse(plugin: &mut Delay3, left: f32, right: f32, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; len], vec![0.0; len]);
        (l[0], r[0]) = (left, right);
        run(plugin, &l, &r)
    }

    /// The first frame of a side that is not 0.
    fn first(side: &[f32]) -> Option<usize> {
        side.iter().position(|value| *value != 0.0)
    }

    fn close(got: f32, want: f64) -> bool {
        (f64::from(got) - want).abs() <= 1e-5 * want.abs().max(1e-3)
    }

    /// The doc's time in samples.
    fn total(time: i32, sync: bool, hz: f64, bpm: f64) -> usize {
        let v = f64::from(time - 1) / 767.0;
        let seconds = if sync {
            ((767.0 * v + 0.5).floor() + 1.0) / 192.0 * 60.0 / bpm
        } else {
            let t = 0.94 * v + 0.06;
            (t.powi(3) - 0.000216) * 1.000216 * 0.999 + 0.001
        };
        (seconds * hz) as usize
    }

    /// The doc's diffuser length for a spread of 0 to 1.
    fn diffuser(total: usize, spread: f64, hz: f64) -> usize {
        let short = (total as f64 / (0.015 * hz)).min(1.0);
        ((short * hz / 44_100.0 * (590.0 * spread + 65.0)) as usize).max(1)
    }

    /// The doc's low pass at a cutoff and a resonance of 0 to 1: `g` and
    /// `k`.
    fn low_pass(c: f64, res: f64, hz: f64) -> (f64, f64) {
        let at = 3.999 * c;
        let whole = at as usize;
        let low = f64::from(LOW_PASS[whole]);
        let fc = low + (f64::from(LOW_PASS[whole + 1]) - low) * (at - whole as f64);
        let g = (fc.min(0.49 * hz) * FILTER_PI / hz).tan();
        (g, 1.0 / (4.0 * (0.5 * res).powi(2) + 0.707))
    }

    /// `a3`, what the low pass makes of the first sample of a click.
    fn a3(c: f64, hz: f64) -> f64 {
        let (g, k) = low_pass(c, 0.0, hz);
        g * g / (1.0 + g * (g + k))
    }

    /// The low pass's gain for a tone that turns `turns` times a sample.
    fn low_gain(c: f64, turns: f64, hz: f64) -> f64 {
        let (g, k) = low_pass(c, 0.0, hz);
        let w = (PI * turns).tan() / g;
        1.0 / ((1.0 - w * w).powi(2) + (k * w).powi(2)).sqrt()
    }

    /// `b0` of the 10 Hz high pass on the feedback.
    fn dc_b0(hz: f64) -> f64 {
        1.0 / (1.0 + (PI * 10.0 / hz).tan())
    }

    /// The doc's ceiling for a distortion amount of 0 to 1.
    fn ceiling(amount: f64) -> f64 {
        let d = 0.95 * amount;
        let loud = 1.0 + 3.7032 * d.powi(2) - 7.5177 * d.powi(3) + 2.9843 * d.powi(4);
        10f64.powf(-30.0 * d / 20.0) * (1.0 / loud).powf(0.707)
    }

    #[test]
    fn synced_at_120_bpm_a_click_comes_back_one_time_on() {
        // 192 is a beat: half a second, 22050 samples. The diffuser takes
        // 65 of them and the line the rest.
        let beat = total(192, true, HZ, 120.0);
        assert_eq!((beat, diffuser(beat, 0.0, HZ)), (22_050, 65));
        let mut plugin = clean(&[]);
        assert_eq!(plugin.heads[0].live, [65.0, 1.0, 1.0, 21_985.0]);
        let (left, right) = impulse(&mut plugin, 0.5, 0.0, beat + 100);
        assert_eq!((first(&left), first(&right)), (Some(beat), None));
        // The open filter is a low pass at 0.49 of the rate, which takes
        // a little off the first sample.
        let open = a3(1.0, HZ);
        assert!((open - 0.9565).abs() < 5e-4, "{open}");
        assert!(close(left[beat], 0.5 * open), "{}", left[beat]);

        // 144 is a dotted eighth and 1 the 192th of a beat, 2.6 ms.
        for (time, hz, bpm) in [
            (1, HZ, 120.0),
            (96, HZ, 120.0),
            (144, HZ, 120.0),
            (144, 48_000.0, 174.0),
            (768, 22_050.0, 140.0),
        ] {
            let samples = total(time, true, hz, bpm);
            let mut plugin = Delay3::default();
            plugin.set_sample_rate(hz as f32);
            plugin.set_tempo(bpm);
            let params = moved(&[(FEEDBACK, 0), (OUTPUT_DRY, 0), (TIME, time)]);
            plugin.set_state(&bytes(&params)).unwrap();
            let a = diffuser(samples, 0.0, hz);
            assert_eq!(plugin.heads[0].live[A], a as f64, "time {time}");
            assert_eq!(plugin.heads[0].live[D], (samples - a) as f64);
            let (left, right) = impulse(&mut plugin, 0.5, 0.5, samples + 10);
            assert_eq!(first(&left), Some(samples), "time {time} at {hz} Hz");
            assert_eq!(left, right);
        }
        assert_eq!(total(144, true, HZ, 120.0), 16_537);
        assert_eq!(total(1, true, HZ, 120.0), 114);
        assert_eq!(diffuser(114, 0.0, HZ), 11);
    }

    #[test]
    fn the_diffuser_s_length_comes_off_the_line() {
        // At full spread the allpass is 655 samples, and with no level it
        // is a delay, so the click still comes back a beat on.
        let beat = total(192, true, HZ, 120.0);
        assert_eq!(diffuser(beat, 1.0, HZ), 655);
        let mut plugin = clean(&[(DIFFUSION_SPREAD, 6000)]);
        let (left, _) = impulse(&mut plugin, 0.5, 0.0, beat + 10);
        assert_eq!(first(&left), Some(beat));

        // With the level up, an allpass of gain g lets g through at once,
        // 1 - g^2 after its length and -g times that every pass after.
        let mut plugin = clean(&[(DIFFUSION_SPREAD, 6000), (DIFFUSION, 6000)]);
        let (left, _) = impulse(&mut plugin, 0.5, 0.0, beat + 700);
        let (g, x) = (0.499, 0.5 * a3(1.0, HZ));
        assert_eq!(first(&left), Some(beat - 655));
        assert!(close(left[beat - 655], g * x));
        assert!(close(left[beat], (1.0 - g * g) * x));
        assert!(close(left[beat + 655], -g * (1.0 - g * g) * x));
    }

    #[test]
    fn unsynced_the_default_time_is_26_ms() {
        // v = 191 / 767 on the cubic from 1 ms to 1 s.
        let samples = total(192, false, HZ, 120.0);
        assert_eq!(samples, 1155);
        let mut plugin = clean(&[(SYNC, 0)]);
        let (left, _) = impulse(&mut plugin, 0.5, 0.0, samples + 10);
        assert_eq!(first(&left), Some(samples));
        // The tempo does not move it.
        plugin.reset();
        plugin.set_tempo(77.0);
        let (left, _) = impulse(&mut plugin, 0.5, 0.0, samples + 10);
        assert_eq!(first(&left), Some(samples));

        // The ends of the wheel, and the doc's reading of Resonator.
        assert_eq!(total(1, false, HZ, 120.0), 44);
        // The cubic ends two thousandths of a sample under a second.
        assert_eq!(total(768, false, HZ, 120.0), 44_099);
        assert_eq!(total(37, false, HZ, 120.0), 84);
        for time in [0, 1, 37, 768] {
            let plugin = clean(&[(SYNC, 0), (TIME, time)]);
            let samples = total(time.max(1), false, HZ, 120.0);
            let a = diffuser(samples, 0.0, HZ) as f64;
            assert_eq!(
                plugin.heads[0].live[A] + plugin.heads[0].live[D],
                samples as f64
            );
            assert_eq!(plugin.heads[0].live[A], a, "time {time}");
        }
    }

    #[test]
    fn at_50_percent_every_repeat_is_0_625_of_the_one_before() {
        // The wheel at half is a gain of 1.25 * 0.5. Around the loop a
        // repeat's first sample also goes through the 10 Hz high pass and
        // the open low pass.
        let time = total(48, true, HZ, 120.0);
        assert_eq!(time, 5512);
        let ratio = 1.25 * 0.5 * dc_b0(HZ) * a3(1.0, HZ);
        assert!((ratio - 0.5974).abs() < 5e-4, "{ratio}");
        let mut plugin = clean(&[(FEEDBACK, 3000), (TIME, 48)]);
        let (left, right) = impulse(&mut plugin, 0.5, 0.0, 6 * time + 10);
        let mut want = 0.5 * a3(1.0, HZ);
        for repeat in 1..=6 {
            let got = left[repeat * time];
            // Before it there is the tail of the 10 Hz high pass and no more.
            assert!(left[repeat * time - 1].abs() < 1e-6, "repeat {repeat}");
            // The tails of the high pass from the repeats before take a
            // little off the later ones.
            let off = (f64::from(got) / want - 1.0).abs();
            assert!(off < 1e-4, "repeat {repeat}: {got} and not {want}");
            want *= ratio;
        }
        // Stereo's two loops do not feed each other.
        assert_eq!(first(&right), None);

        // At the top of the wheel the loop gains: 1.25 a pass.
        let mut plugin = clean(&[(FEEDBACK, 6000), (TIME, 48)]);
        let (left, _) = impulse(&mut plugin, 0.01, 0.0, 2 * time + 10);
        let grows = 1.25 * dc_b0(HZ) * a3(1.0, HZ);
        assert!(close(left[2 * time], f64::from(left[time]) * grows));
    }

    #[test]
    fn mono_sums_the_sides_and_ping_pong_alternates_them() {
        let time = total(48, true, HZ, 120.0);
        let open = a3(1.0, HZ);
        let ratio = 1.25 * 0.5 * dc_b0(HZ) * open;
        // The repeats a side has sound in. The 10 Hz high pass leaves a tail
        // under 1e-6 in the others.
        let repeats = |side: &[f32]| {
            let mut found: Vec<usize> = Vec::new();
            for (n, _) in side.iter().enumerate().filter(|(_, v)| v.abs() > 1e-4) {
                if found.last() != Some(&(n / time)) {
                    found.push(n / time);
                }
            }
            found
        };

        // Mono: half the sum of the sides, the same on both.
        let mut plugin = clean(&[(MODEL, 0), (FEEDBACK, 3000), (TIME, 48)]);
        let (left, right) = impulse(&mut plugin, 0.8, 0.0, 4 * time);
        assert_eq!(first(&left), Some(time));
        assert!(close(left[time], 0.4 * open));
        assert!(close(left[2 * time], 0.4 * open * ratio));
        assert_eq!(left, right);
        assert_eq!(repeats(&left), [1, 2, 3]);

        // Ping pong: the left side's click is on the right first, then on
        // the left, then on the right again.
        let mut plugin = clean(&[(MODEL, 2), (FEEDBACK, 3000), (TIME, 48)]);
        let (left, right) = impulse(&mut plugin, 0.5, 0.0, 5 * time);
        assert_eq!((first(&right), first(&left)), (Some(time), Some(2 * time)));
        assert_eq!((repeats(&right), repeats(&left)), (vec![1, 3], vec![2, 4]));
        assert!(close(right[time], 0.5 * open));
        assert!(close(left[2 * time], 0.5 * open * ratio));
        assert!(close(right[3 * time], 0.5 * open * ratio * ratio));
        // And the right side's the other way around.
        let mut plugin = clean(&[(MODEL, 2), (FEEDBACK, 3000), (TIME, 48)]);
        let (left, right) = impulse(&mut plugin, 0.0, 0.5, 5 * time);
        assert_eq!((repeats(&left), repeats(&right)), (vec![1, 3], vec![2, 4]));

        // Stereo keeps a side's repeats on that side.
        let mut plugin = clean(&[(FEEDBACK, 3000), (TIME, 48)]);
        let (left, right) = impulse(&mut plugin, 0.5, 0.0, 4 * time);
        assert_eq!((repeats(&left), repeats(&right)), (vec![1, 2, 3], vec![]));
    }

    #[test]
    fn time_r_is_ping_pong_s_pan_and_a_later_tap_or_input_in_the_others() {
        // The doc's law at the ends and the centre, as [LL, RR, RL, LR].
        assert_eq!(pan_law(0.5), [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(pan_law(1.0), [0.0, 0.5, 0.0, 0.5]);
        assert_eq!(pan_law(0.0), [0.5, 0.0, 0.5, 0.0]);
        assert_eq!(pan_law(0.75), [0.5, 0.75, 0.0, 0.25]);
        // All the way down, as in FL's Ping pong preset, both inputs go in
        // on the left at half, and the left's loop is heard on the right
        // first.
        let time = total(48, true, HZ, 120.0);
        let open = a3(1.0, HZ);
        let mut plugin = clean(&[(MODEL, 2), (TIME, 48), (TIME_R, 0)]);
        let (left, right) = impulse(&mut plugin, 0.0, 0.8, 2 * time);
        assert_eq!((first(&left), first(&right)), (None, Some(time)));
        assert!(close(right[time], 0.4 * open));

        // Mono reads the right side a quarter of the time further back
        // with the wheel half way up, and the left side with it half way
        // down.
        let late = time + time / 4;
        let mut plugin = clean(&[(MODEL, 0), (TIME, 48), (TIME_R, 4500)]);
        let (left, right) = impulse(&mut plugin, 0.8, 0.8, 2 * time);
        assert_eq!((first(&left), first(&right)), (Some(time), Some(late)));
        let mut plugin = clean(&[(MODEL, 0), (TIME, 48), (TIME_R, 1500)]);
        let (left, right) = impulse(&mut plugin, 0.8, 0.8, 2 * time);
        assert_eq!((first(&left), first(&right)), (Some(late), Some(time)));
        // The feedback is the tap that is the time long, on either side.
        let mut plugin = clean(&[(MODEL, 0), (TIME, 48), (TIME_R, 1500), (FEEDBACK, 3000)]);
        let (_, right) = impulse(&mut plugin, 0.8, 0.8, 3 * time);
        let ratio = 1.25 * 0.5 * dc_b0(HZ) * open;
        assert!(close(right[2 * time], f64::from(right[time]) * ratio));

        // Stereo holds one input back by the square of the lean: a
        // sixteenth of the time with the wheel half way up, on the left.
        let held = time / 16;
        let mut plugin = clean(&[(TIME, 48), (TIME_R, 4500)]);
        assert_eq!(plugin.heads[0].live[B], held as f64);
        let (left, right) = impulse(&mut plugin, 0.8, 0.8, 2 * time);
        // A tap of 1 is no wait, so one of `held` is `held - 1` samples.
        assert_eq!(first(&right), Some(time));
        assert_eq!(first(&left), Some(time + held - 1));
    }

    #[test]
    fn a_low_pass_at_the_bottom_takes_a_high_tone_off_the_repeats() {
        let time = total(48, true, HZ, 120.0);
        // 8820 Hz and 88.2 Hz: 5 and 500 samples a period.
        let (high, low) = (0.2, 0.002);
        let echo = |cutoff: i32, turns: f64| {
            let mut plugin = clean(&[(TIME, 48), (CUTOFF, cutoff)]);
            let tone: Vec<f32> = sine(turns, time + 12_000).iter().map(|v| v * 0.5).collect();
            let (left, _) = run(&mut plugin, &tone, &tone);
            partial(&left[time + 5000..time + 10_000], turns)
        };
        // With the wheel down the cutoff is 270 Hz, two poles: the high
        // tone comes back 63 dB down and the low one as good as whole.
        let (gone, kept) = (low_gain(0.0, high, HZ), low_gain(0.0, low, HZ));
        assert!((20.0 * gone.log10() + 63.1).abs() < 0.1, "{gone}");
        assert!((kept - 0.994).abs() < 1e-3, "{kept}");
        let (got_high, got_low) = (echo(0, high), echo(0, low));
        assert!((got_high / (0.5 * gone) - 1.0).abs() < 0.01, "{got_high}");
        assert!((got_low / (0.5 * kept) - 1.0).abs() < 0.01, "{got_low}");
        // With the wheel up the high tone is whole.
        let open = low_gain(1.0, high, HZ);
        assert!((open - 1.0).abs() < 1e-3, "{open}");
        assert!((echo(6000, high) / (0.5 * open) - 1.0).abs() < 0.01);
    }

    #[test]
    fn limit_holds_a_repeat_6_db_over_at_the_ceiling() {
        let time = total(48, true, HZ, 120.0);
        // A level of 2, which is 6 dB over a ceiling of 1.
        let loud = vec![2.0; time + 22_050];
        // The ceiling is the drive's gain times the make-up. The doc's
        // polynomial makes the make-up 0.89 half way up the wheel and 2.2
        // at the top, where its prose says 0.82.
        assert!((ceiling(1.0) / 10f64.powf(-28.5 / 20.0) - 2.2).abs() < 5e-3);
        for (amount, top) in [(0, 1.0), (3000, 0.1723), (6000, 0.0828)] {
            let want = ceiling(f64::from(amount) / 6000.0);
            assert!((want / top - 1.0).abs() < 5e-3, "amount {amount}: {want}");
            let mut plugin = clean(&[(TIME, 48), (DIST_AMOUNT, amount)]);
            let (left, right) = run(&mut plugin, &loud, &loud);
            // The envelope takes 10 ms to get there, so the start of the
            // repeat is over the ceiling.
            assert!(f64::from(left[time + 5]) > 1.5 * want, "amount {amount}");
            // From then on the repeat is the ceiling.
            for got in &left[time + 4410..] {
                assert!((f64::from(*got) / want - 1.0).abs() < 1e-4, "{got}");
            }
            assert_eq!(left, right);
        }
        // Saturate ends at the same ceiling with no envelope: the top of
        // its parabola.
        let mut plugin = clean(&[(TIME, 48), (DIST_TYPE, 1), (DIST_AMOUNT, 3000)]);
        let (left, _) = run(&mut plugin, &loud, &loud);
        let want = ceiling(0.5);
        assert!((f64::from(left[time + 300]) / want - 1.0).abs() < 1e-4);
    }

    #[test]
    fn saturate_bends_a_sample_along_the_doc_s_parabola() {
        let time = total(48, true, HZ, 120.0);
        let open = a3(1.0, HZ);
        // The doc's shape for one sample.
        let shape = |x: f64, amount: f64, hard: f64, sym: f64| {
            let d = 0.95 * amount;
            let g = 10f64.powf(-30.0 * d / 20.0);
            let top = if x >= 0.0 { 1.0 + sym } else { 1.0 - sym } * g;
            let threshold = top * hard;
            let knee = (top - threshold).max(1e-11);
            let mut size = x.abs();
            if size >= threshold {
                let over = (size - threshold).min(2.0 * knee);
                size = threshold + over - over * over * 0.25 / knee;
            }
            size.copysign(x) * ceiling(amount) / g
        };
        // With every wheel down it is x - x^2 / 4.
        assert!((shape(0.5, 0.0, 0.0, 0.0) - 0.4375).abs() < 1e-12);
        for (amount, hard, sym) in [
            (0, 0, 0),
            (2086, 492, 0),
            (3000, 6000, 0),
            (1500, 3000, 4500),
        ] {
            let unit = |value: i32| f64::from(value) / 6000.0;
            for click in [0.5, -0.5, 0.05] {
                let wheels = [(DIST_AMOUNT, amount), (HARDNESS, hard), (SYMMETRY, sym)];
                let mut plugin = clean(&[&[(TIME, 48), (DIST_TYPE, 1)], &wheels[..]].concat());
                let (left, _) = impulse(&mut plugin, click as f32, 0.0, time + 1);
                let want = shape(click * open, unit(amount), unit(hard), unit(sym));
                assert!(
                    close(left[time], want),
                    "{wheels:?}: {} and not {want}",
                    left[time]
                );
            }
        }
    }

    #[test]
    fn the_bit_reducer_at_the_bottom_leaves_steps_of_a_quarter() {
        let time = total(48, true, HZ, 120.0);
        let tone: Vec<f32> = sine(0.01, time + 2000).iter().map(|v| v * 0.9).collect();
        // The wheel all the way down is 2^2 steps in 1.
        let mut plugin = clean(&[(TIME, 48), (BITS, 0)]);
        let (left, _) = run(&mut plugin, &tone, &tone);
        let mut levels: Vec<i32> = left[time..].iter().map(|v| (v * 4.0) as i32).collect();
        assert!(left[time..].iter().all(|v| (v * 4.0).fract() == 0.0));
        levels.sort_unstable();
        levels.dedup();
        // It rounds towards 0, so 0.9 gets to 3 quarters and no further.
        assert_eq!(levels, [-3, -2, -1, 0, 1, 2, 3]);

        // Half way it is 2^(2 + 14 (1 - sqrt(0.5))) steps, 68.6.
        let steps = 2f64.powf(2.0 + 14.0 * (1.0 - 0.5f64.sqrt()));
        assert!((steps - 68.6).abs() < 0.05);
        let mut plugin = clean(&[(TIME, 48), (BITS, 3000)]);
        let (left, _) = run(&mut plugin, &tone, &tone);
        for got in &left[time..] {
            let level = f64::from(*got) * steps;
            assert!((level - level.round()).abs() < 1e-3, "{got}");
        }
        // And at the top it is off.
        let mut plugin = clean(&[(TIME, 48)]);
        let (left, _) = run(&mut plugin, &tone, &tone);
        assert!(left[time..].iter().any(|v| (v * 4.0).fract() != 0.0));
    }

    #[test]
    fn the_rate_reducer_at_the_bottom_holds_a_sample_for_229() {
        let time = total(48, true, HZ, 120.0);
        // The doc's curve at r = 1, in samples at 44.1 kHz.
        let f = 127.0f32;
        let hold = 0.0002 * f * f * f - 0.0171 * f * f + 0.7435 * f + 1.0;
        assert!((hold - 229.3).abs() < 0.05, "{hold}");
        // The doc's counter says which samples get through.
        let len = 3000;
        let (mut count, mut fresh) = (0.0f32, Vec::new());
        for n in 0..len {
            count += 1.0;
            if count >= hold {
                count = (count / hold).fract() * count;
                fresh.push(n);
            }
        }
        assert_eq!(fresh[..3], [229, 458, 687]);
        assert!(
            fresh
                .windows(2)
                .all(|pair| [229, 230].contains(&(pair[1] - pair[0])))
        );

        let tone: Vec<f32> = sine(0.0013, time + len).iter().map(|v| v * 0.9).collect();
        let mut plugin = clean(&[(TIME, 48), (RATE, 0)]);
        let (left, _) = run(&mut plugin, &tone, &tone);
        // The repeat changes at those samples and nowhere else.
        let echo = &left[time..];
        let changes: Vec<usize> = (1..len).filter(|n| echo[*n] != echo[n - 1]).collect();
        assert_eq!(changes, fresh);
        // At the top of the wheel it is off.
        let mut plugin = clean(&[(TIME, 48)]);
        let (left, _) = run(&mut plugin, &tone, &tone);
        assert!((1..len).all(|n| left[time + n] != left[time + n - 1]));
    }

    #[test]
    fn the_tone_at_the_centre_leaves_the_repeats_alone() {
        // At the centre the filter is 1, 0, 0, whatever it holds.
        assert_eq!(First::tone(0.5, HZ), First::THROUGH);
        let mut held = [0.7, -0.3];
        for x in [0.25f32, -1.0, 1e-20, 3.5] {
            assert_eq!(First::THROUGH.run(&mut held, x).to_bits(), x.to_bits());
        }
        assert_eq!(unit(TONE, 0), 0.5);

        // So what the bit reducer wrote into the line comes out on its
        // steps to the bit, and off them with the tone moved.
        let time = total(48, true, HZ, 120.0);
        let tone: Vec<f32> = sine(0.01, time + 2000).iter().map(|v| v * 0.9).collect();
        let on_steps = |plugin: &mut Delay3| {
            let (left, _) = run(plugin, &tone, &tone);
            left[time..].iter().all(|v| (v * 4.0).fract() == 0.0)
        };
        assert!(on_steps(&mut clean(&[(TIME, 48), (BITS, 0)])));
        assert!(!on_steps(&mut clean(&[(TIME, 48), (BITS, 0), (TONE, -1)])));
        assert!(!on_steps(&mut clean(&[(TIME, 48), (BITS, 0), (TONE, 1)])));

        // Under the centre it is a low pass down to 270 Hz and above it a
        // high pass up to 4.2 kHz: the doc's first order filters.
        let k = (270.0 * PI / HZ).tan();
        let low = First::tone(0.0, HZ);
        assert!(close(low.b0, k / (1.0 + k)) && close(low.b1, k / (1.0 + k)));
        assert!(close(low.a1, (1.0 - k) / (1.0 + k)));
        let k = (4200.0 * PI / HZ).tan();
        let high = First::tone(1.0, HZ);
        assert!((f64::from(high.b0) - 1.0 / (1.0 + k)).abs() < 1e-4);
        assert_eq!(high.b1, -high.b0);
    }

    #[test]
    fn the_default_is_a_beat_of_stereo_echo_at_62_percent_under_the_dry_signal() {
        let mut plugin = Delay3::default();
        let shown: Vec<f32> = (0..PARAMS).map(|index| plugin.param(index)).collect();
        assert_eq!(shown, DEFAULTS.map(|value| value as f32));
        assert_eq!((plugin.param_count(), plugin.param(PARAMS)), (PARAMS, 0.0));
        // Before a tempo comes it is 140 BPM: a beat is 18900 samples.
        assert_eq!(plugin.heads[0].live[A] + plugin.heads[0].live[D], 18_900.0);

        plugin.set_tempo(120.0);
        let beat = total(192, true, HZ, 120.0);
        let (mut left, mut right) = (vec![0.0; 3 * beat], vec![0.0; 3 * beat]);
        (left[0], right[3]) = (0.5, -0.25);
        let (out_left, out_right) = run(&mut plugin, &left, &right);
        // The dry signal is as it was, and nothing else comes for a beat.
        assert_eq!(out_left[..beat], left[..beat]);
        assert_eq!(out_right[..beat + 3], right[..beat + 3]);

        // Each side's echo is on its own side a beat on, with no input
        // held back. It went through the open low pass and Saturate, which
        // with every wheel down is x - x^2 / 4.
        let open = a3(1.0, HZ);
        let bent = |x: f64| x.signum() * (x.abs() - x * x / 4.0);
        let (first_left, first_right) = (bent(0.5 * open), bent(-0.25 * open));
        assert!(close(out_left[beat], first_left), "{}", out_left[beat]);
        assert!(close(out_right[beat + 3], first_right));
        assert_eq!(out_right[beat..beat + 3], [0.0; 3]);
        // The second is the first at 1.25 * 0.5 through the 10 Hz high
        // pass, the low pass and Saturate again.
        let around = |x: f64| bent(1.25 * 0.5 * dc_b0(HZ) * open * x);
        assert_eq!(out_left[2 * beat - 1], 0.0);
        assert!(close(out_left[2 * beat], around(first_left)));
        assert!(close(out_right[2 * beat + 3], around(first_right)));

        // The spread at the top keeps the sides apart: with the right
        // side silent nothing comes out of it.
        let mut plugin = with(&[]);
        let (_, silent) = impulse(&mut plugin, 0.5, 0.0, 3 * beat);
        assert_eq!(first(&silent), None);
        // At the bottom each side has half of both.
        let mut plugin = with(&[(SPREAD, 0)]);
        let (out_left, out_right) = impulse(&mut plugin, 0.5, 0.0, beat + 1);
        assert!(close(out_left[beat], 0.5 * first_left));
        assert_eq!(out_left[beat], out_right[beat]);
    }

    #[test]
    fn the_levels_are_the_wheels_and_the_feedback_is_125_percent_at_the_top() {
        let time = total(48, true, HZ, 120.0);
        let open = a3(1.0, HZ);
        // Input wet and output wet scale the echo, output dry the input.
        let mut plugin = with(&[
            (DIST_TYPE, 0),
            (FEEDBACK, 0),
            (TIME, 48),
            (INPUT_WET, 3000),
            (OUTPUT_WET, 1500),
            (OUTPUT_DRY, 4500),
        ]);
        let (left, _) = impulse(&mut plugin, 0.8, 0.0, time + 1);
        assert_eq!(left[0], 0.8 * 0.75);
        assert!(close(left[time], 0.8 * 0.5 * open * 0.25));
        // The doc's readings of the feedback wheel.
        assert_eq!(1.25 * unit(FEEDBACK, 6000), 1.25);
        assert!((1.25 * unit(FEEDBACK, 4200) - 0.875).abs() < 1e-6);
        // A sample under 1e-13 is silence to the engine.
        let mut plugin = with(&[]);
        let (left, _) = impulse(&mut plugin, 9e-14, 0.0, 4);
        assert_eq!(first(&left), None);
    }

    #[test]
    fn the_lfo_is_as_slow_and_as_deep_as_the_doc_s_table() {
        // 8.872 s a turn at the bottom of the rate wheel and 17.8 Hz at
        // the top.
        let slow = with(&[(MOD_RATE, 0)]);
        assert!(close(slow.phase_step, 1.0 / (HZ * 8.872)));
        assert_eq!(slow.reach, 1.0);
        let fast = with(&[(MOD_RATE, 6000)]);
        let period: f64 = 0.150 + (0.050 - 0.150) * (0.99 * 5.9994 - 5.0);
        assert!((1.0 / period - 17.8).abs() < 0.05);
        assert!(close(fast.phase_step, 1.0 / (HZ * period)));
        assert!(close(fast.reach, (period / 8.872).sqrt()));

        // The Chorus preset: 0.1 v^2 of half a second, less for the
        // faster LFO, is 13 ms that the heads move by half to one and a
        // half of.
        let chorus = with(&[(MOD_RATE, 1032), (MOD_TIME, 3806)]);
        let ms = f64::from(chorus.depth) / 44.1;
        assert!((ms - 13.1).abs() < 0.1, "{ms}");

        // The echo of a tone is bent by it: with the depth up, a repeat
        // is no longer the tone that went in.
        let time = total(48, true, HZ, 120.0);
        let tone: Vec<f32> = sine(0.01, time + 12_000).iter().map(|v| v * 0.5).collect();
        let level = |changes: &[(usize, i32)]| {
            let (left, _) = run(&mut clean(changes), &tone, &tone);
            partial(&left[time + 2000..time + 12_000], 0.01)
        };
        assert!((level(&[(TIME, 48)]) - 0.5).abs() < 1e-3);
        assert!(level(&[(TIME, 48), (MOD_RATE, 6000), (MOD_TIME, 6000)]) < 0.4);
    }

    #[test]
    fn the_filter_modes_are_the_doc_s_tables() {
        let tuned = |changes: &[(usize, i32)]| {
            let mut plugin = with(changes);
            plugin.process(&mut [0.0], &mut [0.0]);
            plugin.filter
        };
        let g = |fc: f64| (fc * FILTER_PI / HZ).tan();
        let a1 = |g: f64, k: f64| 1.0 / (1.0 + g * (g + k));
        // Low pass half way up the wheel: between 480 and 1300 Hz.
        let low = tuned(&[(CUTOFF, 3000)]);
        let (g_low, k) = (g(480.0 + 820.0 * (3.999 * 0.5 - 1.0)), 1.0 / 0.707);
        assert!(close(low.a[0], a1(g_low, k)));
        assert!(close(low.a[2], a1(g_low, k) * g_low * g_low));
        assert_eq!(low.mix, [0.0, 0.0, 1.0]);
        // High pass at the bottom is 20 Hz, and full resonance a Q of
        // 1.707.
        let high = tuned(&[(FILTER_MODE, 1), (CUTOFF, 0), (RESONANCE, 6000)]);
        let k = 1.0 / 1.707;
        assert!(close(high.a[0], a1(g(20.0), k)));
        assert_eq!(high.mix[0], 1.0);
        assert!(close(high.mix[1], -k) && high.mix[2] == -1.0);
        // Band pass at the top is 16 kHz, with a Q of 0.107 to 1.257.
        let band = tuned(&[(FILTER_MODE, 2), (CUTOFF, 6000)]);
        let top = 7000.0 + 15_000.0 * 0.6;
        assert!(close(band.a[0], a1(g(top), 1.0 / 0.107)));
        assert!(close(band.mix[1], 1.0 / 0.107));
        let band = tuned(&[(FILTER_MODE, 2), (RESONANCE, 6000)]);
        assert!(close(band.mix[1], 1.0 / 1.257));

        // The mod cutoff wheel adds half of itself times the LFO, which
        // starts half way: a cutoff of 0 is then a quarter up the wheel.
        let moved = tuned(&[(CUTOFF, 0), (MOD_CUTOFF, 6000)]);
        let g_moved = g(270.0 + 210.0 * (3.999 * 0.25));
        assert!(close(moved.a[1], a1(g_moved, 1.0 / 0.707) * g_moved));
    }

    #[test]
    fn a_moved_wheel_glides_and_a_state_does_not() {
        // Output dry is one of the wheels FL eases: every 8 samples it
        // goes 1 - 0.01^(1 / (0.005 rate)) of what is left.
        let step = 1.0 - 0.01f64.powf(1.0 / (0.005 * HZ));
        let mut plugin = with(&[(FEEDBACK, 0)]);
        plugin.process(&mut [1.0; 8], &mut [1.0; 8]);
        plugin.set_param(OUTPUT_DRY, 0.0);
        assert_eq!(plugin.param(OUTPUT_DRY), 0.0);
        let (left, _) = run(&mut plugin, &[1.0; 64], &[1.0; 64]);
        for (n, got) in left.iter().enumerate() {
            let want = (1.0 - step).powi(n as i32 / 8 + 1);
            assert!(close(*got, want), "frame {n}: {got} and not {want}");
        }
        // It ends on the wheel.
        run(&mut plugin, &[1.0; 4000], &[1.0; 4000]);
        assert!(!plugin.gliding);
        assert_eq!(plugin.now[OUTPUT_DRY], 0.0);

        // A state is there at once, and so is a wheel moved before any
        // sound has gone through.
        let mut plugin = with(&[(FEEDBACK, 0)]);
        plugin.process(&mut [1.0; 8], &mut [1.0; 8]);
        plugin
            .set_state(&bytes(&moved(&[(FEEDBACK, 0), (OUTPUT_DRY, 0)])))
            .unwrap();
        assert_eq!(run(&mut plugin, &[1.0; 8], &[1.0; 8]).0, [0.0; 8]);
        let mut plugin = with(&[(FEEDBACK, 0)]);
        plugin.set_param(OUTPUT_DRY, 3000.0);
        assert_eq!(run(&mut plugin, &[1.0; 8], &[1.0; 8]).0, [0.5; 8]);
    }

    #[test]
    fn a_new_time_glides_without_keep_pitch_and_fades_in_with_it() {
        let (beat, half) = (21_985.0, 10_960.0);
        // Time smooth at its default is 99 % of the way in half a second.
        let mut plugin = with(&[]);
        plugin.process(&mut [0.0; 16], &mut [0.0; 16]);
        plugin.set_param(TIME, 96.0);
        assert_eq!(plugin.heads[0].live[D], beat);
        run(&mut plugin, &vec![0.0; 22_050], &vec![0.0; 22_050]);
        let left = plugin.heads[0].live[D] - half;
        assert!((left / (beat - half) - 0.01).abs() < 1e-6, "{left}");
        // The tempo moves the time the same way.
        plugin.set_tempo(60.0);
        plugin.process(&mut [0.0; 16], &mut [0.0; 16]);
        assert_eq!(plugin.heads[0].target[D], beat);
        assert!(plugin.heads[0].live[D] < half + 200.0);
        // With Time smooth at 0 it is there on the next sample.
        let mut plugin = with(&[(TIME_SMOOTH, 0)]);
        plugin.process(&mut [0.0; 16], &mut [0.0; 16]);
        plugin.set_param(TIME, 96.0);
        plugin.process(&mut [0.0], &mut [0.0]);
        assert_eq!(plugin.heads[0].live[D], half);

        // With keep pitch the idle head has the new time at once and the
        // two are faded over a tenth of a second, 4410 samples.
        let mut plugin = with(&[(KEEP_PITCH, 1)]);
        plugin.process(&mut [0.0; 16], &mut [0.0; 16]);
        plugin.set_param(TIME, 96.0);
        plugin.process(&mut [0.0], &mut [0.0]);
        assert!(plugin.fading);
        assert_eq!(
            (plugin.heads[0].live[D], plugin.heads[1].live[D]),
            (beat, half)
        );
        assert_eq!(plugin.weights, [1.0, 0.0]);
        run(&mut plugin, &vec![0.0; 2205], &vec![0.0; 2205]);
        assert!(close(plugin.weights[0], 0.5) && close(plugin.weights[1], 0.5));
        run(&mut plugin, &vec![0.0; 2204], &vec![0.0; 2204]);
        assert!(!plugin.fading);
        assert_eq!(plugin.active, 1);
        assert!(close(plugin.weights[1], 4409.0 / 4410.0));
        // The next move fades back to the first head.
        plugin.set_param(TIME, 192.0);
        run(&mut plugin, &vec![0.0; 4410], &vec![0.0; 4410]);
        assert_eq!((plugin.active, plugin.heads[0].live[D]), (0, beat));
    }

    #[test]
    fn another_model_or_a_reset_empties_the_lines() {
        let time = total(48, true, HZ, 120.0);
        let tail = |change: &dyn Fn(&mut Delay3)| {
            let mut plugin = clean(&[(TIME, 48), (TIME_SMOOTH, 0)]);
            impulse(&mut plugin, 0.5, 0.5, 100);
            change(&mut plugin);
            let (left, _) = impulse(&mut plugin, 0.0, 0.0, time);
            first(&left)
        };
        // Left alone, the echo comes a time after the click.
        assert_eq!(tail(&|_| {}), Some(time - 100));
        assert_eq!(tail(&|p| p.set_tempo(120.0)), Some(time - 100));
        assert_eq!(tail(&|p| p.set_param(CUTOFF, 100.0)), Some(time - 100));
        // A slower tempo may need a longer line, which keeps its sound.
        let slow = total(48, true, HZ, 30.0);
        let mut plugin = clean(&[(TIME, 48), (TIME_SMOOTH, 0)]);
        impulse(&mut plugin, 0.5, 0.5, 100);
        let before = plugin.line[0].samples.len();
        plugin.set_tempo(30.0);
        assert!(plugin.line[0].samples.len() > before);
        let (left, _) = impulse(&mut plugin, 0.0, 0.0, slow);
        assert_eq!(first(&left), Some(slow - 100));
        // Another model wipes it, and so do a reset and a new rate.
        assert_eq!(tail(&|p| p.set_param(MODEL, 2.0)), None);
        assert_eq!(tail(&|p| p.reset()), None);
        assert_eq!(tail(&|p| p.set_sample_rate(HZ as f32)), None);

        // A tempo FL does not have is held to the nearest it has, and one
        // that is no number is not taken. Nor is such a rate.
        let mut plugin = with(&[]);
        plugin.set_tempo(1.0);
        assert_eq!(plugin.tempo, 10.0);
        plugin.set_tempo(f64::INFINITY);
        plugin.set_tempo(f64::NAN);
        plugin.set_sample_rate(0.0);
        plugin.set_sample_rate(f32::NAN);
        assert_eq!((plugin.tempo, plugin.hz), (522.0, HZ));
    }

    #[test]
    fn no_wheel_asks_for_memory() {
        let mut plugin = with(&[]);
        let memory = |plugin: &mut Delay3| -> Vec<(*const f32, usize)> {
            let rings = plugin.rings();
            rings
                .map(|ring| (ring.samples.as_ptr(), ring.samples.len()))
                .collect()
        };
        let before = memory(&mut plugin);
        // Four beats at 120 are 88200 samples, Mono's far tap half as
        // much again and the LFO 3307 more: 2^18 holds that.
        assert_eq!(before[2].1, 1 << 18);
        plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
        for (index, value) in [(TIME, 768), (MODEL, 0), (TIME_R, 6000), (MOD_TIME, 6000)] {
            plugin.set_param(index, value as f32);
        }
        plugin.set_param(DIFFUSION_SPREAD, 6000.0);
        plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
        assert_eq!(plugin.heads[0].target[C], 88_200.0 * 1.5 - 655.0);
        plugin.set_param(SYNC, 0.0);
        plugin.set_state(&bytes(&DEFAULTS)).unwrap();
        plugin.reset();
        plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
        assert_eq!(memory(&mut plugin), before);

        // The rings are never longer than FL's, which go with the rate.
        assert_eq!([44_100.0, 61_440.0].map(fl_ring), [1 << 20; 2]);
        assert_eq!([96_000.0, 126_976.0].map(fl_ring), [1 << 21; 2]);
        assert_eq!(fl_ring(192_000.0), 1 << 22);
        plugin.set_tempo(10.0);
        assert_eq!(plugin.line[0].samples.len(), 1 << 20);
        // A length over the ring wraps and does not fail.
        plugin.set_param(TIME, 768.0);
        plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
        plugin.set_sample_rate(192_000.0);
        plugin.process(&mut [1.0; 64], &mut [1.0; 64]);
        assert_eq!(plugin.line[0].samples.len(), 1 << 22);
    }

    #[test]
    fn a_wheel_takes_the_nearest_whole_number_inside_its_range() {
        let mut plugin = Delay3::default();
        for (index, value, want) in [
            (TIME, 1000.0, 768.0),
            (TONE, -4000.0, -3000.0),
            (RATE, 70_000.0, 60_000.0),
            (MODEL, 1.6, 2.0),
            (FEEDBACK, 2999.5, 3000.0),
            (DIST_TYPE, -3.0, 0.0),
        ] {
            plugin.set_param(index, value);
            assert_eq!(plugin.param(index), want, "parameter {index}");
        }
        plugin.set_param(PARAMS, 1.0);
        assert_eq!(plugin.param(PARAMS), 0.0);
        // Moved wheels are the same plugin as their state.
        let state = moved(&[
            (TIME, 768),
            (TONE, -3000),
            (RATE, 60_000),
            (MODEL, 2),
            (DIST_TYPE, 0),
        ]);
        let (left, right) = impulse(&mut plugin, 0.5, 0.25, 600);
        let mut same = Delay3::default();
        same.set_state(&bytes(&state)).unwrap();
        assert_eq!(impulse(&mut same, 0.5, 0.25, 600), (left, right));
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with(&[(TIME, 96), (MODEL, 2)]);
        let before = plugin.params;
        assert_eq!(
            plugin.set_state(&[1, 0, 0, 0, 0, 0, 0, 0]),
            Err(StateError::Length {
                expected: 108,
                found: 8
            })
        );
        assert_eq!(
            plugin.set_state(&[1, 0]),
            Err(StateError::Length {
                expected: 108,
                found: 2
            })
        );
        // FL reads the parameters of a version under 2 only.
        let mut newer = bytes(&DEFAULTS);
        newer[0] = 2;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 2 })
        );
        for (param, value) in [
            (INPUT_WET, 6001),
            (MODEL, 3),
            (SYNC, 2),
            (TIME, -1),
            (TIME, 769),
            (TIME_SMOOTH, 1001),
            (DIST_TYPE, 2),
            (FILTER_MODE, -1),
            (RATE, 60_001),
            (TONE, -3001),
            (TONE, 3001),
            (OUTPUT_DRY, -1),
        ] {
            assert_eq!(
                plugin.set_state(&bytes(&moved(&[(param, value)]))),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin.params, before);
        // A Time of 0 is what the Chorus preset has, and is a Time of 1.
        plugin.set_state(&bytes(&moved(&[(TIME, 0)]))).unwrap();
        assert_eq!(plugin.now[TIME], 0.0);
    }

    #[test]
    fn fl_s_presets_load() {
        let Some(presets) = presets(NAME) else { return };
        // What the ten files hold, after the version.
        let want: [(&str, [i32; PARAMS]); 10] = [
            (
                "Bandpass",
                moved(&[
                    (KEEP_PITCH, 1),
                    (TIME, 144),
                    (MOD_TIME, 1275),
                    (FILTER_MODE, 2),
                    (CUTOFF, 3562),
                    (RESONANCE, 2138),
                    (DIST_AMOUNT, 2485),
                ]),
            ),
            (
                "Chorus",
                moved(&[
                    (SYNC, 0),
                    (TIME, 0),
                    (MOD_RATE, 1032),
                    (MOD_TIME, 3806),
                    (DIST_TYPE, 0),
                    (FEEDBACK, 0),
                    (CUTOFF, 4950),
                ]),
            ),
            ("Default", DEFAULTS),
            (
                "Diffused",
                moved(&[
                    (TIME, 144),
                    (DIFFUSION, 5025),
                    (DIFFUSION_SPREAD, 5082),
                    (DIST_TYPE, 0),
                    (FEEDBACK, 3038),
                    (CUTOFF, 5137),
                    (DIST_AMOUNT, 492),
                    (TONE, 900),
                ]),
            ),
            (
                "Mono",
                moved(&[
                    (MODEL, 0),
                    (TIME, 96),
                    (FEEDBACK, 3506),
                    (CUTOFF, 4481),
                    (DIST_AMOUNT, 399),
                ]),
            ),
            (
                "Ping pong",
                moved(&[
                    (MODEL, 2),
                    (KEEP_PITCH, 1),
                    (TIME, 96),
                    (TIME_R, 0),
                    (MOD_RATE, 2109),
                    (MOD_TIME, 1819),
                    (CUTOFF, 4612),
                    (RESONANCE, 975),
                    (DIST_AMOUNT, 398),
                ]),
            ),
            (
                "Resonator",
                moved(&[
                    (SYNC, 0),
                    (TIME, 37),
                    (DIST_TYPE, 0),
                    (FEEDBACK, 4200),
                    (CUTOFF, 4762),
                    (DIST_AMOUNT, 2390),
                    (OUTPUT_WET, 2997),
                ]),
            ),
            (
                "Saturated",
                moved(&[
                    (KEEP_PITCH, 1),
                    (MOD_TIME, 1819),
                    (CUTOFF, 4744),
                    (RESONANCE, 975),
                    (DIST_AMOUNT, 2086),
                    (HARDNESS, 492),
                ]),
            ),
            (
                "Stereoized",
                moved(&[
                    (MODEL, 0),
                    (TIME, 144),
                    (TIME_R, 3469),
                    (FEEDBACK, 2681),
                    (CUTOFF, 4481),
                    (DIST_AMOUNT, 399),
                    (TONE, 1088),
                ]),
            ),
            (
                "Tape",
                moved(&[
                    (KEEP_PITCH, 1),
                    (TIME, 144),
                    (MOD_RATE, 1969),
                    (MOD_TIME, 2737),
                    (CUTOFF, 3787),
                    (RESONANCE, 919),
                    (DIST_AMOUNT, 258),
                    (TONE, 900),
                ]),
            ),
        ];
        assert_eq!(presets.len(), want.len());
        for (preset, (file, params)) in presets.iter().zip(want) {
            assert_eq!(preset.file, file);
            assert!(preset.plugin.eq_ignore_ascii_case(NAME), "{file}");
            assert_eq!(preset.state, bytes(&params), "{file}");
            let mut plugin = with(&[(MODEL, 0), (TIME, 1)]);
            plugin.set_state(&preset.state).unwrap();
            assert_eq!(plugin.params, params, "{file}");
            // And it runs.
            let (left, right) = impulse(&mut plugin, 0.5, 0.5, 30_000);
            assert!(left.iter().chain(&right).all(|v| v.is_finite()), "{file}");
            assert!(left[1..].iter().any(|v| *v != 0.0), "{file}");
        }
    }

    #[test]
    fn every_state_in_fl_s_projects_loads() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, the Delay 3 states in FL's projects skipped");
            return;
        };
        let mut uses = 0;
        for saved in projects(&root) {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let mut plugin = Delay3::default();
            if let Err(error) = plugin.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            assert_eq!(saved.state.len(), STATE_LEN, "{}", saved.file);
            plugin.process(&mut [0.5; 64], &mut [-0.5; 64]);
            uses += 1;
        }
        // What the doc counts in the 165 projects.
        assert_eq!(uses, 110);
    }
}
