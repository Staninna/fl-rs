//! Frequency Splitter: the input cut into two or three bands by crossover
//! filters, each band at its own level on the plugin's output, as
//! `docs/plugins/frequency-splitter.md` has it.
//!
//! The state of version 4 is 113 bytes: the version, the plugin's fourteen
//! parameters and the three bands' target mixer tracks as little-endian
//! i32, then the window's settings, the six mute buttons as the bits of an
//! i32, and more of the window's settings. Versions 1 to 3 are shorter at
//! the end; FL leaves a state of version 5 or more alone, and so does this.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Low/Mid Freq | 0 to 65536 | 19800 |
//! | 1 Mid/High Freq | 0 to 65536 | 48065 |
//! | 2, 3 Low/Mid, Mid/High Slope | 0 to 7 | 3 |
//! | 4 Linear phase | 0 or 1 | 0 |
//! | 5 Number of bands, 2 or 3 | 0 or 1 | 1 |
//! | 6 Linear phase quality | 0 to 5 | 2 |
//! | 7 Linear phase speed | 0 to 65536 | 45820 |
//! | 8, 9, 10 Low, Mid, High send level | -1800 to 1800 | 0 |
//! | 11, 12, 13 Low, Mid, High main output level | -1800 to 1800 | 0 |
//!
//! FL's plugin is a shell around `engine_x64.dll` beside it, a C++ VST
//! with eight outputs: the three bands, each at its send level, and their
//! sum at the main levels. The shell plays the sum and adds each band to
//! the mixer track the state names for it, when it names one. This plays
//! the sum, and hands each band with a target to [`Effect::outputs`],
//! which a Patcher routes. No mixer-slot state FL ships names a track.
//!
//! A frequency wheel is `30 * 2^(9.0589 p)` Hz, 30 Hz to 16 kHz. A slope is
//! 6, 12, 18, 24, 36, 48, 72 or 96 dB an octave, and picks a crossover: one
//! pole filters, Linkwitz-Riley pairs of Butterworth biquads, or for 18 dB
//! a sum and difference of two allpasses. Three bands split at the lower
//! frequency, split the upper part again, and send the lower part through
//! the upper crossover's allpass. A level is `2 p` up to the middle of its
//! wheel and a line from 1 to +18 dB above it, glided to in 5 ms.
//!
//! Linear phase builds the low pass and the high pass as FIR filters from a
//! curve of `s` dB an octave, -6 dB at the crossover, by the engine's own
//! convolver ([`crate::linear_phase`], the one Maximus and Parametric EQ 2
//! ask for), and the middle band is what the two leave of the input. It is
//! `3 N / 4` frames late for an FFT of `N`, and a new setting fades in from
//! the old filters.

use crate::linear_phase::{self, Convolver, Fft};
use crate::{Effect, Fidelity, Output, StateError, knob_value, state_version};
use std::f64::consts::{LOG2_E, PI, TAU};
use std::ops::{Add, Mul, Sub};

/// The plugin's event 201 name.
pub const NAME: &str = "Frequency Splitter";

/// How many parameters the plugin has.
const PARAMS: usize = 14;

/// The range of each parameter's control.
const RANGES: [(i32, i32); PARAMS] = [
    (0, 65_536),
    (0, 65_536),
    (0, 7),
    (0, 7),
    (0, 1),
    (0, 1),
    (0, 5),
    (0, 65_536),
    (-1800, 1800),
    (-1800, 1800),
    (-1800, 1800),
    (-1800, 1800),
    (-1800, 1800),
    (-1800, 1800),
];
const DEFAULTS: [i32; PARAMS] = [19_800, 48_065, 3, 3, 0, 1, 2, 45_820, 0, 0, 0, 0, 0, 0];

const LOW_FREQ: usize = 0;
const HIGH_FREQ: usize = 1;
const LOW_SLOPE: usize = 2;
const HIGH_SLOPE: usize = 3;
const LINEAR: usize = 4;
const BANDS: usize = 5;
const QUALITY: usize = 6;
const SPEED: usize = 7;
/// The first of the three send levels and of the three main output
/// levels.
const SEND_LEVEL: usize = 8;
const MAIN_LEVEL: usize = 11;

/// How many frames of each band's send there is room for from the start.
/// A longer block asks for more, once.
const SEND_ROOM: usize = 4096;

/// The newest version FL reads, and the highest target mixer track.
const VERSION: i32 = 4;
const TRACKS: i32 = 999;

/// The mute buttons: the three sends, then the three main outputs.
const MUTES: usize = 6;
const MAIN_MUTE: usize = 3;

/// The slope each slope wheel picks, in dB an octave.
const SLOPES: [i32; 8] = [6, 12, 18, 24, 36, 48, 72, 96];

/// The Q of each biquad in a crossover's chain, as the engine has them:
/// Butterworth's, with 0.707 rounded to a float in two of them.
const Q_24: f64 = 0.707_000_017_166_137_7;
const Q_36: [f64; 3] = [0.541_195_988_655_090_3, 1.306_563_019_752_502_4, Q_24];
const Q_48: [f64; 2] = [0.541_195_988_655_090_3, 1.306_563_019_752_502_4];
const Q_72: [f64; 3] = [
    0.517_638_087_272_644,
    0.707_106_769_084_930_4,
    1.931_851_744_651_794_4,
];
const Q_96: [f64; 4] = [
    0.509_795_606_136_322,
    0.601_344_883_441_925,
    0.899_976_193_904_876_7,
    2.562_915_325_164_795,
];

/// How long a state of each version is: the version, the 17 words, two
/// words for the monitor, the window's toggles and the mutes, then a byte
/// of flags from version 2, a word from version 3 and five from version 4.
fn state_len(version: i32) -> usize {
    let mut len = 4 + 17 * 4 + 4 * 4;
    if version >= 2 {
        len += 1;
    }
    if version >= 3 {
        len += 4;
    }
    if version >= 4 {
        len += 20;
    }
    len
}

/// Where a wheel stands between its ends, as the shell hands it on.
fn share(value: i32, (min, max): (i32, i32)) -> f32 {
    (f64::from(value - min) * (1.0 / f64::from(max - min))).clamp(0.0, 1.0) as f32
}

/// The engine's frequency for a wheel's share: 30 Hz to 16 kHz.
fn hz(p: f32) -> f32 {
    2f32.powf(p * f32::from_bits(0x4110_f13b)) * 30.0
}

/// The engine's level for a wheel's share: a line to 1 at the middle and
/// another to +18 dB at the top.
fn level(p: f32) -> f32 {
    if p >= 0.5 {
        (p - 0.5) * f32::from_bits(0x415e_2f5e) + 1.0
    } else {
        p + p
    }
}

/// The slope a slope wheel picks: the engine counts eighths of its share.
fn slope(value: i32) -> i32 {
    let p = (f64::from(value) / 7.0) as f32;
    let mut top = 0.0f32;
    for slope in SLOPES {
        top += 0.125;
        if p < top {
            return slope;
        }
    }
    SLOPES[7]
}

/// The size of the linear phase FFT for a quality at a rate: 512 frames
/// at the lowest, doubled for each quality and each octave of rate over
/// 44.1 kHz, halved under it, and never over 65536.
fn fft_size(quality: i32, rate: f32) -> usize {
    let size = 0x200usize << quality;
    if rate < 44_100.0 {
        return size / 2;
    }
    let octaves = ((f64::from(rate) * 2.267_573_696_145_124_8e-5).ln() * LOG2_E + 0.5) as i32;
    (size << (octaves & 0x1f)).min(0x1_0000)
}

/// The engine's quick log2: a polynomial on the mantissa.
fn fast_log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let t = (bits & 0x7f_ffff) as f32;
    let u = 1.0 - t * f32::from_bits(0x3400_0000);
    let c = f32::from_bits(0x329f_62bd);
    let curve = ((f32::from_bits(0x3e1f_62bd) - t * c) - u * t * c)
        + (f32::from_bits(0x3dd9_bc0c) - t * f32::from_bits(0x3259_bc0c));
    let rest = f32::from_bits(0x3f3d_2f1b) - t * f32::from_bits(0x33bd_2f1b);
    (1.0 - (curve * u + rest)) + (((bits as i32) >> 23) - 127) as f32
}

/// The engine's quick 2^x: a cubic on the fraction times the power of two
/// of the whole part, and one over that for a negative `x`.
fn fast_exp2(x: f32) -> f32 {
    let cubic = |f: f32| {
        ((f * f32::from_bits(0x3da1_eca7) + f32::from_bits(0x3e65_b643)) * f
            + f32::from_bits(0x3f32_5448))
            * f
            + f32::from_bits(0x3f7f_f6f8)
    };
    let power = |whole: i32| f32::from_bits(((whole + 127) as u32) << 23);
    if x > 0.0 {
        let whole = x as i32;
        cubic(x - whole as f32) * power(whole)
    } else if x < 0.0 {
        let x = -x;
        let whole = x as i32;
        1.0 / (cubic(x - whole as f32) * power(whole))
    } else {
        1.0
    }
}

/// A biquad's coefficients as the engine keeps them: `b0, b1, b2` and the
/// feedback `c1, c2` with their signs turned, so that
/// `y = b0 x + b1 x1 + b2 x2 + c1 y1 + c2 y2`.
type Coefficients = [f32; 5];

/// The filters the engine designs, by its own numbers for them.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Design {
    /// 0 and 1: the cookbook's low and high pass.
    Low2,
    High2,
    /// 9 and 10: one pole low and high pass from the bilinear transform.
    Low1,
    High1,
}

/// `FUN_1801e33b0`: the coefficients at `hz`, worked in doubles and kept
/// as floats.
fn design(kind: Design, hz: f64, rate: f64, q: f64) -> Coefficients {
    let (b0, b1, b2, a0, a1, a2) = match kind {
        Design::Low1 | Design::High1 => {
            let k = ((hz * PI) / rate).tan() as f32;
            let (b0, b1) = if kind == Design::Low1 {
                (f64::from(k), f64::from(k))
            } else {
                (1.0, -1.0)
            };
            (b0, b1, 0.0, f64::from(k + 1.0), f64::from(k - 1.0), 0.0)
        }
        Design::Low2 | Design::High2 => {
            let (sin, cos) = ((hz * TAU) / rate).sin_cos();
            let alpha = (0.5 / q) * sin;
            let (b0, b1) = if kind == Design::Low2 {
                ((1.0 - cos) * 0.5, 1.0 - cos)
            } else {
                ((cos + 1.0) * 0.5, -(cos + 1.0))
            };
            (b0, b1, b0, alpha + 1.0, cos * -2.0, 1.0 - alpha)
        }
    };
    let scale = 1.0 / a0;
    [
        (scale * b0) as f32,
        (scale * b1) as f32,
        (scale * b2) as f32,
        -((scale * a1) as f32),
        -((scale * a2) as f32),
    ]
}

/// One biquad of a crossover, on four lanes at once: the low pass of the
/// left and the right side, then the high pass of each.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Section {
    /// `b0, b1, b2, c1, c2`, each for the four lanes.
    coefficients: [[f32; 4]; 5],
    /// Each lane's output before and the one before that, and its input
    /// before and the one before that.
    y1: [f32; 4],
    y2: [f32; 4],
    x1: [f32; 4],
    x2: [f32; 4],
}

impl Section {
    /// `FUN_1801e32a0` twice: the low pass on lanes 0 and 1, the high pass
    /// on 2 and 3.
    fn tune(&mut self, low: Coefficients, high: Coefficients) {
        for (lanes, (low, high)) in self.coefficients.iter_mut().zip(low.iter().zip(&high)) {
            *lanes = [*low, *low, *high, *high];
        }
    }

    fn run(&mut self, x: [f32; 4]) -> [f32; 4] {
        let [b0, b1, b2, c1, c2] = self.coefficients;
        let mut y = [0.0; 4];
        for lane in 0..4 {
            y[lane] = self.x1[lane] * b1[lane]
                + x[lane] * b0[lane]
                + b2[lane] * self.x2[lane]
                + self.y1[lane] * c1[lane]
                + c2[lane] * self.y2[lane];
        }
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// The 18 dB crossover: a first order allpass and a second order one, whose
/// half sum is a third order Butterworth low pass and whose half
/// difference is its high pass.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Odd {
    /// The first order allpass: its coefficient, and each side's output and
    /// input before.
    k: f32,
    y: [f32; 2],
    x: [f32; 2],
    /// The second order allpass: `a2` and `-a1`, and each side's two
    /// outputs and two inputs before.
    k1: f32,
    k2: f32,
    y1: [f32; 2],
    y2: [f32; 2],
    x1: [f32; 2],
    x2: [f32; 2],
}

impl Odd {
    /// `FUN_1801e8190` for 18 dB.
    fn tune(&mut self, hz: f64, rate: f64) {
        let (sin, cos) = ((hz * TAU) / rate).sin_cos();
        let first = cos / (sin + 1.0);
        self.k = first as f32;
        let d = ((first + 1.0) / (1.0 - first)) * 0.5 * (1.0 - cos);
        self.k1 = ((1.0 - d) / (d + 1.0)) as f32;
        self.k2 = (f64::from(self.k1 + 1.0) * cos) as f32;
    }

    fn run(&mut self, left: f32, right: f32) -> [f32; 4] {
        let x = [left * 0.5, right * 0.5];
        let mut one = [0.0; 2];
        let mut two = [0.0; 2];
        for side in 0..2 {
            one[side] = (self.y[side] - x[side]) * self.k + self.x[side];
            self.x[side] = x[side];
            self.y[side] = one[side];
            two[side] = (self.y1[side] - self.x1[side]) * self.k2
                + (x[side] - self.y2[side]) * self.k1
                + self.x2[side];
            self.y2[side] = self.y1[side];
            self.x2[side] = self.x1[side];
            self.x1[side] = x[side];
            self.y1[side] = two[side];
        }
        [
            two[0] + one[0],
            two[1] + one[1],
            one[0] - two[0],
            one[1] - two[1],
        ]
    }
}

/// One crossover, `FUN_1801e7f10`: up to eight biquads in a chain on the
/// four lanes, or the 18 dB allpasses.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Crossover {
    sections: [Section; 8],
    /// How many of the biquads run.
    count: usize,
    /// Whether the high pass comes out with its sign turned, as the odd
    /// Linkwitz-Riley orders need.
    invert: bool,
    /// Whether the allpasses run in place of the biquads.
    odd: bool,
    allpasses: Odd,
}

impl Crossover {
    /// `FUN_1801e8190`: the filters for a slope in dB an octave at `hz`.
    /// What the filters hold stays.
    fn tune(&mut self, slope: i32, hz: f32, rate: f64) {
        let hz = f64::from(hz);
        self.count = 0;
        self.invert = false;
        self.odd = false;
        let pair = |q: f64| {
            (
                design(Design::Low2, hz, rate, q),
                design(Design::High2, hz, rate, q),
            )
        };
        // The Qs of the biquads that are designed; the rest copy them.
        let qs: &[f64] = match slope {
            6 => {
                self.count = 1;
                let low = design(Design::Low1, hz, rate, 0.0);
                let high = design(Design::High1, hz, rate, 0.0);
                self.sections[0].tune(low, high);
                return;
            }
            12 => {
                self.invert = true;
                self.count = 1;
                &[0.5]
            }
            18 => {
                self.odd = true;
                self.allpasses.tune(hz, rate);
                return;
            }
            24 => {
                self.count = 2;
                &[Q_24]
            }
            36 => {
                self.invert = true;
                self.count = 3;
                &Q_36
            }
            48 => {
                self.count = 4;
                &Q_48
            }
            72 => {
                self.count = 6;
                &Q_72
            }
            96 => {
                self.count = 8;
                &Q_96
            }
            _ => return,
        };
        for (at, q) in qs.iter().enumerate() {
            let (low, high) = pair(*q);
            self.sections[at].tune(low, high);
        }
        for at in qs.len()..self.count {
            self.sections[at].coefficients = self.sections[at - qs.len()].coefficients;
        }
    }

    /// The low pass of each side, then the high pass of each.
    fn run(&mut self, left: f32, right: f32) -> [f32; 4] {
        if self.odd {
            return self.allpasses.run(left, right);
        }
        let mut lanes = [left, right, left, right];
        for section in &mut self.sections[..self.count] {
            lanes = section.run(lanes);
        }
        if self.invert {
            lanes[2] = -lanes[2];
            lanes[3] = -lanes[3];
        }
        lanes
    }

    /// Forget what the filters hold.
    fn clear(&mut self) {
        for section in &mut self.sections {
            *section = Section {
                coefficients: section.coefficients,
                ..Section::default()
            };
        }
        self.allpasses = Odd {
            k: self.allpasses.k,
            k1: self.allpasses.k1,
            k2: self.allpasses.k2,
            ..Odd::default()
        };
    }
}

/// A complex number in floats, or a frame with the left side in `re` and
/// the right in `im`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct C32 {
    re: f32,
    im: f32,
}

impl Add for C32 {
    type Output = C32;
    fn add(self, other: C32) -> C32 {
        C32 {
            re: self.re + other.re,
            im: self.im + other.im,
        }
    }
}

impl Sub for C32 {
    type Output = C32;
    fn sub(self, other: C32) -> C32 {
        C32 {
            re: self.re - other.re,
            im: self.im - other.im,
        }
    }
}

impl Mul for C32 {
    type Output = C32;
    fn mul(self, other: C32) -> C32 {
        C32 {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
}

impl Mul<f32> for C32 {
    type Output = C32;
    fn mul(self, other: f32) -> C32 {
        C32 {
            re: self.re * other,
            im: self.im * other,
        }
    }
}

/// The magnitude curve of the linear phase low pass at `hz` with `slope`
/// dB an octave, or with `high` of the high pass, on the bins of an FFT
/// of `n` at `rate`: -6 dB at the crossover, the slope away from it on the
/// stop side, and one less the other's curve on the pass side.
fn magnitudes(into: &mut [f32], n: usize, rate: f32, hz: f32, slope: f32, high: bool) {
    let bin = (f64::from(rate) / n as f64) as f32;
    let per_hz = 1.0 / hz;
    let sixth = f32::from_bits(0x3e2a_aaab);
    for (i, out) in into[..=n / 2].iter_mut().enumerate() {
        let x = i as f32 * bin;
        *out = if x == 0.0 {
            if high { 0.0 } else { 1.0 }
        } else {
            let mut octaves = fast_log2(per_hz * x);
            if high {
                octaves = -octaves;
            }
            let db = slope * octaves;
            if octaves < 0.0 {
                let db = db - 6.0;
                if db >= -120.0 {
                    1.0 - fast_exp2(db * sixth)
                } else {
                    1.0
                }
            } else {
                let db = -6.0 - db;
                if db >= -120.0 {
                    fast_exp2(db * sixth)
                } else {
                    0.0
                }
            }
        };
    }
}

/// The linear phase mode, `FUN_1801e4890` and what it keeps: two sets of
/// the low and the high FIR, one playing and one fading in after a new
/// setting, the input of the last FFT's length to start a new set on, and
/// the input as late as the filters for the middle band.
#[derive(Debug, Clone, PartialEq)]
struct LinearPhase {
    /// The largest FFT the buffers hold, and the one in use: 0 before the
    /// first design.
    max: usize,
    n: usize,
    fft: Fft,
    scratch: Vec<linear_phase::C32>,
    magnitudes: Vec<f32>,
    history: Vec<C32>,
    history_at: usize,
    late: Vec<C32>,
    late_at: usize,
    /// Each set's low and high filter.
    sets: [[Convolver; 2]; 2],
    /// The set the next design goes into: the other one plays.
    designing: usize,
    /// Frames of the fade done, and its length.
    faded: i32,
    fade: i32,
}

/// What the engine's frames of a fade start at.
const FADE: i32 = 0x1000;

impl LinearPhase {
    fn new(max: usize) -> Self {
        let convolver = || {
            let mut convolver = Convolver::default();
            convolver.make_room(max);
            convolver
        };
        LinearPhase {
            max,
            n: 0,
            fft: Fft::new(max),
            scratch: vec![linear_phase::C32::default(); max],
            magnitudes: vec![0.0; max / 2 + 1],
            history: vec![C32::default(); max],
            history_at: 0,
            late: vec![C32::default(); max],
            late_at: 0,
            sets: [[convolver(), convolver()], [convolver(), convolver()]],
            designing: 0,
            faded: FADE,
            fade: FADE,
        }
    }

    /// `FUN_1801e4ce0` and the rest of a new setting in `FUN_1801e4890`.
    fn design(&mut self, setting: &Setting, n: usize, rate: f32) {
        let n = n.min(self.max);
        if n != self.n {
            // New buffers for a new size, the filters of both sets empty.
            self.n = n;
            self.history_at = 0;
            self.designing = 0;
            self.history.fill(C32::default());
            for convolver in self.sets.iter_mut().flatten() {
                convolver.silence();
            }
        }
        let designing = self.designing;
        let LinearPhase {
            fft,
            scratch,
            magnitudes,
            sets,
            ..
        } = self;
        let [low, high] = &mut sets[designing];
        let (low_slope, high_slope) = (setting.low_slope as f32, setting.high_slope as f32);
        self::magnitudes(magnitudes, n, rate, setting.low_hz, low_slope, false);
        low.design(magnitudes, n, fft, scratch);
        if !setting.two_bands {
            self::magnitudes(magnitudes, n, rate, setting.high_hz, high_slope, true);
            high.design(magnitudes, n, fft, scratch);
        }
        self.faded = 0;
        self.fade = (setting.fade_seconds * rate) as i32;
        // The new set hears the input of the last FFT's length.
        for convolver in &mut self.sets[self.designing] {
            convolver.clear();
        }
        for _ in 0..n {
            let frame = self.history[self.history_at];
            for convolver in &mut self.sets[self.designing] {
                convolver.frame([frame.re, frame.im], n, &self.fft, &mut self.scratch);
            }
            self.history_at += 1;
            if self.history_at == n {
                self.history_at = 0;
            }
        }
        self.designing = usize::from(self.designing == 0);
    }

    /// `FUN_1801e4730`: the low, middle and high band of a set.
    fn bands(&mut self, set: usize, frame: C32, late: C32, two_bands: bool) -> [C32; 3] {
        let n = self.n;
        let [low, high] = &mut self.sets[set];
        let mut run = |filter: &mut Convolver| {
            let [re, im] = filter.frame([frame.re, frame.im], n, &self.fft, &mut self.scratch);
            C32 { re, im }
        };
        let low = run(low);
        if two_bands {
            [low, C32::default(), late - low]
        } else {
            let high = run(high);
            [low, (late - low) - high, high]
        }
    }

    /// One frame through the filters, after any new design.
    fn run(&mut self, frame: C32, two_bands: bool) -> [C32; 3] {
        let n = self.n;
        if n == 0 {
            return [C32::default(); 3];
        }
        self.history[self.history_at] = frame;
        self.history_at += 1;
        if self.history_at == n {
            self.history_at = 0;
        }
        // The input as late as the filters, `3 N / 4`.
        let mask = self.max - 1;
        self.late[self.late_at] = frame;
        let late = self.late[(self.late_at + self.max - 3 * n / 4) & mask];
        self.late_at = (self.late_at + 1) & mask;
        let playing = usize::from(self.designing == 0);
        let bands = if self.faded < self.fade {
            let first = self.bands(0, frame, late, two_bands);
            let second = self.bands(1, frame, late, two_bands);
            let t = self.faded as f32 / self.fade as f32;
            let to_first = if self.designing == 0 { 1.0 - t } else { t };
            let to_second = 1.0 - to_first;
            [0, 1, 2].map(|band| second[band] * to_second + first[band] * to_first)
        } else {
            self.bands(playing, frame, late, two_bands)
        };
        if self.faded < self.fade {
            self.faded += 1;
        }
        bands
    }

    /// Forget the input, and keep the filters.
    fn clear(&mut self) {
        self.history.fill(C32::default());
        self.late.fill(C32::default());
        for convolver in self.sets.iter_mut().flatten() {
            convolver.clear();
        }
    }
}

/// What the engine makes of the parameters for its filters.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Setting {
    low_hz: f32,
    high_hz: f32,
    low_slope: i32,
    high_slope: i32,
    linear: bool,
    two_bands: bool,
    quality: i32,
    fade_seconds: f32,
}

/// Fruity's Frequency Splitter in an effect slot.
#[derive(Debug, Clone, PartialEq)]
pub struct FrequencySplitter {
    params: [i32; PARAMS],
    /// The mixer track each band also goes to, 0 for none.
    targets: [i32; 3],
    mutes: [bool; MUTES],
    rate: f32,
    setting: Setting,
    /// The filters are to be designed again before the next frame.
    dirty: bool,
    /// The main output levels: where each is going, where it is, and how
    /// far it gets a frame.
    gain_to: [f32; 3],
    gain: [f32; 3],
    glide: f32,
    /// The send levels, as the main ones, and each band's send of the
    /// last block, kept while a target is set.
    send_to: [f32; 3],
    send: [f32; 3],
    sends: [[Vec<f32>; 2]; 3],
    /// The host takes the sends where the targets say, as FL's Patcher
    /// does.
    routed: bool,
    crossovers: [Crossover; 3],
    linear: LinearPhase,
}

impl Default for FrequencySplitter {
    fn default() -> Self {
        let rate = 44_100.0;
        let mut plugin = FrequencySplitter {
            params: DEFAULTS,
            targets: [0; 3],
            mutes: [false; MUTES],
            rate,
            setting: Setting {
                low_hz: 0.0,
                high_hz: 0.0,
                low_slope: 0,
                high_slope: 0,
                linear: false,
                two_bands: true,
                quality: 0,
                fade_seconds: 0.0,
            },
            dirty: true,
            gain_to: [0.0; 3],
            gain: [0.0; 3],
            glide: 1.0,
            send_to: [0.0; 3],
            send: [0.0; 3],
            sends: std::array::from_fn(|_| {
                [Vec::with_capacity(SEND_ROOM), Vec::with_capacity(SEND_ROOM)]
            }),
            routed: false,
            crossovers: [Crossover::default(); 3],
            linear: LinearPhase::new(fft_size(5, rate)),
        };
        plugin.set_sample_rate(rate);
        plugin
    }
}

impl FrequencySplitter {
    /// The shell's `ProcessParam` for a parameter that changed, and the
    /// engine's `setParameter` for what it hands on. A setting that
    /// changes the filters has them designed again.
    fn apply(&mut self, index: usize) {
        let value = self.params[index];
        let old = self.setting;
        let mut setting = old;
        match index {
            // The engine takes the lower of the two as its first.
            LOW_FREQ | HIGH_FREQ => {
                let (low, high) = (self.params[LOW_FREQ], self.params[HIGH_FREQ]);
                let range = RANGES[LOW_FREQ];
                setting.low_hz = hz(share(low.min(high), range));
                setting.high_hz = hz(share(low.max(high), range));
            }
            LOW_SLOPE => setting.low_slope = slope(value),
            HIGH_SLOPE => setting.high_slope = slope(value),
            LINEAR => setting.linear = (value as f32 * f32::from_bits(0x3fff_be77)) as i32 == 1,
            BANDS => setting.two_bands = (value as f32) < 0.5,
            QUALITY => {
                let p = (f64::from(value) / 5.0) as f32;
                setting.quality = (p * f32::from_bits(0x40bf_ced9)) as i32;
            }
            SPEED => {
                let p = share(value, RANGES[SPEED]);
                setting.fade_seconds = 10f32.powf(p + p) * f32::from_bits(0x3b03_126f);
            }
            MAIN_LEVEL.. => self.main_gain(index - MAIN_LEVEL),
            SEND_LEVEL.. => self.send_gain(index - SEND_LEVEL),
        }
        self.setting = setting;
        let filters = Setting {
            fade_seconds: old.fade_seconds,
            ..setting
        };
        if filters != old {
            self.dirty = true;
        }
    }

    /// Where a main output's level goes: its wheel, or nothing when its
    /// mute is on.
    fn main_gain(&mut self, band: usize) {
        let level = level(share(
            self.params[MAIN_LEVEL + band],
            RANGES[MAIN_LEVEL + band],
        ));
        let on = if self.mutes[MAIN_MUTE + band] {
            0.0
        } else {
            1.0
        };
        self.gain_to[band] = level * on;
    }

    /// Where a send's level goes: its wheel, or nothing when its mute is
    /// on. The engine's sends are taken to glide as its main outputs do.
    fn send_gain(&mut self, band: usize) {
        let level = level(share(
            self.params[SEND_LEVEL + band],
            RANGES[SEND_LEVEL + band],
        ));
        let on = if self.mutes[band] { 0.0 } else { 1.0 };
        self.send_to[band] = level * on;
    }

    /// The filters for the setting, `FUN_1801e8190` on each crossover the
    /// bands use.
    fn tune(&mut self) {
        let rate = f64::from(self.rate);
        let Setting {
            low_hz,
            high_hz,
            low_slope,
            high_slope,
            ..
        } = self.setting;
        self.crossovers[0].tune(low_slope, low_hz, rate);
        if !self.setting.two_bands {
            self.crossovers[1].tune(high_slope, high_hz, rate);
            self.crossovers[2].tune(high_slope, high_hz, rate);
        }
    }

    /// The three bands of one frame with the crossovers.
    fn split(&mut self, left: f32, right: f32) -> [C32; 3] {
        let [first, second, third] = &mut self.crossovers;
        let [low_l, low_r, high_l, high_r] = first.run(left, right);
        let pair = |l: f32, r: f32| C32 { re: l, im: r };
        if self.setting.two_bands {
            return [pair(low_l, low_r), C32::default(), pair(high_l, high_r)];
        }
        let [mid_l, mid_r, top_l, top_r] = second.run(high_l, high_r);
        let [a, b, c, d] = third.run(low_l, low_r);
        [pair(a + c, b + d), pair(mid_l, mid_r), pair(top_l, top_r)]
    }
}

impl Effect for FrequencySplitter {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, state_len(VERSION))?;
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        let expected = state_len(version);
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let word = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let mut params = [0; PARAMS];
        for (index, param) in params.iter_mut().enumerate() {
            let value = word(4 + 4 * index);
            let (min, max) = RANGES[index];
            if value < min || value > max {
                return Err(StateError::Range {
                    param: index,
                    value,
                });
            }
            *param = value;
        }
        let mut targets = [0; 3];
        for (band, target) in targets.iter_mut().enumerate() {
            let value = word(4 + 4 * (PARAMS + band));
            if !(0..=TRACKS).contains(&value) {
                return Err(StateError::Range {
                    param: PARAMS + band,
                    value,
                });
            }
            *target = value;
        }
        // After the 17 words: the monitor twice and the window's toggles,
        // then the mutes.
        let mutes = word(4 + 17 * 4 + 12);
        self.params = params;
        self.targets = targets;
        for (bit, mute) in self.mutes.iter_mut().enumerate() {
            *mute = mutes & (1 << bit) != 0;
        }
        for index in 0..PARAMS {
            self.apply(index);
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        let max = fft_size(5, hz);
        if max != self.linear.max {
            self.linear = LinearPhase::new(max);
        }
        for index in 0..PARAMS {
            self.apply(index);
        }
        self.dirty = true;
        self.glide = (1.0 - 0.1f64.powf(200.0 / f64::from(hz))) as f32;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.setting.linear && self.dirty {
            self.tune();
            self.dirty = false;
        }
        let sends = self.targets.map(|target| target > 0);
        for (band, send) in self.sends.iter_mut().enumerate() {
            send[0].clear();
            send[1].clear();
            if sends[band] {
                send[0].resize(left.len(), 0.0);
                send[1].resize(left.len(), 0.0);
            }
        }
        for (frame, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            for (gain, to) in self.gain.iter_mut().zip(&self.gain_to) {
                *gain += (to - *gain) * self.glide;
            }
            for (gain, to) in self.send.iter_mut().zip(&self.send_to) {
                *gain += (to - *gain) * self.glide;
            }
            let bands = if self.setting.linear {
                if self.dirty && self.linear.faded >= self.linear.fade {
                    let n = fft_size(self.setting.quality, self.rate);
                    self.linear.design(&self.setting, n, self.rate);
                    self.dirty = false;
                }
                let frame = C32 { re: *l, im: *r };
                self.linear.run(frame, self.setting.two_bands)
            } else {
                self.split(*l, *r)
            };
            for (band, value) in bands.iter().enumerate() {
                if sends[band] {
                    let gain = self.send[band];
                    self.sends[band][0][frame] = value.re * gain;
                    self.sends[band][1][frame] = value.im * gain;
                }
            }
            let [low, mid, high] = bands;
            let [g0, g1, g2] = self.gain;
            *l = low.re * g0 + mid.re * g1 + high.re * g2;
            *r = low.im * g0 + mid.im * g1 + high.im * g2;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |v| *v as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS {
            self.params[index] = knob_value(value, RANGES[index]);
            self.apply(index);
        }
    }

    fn reset(&mut self) {
        for crossover in &mut self.crossovers {
            crossover.clear();
        }
        self.linear.clear();
    }

    /// Each band whose target is set, at its send level, to the output
    /// the target numbers.
    fn outputs(&self, each: &mut Output<'_>) {
        for (band, target) in self.targets.iter().enumerate() {
            if *target > 0 {
                let [left, right] = &self.sends[band];
                each(*target as usize, left, right);
            }
        }
    }

    fn route_outputs(&mut self) {
        self.routed = true;
    }

    fn fidelity(&self) -> Fidelity {
        if !self.routed && self.targets.iter().any(|target| *target > 0) {
            Fidelity::Approximate("Frequency Splitter's bands to other mixer tracks are not routed")
        } else {
            Fidelity::Exact
        }
    }

    fn latency(&self) -> usize {
        if self.setting.linear {
            3 * fft_size(self.setting.quality, self.rate).min(self.linear.max) / 4
        } else {
            0
        }
    }

    fn max_latency(&self) -> usize {
        3 * self.linear.max / 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, partial, sine, state};

    /// A state as FL writes it, version 4: the parameters, the targets,
    /// the monitor on the main output, the window's toggles, the mutes and
    /// the window's settings of a new plugin.
    fn saved(params: [i32; PARAMS], targets: [i32; 3], mutes: i32) -> Vec<u8> {
        let mut bytes = state(&[VERSION]);
        bytes.extend(state(&params));
        bytes.extend(state(&targets));
        bytes.extend(state(&[2, 2, 12, mutes]));
        bytes.push(0x12);
        bytes.extend(state(&[1, 3, 1, 2, 2, 2]));
        bytes
    }

    fn with(moves: &[(usize, i32)], mutes: i32) -> FrequencySplitter {
        let mut params = DEFAULTS;
        for (index, value) in moves {
            params[*index] = *value;
        }
        let mut plugin = FrequencySplitter::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_state(&saved(params, [0; 3], mutes)).unwrap();
        plugin
    }

    /// The wheel that puts a crossover nearest `hz`.
    fn wheel(hz: f64) -> i32 {
        ((hz / 30.0).log2() / f64::from(f32::from_bits(0x4110_f13b)) * 65_536.0).round() as i32
    }

    /// The left side of a tone at `hz` through the plugin, in blocks of
    /// 300, and how loud the tone is in the last 4800 frames.
    fn level_of(plugin: &mut FrequencySplitter, hz: f64, len: usize) -> f64 {
        let turns = hz / 48_000.0;
        let mut left = sine(turns, len);
        let mut right = left.clone();
        for (l, r) in left.chunks_mut(300).zip(right.chunks_mut(300)) {
            plugin.process(l, r);
        }
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        partial(&left[len - 4800..], turns)
    }

    #[test]
    fn the_defaults_are_fl_s_default_preset() {
        let plugin = FrequencySplitter::default();
        let params: Vec<f32> = (0..PARAMS).map(|index| plugin.param(index)).collect();
        let defaults: Vec<f32> = DEFAULTS.iter().map(|v| *v as f32).collect();
        assert_eq!(params, defaults);
        assert_eq!(plugin.param(PARAMS), 0.0);
        assert_eq!(plugin.param_count(), PARAMS);
        assert_eq!(plugin.latency(), 0);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        // Three bands at 200 Hz and 3 kHz, 24 dB an octave.
        let setting = plugin.setting;
        assert!(!setting.two_bands && !setting.linear);
        assert!((setting.low_hz - 200.0).abs() < 0.1, "{}", setting.low_hz);
        assert!(
            (setting.high_hz - 3000.0).abs() < 1.0,
            "{}",
            setting.high_hz
        );
        assert_eq!((setting.low_slope, setting.high_slope), (24, 24));
        assert_eq!(plugin.gain_to, [1.0; 3]);
    }

    #[test]
    fn the_state_is_read_to_its_last_byte() {
        assert_eq!(state_len(4), 113);
        assert_eq!(state_len(1), 88);
        let mut plugin = FrequencySplitter::default();
        let mut params = DEFAULTS;
        params[MAIN_LEVEL] = -323;
        plugin.set_state(&saved(params, [0, 7, 0], 0x08)).unwrap();
        assert_eq!(plugin.param(MAIN_LEVEL), -323.0);
        assert_eq!(plugin.targets, [0, 7, 0]);
        assert_eq!(plugin.mutes, [false, false, false, true, false, false]);
        assert_eq!(plugin.gain_to[0], 0.0);
        // Only bands to other tracks are left out.
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(_)));

        let bytes = saved(DEFAULTS, [0; 3], 0);
        assert_eq!(
            plugin.set_state(&bytes[..112]),
            Err(StateError::Length {
                expected: 113,
                found: 112
            })
        );
        let mut newer = bytes.clone();
        newer[0] = 5;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 5 })
        );
        let mut steep = DEFAULTS;
        steep[LOW_SLOPE] = 8;
        assert_eq!(
            plugin.set_state(&saved(steep, [0; 3], 0)),
            Err(StateError::Range { param: 2, value: 8 })
        );
        // A state turned down leaves the plugin as it was.
        assert_eq!(plugin.param(MAIN_LEVEL), -323.0);
        // A first version is the 17 words, the monitor, the toggles and
        // the mutes.
        let mut first = state(&[1]);
        first.extend(state(&DEFAULTS));
        first.extend(state(&[0, 0, 0, 2, 2, 12, 0x30]));
        plugin.set_state(&first).unwrap();
        assert_eq!(plugin.mutes, [false, false, false, false, true, true]);
    }

    #[test]
    fn the_wheels_are_the_engine_s_curves() {
        // 30 Hz to 16 kHz.
        assert_eq!(hz(0.0), 30.0);
        assert!((hz(1.0) - 16_000.0).abs() < 1.0, "{}", hz(1.0));
        // Silence, 1 and +18 dB.
        assert_eq!(level(share(-1800, (-1800, 1800))), 0.0);
        assert_eq!(level(share(0, (-1800, 1800))), 1.0);
        let top = level(share(1800, (-1800, 1800)));
        assert!((20.0 * top.log10() - 18.0).abs() < 1e-4);
        // A line under the middle: -900 is half.
        assert_eq!(level(share(-900, (-1800, 1800))), 0.5);
        let slopes: Vec<i32> = (0..8).map(slope).collect();
        assert_eq!(slopes, SLOPES);
        // The FFT for each quality at 44.1, 48 and 96 kHz, and under.
        assert_eq!(fft_size(0, 44_100.0), 512);
        assert_eq!(fft_size(2, 48_000.0), 2048);
        assert_eq!(fft_size(2, 96_000.0), 4096);
        assert_eq!(fft_size(5, 192_000.0), 65_536);
        assert_eq!(fft_size(2, 22_050.0), 1024);
        // The fade is 2 ms to 200 ms; FL's default 50 ms.
        let fade = with(&[], 0).setting.fade_seconds;
        assert!((fade - 0.0500).abs() < 1e-3, "{fade}");
        // The lower wheel is always the first crossover.
        let swapped = with(&[(0, 50_000), (1, 10_000)], 0).setting;
        assert!(swapped.low_hz < swapped.high_hz);
    }

    #[test]
    fn the_quick_log_and_power_are_close() {
        for x in [0.01f32, 0.5, 1.0, 1.5, 3.0, 100.0, 12_345.0] {
            assert!((fast_log2(x) - x.log2()).abs() < 2e-3, "{x}");
        }
        for x in [-20.0f32, -6.5, -1.0, 0.0, 0.25, 3.7] {
            let near = (fast_exp2(x) - x.exp2()).abs() / x.exp2();
            assert!(near < 2e-4, "{x}");
        }
    }

    #[test]
    fn a_cookbook_low_pass_passes_a_tenth_and_stops_ten_times_its_frequency() {
        let rate = 48_000.0;
        let c = design(Design::Low2, 1000.0, rate, 0.5);
        let num = [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])];
        let den = [1.0, -f64::from(c[3]), -f64::from(c[4])];
        let gain = |hz: f64| testing::response(&num, &den, hz / rate);
        assert!((gain(100.0) - 1.0).abs() < 0.02);
        assert!(gain(10_000.0) < 0.02);
        // Q 0.5 is -6 dB at the corner.
        assert!((gain(1000.0) - 0.5).abs() < 1e-3);
        let c = design(Design::High1, 1000.0, rate, 0.0);
        let num = [f64::from(c[0]), f64::from(c[1])];
        let den = [1.0, -f64::from(c[3])];
        let gain = |hz: f64| testing::response(&num, &den, hz / rate);
        assert!((gain(1000.0) - 0.5f64.sqrt()).abs() < 1e-3);
        assert!(gain(10.0) < 0.02);
    }

    #[test]
    fn two_bands_at_one_level_are_the_input_at_every_slope_but_36_db() {
        // Linkwitz-Riley pairs, the one pole pair and the odd allpasses
        // add up to an allpass; 36 dB's chain does not quite.
        for (index, slope) in SLOPES.iter().enumerate() {
            for hz in [100.0, 1000.0, 1200.0, 8000.0] {
                let mut plugin = with(&[(BANDS, 0), (LOW_SLOPE, index as i32)], 0);
                let size = level_of(&mut plugin, hz, 48_000);
                let near = if *slope == 36 { 0.5 } else { 0.01 };
                assert!((size - 1.0).abs() < near, "{slope} dB at {hz} Hz: {size}");
            }
        }
    }

    #[test]
    fn a_muted_band_takes_its_frequencies_away() {
        // Two bands at 1 kHz, 48 dB: the low main output muted.
        let moves = [(BANDS, 0), (LOW_FREQ, wheel(1000.0)), (LOW_SLOPE, 5)];
        let mut plugin = with(&moves, 1 << MAIN_MUTE);
        assert!(level_of(&mut plugin, 125.0, 24_000) < 1e-4);
        let mut plugin = with(&moves, 1 << MAIN_MUTE);
        assert!((level_of(&mut plugin, 8000.0, 24_000) - 1.0).abs() < 1e-3);
        // A send's mute changes nothing on the main output.
        let mut plugin = with(&moves, 0b111);
        assert!((level_of(&mut plugin, 125.0, 24_000) - 1.0).abs() < 1e-3);
        // Three bands at the default 200 Hz and 3 kHz, only the middle on.
        let mut plugin = with(&[], 0b101_000);
        assert!((level_of(&mut plugin, 800.0, 24_000) - 1.0).abs() < 0.03);
        let mut plugin = with(&[], 0b101_000);
        assert!(level_of(&mut plugin, 20.0, 48_000) < 2e-4);
        let mut plugin = with(&[], 0b101_000);
        assert!(level_of(&mut plugin, 16_000.0, 24_000) < 2e-3);
        // And all three bands are the input.
        for hz in [40.0, 200.0, 800.0, 3000.0, 16_000.0] {
            let mut plugin = with(&[], 0);
            assert!(
                (level_of(&mut plugin, hz, 48_000) - 1.0).abs() < 0.01,
                "{hz}"
            );
        }
    }

    #[test]
    fn a_level_glides_to_its_wheel_in_5_ms() {
        // +18 dB on all three.
        let moves = [(11, 1800), (12, 1800), (13, 1800)];
        let mut plugin = with(&moves, 0);
        let top = 10f32.powf(18.0 / 20.0);
        // A tenth of the way left after `rate / 200` frames.
        let mut silence = vec![0.0; 48_000 / 200];
        plugin.process(&mut silence.clone(), &mut silence);
        assert!(
            (plugin.gain[0] - 0.9 * top).abs() < 1e-3,
            "{}",
            plugin.gain[0]
        );
        plugin.process(&mut silence.clone(), &mut silence);
        assert!(
            (plugin.gain[2] - 0.99 * top).abs() < 1e-3,
            "{}",
            plugin.gain[2]
        );
    }

    #[test]
    fn linear_phase_bands_add_up_to_the_input_late() {
        let mut plugin = with(&[(LINEAR, 1)], 0);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        assert_eq!(plugin.latency(), 1536);
        assert_eq!(plugin.max_latency(), 3 * 16_384 / 4);
        let late = plugin.latency();
        let input = sine(440.0 / 48_000.0, 9600);
        let (mut left, mut right) = (input.clone(), input.clone());
        for (l, r) in left.chunks_mut(256).zip(right.chunks_mut(256)) {
            plugin.process(l, r);
        }
        // Past the levels' glide, the three bands are the input.
        for n in 2400..9600 {
            assert!((left[n] - input[n - late]).abs() < 1e-4, "{n}");
        }
    }

    #[test]
    fn linear_phase_takes_a_muted_band_away() {
        let moves = [
            (LINEAR, 1),
            (BANDS, 0),
            (LOW_FREQ, wheel(1000.0)),
            (LOW_SLOPE, 3),
        ];
        // Past the fade in of the filters.
        let mut plugin = with(&moves, 1 << MAIN_MUTE);
        assert!(level_of(&mut plugin, 100.0, 24_000) < 1e-3);
        let mut plugin = with(&moves, 1 << MAIN_MUTE);
        assert!((level_of(&mut plugin, 5000.0, 24_000) - 1.0).abs() < 1e-3);
        // The low pass is -6 dB at its crossover, and its curve the slope.
        let mut curve = vec![0.0; 1025];
        magnitudes(&mut curve, 2048, 48_000.0, 1500.0, 24.0, false);
        assert!((curve[64] - 0.5).abs() < 2e-3);
        let octave_up = 20.0 * curve[128].log10();
        assert!((octave_up + 30.0).abs() < 0.1, "{octave_up}");
        let mut high = vec![0.0; 1025];
        magnitudes(&mut high, 2048, 48_000.0, 1500.0, 24.0, true);
        for k in 1..1025 {
            assert!((curve[k] + high[k] - 1.0).abs() < 1e-5, "{k}");
        }
    }

    #[test]
    fn every_state_fl_ships_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states in FL's files skipped");
            return;
        };
        let mut saved: Vec<_> = testing::projects(&root);
        saved.extend(testing::every_preset(&root, NAME));
        let saved: Vec<_> = saved
            .into_iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        assert!(saved.len() >= 17, "{}", saved.len());
        let tone = |turns: f64| -> Vec<f32> {
            let loud = sine(turns, 24_000).into_iter().enumerate();
            loud.map(|(n, v)| v * (n % 4800) as f32 / 4800.0).collect()
        };
        for state in saved {
            let mut plugin = FrequencySplitter::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_state(&state.state).unwrap();
            let (mut left, mut right) = (tone(0.003), tone(0.07));
            for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
                plugin.process(l, r);
            }
            let peak = left
                .iter()
                .chain(&right)
                .fold(0.0f32, |p, v| p.max(v.abs()));
            assert!(peak.is_finite() && peak < 4.0, "{}: {peak}", state.file);
            assert!(plugin.latency() <= plugin.max_latency());
        }
    }
}
