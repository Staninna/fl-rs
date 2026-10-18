//! Autogun's timbre engine (`docs/plugins/autogun.md` section 3): a
//! spectrum of random partial levels drawn from the two timbre seeds,
//! turned into one long wavetable by an inverse FFT, and a new table every
//! frame as each partial decays at its own rate. A voice reads the newest
//! two tables with FL's 6-point interpolator, crossfading from the older to
//! the newer over the frame, once for every unison copy.
//!
//! What is FL's own and not an IPP call is ported as the binary has it:
//! the generic path of `ippsRandUniform_Direct_32f` from `dsp_ippv2_x64.dll`
//! and the Jenkins mix that makes the seeds.

// The spectrum loops index partials and Pack slots side by side, as FL's
// do.
#![allow(clippy::needless_range_loop)]

use std::f64::consts::TAU;

/// `440 * 2^(75 / 12)`, the frequency the table's step is counted from
/// (`FUN_00847470`, kept as an f32).
const TABLE_HZ: f32 = 33_488.07;
/// `ln 0.01`: every decay is to 40 dB down.
const LN_HUNDREDTH: f64 = -4.605_170_185_988_091;
/// Frames of guard after the interleaved table, and the half of it the
/// read pointer starts at.
const GUARD: usize = 64;
const HALF_GUARD: usize = GUARD / 2;
/// FL's "too small to keep": 2^-24 (`DAT_008d77c4`).
const TINY: f32 = 5.960_464_5e-8;

/// The generic path of `ippsRandUniform_Direct_32f` (`dsp_ippv2_x64.dll`
/// export 338, `0x1815ed2e0`): a subtract-with-borrow generator seeded
/// from `seed` added to a fixed LCG, one i32 a sample scaled into
/// `[low, high)`. The seed comes back as the generator's last state.
pub fn rand_uniform(dst: &mut [f32], low: f32, high: f32, seed: &mut i32) {
    const A: u32 = 69_069;
    const C: u32 = 0x3c6e_f373;
    let len = dst.len();
    if len == 0 {
        return;
    }
    let scale = f64::from(high - low) * f64::from_bits(0x3df0_0000_0000_0007);
    let mid = f64::from(low + high) * 0.5;
    let value = |x: u32| (f64::from(x as i32) * scale + mid) as f32;
    let sign = |x: u32| ((x as i32) >> 31) as u32;

    let mut r10 = *seed as u32;
    let mut eax: u32 = 0x436c_bae9;
    let mut r14 = r10.wrapping_mul(A).wrapping_add(C);
    let mut r12 = r14.wrapping_mul(A).wrapping_add(C);
    let mut r11: u32 = u32::MAX;
    let mut r8: u32 = 0;
    let pairs = len / 2;
    for i in 0..pairs {
        r12 = r14.wrapping_sub(r12).wrapping_add(r11);
        r8 = r14.wrapping_neg();
        r11 = sign(r12);
        let first = r12.wrapping_sub(r11 & 0x12);
        eax = eax.wrapping_mul(A);
        r12 = r10;
        let next = eax.wrapping_add(C);
        r8 = r8.wrapping_add(r12).wrapping_add(r11);
        r14 = first;
        let out1 = first.wrapping_add(eax).wrapping_add(C);
        r11 = sign(r8);
        let lcg = next.wrapping_mul(A);
        eax = lcg.wrapping_add(C);
        r8 = r8.wrapping_sub(r11 & 0x12);
        r10 = r8;
        let out2 = r8.wrapping_add(lcg).wrapping_add(C);
        dst[2 * i] = value(out1);
        dst[2 * i + 1] = value(out2);
    }
    if len > 2 * pairs {
        r14 = r14.wrapping_sub(r12).wrapping_add(r11);
        r8 = r14.wrapping_sub(sign(r14) & 0x12);
        eax = eax.wrapping_mul(A);
        dst[len - 1] = value(r8.wrapping_add(eax).wrapping_add(C));
    }
    *seed = r8 as i32;
}

/// The Jenkins mix FL makes a seed of a number with: `key` is the second
/// constant, which differs by use (`0x1cfc` for a seed wheel, `0x1e42`
/// for a seed linked to the key, `0x1dbc` for a random one). The result
/// is the mix's absolute value.
pub fn mix(x: i32, key: u32) -> i32 {
    let a = x as u32;
    let mut u6 = a.wrapping_neg().wrapping_sub(key) ^ a.wrapping_shl(8);
    let mut u7 = 0x1eef_u32.wrapping_sub(a).wrapping_sub(u6) ^ (u6 >> 13);
    let mut u5 = a.wrapping_sub(u6).wrapping_sub(u7) ^ (u7 >> 12);
    u6 = u6.wrapping_sub(u7).wrapping_sub(u5) ^ (u5 << 16);
    u7 = u7.wrapping_sub(u5).wrapping_sub(u6) ^ (u6 >> 5);
    u5 = u5.wrapping_sub(u6).wrapping_sub(u7) ^ (u7 >> 3);
    u6 = u6.wrapping_sub(u7).wrapping_sub(u5) ^ (u5 << 10);
    u6 = u7.wrapping_sub(u5).wrapping_sub(u6) ^ (u6 >> 15);
    (u6 as i32).wrapping_abs()
}

/// FL's denormal flush (`FUN_00629cb0`): anything under 2^-24 is 0.
fn flush(values: &mut [f32]) {
    for v in values {
        if v.abs() < TINY {
            *v = 0.0;
        }
    }
}

/// The inverse of IPP's real FFT in its Pack format, unscaled, as FL asks
/// for it (`ippsFFTInv_PackToR_32f`, flag `IPP_FFT_NODIV_BY_ANY`): one
/// complex radix 2 transform of half the size and the split around it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RealIfft {
    size: usize,
    swaps: Vec<(u32, u32)>,
    /// A turn of the half size: cos and sin of `2 pi k / (size / 2)` for
    /// the first half of it.
    cos: Vec<f32>,
    sin: Vec<f32>,
    /// `e^(2 pi i k / size)` for the split.
    split_cos: Vec<f32>,
    split_sin: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl RealIfft {
    /// `size` is a power of 2, 4 or more. This allocates.
    pub fn new(size: usize) -> Self {
        let half = size / 2;
        let bits = half.trailing_zeros();
        let reversed = |i: usize| i.reverse_bits() >> (usize::BITS - bits);
        let swaps = (0..half).filter(|i| *i < reversed(*i));
        let turn = |k: usize, n: usize| TAU * k as f64 / n as f64;
        RealIfft {
            size,
            swaps: swaps.map(|i| (i as u32, reversed(i) as u32)).collect(),
            cos: (0..half / 2).map(|k| turn(k, half).cos() as f32).collect(),
            sin: (0..half / 2).map(|k| turn(k, half).sin() as f32).collect(),
            split_cos: (0..half).map(|k| turn(k, size).cos() as f32).collect(),
            split_sin: (0..half).map(|k| turn(k, size).sin() as f32).collect(),
            re: vec![0.0; half],
            im: vec![0.0; half],
        }
    }

    /// `x[n] = X0 + X(N/2) (-1)^n + 2 Re sum X(k) e^(2 pi i k n / N)`
    /// from the Pack spectrum `[R0, R1, I1, .., R(N/2)]`. This allocates
    /// nothing.
    pub fn run(&mut self, pack: &[f32], out: &mut [f32]) {
        let size = self.size;
        let half = size / 2;
        let bin = |k: usize| -> (f32, f32) {
            if k == 0 {
                (pack[0], 0.0)
            } else if k == half {
                (pack[size - 1], 0.0)
            } else {
                (pack[2 * k - 1], pack[2 * k])
            }
        };
        // Z(k) = (X(k) + X*(M-k)) + i W^k (X(k) - X*(M-k)), M = N/2.
        for k in 0..half {
            let (ar, ai) = bin(k);
            let (br, bi) = bin(half - k);
            let (sr, si) = (ar + br, ai - bi);
            let (dr, di) = (ar - br, ai + bi);
            let (c, s) = (self.split_cos[k], self.split_sin[k]);
            // i W^k d = i (c + i s)(dr + i di) = -(c di + s dr) + i (c dr - s di)
            self.re[k] = sr - (c * di + s * dr);
            self.im[k] = si + (c * dr - s * di);
        }
        let (re, im) = (&mut self.re, &mut self.im);
        for (a, b) in &self.swaps {
            re.swap(*a as usize, *b as usize);
            im.swap(*a as usize, *b as usize);
        }
        let mut span = 1;
        while span < half {
            let stride = half / (2 * span);
            let mut start = 0;
            while start < half {
                for k in 0..span {
                    let (low, high) = (start + k, start + k + span);
                    let (c, s) = (self.cos[k * stride], self.sin[k * stride]);
                    let tr = re[high] * c - im[high] * s;
                    let ti = re[high] * s + im[high] * c;
                    re[high] = re[low] - tr;
                    im[high] = im[low] - ti;
                    re[low] += tr;
                    im[low] += ti;
                }
                start += 2 * span;
            }
            span *= 2;
        }
        for m in 0..half {
            out[2 * m] = re[m];
            out[2 * m + 1] = im[m];
        }
    }
}

/// FL's 6-point, 5th-order interpolator over a stereo pair
/// (`FUN_00611420`, coefficients `0x8d5ce8..0x8d5d2c`): `x` is the
/// fraction less 0.5 and `p` the frames -2 to 3 around the read point.
#[inline]
pub fn interpolate(p: [f32; 6], x: f32) -> f32 {
    const C: [f32; 18] = [
        -0.041_666_668,
        0.208_333_33,
        -0.416_666_66,
        0.020_833_334,
        -0.0625,
        0.041_666_668,
        0.104_166_664,
        -0.6875,
        1.541_666_6,
        -0.052_083_332,
        0.406_25,
        -0.354_166_66,
        -0.023_437_5,
        0.158_854_17,
        -1.359_375,
        0.011_718_75,
        -0.097_656_25,
        0.585_937_5,
    ];
    let [m2, m1, z0, p1, p2, p3] = p;
    let (s1, d1) = (p3 + m2, m2 - p3);
    let (s2, d2) = (p2 + m1, m1 - p2);
    let (s3, d3) = (p1 + z0, z0 - p1);
    let e = d3 * C[2] + d2 * C[1] + d1 * C[0];
    let d = s3 * C[5] + s2 * C[4] + s1 * C[3] + e * x;
    let c = d3 * C[8] + d2 * C[7] + d1 * C[6] + d * x;
    let b = s3 * C[11] + s2 * C[10] + s1 * C[9] + c * x;
    let a = d3 * C[14] + d2 * C[13] + d1 * C[12] + b * x;
    s3 * C[17] + s2 * C[16] + s1 * C[15] + a * x
}

/// What every voice of a state shares (the object at plugin+0x230,
/// `FUN_00844bf0`, `FUN_00844d60`): the sizes, the random phases, the
/// four timbre curves and the level they are normalised to.
#[derive(Debug, Clone, PartialEq)]
pub struct Engine {
    /// The richness+5 wheel (+8) and the oversampling bits (+0x14).
    pub order: u32,
    pub bits: u32,
    /// Partials (+0x10): `2^(order - 1) - 1`.
    pub partials: usize,
    /// The table's length (+0xc): `2^(order + bits)`.
    pub size: usize,
    pub rate: f64,
    /// The table step at C5 (+0x7c).
    pub base_step: f64,
    /// The random phase of every partial, in Pack order, scaled by
    /// `1 / sqrt(partials)` (+0x2c, `FUN_00844f90`).
    phases: Vec<f32>,
    /// The forced harmonics (stored as `1 - f`), impulse, decay and
    /// release curves (+0x34 + 8 c), each as long as FL keeps it.
    pub curves: [Option<Vec<f32>>; 4],
    /// The first partial and one past the last that the impulse curve
    /// sounds (+0x24, +0x28).
    pub first: usize,
    pub last: usize,
    /// The RMS the start levels are put at (+0x88).
    pub level: f32,
    /// "To target" decay (+0x20) and normalising to the curves (+0x21).
    pub to_target: bool,
    pub normalise: bool,
    /// A voice is silent under this (+0x84, -80 dB) for `hold` samples
    /// (+0x8c, 100 ms) before it ends.
    pub silence: f32,
    pub hold: i32,
    pub ifft: RealIfft,
}

/// The curves by FL's index: -2 forced harmonics, -1 impulse, 0 decay, 1
/// release.
pub const FORCED: usize = 0;
pub const IMPULSE: usize = 1;
pub const DECAY: usize = 2;
pub const RELEASE: usize = 3;

impl Engine {
    /// An engine of `order` (the richness wheel + 5) and `bits` of
    /// oversampling at this rate. This allocates.
    pub fn new(order: u32, bits: u32, rate: f64) -> Self {
        let size = 1usize << (order + bits);
        let partials = (1usize << (order - 1)) - 1;
        let mut engine = Engine {
            order,
            bits,
            partials,
            size,
            rate,
            base_step: 0.0,
            phases: vec![0.0; size],
            curves: [None, None, None, None],
            first: partials,
            last: partials,
            level: 1.0,
            to_target: false,
            normalise: true,
            silence: 1e-4,
            hold: (0.1 * rate).round_ties_even() as i32,
            ifft: RealIfft::new(size),
        };
        engine.set_rate(rate);
        let mut random = vec![0.0; partials];
        let mut seed = 3;
        rand_uniform(&mut random, 0.0, std::f32::consts::TAU, &mut seed);
        let scale = 1.0 / (partials as f64).sqrt();
        for (i, r) in random.iter().enumerate() {
            let (s, c) = f64::from(*r).sin_cos();
            engine.phases[2 * i + 1] = (scale * c) as f32;
            engine.phases[2 * i + 2] = (scale * s) as f32;
        }
        flush(&mut engine.phases);
        engine
    }

    /// `FUN_00844e40`: the step at C5 and the silence hold at a rate.
    pub fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
        let scaled = TABLE_HZ * (1u32 << self.bits) as f32;
        self.base_step = f64::from(scaled) / rate;
        self.hold = (0.1 * rate).round_ties_even() as i32;
    }

    /// Store curve `which` (`FUN_00845210`), as long as it is given less
    /// what repeats its last value past the partials. The impulse curve,
    /// or with "to target" decay the decay curve, says which partials
    /// sound. This allocates.
    pub fn set_curve(&mut self, which: usize, values: Option<&[f32]>) {
        let Some(values) = values else {
            self.curves[which] = None;
            self.renormalise();
            return;
        };
        let mut len = values.len();
        while self.partials < len && len >= 2 && values[len - 1] == values[len - 2] {
            len -= 1;
        }
        let mut kept = values[..len].to_vec();
        if which == FORCED {
            for v in &mut kept {
                *v = 1.0 - *v;
            }
        }
        self.curves[which] = Some(kept);
        if which == IMPULSE || (which == DECAY && self.to_target) {
            self.first = self.partials;
            self.last = self.partials;
            let impulse = self.curves[IMPULSE].as_deref();
            let decay = self.curves[DECAY].as_deref();
            let both = match (self.to_target, impulse, decay) {
                (true, Some(i), Some(d)) => Some((i, d)),
                _ => None,
            };
            let own = self.curves[which].as_deref().unwrap_or(&[]);
            for i in 0..self.partials {
                let sounds = match both {
                    Some((i_curve, d_curve)) => {
                        let a = i_curve.get(i).copied().unwrap_or(0.0);
                        let b = d_curve.get(i).copied().unwrap_or(0.0);
                        f64::from(a - b).abs() > f64::from(TINY)
                    }
                    None => own.get(i).copied().unwrap_or(0.0) > TINY,
                };
                if sounds {
                    self.first = self.first.min(i);
                    self.last = i + 1;
                }
            }
        }
        self.renormalise();
    }

    /// `FUN_00845170`: the level is 1, or with normalising on 0.5 over the
    /// RMS of the impulse curve (and of the decay curve with "to target"
    /// decay, the larger).
    fn renormalise(&mut self) {
        self.level = 1.0;
        if !self.normalise {
            return;
        }
        let rms = |c: &Option<Vec<f32>>| -> f64 {
            let Some(c) = c else { return 0.0 };
            let sum: f32 = c.iter().map(|v| v * v).sum();
            f64::from((sum / self.partials as f32).sqrt())
        };
        let mut most = rms(&self.curves[IMPULSE]);
        if self.to_target {
            most = most.max(rms(&self.curves[DECAY]));
        }
        if f64::from(TINY) < most {
            self.level = (0.5 / most) as f32;
        }
    }
}

/// One unison copy of a voice (the 0x3c-byte records at voice+0xb8).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Copy {
    /// Gains on the newer and the older table, left and right, and what a
    /// frame adds to each (+0..+0x1c).
    gains: [f32; 4],
    steps: [f32; 4],
    /// Where it reads, in table frames (+0x20), and its pitch against the
    /// voice (+0x28).
    pub phase: f64,
    pub ratio: f64,
    /// Its pan gains (+0x30, +0x34) and its detune share (+0x38).
    pub pan: [f32; 2],
    pub detune: f32,
}

/// What one voice's timbre is made of (`FUN_00845790`): the arguments
/// TriggerVoice hands over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setup {
    /// Frame length in samples.
    pub hop: i32,
    /// Pre-decay (0 to 1 and more), decay in samples (below 0 for none).
    pub predecay: f64,
    pub decay: f64,
    /// Fullness, -1 to 1.
    pub fullness: f64,
    pub seed1: i32,
    pub seed2: i32,
    pub morph: f32,
    /// The pitch ratio against C5.
    pub ratio: f64,
    pub copies: usize,
    /// EQ shift (-0.5 to 0.5) and rotation, both in partials' share.
    pub shift: f32,
    pub rotation: f32,
    /// Read the tables with FL's linear interpolator (`FUN_00610ef0`, the
    /// quality "low" picks it) and not the 6-point one.
    pub linear: bool,
}

/// Scratch every voice uses in turn.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Work {
    /// Four rows of the partials (`FUN_00845f30`'s local buffer).
    rows: Vec<f32>,
    /// The newest table out of the inverse FFT.
    table: Vec<f32>,
    /// One chunk of (newer, older) pairs out of the interpolator.
    pairs: Vec<f32>,
    random: Vec<f32>,
}

/// The most frames a voice renders at once.
pub const CHUNK: usize = 256;

impl Work {
    /// Scratch for an engine. This allocates.
    pub fn new(engine: &Engine) -> Self {
        let n = engine.partials;
        Work {
            rows: vec![0.0; 4 * n],
            table: vec![0.0; engine.size],
            pairs: vec![0.0; 2 * CHUNK],
            random: vec![0.0; n],
        }
    }
}

/// The voice's part of the engine (voice+0xe50).
#[derive(Debug, Clone, PartialEq)]
pub struct Timbre {
    /// The spectrum (+0x70) and what each frame multiplies it by, or with
    /// "to target" decay the spectrum it goes to (+0x78), both in Pack
    /// order; the start levels (+0x80).
    spectrum: Vec<f32>,
    multiplier: Vec<f32>,
    levels: Vec<f32>,
    /// The two newest tables as (newer, older) pairs, with 64 frames of
    /// the start again after the end (+0x98).
    frames: Vec<f32>,
    hop: i32,
    /// Samples played, the sample the next frame starts at, and samples
    /// into this frame (+0x14, +0x18, +0x1c).
    at: i64,
    next: i64,
    into: i32,
    /// The Pack slots the decay runs over (+0x20, +0x24) and the band
    /// limit in partials (+0x28).
    from: usize,
    to: usize,
    top: usize,
    shift: i32,
    rotation: i32,
    seed1: i32,
    seed2: i32,
    morph: f32,
    fullness: f32,
    /// 0 while the spectrum decays, 1 when it has no decay, and -1 when it
    /// has none and the tables are made no more (+0x48).
    endless: i32,
    to_target: bool,
    approach: f64,
    /// Samples left before a silent voice ends (+0x44).
    pub quiet: i32,
    step: f64,
    predecay: f64,
    decay: f64,
    fresh: bool,
    linear: bool,
    pub copies: [Copy; 9],
    pub count: usize,
}

impl Timbre {
    /// A voice's buffers for an engine. This allocates.
    pub fn new(engine: &Engine) -> Self {
        let size = engine.size;
        Timbre {
            spectrum: vec![0.0; size],
            multiplier: vec![0.0; size],
            levels: vec![0.0; (engine.partials + 3) & !3],
            frames: vec![0.0; 2 * (size + GUARD)],
            hop: 1,
            at: 0,
            next: 1,
            into: 0,
            from: 0,
            to: 0,
            top: 0,
            shift: 0,
            rotation: 0,
            seed1: -1,
            seed2: -1,
            morph: 0.0,
            fullness: 0.0,
            endless: 0,
            to_target: false,
            approach: 0.0,
            quiet: 0,
            step: 0.0,
            predecay: 0.0,
            decay: 0.0,
            fresh: true,
            linear: false,
            copies: [Copy::default(); 9],
            count: 1,
        }
    }

    /// Whether these buffers fit the engine.
    pub fn fits(&self, engine: &Engine) -> bool {
        self.spectrum.len() == engine.size
            && self.levels.len() == (engine.partials + 3) & !3
            && self.frames.len() == 2 * (engine.size + GUARD)
    }

    /// `FUN_00845790`: set a voice up. The tables are made on its first
    /// render.
    pub fn start(&mut self, engine: &Engine, s: &Setup) {
        let n = engine.partials as f64;
        self.hop = s.hop.max(1);
        self.at = 0;
        self.next = i64::from(self.hop);
        self.into = 0;
        self.seed1 = s.seed1;
        self.seed2 = s.seed2;
        self.morph = s.morph;
        let mag = (s.fullness as f32).abs();
        let sign = if (s.fullness as f32) > 0.0 {
            1.0
        } else if (s.fullness as f32) < 0.0 {
            -1.0
        } else {
            0.0
        };
        let grown = (f64::from(mag) * 10_000_001f64.ln()).exp();
        self.fullness = ((f64::from((grown - 1.0) as f32)) * 0.01 * -sign) as f32;
        self.quiet = engine.hold;
        self.to_target = engine.to_target;
        self.shift = (f64::from(s.shift) * n).round_ties_even() as i32;
        let turned = (f64::from(s.rotation) * n + n * 256.0).round_ties_even() as i64;
        self.rotation = (turned % engine.partials.max(1) as i64) as i32;
        self.count = s.copies.clamp(1, 9);
        for copy in &mut self.copies {
            *copy = Copy {
                ratio: 1.0,
                ..Copy::default()
            };
        }
        self.set_pitch(engine, s.ratio);
        self.predecay = s.predecay;
        self.decay = s.decay;
        self.fresh = true;
        self.linear = s.linear;
    }

    /// `FUN_00846f10`: the step at this pitch ratio.
    pub fn set_pitch(&mut self, engine: &Engine, ratio: f64) {
        self.step = ratio * engine.base_step;
    }

    /// `FUN_00846f30`: a unison copy's pitch, start phase (as a share of
    /// the table, only before the first render) and pan gains.
    pub fn set_copy(&mut self, i: usize, ratio: f64, phase: f64, left: f32, right: f32) {
        let copy = &mut self.copies[i];
        copy.pan = [left, right];
        copy.ratio = ratio;
        if copy.phase == 0.0 {
            copy.phase = phase;
        }
    }

    /// `FUN_00845f30`: the start levels, the spectrum and what each frame
    /// does to it (`release` false), or what the release does from now on
    /// (`release` true). `p3` is the pre-decay and `p4` the decay or
    /// release in samples.
    fn levels(&mut self, engine: &Engine, work: &mut Work, release: bool, p3: f64, p4: f64) {
        let n = engine.partials;
        let size = engine.size;
        let hop = f64::from(self.hop);
        let floor = engine.rate * 0.025 / hop;
        let over = p4 / hop - floor;
        let frames = if over <= 0.0 { 0.0 } else { over };
        self.endless = i32::from(p4 < 0.0);
        if self.to_target && self.endless == 0 {
            self.approach = 1.0 - (LN_HUNDREDTH / frames).exp();
        }
        let curve = |c: usize| engine.curves[c].as_deref().unwrap_or(&[]);
        let (rows, rest) = work.rows.split_at_mut(n);
        let (t2, rest) = rest.split_at_mut(n);
        let (t3, t4) = rest.split_at_mut(n);
        let t1 = rows;

        if !release {
            let inverse = 1.0 / p3.max(0.001);
            let norm = if self.fullness == 0.0 {
                1.0
            } else {
                (1.0 / f64::from(self.fullness.abs())) as f32
            };
            let mut blank = self.morph == 0.0;
            if !blank {
                random(self.rotation, t1, t3, self.seed2);
                shape(t1, &mut self.levels[..n], self.fullness);
                let gain = self.morph * norm;
                for v in &mut self.levels[..n] {
                    *v *= gain;
                }
            }
            if self.morph < 1.0 {
                let rest = 1.0 - self.morph;
                if self.seed1 < 0 {
                    if blank {
                        self.levels[..n].fill(rest);
                    } else {
                        for v in &mut self.levels[..n] {
                            *v += rest;
                        }
                    }
                } else {
                    random(self.rotation, t1, t3, self.seed1);
                    let gain = rest * norm;
                    if blank {
                        shape(t1, &mut self.levels[..n], self.fullness);
                        for v in &mut self.levels[..n] {
                            *v *= gain;
                        }
                    } else {
                        shape(t1, t2, self.fullness);
                        for (v, t) in self.levels[..n].iter_mut().zip(t2.iter()) {
                            *v += t * gain;
                        }
                    }
                }
                blank = false;
            }
            if !blank {
                // ippsThreshold_LT_32s_I on the bits: a sign bit is 0.
                for v in &mut self.levels[..n] {
                    if v.is_sign_negative() {
                        *v = 0.0;
                    }
                }
            }
            if let Some(forced) = engine.curves[FORCED].as_deref() {
                for (i, v) in self.levels[..n].iter_mut().enumerate() {
                    let f = forced.get(i).copied().unwrap_or(1.0);
                    *v = *v * f + 1.0 - f;
                }
            }
            let inverse = inverse as f32;
            for (t, v) in t2.iter_mut().zip(&self.levels[..n]) {
                *t = v * inverse + 0.001;
            }
            let ln = LN_HUNDREDTH as f32;
            if self.to_target || self.endless != 0 {
                if !self.to_target {
                    t3.fill(1.0);
                }
                for (t, out) in t2.iter().zip(t4.iter_mut()) {
                    *out = (ln / t).exp();
                }
            } else {
                mul_shift(&self.levels[..n], curve(DECAY), t1, self.shift);
                let (frames, floor) = (frames as f32, floor as f32);
                for t in t1.iter_mut() {
                    *t = *t * frames + floor;
                }
                for (t, out) in t1.iter().zip(t3.iter_mut()) {
                    *out = (ln / t).exp();
                }
                for (t, out) in t2.iter().zip(t4.iter_mut()) {
                    *out = (ln / t).exp();
                }
            }
            let sum: f32 = t4.iter().map(|v| v * v).sum();
            let rms = ((sum / n as f32) as f64).sqrt() as f32;
            if rms > 0.0 {
                let gain = engine.level / rms;
                for v in t4.iter_mut() {
                    *v *= gain;
                }
            }
            let reach = (n << engine.bits) as f64 * (40_000.0 / 44_100.0) / self.step;
            self.top = if reach >= 1.0 {
                (reach as usize).min(n)
            } else {
                0
            };
            let (from, to) = if self.shift == 0 {
                (engine.first.min(n), self.top.min(engine.last))
            } else {
                (0, self.top)
            };
            let (lo, hi) = if self.to_target {
                (0, self.top)
            } else {
                (from, to)
            };
            let spectrum = &mut self.spectrum;
            spectrum[..(2 * lo + 1).min(size)].fill(0.0);
            if 2 * hi + 1 < size {
                spectrum[2 * hi + 1..].fill(0.0);
            }
            mul_shift(t4, curve(IMPULSE), t1, self.shift);
            for i in lo..hi {
                let level = t1[i];
                spectrum[2 * i + 1] = engine.phases[2 * i + 1] * level;
                spectrum[2 * i + 2] = engine.phases[2 * i + 2] * level;
            }
            if self.to_target {
                mul_shift(t4, curve(DECAY), t1, self.shift);
                for i in from..to {
                    let level = t1[i];
                    self.multiplier[2 * i + 1] = engine.phases[2 * i + 1] * level;
                    self.multiplier[2 * i + 2] = engine.phases[2 * i + 2] * level;
                }
            }
            self.from = 2 * from + 1;
            self.to = 2 * to + 1;
            if self.from < self.to {
                flush(&mut self.spectrum[self.from..self.to]);
                if self.to_target {
                    flush(&mut self.multiplier[self.from..self.to]);
                }
            }
        } else {
            mul_shift(&self.levels[..n], curve(RELEASE), t1, self.shift);
            let (frames, floor) = (frames as f32, floor as f32);
            let ln = LN_HUNDREDTH as f32;
            for (t, out) in t1.iter_mut().zip(t3.iter_mut()) {
                *t = *t * frames + floor;
                *out = (ln / *t).exp();
            }
            if self.to_target {
                self.to_target = false;
                self.from = 0;
                self.to = 2 * self.top + 1;
            }
        }
        if !self.to_target {
            flush(t3);
            self.multiplier[0] = 0.0;
            self.multiplier[size - 1] = 0.0;
            for (i, m) in t3.iter().enumerate() {
                self.multiplier[2 * i + 1] = *m;
                self.multiplier[2 * i + 2] = *m;
            }
        }
    }

    /// `FUN_00845bb0`: the newer table becomes the older, a new one comes
    /// out of the spectrum, and the spectrum takes a frame of decay.
    fn advance(&mut self, engine: &mut Engine, work: &mut Work) {
        if self.endless < 0 {
            return;
        }
        let size = engine.size;
        engine.ifft.run(&self.spectrum, &mut work.table);
        for (j, fresh) in work.table.iter().enumerate() {
            self.frames[2 * j + 1] = self.frames[2 * j];
            self.frames[2 * j] = *fresh;
        }
        let (body, guard) = self.frames.split_at_mut(2 * size);
        guard.copy_from_slice(&body[..2 * GUARD]);
        if self.endless != 0 || self.from >= self.to {
            return;
        }
        let (from, to) = (self.from, self.to.min(size));
        if !self.to_target {
            for (s, m) in self.spectrum[from..to]
                .iter_mut()
                .zip(&self.multiplier[from..to])
            {
                *s *= m;
            }
            flush(&mut self.spectrum[from..to]);
        } else {
            let k = self.approach as f32;
            for (s, m) in self.spectrum[from..to]
                .iter_mut()
                .zip(&self.multiplier[from..to])
            {
                *s += (m - *s) * k;
            }
        }
    }

    /// `FUN_00846f90`: a new frame crossfades from the older table to the
    /// newer one.
    fn crossfade(&mut self) {
        let per = (1.0 / f64::from(self.hop)) as f32;
        for copy in &mut self.copies[..self.count] {
            let [l, r] = copy.pan;
            copy.gains = [0.0, 0.0, l, r];
            let (sl, sr) = (l * per, r * per);
            copy.steps = [sl, sr, -sl, -sr];
        }
    }

    /// `FUN_00845ad0`: the first two tables, and each copy started where
    /// the newest table is nearest 0.
    fn begin(&mut self, engine: &mut Engine, work: &mut Work) {
        self.fresh = false;
        let (p3, p4) = (self.predecay, self.decay);
        self.levels(engine, work, false, p3, p4);
        self.advance(engine, work);
        self.advance(engine, work);
        let size = engine.size;
        let mut quietest = 0;
        let mut least = 1000.0f32;
        for j in 0..size {
            let v = self.frames[2 * j];
            if v * v < least {
                least = v * v;
                quietest = j;
            }
        }
        let offset = (quietest + 2 * size) as f64 - HALF_GUARD as f64;
        for copy in &mut self.copies[..self.count] {
            copy.phase = copy.phase * size as f64 + offset;
        }
        self.crossfade();
        if self.endless > 0 {
            self.endless = -1;
        }
    }

    /// `FUN_00847080`: the release. The decay from now on is the
    /// release's, and a frame under way starts again from what plays.
    pub fn release(&mut self, engine: &mut Engine, work: &mut Work, samples: f64) {
        if self.fresh {
            self.begin(engine, work);
        }
        self.levels(engine, work, true, 0.0, samples);
        if self.at != 0 {
            let f = self.into as f32 / self.hop as f32;
            for pair in self.frames[..2 * engine.size].as_chunks_mut::<2>().0 {
                pair[0] = pair[0] * f + pair[1] * (1.0 - f);
            }
            self.advance(engine, work);
            self.next = self.at + i64::from(self.hop);
            self.crossfade();
        }
    }

    /// `FUN_008471f0`: write `out.len() / 2` frames of the voice into the
    /// interleaved `out`, and count down to its end while it is silent.
    pub fn render(&mut self, engine: &mut Engine, work: &mut Work, out: &mut [f32]) {
        let total = out.len() / 2;
        let size = engine.size;
        let mask = size as i64 - 1;
        let mut done = 0;
        while done < total {
            let len = (total - done)
                .min((self.next - self.at).max(1) as usize)
                .min(CHUNK);
            self.into += len as i32;
            if self.fresh {
                self.begin(engine, work);
            }
            for c in 0..self.count {
                let copy = self.copies[c];
                let step = self.step * copy.ratio;
                for j in 0..len {
                    let at = copy.phase + step * j as f64;
                    let whole = at as i64;
                    let frac = (at - whole as f64) as f32;
                    let frame = (whole & mask) as usize + HALF_GUARD;
                    let f = &self.frames;
                    if self.linear {
                        let (a, b) = (2 * frame, 2 * frame + 2);
                        work.pairs[2 * j] = (f[b] - f[a]) * frac + f[a];
                        work.pairs[2 * j + 1] = (f[b + 1] - f[a + 1]) * frac + f[a + 1];
                        continue;
                    }
                    let pick = |k: usize, side: usize| f[2 * (frame + k - 2) + side];
                    let newer = [0, 1, 2, 3, 4, 5].map(|k| pick(k, 0));
                    let older = [0, 1, 2, 3, 4, 5].map(|k| pick(k, 1));
                    work.pairs[2 * j] = interpolate(newer, frac - 0.5);
                    work.pairs[2 * j + 1] = interpolate(older, frac - 0.5);
                }
                let copy = &mut self.copies[c];
                copy.phase += step * len as f64;
                let mut g = copy.gains;
                let s = copy.steps;
                let dst = &mut out[2 * done..2 * (done + len)];
                for j in 0..len {
                    let (n, o) = (work.pairs[2 * j], work.pairs[2 * j + 1]);
                    let l = n * g[0] + o * g[2];
                    let r = n * g[1] + o * g[3];
                    if c == 0 {
                        dst[2 * j] = l;
                        dst[2 * j + 1] = r;
                    } else {
                        dst[2 * j] += l;
                        dst[2 * j + 1] += r;
                    }
                    for k in 0..4 {
                        g[k] += s[k];
                    }
                }
                copy.gains = g;
            }
            if self.count == 0 {
                out[2 * done..2 * (done + len)].fill(0.0);
            }
            self.at += len as i64;
            done += len;
            if self.at >= self.next {
                self.advance(engine, work);
                self.next += i64::from(self.hop);
                self.into = 0;
                self.crossfade();
            }
        }
        let loud = out.iter().any(|v| v.abs() > engine.silence);
        if loud {
            self.quiet = engine.hold;
        } else {
            self.quiet -= total as i32;
        }
    }
}

/// `FUN_00845e40`: uniform numbers in `[0, 1)` from a seed into `dst`,
/// turned `rotation` places up (number `i` lands at `i + rotation`, the
/// last ones wrapping to the front) through `spare` when there is a
/// rotation.
fn random(rotation: i32, dst: &mut [f32], spare: &mut [f32], seed: i32) {
    let mut seed = seed;
    let n = dst.len();
    if rotation < 1 {
        rand_uniform(dst, 0.0, 1.0, &mut seed);
        return;
    }
    let spare = &mut spare[..n];
    rand_uniform(spare, 0.0, 1.0, &mut seed);
    let r = (rotation as usize).min(n);
    dst[r..].copy_from_slice(&spare[..n - r]);
    dst[..r].copy_from_slice(&spare[n - r..]);
}

/// `FUN_00845d20`: uniform numbers `x` shaped by the fullness `p`: as
/// they are at 0, `(p + 1)^x - 1` above it and `p' - p'^(1 - x)` with
/// `p' = 1 - p` below it.
fn shape(src: &[f32], dst: &mut [f32], p: f32) {
    if p == 0.0 {
        dst.copy_from_slice(&src[..dst.len()]);
    } else if p < 0.0 {
        let q = 1.0 - p;
        let ln = f64::from(q).ln() as f32;
        for (d, s) in dst.iter_mut().zip(src.iter()) {
            *d = q - ((1.0 - s) * ln).exp();
        }
    } else {
        let ln = (f64::from(p) + 1.0).ln() as f32;
        for (d, s) in dst.iter_mut().zip(src.iter()) {
            *d = (s * ln).exp() - 1.0;
        }
    }
}

/// `dst = src * curve`, the curve moved `shift` partials up (down when
/// below 0) and held at its last value past its end, as FL's inline
/// helper in `FUN_00845f30` does it.
fn mul_shift(src: &[f32], curve: &[f32], dst: &mut [f32], shift: i32) {
    let n = src.len();
    if curve.is_empty() {
        dst[..n].fill(0.0);
        return;
    }
    if shift == 0 && n <= curve.len() {
        for i in 0..n {
            dst[i] = src[i] * curve[i];
        }
        return;
    }
    let (mut start, mut skip, mut len) = (0usize, 0usize, n as i64);
    let mut clen = curve.len() as i64;
    if shift < 1 {
        skip = (-shift) as usize;
        clen += i64::from(shift);
    } else {
        let s = (shift as usize).min(n);
        for i in 0..s {
            dst[i] = src[i] * curve[0];
        }
        start = s;
        len -= s as i64;
    }
    let curve = &curve[skip.min(curve.len())..];
    if clen < len {
        let held = if clen >= 1 {
            curve[(clen - 1) as usize]
        } else {
            0.0
        };
        let from = clen.max(0) as usize;
        for i in from..len as usize {
            dst[start + i] = src[start + i] * held;
        }
        len = clen.max(0);
    }
    for i in 0..len.max(0) as usize {
        dst[start + i] = src[start + i] * curve[i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_uniform_in_its_range_and_hands_its_state_on() {
        let mut out = vec![0.0f32; 10_001];
        let mut seed = 1201;
        rand_uniform(&mut out, 0.0, 1.0, &mut seed);
        assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
        let mean = out.iter().map(|v| f64::from(*v)).sum::<f64>() / out.len() as f64;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
        // Another seed is another sequence; the same seed the same one.
        let mut again = vec![0.0f32; 16];
        let mut other = vec![0.0f32; 16];
        let (mut a, mut b) = (1201, 1202);
        rand_uniform(&mut again, 0.0, 1.0, &mut a);
        rand_uniform(&mut other, 0.0, 1.0, &mut b);
        assert_eq!(again, out[..16]);
        assert_ne!(again, other);
        assert_ne!(seed, 1201);
    }

    #[test]
    fn the_seed_mix_is_positive_and_spreads() {
        let seeds: Vec<i32> = (0..100).map(|k| mix(k - 0x20cf, 0x1cfc)).collect();
        assert!(seeds.iter().all(|s| *s >= 0));
        let mut sorted = seeds.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), seeds.len());
    }

    #[test]
    fn the_inverse_fft_is_the_sum_of_its_partials() {
        let size = 64;
        let mut fft = RealIfft::new(size);
        let mut pack = vec![0.0f32; size];
        // Partial 3 at (0.5, -0.25), partial 10 at (0, 1), and a DC.
        pack[0] = 0.125;
        pack[2 * 3 - 1] = 0.5;
        pack[2 * 3] = -0.25;
        pack[2 * 10] = 1.0;
        let mut out = vec![0.0f32; size];
        fft.run(&pack, &mut out);
        for (n, v) in out.iter().enumerate() {
            let t = |k: f64| TAU * k * n as f64 / size as f64;
            let want =
                0.125 + 2.0 * (0.5 * t(3.0).cos() + 0.25 * t(3.0).sin()) - 2.0 * t(10.0).sin();
            assert!((f64::from(*v) - want).abs() < 1e-5, "{n}: {v} {want}");
        }
    }

    #[test]
    fn the_interpolator_passes_through_its_points_and_a_line() {
        let p = [0.3, -1.0, 0.25, 0.75, 2.0, -0.5];
        assert!((interpolate(p, -0.5) - 0.25).abs() < 1e-6);
        assert!((interpolate(p, 0.5) - 0.75).abs() < 1e-6);
        let line = [-2.0, -1.0, 0.0, 1.0, 2.0, 3.0];
        for k in 0..10 {
            let frac = k as f32 / 10.0;
            assert!((interpolate(line, frac - 0.5) - frac).abs() < 1e-5);
        }
    }

    #[test]
    fn a_curve_shifted_up_holds_its_first_value_and_its_last_past_its_end() {
        let src = [1.0; 6];
        let curve = [1.0, 2.0, 3.0, 4.0];
        let mut dst = [0.0; 6];
        mul_shift(&src, &curve, &mut dst, 2);
        assert_eq!(dst, [1.0, 1.0, 1.0, 2.0, 3.0, 4.0]);
        mul_shift(&src, &curve, &mut dst, -1);
        assert_eq!(dst, [2.0, 3.0, 4.0, 4.0, 4.0, 4.0]);
        mul_shift(&src, &curve, &mut dst, 0);
        assert_eq!(dst, [1.0, 2.0, 3.0, 4.0, 4.0, 4.0]);
    }

    #[test]
    fn the_fullness_curve_meets_its_ends() {
        let src = [0.0, 1.0, 0.5];
        let mut dst = [0.0; 3];
        shape(&src, &mut dst, 575.0);
        assert!(dst[0].abs() < 1e-6 && (dst[1] - 575.0).abs() < 0.1);
        let src = [0.0, 1.0, 0.5];
        shape(&src, &mut dst, -3.0);
        assert!(dst[0].abs() < 1e-6 && (dst[1] - 3.0).abs() < 1e-5);
    }
}
