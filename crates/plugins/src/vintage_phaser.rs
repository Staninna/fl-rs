//! Vintage Phaser: four first-order allpasses swept by an LFO, at four or
//! eight times the rate, as `docs/plugins/vintage-phaser.md` has it. FL's
//! plugin is a Delphi shell around a VST engine (`engine_x64.dll`); this is
//! the engine, driven the way the shell drives it.
//!
//! The state is a little-endian i32 version and the 36 parameters as i32,
//! then a byte of the editor's: 149 bytes at version 3, 148 at version 2,
//! and 132 at version 1, which has 32 parameters in another order. FL
//! leaves a state alone whose version is 4 or more, and so does this. The
//! shell hands the engine each parameter as `(value - min) / (max - min)`,
//! kept between 0 and 1.
//!
//! | parameter | range |
//! |---|---|
//! | 0, 1 Min, max frequency | 125 to 2048000 |
//! | 2 Feedback | 0 to 6000 |
//! | 3 Manual | 0 to 1 |
//! | 4 Manual frequency | 125 to 2048000 |
//! | 5 High quality | 0 to 1 |
//! | 6 Speed | 1 to 245760 |
//! | 7 Tempo sync | 0 to 1 |
//! | 8 LFO type (triangle, sine) | 0 to 1 |
//! | 9 Tension | 0 to 6000 |
//! | 10 Start phase, 11 LR phase | 0 to 6000 |
//! | 12 Delay on, 13 keep pitch | 0 to 1 |
//! | 14 Delay mix, 15 stereo | 0 to 6000 |
//! | 16 Delay time | 1 to 768 |
//! | 17 Delay tempo sync | 0 to 1 |
//! | 18 Delay feedback | 0 to 6000 |
//! | 19 Delay low pass | 3200 to 204800 |
//! | 20 Delay high pass | 2000 to 204800 |
//! | 21 Mix | 0 to 100 |
//! | 22 Mono input | 0 to 1 |
//! | 23 Noise | 0 to 6000 |
//! | 24 Noise gate | 0 to 1 |
//! | 25 Colour low shelf gain | 0 to 6000 |
//! | 26 Colour low shelf frequency | 5000 to 650000 |
//! | 27 Input high pass, 28 feedback high pass | 2000 to 80000 |
//! | 29, 30 Rate frequencies | 25 to 150000 |
//! | 31 LFO depth scale, 32 pull down | 0 to 6000 |
//! | 33 Engine (V1, V2) | 0 to 1 |
//! | 34 Drive | 5000 to 650000 |
//! | 35 Invert feedback | 0 to 1 |
//!
//! Each side runs: a low shelf, a high-pass, an upsampler (a windowed sinc
//! 88 samples wide), and at the higher rate the four allpasses with their
//! feedback through a high-pass, mixed with their input. Engine V1's
//! allpasses are linear, in f64; V2's are four TPT one-poles with a Padé
//! tanh, driven, with two halfband filters before the rate comes down by
//! taking every fourth or eighth sample. Then hiss through a low shelf the
//! LFO moves, a noise gate and a delay. The engine reports 44 frames of
//! latency, the upsampler's, and the shell passes them on.

use crate::{Effect, Fidelity, StateError, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Vintage Phaser";

const COUNT: usize = 36;

/// The range of each parameter's control (the editor's DFM; the check boxes,
/// switches and buttons have no range there and take 0 to 1).
const RANGES: [(i32, i32); COUNT] = [
    (125, 2_048_000),
    (125, 2_048_000),
    (0, 6000),
    (0, 1),
    (125, 2_048_000),
    (0, 1),
    (1, 245_760),
    (0, 1),
    (0, 1),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 1),
    (0, 1),
    (0, 6000),
    (0, 6000),
    (1, 768),
    (0, 1),
    (0, 6000),
    (3200, 204_800),
    (2000, 204_800),
    (0, 100),
    (0, 1),
    (0, 6000),
    (0, 1),
    (0, 6000),
    (5000, 650_000),
    (2000, 80_000),
    (2000, 80_000),
    (25, 150_000),
    (25, 150_000),
    (0, 6000),
    (0, 6000),
    (0, 1),
    (5000, 650_000),
    (0, 1),
];

/// What a new plugin has: the engine's own values, put in each range by
/// the shell's constructor.
const DEFAULTS: [i32; COUNT] = [
    896_070, 1_588_996, 3000, 0, 125, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 3000, 1, 0, 0, 3200, 2000, 50,
    0, 0, 0, 0, 5000, 2000, 2000, 25, 150_000, 6000, 0, 1, 5000, 0,
];

/// The lengths of the three versions of the state, and the newest version.
const LEN_1: usize = 132;
const LEN_2: usize = 148;
const LEN_3: usize = 149;
const VERSION: i32 = 3;

/// Where each of version 1's 32 parameters goes (the shell's table at
/// `0x00742a30`).
const ORDER_1: [usize; 32] = [
    0, 1, 2, 3, 4, 6, 8, 11, 7, 10, 21, 22, 23, 5, 12, 13, 14, 15, 16, 17, 18, 19, 20, 25, 26, 27,
    28, 29, 30, 31, 32, 9,
];

/// How late the engine says it is: half the upsampler's window.
const LATENCY: usize = 44;

/// The noise seed of the first engine FL loads, and what each engine after
/// it adds to it.
const FIRST_SEED: i32 = 0x3a916;

/// The position the shell gives when the song is not playing.
const STOPPED: f64 = -1.0;

/// The engine's single-precision pi.
const PI_F32: f32 = std::f32::consts::PI;

/// The delay lines: 65536 samples whatever the rate.
const DELAY_LINE: usize = 0x1_0000;

/// The fast `2^x` of the LFO and the hiss: a cubic over the fraction times
/// the power of two of the whole part.
fn exp2_cubic(x: f32) -> f32 {
    let whole = x as i32;
    let f = x - whole as f32;
    let power = f32::from_bits(((whole + 0x7f) as u32) << 23);
    (((f * f32::from_bits(0x3da1_eca7) + f32::from_bits(0x3e65_b643)) * f
        + f32::from_bits(0x3f32_5448))
        * f
        + f32::from_bits(0x3f7f_f6f8))
        * power
}

/// `FUN_18001aad0`: [`exp2_cubic`] for either sign, 1 at 0.
fn exp2_signed(x: f32) -> f32 {
    if x > 0.0 {
        exp2_cubic(x)
    } else if x < 0.0 {
        1.0 / exp2_cubic(-x)
    } else {
        1.0
    }
}

/// The engine's rational tangent for `x = w / 2` up to pi / 2.
fn tan_rational(x: f32) -> f32 {
    let x2 = x * x;
    (f32::from_bits(0x401e_3025) - x2 * f32::from_bits(0x3e42_505d)) * x
        / (f32::from_bits(0x401d_e9e6) - x2)
}

/// The engine's Padé tanh, of a value already kept within 5.
fn tanh_pade(u: f32) -> f32 {
    let u2 = u * u;
    let num = (((u2 + 378.0) * u2 + 17325.0) * u2 + 135_135.0) * u;
    let den = ((u2 * 28.0 + 3150.0) * u2 + 62370.0) * u2 + 135_135.0;
    num / den
}

/// The LFO's shapes (`FUN_180017f90`), from 0 to 1.
fn shape(sine: bool, p: f32) -> f32 {
    if sine {
        ((p * f32::from_bits(0x40c9_0fdb)).sin() + 1.0) * 0.5
    } else if p < 0.25 {
        p + p + 0.5
    } else if p < 0.75 {
        1.0 - (((p - 0.25) + p) - 0.25)
    } else {
        ((p - 0.75) + p) - 0.75
    }
}

/// A second-order section: `y = ((a1 y1 + a2 y2) + b2 x2) + (b0 x + b1 x1)`,
/// with its coefficients rounded from f64 to f32 and the rate it is
/// designed at.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Biquad {
    rate: f64,
    /// `b0 b1 b2 a1 a2`, the feedback signs folded in.
    c: [f32; 5],
}

/// What a [`Biquad`] keeps of a side: `x1 x2 y1 y2`.
type Section = [f32; 4];

impl Biquad {
    fn new(rate: f64) -> Self {
        Biquad { rate, c: [0.0; 5] }
    }

    fn run(&self, x: f32, s: &mut Section) -> f32 {
        let [b0, b1, b2, a1, a2] = self.c;
        let [x1, x2, y1, y2] = *s;
        let y = ((a1 * y1 + a2 * y2) + b2 * x2) + (b0 * x + b1 * x1);
        *s = [x, x1, y, y1];
        y
    }

    /// `FUN_18001b1e0`'s last step: everything over `a0`.
    fn set(&mut self, a0: f64, a1: f64, a2: f64, b0: f64, b1: f64, b2: f64) {
        let n = 1.0 / a0;
        self.c = [
            (n * b0) as f32,
            (n * b1) as f32,
            (n * b2) as f32,
            -(n * a1) as f32,
            -(n * a2) as f32,
        ];
    }

    /// Type 11: a first-order high-pass at `hz`.
    fn high_pass(&mut self, hz: f64) {
        let t = (hz * std::f64::consts::PI / self.rate).tan();
        self.set(t + 1.0, t - 1.0, 0.0, 1.0, -1.0, 0.0);
    }

    /// Type 4: a low shelf at `hz` of `db` with Q `q`.
    fn low_shelf(&mut self, hz: f64, q: f64, db: f64) {
        let a = 10f64.powf(db * 0.025);
        let w = hz * std::f64::consts::TAU / self.rate;
        let (sin, cos) = (w.sin(), w.cos());
        let root = a.sqrt() * (1.0 / q) * sin;
        let (am1, ap1) = (a - 1.0, a + 1.0);
        let b0 = (root + (ap1 - am1 * cos)) * a;
        let b1 = (am1 - ap1 * cos) * (a + a);
        let b2 = ((ap1 - am1 * cos) - root) * a;
        let a0 = root + (ap1 + am1 * cos);
        let a1 = am1 * -2.0 - (ap1 * cos + ap1 * cos);
        let a2 = (ap1 + am1 * cos) - root;
        self.set(a0, a1, a2, b0, b1, b2);
    }
}

/// One side's upsampler (`FUN_18001be70`, `FUN_18001bd20`): every sample
/// adds the kernel, scaled, into a ring, and `factor` samples come out.
#[derive(Debug, Clone, PartialEq)]
struct Upsampler {
    factor: usize,
    kernel: Vec<f32>,
    ring: Vec<f32>,
    at: usize,
}

impl Upsampler {
    fn new(factor: usize) -> Self {
        let len = factor * 88 + 1;
        let step = 1.0 / factor as f64;
        let kernel = (0..len)
            .map(|i| {
                let t = step * i as f64 - 44.0;
                let sinc = if t.abs() < 1e-6 {
                    1.0
                } else {
                    let w = t * std::f64::consts::PI;
                    w.sin() / w
                };
                let window = (i as f64 / (len - 1) as f64 * std::f64::consts::PI).sin();
                (window.max(0.0) * sinc) as f32
            })
            .collect();
        let ring = len + factor * 2;
        Upsampler {
            factor,
            kernel,
            ring: vec![0.0; ring - ring % factor],
            at: 0,
        }
    }

    /// What a new one has, as the engine makes it again for a new rate.
    fn clear(&mut self) {
        self.ring.fill(0.0);
        self.at = 0;
    }

    /// Put `x` in; the next `factor` samples start at the index returned.
    fn push(&mut self, x: f32) -> usize {
        let (len, size, at) = (self.kernel.len(), self.ring.len(), self.at);
        let add = |ring: &mut [f32], kernel: &[f32]| {
            for (r, k) in ring.iter_mut().zip(kernel) {
                *r += *k * x;
            }
        };
        if size <= len + at {
            let (first, rest) = self.kernel.split_at(size - at);
            add(&mut self.ring[at..], first);
            add(&mut self.ring[..rest.len()], rest);
        } else {
            add(&mut self.ring[at..at + len], &self.kernel);
        }
        let mut clear = if len + at < size {
            len + at
        } else {
            len + at - size
        };
        for _ in 0..self.factor {
            self.ring[clear] = 0.0;
            clear = if clear + 1 < size { clear + 1 } else { 0 };
        }
        self.at += self.factor;
        if self.at >= size {
            self.at -= size;
        }
        at
    }
}

/// Engine V1's four allpasses (the object at `E+0x198`), in f64: the
/// coefficient of each side and, for each side, the input and the three
/// allpasses' outputs a sample back, then the last output.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Allpasses {
    c: [f64; 2],
    s: [[f64; 2]; 5],
    /// One over the higher rate.
    over: f64,
}

impl Allpasses {
    fn set(&mut self, side: usize, hz: f32) {
        let t = (f64::from(hz) * std::f64::consts::PI * self.over).tan();
        self.c[side] = (t - 1.0) / (t + 1.0);
    }

    fn run(&mut self, x: [f32; 2]) -> [f32; 2] {
        let mut out = [0.0; 2];
        for side in 0..2 {
            let c = self.c[side];
            let s = &mut self.s;
            let mut x = f64::from(x[side]);
            for k in 0..4 {
                let y = (x - s[k + 1][side]) * c + s[k][side];
                s[k][side] = x;
                x = y;
            }
            s[4][side] = x;
            out[side] = x as f32;
        }
        out
    }
}

/// Engine V2's ladder (the object at `E+0x410`), a side a lane: four
/// one-poles with a tanh, then a one-pole high-pass on the feedback.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Ladder {
    /// `2G - 1`, `1 - G`, `1 - Ghp`, the feedback's normaliser, `G`, `Ghp`
    /// and the feedback.
    a: [f32; 2],
    b: [f32; 2],
    hp_rest: [f32; 2],
    k: [f32; 2],
    g: [f32; 2],
    hp_g: f32,
    fb: f32,
    /// Pi over the higher rate.
    turn: f32,
    s: [[f32; 2]; 4],
    hp: [f32; 2],
}

impl Ladder {
    /// The cutoff of the feedback's high-pass.
    fn set_high_pass(&mut self, hz: f32) {
        let t = tan_rational(hz * self.turn);
        self.hp_g = t / (t + 1.0);
    }

    /// The LFO's frame: both sides' frequencies and the feedback.
    fn set(&mut self, hz: [f32; 2], fb: f32) {
        self.fb = fb;
        for (side, hz) in hz.into_iter().enumerate() {
            let t = tan_rational(hz * self.turn);
            let g = t / (t + 1.0);
            self.hp_rest[side] = 1.0 - self.hp_g;
            self.g[side] = g;
            self.b[side] = 1.0 - g;
            let a = (g + g) - 1.0;
            self.a[side] = a;
            let a2 = a * a;
            self.k[side] = 1.0 / (1.0 - a2 * a2 * fb);
        }
    }

    /// `FUN_1800176b0`, a side.
    fn run(&mut self, side: usize, x: f32) -> f32 {
        let [s0, s1, s2, s3] = self.s.map(|s| s[side]);
        let (a, b, g) = (self.a[side], self.b[side], self.g[side]);
        let hp = self.hp[side];
        let sum = ((((s0 + s0) * b * a + (s1 + s1) * b) * a + (s2 + s2) * b) * a + (s3 + s3) * b)
            + -hp * self.hp_rest[side];
        let old = [s0, s1, s2, s3];
        let mut u = (sum * self.fb + x) * self.k[side] - s0;
        let mut y = 0.0;
        for (k, s) in old.into_iter().enumerate() {
            // MINPS and MAXPS: a NaN comes out at 5, as in the DLL.
            #[allow(clippy::manual_clamp)]
            let t = tanh_pade(u.min(5.0).max(-5.0));
            let v = g * t;
            let low = s + v;
            self.s[k][side] = low + v;
            y = low - b * t;
            if let Some(next) = old.get(k + 1) {
                u = y - next;
            }
        }
        let d = (y - hp) * self.hp_g;
        self.hp[side] = hp + d + d;
        y
    }
}

/// A cascade of six second-order allpasses in z^-1 of each of two paths
/// (`FUN_180016ed0`, the object at `E+0x3e0`), the halfband low-pass of the
/// ladder's first halving. Lanes 0 and 1 are the sides on one path, lanes 2
/// and 3 on the other.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HalfbandSix {
    o: [[f32; 4]; 32],
}

/// A cascade of six first-order allpasses in z^-2 of each of two paths
/// (the object at `E+0x3e8`), the ladder's second halving at 8 times.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HalfbandTwo {
    p: [[f32; 4]; 26],
}

fn lanes(bits: [u32; 2]) -> [f32; 4] {
    let [a, b] = bits.map(f32::from_bits);
    [a, a, b, b]
}

fn sub(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| a[i] * b[i])
}

fn add(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + b[i])
}

impl HalfbandSix {
    fn new() -> Self {
        let mut o = [[0.0; 4]; 32];
        // The constructor's tables at 0x180031880.. and 0x1800316d0..
        let c0 = [
            [0xbf6f_5165, 0xbf78_9c1c],
            [0xbf83_3a2b, 0xbf8b_7fe4],
            [0xbf94_4163, 0xbf9c_d986],
            [0xbfa4_e819, 0xbfac_4aa4],
            [0xbfb3_0dde, 0xbfb9_6027],
            [0xbfbf_88db, 0xbeec_0905],
        ];
        let c1 = [
            [0x3e6a_e54d, 0x3e8d_e315],
            [0x3eb2_4d02, 0x3edd_d0fb],
            [0x3f05_f004, 0x3f1c_8ac0],
            [0x3f31_bbaf, 0x3f45_2823],
            [0x3f56_f190, 0x3f67_91f1],
            [0x3f77_c4f1, 0],
        ];
        for k in 0..6 {
            let at = if k == 0 { 0 } else { 5 * k + 2 };
            o[at] = lanes(c0[k]);
            o[at + 1] = lanes(c1[k]);
        }
        HalfbandSix { o }
    }

    fn run(&mut self, l: f32, r: f32) -> (f32, f32) {
        let o = &mut self.o;
        let x = [l, r, l, r];
        let (x1, x2) = (o[4], o[3]);
        o[2] = o[5];
        o[3] = x1;
        o[4] = x;
        let mut input = (x, x1, x2);
        for k in 0..6 {
            let (c0, c1, y2, y1) = if k == 0 {
                (0, 1, 5, 6)
            } else {
                (5 * k + 2, 5 * k + 3, 5 * k + 5, 5 * k + 6)
            };
            let (x, x1, x2) = input;
            let (old2, old1) = (o[y2], o[y1]);
            let y = add(add(mul(sub(x1, old1), o[c0]), x2), mul(sub(x, old2), o[c1]));
            if k > 0 {
                o[y2 - 1] = old2;
            }
            o[y2] = old1;
            o[y1] = y;
            input = (y, old1, old2);
        }
        let (y, old) = (o[31], input.1);
        ((y[2] + old[0]) * 0.5, (y[3] + old[1]) * 0.5)
    }
}

impl HalfbandTwo {
    fn new() -> Self {
        let mut p = [[0.0; 4]; 26];
        // The constructor's table at 0x1800316b0..
        let c = [
            [0x3d16_3f58, 0x3e0b_d324],
            [0x3e8c_9c89, 0x3ed8_a5a0],
            [0x3f0f_a42f, 0x3f2d_7344],
            [0x3f45_0dcd, 0x3f57_0302],
            [0x3f64_6b34, 0x3f6e_7989],
            [0x3f76_4bd4, 0x3f7c_e189],
        ];
        for (k, c) in c.into_iter().enumerate() {
            p[if k == 0 { 0 } else { 4 * k + 2 }] = lanes(c);
        }
        HalfbandTwo { p }
    }

    fn run(&mut self, l: f32, r: f32) -> (f32, f32) {
        let p = &mut self.p;
        let x = [l, r, l, r];
        // Stage 0: x two back at 2, one back at 3; y two back at 4, one at 5.
        let (x2, x1) = (p[2], p[3]);
        let (y2, y1) = (p[4], p[5]);
        p[1] = y2;
        let y = add(mul(sub(x, y2), p[0]), x2);
        p[2] = x1;
        p[3] = x;
        p[4] = y1;
        p[5] = y;
        let mut input = (y, y2);
        for k in 1..6 {
            let (c, h2, h1) = (4 * k + 2, 4 * k + 4, 4 * k + 5);
            let (old2, old1) = (p[h2], p[h1]);
            let (x, x2) = input;
            let y = add(mul(sub(x, old2), p[c]), x2);
            p[c + 1] = old2;
            p[h2] = old1;
            p[h1] = y;
            input = (y, old2);
        }
        let (y, old) = (p[25], p[24]);
        ((y[0] + old[2]) * 0.5, (y[1] + old[3]) * 0.5)
    }
}

/// The delay (the object at `E+0x2b8`, `FUN_18001c200`).
#[derive(Debug, Clone, PartialEq)]
struct Delay {
    mix: f32,
    stereo: f32,
    time: f32,
    keep_pitch: bool,
    feedback: f32,
    rate: f64,
    tempo: f64,
    /// How far the delay glides to its time each sample.
    glide: f32,
    /// The one-poles on the feedback: high-pass and low-pass, `b0 b1 a1`,
    /// and what they keep a side, `y1 x1`.
    high: [f32; 3],
    high_hz: f32,
    high_state: [[f32; 2]; 2],
    low: [f32; 3],
    low_hz: f32,
    low_state: [[f32; 2]; 2],
    turn: f32,
    fed: [f32; 2],
    lines: [Vec<f32>; 2],
    write: u32,
    synced: bool,
    dirty: bool,
    target: f32,
    now: f32,
    /// Keep pitch: a fade between two taps instead of a glide.
    pending: bool,
    fading: bool,
    fade_step: f32,
    first_tap: bool,
    taps: [i32; 2],
    fade: f32,
    fade_by: f32,
}

impl Delay {
    fn new() -> Self {
        let pi_rate = f32::from_bits(0x3895_6593);
        Delay {
            mix: 0.0,
            stereo: 0.0,
            time: 0.0,
            keep_pitch: false,
            feedback: 0.0,
            rate: 44_100.0,
            tempo: 120.0,
            glide: 0.0,
            high: [0.0; 3],
            high_hz: 100.0,
            high_state: [[0.0; 2]; 2],
            low: [0.0; 3],
            low_hz: 100.0,
            low_state: [[0.0; 2]; 2],
            turn: pi_rate,
            fed: [0.0; 2],
            lines: [vec![0.0; DELAY_LINE], vec![0.0; DELAY_LINE]],
            write: 0,
            synced: false,
            dirty: true,
            target: 0.0,
            now: 0.0,
            pending: false,
            fading: false,
            fade_step: 0.0,
            first_tap: false,
            taps: [0; 2],
            fade: 1.0,
            fade_by: 0.0,
        }
    }

    /// A first-order section at `hz` (`FUN_18001ad70` and the parameter
    /// cases): `b0 b1 a1`.
    fn one_pole(hz: f32, turn: f32, high: bool) -> [f32; 3] {
        let t = (turn * hz).tan();
        let n = 1.0 / (t + 1.0);
        let a1 = -(n * (t - 1.0));
        if high {
            [n, -n, a1]
        } else {
            [n * t, n * t, a1]
        }
    }

    /// `FUN_18001cab0`: the rate. The lines start again from silence.
    fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
        self.fade_step = 1.0 / (rate * 0.1) as f32;
        self.turn = (1.0 / rate) as f32 * PI_F32;
        self.low = Self::one_pole(self.low_hz, self.turn, false);
        self.high = Self::one_pole(self.high_hz, self.turn, true);
        let rest = f64::from_bits(0x3fb9_9999_9999_9998).powf(10.0 / rate);
        self.glide = (1.0 - rest) as f32;
        for line in &mut self.lines {
            line.fill(0.0);
        }
    }

    fn set_low(&mut self, v: f32) {
        self.low_hz = (2f64.powf(f64::from(v * 6.0)) * 320.0) as f32;
        self.low = Self::one_pole(self.low_hz, self.turn, false);
    }

    fn set_high(&mut self, v: f32) {
        self.high_hz = (2f64.powf(f64::from(v * 10.0)) * 20.0) as f32;
        self.high = Self::one_pole(self.high_hz, self.turn, true);
    }

    fn set_keep_pitch(&mut self, v: f32) {
        self.keep_pitch = v > 0.5;
        if self.keep_pitch {
            self.pending = false;
            self.fade_by = 0.0;
            self.taps = [self.target as i32; 2];
            self.first_tap = true;
        } else {
            self.first_tap = false;
        }
        self.fading = false;
        self.fade = 1.0;
    }

    fn run(&mut self, l: &mut f32, r: &mut f32) {
        let mask = (DELAY_LINE - 1) as u32;
        if self.dirty {
            self.pending = true;
            self.dirty = false;
            let rate = self.rate as f32;
            self.target = if self.synced {
                let ticks = (self.time * -767.0) as i32;
                (1 - ticks) as f32 * (0.3125 / self.tempo as f32) * rate
            } else {
                rate * self.time
            };
        }
        if self.keep_pitch && !self.fading && self.pending {
            let mut step = self.fade_step;
            let tap = self.target as i32;
            self.pending = false;
            self.fading = true;
            if self.first_tap {
                step = -step;
                self.first_tap = false;
                self.taps[1] = tap;
            } else {
                self.first_tap = true;
                self.taps[0] = tap;
            }
            self.fade_by = step;
        }
        let w = self.write;
        self.lines[0][w as usize] = self.feedback * self.fed[0] + *l;
        self.lines[1][w as usize] = self.feedback * self.fed[1] + *r;
        let [left, right] = &self.lines;
        let (wet_l, wet_r);
        if !self.keep_pitch {
            self.now += (self.target - self.now) * self.glide;
            let whole = self.now as i64 as u32;
            let f = self.now - whole as f32;
            let a = (w.wrapping_sub(whole) & mask) as usize;
            let b = (w.wrapping_sub(whole).wrapping_sub(1) & mask) as usize;
            let rest = 1.0 - f;
            wet_l = rest * left[a] + f * left[b];
            wet_r = rest * right[a] + f * right[b];
        } else {
            let f = self.fade;
            let a = (w.wrapping_sub(self.taps[0] as u32) & mask) as usize;
            let b = (w.wrapping_sub(self.taps[1] as u32) & mask) as usize;
            let rest = 1.0 - f;
            wet_l = f * left[a] + rest * left[b];
            wet_r = f * right[a] + rest * right[b];
            if self.fading {
                self.fade = f + self.fade_by;
                if self.fade < 0.0 {
                    self.fade = 0.0;
                    self.fading = false;
                } else if self.fade > 1.0 {
                    self.fade = 1.0;
                    self.fading = false;
                }
            }
        }
        self.write = w.wrapping_add(1) & mask;
        for (side, wet) in [wet_l, wet_r].into_iter().enumerate() {
            let [b0, b1, a1] = self.low;
            let [y1, x1] = self.low_state[side];
            let low = (b1 * x1 + wet * b0) + a1 * y1;
            self.low_state[side] = [low, wet];
            let [b0, b1, a1] = self.high;
            let [y1, x1] = self.high_state[side];
            let high = (b1 * x1 + low * b0) + a1 * y1;
            self.high_state[side] = [high, low];
            self.fed[side] = high;
        }
        let s = self.stereo;
        let rest = 1.0 - s;
        let gain_l = (f32::from_bits(0x4025_7a06) - rest * f32::from_bits(0x3f95_e81a)) * rest;
        let gain_r = (f32::from_bits(0x4025_7a06) - s * f32::from_bits(0x3f95_e81a)) * s;
        let out_r = self.mix * wet_r * gain_r + gain_l * *r;
        let out_l = self.mix * wet_l * gain_l + gain_r * *l;
        *l = out_l;
        *r = out_r;
    }
}

/// The engine, `E` at the VST's `TestPlug+0xf0`.
#[derive(Debug, Clone, PartialEq)]
struct Engine {
    /// The parameters as the engine has them, 0 to 1 (`E+0x218`).
    values: [f32; COUNT],
    rate: f64,
    over_rate: f64,
    /// 4, or 8 with high quality.
    factor: usize,
    v2: bool,

    // The LFO.
    lfo_on: bool,
    sine: bool,
    sync: bool,
    resync: bool,
    last_position: f64,
    beats: f64,
    phase: f64,
    step: f64,
    min_hz: f32,
    octaves: f32,
    manual_hz: f32,
    tension: [f32; 3],
    lfo: [f32; 2],

    mono: bool,
    colour: Biquad,
    colour_state: [Section; 2],
    input: Biquad,
    input_state: [Section; 2],
    gain: f32,
    gain_to: f32,
    glide: f32,
    up: [[Upsampler; 2]; 2],
    feedback: f32,
    feedback_hp: Biquad,
    feedback_state: [Section; 2],
    last: [f32; 2],
    allpasses: Allpasses,
    ladder: Ladder,
    halfband_six: HalfbandSix,
    halfband_two: HalfbandTwo,
    mix: f32,

    noise: f32,
    seeds: [i32; 2],
    hiss: [Biquad; 2],
    hiss_state: [Section; 2],
    gate: bool,
    gate_threshold: f32,
    gate_count: i32,
    gate_gain: f32,
    delay_on: bool,
    delay: Delay,
}

impl Engine {
    /// A new engine (`FUN_18001a250`) whose noise seeds are those of the
    /// `n`th one a process loads.
    fn new(n: i32) -> Self {
        let seed = FIRST_SEED.wrapping_add(0xb66ad_i32.wrapping_mul(n));
        let mut values = [0.0; COUNT];
        values[0] = f32::from_bits(0x3edf_fffb);
        values[1] = f32::from_bits(0x3f46_9efc);
        for (i, v) in [
            (2, 0.5),
            (15, 0.5),
            (21, 0.5),
            (30, 1.0),
            (31, 1.0),
            (33, 1.0),
        ] {
            values[i] = v;
        }
        let mut engine = Engine {
            values,
            rate: 44_100.0,
            over_rate: 1.0 / 44_100.0,
            factor: 8,
            v2: false,
            lfo_on: true,
            sine: false,
            sync: false,
            resync: false,
            last_position: -1.0,
            beats: 1.0,
            phase: 0.0,
            step: 0.0,
            min_hz: 0.0,
            octaves: 0.0,
            manual_hz: 0.0,
            tension: [0.0; 3],
            lfo: [0.0; 2],
            mono: false,
            colour: Biquad::new(44_100.0),
            colour_state: [[0.0; 4]; 2],
            input: Biquad::new(44_100.0),
            input_state: [[0.0; 4]; 2],
            gain: 1.0,
            gain_to: 1.0,
            glide: 1.0,
            up: [
                [Upsampler::new(4), Upsampler::new(4)],
                [Upsampler::new(8), Upsampler::new(8)],
            ],
            feedback: 0.0,
            feedback_hp: Biquad::new(44_100.0 * 8.0),
            feedback_state: [[0.0; 4]; 2],
            last: [0.0; 2],
            allpasses: Allpasses::default(),
            ladder: Ladder::default(),
            halfband_six: HalfbandSix::new(),
            halfband_two: HalfbandTwo::new(),
            mix: 0.5,
            noise: 0.0,
            seeds: [seed, seed.wrapping_add(0x1099e)],
            hiss: [Biquad::new(44_100.0); 2],
            hiss_state: [[0.0; 4]; 2],
            gate: false,
            gate_threshold: f32::from_bits(0x3b23_d70a),
            gate_count: 0,
            gate_gain: 0.0,
            delay_on: false,
            delay: Delay::new(),
        };
        engine.set_rate(44_100.0);
        engine
    }

    fn up_index(&self) -> usize {
        usize::from(self.factor == 8)
    }

    /// `FUN_180019560`: the rate, then every parameter again.
    fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
        self.over_rate = 1.0 / rate;
        let rest = f64::from_bits(0x3fa9_9999_9999_99a0).powf(20.0 / rate);
        self.glide = (1.0 - rest) as f32;
        self.delay.set_rate(rate);
        let over = rate * self.factor as f64;
        self.colour.rate = rate;
        self.input.rate = rate;
        self.hiss = [Biquad {
            rate,
            ..self.hiss[0]
        }; 2];
        self.feedback_hp.rate = over;
        self.allpasses.over = 1.0 / over;
        self.ladder.turn = PI_F32 / over as f32;
        let up = self.up_index();
        for side in &mut self.up[up] {
            side.clear();
        }
        for index in 0..COUNT {
            self.set(index, self.values[index]);
        }
    }

    /// `FUN_180018a00`: the LFO's range.
    fn set_range(&mut self) {
        let lo = 2f32.powf(self.values[0] * 14.0) * 1.25;
        let hi = 2f32.powf(self.values[1] * 14.0) * 1.25;
        let (lo, hi) = if hi < lo { (hi, lo) } else { (lo, hi) };
        self.min_hz = lo;
        self.octaves = (f64::from(hi / lo).ln() * std::f64::consts::LOG2_E) as f32;
    }

    /// `FUN_180018b80`: the LFO's rate in Hz, between the two rate wheels.
    fn lfo_hz(&self) -> f32 {
        let wheel = |v: f32| {
            let v2 = v * v;
            v2 * v2 * f32::from_bits(0x4315_f99a) + f32::from_bits(0x3ccc_cccd)
        };
        let (one, two) = (wheel(self.values[30]), wheel(self.values[29]));
        let (hi, lo) = (one.max(two), one.min(two));
        let octaves = (f64::from(hi / lo).ln() * std::f64::consts::LOG2_E) as f32;
        2f32.powf(octaves * self.values[6]) * lo
    }

    /// `FUN_18001cf50`: the tension's curve.
    fn set_tension(&mut self, t: f32) {
        let x = (t + t) - 1.0;
        let sign = if x > 0.0 { 0.1 } else { -0.1 };
        let k = ((f64::from(x.abs()) * f64::from_bits(0x4021_82f0_07df_5c92)).exp() - 1.0) as f32
            * sign;
        if k != self.tension[0] {
            self.tension[0] = k;
            if k != 0.0 {
                self.tension[1] = 1.0 / k.abs();
                self.tension[2] = (k.abs() + 1.0).ln();
            }
        }
    }

    /// `FUN_18001ceb0`.
    fn tense(&self, p: f32) -> f32 {
        let [k, scale, log] = self.tension;
        if k == 0.0 {
            p
        } else if k > 0.0 {
            let e = f64::from((1.0 - p) * log).exp();
            (1.0 - (e - 1.0) * f64::from(scale)) as f32
        } else {
            ((f64::from(log * p).exp() - 1.0) * f64::from(scale)) as f32
        }
    }

    /// The engine's setParameter (`FUN_180018cd0`), by its own index.
    fn set(&mut self, index: usize, v: f32) {
        self.values[index] = v;
        match index {
            0 | 1 => self.set_range(),
            2 | 35 => {
                let fb = self.values[2] * 0.9;
                self.feedback = if self.values[35] >= 0.5 { -fb } else { fb };
            }
            3 => self.lfo_on = v < 0.5,
            4 => self.manual_hz = 2f32.powf(v * 14.0) * 1.25,
            5 => {
                let factor = if v >= 0.5 { 8 } else { 4 };
                if factor != self.factor {
                    self.factor = factor;
                    self.set_rate(self.rate);
                }
            }
            6 | 29 | 30 => {
                self.step = f64::from(self.lfo_hz()) / self.rate;
                let notes = 2f32.powf((1.0 - self.values[6]) * 7.0);
                let ticks = ((notes - 1.0) * f32::from_bits(0x4241_7af6) + 1.1) as i32;
                self.beats = f64::from(ticks) * f64::from_bits(0x3f75_5555_5555_5555);
                if self.sync {
                    self.resync = false;
                }
            }
            7 => {
                self.resync = false;
                self.sync = v >= 0.5;
            }
            8 => self.sine = v >= 0.5,
            9 => self.set_tension((v + 1.0) * 0.5),
            12 => self.delay_on = v >= 0.5,
            13 => self.delay.set_keep_pitch(v),
            14 => self.delay.mix = v,
            15 => self.delay.stereo = v,
            16 => {
                self.delay.time = v;
                self.delay.dirty = true;
            }
            17 => self.delay.synced = v >= 0.5,
            18 => self.delay.feedback = v,
            19 => self.delay.set_low(v),
            20 => self.delay.set_high(v),
            21 => self.mix = v,
            22 => {
                self.noise = if v != 0.0 {
                    10f32.powf(v * 2.5 - 4.5) * f32::from_bits(0x3eb3_3333)
                } else {
                    0.0
                };
                self.gate_threshold = self.noise * 8.0;
            }
            23 => self.mono = v >= 0.5,
            24 => self.gate = v >= 0.5,
            25 | 26 => {
                let hz = self.values[26] * 600.0 + 50.0;
                let db = self.values[25] * 30.0;
                self.colour.low_shelf(f64::from(hz), 0.5, f64::from(db));
            }
            27 => {
                let hz = (v * v) * (v * 195.0) + 5.0;
                self.input.high_pass(f64::from(hz));
            }
            28 => {
                let hz = (v * v) * (v * 195.0) + 5.0;
                self.ladder.set_high_pass(hz);
                self.feedback_hp.high_pass(f64::from(hz));
            }
            33 | 34 => {
                if index == 33 {
                    self.v2 = v >= 0.5;
                }
                self.gain_to = if self.v2 {
                    10f32.powf(self.values[34] * 1.5 - 0.75)
                } else {
                    1.0
                };
            }
            _ => {}
        }
    }

    /// `FUN_180001190`: the LFO's frame, and the allpasses' frequencies.
    fn lfo(&mut self) {
        let hz = if self.lfo_on {
            let p = self.phase as f32;
            let left = self.tense(shape(self.sine, p));
            let right = p + self.values[11];
            let right = self.tense(shape(self.sine, right - right as i32 as f32));
            self.lfo = [left, right];
            let speed = self.values[6];
            let depth = (1.0 - speed) * (1.0 - self.values[31]) + self.values[31];
            let offset = (1.0 - depth) - speed * self.values[32] * speed * (speed * speed);
            self.phase += self.step;
            self.phase -= f64::from(self.phase as i32);
            [left, right].map(|l| exp2_cubic((depth * l + offset) * self.octaves) * self.min_hz)
        } else {
            self.lfo = [0.0; 2];
            [self.manual_hz; 2]
        };
        if self.v2 {
            self.ladder.set(hz, self.feedback);
        } else {
            self.allpasses.set(0, hz[0]);
            self.allpasses.set(1, hz[1]);
        }
    }

    /// `FUN_180018210`: a block, starting `position` beats into the song
    /// or at [`STOPPED`], with the song's tempo.
    fn block(&mut self, left: &mut [f32], right: &mut [f32], position: f64, tempo: f64) {
        self.delay.tempo = tempo;
        if self.last_position < 0.0 && position >= 0.0 {
            self.resync = true;
        }
        self.last_position = position;
        if self.sync {
            if self.resync && position >= 0.0 {
                let p = position / self.beats + f64::from(self.values[10]);
                self.phase = p - f64::from(p as i32);
            }
            self.step = tempo * f64::from_bits(0x3f91_1111_1111_1111) / self.beats * self.over_rate;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            [*l, *r] = self.frame([*l, *r]);
        }
    }

    fn frame(&mut self, input: [f32; 2]) -> [f32; 2] {
        self.lfo();
        let mut x = input;
        if self.mono {
            let both = (x[1] + x[0]) * 0.5;
            x = [both; 2];
        }
        let mut wet = [0.0; 2];
        let gain = self.gain;
        self.gain = (self.gain_to - gain) * self.glide + gain;
        let up = self.up_index();
        let mut at = 0;
        for (side, x) in x.into_iter().enumerate() {
            let y = self.colour.run(x, &mut self.colour_state[side]);
            let y = self.input.run(y, &mut self.input_state[side]);
            at = self.up[up][side].push(gain * y);
        }
        let factor = self.factor;
        let mix = self.mix;
        let [left, right] = &mut self.up[up];
        let (left, right) = (
            &mut left.ring[at..at + factor],
            &mut right.ring[at..at + factor],
        );
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let dry = [*l, *r];
            let y = if self.v2 {
                [self.ladder.run(0, dry[0]), self.ladder.run(1, dry[1])]
            } else {
                let mut fed = [0.0; 2];
                for side in 0..2 {
                    let x = self.feedback * self.last[side] + dry[side];
                    fed[side] = self.feedback_hp.run(x, &mut self.feedback_state[side]);
                }
                let y = self.allpasses.run(fed);
                self.last = y;
                y
            };
            *l = mix * y[0] + (1.0 - mix) * dry[0];
            *r = mix * y[1] + (1.0 - mix) * dry[1];
        }
        if self.v2 {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                (*l, *r) = self.halfband_six.run(*l, *r);
            }
            if factor == 8 {
                for i in [0, 4] {
                    (left[i], right[i]) = self.halfband_two.run(left[i], right[i]);
                }
            }
        }
        let over = 1.0 / self.gain;
        wet[0] = over * left[0];
        wet[1] = over * right[0];

        let mut hiss = [0.0; 2];
        if self.noise > 0.0 {
            let base = f32::from_bits(0x404d_c2c5);
            let hz_r = exp2_signed(base * self.lfo[1]) * 280.0;
            let hz_l = exp2_signed(base * self.lfo[0]) * 280.0;
            self.hiss[0].low_shelf(f64::from(hz_l), 0.5, 18.0);
            self.hiss[1].low_shelf(f64::from(hz_r), 0.5, 18.0);
            for side in 0..2 {
                let seed = &mut self.seeds[side];
                *seed = seed.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b);
                let n = *seed as f32 * f32::from_bits(0x3000_0000);
                let h = self.hiss[side].run(n, &mut self.hiss_state[side]) * self.noise;
                hiss[side] = h;
                wet[side] += h;
            }
        }
        if self.gate {
            let t = self.gate_threshold;
            if wet[0].abs() >= t
                || wet[1].abs() >= t
                || (hiss[0] + x[0]).abs() >= t
                || (hiss[1] + x[1]).abs() >= t
            {
                self.gate_count = 440;
                self.gate_gain = 1.0;
            }
            if self.gate_count > 0 {
                self.gate_count -= 1;
            } else {
                self.gate_gain *= f32::from_bits(0x3f7f_e931);
            }
            wet = wet.map(|w| self.gate_gain * w);
        }
        if self.delay_on {
            let [l, r] = &mut wet;
            self.delay.run(l, r);
        }
        wet
    }
}

/// Vintage Phaser. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct VintagePhaser {
    params: [i32; COUNT],
    hz: f32,
    engine: Engine,
    tempo: f64,
    ppq: u16,
    /// Where the song is, in beats, while it plays.
    position: Option<f64>,
}

impl Default for VintagePhaser {
    fn default() -> Self {
        let mut plugin = VintagePhaser {
            params: DEFAULTS,
            hz: 44_100.0,
            engine: Engine::new(0),
            tempo: 120.0,
            ppq: 96,
            position: None,
        };
        plugin.apply();
        plugin
    }
}

impl VintagePhaser {
    /// What the shell hands the engine for a parameter: its place in its
    /// range, 0 to 1.
    fn value(index: usize, value: i32) -> f32 {
        let (min, max) = RANGES[index];
        let v = f64::from(value - min) * (1.0 / f64::from(max - min));
        v.clamp(0.0, 1.0) as f32
    }

    /// The engine's index of the shell's parameter: the VST swaps 22 and 23.
    fn engine_index(index: usize) -> usize {
        match index {
            22 => 23,
            23 => 22,
            index => index,
        }
    }

    fn send(&mut self, index: usize) {
        let v = Self::value(index, self.params[index]);
        self.engine.set(Self::engine_index(index), v);
    }

    /// Every parameter to the engine, in order, as the shell does after a
    /// state.
    fn apply(&mut self) {
        for index in 0..COUNT {
            self.send(index);
        }
    }
}

impl Effect for VintagePhaser {
    /// The filters, lines and LFO stay as they are, as in FL.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN_3)?;
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        let expected = match version {
            ..=1 => LEN_1,
            2 => LEN_2,
            _ => LEN_3,
        };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let words = bytes[4..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|w| i32::from_le_bytes(*w));
        let mut params = self.params;
        if version < 2 {
            for (word, index) in words.zip(ORDER_1) {
                params[index] = word;
            }
            // The noise gate from its control's default, and V1 with no
            // drive and the feedback upright.
            params[24] = 0;
            params[33..].fill(0);
        } else {
            for (param, word) in params.iter_mut().zip(words) {
                *param = word;
            }
        }
        // FL's own presets have values under a wheel's minimum (a manual
        // frequency of 0), which the shell keeps and clamps as it hands
        // them on, so no value is turned down.
        self.params = params;
        self.apply();
        Ok(())
    }

    /// The engine works out its filters again and takes every parameter
    /// again. Its upsamplers and delay lines start from silence.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 {
            self.hz = hz;
            self.engine.set_rate(f64::from(hz));
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm;
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        self.ppq = ppq.max(1);
    }

    fn tick(&mut self, ppq_tick: u64, tempo: f64) {
        self.set_tempo(tempo);
        self.position = Some(ppq_tick as f64 / f64::from(self.ppq));
    }

    /// A synced LFO or delay wants the song's place.
    fn follows_song(&self) -> bool {
        (self.engine.sync && self.engine.lfo_on)
            || (self.engine.delay_on && self.engine.delay.synced)
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let position = self.position.unwrap_or(STOPPED);
        self.engine.block(left, right, position, self.tempo);
        if let Some(position) = &mut self.position {
            *position += self.tempo / 60.0 / f64::from(self.hz) * left.len() as f64;
        }
    }

    fn param_count(&self) -> usize {
        COUNT
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.send(index);
        }
    }

    /// FL's engine keeps its state through a flush: only the song stops.
    fn reset(&mut self) {
        self.position = None;
    }

    fn latency(&self) -> usize {
        LATENCY
    }

    fn fidelity(&self) -> Fidelity {
        if self.engine.noise > 0.0 {
            Fidelity::Approximate("hiss seeds of the first engine FL loads")
        } else if self.follows_song() {
            Fidelity::Approximate("tempo sync follows the host's ticks")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, partial, presets, sine, state};

    /// Stroll Up's state ("Lowgrounder"), less its version and editor byte.
    const STROLL_UP: [i32; COUNT] = [
        716_925, 2_048_000, 1320, 0, 1_182_845, 0, 1, 0, 1, 3030, 0, 0, 0, 1, 0, 864, 1, 0, 0,
        31_616, 5328, 100, 0, 0, 0, 2145, 77_200, 58_100, 80_000, 66_220, 25, 0, 0, 1, 5000, 0,
    ];

    fn bytes(params: [i32; COUNT]) -> Vec<u8> {
        let mut bytes = state(&[3]);
        bytes.extend(state(&params));
        bytes.push(0);
        bytes
    }

    fn with(params: [i32; COUNT]) -> VintagePhaser {
        let mut plugin = VintagePhaser::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin
    }

    fn edit(edits: &[(usize, i32)]) -> [i32; COUNT] {
        let mut params = DEFAULTS;
        for (index, value) in edits {
            params[*index] = *value;
        }
        params
    }

    fn run(plugin: &mut VintagePhaser, l: &[f32], r: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (l.to_vec(), r.to_vec());
        for (l, r) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            plugin.process(l, r);
        }
        (l, r)
    }

    /// A saw and an LCG's noise, in f32 arithmetic only, so that the
    /// emulated DLL had the same input to the bit.
    fn dll_input() -> (Vec<f32>, Vec<f32>) {
        let mut seed = 1u32;
        (0..2048u32)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 8) as f32 / 16_777_216.0 - 0.5;
                let saw = ((i % 200) as f32 / 100.0 - 1.0) * 0.4;
                (saw + noise * 0.25, saw * -0.5 + noise * 0.5)
            })
            .unzip()
    }

    /// What `engine_x64.dll` itself gives for Stroll Up's state at 44.1 kHz
    /// in blocks of 256, run under an emulator (`tools/plugstate/vst_emu.py`):
    /// every 16th sample of the first 2048 the input of [`dll_input`] makes,
    /// from the 16th.
    #[rustfmt::skip]
    const DLL_LEFT: [u32; 128] = [
        0xb79a_cd78, 0x36e3_71a6, 0xbb7d_b0ec, 0xbea2_328b, 0xbf02_74fa, 0xbea4_b7fc,
        0xbe7e_9942, 0xbe76_2d6a, 0xbe2c_b85b, 0x3d60_e653, 0x3d48_f5ad, 0x3e86_d57b,
        0x3ec8_4708, 0x3eb8_ae6a, 0x3f36_b7e3, 0x3f83_3421, 0xbeaa_743f, 0xbebb_1556,
        0xbeba_8844, 0xbe9a_911f, 0xbe1f_1cfc, 0x3e1c_e7e7, 0x3e22_78a0, 0x3e6f_2ce2,
        0x3e6b_9408, 0x3ec9_a86f, 0x3ed4_e183, 0x3ef4_0335, 0xbe45_4db2, 0xbef1_bcc3,
        0xbed0_95a0, 0xbea8_5dfb, 0xbebf_9738, 0xbd44_208a, 0xbd93_7c01, 0x3dbb_f2af,
        0x3e59_d734, 0x3ebe_ddc9, 0x3e91_a1c9, 0x3e75_b17d, 0x3f72_2515, 0xbea7_ea33,
        0xbed1_34b0, 0xbea9_97a3, 0xbe9b_f3b8, 0xbe76_afe9, 0xbe21_08f4, 0xbb98_3c5a,
        0x3e1d_1acf, 0x3ea1_31be, 0x3ea6_8a85, 0x3e9d_429b, 0x3ebb_f7a3, 0xbdc6_1edd,
        0xbf07_cead, 0xbec8_2e72, 0xbec3_30b6, 0xbe02_4eec, 0xbd88_26e7, 0xbd78_4c0d,
        0x3dc2_bf59, 0x3e13_d1d1, 0x3e6d_0512, 0x3ebd_f655, 0x3ee3_3028, 0x3f58_324d,
        0xbedd_284e, 0xbee2_decf, 0xbeb1_2426, 0xbea7_ddb8, 0xbe0b_fd8c, 0xbd5c_6807,
        0x3d3b_4712, 0x3e01_5515, 0x3e8b_b877, 0x3e80_fbd3, 0x3ee0_2f7c, 0x3eed_70c7,
        0xbe3b_2b5b, 0xbeed_4408, 0xbed9_b268, 0xbe7f_45b9, 0xbe8f_705e, 0xbe2a_1a53,
        0xbdb0_5f8e, 0x3da7_c866, 0x3e82_dc35, 0x3dcf_bbfd, 0x3ea8_4144, 0x3ed8_6d65,
        0x3f68_54f9, 0xbeb0_5f41, 0xbee1_2d9a, 0xbed8_23b7, 0xbe96_aad7, 0xbd8b_8b9b,
        0xbdf1_9cc5, 0xbdbb_5309, 0x3e49_956d, 0x3e63_a392, 0x3ea2_69fb, 0x3ec2_4982,
        0x3ea4_4817, 0xbe21_a352, 0xbf08_ccda, 0xbed6_7484, 0xbea6_e3ef, 0xbe9d_d1f9,
        0xbe30_9847, 0xbd61_2713, 0x3d30_4452, 0x3e47_490c, 0x3e6c_9d25, 0x3eb7_6a9e,
        0x3edc_ae61, 0x3f64_55c4, 0xbed0_3078, 0xbef0_f0f5, 0xbec7_c9e8, 0xbe75_b4ad,
        0xbe3d_2799, 0xbe49_4ac9, 0xbd2c_3708, 0x3e34_2cc3, 0x3e3a_bd01, 0x3e7b_c22a,
        0x3ef4_374e, 0x3efe_8bc0,
    ];
    #[rustfmt::skip]
    const DLL_RIGHT: [u32; 128] = [
        0xb777_49ff, 0x380b_3ba3, 0x3a64_4767, 0x3e8d_1c65, 0x3e35_1224, 0x3ebb_72ef,
        0x3e81_6c01, 0xbc1f_36c3, 0xbe15_9216, 0x3cae_6d05, 0xbe82_f837, 0xbdc4_4801,
        0xbdb7_dff3, 0xbec3_336e, 0x3dc8_176c, 0xbe93_82a2, 0x3dce_3144, 0x3e86_f28f,
        0x3dfd_98b0, 0x3c9e_ef3a, 0x3d7e_1827, 0x3ed5_714d, 0x3e35_4e62, 0x3d9c_3767,
        0xbe31_feb6, 0xbdad_b5c9, 0xbe87_99ab, 0xbeae_32c4, 0xbd17_d2bb, 0x3e2c_2097,
        0x3e71_2640, 0x3e29_135c, 0xbe30_af4d, 0x3e62_6188, 0xbdaf_6f78, 0xbc9e_7783,
        0xbcf6_afa3, 0x3d28_5ab7, 0xbebc_f4f9, 0xbf2d_95a5, 0xbe96_8cca, 0x3e7d_35fc,
        0x3e96_eaa0, 0x3e97_9cbf, 0x3de2_bf1a, 0xbc89_2736, 0xbde3_c100, 0xbd88_4c5b,
        0xbc02_2f32, 0x3d80_7bad, 0xbe21_01ab, 0xbed7_cbaa, 0xbf00_af91, 0x3e4e_aba5,
        0x3da8_8514, 0x3e91_daf4, 0x3d9f_0a72, 0x3eab_add3, 0x3e4d_87b2, 0xbd3a_6645,
        0x3bf5_c34a, 0xbe16_78e0, 0xbe5f_b775, 0xbe35_5d11, 0xbe81_02f7, 0xbeed_3978,
        0x3d31_f929, 0x3e5f_bd3b, 0x3e83_eb0b, 0x3d79_3322, 0x3e40_ce51, 0x3dc3_e58e,
        0x3d1c_fe73, 0xbd66_6792, 0xbc3f_b715, 0xbe95_9b8a, 0xbe18_89af, 0xbe97_0da3,
        0x3cf5_9569, 0x3e50_57c5, 0x3e51_968c, 0x3ea8_fc6f, 0x3cc4_45d0, 0xba99_c2ab,
        0xbdc7_3df4, 0xbc87_c559, 0x3da3_5d2f, 0xbef1_2b89, 0xbe7f_eed9, 0xbe90_6d47,
        0xbe9d_9266, 0x3e58_5315, 0x3e57_d379, 0x3dbf_10ac, 0x3df0_536c, 0x3ea3_6343,
        0xbd0c_412b, 0xbe71_6422, 0x3db4_b7b8, 0xbdda_43f2, 0xbe1d_50cc, 0xbe82_6711,
        0xbf11_6722, 0x3da3_e193, 0x3d50_2205, 0x3e50_799d, 0x3e2a_822e, 0xbd24_8fe0,
        0xbc95_c0ce, 0xbd14_39a7, 0xbdbd_f981, 0xbd1b_858e, 0xbe52_7933, 0xbe3a_1703,
        0xbe82_3037, 0xbe9f_72fa, 0x3da5_fa49, 0x3e0c_1350, 0x3e15_6828, 0x3e5e_c751,
        0x3da6_2f3e, 0xbe46_403b, 0xbe0b_e276, 0x3d4a_c47c, 0xbe39_905a, 0xbe94_0d8a,
        0xbd4f_e403, 0xbe53_3d58,
    ];

    #[test]
    fn stroll_up_s_phaser_is_the_dll_s() {
        let (l, r) = dll_input();
        let mut plugin = with(STROLL_UP);
        let (l, r) = run(&mut plugin, &l, &r, 256);
        // To the bit with glibc's libm; any other libm may round a
        // parameter's curve another way.
        for (got, want) in [(&l, &DLL_LEFT), (&r, &DLL_RIGHT)] {
            for (n, want) in want.iter().enumerate() {
                let (got, want) = (got[n * 16 + 15], f32::from_bits(*want));
                assert!((got - want).abs() <= 1e-5, "{n}: {got} {want}");
            }
        }
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        assert_eq!(plugin.latency(), 44);
    }

    #[test]
    fn the_wheels_go_where_the_doc_says() {
        let e = with(edit(&[(0, 125), (1, 2_048_000), (29, 25), (30, 150_000)])).engine;
        assert_eq!(e.min_hz, 1.25);
        assert!((e.octaves - 14.0).abs() < 1e-5);
        // The speed between the rate wheels, 0.025 to 150 Hz, in octaves.
        assert!((e.step * 44_100.0 - 0.025).abs() < 1e-6);
        let e = with(edit(&[(6, 245_760), (29, 25), (30, 150_000)])).engine;
        assert!(
            (e.step * 44_100.0 - 150.0).abs() < 1e-3,
            "{}",
            e.step * 44_100.0
        );
        // Synced, the slowest is 32 bars of 4 and the fastest a 192nd.
        assert_eq!(with(edit(&[(6, 1)])).engine.beats, 6144.0 / 192.0);
        assert_eq!(with(edit(&[(6, 245_760)])).engine.beats, 1.0 / 192.0);
        // The ends swap when the minimum is over the maximum.
        let e = with(edit(&[(0, 2_048_000), (1, 125)])).engine;
        assert_eq!(e.min_hz, 1.25);
        let e = with(edit(&[(4, 2_048_000), (34, 650_000), (2, 6000), (35, 1)])).engine;
        assert_eq!(e.manual_hz, 20_480.0);
        assert!((e.gain_to - 10f32.powf(0.75)).abs() < 1e-5);
        assert_eq!(e.feedback, -0.9);
        assert_eq!(with(edit(&[(33, 0), (34, 650_000)])).engine.gain_to, 1.0);
    }

    #[test]
    fn with_the_mix_at_0_the_input_comes_out_44_frames_late() {
        // V1, no mix: what comes out is the upsampled input, read every
        // fourth sample, which the sinc puts back where it was.
        let mut plugin = with(edit(&[(33, 0), (21, 0)]));
        let mut impulse = vec![0.0; 200];
        impulse[10] = 1.0;
        let (l, r) = run(&mut plugin, &impulse, &impulse, 64);
        assert_eq!(l, r);
        for (n, v) in l.iter().enumerate() {
            // The 5 Hz high-pass takes a little off.
            let want = if n == 54 { 1.0 } else { 0.0 };
            assert!((v - want).abs() < 1e-3, "{n} {v}");
        }
    }

    #[test]
    fn the_notches_are_where_the_four_allpasses_turn_half_a_cycle() {
        // V1 at a fixed frequency, no feedback, half wet: four allpasses
        // of a quarter cycle each at the corner add up in phase, and an
        // eighth or three eighths each cancel.
        let p = with(edit(&[(33, 0), (3, 1), (4, 1_400_000), (2, 0), (21, 50)]));
        let corner = f64::from(p.engine.manual_hz);
        let over = 44_100.0 * 4.0;
        let t = (std::f64::consts::PI * corner / over).tan();
        let at = |eighths: f64| {
            let k = (t * (std::f64::consts::PI * eighths / 8.0).tan()).atan();
            over / std::f64::consts::PI * k
        };
        let len = 16_384;
        for (hz, want) in [
            (at(1.0), 0.0),
            (corner, 1.0),
            (at(3.0), 0.0),
            (at(4.0) * 0.6, -1.0),
        ] {
            let turns = (hz / 44_100.0 * len as f64).round() / len as f64;
            let tone = sine(turns, len * 2);
            let mut plugin = p.clone();
            let (l, _) = run(&mut plugin, &tone, &tone, 512);
            let size = partial(&l[len..], turns);
            if want >= 0.0 {
                assert!((size - want).abs() < 0.03, "{hz} Hz: {size}");
            } else {
                assert!(size > 0.1 && size < 0.95, "{hz} Hz: {size}");
            }
        }
    }

    #[test]
    fn the_halfbands_pass_low_and_stop_the_top() {
        for (dc, top) in [(1.0f32, 0.0f32)] {
            let (mut six, mut two) = (HalfbandSix::new(), HalfbandTwo::new());
            let (mut a, mut b) = ((0.0, 0.0), (0.0, 0.0));
            for n in 0..4000 {
                a = six.run(dc, if n % 2 == 0 { 1.0 } else { -1.0 });
                b = two.run(dc, if n % 2 == 0 { 1.0 } else { -1.0 });
            }
            assert!(
                (a.0 - dc).abs() < 1e-4 && (b.0 - dc).abs() < 1e-4,
                "{a:?} {b:?}"
            );
            assert!(
                (a.1 - top).abs() < 1e-3 && (b.1 - top).abs() < 1e-3,
                "{a:?} {b:?}"
            );
        }
    }

    #[test]
    fn the_ladder_is_quiet_when_quiet_and_bounded_when_loud() {
        // V2 with the most drive and feedback still comes out bounded:
        // every stage goes through a tanh.
        let mut plugin = with(edit(&[(34, 650_000), (2, 6000), (5, 1)]));
        let loud = sine(0.01, 20_000)
            .iter()
            .map(|v| v * 4.0)
            .collect::<Vec<_>>();
        let (l, r) = run(&mut plugin, &loud, &loud, 333);
        assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 40.0));
        let mut plugin = with(edit(&[]));
        let (l, _) = run(&mut plugin, &[0.0; 5000], &[0.0; 5000], 100);
        assert!(l.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_shapes_and_the_tension_run_from_0_to_1() {
        assert_eq!(shape(false, 0.0), 0.5);
        assert_eq!(shape(false, 0.25), 1.0);
        assert_eq!(shape(false, 0.75), 0.0);
        assert!((shape(true, 0.25) - 1.0).abs() < 1e-6);
        let mut e = with(edit(&[(9, 6000)])).engine;
        assert!(
            e.tense(0.5) > 0.5 && (e.tense(1.0) - 1.0).abs() < 1e-6 && e.tense(0.0).abs() < 1e-5
        );
        e.set_tension(0.25);
        assert!(e.tense(0.5) < 0.5 && (e.tense(1.0) - 1.0).abs() < 1e-6);
        e.set_tension(0.5);
        assert_eq!(e.tense(0.3), 0.3);
        // The fast 2^x is within a part in a thousand.
        for x in [0.0f32, 0.3, 1.7, 5.5, 13.9] {
            let got = f64::from(exp2_cubic(x));
            assert!((got / 2f64.powf(f64::from(x)) - 1.0).abs() < 1e-3, "{x}");
        }
        assert_eq!(exp2_signed(0.0), 1.0);
        assert!((exp2_signed(-1.0) - 0.5).abs() < 1e-3);
        assert!((tanh_pade(1.0) - 1f32.tanh()).abs() < 1e-4);
        assert!((tan_rational(0.7) - 0.7f32.tan()).abs() < 1e-3);
    }

    #[test]
    fn hiss_is_the_engine_s_lcg_and_moves_only_when_heard() {
        let mut plugin = with(edit(&[]));
        let seeds = plugin.engine.seeds;
        assert_eq!(seeds, [0x3a916, 0x3a916 + 0x1099e]);
        run(&mut plugin, &[0.0; 100], &[0.0; 100], 50);
        assert_eq!(plugin.engine.seeds, seeds);
        let mut plugin = with(edit(&[(23, 6000)]));
        // -20 dB times 0.35, through an 18 dB shelf the LFO moves.
        assert!((plugin.engine.noise - 0.0035).abs() < 1e-8);
        let (l, r) = run(&mut plugin, &[0.0; 4000], &[0.0; 4000], 64);
        assert_ne!(plugin.engine.seeds, seeds);
        let peak = l.iter().map(|v| v.abs()).fold(0.0, f32::max);
        assert!(peak > 0.003 && peak < 0.1, "{peak}");
        assert_ne!(l, r);
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(_)));
    }

    #[test]
    fn the_gate_holds_440_samples_then_falls() {
        // The gate is keyed on the output, the input and the hiss, against
        // eight times the hiss's level. Hiss that loud keeps opening it, so
        // take the hiss away and keep the threshold.
        let mut plugin = with(edit(&[(24, 1), (23, 3000), (21, 0), (33, 0)]));
        plugin.engine.noise = 0.0;
        let fall = f32::from_bits(0x3f7f_e931);
        let loud = sine(0.01, 3000);
        run(&mut plugin, &loud, &loud, 100);
        assert_eq!(plugin.engine.gate_gain, 1.0);
        let mut seen = Vec::new();
        for _ in 0..12_000 {
            plugin.process(&mut [0.0], &mut [0.0]);
            seen.push((plugin.engine.gate_count, plugin.engine.gate_gain));
        }
        let first = seen.iter().position(|(_, gain)| *gain < 1.0).unwrap();
        assert!(first > 440);
        assert_eq!(seen[first - 440].0, 439);
        assert_eq!(seen[first - 1], (0, 1.0));
        let mut gain = 1.0;
        for (count, now) in &seen[first..first + 2000] {
            gain *= fall;
            assert_eq!((*count, *now), (0, gain));
        }
    }

    #[test]
    fn the_delay_repeats_at_its_time_and_pans_at_the_middle_to_unity() {
        // Delay on, a step of the wheel (57.5 samples at 44.1 kHz), no
        // feedback, all wet, stereo at the middle: the input and a copy,
        // both at unity.
        let mut plugin = with(edit(&[(21, 0), (33, 0), (12, 1), (14, 6000), (16, 2)]));
        let time = plugin.engine.delay.time * 44_100.0;
        assert!((time - 44_100.0 / 767.0).abs() < 1e-3, "{time}");
        // The delay glides to its time from nothing: let it get there.
        run(&mut plugin, &[0.0; 40_000], &[0.0; 40_000], 512);
        let mut impulse = vec![0.0; 600];
        impulse[10] = 1.0;
        let (l, _) = run(&mut plugin, &impulse, &impulse, 64);
        assert!((l[54] - 1.0).abs() < 2e-3, "{}", l[54]);
        let late = 54 + time as usize;
        let copy = l[late] + l[late + 1];
        assert!(
            (copy - 1.0).abs() < 5e-3,
            "{copy} {:?}",
            &l[late - 2..late + 3]
        );
    }

    #[test]
    fn states_of_every_version_load_and_newer_ones_are_turned_down() {
        let mut plugin = VintagePhaser::default();
        assert_eq!(plugin.set_state(&bytes(STROLL_UP)), Ok(()));
        assert_eq!(plugin.param(15), 864.0);
        let mut two = bytes(STROLL_UP);
        two[0] = 2;
        two.pop();
        assert_eq!(plugin.set_state(&two), Ok(()));
        // Version 1: 32 parameters in another order, the rest at V1's.
        let mut one = state(&[1]);
        let old: Vec<i32> = (0..32).map(|i| RANGES[ORDER_1[i]].0 + 1).collect();
        one.extend(state(&old));
        assert_eq!(plugin.set_state(&one), Ok(()));
        assert_eq!(plugin.param(9), 1.0);
        assert_eq!(plugin.param(21), 1.0);
        assert_eq!(plugin.param(24), 0.0);
        assert_eq!(plugin.param(33), 0.0);
        // A value outside its wheel is kept, and the engine has its end.
        let mut params = STROLL_UP;
        params[4] = 0;
        params[21] = 200;
        assert_eq!(plugin.set_state(&bytes(params)), Ok(()));
        assert_eq!(plugin.param(4), 0.0);
        assert_eq!((plugin.engine.values[4], plugin.engine.mix), (0.0, 1.0));
        let before = plugin.clone();
        for len in [3, 131, 133, 150] {
            let mut wrong = bytes(STROLL_UP);
            wrong.resize(len, 0);
            assert!(matches!(
                plugin.set_state(&wrong),
                Err(StateError::Length { .. })
            ));
        }
        let mut newer = bytes(STROLL_UP);
        newer[0] = 4;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 4 })
        );
        assert_eq!(plugin, before);
    }

    #[test]
    fn a_moved_wheel_is_the_same_plugin_as_its_state_and_asks_for_no_memory() {
        let mut plugin = VintagePhaser::default();
        let memory = plugin.engine.delay.lines[0].as_ptr();
        let ring = plugin.engine.up[0][0].ring.as_ptr();
        for (index, value) in STROLL_UP.iter().enumerate() {
            plugin.set_param(index, *value as f32);
        }
        plugin.set_param(5, 1.0);
        plugin.set_param(5, 0.0);
        plugin.set_param(40, 3.0);
        let same = with(STROLL_UP);
        assert_eq!(plugin.engine.values, same.engine.values);
        assert_eq!(plugin.engine.delay.lines[0].as_ptr(), memory);
        assert_eq!(plugin.engine.up[0][0].ring.as_ptr(), ring);
        assert_eq!(plugin.param_count(), 36);
    }

    #[test]
    fn a_new_rate_keeps_the_filters_and_empties_the_upsamplers() {
        let mut plugin = with(STROLL_UP);
        run(&mut plugin, &[0.3; 1000], &[0.3; 1000], 100);
        let ladder = plugin.engine.ladder;
        plugin.set_sample_rate(96_000.0);
        assert_eq!(plugin.engine.ladder.s, ladder.s);
        assert!(plugin.engine.up[0][0].ring.iter().all(|v| *v == 0.0));
        assert!((plugin.engine.ladder.turn - PI_F32 / 384_000.0).abs() < 1e-12);
        let before = plugin.clone();
        plugin.set_sample_rate(f32::NAN);
        assert_eq!(plugin, before);
    }

    #[test]
    fn sync_counts_a_cycle_in_beats_from_the_song_s_place() {
        let mut plugin = with(edit(&[(7, 1), (6, 200_000), (10, 1500)]));
        assert!(plugin.follows_song());
        let beats = plugin.engine.beats;
        plugin.tick(96 * 3, 120.0);
        plugin.process(&mut [0.0; 10], &mut [0.0; 10]);
        let p = 3.0 / beats + 0.25;
        let step = 120.0 / 60.0 / beats / 44_100.0;
        let want = (p - p.floor() + step * 10.0).fract();
        assert!(
            (plugin.engine.phase - want).abs() < 1e-9,
            "{}",
            plugin.engine.phase
        );
        plugin.reset();
        assert!(plugin.position.is_none());
    }

    #[test]
    fn fl_s_presets_load_and_play() {
        let Some(own) = presets(NAME) else { return };
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert!(!own.is_empty() && all.len() > own.len());
        for preset in &all {
            let mut plugin = VintagePhaser::default();
            assert_eq!(plugin.set_state(&preset.state), Ok(()), "{}", preset.file);
            let (l, r) = run(&mut plugin, &sine(0.01, 6000), &sine(0.02, 6000), 512);
            assert!(
                l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 8.0),
                "{}",
                preset.file
            );
        }
    }
}
