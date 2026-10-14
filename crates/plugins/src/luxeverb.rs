//! Luxeverb: a sixteen-line feedback delay network with a pitch shifter
//! in its feedback, as `docs/plugins/luxeverb.md` has it.
//!
//! The plugin is a Delphi shell around `Shimmer_CreateInstance` in FL's
//! `Shared/ildsp_x64.dll`. The shell keeps the state and turns each of its
//! 49 controls into one float for the library, which does all the sound.
//! This file follows the same split: [`Luxeverb`] is the shell and
//! [`Engine`] the library.
//!
//! The state is a version (7, 8 or 9), the sidechain input, five bytes for
//! the editor (the first two are the tempo-sync buttons of the predelay and
//! of the feedback delay), the parameters as i32 (38 in version 7, 39 in 8,
//! 49 in 9) and, in version 9, two more sync buttons for the feedforward
//! delays. Older versions get the newer parameters filled in as FL's loader
//! fills them.

use crate::{Effect, StateError, knob_value};

pub const NAME: &str = "Luxeverb";

/// How many parameters the shell has.
const PARAMS: usize = 49;

/// The range of each control, from the editor's form.
const RANGES: [(i32, i32); PARAMS] = [
    (-600, 1500),  // 0 input wet gain
    (40, 20000),   // 1 input high cut
    (20, 5000),    // 2 input low cut
    (0, 1),        // 3 reverb on
    (250, 20000),  // 4 decay
    (0, 5000),     // 5 predelay
    (1, 768),      // 6 predelay, synced
    (0, 1000),     // 7 brightness
    (0, 100),      // 8 character
    (1, 100),      // 9 size
    (0, 1000),     // 10 modulation rate
    (0, 100),      // 11 modulation depth
    (0, 1000),     // 12 diffusion
    (0, 2),        // 13 freeze mode
    (0, 1),        // 14 high quality
    (0, 1),        // 15 feedback on
    (-120, 120),   // 16 feedback pitch
    (10, 5000),    // 17 feedback delay
    (1, 768),      // 18 feedback delay, synced
    (0, 1000),     // 19 feedback gain
    (20, 5000),    // 20 feedback high cut
    (20, 5000),    // 21 feedback low cut
    (0, 1000),     // 22 feedback reverb mix
    (-1000, 1000), // 23 envelope scale
    (-6000, 0),    // 24 envelope threshold
    (50, 1000),    // 25 envelope attack
    (50, 1000),    // 26 envelope decay
    (0, 100),      // 27 envelope offset
    (0, 2),        // 28 envelope mode
    (0, 1),        // 29 envelope sidechain
    (20, 2000),    // 30 envelope low cut
    (10, 1000),    // 31 envelope smoothing
    (-600, 1500),  // 32 dry level
    (-600, 1500),  // 33 wet level
    (40, 10000),   // 34 peak frequency
    (-1800, 1800), // 35 peak gain
    (25, 5000),    // 36 peak Q
    (0, 200),      // 37 stereo width
    (0, 1),        // 38 limiter
    (0, 1),        // 39 pitch type
    (0, 1000),     // 40 feedforward unpitched
    (-24, 24),     // 41 feedforward pitch 1
    (-24, 24),     // 42 feedforward pitch 2
    (0, 1000),     // 43 feedforward amp 1
    (0, 1000),     // 44 feedforward amp 2
    (10, 5000),    // 45 feedforward delay 1
    (1, 768),      // 46 feedforward delay 1, synced
    (0, 1000),     // 47 feedforward delay 2
    (1, 768),      // 48 feedforward delay 2, synced
];

/// What a version 7 or 8 state leaves the newer parameters at: the loader
/// sets the limiter (38, version 7 only) and these ten (39 to 48).
const FILL: [i32; 10] = [0, 0, 24, 20, 1719, 20, 1000, 192, 1000, 192];

/// FL's plugin database state, `LuxeVerb.fst`: a version 7 state, so the
/// last eleven are the loader's fill.
const DEFAULTS: [i32; PARAMS] = [
    721, 12991, 20, 1, 11625, 326, 192, 253, 40, 20, 47, 20, 403, 0, 1, 0, 120, 2988, 192, 0, 1719,
    720, 1000, 0, -4000, 50, 400, 0, 0, 0, 301, 10, 746, 746, 1570, 315, 865, 120, 1, 0, 0, 24, 20,
    1719, 20, 1000, 192, 1000, 192,
];

/// The first 2262 primes, all those under 20000: the library's table of
/// delay lengths at 0x1803dac50.
static PRIMES: [u16; 2262] = primes();

const fn primes() -> [u16; 2262] {
    let mut table = [0u16; 2262];
    let mut count = 0;
    let mut n = 2u32;
    while count < 2262 {
        let mut prime = true;
        let mut d = 2;
        while d * d <= n {
            if n.is_multiple_of(d) {
                prime = false;
                break;
            }
            d += 1;
        }
        if prime {
            table[count] = n as u16;
            count += 1;
        }
        n += 1;
    }
    table
}

/// The sixteen lines' lengths in seconds at prepare time: primes over
/// 44100.
const BASE_SECONDS: [f32; 16] = [
    0.019_886_62,
    0.029_546_484,
    0.043_242_63,
    0.046_553_288,
    0.061_473_92,
    0.060_294_785,
    0.079_977_326,
    0.086_235_83,
    0.099_705_21,
    0.099_161,
    0.112_403_624,
    0.123_560_086,
    0.130_181_4,
    0.140_975_06,
    0.153_310_66,
    0.158_662_13,
];

/// Where each line's length sits in the prime table, against the room
/// size (0x1803dac10).
const SIZE_TABLE: [f32; 16] = [
    155.0, 217.0, 296.0, 313.0, 398.0, 388.0, 494.0, 531.0, 601.0, 599.0, 664.0, 722.0, 755.0,
    809.0, 870.0, 900.0,
];

/// The diffusers' allpass lengths in seconds and their gains.
const DIFFUSER_SECONDS: [f32; 8] = [
    0.004_737_700_3,
    0.003_595_3,
    0.012_735,
    0.009_307_5,
    0.007_773_100_4,
    0.016_112_5,
    0.013_041_2,
    0.008_912_301,
];
const DIFFUSER_GAINS: [f32; 8] = [
    0.75,
    0.625,
    0.7,
    0.5,
    0.675,
    f32::from_bits(0x3f36_45a2),
    0.6,
    f32::from_bits(0x3f15_3f7d),
];

/// The longest predelay, in samples, at any rate.
const PREDELAY: usize = 172_000;

/// The crossover of the lines' damping shelves, Hz.
const CROSSOVER: f32 = 2124.7974;

/// The base the smoothers raise to `1 / (time * rate)`.
const SMOOTH_BASE: f32 = 0.37;

/// `x ^ (1 / k)` stretched over `lo..hi`: the shell's curve, `FUN_0072deb0`.
fn curve(x: f32, lo: f64, hi: f64, k: f64) -> f32 {
    (lo + (hi - lo) * ((f64::from(x)).ln() / k).exp()) as f32
}

/// `powf(10, db / 20)`.
fn decibels(db: f32) -> f32 {
    10f32.powf(db * 0.05)
}

/// MSVC's `fminf` and `fmaxf` as the library calls them.
fn min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b <= a { b } else { a }
}
fn max(a: f32, b: f32) -> f32 {
    if a.is_nan() || a < b { b } else { a }
}

/// A value that moves a share of the way to its target every sample.
#[derive(Debug, Clone, Copy)]
struct Smoother {
    current: f32,
    target: f32,
    time: f32,
    coef: f32,
}

impl Default for Smoother {
    fn default() -> Self {
        Smoother {
            current: 0.0,
            target: 0.0,
            time: 0.005,
            coef: f32::from_bits(0x3b93_6c00),
        }
    }
}

impl Smoother {
    fn rate(&mut self, rate: f32) {
        self.coef = 1.0 - SMOOTH_BASE.powf(1.0 / (rate * self.time));
    }

    fn tick(&mut self) -> f32 {
        self.current += (self.target - self.current) * self.coef;
        self.current
    }
}

/// A biquad in transposed direct form II, `[b0, b1, b2, a1, a2]`.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    c: [f32; 5],
    s: [f32; 2],
}

impl Biquad {
    fn run(&mut self, x: f32) -> f32 {
        let y = x * self.c[0] + self.s[0];
        self.s[0] = x * self.c[1] - y * self.c[3] + self.s[1];
        self.s[1] = x * self.c[2] - y * self.c[4];
        y
    }

    /// FL's high pass after the cookbook, `FUN_1801871b0`.
    fn high_pass(freq: f32, q: f32, rate: f32) -> [f32; 5] {
        let w = freq * std::f32::consts::TAU / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin * (0.5 / q);
        let norm = 1.0 / (alpha + 1.0);
        let b0 = (cos + 1.0) * 0.5 * norm;
        let b1 = -norm - cos / (alpha + 1.0);
        [b0, b1, b0, cos * -2.0 * norm, (1.0 - alpha) * norm]
    }
}

/// A first-order section `[b0, b1, a1]` and its state, one per side.
#[derive(Debug, Clone, Copy, Default)]
struct OnePole {
    c: [f32; 3],
    s: [f32; 2],
}

impl OnePole {
    fn run(&mut self, side: usize, x: f32) -> f32 {
        let y = self.c[0] * x + self.s[side];
        self.s[side] = self.c[1] * x - y * self.c[2];
        y
    }
}

/// The lines' four-point Hermite read, `FUN_180188d10`: `newer` is one
/// sample nearer the write than `x0`.
fn hermite(newer: f32, x0: f32, x1: f32, x2: f32, frac: f32) -> f32 {
    let d = x0 - x1;
    let c1 = (x1 - newer) * 0.5;
    let a = d + c1;
    let b = (x2 - x0) * 0.5 + a + d;
    ((b * frac - (b + a)) * frac + c1) * frac + x0
}

/// The four-point Lagrange read the predelay and the pitch shifter use:
/// `newer` is one sample nearer the write than `x0`, `x1` and `x2` older.
fn lagrange(newer: f32, x0: f32, x1: f32, x2: f32, frac: f32) -> f32 {
    const THIRD: f32 = f32::from_bits(0x3eaa_aa9f);
    const SIXTH: f32 = f32::from_bits(0x3e2a_aac1);
    let f2 = frac * frac;
    let x2 = x2 * SIXTH;
    (((x1 - (newer * THIRD + x0 * 0.5)) - x2) + ((x2 - newer * SIXTH) + (x0 - x1) * 0.5) * f2)
        * frac
        + x0
        + ((x1 * 0.5 + newer * 0.5) - x0) * f2
}

/// The window of a grain: `sin(i * pi / 1023)^2` in doubles.
const WINDOW: usize = 1024;

/// MSVC's `rand` step, which the shifters share through one static in the
/// library: the next 15-bit number.
fn rand(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
    (*seed >> 16) & 0x7fff
}

/// One grain of a pitch shifter.
#[derive(Debug, Clone, Copy, Default)]
struct Grain {
    on: bool,
    len: i32,
    left: i32,
    place: f32,
    step: f32,
}

/// The library's granular pitch shifter (the object at 0x190b70 and its two
/// feedforward siblings): a delay, then grains read from a buffer at the
/// pitch ratio under a squared-sine window, then a soft clip. Only the
/// mode the reverb sets (3) is built; its feedback gain stays 0, so the
/// three biquads on the feedback path never reach the output and are left
/// out.
#[derive(Debug, Clone, Default)]
struct Shifter {
    rate: f32,
    stereo: bool,
    /// The grain buffers and where the next sample goes.
    buf: [Vec<f32>; 2],
    write: usize,
    ratio: f32,
    /// Grains a grain length: 0x2c.
    density: f32,
    /// How far the next grain's start and place wander: 0x30 and 0x34.
    time_jitter: f32,
    place_jitter: f32,
    mix: f32,
    grain_len: i32,
    countdown: i32,
    /// `rate * 2000 / 44100`, what the jitters scale.
    jitter_scale: f32,
    grains: [Grain; 20],
    window: Vec<f32>,
    /// The two-tap filter ahead of the buffer.
    prev: [f32; 2],
    /// The delay ahead of the shifter.
    line: [Vec<f32>; 2],
    line_write: usize,
    delay: f32,
    delay_target: f32,
    delay_coef: f32,
    delay_min: f32,
    delay_max: f32,
    tempo: f32,
    clip: bool,
    clip_gain: f32,
    clip_scale: f32,
}

impl Shifter {
    /// `FUN_1801821f0`: buffers and defaults for a rate.
    fn prepare(&mut self, rate: f32, stereo: bool) {
        self.rate = rate;
        self.stereo = stereo;
        let n = (rate * 0.8) as usize;
        let lines = (rate * 4.01) as usize;
        for side in 0..2 {
            self.buf[side] = vec![0.0; n.max(4)];
            self.line[side] = vec![0.0; lines.max(2)];
        }
        self.write = 0;
        self.line_write = 0;
        self.ratio = 1.0;
        self.density = 2.5;
        self.time_jitter = 0.0;
        self.place_jitter = 0.0;
        self.mix = 0.5;
        self.jitter_scale = rate * 0.045_351_47;
        self.grain_len = (rate * -421.7265) as i32;
        self.countdown = ((self.grain_len as f32 * 0.4) as f64).round() as i32;
        self.grains = [Grain::default(); 20];
        self.prev = [0.0; 2];
        self.delay = rate * 0.1;
        self.delay_target = rate * 0.1;
        self.delay_coef = 4.41 / rate;
        self.delay_min = rate * 0.001;
        self.delay_max = (lines as i32 - 1) as f32;
        self.tempo = 120.0;
        self.clip = true;
        self.clip_gain = 1.0;
        self.clip_scale = 0.2;
        self.window = (0..WINDOW)
            .map(|i| {
                let x = f64::from(i as f32 * std::f32::consts::PI) / (WINDOW as f64 - 1.0);
                let s = x.sin();
                (s * s) as f32
            })
            .collect();
    }

    /// `FUN_180183e80`: empty the buffers.
    fn clear(&mut self) {
        for side in 0..2 {
            self.buf[side].fill(0.0);
            self.line[side].fill(0.0);
        }
    }

    /// `FUN_180182910` for the cases the reverb uses.
    fn set(&mut self, case: u32, value: f32) {
        let rate = self.rate;
        match case {
            0 => self.ratio = 2f32.powf(value * 0.083_333_336),
            1 => self.grain_len = (value * 0.001 * rate) as i32,
            2 => self.density = value,
            3 => self.time_jitter = value,
            4 => self.place_jitter = value,
            5 => self.mix = value,
            6 => {
                let samples = value * rate * 0.001;
                self.delay_target = max(self.delay_min, min(samples, self.delay_max));
            }
            9 => self.tempo = value,
            10 => {
                let samples = value * rate * 15.0 / self.tempo;
                self.delay_target = min(samples + 1.0, self.delay_max);
            }
            13 => {
                self.clip_gain = value;
                self.clip_scale = 0.2 / value;
            }
            14 => self.clip = value >= 0.5,
            15 => {
                self.ratio = 2f32.powf(value * 0.083_333_336);
                self.density = 2.25;
                self.time_jitter = 0.0;
                let base = rate * 0.15;
                let whole = base as i32 as f32;
                if value == 0.0 {
                    self.place_jitter = 0.0;
                    self.grain_len = base as i32;
                } else {
                    self.place_jitter = 0.3;
                    self.grain_len = whole as i32;
                    if value > 12.0 {
                        self.grain_len = (whole - (value - 12.0) * 6.25) as i32;
                    } else if value < -12.0 {
                        self.grain_len = (whole - (value * 6.25 + 75.0)) as i32;
                    }
                }
            }
            _ => {}
        }
    }

    fn soft_clip(&self, x: f32) -> f32 {
        let x = self.clip_scale * x;
        let y = if x > 2.0 {
            1.0
        } else if x < -2.0 {
            -1.0
        } else {
            (2.0 - (x * 0.5).abs()) * x * 0.5
        };
        self.clip_gain * y
    }

    /// `FUN_180183180` in mode 3 for one frame.
    fn run(&mut self, input: [f32; 2], seed: &mut u32) -> [f32; 2] {
        let sides = if self.stereo { 2 } else { 1 };
        let input = if self.stereo { input } else { [input[0], 0.0] };

        // The delay ahead of the grains.
        self.delay += (self.delay_target - self.delay) * self.delay_coef;
        let whole = self.delay as i32;
        let frac = self.delay - whole as f32;
        let size = self.line[0].len() as i32;
        let mut i = self.line_write as i32 - whole;
        while i < 0 {
            i += size;
        }
        let mut j = i - 1;
        while j < 0 {
            j += size;
        }
        let mut delayed = [0.0f32; 2];
        for (side, out) in delayed.iter_mut().enumerate() {
            let line = &self.line[side];
            *out = (1.0 - frac) * line[i as usize] + frac * line[j as usize];
        }

        // The two-tap filter into the grain buffer.
        let c = min(2.0 - self.ratio, 1.0);
        let n = self.buf[0].len();
        // A mono shifter's right side carries zeros all the way.
        for ((x, prev), buf) in delayed.iter().zip(&mut self.prev).zip(&mut self.buf) {
            buf[self.write] = ((c + 1.0) * x + (1.0 - c) * *prev) * 0.5;
            *prev = *x;
        }
        self.write += 1;
        if self.write >= n {
            self.write = 0;
        }

        // A new grain when the countdown runs out.
        self.countdown -= 1;
        if self.countdown < 1 {
            let r = rand(seed) as f32;
            self.countdown = (self.grain_len as f32 / self.density + 0.5) as i32
                - (r * -3.051_851e-5 * self.time_jitter * self.jitter_scale) as i32;
            if let Some(slot) = self.grains.iter().position(|g| !g.on) {
                let back = if self.ratio > 1.0 {
                    ((self.ratio - 1.0) * self.grain_len as f32) as i32
                } else {
                    0
                };
                let r = rand(seed) as f32;
                let n = n as i32;
                let mut place = ((self.write as i32 - back) - 5) as f32
                    - r * 3.051_851e-5 * self.place_jitter * self.jitter_scale;
                while place < 0.0 {
                    place += n as f32;
                }
                while place >= n as f32 {
                    place -= n as f32;
                }
                self.grains[slot] = Grain {
                    on: true,
                    len: self.grain_len,
                    left: self.grain_len,
                    place,
                    step: self.ratio,
                };
            }
        }

        // The grains.
        let mut sum = [0.0f32; 2];
        let n = n as i32;
        for g in self.grains.iter_mut().filter(|g| g.on) {
            let at = (1.0 - g.left as f32 / g.len as f32) * (WINDOW as f32 - 1.0);
            let wi = at as i32;
            let wf = at - wi as f32;
            let mut weight = (1.0 - wf) * self.window[wi as usize];
            if wi != WINDOW as i32 - 1 {
                weight += wf * self.window[wi as usize + 1];
            }
            let i = g.place as i32;
            let frac = g.place - i as f32;
            let older = if i - 1 < 0 { i - 1 + n } else { i - 1 };
            let i1 = if i + 1 < n { i + 1 } else { i + 1 - n };
            let i2 = if i + 2 < n { i + 2 } else { i + 2 - n };
            for (side, total) in sum.iter_mut().enumerate().take(sides) {
                let b = &self.buf[side];
                let v = lagrange(
                    b[older as usize],
                    b[i as usize],
                    b[i1 as usize],
                    b[i2 as usize],
                    frac,
                );
                *total += v * weight;
            }
            g.place += g.step;
            if g.place >= n as f32 {
                g.place -= n as f32;
            }
            g.left -= 1;
            if g.left < 1 {
                g.on = false;
            }
        }
        if self.clip {
            for total in sum.iter_mut() {
                *total = self.soft_clip(*total);
            }
        }

        // Mode 3 writes the input into the delay; the feedback is silent.
        for (x, line) in input.iter().zip(&mut self.line) {
            line[self.line_write] = *x;
        }
        self.line_write += 1;
        if self.line_write as i32 > size - 1 {
            self.line_write = 0;
        }

        let mut out = [0.0f32; 2];
        for side in 0..sides {
            out[side] = self.mix * sum[side] + (1.0 - self.mix) * input[side];
        }
        out
    }
}

/// One of the four groups of four delay lines.
#[derive(Debug, Clone, Default)]
struct Line {
    buf: Vec<f32>,
    /// Where the length is, where it is going and where size puts it.
    len: f32,
    target: f32,
    base: f32,
    /// The damping shelf `[a1, b0, b1]` and its state.
    shelf: [f32; 3],
    state: f32,
    /// What the line gave the matrix this frame.
    out: f32,
}

/// An allpass diffuser of eight stages, `FUN_180184170`.
#[derive(Debug, Clone)]
struct Diffuser {
    buf: Vec<f32>,
    lens: [i32; 8],
    write: usize,
}

impl Default for Diffuser {
    fn default() -> Self {
        Diffuser {
            buf: vec![0.0; 8 * 4096],
            lens: [1; 8],
            write: 0,
        }
    }
}

impl Diffuser {
    fn run(&mut self, x: f32) -> f32 {
        let mut y = x;
        for (k, g) in DIFFUSER_GAINS.iter().enumerate() {
            let stage = &mut self.buf[k * 4096..(k + 1) * 4096];
            let d = stage[(self.write as i32 - self.lens[k]) as usize & 0xfff];
            let v = d * g + y;
            stage[self.write] = v;
            y = d - v * g;
        }
        self.write = (self.write + 1) & 0xfff;
        y
    }
}

/// A peak follower: up by `attack`, down by `release`.
#[derive(Debug, Clone, Copy)]
struct Follower {
    level: f32,
    rise: f32,
    keep: f32,
    fall: f32,
}

impl Follower {
    fn run(&mut self, x: f32) -> f32 {
        let x = x.abs();
        self.level = if x > self.level {
            self.level * self.keep + x * self.rise
        } else {
            self.level * self.fall
        };
        self.level
    }
}

/// The library's reverb, `ShimmerReverbImpl` and the object behind it.
#[derive(Debug, Clone)]
struct Engine {
    rate: f32,
    lines: [Line; 16],
    mask: usize,
    write: usize,
    /// Every line's length moves this share of the way to its target.
    glide: f32,
    lfo_phase: f32,
    lfo_step: f32,
    lfo_depth: f32,
    matrix: [[f32; 16]; 16],
    angle: f32,
    rr: usize,
    size_gain: f32,
    /// The decay and its high-frequency share, seconds.
    decay: f32,
    decay_high: f32,
    predelay: [Vec<f32>; 2],
    predelay_write: usize,
    predelay_len: f32,
    predelay_target: f32,
    diffusers: [Diffuser; 2],
    diffusion: f32,
    low_cut: OnePole,
    high_cut: OnePole,
    in_gain: f32,
    tempo: f32,
    /// The envelope: its detector filter, follower and the dB it makes.
    detector: Biquad,
    follower: Follower,
    env_scale: f32,
    env_threshold: f32,
    env_offset: f32,
    env_mode: i32,
    sidechain: bool,
    /// S0 input, S1 damping, S2 envelope, S3 width, S4 brightness, S5
    /// decay, S6 peak frequency, S7 peak gain, S8 peak Q, S9 input low cut,
    /// S10 input high cut, S11 feedback high cut, S12 feedback low cut.
    s: [Smoother; 13],
    reverb_on: bool,
    hq: bool,
    feedback_on: bool,
    limiter: bool,
    freeze: i32,
    pitch_type: i32,
    /// The feedback: shifter, its signal, DC blocker and filters.
    shifters: [Shifter; 3],
    seed: u32,
    fb: f32,
    fb_pitch: f32,
    fb_gain: f32,
    fb_mix: f32,
    dc: f32,
    dc_coef: f32,
    fb_low: Biquad,
    fb_high: Biquad,
    ff_dry: f32,
    ff_amp: [f32; 2],
    /// The output: followers, limiter, peak filter, levels.
    out_follow: [Follower; 2],
    limit: f32,
    limit_coef: f32,
    peak: [Biquad; 2],
    dry: f32,
    dry_target: f32,
    wet: f32,
    wet_db: f32,
    level_coef: f32,
}

impl Engine {
    /// The library object as its constructor leaves it, prepared for `rate`.
    fn new(rate: f32) -> Self {
        let follower = Follower {
            level: 0.0,
            rise: f32::from_bits(0x3ba3_0f80),
            keep: f32::from_bits(0x3f7e_b9e1),
            fall: f32::from_bits(0x3f7f_efa8),
        };
        let mut engine = Engine {
            rate,
            lines: Default::default(),
            mask: 0,
            write: 0,
            glide: 0.0,
            lfo_phase: 0.0,
            lfo_step: 0.0,
            lfo_depth: 0.0,
            matrix: [[0.0; 16]; 16],
            angle: 45.0,
            rr: 0,
            size_gain: 1.0,
            decay: 3.5,
            decay_high: 0.5,
            predelay: [vec![0.0; PREDELAY], vec![0.0; PREDELAY]],
            predelay_write: 0,
            predelay_len: 1.0,
            predelay_target: 1.0,
            diffusers: Default::default(),
            diffusion: 0.0,
            low_cut: OnePole::default(),
            high_cut: OnePole::default(),
            in_gain: 1.0,
            tempo: 120.0,
            detector: Biquad::default(),
            follower,
            env_scale: 0.0,
            env_threshold: 0.0,
            env_offset: 0.0,
            env_mode: 1,
            sidechain: false,
            s: [Smoother::default(); 13],
            reverb_on: true,
            hq: true,
            feedback_on: true,
            limiter: true,
            freeze: 1,
            pitch_type: 0,
            shifters: Default::default(),
            seed: 0x3920a,
            fb: 0.0,
            fb_pitch: 0.0,
            fb_gain: 0.0,
            fb_mix: 0.0,
            dc: 0.0,
            dc_coef: 0.95,
            fb_low: Biquad::default(),
            fb_high: Biquad::default(),
            ff_dry: 0.5,
            ff_amp: [0.5, 0.5],
            out_follow: [follower; 2],
            limit: 0.0,
            limit_coef: 0.0,
            peak: Default::default(),
            dry: 0.5,
            dry_target: 0.5,
            wet: 0.5,
            wet_db: 0.0,
            level_coef: 0.0,
        };
        engine.prepare(rate);
        engine
    }

    /// The damping shelf of a line of `base` samples: `-60 dB` over the
    /// decay at DC and over the high decay above the crossover.
    fn shelf(&self, base: f32) -> [f32; 3] {
        let rate = self.rate;
        let x = base / rate * -3.0;
        let low = 10f32.powf(x / self.decay);
        let high = 10f32.powf(x / self.decay_high);
        let inv = 1.0 / (CROSSOVER * std::f32::consts::TAU);
        let k = inv * (rate + rate);
        let high_k = high * inv * (rate + rate);
        let norm = 1.0 / (k + 1.0);
        [
            (1.0 - k) * norm,
            (high_k + low) * norm,
            (low - high_k) * norm,
        ]
    }

    /// The feedback matrix: a rotation by the character, Kronecker-squared
    /// twice into 16 by 16.
    fn rotate(&mut self, cos: f32, sin: f32) {
        let m2 = [[cos, sin], [-sin, cos]];
        let mut m4 = [[0.0f32; 4]; 4];
        for (r, row) in m4.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = m2[r >> 1][c >> 1] * m2[r & 1][c & 1];
            }
        }
        for (r, row) in self.matrix.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = m4[r >> 2][c >> 2] * m4[r & 3][c & 3];
            }
        }
    }

    /// `FUN_1801849a0`: buffers, rates and the library's own defaults.
    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        let sr16 = rate * 0.0625;
        self.decay = 3.5;
        self.decay_high = 0.5;
        let size = (f64::from(rate) + 0.5) as i32;
        let mut buf = 1usize;
        while (buf as i32) < size {
            buf *= 2;
        }
        self.mask = buf - 1;
        self.write = 0;
        self.glide = 6.615 / rate;
        for (line, seconds) in self.lines.iter_mut().zip(BASE_SECONDS) {
            line.buf = vec![0.0; buf];
            line.base = rate * seconds;
            line.len = line.base;
            line.target = line.base;
        }
        self.dry = 0.5;
        self.dry_target = 0.5;
        self.wet = 0.5;
        self.level_coef = 1.0 - SMOOTH_BASE.powf(45.454_544 / rate);
        self.env_offset = 0.0;
        self.sidechain = false;
        self.hq = true;
        self.reverb_on = true;
        self.feedback_on = true;
        self.limiter = true;
        self.in_gain = 1.0;
        self.tempo = 120.0;
        self.pitch_type = 0;
        self.ff_amp = [0.5, 0.5];
        self.ff_dry = 0.5;
        self.size_gain = f32::from_bits(0x3fee_eeef);
        self.detector.c = Biquad::high_pass(20.0, 0.75, rate);
        for d in self.diffusers.iter_mut() {
            for (len, seconds) in d.lens.iter_mut().zip(DIFFUSER_SECONDS) {
                *len = (rate * seconds) as i32;
            }
        }
        self.lfo_phase = 0.0;
        self.lfo_step = 0.5 / rate;
        self.angle = 45.0;
        self.rotate(f32::from_bits(0x3f35_04f2), f32::from_bits(0x3f35_04f3));
        self.limit_coef = 1.0 - SMOOTH_BASE.powf(100.0 / 44100.0);
        let fall = (-110.0f32 / 44100.0).exp();
        for f in self.out_follow.iter_mut() {
            f.fall = fall;
        }
        for k in 0..16 {
            let shelf = self.shelf(self.lines[k].base);
            self.lines[k].shelf = shelf;
        }
        self.follower.level = 0.0;
        let attack = (-220.0 / rate).exp();
        self.follower.rise = 1.0 - attack;
        self.follower.keep = attack;
        self.follower.fall = (-11.0 / rate).exp();
        self.fb_gain = 0.0;
        self.fb = 0.0;
        self.dc_coef = (8.946_186e-7 - rate * 2.672_287e-12) * rate + 0.915_744_4;
        let setups: [(bool, [(u32, f32); 11]); 3] = [
            (
                false,
                [
                    (0, 12.0),
                    (1, 150.0),
                    (2, 3.5),
                    (3, 0.1),
                    (4, 0.1),
                    (5, 1.0),
                    (6, 1.0),
                    (7, 0.0),
                    (13, 5.0),
                    (8, 3.0),
                    (14, 1.0),
                ],
            ),
            (
                true,
                [
                    (0, 7.0),
                    (1, 150.0),
                    (2, 2.25),
                    (3, 0.0),
                    (4, 0.1),
                    (5, 1.0),
                    (6, 1.0),
                    (7, 0.0),
                    (13, 5.0),
                    (8, 3.0),
                    (14, 0.0),
                ],
            ),
            (
                true,
                [
                    (0, 12.0),
                    (1, 150.0),
                    (2, 2.25),
                    (3, 0.0),
                    (4, 0.1),
                    (5, 1.0),
                    (6, 1.0),
                    (7, 0.0),
                    (13, 5.0),
                    (8, 3.0),
                    (14, 0.0),
                ],
            ),
        ];
        for (shifter, (stereo, calls)) in self.shifters.iter_mut().zip(setups) {
            shifter.prepare(rate, stereo);
            for (case, value) in calls {
                shifter.set(case, value);
            }
        }
        self.diffusion = 0.0;
        self.fb_mix = 0.0;
        self.freeze = 1;
        self.s[0].current = 1.0;
        self.s[0].target = 1.0;
        self.s[0].time = 0.02;
        self.s[1].current = 1.0;
        self.s[1].target = 1.0;
        self.s[1].time = 0.02;
        for (k, s) in self.s.iter_mut().enumerate() {
            s.rate(if k < 6 { rate } else { sr16 });
        }
        self.clear();
    }

    /// `FUN_180188ac0`: empty the lines, the diffusers, the predelay and
    /// the filters.
    fn clear(&mut self) {
        self.clear_lines();
        for b in [&mut self.detector, &mut self.fb_low, &mut self.fb_high] {
            b.s = [0.0; 2];
        }
        self.high_cut.s = [0.0; 2];
        self.low_cut.s = [0.0; 2];
        for p in self.peak.iter_mut() {
            p.s = [0.0; 2];
        }
        self.dc = 0.0;
        self.shifters[0].clear();
        self.fb = 0.0;
    }

    fn clear_lines(&mut self) {
        for line in self.lines.iter_mut() {
            line.buf.fill(0.0);
            line.state = 0.0;
        }
        for p in self.predelay.iter_mut() {
            p.fill(0.0);
        }
        for d in self.diffusers.iter_mut() {
            d.buf.fill(0.0);
        }
    }

    fn apply_freeze(&mut self) {
        if self.freeze == 1 {
            self.s[0].target = 1.0;
            self.s[1].target = 1.0;
        } else {
            self.s[1].target = 0.0;
            self.s[0].target = if self.freeze == 2 { 0.0 } else { 1.0 };
        }
    }

    /// `FUN_180186030`: one parameter as the shell hands it over.
    fn set(&mut self, index: usize, p: f32) {
        let rate = self.rate;
        match index {
            0 => self.in_gain = decibels(p),
            1 => self.s[10].target = p,
            2 => self.s[9].target = p,
            3 => {
                let on = p >= 0.5;
                let was = self.reverb_on;
                self.reverb_on = on;
                if !on {
                    if was {
                        self.clear_lines();
                        self.s[0].target = 1.0;
                        self.s[1].target = 1.0;
                    }
                } else if !was {
                    self.apply_freeze();
                }
            }
            4 => self.s[5].target = p,
            5 => self.set_predelay(p * 0.001 * rate),
            6 => self.set_predelay(15.0 / self.tempo * (p * rate)),
            7 => self.s[4].target = p,
            8 => {
                self.angle = p * 90.0;
                let (sin, cos) = (self.angle * 0.017_453_294).sin_cos();
                self.rotate(cos, sin);
            }
            9 => self.set_size(p),
            10 => self.lfo_step = p / rate,
            11 => self.lfo_depth = p * 0.1 * (rate * (1.0 / 44100.0)),
            12 => self.diffusion = p,
            13 => {
                self.freeze = (p as f64).round() as i32;
                if self.reverb_on {
                    self.apply_freeze();
                }
            }
            14 => self.hq = p >= 0.5,
            15 => {
                let on = p >= 0.5;
                let was = self.feedback_on;
                self.feedback_on = on;
                if !on && was {
                    self.fb_low.s = [0.0; 2];
                    self.fb_high.s = [0.0; 2];
                    self.dc = 0.0;
                    self.shifters[0].clear();
                }
            }
            16 => {
                self.fb_pitch = p;
                self.shifters[0].set(0, p);
            }
            17 => self.shifters[0].set(6, p),
            18 => self.shifters[0].set(10, p),
            19 => self.fb_gain = p,
            20 => self.s[11].target = p,
            21 => self.s[12].target = p,
            22 => self.fb_mix = p,
            23 => self.env_scale = p,
            24 => self.env_threshold = p,
            25 => {
                let a = (-2200.0 / (p * rate)).exp();
                self.follower.keep = a;
                self.follower.rise = 1.0 - a;
            }
            26 => self.follower.fall = (-2200.0 / (p * rate)).exp(),
            27 => self.env_offset = p,
            28 => self.env_mode = (p as f64).round() as i32,
            29 => self.sidechain = p >= 0.5,
            30 => self.detector.c = Biquad::high_pass(p, 0.75, rate),
            31 => {
                self.s[2].time = p * 0.001;
                self.s[2].rate(rate);
            }
            32 => self.dry_target = max(min(decibels(p), 5.6), 0.0),
            33 => self.wet_db = p,
            34 => self.s[6].target = p,
            35 => self.s[7].target = decibels(p),
            36 => self.s[8].target = p,
            37 => self.s[3].target = p,
            38 => self.limiter = p >= 0.5,
            39 => self.pitch_type = (p as f64).round() as i32,
            40 => self.ff_dry = p,
            41 => self.shifters[1].set(15, p),
            42 => self.shifters[2].set(15, p),
            43 => self.ff_amp[0] = p,
            44 => self.ff_amp[1] = p,
            45 => self.shifters[1].set(6, p),
            46 => self.shifters[1].set(10, p),
            47 => self.shifters[2].set(6, p),
            48 => self.shifters[2].set(10, p),
            _ => {}
        }
    }

    fn set_predelay(&mut self, samples: f32) {
        self.predelay_target = max(1.0, min(PREDELAY as f32, samples));
    }

    /// Case 9: the room size picks each line's length from the primes.
    fn set_size(&mut self, size: f32) {
        let a = size * 19.685 + 230.315;
        let mut b = (a - 175.0) * 0.068_965_52 + 100.0;
        let offset = (-0.208_053_71 - (b / a) * -1.208_053_7) * a;
        b = (b - offset) * 0.006_451_613;
        for (k, place) in SIZE_TABLE.iter().enumerate() {
            let index = (b * place + offset) as i32;
            let prime = PRIMES[index.clamp(0, PRIMES.len() as i32 - 1) as usize];
            let base = f32::from(prime) * self.rate * (1.0 / 44100.0);
            self.lines[k].base = base;
            self.lines[k].target = base;
            self.lines[k].shelf = self.shelf(base);
        }
        self.size_gain = (a - 250.0) * 0.001_333_333_3 + 1.0;
    }

    fn set_tempo(&mut self, tempo: f32) {
        self.tempo = tempo;
        for s in self.shifters.iter_mut() {
            s.set(9, tempo);
        }
    }

    /// The predelay read, `FUN_180183f40`.
    fn read_predelay(&self, side: usize) -> f32 {
        let buf = &self.predelay[side];
        let whole = self.predelay_len as i32;
        let frac = self.predelay_len - whole as f32;
        let n = PREDELAY as i32;
        let mut i = self.predelay_write as i32 - whole;
        while i < 0 {
            i += n;
        }
        let older = if i - 1 < 0 { i - 1 + n } else { i - 1 };
        if !self.hq {
            return (1.0 - frac) * buf[i as usize] + frac * buf[older as usize];
        }
        let newer = if i + 1 > n - 1 { i + 1 - n } else { i + 1 };
        let oldest = if i - 2 < 0 { i - 2 + n } else { i - 2 };
        lagrange(
            buf[newer as usize],
            buf[i as usize],
            buf[older as usize],
            buf[oldest as usize],
            frac,
        )
    }

    /// `FUN_180187290` for one frame. `dry` is the plugin's input, `key`
    /// what the envelope listens to when it listens to the sidechain.
    fn run(&mut self, dry: [f32; 2], key: [f32; 2]) -> [f32; 2] {
        let rate = self.rate;

        // The envelope.
        let mut env_db = 0.0;
        if self.env_mode == 1 {
            self.s[2].target = 0.0;
            self.follower.level = 0.0;
            self.s[2].tick();
        } else {
            let x = if self.sidechain {
                (key[1] + key[0]) * 0.5
            } else {
                (dry[1] + dry[0]) * (self.in_gain * 0.5)
            };
            let y = self.detector.run(x);
            self.s[2].target = self.follower.run(y);
            env_db = self.s[2].tick().log10() * 20.0;
        }

        let mut l = self.in_gain * dry[0];
        let mut r = self.in_gain * dry[1];
        if self.pitch_type == 1 && self.feedback_on {
            let input = [l, r];
            l *= self.ff_dry;
            r *= self.ff_dry;
            for k in 0..2 {
                let out = self.shifters[1 + k].run(input, &mut self.seed);
                l += self.ff_amp[k] * out[0];
                r += self.ff_amp[k] * out[1];
            }
        }

        // The input filters, recomputed every sample: FL's counter that
        // would space them sixteen samples apart never moves.
        let w = self.s[9].tick() * std::f32::consts::PI / rate;
        let norm = 1.0 / (w + 1.0);
        self.low_cut.c = [norm, -norm, (w - 1.0) * norm];
        let w = self.s[10].tick() * std::f32::consts::PI / rate;
        let norm = 1.0 / (w + 1.0);
        self.high_cut.c = [w * norm, w * norm, (w - 1.0) * norm];
        let mut l = self.low_cut.run(0, l);
        let mut r = self.low_cut.run(1, r);
        l = self.high_cut.run(0, l);
        r = self.high_cut.run(1, r);

        if self.reverb_on {
            let pl = self.read_predelay(0);
            let pr = self.read_predelay(1);
            self.predelay[0][self.predelay_write] = l;
            self.predelay[1][self.predelay_write] = r;
            self.predelay_write += 1;
            if self.predelay_write >= PREDELAY {
                self.predelay_write = 0;
            }
            let rest = 1.0 - self.diffusion;
            l = self.diffusers[0].run(pl) * self.diffusion + rest * pl;
            r = self.diffusers[1].run(pr) * self.diffusion + rest * pr;
        }
        if self.feedback_on && self.pitch_type == 0 {
            let mut fb = self.fb_gain * self.fb * 8.0;
            fb *= min(self.decay.powf(-0.2), 1.0);
            let semis = self.fb_pitch * 0.083_333_336;
            fb *= if self.fb_pitch < 0.0 {
                0.5 - semis * 2.0
            } else {
                semis * 0.7 + 0.5
            };
            fb *= self.s[1].current;
            l += fb;
            r += fb;
        }
        let mid = (r + l) * 0.5;
        let s0 = self.s[0].tick();
        let s1 = self.s[1].tick();

        let (mut wet_l, mut wet_r) = (l, r);
        if self.reverb_on {
            self.network(l * s0, r * s0, s1, env_db);
            let y: [f32; 16] = std::array::from_fn(|k| self.lines[k].out);
            let gain = self.size_gain * 0.125;
            let mut a = y[1] + y[0];
            let mut b = a;
            for k in 1..8 {
                a += y[2 * k];
                b += y[2 * k + 1];
            }
            wet_l = a * gain;
            wet_r = b * gain;
        }

        // The output followers and the limiter.
        let el = self.out_follow[0].run(wet_l);
        let er = self.out_follow[1].run(wet_r);
        let goal = if self.limiter {
            max(el, er)
        } else {
            f32::from_bits(0x3e7f_fd61)
        };
        self.limit += (goal - self.limit) * self.limit_coef;
        let g = 1.0 / max(self.limit * 4.0, 1.0);
        wet_l *= g;
        wet_r *= g;

        if self.feedback_on && self.pitch_type == 0 {
            let input = (wet_r + wet_l) * (self.fb_mix * 0.5) + (1.0 - self.fb_mix) * mid;
            let x = self.shifters[0].run([input, 0.0], &mut self.seed)[0];
            let y = x + self.dc;
            self.dc = y * self.dc_coef - x;
            let w = self.s[11].tick() * 4.881_407 / rate;
            let norm = 1.0 / (w + 1.0);
            let h = (w - 1.0) * norm;
            let g = w * norm;
            self.fb_low.c = [g * g, g * 2.0 * g, g * g, h * 2.0, h * h];
            let w = self.s[12].tick() * 4.881_407 / rate;
            let norm = 1.0 / (w + 1.0);
            let h = (w - 1.0) * norm;
            let neg = -norm;
            self.fb_high.c = [norm * norm, norm * 2.0 * neg, neg * neg, h * 2.0, h * h];
            let y = self.fb_low.run(y);
            self.fb = self.fb_high.run(y);
        }

        // The peak filter.
        let freq = self.s[6].tick();
        let gain = self.s[7].tick();
        let q = self.s[8].tick();
        let w = freq * std::f32::consts::TAU / rate;
        let sin = w.sin();
        let q2 = q * 2.0;
        let k = (q2 - sin) / (q2 + sin);
        let cos = w.cos();
        let a1 = -(cos * (k + 1.0));
        let half = (k + 1.0) * 0.5;
        let side = (1.0 - k) * 0.5 * gain;
        let c = [half + side, a1, half - side, a1, k];
        self.peak[0].c = c;
        self.peak[1].c = c;
        let yl = self.peak[0].run(wet_l);
        let yr = self.peak[1].run(wet_r);

        // Levels and width.
        let wet_db = if self.env_mode == 2 {
            let over = max(env_db - self.env_threshold, 0.0) + self.env_offset * self.env_threshold;
            self.env_scale * 2.0 * over + self.wet_db
        } else {
            self.wet_db
        };
        let wet_goal = max(min(decibels(wet_db), 5.6), 0.0);
        self.dry += (self.dry_target - self.dry) * self.level_coef;
        self.wet += (wet_goal - self.wet) * self.level_coef;
        let mid = (yr + yl) * 0.5;
        let width = self.s[3].tick();
        let side = (yl - mid) * width;
        [
            (side + mid) * self.wet + self.dry * dry[0],
            dry[1] * self.dry + (mid - side) * self.wet,
        ]
    }

    /// The modulated lines, their damping and the matrix, for one frame of
    /// input to the network.
    fn network(&mut self, l: f32, r: f32, s1: f32, env_db: f32) {
        let phase = self.lfo_step + self.lfo_phase;
        self.lfo_phase = phase - phase.trunc();
        for (k, line) in self.lines.iter_mut().enumerate() {
            let p = self.lfo_phase + k as f32 * 0.0625;
            let t = p - p.trunc();
            let t = t + t;
            let a = t.min(1.0);
            let b = t.max(1.0) - 1.0;
            let lfo = ((a - a * a) - (b - b * b)) * 4.0;
            line.target = (lfo * self.lfo_depth + 1.0) * line.base;
        }

        self.decay = self.s[5].tick();
        if self.env_mode == 3 {
            let over = max(env_db - self.env_threshold, 0.0) + self.env_offset * self.env_threshold;
            let t = over * self.env_scale + self.decay;
            self.decay = max(min(t, 20.0), 0.25);
        }
        let bright = self.s[4].tick();
        self.decay_high = self.decay * bright;
        let rr = self.rr;
        self.lines[rr].shelf = self.shelf(self.lines[rr].base);
        self.rr = (rr + 1) % 16;

        let mask = self.mask;
        let write = self.write;
        let mut y = [0.0f32; 16];
        for (k, line) in self.lines.iter_mut().enumerate() {
            line.len += (line.target - line.len) * self.glide;
            let whole = line.len as i32;
            let frac = line.len - whole as f32;
            let i = (write as i32 - whole) as usize & mask;
            let b = &line.buf;
            let x = if self.hq {
                hermite(
                    b[(i + 1) & mask],
                    b[i],
                    b[i.wrapping_sub(1) & mask],
                    b[i.wrapping_sub(2) & mask],
                    frac,
                )
            } else {
                b[i] * (1.0 - frac) + b[i.wrapping_sub(1) & mask] * frac
            };
            let [a1, b0, b1] = line.shelf;
            let t = x * b0 + line.state;
            line.state = x * b1 - t * a1;
            y[k] = t * s1 + x * (1.0 - s1);
        }
        self.write = (write + 1) & mask;

        let inject = [l, r];
        let mid = (r + l) * 0.5;
        for (k, line) in self.lines.iter_mut().enumerate() {
            let mut sum = [0.0f32; 4];
            for (j, s) in sum.iter_mut().enumerate() {
                let row = &self.matrix[k];
                *s = (y[4 + j] * row[4 + j] + y[j] * row[j])
                    + (y[12 + j] * row[12 + j] + y[8 + j] * row[8 + j]);
            }
            let own = if k < 2 { mid } else { inject[k & 1] };
            let v = sum[0] + sum[1] + sum[2] + sum[3] + own;
            line.buf[self.write] = v;
            line.out = y[k];
        }
    }
}

/// The shell: the state, the controls and what each control hands the
/// library.
#[derive(Debug, Clone)]
pub struct Luxeverb {
    params: [i32; PARAMS],
    /// The insert the envelope's sidechain comes from, 0 for none.
    sidechain: usize,
    /// The sync buttons of the predelay, the feedback delay and the two
    /// feedforward delays.
    sync: [bool; 4],
    engine: Engine,
    tempo: f64,
    key: crate::Sidechain,
}

impl Default for Luxeverb {
    fn default() -> Self {
        let mut plugin = Luxeverb {
            params: DEFAULTS,
            sidechain: 0,
            sync: [false; 4],
            engine: Engine::new(44100.0),
            tempo: 120.0,
            key: crate::Sidechain::default(),
        };
        plugin.push_all();
        plugin
    }
}

impl Luxeverb {
    /// The control's place in its range, 0 to 1, as the shell works it
    /// out in doubles.
    fn place(index: usize, value: i32) -> f32 {
        let (lo, hi) = RANGES[index];
        let x = f64::from(value - lo) * (1.0 / f64::from(hi - lo));
        x.clamp(0.0, 1.0) as f32
    }

    /// `FUN_00730100`: what the library gets for a control at `value`.
    fn engine_value(index: usize, value: i32) -> f32 {
        let x = Self::place(index, value);
        let (lo, hi) = RANGES[index];
        let span = (f64::from(lo), f64::from(hi));
        match index {
            0 | 32 | 33 => curve(x, -60.0, 15.0, 2.0),
            1 => curve(x, span.0, span.1, 1.5),
            2 | 20 | 21 | 30 | 34 => curve(x, span.0, span.1, 0.5),
            4 => curve(x, 0.25, 20.0, 0.4),
            5 => curve(x, 0.0, 500.0, 0.5),
            6 | 18 | 46 | 48 => x * 16.0,
            8 | 11 => (f64::from(value) * 0.01) as f32,
            9 | 31 => x * 99.0 + 1.0,
            13 | 28 => (value + 1) as f32,
            16 => ((f64::from(x) - 0.5) * 24.0) as f32,
            17 | 45 | 47 => x * 499.0 + 1.0,
            23 => x * 2.0 - 1.0,
            24 | 35 | 37 => value as f32 * 0.01,
            25 | 26 | 38 | 41 | 42 => value as f32,
            27 => (f64::from(x) - 0.5) as f32,
            36 => (f64::from(x) * 4.75 + 0.25) as f32,
            _ => x,
        }
    }

    fn push(&mut self, index: usize) {
        let value = Self::engine_value(index, self.params[index]);
        self.engine.set(index, value);
    }

    /// The shell after a state, a rate or a tempo: every control in order,
    /// then the tempo and the delays the sync buttons pick.
    fn push_all(&mut self) {
        for index in 0..PARAMS {
            self.push(index);
        }
        self.push_tempo();
    }

    /// `FUN_0072fc40`.
    fn push_tempo(&mut self) {
        self.engine.set_tempo(self.tempo as f32);
        for (k, pair) in [(5, 6), (17, 18), (45, 46), (47, 48)]
            .into_iter()
            .enumerate()
        {
            self.push(if self.sync[k] { pair.1 } else { pair.0 });
        }
    }

    /// Read a state into its parts, as `FUN_0072f610` does.
    fn parse(bytes: &[u8]) -> Result<([i32; PARAMS], usize, [bool; 4]), StateError> {
        let version = crate::state_version(bytes, 4 + 4 + 5 + 4 * PARAMS + 2)?;
        if version >= 10 {
            return Err(StateError::Version { found: version });
        }
        let count = match version {
            ..8 => 38,
            8 => 39,
            _ => PARAMS,
        };
        let extra = if version >= 9 { 2 } else { 0 };
        let expected = 13 + 4 * count + extra;
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let word = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let sidechain = word(4).max(0) as usize;
        let mut sync = [bytes[8] != 0, bytes[9] != 0, false, false];
        let mut params = DEFAULTS;
        for (index, param) in params.iter_mut().enumerate().take(count) {
            let value = word(13 + 4 * index);
            let (lo, hi) = RANGES[index];
            if value < lo || value > hi {
                return Err(StateError::Range {
                    param: index,
                    value,
                });
            }
            *param = value;
        }
        if version < 8 {
            params[38] = 1;
        }
        if version < 9 {
            params[39..].copy_from_slice(&FILL);
        } else {
            sync[2] = bytes[13 + 4 * count] != 0;
            sync[3] = bytes[14 + 4 * count] != 0;
        }
        Ok((params, sidechain, sync))
    }
}

impl Effect for Luxeverb {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let (params, sidechain, sync) = Self::parse(bytes)?;
        self.params = params;
        self.sidechain = sidechain;
        self.sync = sync;
        self.push_all();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz > 0.0 && hz != self.engine.rate {
            self.engine.prepare(hz);
            self.push_all();
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 && bpm != self.tempo {
            self.tempo = bpm;
            self.push_tempo();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (at, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let dry = [*l, *r];
            let key = self.key.frame(at).unwrap_or(dry);
            [*l, *r] = self.engine.run(dry, key);
        }
        self.key.clear();
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.push(index);
        }
    }

    fn reset(&mut self) {
        self.engine.clear();
    }

    fn sidechain_input(&self) -> usize {
        self.sidechain
    }

    fn set_sidechain(&mut self, index: usize, left: &[f32], right: &[f32]) {
        if index > 0 && index == self.sidechain {
            self.key.keep(left, right);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    /// A state of the newest version with these parameters.
    fn state(params: &[i32; PARAMS]) -> Vec<u8> {
        let mut bytes = testing::state(&[9, 0]);
        bytes.extend([0, 0, 1, 0, 0]);
        bytes.extend(testing::state(params));
        bytes.extend([0, 0]);
        bytes
    }

    fn with(edit: impl Fn(&mut [i32; PARAMS])) -> Luxeverb {
        let mut params = DEFAULTS;
        // The fill's 1719 is past the control's range, which a saved state
        // cannot hold.
        params[43] = 0;
        edit(&mut params);
        let mut plugin = Luxeverb::default();
        plugin.set_state(&state(&params)).unwrap();
        plugin
    }

    #[test]
    fn the_prime_table_is_the_librarys() {
        assert_eq!(&PRIMES[..6], &[2, 3, 5, 7, 11, 13]);
        assert_eq!(PRIMES[2261], 19997);
        assert!(PRIMES.windows(2).all(|w| w[0] < w[1]));
        // The longest room reaches index 2198 and no further.
        let mut engine = Engine::new(44100.0);
        engine.set_size(100.0);
        assert_eq!(engine.lines[15].base, 19421.0);
    }

    #[test]
    fn the_level_curve_puts_746_at_unity() {
        let db = Luxeverb::engine_value(0, 746);
        assert!(db.abs() < 0.05, "{db}");
        assert_eq!(Luxeverb::engine_value(0, -600), -60.0);
        assert_eq!(Luxeverb::engine_value(0, 1500), 15.0);
        // The decay knob's default is about seven seconds.
        let decay = Luxeverb::engine_value(4, 13105);
        assert!((decay - 7.0).abs() < 0.05, "{decay}");
    }

    #[test]
    fn the_feedback_matrix_is_orthogonal_at_any_character() {
        let mut engine = Engine::new(44100.0);
        for character in [0.0, 0.33, 0.5, 1.0] {
            engine.set(8, character);
            for a in 0..16 {
                for b in 0..16 {
                    let dot: f32 = (0..16)
                        .map(|k| engine.matrix[a][k] * engine.matrix[b][k])
                        .sum();
                    let want = if a == b { 1.0 } else { 0.0 };
                    assert!((dot - want).abs() < 1e-5, "{character} {a} {b} {dot}");
                }
            }
        }
    }

    #[test]
    fn a_lines_shelf_loses_60_db_over_the_decay_at_dc_and_the_high_decay_at_nyquist() {
        let mut engine = Engine::new(48000.0);
        engine.decay = 2.0;
        engine.decay_high = 0.5;
        let len = 3000.0;
        let [a1, b0, b1] = engine.shelf(len);
        let num = [f64::from(b0), f64::from(b1)];
        let den = [1.0, f64::from(a1)];
        let passes = 48000.0 * 2.0 / len as f64;
        let dc = testing::response(&num, &den, 0.0).powf(passes);
        assert!((dc - 1e-3).abs() < 1e-5, "{dc}");
        let passes = 48000.0 * 0.5 / len as f64;
        let top = testing::response(&num, &den, 0.5).powf(passes);
        assert!((top - 1e-3).abs() < 1e-5, "{top}");
    }

    #[test]
    fn the_diffuser_passes_all_of_an_impulses_energy() {
        let mut d = Diffuser::default();
        for (len, seconds) in d.lens.iter_mut().zip(DIFFUSER_SECONDS) {
            *len = (44100.0 * seconds) as i32;
        }
        let energy: f64 = (0..44100)
            .map(|n| f64::from(d.run(if n == 0 { 1.0 } else { 0.0 })).powi(2))
            .sum();
        assert!((energy - 1.0).abs() < 1e-4, "{energy}");
    }

    #[test]
    fn both_interpolators_follow_a_line() {
        // Samples of 3 + 2t, oldest last.
        let at = |t: f32| 3.0 + 2.0 * t;
        for frac in [0.0, 0.25, 0.5, 0.9] {
            let want = at(-frac);
            let h = hermite(at(1.0), at(0.0), at(-1.0), at(-2.0), frac);
            let l = lagrange(at(1.0), at(0.0), at(-1.0), at(-2.0), frac);
            assert!(
                (h - want).abs() < 1e-5 && (l - want).abs() < 1e-4,
                "{frac} {h} {l}"
            );
        }
    }

    #[test]
    fn the_shifter_moves_a_tone_up_an_octave() {
        let rate = 44100.0;
        let mut shifter = Shifter::default();
        shifter.prepare(rate, false);
        for (case, value) in [(0, 12.0), (1, 150.0), (2, 3.5), (3, 0.1), (4, 0.1)] {
            shifter.set(case, value);
        }
        shifter.set(5, 1.0);
        shifter.set(6, 1.0);
        shifter.set(13, 5.0);
        let mut seed = 0x3920a;
        let turns = 441.0 / rate as f64;
        let tone = testing::sine(turns, 88200);
        let out: Vec<f32> = tone
            .iter()
            .map(|x| shifter.run([*x, 0.0], &mut seed)[0])
            .collect();
        let tail = &out[44100..];
        let up = testing::partial(tail, 2.0 * turns);
        let same = testing::partial(tail, turns);
        assert!(up > 4.0 * same && up > 0.05, "{up} {same}");
    }

    #[test]
    fn the_peak_filter_reaches_its_gain_at_its_frequency() {
        let rate = 44100.0;
        let turns = f64::from(Luxeverb::engine_value(34, 4000)) / rate;
        // With the reverb off the wet is the filtered input: the peak's
        // gain is the ratio of a tone through it at +12 dB and at 0 dB.
        let through = |gain: i32| {
            let mut plugin = with(|p| {
                p[3] = 0;
                p[15] = 0;
                p[32] = -600;
                p[34] = 4000;
                p[35] = gain;
                p[38] = 0;
            });
            let tone: Vec<f32> = testing::sine(turns, 44100)
                .iter()
                .map(|x| x * 0.1)
                .collect();
            let (mut l, mut r) = (tone.clone(), tone);
            plugin.process(&mut l, &mut r);
            testing::partial(&l[22050..], turns)
        };
        let got = through(1200) / through(0);
        let want = 10f64.powf(12.0 / 20.0);
        assert!((got / want - 1.0).abs() < 0.01, "{got} {want}");
    }

    /// The time the wet energy takes to fall 60 dB: the slope of the
    /// level in 50 ms blocks from 0.3 s on, while it is within 40 dB.
    fn rt60(out: &[f32], rate: f32) -> f32 {
        let block = (rate * 0.05) as usize;
        let levels: Vec<f64> = out
            .chunks(block)
            .map(|c| {
                10.0 * (c.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / block as f64).log10()
            })
            .collect();
        let start = 6;
        let points: Vec<(f64, f64)> = levels[start..]
            .iter()
            .enumerate()
            .take_while(|(_, l)| **l > levels[start] - 40.0)
            .map(|(k, l)| (k as f64 * 0.05, *l))
            .collect();
        let n = points.len() as f64;
        let (sx, sy) = points
            .iter()
            .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
        let (mx, my) = (sx / n, sy / n);
        let num: f64 = points.iter().map(|(x, y)| (x - mx) * (y - my)).sum();
        let den: f64 = points.iter().map(|(x, _)| (x - mx).powi(2)).sum();
        (-60.0 / (num / den)) as f32
    }

    #[test]
    fn the_tail_dies_away_in_the_decay_time() {
        let rate = 44100.0;
        for knob in [6000, 9000, 13105] {
            let mut plugin = with(|p| {
                p[4] = knob;
                p[7] = 1000; // as bright as the low end
                p[11] = 0;
                p[15] = 0;
                p[32] = -600;
                p[38] = 0;
            });
            let decay = Luxeverb::engine_value(4, knob);
            let len = (rate * (decay + 2.0)) as usize;
            let mut l = vec![0.0f32; len];
            l[100] = 0.5;
            let mut r = l.clone();
            // Let the smoothers settle first.
            let mut quiet = vec![0.0f32; 44100];
            let mut quiet_r = quiet.clone();
            plugin.process(&mut quiet, &mut quiet_r);
            plugin.process(&mut l, &mut r);
            let got = rt60(&l, rate);
            assert!((got / decay - 1.0).abs() < 0.1, "{decay} {got}");
        }
    }

    #[test]
    fn every_state_fl_ships_or_saves_parses_and_plays() {
        let Some(root) = testing::corpus() else {
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        saved.extend(
            testing::projects(&root)
                .into_iter()
                .filter(|s| s.plugin.eq_ignore_ascii_case(NAME)),
        );
        assert!(saved.len() >= 120, "{}", saved.len());
        let mut plugin = Luxeverb::default();
        plugin.set_tempo(128.0);
        let rate = 44100;
        let noise: Vec<f32> = (0..rate)
            .map(|n| {
                let x = ((n as u32).wrapping_mul(2_654_435_761) >> 8) as f32 / (1 << 24) as f32;
                if n < rate / 4 { (x - 0.5) * 0.5 } else { 0.0 }
            })
            .collect();
        for s in &saved {
            plugin
                .set_state(&s.state)
                .unwrap_or_else(|e| panic!("{} {e}", s.file));
            plugin.reset();
            let (mut l, mut r) = (noise.clone(), noise.clone());
            for _ in 0..3 {
                plugin.process(&mut l, &mut r);
                let peak = l.iter().chain(&r).fold(0.0f32, |m, x| m.max(x.abs()));
                assert!(peak.is_finite() && peak < 8.0, "{} {peak}", s.file);
                l.fill(0.0);
                r.fill(0.0);
            }
        }
    }
}
