//! Fruity Multiband Compressor: three bands, a compressor on each, and a
//! limiter after their sum, as
//! `docs/plugins/fruity-multiband-compressor.md` has it.
//!
//! The state is 124 bytes: a version of 0 and thirty little-endian i32.
//! The first 28 are the plugin's parameters:
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Master volume | 0 to 65536 | 32768 |
//! | 1 Limiter | 0 or 1 | 0 |
//! | 2 Filter type | 0 IIR, 1 FIR | 0 |
//! | 3, 11, 20 High, mid and low band state | 0 active, 1 muted, 2 bypassed | 0 |
//! | 4 H freq, 12 M freq H | 20 to 20000 | 17000 |
//! | 13 M freq L, 21 L freq | 20 to 20000 | 6500 |
//! | 5, 14, 22 Output | 0 to 65536 | 21845 |
//! | 6, 15, 23 Threshold | 0 to 65536 | 45875, 48875 for mid |
//! | 7, 16, 24 Ratio | 0 to 65536 | 32768 |
//! | 8, 17, 25 Attack | 1 to 1000 | 100 |
//! | 9, 18, 26 Release | 100 to 10000 | 1000 |
//! | 10, 19, 27 Knee | 0 to 65536 | 52429 |
//!
//! The last two i32 are the window's: which meters show the input, and how
//! fast the graph scrolls.
//!
//! FL's plugin is a shell around `comp_eng_x64.dll`, which takes each
//! parameter as a number from 0 to 1. The engine is its own: none of FL's
//! limiter classes are in it. Each band filters the input, by 4th order
//! Butterworth filters or by 105 tap FIRs that are 52 frames late,
//! compresses what is left with one gain for both sides, and has an output
//! gain. The bands are added, the master volume is on their sum, and the
//! limiter is last.

use crate::{Effect, StateError, knob_value, read_state, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Multiband Compressor";

const LEVEL: (i32, i32) = (0, 65_536);
const HERTZ: (i32, i32) = (20, 20_000);
const ATTACK: (i32, i32) = (1, 1000);
const RELEASE: (i32, i32) = (100, 10_000);

/// The range of each parameter's control.
const RANGES: [(i32, i32); 28] = [
    LEVEL,
    (0, 1),
    (0, 1),
    (0, 2),
    HERTZ,
    LEVEL,
    LEVEL,
    LEVEL,
    ATTACK,
    RELEASE,
    LEVEL,
    (0, 2),
    HERTZ,
    HERTZ,
    LEVEL,
    LEVEL,
    LEVEL,
    ATTACK,
    RELEASE,
    LEVEL,
    (0, 2),
    HERTZ,
    LEVEL,
    LEVEL,
    LEVEL,
    ATTACK,
    RELEASE,
    LEVEL,
];

const DEFAULTS: [i32; 28] = [
    32_768, 0, 0, 0, 17_000, 21_845, 45_875, 32_768, 100, 1000, 52_429, 0, 17_000, 6500, 21_845,
    48_875, 32_768, 100, 1000, 52_429, 0, 6500, 21_845, 45_875, 32_768, 100, 1000, 52_429,
];

/// How long the state is, and where its parameters end.
const LEN: usize = 124;
const PARAMS_END: usize = 116;

/// The FIR filters' length and their middle tap, which is how late they
/// are.
const TAPS: usize = 105;
const MIDDLE: usize = 52;

/// The engine's pi and two pi are the nearest floats to them.
use std::f32::consts::{PI, TAU};

/// What counts as nothing, in the input and in what the filters hold.
const QUIET: f32 = 1e-7;

/// The first parameter of each band: high, mid and low.
const HIGH: usize = 3;
const MID: usize = 11;
const LOW: usize = 20;

/// What the engine makes of a number with no exponent: 0.
fn tiny(value: f32) -> f32 {
    if value.to_bits() & 0x7f80_0000 == 0 {
        0.0
    } else {
        value
    }
}

/// A frequency wheel's 0 to 1 in Hz: `19981^sqrt(p) + 19`.
fn hertz(p: f32) -> f32 {
    19_981f32.powf(p.sqrt()) + 19.0
}

/// One pole low pass, the smoothing of a level.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OnePole {
    before: f32,
    out: f32,
    /// What it takes of two inputs and what it keeps of itself.
    take: f32,
    keep: f32,
}

impl OnePole {
    fn new() -> Self {
        OnePole {
            before: 0.0,
            out: 0.0,
            take: 0.0,
            keep: 0.0,
        }
    }

    /// For a corner of `hz` at `rate`.
    fn tune(&mut self, hz: f32, rate: f32) {
        let span = rate * TAU;
        let period = 1.0 / rate;
        let turn = hz * TAU * period * 0.5;
        let warped = (f64::from(turn).tan() * f64::from(2.0 / period)) as f32;
        let warped = warped * PI;
        let sum = warped + span;
        self.take = warped / sum;
        self.keep = (span - warped) / sum;
    }

    fn step(&mut self, x: f32) -> f32 {
        let x = tiny(x);
        let out = tiny((x + self.before) * self.take + self.keep * self.out);
        self.before = x;
        self.out = out;
        out
    }

    fn reset(&mut self) {
        self.before = 0.0;
        self.out = 0.0;
    }
}

/// The engine's envelope: a peak that falls from where it last was by a
/// curve, a low pass on it, and the gain for what is over the threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Env {
    rate: f32,
    /// The threshold, linear, and how much of what is over it comes off:
    /// 0 is 1:1 and 1 holds the level at the threshold.
    threshold: f32,
    ratio: f32,
    /// The time the peak falls to 1/e in, in seconds.
    release: f32,
    /// Where under the threshold the knee starts, as a part of it.
    knee: f32,
    /// The knee: where it starts and ends, and its parabola.
    low: f32,
    high: f32,
    curve: [f32; 3],
    /// The last peak, the frames since, and the level now.
    peak: f32,
    since: u32,
    level: f32,
    smooth: OnePole,
}

impl Env {
    fn new() -> Self {
        Env {
            rate: 44_100.0,
            threshold: 1.0,
            ratio: 0.0,
            release: 1.0,
            knee: 1.0,
            low: 0.0,
            high: 0.0,
            curve: [0.0; 3],
            peak: 0.0,
            since: 0,
            level: 0.0,
            smooth: OnePole::new(),
        }
    }

    /// Take the five wheels of a compressor, each from 0 to 1.
    fn set(&mut self, [threshold, ratio, attack, release, knee]: [f32; 5], rate: f32) {
        self.rate = rate;
        self.threshold = 1000f32.powf(threshold) * 0.001;
        self.ratio = ratio;
        let hz = (1.0 / (f64::from(attack * 0.1) + 5.882_352_941_176_471e-5)) as f32;
        self.smooth.tune(hz, rate);
        self.release = release * 0.99 + 0.01;
        self.knee = knee * knee * 0.995 + 0.005;

        let t = self.threshold;
        self.low = t * self.knee - 0.005;
        self.high = (t + t) - self.low;
        let a = 1.0 / ((self.low - t) * 4.0);
        self.curve = [a, self.high * -2.0 * a, a * self.high * self.high + t];
    }

    /// The peak after one more level. `pace` is the limiter's, which is
    /// the same for both of its sides: it stretches the release, and a new
    /// peak moves it.
    fn follow(&mut self, x: f32, pace: Option<&mut f32>) -> f32 {
        let mut stretch = 1.0;
        if let Some(pace) = pace {
            if x > self.level {
                if *pace < 1.0 {
                    *pace = 1.0;
                } else {
                    *pace = (1.0 - (x - 1.0) * 0.4).max(0.01);
                }
            }
            stretch = *pace;
        }
        if x > self.level {
            self.level = x;
            self.peak = x;
            self.since = 0;
            return x;
        }
        let since = self.since as f32;
        if since <= self.rate * 10.0 {
            let fallen = -(since / self.rate) / (stretch * self.release);
            self.level = std::f32::consts::E.powf(fallen) * self.peak;
            self.since += 1;
        }
        self.level
    }

    /// How far a smoothed level is over the threshold, through the knee.
    fn over(&self, s: f32) -> f32 {
        if s < self.low {
            0.0
        } else if s <= self.high {
            let [a, b, c] = self.curve;
            s - (s * a * s + s * b + c)
        } else {
            s - self.threshold
        }
    }

    /// The gain for one more level.
    fn gain(&mut self, x: f32, pace: Option<&mut f32>) -> f32 {
        let peak = self.follow(x, pace);
        let smoothed = self.smooth.step(peak);
        let over = self.over(smoothed);
        self.threshold / (over * self.ratio + self.threshold)
    }

    fn reset(&mut self) {
        self.smooth.reset();
        self.peak = 0.0;
        self.since = 0;
        self.level = 0.0;
    }
}

/// A 4th order Butterworth filter: two biquads, with what both sides hold.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Butter {
    /// Each stage on its input, the input before and the one before that,
    /// and on its two outputs before.
    stages: [[f32; 5]; 2],
    /// For each side and stage: the two inputs and the two outputs before.
    held: [[[f32; 4]; 2]; 2],
}

impl Butter {
    fn new() -> Self {
        Butter {
            stages: [[0.0; 5]; 2],
            held: [[[0.0; 4]; 2]; 2],
        }
    }

    /// A low pass or a high pass at `hz`.
    fn tune(&mut self, hz: f32, rate: f32, low_pass: bool) {
        let turn = hz * TAU / rate;
        let (sin, cos) = (turn.sin(), turn.cos());
        let (middle, edge) = if low_pass {
            (1.0 - cos, (1.0 - cos) * 0.5)
        } else {
            (-(cos + 1.0), (cos + 1.0) * 0.5)
        };
        // Twice the Q of each stage.
        for (stage, width) in self.stages.iter_mut().zip([1.082, 2.614]) {
            let alpha = sin / width;
            let scale = alpha + 1.0;
            *stage = [
                edge / scale,
                middle / scale,
                edge / scale,
                cos * -2.0 / scale,
                (1.0 - alpha) / scale,
            ];
        }
    }

    /// One sample of one side.
    fn step(&mut self, side: usize, x: f32) -> f32 {
        let mut x = x;
        for (stage, held) in self.stages.iter().zip(&mut self.held[side]) {
            let [b0, b1, b2, a1, a2] = *stage;
            let input = tiny(x);
            let [x1, x2, y1, y2] = *held;
            let out = tiny(((x1 * b1 + input * b0 + x2 * b2) - y1 * a1) - y2 * a2);
            *held = [input, x1, out, y1];
            x = out;
        }
        x
    }

    fn reset(&mut self) {
        self.held = [[[0.0; 4]; 2]; 2];
    }

    fn rings(&self) -> bool {
        let held = self.held.iter().flatten().flatten();
        held.into_iter().any(|v| *v > QUIET || *v < -QUIET)
    }
}

/// The engine's Kaiser window: `I0(5.10226 * sqrt(1 - (n / 52)^2))` over
/// `I0(5.10226)`, with its own series for `I0`.
fn window() -> [f32; TAPS] {
    fn bessel(x: f32) -> f32 {
        let half = x * 0.5;
        let mut sum = 0.0;
        for m in 1..=15u32 {
            // The factorial is multiplied from its top down.
            let factorial = (1..=m).rev().fold(1f32, |product, k| product * k as f32);
            sum += (half.powf(m as f32) / factorial).powf(2.0);
        }
        sum + 1.0
    }
    let mut window = [0.0; TAPS];
    for k in 0..=MIDDLE {
        let n = (MIDDLE - k) as f32;
        let inside = (1.0 - ((n + n) / 104.0).powf(2.0)).max(0.0);
        let value = bessel(inside.sqrt() * 5.102_26) / bessel(5.102_26);
        window[k] = value;
        window[TAPS - 1 - k] = value;
    }
    window
}

/// A point of a FIR's response: a frequency in Hz and the gain there.
type Point = (f32, f32);

/// Add to `taps` the response that is `gain` from 0 Hz up to `hz`.
fn flat_below(taps: &mut [f32; TAPS], (hz, gain): Point, rate: f32) {
    let period = 1.0 / rate;
    let turn = hz * TAU * period;
    for (k, tap) in taps.iter_mut().enumerate() {
        let mut value = (gain + gain) * hz * period;
        if k != MIDDLE {
            let angle = (k as i32 - MIDDLE as i32) as f32 * turn;
            value *= angle.sin() / angle;
        }
        *tap += value;
    }
}

/// Add to `taps` the response that is `gain` from `hz` up.
fn flat_above(taps: &mut [f32; TAPS], (hz, gain): Point, rate: f32) {
    let period = 1.0 / rate;
    let turn = hz * TAU * period;
    for (k, tap) in taps.iter_mut().enumerate() {
        let part = (hz + hz) * period;
        let value = if k == MIDDLE {
            1.0 - part
        } else {
            let n = (k as i32 - MIDDLE as i32) as f32;
            let (all, below) = (n * PI, turn * n);
            all.sin() / all - below.sin() / below * part
        };
        *tap += gain * value;
    }
}

/// The response from 0 Hz up to `hz` of a gain that is the line
/// `offset + slope * w` over the angular frequency `w`.
fn line_below(offset: f32, slope: f32, hz: f32, rate: f32) -> [f32; TAPS] {
    let w = hz * TAU;
    let span = rate * TAU;
    let period = 1.0 / rate;
    let turn = w * period;
    let flat = (offset + offset) * w / span;
    let mut taps = [0.0; TAPS];
    for (k, tap) in taps.iter_mut().enumerate() {
        *tap = if k == MIDDLE {
            w * slope * w / span + flat
        } else {
            let n = (k as i32 - MIDDLE as i32) as f32;
            let twice = slope + slope;
            let angle = n * turn;
            let level = angle.sin() / angle * (twice * w * w / span + flat);
            level - (1.0 - angle.cos()) * (twice / (span * period * period * n * n))
        };
    }
    taps
}

/// Add to `taps` the response that goes in a straight line from one point
/// to the other. Two points at one frequency add nothing: FL divides by
/// their distance.
fn ramp(taps: &mut [f32; TAPS], a: Point, b: Point, rate: f32) {
    let (low, high) = if b.0 <= a.0 { (b, a) } else { (a, b) };
    let apart = low.0 * TAU - high.0 * TAU;
    if apart == 0.0 {
        return;
    }
    let slope = (low.1 - high.1) / apart;
    let offset = low.1 - (low.1 - high.1) * low.0 * TAU / apart;
    let under = line_below(offset, slope, low.0, rate);
    let whole = line_below(offset, slope, high.0, rate);
    for ((tap, whole), under) in taps.iter_mut().zip(whole).zip(under) {
        *tap += whole - under;
    }
}

/// Which of the three bands a filter is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Low,
    Mid,
    High,
}

/// A 105 tap FIR, with what both sides hold.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fir {
    taps: [f32; TAPS],
    held: [[f32; TAPS]; 2],
}

impl Fir {
    fn new() -> Self {
        Fir {
            taps: [0.0; TAPS],
            held: [[0.0; TAPS]; 2],
        }
    }

    /// Design the band's filter: full from 1.17 times its lower corner to
    /// 0.83 times its upper one, and a straight line down to nothing from
    /// there to 0.83 and 1.17 times them.
    fn tune(&mut self, kind: Kind, lower: f32, upper: f32, rate: f32, window: &[f32; TAPS]) {
        let nyquist = rate * 0.5;
        let cap = |hz: f32| if nyquist < hz { nyquist - 1.0 } else { hz };
        let mut taps = [0.0; TAPS];
        match kind {
            Kind::Low => {
                let full = (upper * 0.83, 1.0);
                flat_below(&mut taps, full, rate);
                ramp(&mut taps, full, (cap(upper * 1.17), 0.0), rate);
            }
            Kind::High => {
                let full = (cap(lower * 1.17), 1.0);
                ramp(&mut taps, (lower * 0.83, 0.0), full, rate);
                flat_above(&mut taps, full, rate);
            }
            Kind::Mid => {
                let (rise_end, rise_start) = (cap(lower * 1.17), lower * 0.83);
                let (fall_start, fall_end) = (upper * 0.83, cap(upper * 1.17));
                if rise_start <= fall_end {
                    let start = (rise_start, 0.0);
                    let last = if fall_start <= rise_end {
                        // The two lines cross before either is full.
                        let rise = rise_end - rise_start;
                        let fall = fall_start - fall_end;
                        let hz = (rise_start / rise - fall_end / fall) / (1.0 / rise - 1.0 / fall);
                        let top = (hz, (hz - rise_start) * (1.0 / rise));
                        ramp(&mut taps, start, top, rate);
                        top
                    } else {
                        ramp(&mut taps, start, (rise_end, 1.0), rate);
                        ramp(&mut taps, (rise_end, 1.0), (fall_start, 1.0), rate);
                        (fall_start, 1.0)
                    };
                    ramp(&mut taps, last, (fall_end, 0.0), rate);
                }
            }
        }
        for (tap, window) in taps.iter_mut().zip(window) {
            *tap = tiny(window * *tap);
        }
        self.taps = taps;
    }

    /// One sample of one side.
    fn step(&mut self, side: usize, x: f32) -> f32 {
        let held = &mut self.held[side];
        let out = x * self.taps[0] + held[0];
        for k in 0..TAPS - 1 {
            held[k] = x * self.taps[k + 1] + held[k + 1];
        }
        out
    }

    fn reset(&mut self) {
        self.held = [[0.0; TAPS]; 2];
    }

    fn rings(&self) -> bool {
        let held = self.held.iter().flatten();
        held.into_iter().any(|v| *v > QUIET || *v < -QUIET)
    }
}

/// One band: its filters of both types, its compressor and its gains.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    kind: Kind,
    /// The IIR type's filters in the order they run: one, or for the mid
    /// band the low pass and then the high pass.
    iir: [Butter; 2],
    fir: Fir,
    env: Env,
    /// Whether the band is compressed, which only an active one is.
    compress: bool,
    /// 0 for a muted band and 1 for another.
    mix: f32,
    /// The output wheel's gain.
    out: f32,
}

impl Band {
    fn new(kind: Kind) -> Self {
        Band {
            kind,
            iir: [Butter::new(); 2],
            fir: Fir::new(),
            env: Env::new(),
            compress: true,
            mix: 1.0,
            out: 1.0,
        }
    }

    /// One frame through the filter of the type in use, the compressor
    /// and the output gain.
    fn frame(&mut self, left: f32, right: f32, fir: bool) -> (f32, f32) {
        let (mut left, mut right) = (left, right);
        if fir {
            left = self.fir.step(0, left);
            right = self.fir.step(1, right);
        } else {
            let stages = if self.kind == Kind::Mid { 2 } else { 1 };
            for filter in &mut self.iir[..stages] {
                left = filter.step(0, left);
                right = filter.step(1, right);
            }
        }
        if self.compress {
            let gain = self.env.gain(left.abs().max(right.abs()), None);
            left *= gain;
            right *= gain;
        } else {
            self.env.reset();
        }
        (left * self.out, right * self.out)
    }

    fn reset(&mut self) {
        for filter in &mut self.iir {
            filter.reset();
        }
        self.fir.reset();
        self.env.reset();
    }

    fn rings(&self) -> bool {
        self.iir.iter().any(Butter::rings) || self.fir.rings()
    }
}

/// The limiter: a compressor for each side with a threshold of -1.2 dB,
/// and a soft clip of the last 0.2 % under full scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Limiter {
    sides: [Env; 2],
    /// What stretches both sides' release. FL has one of these for every
    /// instance of the plugin in the program.
    pace: f32,
}

impl Limiter {
    /// The engine's own settings, each a wheel from 0 to 1: threshold,
    /// ratio, attack, release and knee.
    const WHEELS: [f32; 5] = [0.98, 0.75, 3e-5, 0.005, 0.9];
    /// Where the clip starts, how wide its curve's input and output are,
    /// and the gain after it.
    const CLIP: f32 = 0.998;
    const CLIP_IN: f32 = 0.002_999_961_4;
    const CLIP_OUT: f32 = 0.001_999_974_3;
    const AFTER: f32 = 0.999_99;

    fn new() -> Self {
        Limiter {
            sides: [Env::new(); 2],
            pace: 1.0,
        }
    }

    fn tune(&mut self, rate: f32) {
        for side in &mut self.sides {
            side.set(Self::WHEELS, rate);
        }
    }

    fn clip(sample: f32) -> f32 {
        let size = sample.abs();
        if size < Self::CLIP {
            return sample;
        }
        let t = (size - Self::CLIP) / Self::CLIP_IN;
        let bent = if t.abs() >= 1.0 {
            if t > 0.0 { 1.0 } else { -1.0 }
        } else {
            (1.5 - t * 0.5 * t) * t
        };
        let out = bent * Self::CLIP_OUT + Self::CLIP;
        if sample <= 0.0 { -out } else { out }
    }

    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        let left = self.sides[0].gain(left.abs(), Some(&mut self.pace)) * left;
        let right = self.sides[1].gain(right.abs(), Some(&mut self.pace)) * right;
        (
            Self::clip(left) * Self::AFTER,
            Self::clip(right) * Self::AFTER,
        )
    }

    fn reset(&mut self) {
        for side in &mut self.sides {
            side.reset();
        }
    }
}

/// Fruity Multiband Compressor. A new one has FL's defaults: IIR filters
/// that cross at 300 Hz and 9.2 kHz, three compressors at -18 and -15 dB, and
/// no limiter.
#[derive(Debug, Clone, PartialEq)]
pub struct MultibandCompressor {
    params: [i32; 28],
    rate: f32,
    /// The bands in the order their parameters are in: high, mid, low.
    bands: [Band; 3],
    limiter: Limiter,
    /// Whether the limiter is on, and whether the filters are the FIRs.
    limit: bool,
    fir: bool,
    /// The master volume's gain.
    master: f32,
    window: [f32; TAPS],
}

impl Default for MultibandCompressor {
    fn default() -> Self {
        let mut plugin = MultibandCompressor {
            params: DEFAULTS,
            rate: 44_100.0,
            bands: [
                Band::new(Kind::High),
                Band::new(Kind::Mid),
                Band::new(Kind::Low),
            ],
            limiter: Limiter::new(),
            limit: false,
            fir: false,
            master: 1.0,
            window: window(),
        };
        plugin.set(DEFAULTS);
        plugin
    }
}

impl MultibandCompressor {
    /// A parameter as the engine takes it, from 0 to 1.
    fn normal(&self, index: usize) -> f32 {
        let (min, max) = RANGES[index];
        (self.params[index] - min) as f32 / (max - min) as f32
    }

    /// Take parameters that are inside their ranges, and work out what
    /// they and the rate make of every band. A change of filter type
    /// empties everything, as in FL.
    fn set(&mut self, params: [i32; 28]) {
        self.params = params;
        let rate = self.rate;
        self.master = 4f32.powf(self.normal(0)) - 1.0;
        self.limit = self.normal(1) > 0.5;
        let fir = 1.0 - self.normal(2) < 0.5;
        if fir != self.fir {
            self.fir = fir;
            self.flush();
        }
        self.limiter.tune(rate);

        let high_from = hertz(self.normal(HIGH + 1));
        let mid_to = hertz(self.normal(MID + 1));
        let mid_from = hertz(self.normal(MID + 2));
        let low_to = hertz(self.normal(LOW + 1));
        let window = self.window;
        for (at, first) in [HIGH, MID, LOW].into_iter().enumerate() {
            // The band's state wheel, and where its five compressor wheels
            // start: the mid band has a second frequency before them.
            let state = self.normal(first);
            let wheels = first + if first == MID { 3 } else { 2 };
            let normal: [f32; 6] = std::array::from_fn(|k| self.normal(wheels + k));
            let band = &mut self.bands[at];
            band.compress = f64::from(state) < 0.33;
            band.mix = if (0.33..=0.66).contains(&state) {
                0.0
            } else {
                1.0
            };
            band.out = 8f32.powf(normal[0]) - 1.0;
            band.env.set(
                [normal[1], normal[2], normal[3], normal[4], normal[5]],
                rate,
            );
            match band.kind {
                Kind::High => {
                    band.iir[0].tune(high_from, rate, false);
                    band.fir.tune(Kind::High, high_from, 0.0, rate, &window);
                }
                Kind::Mid => {
                    band.iir[0].tune(mid_to, rate, true);
                    band.iir[1].tune(mid_from, rate, false);
                    band.fir.tune(Kind::Mid, mid_from, mid_to, rate, &window);
                }
                Kind::Low => {
                    band.iir[0].tune(low_to, rate, true);
                    band.fir.tune(Kind::Low, 0.0, low_to, rate, &window);
                }
            }
        }
    }

    /// What FL's flush does: every filter, envelope and the limiter
    /// start from nothing.
    fn flush(&mut self) {
        for band in &mut self.bands {
            band.reset();
        }
        self.limiter.reset();
    }

    /// Whether a block has to go through the engine: it has a sample
    /// that is not nothing, or a filter still rings.
    fn awake(&self, left: &[f32], right: &[f32]) -> bool {
        let loud = |side: &[f32]| side.iter().any(|v| *v > QUIET || *v < -QUIET);
        loud(left) || loud(right) || self.bands.iter().any(Band::rings)
    }
}

impl Effect for MultibandCompressor {
    /// FL reads a version of 0 or under and leaves the plugin alone for
    /// another. The last two i32 are its window's and are not kept.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version > 0 {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        self.set(read_state(&bytes[4..PARAMS_END], &RANGES)?);
        Ok(())
    }

    /// Every filter and time is worked out for the rate, and the FIRs
    /// start from nothing. A rate that is not a number above 0 is not
    /// taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.rate = hz;
            for band in &mut self.bands {
                band.fir.reset();
            }
            self.set(self.params);
        }
    }

    /// A block of nothing, once no filter rings any more, passes as it is
    /// and empties the engine.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.awake(left, right) {
            self.flush();
            return;
        }
        let fir = self.fir;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let [high, mid, low] = &mut self.bands;
            let (high_mix, mid_mix, low_mix) = (high.mix, mid.mix, low.mix);
            let high = high.frame(*l, *r, fir);
            let mid = mid.frame(*l, *r, fir);
            let low = low.frame(*l, *r, fir);
            let mut out = (
                (mid.0 * mid_mix + low.0 * low_mix + high.0 * high_mix) * self.master,
                (mid.1 * mid_mix + low.1 * low_mix + high.1 * high_mix) * self.master,
            );
            if self.limit {
                out = self.limiter.frame(out.0, out.1);
            }
            (*l, *r) = out;
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

    /// The FIRs' 52 frames, which FL tells its host of. The IIR filters
    /// are on time.
    fn latency(&self) -> usize {
        if self.fir { MIDDLE } else { 0 }
    }

    fn max_latency(&self) -> usize {
        MIDDLE
    }

    /// What FL's flush does, and the limiter's pace back at 1.
    fn reset(&mut self) {
        self.flush();
        self.limiter.pace = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bits, partial, presets, sine, state};

    fn saved(params: [i32; 28]) -> Vec<u8> {
        let mut bytes = state(&[0]);
        bytes.extend(state(&params));
        bytes.extend(state(&[0, 50]));
        bytes
    }

    fn with(moves: &[(usize, i32)]) -> MultibandCompressor {
        let mut params = DEFAULTS;
        for (index, value) in moves {
            params[*index] = *value;
        }
        let mut plugin = MultibandCompressor::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_state(&saved(params)).unwrap();
        plugin
    }

    /// Both sides through the plugin, in blocks of 512.
    fn run(plugin: &mut MultibandCompressor, input: &[f32]) -> Vec<f32> {
        let mut left = input.to_vec();
        let mut right = input.to_vec();
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            plugin.process(l, r);
        }
        assert_eq!(bits(&left), bits(&right));
        left
    }

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |peak, v| peak.max(v.abs()))
    }

    /// Every band bypassed: no compressor, only the filters.
    const BYPASSED: [(usize, i32); 3] = [(HIGH, 2), (MID, 2), (LOW, 2)];

    #[test]
    fn the_defaults_are_the_wheels_of_fl_s_window() {
        let plugin = MultibandCompressor::default();
        let params: Vec<f32> = (0..28).map(|index| plugin.param(index)).collect();
        assert_eq!(params, DEFAULTS.map(|v| v as f32));
        assert_eq!(plugin.param(28), 0.0);
        assert_eq!(plugin.param_count(), 28);
        // Unity on the master and on every band, no limiter, IIR.
        assert_eq!(plugin.master, 1.0);
        for band in &plugin.bands {
            assert!((band.out - 1.0).abs() < 1e-4, "{}", band.out);
            assert!(band.compress && band.mix == 1.0);
        }
        assert!(!plugin.limit && !plugin.fir);
        assert_eq!((plugin.latency(), plugin.max_latency()), (0, 52));
    }

    #[test]
    fn the_wheels_are_the_doc_s_curves() {
        // The frequency wheels end on 20 Hz and 20 kHz, and the defaults
        // of 6500 and 17000 are 300 Hz and 9.2 kHz.
        assert_eq!(hertz(0.0), 20.0);
        assert!((hertz(1.0) - 20_000.0).abs() < 0.01);
        let p = (6500.0f32 - 20.0) / 19_980.0;
        assert!((hertz(p) - 300.3).abs() < 0.5, "{}", hertz(p));
        let p = (17_000.0f32 - 20.0) / 19_980.0;
        assert!((hertz(p) - 9237.0).abs() < 5.0, "{}", hertz(p));

        // The master is 4^p - 1 and a band's output 8^p - 1.
        let plugin = with(&[(0, 65_536), (HIGH + 2, 65_536), (LOW + 2, 0)]);
        assert_eq!((plugin.master, plugin.bands[0].out), (3.0, 7.0));
        assert_eq!(plugin.bands[2].out, 0.0);

        // The threshold is 60 dB of wheel under full scale, the release
        // 10 ms to 1 s, and the attack the corner of a low pass.
        let plugin = with(&[(HIGH + 3, 32_768), (HIGH + 5, 1), (HIGH + 6, 10_000)]);
        let env = plugin.bands[0].env;
        assert!((db(f64::from(env.threshold)) + 30.0).abs() < 1e-3);
        assert_eq!(env.release, 1.0);
        let plugin = with(&[(HIGH + 6, 100), (HIGH + 4, 0), (HIGH + 7, 65_536)]);
        let env = plugin.bands[0].env;
        assert_eq!((env.release, env.ratio, env.knee), (0.01, 0.0, 1.0));

        // A state wheel: active, muted, bypassed.
        let plugin = with(&[(HIGH, 1), (MID, 2)]);
        let flags = plugin.bands.map(|band| (band.compress, band.mix));
        assert_eq!(flags, [(false, 0.0), (false, 1.0), (true, 1.0)]);
        // The filter type and the limiter.
        let plugin = with(&[(1, 1), (2, 1)]);
        assert!(plugin.limit && plugin.fir);
        assert_eq!(plugin.latency(), 52);
    }

    #[test]
    fn the_knee_is_a_parabola_between_two_lines() {
        let mut env = Env::new();
        // -20 dB, a wheel half way, the knee wheel at 0.8.
        env.set([2.0 / 3.0, 0.5, 0.0, 0.5, 0.8], 48_000.0);
        let t = env.threshold;
        assert!((t - 0.1).abs() < 1e-6);
        assert!((env.knee - 0.6418).abs() < 1e-6);
        assert!((env.low - (t * env.knee - 0.005)).abs() < 1e-7);
        assert!((env.high + env.low - 2.0 * t).abs() < 1e-7);
        // Nothing under the knee, what is over the threshold above it,
        // and no step where the three meet.
        assert_eq!(env.over(env.low - 1e-4), 0.0);
        assert!(env.over(env.low).abs() < 1e-6);
        assert!((env.over(env.high) - (env.high - t)).abs() < 1e-6);
        assert!((env.over(0.5) - 0.4).abs() < 1e-7);
        // In the middle of the knee, a quarter of its half width.
        assert!((env.over(t) - (t - env.low) * 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_compressor_under_its_threshold_is_unity() {
        let mut env = Env::new();
        env.set([0.7, 1.0, 0.1, 0.5, 0.8], 48_000.0);
        for n in 0..4800 {
            let level = env.low * 0.99 * (n as f32 * 0.01).sin().abs();
            assert_eq!(env.gain(level, None), 1.0);
        }
        // The plugin with its bands active does to a quiet tone what it
        // does with them bypassed: the filters and nothing more.
        let quiet: Vec<f32> = sine(0.01, 9600).iter().map(|v| v * 0.02).collect();
        for filter in [0, 1] {
            let active = run(&mut with(&[(2, filter)]), &quiet);
            let mut moves = BYPASSED.to_vec();
            moves.push((2, filter));
            let bypassed = run(&mut with(&moves), &quiet);
            assert_eq!(bits(&active), bits(&bypassed), "{filter}");
        }
    }

    #[test]
    fn a_compressor_over_its_threshold_settles_on_the_doc_s_gain() {
        // -20 dB, hard knee. A level that stands still at 0.5.
        for (ratio, wanted) in [(0.0, 1.0), (0.5, 0.1 / (0.4 * 0.5 + 0.1)), (1.0, 0.2)] {
            let mut env = Env::new();
            env.set([2.0 / 3.0, ratio, 0.01, 0.5, 1.0], 48_000.0);
            let mut gain = 0.0;
            for _ in 0..4800 {
                gain = env.gain(0.5, None);
            }
            assert!((gain - wanted).abs() < 1e-5, "{ratio}: {gain}");
        }
    }

    #[test]
    fn the_release_falls_from_the_last_peak_by_e_in_its_time() {
        let mut env = Env::new();
        // 0.505 s of release.
        env.set([0.5, 0.5, 0.0, 0.5, 1.0], 48_000.0);
        assert_eq!(env.follow(0.8, None), 0.8);
        // The frame after the peak still has it.
        assert_eq!(env.follow(0.0, None), 0.8);
        for _ in 0..24_240 - 1 {
            env.follow(0.0, None);
        }
        let fallen = f64::from(env.follow(0.0, None));
        assert!(
            (fallen - 0.8 / std::f64::consts::E).abs() < 1e-4,
            "{fallen}"
        );
        // A higher level takes over at once, a lower one does not.
        assert!(env.follow(0.1, None) < 0.3);
        assert_eq!(env.follow(0.9, None), 0.9);
    }

    #[test]
    fn the_iir_bands_are_4th_order_butterworth() {
        // 3 dB down at the corner, 24 dB an octave beyond, and what the
        // two halves of one corner add up to is 3 dB up there.
        let rate = 48_000.0;
        let gain = |low_pass: bool, hz: f64| -> f64 {
            let mut filter = Butter::new();
            filter.tune(1000.0, rate, low_pass);
            let tone = sine(hz / 48_000.0, 48_000);
            let out: Vec<f32> = tone.iter().map(|x| filter.step(0, *x)).collect();
            db(partial(&out[24_000..], hz / 48_000.0))
        };
        assert!((gain(true, 1000.0) + 3.01).abs() < 0.05);
        assert!((gain(false, 1000.0) + 3.01).abs() < 0.05);
        assert!((gain(true, 100.0)).abs() < 0.01);
        assert!(
            (gain(true, 4000.0) + 48.2).abs() < 1.0,
            "{}",
            gain(true, 4000.0)
        );
        assert!((gain(false, 250.0) + 48.2).abs() < 1.0);

        let mut low = Butter::new();
        let mut high = Butter::new();
        low.tune(1000.0, rate, true);
        high.tune(1000.0, rate, false);
        let tone = sine(1000.0 / 48_000.0, 48_000);
        let both = tone.iter().map(|x| low.step(0, *x) + high.step(0, *x));
        let both: Vec<f32> = both.collect();
        let sum = db(partial(&both[24_000..], 1000.0 / 48_000.0));
        assert!((sum - 3.01).abs() < 0.05, "{sum}");
    }

    #[test]
    fn the_fir_bands_sum_flat_and_52_frames_late() {
        // The corners of the defaults meet, so the three responses add up
        // to 1 at every frequency: the input comes back as it went in.
        let mut moves = BYPASSED.to_vec();
        moves.push((2, 1));
        let mut plugin = with(&moves);
        let mut click = vec![0.0; 512];
        click[10] = 1.0;
        let out = run(&mut plugin, &click);
        for (n, value) in out.iter().enumerate() {
            let wanted = if n == 62 { 1.0 } else { 0.0 };
            assert!((value - wanted).abs() < 1e-4, "{n}: {value}");
        }
        // And so for a tone anywhere: within 0.1 dB.
        for hz in [50.0, 300.0, 1000.0, 9237.0, 15_000.0] {
            let mut plugin = with(&moves);
            let turns = hz / 48_000.0;
            let out = run(&mut plugin, &sine(turns, 48_000));
            let gain = db(partial(&out[24_000..], turns));
            assert!(gain.abs() < 0.1, "{hz}: {gain}");
        }
        // One band alone, the low one up to 7 kHz: full inside it, and
        // nothing well outside.
        let alone = [(2, 1), (HIGH, 1), (MID, 1), (LOW, 2), (LOW + 1, 16_000)];
        let mut plugin = with(&alone);
        let tone = |plugin: &mut MultibandCompressor, hz: f64| {
            let out = run(plugin, &sine(hz / 48_000.0, 48_000));
            db(partial(&out[24_000..], hz / 48_000.0))
        };
        assert!(tone(&mut plugin, 1000.0).abs() < 0.1);
        assert!(tone(&mut plugin, 15_000.0) < -40.0);
    }

    #[test]
    fn the_fir_window_is_a_kaiser_window() {
        let window = window();
        assert_eq!(window[MIDDLE], 1.0);
        assert_eq!(bits(&window[..MIDDLE]), {
            let mut back = window[MIDDLE + 1..].to_vec();
            back.reverse();
            bits(&back)
        });
        // The ends are one over I0(5.10226), which is 29.85.
        assert!(
            (f64::from(window[0]) - 1.0 / 29.849).abs() < 1e-4,
            "{}",
            window[0]
        );
        assert!(window.windows(2).take(MIDDLE).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_muted_band_is_gone_and_the_output_wheel_scales_it() {
        // The low band alone, IIR, and a tone well inside it.
        let tone: Vec<f32> = sine(100.0 / 48_000.0, 9600)
            .iter()
            .map(|v| v * 0.01)
            .collect();
        let alone = [(HIGH, 1), (MID, 1), (LOW, 2)];
        let plain = peak(&run(&mut with(&alone), &tone)[4800..]);
        assert!((plain - 0.01).abs() < 1e-4, "{plain}");
        let mut louder = alone.to_vec();
        louder.push((LOW + 2, 65_536));
        let loud = peak(&run(&mut with(&louder), &tone)[4800..]);
        assert!((loud / plain - 7.0).abs() < 1e-3);
        let none = [(HIGH, 1), (MID, 1), (LOW, 1)];
        assert_eq!(peak(&run(&mut with(&none), &tone)), 0.0);
    }

    #[test]
    fn the_limiter_keeps_the_output_under_full_scale() {
        let loud: Vec<f32> = sine(100.0 / 48_000.0, 48_000)
            .iter()
            .map(|v| v * 4.0)
            .collect();
        let bypassed = [(HIGH, 2), (MID, 2), (LOW, 2), (1, 1)];
        let out = run(&mut with(&bypassed), &loud);
        let top = peak(&out);
        assert!(top <= 1.0 && top > 0.9, "{top}");
        // Without it the tone is as loud as it came.
        let open = [(HIGH, 2), (MID, 2), (LOW, 2)];
        assert!(peak(&run(&mut with(&open), &loud)) > 3.9);
        // The clip bends the last 0.2 % and ends just under 1.
        assert_eq!(Limiter::clip(0.5), 0.5);
        assert_eq!(Limiter::clip(-0.99), -0.99);
        assert!((Limiter::clip(2.0) - 0.999_999_97).abs() < 1e-6);
        assert!((Limiter::clip(-2.0) + 0.999_999_97).abs() < 1e-6);
        let half = Limiter::clip(0.998 + 0.0015);
        assert!((half - (0.998 + 0.002 * (1.5 - 0.125) * 0.5)).abs() < 1e-6);
    }

    #[test]
    fn silence_passes_and_empties_the_engine() {
        let mut plugin = with(&[(2, 1)]);
        run(&mut plugin, &sine(0.01, 2048));
        assert!(plugin.bands[0].rings() || plugin.bands[2].rings());
        // The FIRs ring for 104 frames, and then a block of nothing is
        // not touched.
        run(&mut plugin, &[0.0; 512]);
        assert!(!plugin.bands.iter().any(Band::rings));
        let quiet = vec![5e-8; 512];
        assert_eq!(bits(&run(&mut plugin, &quiet)), bits(&quiet));
        assert_eq!(plugin.bands[2].env.level, 0.0);
    }

    #[test]
    fn a_new_filter_type_or_a_reset_starts_from_nothing() {
        let input: Vec<f32> = sine(0.013, 4096).iter().map(|v| v * 0.7).collect();
        let mut plugin = with(&[(1, 1)]);
        let first = run(&mut plugin, &input);
        plugin.reset();
        assert_eq!(bits(&run(&mut plugin, &input)), bits(&first));
        // To the FIRs and back.
        plugin.set_param(2, 1.0);
        assert_eq!(plugin.latency(), 52);
        run(&mut plugin, &input);
        plugin.set_param(2, 0.0);
        plugin.limiter.pace = 1.0;
        assert_eq!(bits(&run(&mut plugin, &input)), bits(&first));
        // Another wheel leaves what the filters hold.
        plugin.set_param(0, 20_000.0);
        assert!(plugin.bands[2].rings());
        plugin.set_param(28, 1.0);
    }

    #[test]
    fn a_state_of_another_version_length_or_range_is_turned_down() {
        let mut plugin = MultibandCompressor::default();
        let mut params = DEFAULTS;
        params[1] = 1;
        let good = saved(params);
        assert_eq!(plugin.set_state(&good), Ok(()));
        assert_eq!(plugin.param(1), 1.0);
        let mut newer = good.clone();
        newer[0] = 1;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 1 })
        );
        assert_eq!(
            plugin.set_state(&good[..120]),
            Err(StateError::Length {
                expected: 124,
                found: 120
            })
        );
        params[4] = 19;
        assert_eq!(
            plugin.set_state(&saved(params)),
            Err(StateError::Range {
                param: 4,
                value: 19
            })
        );
    }

    #[test]
    fn fl_s_presets_load_and_run() {
        let Some(presets) = presets(NAME) else {
            return;
        };
        assert_eq!(presets.len(), 31);
        let default = presets.iter().find(|p| p.file == "Default").unwrap();
        assert_eq!(default.state, saved(DEFAULTS));
        let (mut firs, mut limited) = (0, 0);
        for preset in &presets {
            let mut plugin = MultibandCompressor::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_state(&preset.state).unwrap();
            firs += usize::from(plugin.fir);
            limited += usize::from(plugin.limit);
            let out = run(&mut plugin, &sine(0.004, 9600));
            assert!(
                out.iter().all(|v| v.is_finite() && v.abs() < 30.0),
                "{}",
                preset.file
            );
        }
        assert!(firs > 0 && limited > 0, "{firs} {limited}");
    }
}
