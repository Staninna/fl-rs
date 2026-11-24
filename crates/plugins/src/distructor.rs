//! Distructor: a rack of up to four modules, each a distortion, a filter,
//! a chorus or a speaker, played one after the other and then through a
//! soft clipper, as `docs/plugins/distructor.md` reads it from
//! `Distructor_x64.dll` and the module plugin `module/Module_x64.dll`.
//!
//! # The state
//!
//! The rack's: an i32 version (5), an i32 count of modules (0 to 4), each
//! module's own state, a byte for the soft clipper on the output, then
//! from version 4 an i32 count and that many i32 (the editor's panel of
//! each module), from version 2 an i32 that is 0, and from version 5 a
//! byte for the peak meters. Only the modules and the soft clipper make
//! sound; the modules play in the order the state has them.
//!
//! A module's: an i32 version (4), its kind (0 distortion, 1 filter, 2
//! chorus, 3 speaker), the distortion algorithm (0 to 17), the
//! oversampling (0 to 4 stages of 2x), a byte "remove DC offset", a byte
//! "bypass", the filter type (0 to 21) and a byte "smooth up modulation",
//! the speaker's impulse (0 to 58), the 49 parameters as i32, an i32 that
//! is 0 (from version 2) and the chorus mode (from version 4).
//!
//! # A module
//!
//! ```text
//! x    = in * mix_in                        mix_in = p36 / 5000
//! x    = up(x)                              distortion and filter, 2^stages times
//! wet  = section(x)
//! out  = (x * (1 - dw) + wet * dw) * mix_out     dw = p37 / 10000, mix_out = p38 / 5000
//! out  = down(out)
//! out  = out - DC                           distortion with "remove DC offset"
//! ```
//!
//! The distortion's 18 algorithms are Blood Overdrive's, Fast Dist's and
//! Soft Clipper's, Harmor's eleven, a wave folder, Aperture, Abrasive and
//! Crusher; the filter is one of 22 types; the chorus is Fruity Delay 3's
//! engine set up as a chorus; the speaker convolves with one of the
//! cabinets in `module/impulses`.

use crate::{Effect, Fidelity, StateError, convolver, delay_3::Delay3};
use crate::{fast_dist::FastDist, soft_clipper::SoftClipper};
use dsp::halfband::HalfBand;
use std::f64::consts::{LN_2, PI};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The plugin's event 201 name.
pub const NAME: &str = "Distructor";

/// The most modules a rack has.
const MODULES: usize = 4;
/// The parameters of a module.
const PARAMS: usize = 49;
/// The newest rack and module versions FL reads.
const VERSION: i32 = 5;
const MODULE_VERSION: i32 = 4;

/// What a module FL has just made holds (`0x007a7c48`).
const DEFAULTS: [i32; PARAMS] = [
    0, 5000, 0, 10000, 0, 0, 500, 5000, 5000, 10000, 500, 5000, 5000, 10000, 2000, 0, 128, 10, 128,
    128, 0, 100, 128, 500, 0, 1000, 0, 950, 2000, 5000, 2000, 5000, 8400, 0, 0, 0, 5000, 10000,
    5000, 10000, 0, 0, 0, 108, 2880, 1500, 1995, 0, 0,
];

/// The range of each parameter's wheel, for [`Effect::set_param`]; a state
/// is taken as it is, as FL takes it.
const RANGES: [(i32, i32); PARAMS] = [
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 1),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 1),
    (64, 192),
    (1, 10),
    (0, 128),
    (0, 128),
    (0, 1),
    (1, 127),
    (0, 160),
    (0, 1000),
    (-1000, 1000),
    (0, 1000),
    (-1000, 1000),
    (0, 1100),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 1),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (0, 10000),
    (-1000, 1000),
    (-1000, 1000),
    (0, 768),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (-3000, 3000),
];

/// The frames the rack works on at a time, and the most stages of 2x
/// oversampling a module runs.
const BLOCK: usize = 256;
const MAX_STAGES: usize = 4;
const WIDE: usize = BLOCK << MAX_STAGES;

/// The soft clipper on the rack's output (`FUN_005df630` with 0.78125 and
/// 1): the knee starts at `T` and reaches 1 at infinity.
const OUT_KNEE: f32 = 0.78125;

/// The kinds of module.
const DISTORTION: i32 = 0;
const FILTER: i32 = 1;
const CHORUS: i32 = 2;
const SPEAKER: i32 = 3;

/// What [`Effect::fidelity`] says.
const UNBUILT_FILTER: &str = "Distructor passes a filter module of a type that is not built";
const NO_IMPULSE: &str = "Distructor has a speaker module whose impulse is not found";

/// `ln 0.01`, the base of every 15 ms smoother.
fn ln_hundredth() -> f64 {
    0.01f64.ln()
}

/// FL's soft clipper (`FUN_005df670`, `FUN_005fdb10`): a sample past `t`
/// in size is bent to `t + a (1 - e^(-(|x| - t) b))`, its sign kept.
fn soft_clip(x: f32, t: f32, a: f32, b: f32) -> f32 {
    if x.abs() > t {
        let over = f64::from(t) - f64::from(x.abs());
        let bent = (1.0 - (over * f64::from(b)).exp()) * f64::from(a) + f64::from(t);
        (bent as f32).copysign(x)
    } else {
        x
    }
}

/// The knee's `a` and `b` for a threshold (`FUN_005fdad0` with top 1).
fn knee(t: f32) -> (f32, f32) {
    let mut a = 1.0 - t;
    if a <= 0.0 {
        a = 1.525_878_9e-5;
    }
    (a, 1.0 / a)
}

/// The sign of a sample as FL's code takes it: 0 for 0.
fn sign(x: f32) -> f32 {
    if x == 0.0 { 0.0 } else { 1f32.copysign(x) }
}

/// `x - trunc(x / t) * t` in f32 (`FUN_0045f9a0`).
fn fmod32(x: f32, t: f32) -> f32 {
    x - ((f64::from(x / t)) as i64 as f32) * t
}

/// One of the module's smoothers (`FUN_00713fa0`): a pair of targets, the
/// pair it is at, and what is left of the way each sample.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Ramp {
    target: [f32; 2],
    now: [f32; 2],
    keep: f32,
}

impl Ramp {
    /// Snap to the targets when both are within 2^-24 (`FUN_00713f30`),
    /// and say whether it did.
    fn settled(&mut self) -> bool {
        let eps = 5.960_464_5e-8;
        let close = (0..2).all(|s| (self.now[s] - self.target[s]).abs() < eps);
        if close {
            self.now = self.target;
        }
        close
    }

    /// Whether the targets and where it is are all 0 (`FUN_00713f80`).
    fn silent(&self) -> bool {
        self.target == [0.0; 2] && self.now == [0.0; 2]
    }

    /// The next `n` values of one side, from where it is (`FUN_00713ee0`).
    fn fill(&mut self, side: usize, out: &mut [f32]) {
        let target = self.target[side];
        let mut d = self.now[side] - target;
        for y in out.iter_mut() {
            d *= self.keep;
            *y = d + target;
        }
        self.now[side] = d + target;
    }

    /// `x *= gain` over a block, in place (`FUN_00713d70`).
    fn scale(&mut self, x: [&mut [f32]; 2]) {
        if self.settled() {
            let zero = self.target == [0.0; 2];
            for (s, x) in x.into_iter().enumerate() {
                let g = self.target[s];
                for v in x.iter_mut() {
                    *v = if zero { 0.0 } else { *v * g };
                }
            }
        } else {
            for (s, x) in x.into_iter().enumerate() {
                let target = self.target[s];
                let mut d = self.now[s] - target;
                for v in x.iter_mut() {
                    d *= self.keep;
                    *v *= d + target;
                }
                self.now[s] = d + target;
            }
        }
    }

    /// `out += x * gain` over a block (`FUN_00713d00`).
    fn add(&mut self, x: [&[f32]; 2], out: [&mut [f32]; 2]) {
        let settled = self.settled();
        for (s, (x, out)) in x.into_iter().zip(out).enumerate() {
            let target = self.target[s];
            if settled {
                if self.target != [0.0; 2] {
                    for (y, x) in out.iter_mut().zip(x) {
                        *y += x * target;
                    }
                }
            } else {
                let mut d = self.now[s] - target;
                for (y, x) in out.iter_mut().zip(x) {
                    d *= self.keep;
                    *y += x * (d + target);
                }
                self.now[s] = d + target;
            }
        }
    }
}

/// An RBJ low pass at Q `1/sqrt 2` in doubles, direct form I
/// (`FUN_005f86c0` type 1, `FUN_005f8ee0`), one state a side.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    state: [[f64; 4]; 2],
}

impl Biquad {
    fn design(&mut self, w: f64) {
        let (sin, cos) = w.sin_cos();
        let alpha = sin * std::f64::consts::FRAC_1_SQRT_2;
        let a0 = 1.0 / (alpha + 1.0);
        let b1 = a0 * (1.0 - cos);
        self.a = [a0 * cos * -2.0, a0 * (1.0 - alpha)];
        self.b = [b1 * 0.5, b1, b1 * 0.5];
    }

    fn run(&mut self, side: usize, block: &mut [f32]) {
        let [x1, x2, y1, y2] = &mut self.state[side];
        for v in block {
            let x = f64::from(*v);
            let y = self.b[0] * x + self.b[1] * *x1 + self.b[2] * *x2
                - self.a[0] * *y1
                - self.a[1] * *y2;
            (*x2, *x1, *y2, *y1) = (*x1, x, *y1, y);
            *v = y as f32;
        }
    }
}

/// FL's Mersenne twister (`FUN_007140f0`, `FUN_00714200`).
#[derive(Debug, Clone, PartialEq)]
struct Twister {
    state: Box<[u32; 624]>,
    index: usize,
}

impl Twister {
    fn new(seed: u32) -> Self {
        let mut state = Box::new([0u32; 624]);
        state[0] = seed;
        for i in 1..624 {
            let prev = state[i - 1];
            state[i] = (prev ^ (prev >> 30))
                .wrapping_mul(0x6c07_8965)
                .wrapping_add(i as u32);
        }
        Twister { state, index: 624 }
    }

    fn next(&mut self) -> u32 {
        if self.index >= 624 {
            let s = &mut self.state;
            for i in 0..624 {
                let y = (s[i] & 0x8000_0000) | (s[(i + 1) % 624] & 0x7fff_ffff);
                let mut v = s[(i + 397) % 624] ^ (y >> 1);
                if y & 1 != 0 {
                    v ^= 0x9908_b0df;
                }
                s[i] = v;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }
}

/// The 65536-step half sine of Blood Overdrive's shaper (`FUN_0071e540`).
static SINE: LazyLock<Box<[f32]>> = LazyLock::new(|| {
    let mut table = vec![0.0; 0x1_0000];
    for i in 0..0x8000 {
        let value = (i as f64 * PI / 65_536.0).sin() as f32;
        table[0x7fff - i] = -value;
        table[0x8000 + i] = value;
    }
    table.into_boxed_slice()
});

/// Blood Overdrive's shaper (`FUN_007201f0`): `x` in 32767ths, cut
/// towards 0, held at the table's ends.
fn blood_shape(x: f32) -> f32 {
    let steps = f64::from(x * 32_767.0);
    let steps = if steps.abs() < 9.2e18 {
        steps as i64 as i32
    } else {
        0
    };
    SINE[steps.wrapping_add(0x8000).clamp(0, 0xffff) as usize]
}

/// Blood Overdrive in the module (`FUN_0071efe0`, setter `FUN_007203a0`).
/// Its four smoothers take `0.00025` of a target a frame, with the target
/// times that worked out once.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Blood {
    /// The wheels over 10000, and x100 as saved.
    band: f32,
    colour: f32,
    amp: f32,
    post: f32,
    filter: f32,
    x100: f32,
    /// The targets times 0.00025: gain, blend, post, colour.
    take: [f32; 4],
    /// The post filter's `1 - a` and `a`.
    lp: [f32; 2],
    /// The smoothed gain, blend, post and colour.
    live: [f32; 4],
    /// A side: low, band, low2, band2, lp1, lp2.
    sides: [[f32; 6]; 2],
}

impl Default for Blood {
    fn default() -> Self {
        Blood {
            band: 0.0,
            colour: 0.0,
            amp: 0.0,
            post: 0.0,
            filter: 0.0,
            x100: 0.0,
            take: [0.0; 4],
            lp: [0.8, 0.2],
            live: [1.0, 1.0, 1.0, 0.0],
            sides: [[0.0; 6]; 2],
        }
    }
}

impl Blood {
    const KEEP: f32 = 0.999_75;
    const TAKE: f32 = 0.000_25;
    const CEILING: f32 = 0.999_95;

    fn targets(&mut self) {
        let amp3 = self.amp * self.amp * self.amp;
        let times = if f64::from(self.x100) <= 0.5 {
            25.0
        } else {
            2500.0
        };
        self.take = [
            (times * amp3 + 1.0) * Self::TAKE,
            (1.0 - self.band) * Self::TAKE,
            self.post * self.post * Self::TAKE,
            self.colour * self.colour * self.colour * Self::TAKE,
        ];
        let a = self.filter * 0.7 + 0.2;
        self.lp = [1.0 - a, a];
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            for (live, take) in self.live.iter_mut().zip(self.take) {
                *live = *live * Self::KEEP + take;
            }
            let [gain, blend, post, colour] = self.live;
            for (side, x) in [l, r].into_iter().enumerate() {
                let [low, band, low2, band2, lp1, lp2] = &mut self.sides[side];
                *low += colour * *band;
                let high = *x - *low - *band;
                *band += colour * high;
                *low2 += *band2 * 0.25;
                let high2 = ((high + *low) * blend + *band) * gain - *low2 - *band2;
                *band2 += high2 * 0.25;
                let shaped = blood_shape(*low2) + blood_shape(*band2) + blood_shape(high2);
                let y = (shaped * post).clamp(-Self::CEILING, Self::CEILING);
                *lp1 = y * self.lp[0] + *lp1 * self.lp[1];
                *lp2 = *lp1 * self.lp[0] + *lp2 * self.lp[1];
                *x = *lp2;
            }
        }
        for v in self.sides.as_flattened_mut() {
            if v.is_subnormal() {
                *v = 0.0;
            }
        }
    }
}

/// The band pass on Abrasive's noise: FL's TPT state-variable filter
/// (`FUN_00716690` type 2, run in `FUN_0071ea60`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct NoiseFilter {
    a: [f32; 3],
    ic: [f32; 2],
}

impl NoiseFilter {
    fn design(&mut self, hz: f32, pi_over_rate: f32, k: f32) {
        let g = f64::from(hz * pi_over_rate).tan() as f32;
        let a1 = 1.0 / (g * (g + k) + 1.0);
        let a2 = g * a1;
        self.a = [a1, a2, g * a2];
    }

    fn run(&mut self, x: f32) -> f32 {
        let [a1, a2, a3] = self.a;
        let [ic1, ic2] = &mut self.ic;
        let v3 = x - *ic2;
        let v1 = a1 * *ic1 + a2 * v3;
        let v2 = *ic2 + a2 * *ic1 + a3 * v3;
        *ic1 = v1 * 2.0 - *ic1;
        *ic2 = v2 * 2.0 - *ic2;
        v1
    }
}

/// Crusher's two parts (`FUN_00721c70`): a sample and hold whose hold the
/// rate wheel sets, then a quantiser.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Crusher {
    /// A side's count and the sample it holds.
    count: [f32; 2],
    held: [f32; 2],
}

impl Crusher {
    /// The hold in samples for a rate amount (`FUN_007218f0`).
    fn hold(amount: f32, ratio: f32) -> f32 {
        let f = f64::from(amount) * 127.0;
        let f = f64::from(f as f32);
        let samples = 0.0002 * f * f * f - 0.0171 * f * f + 0.7435 * f + 1.0;
        let hold = samples as f32 * ratio;
        if f64::from(hold) < 1.0 { 1.0 } else { hold }
    }

    /// The quantiser's levels for a bit amount (`FUN_00721c00`).
    fn levels(amount: f32) -> f32 {
        let q = f64::from(amount).sqrt().sqrt();
        2f64.powf((1.0 - q) * 14.0 + 2.0) as f32
    }

    fn step(&mut self, side: usize, x: f32, rate: f32, hold: f32, bits: f32, levels: f32) -> f32 {
        let mut x = x;
        if rate > 0.0 {
            self.count[side] = (f64::from(self.count[side]) + 1.0) as f32;
            if self.count[side] >= hold {
                self.count[side] = fmod32(self.count[side], hold);
                self.held[side] = x;
            }
            x = self.held[side];
        }
        if bits > 0.0 {
            let v = f64::from(x * levels);
            // Delphi's Int: towards 0, and 0 under 1 in size.
            let v = if v.abs() < 1.0 { 0.0 } else { v.trunc() };
            x = v as f32 / levels;
        }
        x
    }
}

/// Harmor's shape for its mode, on one value. Ribbon's needs the bias
/// ramp and is in [`Distortion::run_harmor`].
fn harmor_shape(mode: i32, jam: [f32; 2], x: f32) -> f32 {
    let tanh = |v: f32| f64::from(v).tanh() as f32;
    match mode {
        1 => tanh(x),
        2 => f64::from(tanh(x)).exp() as f32 - 1.0,
        3 => tanh(f64::from(x).exp() as f32 - 1.0),
        4 => {
            let t = tanh(x);
            t * t * t
        }
        5 => (f64::from(x.abs() + 1.0).ln() as f32) * sign(x),
        6 => f64::from(tanh(x * x * x)).cbrt() as f32,
        7 => (f64::from(f64::from(x.abs()).sqrt() as f32).sin() as f32) * sign(x),
        9 => f64::from(x).sin() as f32,
        10 => x.round_ties_even(),
        _ => {
            let level = f64::from(x.abs() + 1.0).ln() as f32;
            let q = (level * jam[0]).round_ties_even() * jam[1];
            (f64::from(q).exp_m1() as f32) * sign(x)
        }
    }
}

/// The distortion section (`TDistortionProcessor`, setter
/// `FUN_00720860`, process `FUN_00720150`).
struct Distortion {
    algo: i32,
    params: [i32; 36],
    /// The rate the section runs at, and how many times the song's.
    rate: f64,
    factor: u32,
    blood: Blood,
    fast: Box<FastDist>,
    soft: SoftClipper,
    /// Harmor's: the dry mix, the drive and the wet level; and the bias
    /// and the wave folder's and Aperture's warp.
    mix: Ramp,
    drive: Ramp,
    wet: Ramp,
    bias: Ramp,
    warp: Ramp,
    /// Whether a Harmor mode adds its bias (`+0x40464`).
    biased: bool,
    /// Bitjam's step and its inverse.
    jam: [f32; 2],
    /// The cut wheel's two low passes, and the frequency it asks.
    cut_on: bool,
    cut_hz: f32,
    cut: [Biquad; 2],
    noise: Twister,
    noise_filter: NoiseFilter,
    crusher: Crusher,
    /// Scratch for a block at the section's rate.
    dry: [Vec<f32>; 2],
    ramp: Vec<f32>,
    shaped: [Vec<f32>; 2],
    grains: Vec<f32>,
}

impl Distortion {
    fn new() -> Self {
        let buffer = || vec![0.0; WIDE];
        let mut d = Distortion {
            algo: 0,
            params: [0; 36],
            rate: 88_200.0,
            factor: 2,
            blood: Blood::default(),
            fast: Box::default(),
            soft: SoftClipper::default(),
            mix: Ramp::default(),
            drive: Ramp::default(),
            wet: Ramp::default(),
            bias: Ramp::default(),
            warp: Ramp::default(),
            biased: false,
            jam: [1.0; 2],
            cut_on: false,
            cut_hz: 0.0,
            cut: [Biquad::default(); 2],
            noise: Twister::new(0x012b_d6aa),
            noise_filter: NoiseFilter::default(),
            crusher: Crusher::default(),
            dry: [buffer(), buffer()],
            ramp: buffer(),
            shaped: [buffer(), buffer()],
            grains: buffer(),
        };
        d.params.copy_from_slice(&DEFAULTS[..36]);
        d
    }

    /// Harmor's mode, 1 to 11, for algorithms 3 to 13.
    fn mode(&self) -> i32 {
        self.algo - 2
    }

    /// The rate and the stages (`FUN_00720610`): the smoothers' 15 ms and
    /// the cut filter at the new rate.
    fn set_rate(&mut self, hz: f64, stages: usize) {
        self.factor = 1 << stages;
        self.rate = hz * f64::from(self.factor);
        let keep = (ln_hundredth() / (f64::from(self.factor * 15) * 0.001 * hz)).exp() as f32;
        for ramp in [
            &mut self.bias,
            &mut self.warp,
            &mut self.drive,
            &mut self.mix,
            &mut self.wet,
        ] {
            ramp.keep = keep;
        }
        self.design_cut(hz);
    }

    /// The cut wheel's two low passes (`FUN_007204d0`).
    fn design_cut(&mut self, hz: f64) {
        let mut most = (0.4999 * hz) as f32 as f64 * f64::from(self.factor);
        if self.factor == 1 {
            most -= 45.0;
        }
        if most < f64::from(self.cut_hz) {
            self.cut_hz = most as f32;
        }
        let w = f64::from(self.cut_hz) * (2.0 * PI / hz / f64::from(self.factor));
        for f in &mut self.cut {
            f.design(w);
        }
    }

    /// Set the algorithm and send every parameter again, as `FUN_00720300`
    /// and `FUN_007201b0` do.
    fn set_algo(&mut self, algo: i32, hz: f64) {
        self.algo = algo.clamp(0, 17);
        for index in 0..36 {
            self.apply(index, hz);
        }
    }

    /// What `FUN_00720860` makes of parameter `index`.
    fn apply(&mut self, index: usize, hz: f64) {
        let v = self.params[index];
        let unit = (f64::from(v) / 10_000.0) as f32;
        let centred = (f64::from(v) / 5000.0 - 1.0) as f32;
        match index {
            0..=5 => {
                match index {
                    0 => self.blood.band = unit,
                    1 => self.blood.colour = unit,
                    2 => self.blood.amp = unit,
                    3 => self.blood.post = unit,
                    4 => self.blood.filter = unit,
                    _ => self.blood.x100 = v as f32,
                }
                self.blood.targets();
            }
            7 if self.algo == 14 => self.warp.target = [centred; 2],
            8 if self.algo == 14 => self.bias.target = [centred; 2],
            11 if self.algo == 15 => self.warp.target = [centred; 2],
            12 if self.algo == 15 => self.bias.target = [centred; 2],
            16..=20 => {
                // Fast Dist's pre, threshold, post, mix and type, as the
                // plugin numbers them: pre, threshold, type, mix, post.
                let at = [0, 1, 4, 3, 2][index - 16];
                self.fast.set_param(at, v as f32);
            }
            21 => self.soft.set_param(0, v as f32),
            22 => self.soft.set_param(1, v as f32),
            23..=26 => self.harmor(),
            27 => {
                if v == 1100 {
                    self.cut_on = false;
                } else {
                    if !self.cut_on {
                        self.cut_on = true;
                        for f in &mut self.cut {
                            f.state = [[0.0; 4]; 2];
                        }
                    }
                    let k = (f64::from((v as f32) * 0.01) * LN_2).exp();
                    let base = (LN_2 / 12.0 * -57.0).exp();
                    self.cut_hz = (k * base * 440.0) as f32;
                    self.design_cut(hz);
                }
            }
            _ => {}
        }
    }

    /// Harmor's drive, wet, mix and bias for modes 1 to 11 (the second
    /// half of `FUN_00720860`).
    fn harmor(&mut self) {
        let mode = self.mode();
        if !(1..=11).contains(&mode) {
            return;
        }
        let [drive_v, asym, out, mix] = [23, 24, 25, 26].map(|i| f64::from(self.params[i]));
        let level = (out as f32 * 0.001) as f64;
        let level = if level as f32 == 1.0 {
            1.0
        } else {
            (((level as f32 as f64) * 3f64.ln()).exp() - 1.0) * 0.5
        } as f32;
        let mix = (mix as f32) * 0.001;
        self.mix.target = [mix; 2];
        let w = f64::from(level) * (1.0 - f64::from(mix.abs()));
        self.biased = true;
        let exp2 = |x: f64| (x * LN_2).exp();
        let (drive, wet): (f64, f64) = match mode {
            1..=3 => {
                let k = exp2(f64::from(drive_v as f32 * 0.01) - 4.0);
                (k * PI, w / (k.tanh() * PI))
            }
            4 => {
                let k = exp2(f64::from(drive_v as f32 * 0.01) - 2.0);
                let t = (k * 4.0).tanh();
                (k * PI, w / (t * t * t * PI))
            }
            5 => {
                let k = exp2(drive_v * 0.025 - 5.0);
                let lift = f64::from(drive_v as f32 * 0.001 + 1.0);
                (k, w / ((k + 1.0).ln() * lift))
            }
            6 => {
                let k = exp2(drive_v * 0.005 - 3.0);
                (k * PI, w / (k.tanh() * PI))
            }
            7 => {
                let k = exp2(drive_v * 0.002 - 1.0) * PI;
                (k * k, w / k.sqrt())
            }
            8 => {
                let k = exp2(f64::from(drive_v as f32 * 0.01) - 4.0);
                (k * PI, w / (k.tanh() * PI))
            }
            9 => {
                let g = exp2(asym * 0.004) as f32;
                self.bias.target = [g; 2];
                let d = (exp2(7.0 - drive_v * 0.007) * PI) as f32;
                self.biased = false;
                (f64::from(d), w / f64::from(d * g))
            }
            10 => {
                let d = exp2(9.0 - drive_v * 0.007) as f32;
                (f64::from(d), w / f64::from(d))
            }
            _ => {
                let q = (exp2(asym * 0.02) * 1024.0) as f32;
                let c = exp2(10.0 - drive_v * 0.007) / f64::from(q + 1.0).ln();
                let c = c as f32;
                self.jam = [c, 1.0 / c];
                self.biased = false;
                (f64::from(q), w / f64::from(q))
            }
        };
        self.drive.target = [drive as f32; 2];
        self.wet.target = [wet as f32; 2];
        if self.biased {
            let b = (asym * 0.002) as f32;
            let drive = self.drive.target[0];
            let bias = if mode < 10 {
                if mode == 8 {
                    b * b * drive
                } else {
                    (f64::from(b * b) * 0.75) as f32
                }
            } else {
                b / drive
            };
            self.bias.target = if (b as i32) < 1 {
                [bias, -bias]
            } else {
                [bias, bias]
            };
            self.biased = mode != 8;
        }
    }

    /// Add the bias (`FUN_0071e3a0`) and, for the folder and Aperture, the
    /// warp away from 0 (`FUN_0071e430`).
    fn bias_and_warp(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let mut add = !self.bias.silent();
        if (3..=13).contains(&self.algo) {
            add = add && self.biased;
        }
        if add {
            for (side, x) in [&mut *l, &mut *r].into_iter().enumerate() {
                self.bias.fill(side, &mut self.ramp[..n]);
                for (x, b) in x.iter_mut().zip(&self.ramp) {
                    *x += b;
                }
            }
        }
        if self.algo == 14 || self.algo == 15 {
            for (side, x) in [l, r].into_iter().enumerate() {
                self.warp.fill(side, &mut self.ramp[..n]);
                for (x, w) in x.iter_mut().zip(&self.ramp) {
                    *x += sign(*x) * w;
                }
            }
        }
    }

    /// Harmor's eleven (`FUN_0071f650`): the dry part at the mix, the
    /// bias, the drive, the shape, the wet part on top and the cut.
    fn run_harmor(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let [dl, dr] = &mut self.dry;
        dl[..n].copy_from_slice(l);
        dr[..n].copy_from_slice(r);
        self.mix.scale([&mut dl[..n], &mut dr[..n]]);
        self.bias_and_warp(l, r);
        self.drive.scale([&mut *l, &mut *r]);
        let (mode, jam) = (self.mode(), self.jam);
        for (side, x) in [&*l, &*r].into_iter().enumerate() {
            let shaped = &mut self.shaped[side][..n];
            if mode == 8 {
                self.bias.fill(side, &mut self.ramp[..n]);
                for ((y, x), b) in shaped.iter_mut().zip(x).zip(&self.ramp) {
                    let t = f64::from(*x).tanh() as f32;
                    let a = f64::from(x + b).tanh() as f32;
                    let c = f64::from(x + b + b).tanh() as f32;
                    *y = t * a * c;
                }
            } else {
                for (y, x) in shaped.iter_mut().zip(x) {
                    *y = harmor_shape(mode, jam, *x);
                }
            }
        }
        if mode == 9 {
            let [sl, sr] = &mut self.shaped;
            self.bias.add([&*l, &*r], [&mut sl[..n], &mut sr[..n]]);
        }
        let [sl, sr] = &self.shaped;
        let [dl, dr] = &mut self.dry;
        self.wet
            .add([&sl[..n], &sr[..n]], [&mut dl[..n], &mut dr[..n]]);
        if self.cut_on {
            for f in &mut self.cut {
                f.run(0, &mut dl[..n]);
                f.run(1, &mut dr[..n]);
            }
        }
        l.copy_from_slice(&dl[..n]);
        r.copy_from_slice(&dr[..n]);
    }

    /// The wave folder (`FUN_0071fd40`, `FUN_005fde90`).
    fn run_folder(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.bias_and_warp(l, r);
        let gain = f64::from(self.params[6]) / 500.0 + 1.0;
        let t = f64::from(self.params[9]) * 0.9 / 10_000.0 + 0.1;
        let t = t.abs();
        for x in l.iter_mut().chain(r.iter_mut()) {
            if t <= 0.0 || t > 1.0 || gain == 0.0 {
                *x = 0.0;
                continue;
            }
            let y = f64::from(*x) * gain;
            let p = y.abs() * 0.25 / t;
            let p = p - if p.abs() < 1.0 { 0.0 } else { p.trunc() };
            let tri = if p < 0.25 {
                p * 4.0
            } else if p < 0.75 {
                1.0 - (p - 0.25) * 4.0
            } else {
                (p - 0.75) * 4.0 - 1.0
            };
            let tri = if y < 0.0 { -tri } else { tri };
            *x = (tri * t) as f32;
        }
    }

    /// Aperture (`FUN_0071ee70`, `FUN_005fe170`): what is past the
    /// threshold folds back by it and the offset.
    fn run_aperture(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.bias_and_warp(l, r);
        let p = |i: usize| f64::from(self.params[i]);
        let gain = p(10) / 500.0 + 1.0;
        let t = (p(13) * 0.5 / 10_000.0 + 0.5).abs();
        let offset = p(14) * 2.0 / 10_000.0 - 1.0;
        let wrap = self.params[15] == 0;
        let sign64 = |d: f64| if d == 0.0 { 0.0 } else { 1f64.copysign(d) };
        for x in l.iter_mut().chain(r.iter_mut()) {
            if t <= 0.0 || t > 1.0 || gain == 0.0 {
                *x = 0.0;
                continue;
            }
            let mut d = f64::from(*x) * gain;
            if wrap {
                if d.abs() > t {
                    d = d - ((d / t) as i64 as f64) * t - (t + offset);
                }
            } else {
                let mut inside = false;
                for _ in 0..8 {
                    if d.abs() <= t {
                        inside = true;
                        break;
                    }
                    d = sign64(d) * (d.abs() - (t + offset));
                }
                if !inside && d.abs() > t {
                    d = sign64(d) * t;
                }
            }
            *x = d as f32;
        }
    }

    /// Abrasive (`FUN_0071ebf0`): a gain, noise let through by a gate on
    /// the input and mixed in, a gain and a clip at full scale.
    fn run_abrasive(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let p = |i: usize| f64::from(self.params[i]);
        let noisy = self.params[30] != 0;
        let pre = p(28) / 5000.0 + 1.0;
        let mix = p(30) / 10_000.0;
        let gate = p(29) / 10_000.0;
        let post = p(31) / 5000.0 + 1.0;
        if noisy {
            for g in &mut self.grains[..n] {
                *g = (f64::from(self.noise.next()) * 4.656_612_873_077_393e-10 - 1.0) as f32;
            }
            if self.params[33] == 1 {
                let hz = (2f64.powf(p(32) / 1000.0) * 20.0) as f32;
                let k = (1.0 - 0.5 * 0.99) as f32;
                let pi_over_rate = (PI / self.rate) as f32;
                self.noise_filter.design(hz, pi_over_rate, k);
                for g in &mut self.grains[..n] {
                    *g = self.noise_filter.run(*g);
                }
            }
        }
        let sign64 = |d: f64| if d == 0.0 { 0.0 } else { 1f64.copysign(d) };
        for i in 0..n {
            let grain = if noisy {
                f64::from(self.grains[i])
            } else {
                0.0
            };
            for x in [&mut l[i], &mut r[i]] {
                let raw = f64::from(*x);
                let mut d = raw * pre;
                if noisy {
                    let open = if gate < raw.abs() { sign64(raw) } else { 0.0 };
                    d = d * (1.0 - mix) + d * mix * open * grain;
                }
                d *= post;
                *x = (sign64(d) * d.abs().min(1.0)) as f32;
            }
        }
    }

    /// Crusher (`FUN_0071ef50`).
    fn run_crusher(&mut self, l: &mut [f32], r: &mut [f32]) {
        let rate = (f64::from(self.params[34]) / 10_000.0) as f32;
        let rate = rate * rate * rate;
        let bits = (f64::from(self.params[35]) / 10_000.0) as f32;
        let bits = bits * bits;
        let ratio = (self.rate / 44_100.0) as f32;
        let hold = Crusher::hold(rate, ratio);
        let levels = Crusher::levels(bits);
        for (x, y) in l.iter_mut().zip(r.iter_mut()) {
            *x = self.crusher.step(0, *x, rate, hold, bits, levels);
            *y = self.crusher.step(1, *y, rate, hold, bits, levels);
        }
    }

    /// The selected algorithm on a block at the section's rate.
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        match self.algo {
            0 => self.blood.run(l, r),
            1 => self.fast.process(l, r),
            2 => self.soft.process(l, r),
            3..=13 => self.run_harmor(l, r),
            14 => self.run_folder(l, r),
            15 => self.run_aperture(l, r),
            16 => self.run_abrasive(l, r),
            _ => self.run_crusher(l, r),
        }
    }

    fn reset(&mut self) {
        self.blood.sides = [[0.0; 6]; 2];
        for f in &mut self.cut {
            f.state = [[0.0; 4]; 2];
        }
        self.noise_filter.ic = [0.0; 2];
        self.crusher = Crusher::default();
    }
}

/// The filter section (`TFilterProcessor`, process `FUN_0071cf80`). Built:
/// the one-pole low and high pass and the two combs.
struct Filter {
    kind: i32,
    /// The cutoff and resonance wheels as 0..1.
    cutoff: f32,
    resonance: f32,
    rate: f64,
    /// The one-pole's `b0`, `b1`, `a1`, and a side's last input and output.
    pole: [f32; 3],
    pole_state: [[f32; 2]; 2],
    /// The combs' rings and where the next sample goes.
    rings: [Vec<f32>; 2],
    write: usize,
}

/// The comb's rings (`FUN_00718810`).
const COMB: usize = 0x4000;

impl Filter {
    fn new() -> Self {
        Filter {
            kind: 2,
            cutoff: 1.0,
            resonance: 0.0,
            rate: 44_100.0,
            pole: [0.0; 3],
            pole_state: [[0.0; 2]; 2],
            rings: [vec![0.0; COMB], vec![0.0; COMB]],
            write: 0,
        }
    }

    /// Whether the type is one this port plays.
    fn built(&self) -> bool {
        matches!(self.kind, 0 | 1 | 13 | 14)
    }

    /// The one-pole for the cutoff (`FUN_0071b740`, `FUN_007183c0` low,
    /// `FUN_00718300` high).
    fn design(&mut self) {
        let hz = (2f64.powf(f64::from(self.cutoff * 10.0)) * 20.0) as f32;
        let pi_over_rate = (1.0 / self.rate * PI) as f32;
        let g = f64::from(hz * pi_over_rate).tan() as f32;
        let sum = g + 1.0;
        let a = (-(f64::from(g) - 1.0) / f64::from(sum)) as f32;
        self.pole = if self.kind == 0 {
            [g / sum, g / sum, a]
        } else {
            let b = (1.0 / f64::from(sum)) as f32;
            [b, -b, a]
        };
    }

    /// A ring's sample `back` samples before the next write, between two
    /// (`FUN_00718710`).
    fn read(ring: &[f32], write: usize, back: f32) -> f32 {
        let whole = f64::from(back) as i64;
        let part = back - whole as f32;
        let at = |k: i64| ring[(write as i64 - k) as usize & (COMB - 1)];
        let (a, b) = (at(whole), at(whole + 1));
        let tail = f64::from(b * part);
        (f64::from(a) * (1.0 - f64::from(part)) + tail) as f32
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        match self.kind {
            0 | 1 => {
                self.design();
                let [b0, b1, a] = self.pole;
                for (side, x) in [l, r].into_iter().enumerate() {
                    let [x1, y1] = &mut self.pole_state[side];
                    for v in x.iter_mut() {
                        *y1 = *v * b0 + *x1 * b1 + *y1 * a;
                        *x1 = *v;
                        *v = *y1;
                    }
                }
            }
            13 | 14 => {
                let back = (f64::from(self.cutoff) * self.rate * f64::from(0.05f32) + 1.0) as f32;
                let feedback = (f64::from(self.resonance) * 0.95) as f32;
                let late = self.kind == 14;
                for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                    let w = self.write;
                    if late {
                        *x += Self::read(&self.rings[0], w, back) * feedback;
                        *y += Self::read(&self.rings[1], w, back) * feedback;
                        self.rings[0][w] = *x;
                        self.rings[1][w] = *y;
                    } else {
                        self.rings[0][w] = *x;
                        self.rings[1][w] = *y;
                        let w = w + 1;
                        *x += Self::read(&self.rings[0], w, back) * feedback;
                        *y += Self::read(&self.rings[1], w, back) * feedback;
                    }
                    self.write = (w + 1) & (COMB - 1);
                }
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.pole_state = [[0.0; 2]; 2];
        self.rings.iter_mut().for_each(|ring| ring.fill(0.0));
        self.write = 0;
    }
}

/// The speaker's cabinets (`0x007ae838`): the file in `module/impulses`
/// and the gain after the convolution. Headings are `None`.
const CABINETS: [Option<(&str, f64)>; 59] = [
    None,
    Some(("IMP Cabinet Stereo Blues.wv", 0.026)),
    Some(("IMP Cabinet Stereo Guitaro.wv", 0.1)),
    Some(("IMP Cabinet Stereo Metal.wv", 0.08)),
    Some(("IMP Cabinet Stereo Muffled.wv", 0.058)),
    Some(("IMP Cabinet Stereo Wide.wv", 0.086)),
    Some(("IMP Cabinet Stereo Combo 01.wv", 0.15)),
    Some(("IMP Cabinet Stereo Combo 02.wv", 0.136)),
    Some(("IMP Cabinet Stereo Combo 03.wv", 0.128)),
    Some(("IMP Cabinet Stereo Combo 04.wv", 0.1)),
    Some(("IMP Cabinet Stereo Combo 05.wv", 0.16)),
    Some(("IMP Cabinet Stereo Combo 06.wv", 0.128)),
    Some(("IMP Cabinet Stereo Combo 07.wv", 0.12)),
    Some(("IMP Cabinet Stereo Combo 08.wv", 0.1)),
    Some(("IMP Cabinet Stereo Combo 09.wv", 0.13)),
    Some(("IMP Cabinet Stereo Combo 10.wv", 0.12)),
    Some(("IMP Cabinet Stereo Combo 11.wv", 0.1)),
    Some(("IMP Cabinet Stereo Combo 12.wv", 0.12)),
    Some(("IMP Cabinet Stereo Combo 13.wv", 0.102)),
    Some(("IMP Cabinet Stereo Combo 14.wv", 0.1)),
    None,
    Some(("IMP Cabinet Mono Closest.wv", 0.158)),
    Some(("IMP Cabinet Mono Closer.wv", 0.13)),
    Some(("IMP Cabinet Mono Close.wv", 0.13)),
    Some(("IMP Cabinet Mono Medium.wv", 0.11)),
    Some(("IMP Cabinet Mono Far.wv", 0.096)),
    Some(("IMP Cabinet Mono Farther.wv", 0.088)),
    None,
    Some(("IMP Cabinet Model AB.wv", 0.026)),
    Some(("IMP Cabinet Model CD.wv", 0.13)),
    Some(("IMP Cabinet Model EF.wv", 0.12)),
    Some(("IMP Cabinet Model GH.wv", 0.2)),
    Some(("IMP Cabinet Model IJ.wv", 0.07)),
    Some(("IMP Cabinet Model KL.wv", 0.172)),
    Some(("IMP Cabinet Model MN.wv", 0.068)),
    Some(("IMP Cabinet Model OP.wv", 0.025)),
    Some(("IMP Cabinet Model QR.wv", 0.056)),
    Some(("IMP Cabinet Model ST.wv", 0.024)),
    None,
    Some(("IMP Cabinet Model A.wv", 0.104)),
    Some(("IMP Cabinet Model B.wv", 0.12)),
    Some(("IMP Cabinet Model C.wv", 0.102)),
    Some(("IMP Cabinet Model D.wv", 0.138)),
    Some(("IMP Cabinet Model E.wv", 0.12)),
    Some(("IMP Cabinet Model F.wv", 0.08)),
    Some(("IMP Cabinet Model G.wv", 0.12)),
    Some(("IMP Cabinet Model H.wv", 0.136)),
    Some(("IMP Cabinet Model I.wv", 0.108)),
    Some(("IMP Cabinet Model J.wv", 0.148)),
    Some(("IMP Cabinet Model K.wv", 0.124)),
    Some(("IMP Cabinet Model L.wv", 0.14)),
    Some(("IMP Cabinet Model M.wv", 0.08)),
    Some(("IMP Cabinet Model N.wv", 0.11)),
    Some(("IMP Cabinet Model O.wv", 0.056)),
    Some(("IMP Cabinet Model P.wv", 0.052)),
    Some(("IMP Cabinet Model Q.wv", 0.076)),
    Some(("IMP Cabinet Model R.wv", 0.064)),
    Some(("IMP Cabinet Model S.wv", 0.092)),
    Some(("IMP Cabinet Model T.wv", 0.064)),
];

/// A stereo separation's 2x2 gains (`FUN_005f1f50` at pan 0): the left
/// and right to themselves, and the right into the left and the left into
/// the right.
fn separation(volume: f32, separation: f32) -> [f32; 4] {
    if separation == 0.0 {
        return [volume, volume, 0.0, 0.0];
    }
    if volume == 0.0 {
        return [0.0; 4];
    }
    // The pan law at the centre, `cos(pi/4) * volume * sqrt 2`.
    let scaled = f64::from(volume * std::f32::consts::SQRT_2);
    let (sin, cos) = (PI / 4.0).sin_cos();
    let (l, r) = ((cos * scaled) as f32, (sin * scaled) as f32);
    let f = if separation > 0.0 {
        ((f64::from(separation) * 5f64.ln()).exp() - 1.0) as f32 * 0.25
    } else {
        separation
    };
    let f = f.min(1.0);
    [
        l * (1.0 - f * 0.5),
        r * (1.0 - f * 0.5),
        l * f * 0.5,
        r * f * 0.5,
    ]
}

/// Four gains that glide to their targets, no faster than a step a
/// sample (`FUN_005fed40`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Glide {
    target: [f32; 4],
    now: [f32; 4],
}

impl Glide {
    /// The gains at the block's start and their steps, and where the
    /// next block starts.
    fn block(&mut self, n: usize, most: f32) -> ([f32; 4], [f32; 4]) {
        let mut steps = [0.0; 4];
        let start = self.now;
        for k in 0..4 {
            let step = self.target[k] - start[k];
            if step == 0.0 {
                continue;
            }
            self.now[k] = self.target[k];
            let step = step / n as f32;
            if step.abs() > most {
                let step = most.copysign(step);
                steps[k] = step;
                self.now[k] = start[k] + step * n as f32;
            } else {
                steps[k] = step;
            }
        }
        (start, steps)
    }

    /// `out (+)= the 2x2 of in`, the gains gliding.
    fn apply(&mut self, n: usize, most: f32, input: [&[f32]; 2], out: [&mut [f32]; 2], add: bool) {
        let (mut g, steps) = self.block(n, most);
        let [ol, or] = out;
        for i in 0..n {
            let (l, r) = (input[0][i], input[1][i]);
            let (yl, yr) = (l * g[0] + r * g[2], r * g[1] + l * g[3]);
            if add {
                ol[i] += yl;
                or[i] += yr;
            } else {
                ol[i] = yl;
                or[i] = yr;
            }
            for k in 0..4 {
                g[k] += steps[k];
            }
        }
    }
}

/// The speaker section (`TImpulseProcessor`, process `FUN_00734260`).
struct Speaker {
    impulse: i32,
    engine: Option<Box<convolver::Engine>>,
    /// The impulse's length, and what is left of its tail after the input
    /// fell silent.
    len: i32,
    tail: i32,
    gain: f32,
    inward: Glide,
    outward: Glide,
    rate: f64,
    buffer: [Vec<f32>; 2],
}

impl Speaker {
    fn new() -> Self {
        Speaker {
            impulse: 1,
            engine: None,
            len: 0,
            tail: 0,
            gain: 0.1,
            inward: Glide::default(),
            outward: Glide::default(),
            rate: 44_100.0,
            buffer: [vec![0.0; BLOCK], vec![0.0; BLOCK]],
        }
    }

    /// Read the cabinet's file from FL's install. This asks for memory.
    fn load(&mut self, effects: &Path) {
        self.engine = None;
        let Some(Some((file, gain))) = CABINETS.get(self.impulse.clamp(0, 58) as usize) else {
            return;
        };
        let path = effects.join("Distructor/module/impulses").join(file);
        let Some((left, right)) = std::fs::read(path)
            .ok()
            .and_then(|bytes| convolver::decode_impulse(&bytes))
        else {
            return;
        };
        self.gain = *gain as f32;
        self.len = left.len() as i32;
        self.tail = self.len;
        self.engine = Some(Box::new(convolver::Engine::new(&left, &right)));
    }

    /// The separations, in and out (`FUN_00734680`).
    fn set_separation(&mut self, inward: i32, outward: i32) {
        self.inward.target = separation(1.0, (inward as f32) * 0.001);
        let volume = (44_100.0 / self.rate) as f32;
        self.outward.target = separation(volume, (outward as f32) * 0.001);
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let Some(engine) = &mut self.engine else {
            return;
        };
        let n = l.len();
        let silent = l.iter().chain(r.iter()).all(|x| x.abs() <= 1e-6);
        if silent {
            self.tail -= n as i32;
        } else {
            self.tail = self.len;
        }
        if self.tail + n as i32 <= 0 {
            self.tail = 0;
            self.inward.now = self.inward.target;
            self.outward.now = self.outward.target;
            return;
        }
        let most = (44_100.0 / self.rate * 0.001) as f32;
        let [bl, br] = &mut self.buffer;
        self.inward
            .apply(n, most, [&*l, &*r], [&mut bl[..n], &mut br[..n]], false);
        engine.process(0, &mut bl[..n]);
        engine.process(1, &mut br[..n]);
        l.fill(0.0);
        r.fill(0.0);
        self.outward
            .apply(n, most, [&bl[..n], &br[..n]], [&mut *l, &mut *r], true);
        for x in l.iter_mut().chain(r.iter_mut()) {
            *x *= self.gain;
        }
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset();
        }
        self.tail = self.len;
    }
}

/// The chorus modes' Delay 3 model and Time R (`FUN_0074b5a0`): Wide, Mono
/// and Stereo.
const CHORUS_MODES: [(f32, f32); 3] = [(0.33, 0.84), (0.0, 0.5), (0.66, 0.5)];

/// What the chorus sets the Delay 3 engine to before its wheels
/// (`0x007adba0`).
const CHORUS_ENGINE: [f32; 26] = [
    1.0,
    0.33,
    0.0,
    1.0,
    0.067_796_61,
    0.84,
    0.5,
    1.0,
    0.5,
    0.5,
    0.0,
    0.0,
    0.0,
    1.0,
    0.16,
    0.0,
    1.0,
    0.0,
    0.0,
    0.0,
    0.0,
    1.0,
    1.0,
    1.0,
    0.5,
    0.0,
];

/// The engine's values for a chorus mode and the six chorus wheels
/// (`FUN_0074b3f0`), with the model and the switches as the whole numbers
/// the engine makes of them (`int((n - 1) v + 0.5)`).
fn chorus_engine(mode: i32, wheels: &[i32]) -> [f32; 26] {
    let mut v = CHORUS_ENGINE;
    let (model, time_r) = CHORUS_MODES[mode.clamp(0, 2) as usize];
    v[1] = model;
    v[5] = time_r;
    let w = |i: usize, over: f64| (f64::from(wheels[i]) / over) as f32;
    v[4] = w(0, 768.0);
    v[14] = w(1, 6000.0);
    v[8] = w(2, 6000.0);
    v[9] = w(3, 6000.0);
    v[11] = w(4, 6000.0);
    v[12] = w(4, 6000.0);
    v[24] = ((f64::from(wheels[5]) + 3000.0) / 6000.0) as f32;
    for (index, steps) in [(1, 4.0), (2, 2.0), (3, 2.0), (13, 2.0), (15, 4.0)] {
        let x = v[index].clamp(0.0, 1.0);
        v[index] = ((steps - 1.0) * x + 0.5).trunc();
    }
    v
}

/// A module (`TTestPlug` of `Module_x64.dll`, render `FUN_007504b0`).
struct Module {
    kind: i32,
    stages: usize,
    center: bool,
    bypass: bool,
    params: [i32; PARAMS],
    chorus_mode: i32,
    hz: f64,
    up: [[HalfBand; 2]; MAX_STAGES],
    down: [[HalfBand; 2]; MAX_STAGES],
    /// A side's DC remover: its speed and its level.
    dc: [[f64; 2]; 2],
    dist: Box<Distortion>,
    filter: Box<Filter>,
    chorus: Option<Box<Delay3>>,
    speaker: Box<Speaker>,
    /// The block at the module's rate, what it had before the section,
    /// and the stages' scratch.
    wide: [Vec<f32>; 2],
    keep: [Vec<f32>; 2],
    scratch: Vec<f32>,
}

impl Module {
    fn new(hz: f64) -> Self {
        let buffer = || vec![0.0; WIDE];
        let mut module = Module {
            kind: DISTORTION,
            stages: 1,
            center: true,
            bypass: false,
            params: DEFAULTS,
            chorus_mode: 0,
            hz,
            up: Default::default(),
            down: Default::default(),
            dc: [[0.0; 2]; 2],
            dist: Box::new(Distortion::new()),
            filter: Box::new(Filter::new()),
            chorus: None,
            speaker: Box::new(Speaker::new()),
            wide: [buffer(), buffer()],
            keep: [buffer(), buffer()],
            scratch: buffer(),
        };
        module.prepare();
        module
    }

    /// The stages the module runs: the distortion and the filter
    /// oversample, the filter by 4 at most (`FUN_0074e6b0`).
    fn running_stages(&self) -> usize {
        match self.kind {
            DISTORTION => self.stages,
            FILTER => self.stages.min(2),
            _ => 0,
        }
    }

    /// Work out what the sections read from the parameters, at the rate.
    /// A chorus module's engine is made here, which asks for memory.
    fn prepare(&mut self) {
        let hz = self.hz;
        let stages = self.running_stages();
        let factor = f64::from(1u32 << stages);
        self.dist.params.copy_from_slice(&self.params[..36]);
        self.dist.set_rate(hz, self.stages);
        let algo = self.dist.algo;
        self.dist.set_algo(algo, hz);
        self.filter.rate = hz * factor;
        self.filter.cutoff = (f64::from(self.params[39]) / 10_000.0).clamp(0.0, 1.0) as f32;
        self.filter.resonance = (f64::from(self.params[40]) / 10_000.0).clamp(0.0, 1.0) as f32;
        self.speaker.rate = hz;
        self.speaker
            .set_separation(self.params[41], self.params[42]);
        if self.kind == CHORUS {
            let chorus = self.chorus.get_or_insert_with(Box::default);
            chorus.set_sample_rate(hz as f32);
        }
        if let Some(chorus) = &mut self.chorus {
            chorus.set_engine(chorus_engine(self.chorus_mode, &self.params[43..49]));
        }
    }

    /// The DC remover's speed at the rate (`Dispatcher` 4).
    fn dc_speed(&self) -> f64 {
        f64::from((44_100.0 / self.hz * 2.2835e-5) as f32)
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if self.bypass {
            return;
        }
        let n = l.len();
        let stages = self.running_stages();
        let wide = n << stages;
        let mix_in = (f64::from(self.params[36]) / 5000.0) as f32;
        let dry_wet = f64::from(self.params[37]) / 10_000.0;
        let mix_out = (f64::from(self.params[38]) / 5000.0) as f32;

        // Up, the first stage at the song's rate (`FUN_00714ba0`).
        for (side, input) in [&*l, &*r].into_iter().enumerate() {
            let buf = &mut self.wide[side];
            buf[..n].copy_from_slice(input);
            let mut len = n;
            for k in 0..stages {
                let stage = &mut self.up[stages - 1 - k][side];
                self.scratch[..len].copy_from_slice(&buf[..len]);
                stage.up(&self.scratch[..len], &mut buf[..2 * len]);
                stage.flush();
                len *= 2;
            }
            for x in &mut buf[..wide] {
                *x *= mix_in;
            }
            self.keep[side][..wide].copy_from_slice(&buf[..wide]);
        }
        {
            let [wl, wr] = &mut self.wide;
            let (wl, wr) = (&mut wl[..wide], &mut wr[..wide]);
            match self.kind {
                DISTORTION => self.dist.process(wl, wr),
                FILTER => self.filter.process(wl, wr),
                CHORUS => {
                    if let Some(chorus) = &mut self.chorus {
                        chorus.process(wl, wr);
                    }
                }
                _ => self.speaker.process(wl, wr),
            }
        }
        let dry = (1.0 - dry_wet) as f32;
        let wet = dry_wet as f32;
        for (side, out) in [l, r].into_iter().enumerate() {
            let buf = &mut self.wide[side];
            for (y, k) in buf[..wide].iter_mut().zip(&self.keep[side]) {
                *y = (0.0 + k * dry + *y * wet) * mix_out;
            }
            // Down, the first stage at the highest rate (`FUN_00714c40`).
            let mut len = wide;
            for k in 0..stages {
                let stage = &mut self.down[stages - 1 - k][side];
                self.scratch[..len].copy_from_slice(&buf[..len]);
                stage.down(&self.scratch[..len], &mut buf[..len / 2]);
                stage.flush();
                len /= 2;
            }
            out.copy_from_slice(&buf[..n]);
            if self.kind == DISTORTION && self.center {
                let a = self.dc_speed();
                let [speed, level] = &mut self.dc[side];
                for x in out.iter_mut() {
                    let v = f64::from(*x);
                    *speed += (v - *level) * a;
                    *level += *speed;
                    *speed *= f64::from(0.96f32);
                    *x = (v - *level) as f32;
                }
            }
        }
    }

    fn reset(&mut self) {
        self.up = Default::default();
        self.down = Default::default();
        self.dc = [[0.0; 2]; 2];
        self.dist.reset();
        self.filter.reset();
        self.speaker.reset();
        if let Some(chorus) = &mut self.chorus {
            chorus.reset();
        }
    }

    /// Whether the module, as set, plays what FL's plays.
    fn fidelity(&self) -> Fidelity {
        if self.bypass {
            return Fidelity::Exact;
        }
        match self.kind {
            FILTER if !self.filter.built() => Fidelity::Approximate(UNBUILT_FILTER),
            SPEAKER if self.speaker.engine.is_none() => Fidelity::Approximate(NO_IMPULSE),
            _ => Fidelity::Exact,
        }
    }
}

/// A module as its state has it.
#[derive(Debug, Clone, PartialEq)]
struct Saved {
    kind: i32,
    algo: i32,
    stages: i32,
    center: bool,
    bypass: bool,
    filter_type: i32,
    impulse: i32,
    params: [i32; PARAMS],
    chorus_mode: i32,
}

/// Bytes read from the front of a state.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], StateError> {
        let end = self.at + n;
        let slice = self.bytes.get(self.at..end).ok_or(StateError::Length {
            expected: end,
            found: self.bytes.len(),
        })?;
        self.at = end;
        Ok(slice)
    }

    fn i32(&mut self) -> Result<i32, StateError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn byte(&mut self) -> Result<u8, StateError> {
        Ok(self.take(1)?[0])
    }

    /// A module's state (`FUN_0074ccd0`).
    fn module(&mut self) -> Result<Saved, StateError> {
        let version = self.i32()?;
        if version > MODULE_VERSION {
            return Err(StateError::Version { found: version });
        }
        let kind = self.i32()?;
        let algo = self.i32()?;
        let stages = self.i32()?;
        let center = self.byte()? != 0;
        let bypass = self.byte()? != 0;
        let filter_type = self.i32()?;
        let _smooth = self.byte()?;
        let impulse = self.i32()?;
        let mut params = [0; PARAMS];
        for p in &mut params {
            *p = self.i32()?;
        }
        if version >= 2 {
            self.i32()?;
        }
        let chorus_mode = if version >= 4 { self.i32()? } else { 0 };
        Ok(Saved {
            kind,
            algo,
            stages,
            center,
            bypass,
            filter_type,
            impulse,
            params,
            chorus_mode,
        })
    }
}

/// A rack's state (`FUN_006a7330`): the modules and the output's soft
/// clipper.
fn read(bytes: &[u8]) -> Result<(Vec<Saved>, bool), StateError> {
    let mut reader = Reader { bytes, at: 0 };
    let version = reader.i32()?;
    if version > VERSION {
        return Err(StateError::Version { found: version });
    }
    let count = reader.i32()?;
    if !(0..=MODULES as i32).contains(&count) {
        return Err(StateError::Range {
            param: 0,
            value: count,
        });
    }
    let mut modules = Vec::with_capacity(MODULES);
    for _ in 0..count {
        modules.push(reader.module()?);
    }
    let soft_clip = reader.byte()? != 0;
    if version >= 4 {
        let panels = reader.i32()?;
        for _ in 0..panels.max(0) {
            reader.i32()?;
        }
    }
    if version >= 2 {
        reader.i32()?;
    }
    if version >= 5 {
        reader.byte()?;
    }
    if reader.at != bytes.len() {
        return Err(StateError::Length {
            expected: reader.at,
            found: bytes.len(),
        });
    }
    Ok((modules, soft_clip))
}

/// Distructor. A new one has four modules, a distortion, a filter, a
/// chorus and a speaker, each as FL makes it, and the soft clipper on.
pub struct Distructor {
    modules: Vec<Module>,
    soft_clip: bool,
    hz: f64,
    /// FL's `Plugins/Fruity/Effects`, for the speaker's cabinets.
    effects: Option<PathBuf>,
}

impl Default for Distructor {
    fn default() -> Self {
        LazyLock::force(&SINE);
        let hz = 44_100.0;
        let modules = (0..MODULES as i32)
            .map(|kind| {
                let mut module = Module::new(hz);
                module.kind = kind;
                module.prepare();
                module
            })
            .collect();
        Distructor {
            modules,
            soft_clip: true,
            hz,
            effects: None,
        }
    }
}

impl Distructor {
    fn load_impulses(&mut self) {
        let Some(effects) = &self.effects else {
            return;
        };
        for module in &mut self.modules {
            if module.kind == SPEAKER {
                module.speaker.load(effects);
            }
        }
    }
}

impl Effect for Distructor {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let (saved, soft_clip) = read(bytes)?;
        let hz = self.hz;
        let mut modules = Vec::with_capacity(MODULES);
        for s in saved {
            let mut module = Module::new(hz);
            module.kind = s.kind.clamp(0, 3);
            module.stages = s.stages.clamp(0, MAX_STAGES as i32) as usize;
            module.center = s.center;
            module.bypass = s.bypass;
            module.params = s.params;
            module.chorus_mode = s.chorus_mode;
            module.dist.algo = s.algo.clamp(0, 17);
            module.filter.kind = s.filter_type.clamp(0, 21);
            module.speaker.impulse = s.impulse.clamp(0, 58);
            module.prepare();
            modules.push(module);
        }
        self.modules = modules;
        self.soft_clip = soft_clip;
        self.load_impulses();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
            for module in &mut self.modules {
                module.hz = self.hz;
                module.prepare();
                module.reset();
            }
        }
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        // `dir` is `Plugins/Fruity/Generators`; the cabinets are beside it.
        self.effects = dir.parent().map(|fruity| fruity.join("Effects"));
        self.load_impulses();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let chunks = left[..len]
            .chunks_mut(BLOCK)
            .zip(right[..len].chunks_mut(BLOCK));
        for (l, r) in chunks {
            for module in &mut self.modules {
                module.process(l, r);
            }
            if self.soft_clip {
                let (a, b) = knee(OUT_KNEE);
                for x in l.iter_mut().chain(r.iter_mut()) {
                    *x = soft_clip(*x, OUT_KNEE, a, b);
                }
            }
        }
    }

    /// A module's 49 parameters after the one before it's, as FL numbers
    /// them.
    fn param_count(&self) -> usize {
        MODULES * PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        let module = self.modules.get(index / PARAMS);
        module.map_or(0.0, |m| m.params[index % PARAMS] as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(module) = self.modules.get_mut(index / PARAMS) else {
            return;
        };
        let at = index % PARAMS;
        module.params[at] = crate::knob_value(value, RANGES[at]);
        let hz = module.hz;
        if at < 36 {
            module.dist.params[at] = module.params[at];
            module.dist.apply(at, hz);
        } else {
            match at {
                39 | 40 => {
                    let v = |p: i32| (f64::from(p) / 10_000.0).clamp(0.0, 1.0) as f32;
                    module.filter.cutoff = v(module.params[39]);
                    module.filter.resonance = v(module.params[40]);
                }
                41 | 42 => module
                    .speaker
                    .set_separation(module.params[41], module.params[42]),
                43..=48 => {
                    let values = chorus_engine(module.chorus_mode, &module.params[43..49]);
                    if let Some(chorus) = &mut module.chorus {
                        chorus.set_engine(values);
                    }
                }
                _ => {}
            }
        }
    }

    fn reset(&mut self) {
        for module in &mut self.modules {
            module.reset();
        }
    }

    fn fidelity(&self) -> Fidelity {
        let approximate = self.modules.iter().map(Module::fidelity);
        approximate
            .into_iter()
            .find(|f| *f != Fidelity::Exact)
            .unwrap_or(Fidelity::Exact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, presets, projects, sine};

    /// A state of the rack with these modules, the soft clipper as asked.
    fn state(modules: &[Saved], soft_clip: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(5i32.to_le_bytes());
        out.extend((modules.len() as i32).to_le_bytes());
        for m in modules {
            for v in [4, m.kind, m.algo, m.stages] {
                out.extend(v.to_le_bytes());
            }
            out.extend([u8::from(m.center), u8::from(m.bypass)]);
            out.extend(m.filter_type.to_le_bytes());
            out.push(0);
            out.extend(m.impulse.to_le_bytes());
            for p in m.params {
                out.extend(p.to_le_bytes());
            }
            out.extend(0i32.to_le_bytes());
            out.extend(m.chorus_mode.to_le_bytes());
        }
        out.push(u8::from(soft_clip));
        out.extend(4i32.to_le_bytes());
        for i in 0..4i32 {
            out.extend(i.to_le_bytes());
        }
        out.extend(0i32.to_le_bytes());
        out.push(1);
        out
    }

    fn module(kind: i32, algo: i32) -> Saved {
        Saved {
            kind,
            algo,
            stages: 0,
            center: false,
            bypass: false,
            filter_type: 0,
            impulse: 1,
            params: DEFAULTS,
            chorus_mode: 0,
        }
    }

    fn run(plugin: &mut Distructor, input: &[f32]) -> Vec<f32> {
        let (mut l, mut r) = (input.to_vec(), input.to_vec());
        plugin.process(&mut l, &mut r);
        assert_eq!(l, r);
        l
    }

    #[test]
    fn the_output_knee_is_soft_clipper_s_at_100() {
        let (a, b) = knee(OUT_KNEE);
        assert_eq!(soft_clip(0.5, OUT_KNEE, a, b), 0.5);
        assert_eq!(soft_clip(OUT_KNEE, OUT_KNEE, a, b), OUT_KNEE);
        // Its slope is 1 at the knee and it never gets to full scale.
        let just = soft_clip(0.782, OUT_KNEE, a, b);
        assert!((just - 0.782).abs() < 1e-5, "{just}");
        assert!(soft_clip(100.0, OUT_KNEE, a, b) <= 1.0);
        assert_eq!(
            soft_clip(-2.0, OUT_KNEE, a, b),
            -soft_clip(2.0, OUT_KNEE, a, b)
        );
    }

    #[test]
    fn an_empty_rack_without_the_clipper_passes() {
        let mut plugin = Distructor::default();
        plugin.set_state(&state(&[], false)).unwrap();
        let input = sine(0.01, 1000);
        assert_eq!(run(&mut plugin, &input), input);
    }

    #[test]
    fn a_dry_module_at_unity_passes() {
        // Mix in and out at 5000 are 1, dry/wet at 0 keeps the dry part.
        let mut m = module(DISTORTION, 17);
        m.params[37] = 0;
        let mut plugin = Distructor::default();
        plugin.set_state(&state(&[m], false)).unwrap();
        let input: Vec<f32> = sine(0.01, 1000).iter().map(|x| x * 0.5).collect();
        assert_eq!(run(&mut plugin, &input), input);
    }

    #[test]
    fn the_crusher_holds_by_its_polynomial() {
        // At 44.1 kHz and the top of the wheel, 127 steps: about 70 samples.
        let hold = Crusher::hold(1.0, 1.0);
        let f = 127.0f64;
        let expected = 0.0002 * f * f * f - 0.0171 * f * f + 0.7435 * f + 1.0;
        assert!((f64::from(hold) - expected).abs() < 1e-3, "{hold}");
        assert_eq!(Crusher::hold(0.0, 1.0), 1.0);
        // A crusher at a hold of 3 repeats every third sample.
        let mut c = Crusher::default();
        let out: Vec<f32> = (1..=7)
            .map(|i| c.step(0, i as f32, 1.0, 3.0, 0.0, 1.0))
            .collect();
        assert_eq!(out, [0.0, 0.0, 3.0, 3.0, 3.0, 6.0, 6.0]);
    }

    #[test]
    fn aperture_folds_back_past_its_threshold() {
        let mut d = Distortion::new();
        d.params[10] = 0; // gain 1
        d.params[13] = 0; // threshold 0.5
        d.params[14] = 5000; // offset 0
        d.params[15] = 1; // the iterating fold
        d.algo = 15;
        let (mut l, mut r) = (vec![0.3, 0.7, -0.7, 1.4], vec![0.0; 4]);
        d.run_aperture(&mut l, &mut r);
        // 0.7 is 0.2 past 0.5 and comes back to 0.2; 1.4 twice to 0.4.
        let expected = [0.3, 0.2, -0.2, 0.4];
        for (y, e) in l.iter().zip(expected) {
            assert!((y - e).abs() < 1e-6, "{l:?}");
        }
    }

    #[test]
    fn the_comb_echoes_at_its_delay() {
        let mut f = Filter::new();
        f.kind = 14;
        f.rate = 44_100.0;
        // 0.5 of the wheel: 1103.5 samples back.
        f.cutoff = 0.5;
        f.resonance = 1.0;
        let n = 2048;
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        l[0] = 1.0;
        f.process(&mut l, &mut r);
        let peak = (1..n)
            .max_by(|a, b| l[*a].abs().total_cmp(&l[*b].abs()))
            .unwrap();
        assert!((1103..=1104).contains(&peak), "{peak}");
        assert!((l[1103] + l[1104] - 0.95).abs() < 1e-3);
    }

    #[test]
    fn the_one_pole_passes_low_and_stops_high() {
        let mut f = Filter::new();
        f.kind = 0;
        f.rate = 44_100.0;
        // 2^5 * 20 = 640 Hz.
        f.cutoff = 0.5;
        let level = |f: &mut Filter, turns: f64| {
            f.reset();
            let input = sine(turns, 8820);
            let (mut l, mut r) = (input.clone(), input);
            f.process(&mut l, &mut r);
            l[4410..].iter().fold(0f32, |m, x| m.max(x.abs()))
        };
        assert!(level(&mut f, 64.0 / 44_100.0) > 0.99);
        assert!(level(&mut f, 6400.0 / 44_100.0) < 0.12);
    }

    #[test]
    fn the_harmor_shapes_are_harmor_s() {
        let mut d = Distortion::new();
        d.algo = 5 + 2; // Log
        assert!((harmor_shape(d.mode(), d.jam, 1.0) - 2f32.ln()).abs() < 1e-6);
        assert!((harmor_shape(d.mode(), d.jam, -1.0) + 2f32.ln()).abs() < 1e-6);
        d.algo = 1 + 2; // Classic
        assert!((harmor_shape(d.mode(), d.jam, 0.5) - 0.5f32.tanh()).abs() < 1e-7);
        d.algo = 3 + 2; // Flathat
        assert!((harmor_shape(d.mode(), d.jam, 0.5) - (0.5f32.exp() - 1.0).tanh()).abs() < 1e-6);
    }

    #[test]
    fn the_twister_is_mt19937() {
        let mut t = Twister::new(5489);
        assert_eq!(t.next(), 3_499_211_612);
        assert_eq!(t.next(), 581_869_302);
    }

    #[test]
    fn the_chorus_modes_are_delay_3_s_models() {
        let wheels = [97, 0, 542, 1250, 6000, -1033];
        let v = chorus_engine(0, &wheels);
        assert_eq!(v[1], 1.0);
        assert_eq!(chorus_engine(1, &wheels)[1], 0.0);
        assert_eq!(chorus_engine(2, &wheels)[1], 2.0);
        assert!((v[24] - 1967.0 / 6000.0).abs() < 1e-6);
        assert_eq!(v[11], 1.0);
        assert_eq!(v[25], 0.0);
    }

    /// The cabinet impulses ship with FL.
    fn effects() -> Option<PathBuf> {
        let root = corpus()?;
        let install = root.parent()?;
        Some(install.join("Plugins/Fruity/Effects"))
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, Distructor's states skipped");
            return;
        };
        let generators = effects().unwrap().parent().unwrap().join("Generators");
        let mut saved = presets(NAME).unwrap();
        let in_projects: Vec<_> = projects(&root)
            .into_iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        // DEDZII - NIGHT TERROR, Edlan & Ella Noel, Sid De La Cruz twice.
        let mut lengths: Vec<_> = in_projects.iter().map(|s| s.state.len()).collect();
        lengths.sort_unstable();
        assert_eq!(lengths, [496, 958, 958, 958]);
        saved.extend(in_projects);
        assert!(saved.len() > 50);
        let input: Vec<f32> = sine(220.0 / 44_100.0, 8192)
            .iter()
            .map(|x| x * 0.5)
            .collect();
        for s in saved {
            let mut plugin = Distructor::default();
            plugin
                .set_state(&s.state)
                .unwrap_or_else(|e| panic!("{}: {e}", s.file));
            plugin.set_resource_dir(&generators);
            let (mut l, mut r) = (input.clone(), input.clone());
            plugin.process(&mut l, &mut r);
            let peak = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{}", s.file);
            assert!(peak < 8.0, "{}: {peak}", s.file);
        }
    }

    #[test]
    fn the_corpus_speakers_find_their_cabinets() {
        let Some(effects) = effects() else {
            return;
        };
        for impulse in [1, 4] {
            let mut s = Speaker::new();
            s.impulse = impulse;
            s.load(&effects);
            assert!(s.engine.is_some(), "{impulse}");
            assert!(s.len > 100, "{}", s.len);
        }
    }
}
