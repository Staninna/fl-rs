//! Low Lifter: what is under a crossover is shaped into its own second and
//! third harmonics, diffused and mixed back over a phase-matched dry
//! signal, as `docs/plugins/low-lifter.md` has it.
//!
//! The state is 82 bytes: a little-endian i32 version, 6 (FL reads no
//! other), the fifteen parameters as i32, and 18 bytes of the editor's
//! (spectrum settings) that nothing here reads.
//!
//! | parameter | control | processor zone |
//! |---|---|---|
//! | 0 Enabled | 0 or 1 | On/Off |
//! | 1 Dry/Wet | 0 to 10000 | `n` |
//! | 2 Low band gain | 0 to 10000 | -120 + 132 `n^(1/7.275)` dB |
//! | 3 Harmonics decay slope | 0 to 10000 | -60 + 60 `n^(1/3.105)` dB |
//! | 4 Lookahead | 0 or 1 | 10 ms |
//! | 5 Invert polarity | 0 or 1 | |
//! | 6 Pre-shaping boost | 0 to 10000 | 18 `n` dB |
//! | 7 Diffuser mix | 0 to 10000 | `n` |
//! | 8 Diffuser coefficient | 0 to 10000 | -0.99 + 1.98 `n` |
//! | 9 High band gain | 0 to 10000 | -120 + 125 `n^(1/5)` dB |
//! | 10 Crossover cutoff | 0 to 10000 | 20 + 100 `n^(1/0.498918)` Hz |
//! | 11 Low cut | 0 to 10000 | as the crossover |
//! | 12 Multiband | 0 or 1 | |
//! | 13 High band | 0 or 1 | |
//! | 14 Low band | 0 or 1 | |
//!
//! `n` is the wheel over its range. FL's plugin hands these to a processor
//! in `ildsp_x64.dll` that Faust made, `ILRDLowFreqEnhancer`. This is that
//! processor, operation for operation, per side:
//!
//! ```text
//! low, high      = a fourth-order crossover of x at the cutoff c
//! split          = x through allpasses at c/2 and c/4, then less its own
//!                  low, crossed again at c (the multiband path)
//! four bands     = the low part split at c/4 and c, times the pre gain
//! each band      = held behind the lookahead, over its peak envelope (at
//!                  least 1): (u + s T2(u) + s^2 T3(u)) / (1 + 2 s), less
//!                  a 20 Hz highpass, with s the slope as a gain
//! sum            = multiband ? bands 1 + 2 + 3 : band 4, faded in by a
//!                  one-pole of 1 Hz after a clear
//! diffused       = sum through four rotations of depth d with delays of
//!                  0.5, 0.207, 0.159 and 0.134 times a period of c
//! wet            = highpass(low gain * mix(sum, diffused) + high gain * high)
//!                  at the low cut, second order
//! out            = dry * (x through the same allpasses) + wet amount * wet
//! ```
//!
//! Every control goes through a one-pole of 20 Hz (the slope 5 Hz), so a
//! new state glides in. After one second of input under -120 dB the
//! plugin stops running its processor and passes the input, as FL's does,
//! until the input comes back.

use crate::{Effect, StateError, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Low Lifter";

/// How long the state is, and the one version FL reads.
const LEN: usize = 82;
const VERSION: i32 = 6;

/// The range of each parameter's control: the wheels from the DFM, the
/// others checkboxes.
const WHEEL: (i32, i32) = (0, 10_000);
const BOX: (i32, i32) = (0, 1);
const RANGES: [(i32, i32); 15] = [
    BOX, WHEEL, WHEEL, WHEEL, BOX, BOX, WHEEL, WHEEL, WHEEL, WHEEL, WHEEL, WHEEL, BOX, BOX, BOX,
];

/// A new Low Lifter, as the render-test project saves it: the processor's
/// own defaults turned back into wheels.
const DEFAULTS: [i32; 15] = [
    1, 10_000, 4999, 7210, 1, 0, 0, 0, 5000, 10_000, 7750, 0, 0, 1, 1,
];

/// The editor's part of a new plugin's state.
const EDITOR: [u8; 18] = [1, 0, 3, 0, 0, 0, 3, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0];

/// The curves of the wheels (`FUN_006c54c0`): `a + (b - a) * n^(1/c)`,
/// with the constants at `0x006c7418..0x006c7480`, by parameter.
const CURVES: [Option<(f64, f64, f64)>; 15] = [
    None,
    None,
    Some((-120.0, 12.0, 7.275)),
    Some((-60.0, 0.0, 3.105)),
    None,
    None,
    Some((0.0, 18.0, 1.0)),
    None,
    Some((-0.99, 0.99, 1.0)),
    Some((-120.0, 5.0, 5.0)),
    Some((20.0, 120.0, 0.498918)),
    Some((20.0, 120.0, 0.498918)),
    None,
    None,
    None,
];

/// The lines' lengths: the long one of the first rotation, and the rest.
const LONG: usize = 8192;
const LINE: usize = 2048;
const LONG_MASK: u32 = LONG as u32 - 1;
const MASK: u32 = LINE as u32 - 1;

/// The most frames the processor's wrapper takes in one call.
const CHUNK: usize = 0x4000;

/// Input at or under this is silence to the plugin's render.
const QUIET: f32 = 1e-6;

/// The constants of `compute`, as their bits in `ildsp_x64.dll`.
const SQRT2: f32 = std::f32::consts::SQRT_2;
const SQRT8: f32 = f32::from_bits(0x4035_04f3);
const DEPTH: f32 = f32::from_bits(0x3f7d_70a4);
const LONGEST: f32 = 48_000.0;
const LOWEST: f32 = 20.0;

/// The Delphi runtime's `Ln` (`0x00409870`) and `Exp` (`0x00408810`),
/// which the plugin's wheel curves go through, for the arguments those
/// give them: `Ln` of a wheel in (0, 1] and `Exp` of what that is over a
/// curve's power, from 0 down to -19. The wheel at 0 gives `Ln` minus
/// infinity (FL masks the error), and `Exp` 0.
mod delphi {
    /// `Ln`'s polynomial (`FUN_00409780`), at `0x006e2a30`.
    const LOG: [u64; 12] = [
        0x3fb0c835bd538f8c,
        0x3fa988ab1ef94f29,
        0x3fae3c098f53b5ee,
        0x3fb11053fca919d4,
        0x3fb3b140d1e4050f,
        0x3fb745d159460dd7,
        0x3fbc71c71cbb0290,
        0x3fc249249248f262,
        0x3c602e384367005f,
        0x3fc99999999999b4,
        0x3c750aaac32dac90,
        0x3fd5555555555555,
    ];

    /// `e^(j/64) - 1` and what it lost, for `j` from -22 to 22: the part
    /// of the table at `0x006e24e8` an argument under ln 2 / 2 reaches.
    const EXP: [u64; 90] = [
        0xbfd29e011a428ec6,
        0xbc6dabf5975c0c02,
        0xbfd1e70c28b987f3,
        0x3c74e91dbb1734bd,
        0xbfd12d35a41ba104,
        0x3c63445f7544e0ef,
        0xbfd07071eef11388,
        0xbc309aa682553231,
        0xbfcf616a79dda3a8,
        0xbc66b2eab63020c1,
        0xbfcddbe7247382af,
        0xbc431eb13933e894,
        0xbfcc5041854df7d4,
        0xbc6797d4686c5393,
        0xbfcabe60e1f21836,
        0xbc36f8b82e653e2d,
        0xbfc9262c1c3430a1,
        0xbc646ff6ec4a4251,
        0xbfc78789b0a5e0c0,
        0x3c5e3a6bdaece8f9,
        0xbfc5e25fb4fde211,
        0x3c064eec82915df3,
        0xbfc43693d679612d,
        0xbc69da94a869862a,
        0xbfc2840b5836cf67,
        0xbc685405051eb425,
        0xbfc0caab118a1278,
        0x3c26ad4c353465b0,
        0xbfbe14aed893eef4,
        0x3c4e1f58934f97af,
        0xbfba85e8c62d9c13,
        0xbc5adf7745e77188,
        0xbfb6e8caff341fea,
        0xbc59573ded7888b2,
        0xbfb33d1bb17df2e7,
        0xbc4e19c873b1d6a8,
        0xbfaf0540438fd5c3,
        0xbc2a1ce01f9f6ca7,
        0xbfa7723950130405,
        0x3c2c677ad8fa478d,
        0xbf9f8152aee9450e,
        0x3c24b00abf977627,
        0xbf8fc055004416db,
        0xbc282ef422ab152a,
        0x0000000000000000,
        0x0000000000000000,
        0x3f90202ad5778e46,
        0xbc151e6d305beec6,
        0x3fa040ac0224fd93,
        0x3c2c17a107575019,
        0x3fa89246d053d178,
        0x3c44967f31eb2595,
        0x3fb082b577d34ed8,
        0xbc45272ff30eed1b,
        0x3fb4cd4fc989cd64,
        0x3c5557a8671b89e7,
        0x3fb92937074e0cd7,
        0xbc5db0b9cc915fc5,
        0x3fbd96b0eff0e794,
        0xbc475385b2cdf93d,
        0x3fc10b022db7ae68,
        0xbc58c4a5df1ec7e5,
        0x3fc353bc9fb00b21,
        0x3c66bae618011342,
        0x3fc5a5ac59b963cb,
        0xbc6fd91307e74c50,
        0x3fc800f67b00d7b8,
        0x3c27ab912c69ffeb,
        0x3fca65c0b85ac1a9,
        0x3c6a9c189196f8cd,
        0x3fccd4315e9e0833,
        0xbc2172c31a1781f1,
        0x3fcf4c6f5508ee5d,
        0x3c646ef7b808180a,
        0x3fd0e7510fd7c564,
        0xbc71c5b2e8735a43,
        0x3fd22d78f0fa061a,
        0xbc789843c4964554,
        0x3fd378c3b0847980,
        0x3c73b5223eca1712,
        0x3fd4c946033eb3de,
        0xbc735d267d66dc96,
        0x3fd61f14f169ebc1,
        0xbc789e2d87fd0d92,
        0x3fd77a45d8117fd5,
        0xbc52bb36e6b3a2af,
        0x3fd8daee6a60c961,
        0x3c6a4e618fb92468,
        0x3fda4124b2fe50cb,
        0x3c6fb5f3020a46f5,
    ];

    /// `Exp`'s polynomial, at `0x006e2910`.
    const POLY: [u64; 4] = [
        0x3fc5555555554855,
        0x3fa5555555553b54,
        0x3f811112b12b282f,
        0x3f56c16e86e89028,
    ];

    fn c(bits: u64) -> f64 {
        f64::from_bits(bits)
    }

    fn kernel(x: f64) -> f64 {
        let p = LOG.map(c);
        let s = x / (x + 2.0);
        let h = x * 0.5;
        let x2 = x * h;
        let x3 = x2 * h;
        let z = s * s;
        let w = z * z;
        let a = ((((p[0] * w + p[2]) * w + p[4]) * w + p[6]) * w + p[8]) + p[9];
        let b = (((p[1] * w + p[3]) * w + p[5]) * w + p[7]) * w + p[10];
        let q = ((((b + a * z) + p[11]) * 2.0) * z) * s;
        (((q + x3) - x2) - s * x3) + x
    }

    pub(super) fn ln(x: f64) -> f64 {
        if x <= 0.0 {
            return f64::NEG_INFINITY;
        }
        let bits = x.to_bits();
        let mut e = ((bits >> 52) & 0x7ff) as i32 - 0x3ff;
        let m = bits & 0x000f_ffff_ffff_ffff;
        let f = if m <= 0x0006_a09e_667f_3bcc {
            c(0x3ff0_0000_0000_0000 + m)
        } else {
            e += 1;
            c(0x3fe0_0000_0000_0000 + m)
        };
        let k = kernel(f - 1.0);
        let e = f64::from(e);
        (k + e * c(0x3e37_d1cf_79ab_c9e4)) + e * c(0x3fe6_2e42_fc00_0000)
    }

    pub(super) fn exp(x: f64) -> f64 {
        if x == f64::NEG_INFINITY {
            return 0.0;
        }
        if x.to_bits() & 0x7ff0_0000_0000_0000 == 0 {
            return 1.0;
        }
        let t = x * c(0x3ff7_1547_652b_82fe);
        let n = if x < 0.0 { t - 0.5 } else { t + 0.5 } as i32;
        let scale = c(((i64::from(n) + 0x3ff) as u64) << 52);
        let nd = f64::from(n);
        let hi = x - nd * c(0x3fe6_2e42_fefa_4000);
        let lo = nd * c(0x3d48_432a_1b0e_2634);
        let r = hi + lo;
        let j = (r * 64.0 + 0.5f64.copysign(r)) as i32;
        let hi = hi - f64::from(j) * 0.015625;
        let u = hi + lo;
        let u2 = u * u;
        let p = POLY.map(c);
        let q = lo + u2 * (0.5 + u * ((p[0] + p[2] * u2) + (p[1] + p[3] * u2) * u));
        let at = (2 * (j + 22)) as usize;
        let (th, tl) = (c(EXP[at]), c(EXP[at + 1]));
        let s1 = th + 1.0;
        let cc = (th - (s1 - 1.0)) + tl;
        let big = s1 + cc;
        let small = cc - (big - s1);
        scale * (((((q * small + hi * small) + small) + q * big) + hi * big) + big)
    }
}

/// The value a parameter's processor zone takes from its wheel, as the
/// plugin works it out (`FUN_006c71d0`): the checkboxes as they are, the
/// mix wheels over their range, the rest through their curves. The wheel
/// goes over as an f32 and the curve is worked in f64.
pub fn zone(param: usize, value: i32) -> f32 {
    let (min, max) = RANGES[param];
    let n = (f64::from(value - min) * (1.0 / f64::from(max - min))).clamp(0.0, 1.0) as f32;
    match (param, CURVES[param]) {
        (_, Some((a, b, c))) => (a + (b - a) * delphi::exp(delphi::ln(f64::from(n)) / c)) as f32,
        (0 | 1 | 7, None) => n,
        _ => value as f32,
    }
}

/// The processor's zones (`buildUserInterface` @ `0x180177a80`), by the
/// wrapper's index.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Zones {
    on: f32,
    dry_wet: f32,
    low_db: f32,
    slope_db: f32,
    lookahead: f32,
    invert: f32,
    pre_db: f32,
    mix: f32,
    depth: f32,
    high_db: f32,
    cutoff: f32,
    low_cut: f32,
    multiband: f32,
    high_on: f32,
    low_on: f32,
}

impl Zones {
    fn new(params: &[i32; 15]) -> Self {
        let z: [f32; 15] = std::array::from_fn(|i| zone(i, params[i]));
        Zones {
            on: z[0],
            dry_wet: z[1],
            low_db: z[2],
            slope_db: z[3],
            lookahead: z[4],
            invert: z[5],
            pre_db: z[6],
            mix: z[7],
            depth: z[8],
            high_db: z[9],
            cutoff: z[10],
            low_cut: z[11],
            multiband: z[12],
            high_on: z[13],
            low_on: z[14],
        }
    }
}

/// What the processor works out from the rate (`instanceConstants` @
/// `0x180177050`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Consts {
    rate: f32,
    /// The 20 Hz smoother: `g / (1 + g)` and twice that.
    take: f32,
    take2: f32,
    /// Pi over the rate, and a quarter and a half of it.
    turn: f32,
    turn4: f32,
    turn2: f32,
    /// The 20 Hz highpass after each band: `g + k`, `g / den`, twice
    /// that, `2 g` and `1 / den`.
    dc_gk: f32,
    dc_g: f32,
    dc_g2: f32,
    dc_2g: f32,
    dc_d: f32,
    /// A quarter of the rate, for the peak holders.
    quarter: f32,
    /// The envelopes' fall, and what their rise is divided by.
    fall: f32,
    rise: f32,
    /// The slope's 5 Hz smoother, and the 1 Hz fade-in.
    slope_take: f32,
    slope_take2: f32,
    fade_take: f32,
    fade_take2: f32,
    /// The rotations' delays in periods of the cutoff, times the rate.
    delays: [f32; 4],
}

impl Consts {
    fn new(hz: u32) -> Self {
        let rate = hz as f32;
        let rate = if rate <= 1.0 { 1.0 } else { rate };
        let rate = if rate < 192_000.0 { rate } else { 192_000.0 };
        let over = 1.0 / rate;
        let g = (over * f32::from_bits(0x427b_53d1)).tan_f();
        let take = g / (g + 1.0);
        let dc_gk = g + SQRT2;
        let dc_d = 1.0 / (dc_gk * g + 1.0);
        let dc_g = dc_d * g;
        let slope = (over * f32::from_bits(0x417b_53d1)).tan_f();
        let slope_take = slope / (slope + 1.0);
        let turn = over * std::f32::consts::PI;
        let fade = turn.tan_f();
        let fade_take = fade / (fade + 1.0);
        Consts {
            rate,
            take,
            take2: take + take,
            turn,
            turn4: over * std::f32::consts::FRAC_PI_4,
            turn2: over * std::f32::consts::FRAC_PI_2,
            dc_gk,
            dc_g,
            dc_g2: dc_g + dc_g,
            dc_2g: g + g,
            dc_d,
            quarter: rate * 0.25,
            fall: (over * f32::from_bits(0xc310_72a7)).exp_f(),
            rise: over * f32::from_bits(0x4167_1dd9),
            slope_take,
            slope_take2: slope_take + slope_take,
            fade_take,
            fade_take2: fade_take + fade_take,
            delays: [
                rate * 0.5,
                rate * f32::from_bits(0x3e54_13cd),
                rate * f32::from_bits(0x3e22_bb90),
                rate * f32::from_bits(0x3e09_30a3),
            ],
        }
    }
}

/// The processor's maths: `tanf`, `expf` and `powf` of the MSVC runtime
/// linked into `ildsp_x64.dll`, which work in f64 and round once. Each
/// has an SSE2 path and an FMA3 path, and the runtime takes the second
/// on a CPU with AVX and FMA (`FUN_1801c3860` sets the switch at
/// `0x1803f84f4`), so this is that path, for the arguments the plugin
/// gives them. The two paths agree but for one argument in many
/// thousands; the SSE2 one, which the emulator runs, was matched to the
/// bit over 20000 arguments of each.
mod msvc {
    /// `2^(i/64)`, at `0x1803e3720`.
    const EXP2: [u64; 64] = [
        0x3ff0000000000000,
        0x3ff02c9a3e778061,
        0x3ff059b0d3158574,
        0x3ff0874518759bc8,
        0x3ff0b5586cf9890f,
        0x3ff0e3ec32d3d1a2,
        0x3ff11301d0125b51,
        0x3ff1429aaea92de0,
        0x3ff172b83c7d517b,
        0x3ff1a35beb6fcb75,
        0x3ff1d4873168b9aa,
        0x3ff2063b88628cd6,
        0x3ff2387a6e756238,
        0x3ff26b4565e27cdd,
        0x3ff29e9df51fdee1,
        0x3ff2d285a6e4030b,
        0x3ff306fe0a31b715,
        0x3ff33c08b26416ff,
        0x3ff371a7373aa9cb,
        0x3ff3a7db34e59ff7,
        0x3ff3dea64c123422,
        0x3ff4160a21f72e2a,
        0x3ff44e086061892d,
        0x3ff486a2b5c13cd0,
        0x3ff4bfdad5362a27,
        0x3ff4f9b2769d2ca7,
        0x3ff5342b569d4f82,
        0x3ff56f4736b527da,
        0x3ff5ab07dd485429,
        0x3ff5e76f15ad2148,
        0x3ff6247eb03a5585,
        0x3ff6623882552225,
        0x3ff6a09e667f3bcd,
        0x3ff6dfb23c651a2f,
        0x3ff71f75e8ec5f74,
        0x3ff75feb564267c9,
        0x3ff7a11473eb0187,
        0x3ff7e2f336cf4e62,
        0x3ff82589994cce13,
        0x3ff868d99b4492ed,
        0x3ff8ace5422aa0db,
        0x3ff8f1ae99157736,
        0x3ff93737b0cdc5e5,
        0x3ff97d829fde4e50,
        0x3ff9c49182a3f090,
        0x3ffa0c667b5de565,
        0x3ffa5503b23e255d,
        0x3ffa9e6b5579fdbf,
        0x3ffae89f995ad3ad,
        0x3ffb33a2b84f15fb,
        0x3ffb7f76f2fb5e47,
        0x3ffbcc1e904bc1d2,
        0x3ffc199bdd85529c,
        0x3ffc67f12e57d14b,
        0x3ffcb720dcef9069,
        0x3ffd072d4a07897c,
        0x3ffd5818dcfba487,
        0x3ffda9e603db3285,
        0x3ffdfc97337b9b5f,
        0x3ffe502ee78b3ff6,
        0x3ffea4afa2a490da,
        0x3ffefa1bee615a27,
        0x3fff50765b6e4540,
        0x3fffa7c1819e90d8,
    ];

    /// 64 over ln 2 and its inverse, at `0x1803e0e10` and `0x1803e0e20`.
    const INV: f64 = f64::from_bits(0x4057_1547_652b_82fe);
    const STEP: f64 = f64::from_bits(0x3f86_2e42_fefa_39ef);
    const SIXTH: f64 = f64::from_bits(0x3fc5_5555_5555_5555);
    const THIRD: f64 = f64::from_bits(0x3fd5_5555_5555_5555);

    /// ln 10 as `powf`'s logarithm makes it: 3 ln 2 plus ln 1.25 from its
    /// table (`0x1803e2870`), with nothing left for the polynomial.
    const LN10: f64 =
        3.0 * f64::from_bits(0x3fe6_2e42_fefa_39ef) + f64::from_bits(0x3fcc_8ff7_c79a_9a22);

    /// `cvtpd2dq` with the rounding FL runs with.
    fn nearest(x: f64) -> i32 {
        x.round_ties_even() as i32
    }

    fn table(n: i32) -> f64 {
        f64::from_bits(EXP2[(n & 63) as usize])
    }

    /// `powf(10, y)` (`0x1801c8b16`). Its base is always 10 here, which
    /// leaves its logarithm a constant.
    pub(super) fn pow10(y: f32) -> f32 {
        if y == 0.0 {
            return 1.0;
        }
        if y == 1.0 {
            return 10.0;
        }
        let t = f64::from(y) * LN10;
        if t > 88.722_839_355_468_75 {
            return f32::INFINITY;
        }
        if t <= -103.278_930_664_062_5 {
            return 0.0;
        }
        let n = nearest(t * INV);
        let r = (-f64::from(n)).mul_add(STEP, t);
        let q = r * SIXTH.mul_add(r, 0.5).mul_add(r, 1.0);
        let res = q.mul_add(table(n), table(n));
        let scale = (i64::from(n >> 6) as u64) << 52;
        f64::from_bits(res.to_bits().wrapping_add(scale)) as f32
    }

    /// `expf` (`0x1801c60b0`).
    pub(super) fn exp(x: f32) -> f32 {
        if x == f32::NEG_INFINITY {
            return 0.0;
        }
        let x = f64::from(x);
        let z = x * INV;
        if !(-9600.0..8192.0).contains(&z) {
            return (x.exp()) as f32;
        }
        let n = nearest(z);
        let r = (-f64::from(n)).mul_add(STEP, x);
        let p = (r * r).mul_add(SIXTH.mul_add(r, 0.5), r);
        let res = p.mul_add(table(n), table(n));
        let k = i64::from((n - (n & 63)) >> 6);
        (res * f64::from_bits(((k + 0x3ff) as u64) << 52)) as f32
    }

    /// `tanf` (`0x1801ca260`) up to pi/4, the most the plugin asks of it
    /// at a rate of 480 Hz or more. Past that the runtime reduces the
    /// argument, and this takes the f64 tangent.
    pub(super) fn tan(x: f32) -> f32 {
        let xd = f64::from(x);
        let a = xd.abs();
        if a.is_nan() || a > std::f64::consts::FRAC_PI_4 {
            return xd.tan() as f32;
        }
        if a < f64::from_bits(0x3e40_0000_0000_0000) {
            return x;
        }
        let x2 = xd * xd;
        if a < f64::from_bits(0x3f20_0000_0000_0000) {
            return (x2 * xd).mul_add(THIRD, xd) as f32;
        }
        let num = f64::from_bits(0xbf91_9dba_6efd_6aad)
            .mul_add(x2, f64::from_bits(0x3fd8_a8b0_da56_cb17));
        let den = f64::from_bits(0x3f92_e290_03c6_92d9)
            .mul_add(x2, f64::from_bits(0xbfe0_7266_d7b3_511b))
            .mul_add(x2, f64::from_bits(0x3ff2_7e84_a3e7_3a2e));
        (x2 * xd).mul_add(num / den, xd) as f32
    }
}

trait Libm {
    fn tan_f(self) -> f32;
    fn exp_f(self) -> f32;
}

impl Libm for f32 {
    fn tan_f(self) -> f32 {
        msvc::tan(self)
    }

    fn exp_f(self) -> f32 {
        msvc::exp(self)
    }
}

use msvc::pow10;

/// A Faust one-pole smoother: what comes out, and the state goes twice as
/// far.
fn glide(state: &mut f32, target: f32, take: f32, take2: f32) -> f32 {
    let s = *state;
    *state = take2 * (target - s) + s;
    take * (target - s) + s
}

/// A state-variable filter's two states.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Svf {
    a: f32,
    b: f32,
}

impl Svf {
    /// What the input loses before the step: `a * gk + b`.
    fn pre(&self, gk: f32) -> f32 {
        self.a * gk + self.b
    }

    /// Take the step `v` at tangent `g`: the band-pass and the low-pass.
    fn take(&mut self, v: f32, g: f32) -> (f32, f32) {
        let band = self.a + v;
        let part = band * g;
        self.a += v + v;
        let low = self.b + part;
        self.b += part + part;
        (band, low)
    }

    /// A step with `1 / den` given.
    fn step(&mut self, x: f32, f: &Tan) -> (f32, f32) {
        let v = (x - self.pre(f.gk)) * f.g * f.d;
        self.take(v, f.g)
    }

    /// A step that divides by `den`, as the right side's dry path does.
    fn step_div(&mut self, x: f32, f: &Tan) -> (f32, f32) {
        let v = ((x - self.pre(f.gk)) * f.g) / f.den;
        self.take(v, f.g)
    }
}

/// A tangent and what a filter at it uses: `g + k`, `g (g + k) + 1` and
/// one over that.
#[derive(Debug, Clone, Copy)]
struct Tan {
    g: f32,
    gk: f32,
    den: f32,
    d: f32,
}

impl Tan {
    fn new(g: f32) -> Self {
        let gk = g + SQRT2;
        let den = gk * g + 1.0;
        Tan {
            g,
            gk,
            den,
            d: 1.0 / den,
        }
    }
}

/// One band's envelope and shaper.
#[derive(Debug, Clone, Default, PartialEq)]
struct Band {
    line: Vec<f32>,
    /// Four peak holders: how long each has held, and its value.
    held: [(i32, f32); 4],
    /// Four followers of what the holders give.
    follow: [f32; 4],
    /// The 20 Hz highpass after the shaper.
    dc: Svf,
}

/// What every band shares in a sample.
struct Shape {
    lag: u32,
    hold: f32,
    rise: f32,
    /// The slope as a gain, its square and `1 / (1 + 2 s)`.
    s: f32,
    s2: f32,
    norm: f32,
}

impl Band {
    fn new() -> Self {
        Band {
            line: vec![0.0; LINE],
            ..Band::default()
        }
    }

    fn clear(&mut self) {
        self.line.fill(0.0);
        self.held = [(0, 0.0); 4];
        self.follow = [0.0; 4];
        self.dc = Svf::default();
    }

    /// Take a sample: the shaped one from the lookahead before, less its
    /// DC and before `1 / den`.
    fn step(&mut self, x: f32, at: u32, sh: &Shape, c: &Consts) -> f32 {
        self.line[(at & MASK) as usize] = x;
        let y = self.line[(at.wrapping_sub(sh.lag) & MASK) as usize];
        let mut peak = x.abs();
        for (count, value) in &mut self.held {
            let hold = (*count as f32) < sh.hold && peak < *value;
            *count = if hold { *count + 1 } else { 0 };
            if hold {
                peak = *value;
            }
            *value = peak;
        }
        if peak <= 1.0 {
            peak = 1.0;
        }
        for state in &mut self.follow {
            let pole = if peak <= *state { c.fall } else { sh.rise };
            peak = (1.0 - pole) * peak + pole * *state;
            *state = peak;
        }
        let u = (y * y) / (peak * peak);
        let t2 = (u + u) - 1.0;
        let w = (((((t2 + t2) - 1.0) * sh.s2 + 1.0) * y) / peak + t2 * sh.s) * sh.norm;
        let v = w - (self.dc.a * c.dc_gk + self.dc.b);
        let a = self.dc.a;
        self.dc.a = v * c.dc_g2 + a;
        self.dc.b += (v * c.dc_g + a) * c.dc_2g;
        v
    }
}

/// One rotation of the diffuser: a line and the tap it read last.
#[derive(Debug, Clone, Default, PartialEq)]
struct Turn {
    line: Vec<f32>,
    last: f32,
}

impl Turn {
    fn new(len: usize) -> Self {
        Turn {
            line: vec![0.0; len],
            last: 0.0,
        }
    }

    fn clear(&mut self) {
        self.line.fill(0.0);
        self.last = 0.0;
    }

    /// Write `into` the line, read `back` samples behind it, and give
    /// `sn * x + cs * last`. The caller works out `into` and the output
    /// where the order of their products differs.
    fn write(&mut self, into: f32, at: u32, back: i32, mask: u32) -> f32 {
        self.line[(at & mask) as usize] = into;
        let last = self.last;
        self.last = self.line[(at.wrapping_sub(back as u32) & mask) as usize];
        last
    }
}

/// One side's processor.
#[derive(Debug, Clone, PartialEq)]
struct Side {
    low1: Svf,
    low2: Svf,
    half1: Svf,
    half2: Svf,
    quarter: Svf,
    cross1: Svf,
    cross2: Svf,
    high: Vec<f32>,
    band1: Svf,
    band2: Svf,
    band3: Svf,
    bands: [Band; 4],
    turns: [Turn; 4],
    low_cut: Svf,
    dry_svf: Svf,
    dry_ap: f32,
    dry_quarter: Svf,
    dry_half: Svf,
    dry: Vec<f32>,
}

impl Side {
    fn new() -> Self {
        Side {
            low1: Svf::default(),
            low2: Svf::default(),
            half1: Svf::default(),
            half2: Svf::default(),
            quarter: Svf::default(),
            cross1: Svf::default(),
            cross2: Svf::default(),
            high: vec![0.0; LINE],
            band1: Svf::default(),
            band2: Svf::default(),
            band3: Svf::default(),
            bands: [Band::new(), Band::new(), Band::new(), Band::new()],
            turns: [
                Turn::new(LONG),
                Turn::new(LINE),
                Turn::new(LINE),
                Turn::new(LINE),
            ],
            low_cut: Svf::default(),
            dry_svf: Svf::default(),
            dry_ap: 0.0,
            dry_quarter: Svf::default(),
            dry_half: Svf::default(),
            dry: vec![0.0; LINE],
        }
    }

    fn clear(&mut self) {
        let svfs = [
            &mut self.low1,
            &mut self.low2,
            &mut self.half1,
            &mut self.half2,
            &mut self.quarter,
            &mut self.cross1,
            &mut self.cross2,
            &mut self.band1,
            &mut self.band2,
            &mut self.band3,
            &mut self.low_cut,
            &mut self.dry_svf,
            &mut self.dry_quarter,
            &mut self.dry_half,
        ];
        for svf in svfs {
            *svf = Svf::default();
        }
        self.high.fill(0.0);
        self.bands.iter_mut().for_each(Band::clear);
        self.turns.iter_mut().for_each(Turn::clear);
        self.dry_ap = 0.0;
        self.dry.fill(0.0);
    }

    /// The filters' states, for the flush of tiny numbers.
    fn states(&mut self) -> impl Iterator<Item = &mut f32> {
        let svfs = [
            &mut self.low1,
            &mut self.low2,
            &mut self.half1,
            &mut self.half2,
            &mut self.quarter,
            &mut self.cross1,
            &mut self.cross2,
            &mut self.band1,
            &mut self.band2,
            &mut self.band3,
            &mut self.low_cut,
            &mut self.dry_svf,
            &mut self.dry_quarter,
            &mut self.dry_half,
        ];
        let filters = svfs.into_iter().flat_map(|svf| [&mut svf.a, &mut svf.b]);
        let bands = self.bands.iter_mut().flat_map(|band| {
            let follow = band.follow.iter_mut();
            let held = band.held.iter_mut().map(|(_, value)| value);
            follow.chain(held).chain([&mut band.dc.a, &mut band.dc.b])
        });
        let turns = self.turns.iter_mut().map(|turn| &mut turn.last);
        filters
            .chain(bands)
            .chain(turns)
            .chain(std::iter::once(&mut self.dry_ap))
    }
}

/// What is the same for both sides in a sample.
struct Frame {
    at: u32,
    lag: u32,
    mb: f32,
    nmb: f32,
    hi: f32,
    gain: f32,
    mix: f32,
    sn: f32,
    cs: f32,
    pre: f32,
    fade: f32,
    back: [i32; 4],
    x: Tan,
    x2: Tan,
    x4: Tan,
    cut: f32,
    cut_gk: f32,
    cut_d: f32,
    cut_1: f32,
}

/// The two sides' wet paths up to the shaped bands: the high part for the
/// output, and the four band inputs.
fn split(side: &mut Side, x: f32, f: &Frame) -> [f32; 4] {
    let (band1, low1) = side.low1.step(x, &f.x);
    let (_, low2) = side.low2.step(low1, &f.x);
    let (band3, low3) = side.half1.step(x, &f.x2);
    let ap = x - band3 * SQRT8;
    let (_, low4) = side.half2.step(low3, &f.x2);
    let q = &mut side.quarter;
    let v = (ap - ((q.a * f.x4.gk + low4) + q.b)) * f.x4.g * f.x4.d;
    let (band5, _) = q.take(v, f.x4.g);
    let m = ap - (band5 * SQRT8 + low4);
    let (band6, low6) = side.cross1.step(m, &f.x);
    let (_, low7) = side.cross2.step(low6, &f.x);
    side.high[(f.at & MASK) as usize] =
        ((m - band6 * SQRT8) - low7) * f.mb + ((x - band1 * SQRT8) - low2) * f.nmb;
    let (band8, _) = side.band1.step(low4, &f.x);
    let u = low4 - band8 * SQRT8;
    let (band9, low9) = side.band2.step(u, &f.x4);
    let (_, low10) = side.band3.step(low9, &f.x4);
    [
        f.pre * low10,
        ((u - band9 * SQRT8) - low10) * f.pre,
        f.pre * low7,
        f.pre * low2,
    ]
}

/// The diffuser past its first rotation, from that rotation's output.
fn diffuse(turns: &mut [Turn; 4], first: f32, f: &Frame) -> f32 {
    let mut o = first;
    for (turn, back) in turns[1..].iter_mut().zip(&f.back[1..]) {
        let last = turn.write(f.cs * o - f.sn * turn.last, f.at, *back, MASK);
        o = f.sn * o + f.cs * last;
    }
    o
}

/// The low cut on the wet sum: what comes out, before the wet amount.
fn low_cut(svf: &mut Svf, mix: f32, f: &Frame) -> (f32, f32) {
    let x = mix - (svf.a * f.cut_gk + svf.b);
    let v = f.cut * x * f.cut_d;
    let a = svf.a;
    svf.a = (v + v) + a;
    let part = (v + a) * f.cut;
    svf.b += part + part;
    (x, f.cut_d * x)
}

/// The processor's state (`instanceClear` @ `0x180177320` zeroes it).
#[derive(Debug, Clone, PartialEq)]
struct Dsp {
    count: u32,
    dry: f32,
    cut: f32,
    high_on: f32,
    high_gain: f32,
    multiband: f32,
    cutoff: f32,
    polarity: f32,
    low_on: f32,
    low_gain: f32,
    mix: f32,
    depth: f32,
    pre: f32,
    slope: f32,
    fade: f32,
    sides: [Side; 2],
}

impl Dsp {
    fn new() -> Self {
        Dsp {
            count: 0,
            dry: 0.0,
            cut: 0.0,
            high_on: 0.0,
            high_gain: 0.0,
            multiband: 0.0,
            cutoff: 0.0,
            polarity: 0.0,
            low_on: 0.0,
            low_gain: 0.0,
            mix: 0.0,
            depth: 0.0,
            pre: 0.0,
            slope: 0.0,
            fade: 0.0,
            sides: [Side::new(), Side::new()],
        }
    }

    /// Everything at 0, with no new memory.
    fn clear(&mut self) {
        self.count = 0;
        for state in self.smoothers() {
            *state = 0.0;
        }
        self.sides.iter_mut().for_each(Side::clear);
    }

    fn smoothers(&mut self) -> [&mut f32; 14] {
        [
            &mut self.dry,
            &mut self.cut,
            &mut self.high_on,
            &mut self.high_gain,
            &mut self.multiband,
            &mut self.cutoff,
            &mut self.polarity,
            &mut self.low_on,
            &mut self.low_gain,
            &mut self.mix,
            &mut self.depth,
            &mut self.pre,
            &mut self.slope,
            &mut self.fade,
        ]
    }

    /// A number too small for an f32 to hold in full becomes 0. FL runs
    /// with such numbers rounded away.
    fn flush(&mut self) {
        let [left, right] = &mut self.sides;
        for state in left.states().chain(right.states()) {
            if state.is_subnormal() {
                *state = 0.0;
            }
        }
        for state in self.smoothers() {
            if state.is_subnormal() {
                *state = 0.0;
            }
        }
    }

    /// `compute` @ `0x180177f50`: the zones are read once a call.
    fn compute(&mut self, z: &Zones, c: &Consts, left: &mut [f32], right: &mut [f32]) {
        let (take, take2) = (c.take, c.take2);
        let dry_t = if z.on as i32 == 0 {
            1.0
        } else {
            1.0 - z.dry_wet
        };
        let high_t = pow10(z.high_db * 0.05);
        let ahead = if z.lookahead as i32 == 0 { 0.0 } else { 0.01 };
        let lag = (ahead * c.rate) as i32 as u32;
        let polarity_t = if z.invert as i32 == 0 { 1.0 } else { -1.0 };
        let low_t = pow10(z.low_db * 0.05);
        let pre_t = pow10(z.pre_db * 0.05);
        let hold = ((ahead + 0.05) * c.quarter).round_ties_even();
        let rise = (-(c.rise / ahead)).exp_f();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let dry = glide(&mut self.dry, dry_t, take, take2);
            let wet = 1.0 - dry;
            let cut = glide(&mut self.cut, z.low_cut, take, take2);
            let cut = (cut * c.turn).tan_f();
            let high_on = glide(&mut self.high_on, z.high_on, take, take2);
            let high_gain = glide(&mut self.high_gain, high_t, take, take2);
            let mb = glide(&mut self.multiband, z.multiband, take, take2);
            let cutoff = glide(&mut self.cutoff, z.cutoff, take, take2);
            let cutoff = if cutoff <= LOWEST { LOWEST } else { cutoff };
            let x = Tan::new((c.turn * cutoff).tan_f());
            let x4 = Tan::new((cutoff * c.turn4).tan_f());
            let x2 = Tan::new((cutoff * c.turn2).tan_f());

            let polarity = glide(&mut self.polarity, polarity_t, take, take2);
            let low_on = glide(&mut self.low_on, z.low_on, take, take2);
            let low_gain = glide(&mut self.low_gain, low_t, take, take2);
            let mix = glide(&mut self.mix, z.mix, take, take2);
            let depth = glide(&mut self.depth, z.depth, take, take2);
            let sn = if depth >= DEPTH { DEPTH } else { depth };
            let sn = if sn <= -DEPTH { -DEPTH } else { sn };
            let pre = glide(&mut self.pre, pre_t, take, take2);
            let fade = glide(&mut self.fade, 1.0, c.fade_take, c.fade_take2);
            let slope = glide(&mut self.slope, z.slope_db, c.slope_take, c.slope_take2);
            let s = pow10(slope * 0.05);
            let s2 = pow10(slope * 0.1);

            let over = 1.0 / cutoff;
            let back = c.delays.map(|d| {
                let d = over * d - 1.0;
                let d = if d <= 0.0 { 0.0 } else { d };
                (if LONGEST <= d { LONGEST } else { d }) as i32
            });
            let cut_gk = cut + SQRT2;
            let f = Frame {
                at: self.count,
                lag,
                mb,
                nmb: 1.0 - mb,
                hi: high_gain * high_on,
                gain: low_gain * (low_on * polarity),
                mix,
                sn,
                cs: (1.0 - sn * sn).sqrt(),
                pre,
                fade,
                back,
                x,
                x2,
                x4,
                cut,
                cut_gk,
                cut_d: 1.0 / (cut * cut_gk + 1.0),
                cut_1: cut + 1.0,
            };
            let sh = Shape {
                lag,
                hold,
                rise,
                s,
                s2,
                norm: 1.0 / ((s + s) + 1.0),
            };

            let (in_l, in_r) = (*l, *r);
            *l = self.side(0, in_l, &f, &sh, c, dry, wet);
            *r = self.side(1, in_r, &f, &sh, c, dry, wet);
            self.count = self.count.wrapping_add(1);
        }
    }

    /// One side of a sample. The two differ in the order of a few products
    /// and in how the dry path divides, as the binary has them.
    #[allow(clippy::too_many_arguments)]
    fn side(
        &mut self,
        which: usize,
        x: f32,
        f: &Frame,
        sh: &Shape,
        c: &Consts,
        dry: f32,
        wet: f32,
    ) -> f32 {
        let side = &mut self.sides[which];
        let inputs = split(side, x, f);
        let [v1, v2, v3, v4] = {
            let [b1, b2, b3, b4] = &mut side.bands;
            [
                b1.step(inputs[0], f.at, sh, c),
                b2.step(inputs[1], f.at, sh, c),
                b3.step(inputs[2], f.at, sh, c),
                b4.step(inputs[3], f.at, sh, c),
            ]
        };
        let k = c.dc_d;
        let three = ((v2 * k) + (v1 * k)) + (v3 * k);
        let first = &mut side.turns[0];
        let (sum, rotated) = if which == 0 {
            let sum = (three * f.mb + f.nmb * (v4 * k)) * f.fade;
            let last = first.write(f.cs * sum - f.sn * first.last, f.at, f.back[0], LONG_MASK);
            (sum, f.sn * sum + f.cs * last)
        } else {
            let sum = three * f.mb + (v4 * k) * f.nmb;
            let into = sum * f.fade * f.cs - f.sn * first.last;
            let last = first.write(into, f.at, f.back[0], LONG_MASK);
            (sum, f.sn * f.fade * sum + f.cs * last)
        };
        let diffused = diffuse(&mut side.turns, rotated, f);
        let plain = if which == 0 {
            (1.0 - f.mix) * sum
        } else {
            (1.0 - f.mix) * f.fade * sum
        };
        let high = side.high[(f.at.wrapping_sub(f.lag) & MASK) as usize];
        let mixed = (diffused * f.mix + plain) * f.gain + f.hi * high;
        let (_, wet_out) = low_cut(&mut side.low_cut, mixed, f);

        // The dry path: the allpasses the wet path went through, at the
        // cutoff, the low cut, a quarter and a half of the cutoff. The
        // right side divides where the left multiplies.
        let divide = which == 1;
        let d = &mut side.dry_svf;
        let t = ((d.a * f.x.gk + x) + d.b) * f.x.g;
        let t = if divide { t / f.x.den } else { t * f.x.d };
        let (band, _) = neg(d, t, f.x.g);
        let input = -x - band * SQRT8;
        let s = side.dry_ap;
        let v = ((input - s) * f.cut) / f.cut_1;
        let v = v + v;
        side.dry_ap = v + s;
        let ap = ((s + s) + v) - input;
        let step = if divide { Svf::step_div } else { Svf::step };
        let (band_q, _) = step(&mut side.dry_quarter, ap, &f.x4);
        let ap_q = ap - band_q * SQRT8;
        let (band_h, _) = step(&mut side.dry_half, ap_q, &f.x2);
        side.dry[(f.at & MASK) as usize] = (ap_q - band_h * SQRT8) * f.mb + ap * f.nmb;
        let late = side.dry[(f.at.wrapping_sub(f.lag) & MASK) as usize];
        dry * late + wet_out * wet
    }
}

/// The dry path's first filter, which takes its step `t` negated.
fn neg(svf: &mut Svf, t: f32, g: f32) -> (f32, f32) {
    let band = svf.a - t;
    svf.a -= t + t;
    let part = band * g;
    let low = svf.b + part;
    svf.b += part + part;
    (band, low)
}

/// Low Lifter. A new one has the defaults FL's render-test project saves.
#[derive(Debug, Clone, PartialEq)]
pub struct LowLifter {
    params: [i32; 15],
    /// The editor's part of the state, which nothing here reads.
    editor: [u8; 18],
    hz: u32,
    consts: Consts,
    zones: Zones,
    dsp: Dsp,
    /// The render's count of quiet input (`0x128`), and what loud input
    /// sets it back to: the rate (`0x124`).
    quiet: i32,
    wake: i32,
}

impl Default for LowLifter {
    fn default() -> Self {
        LowLifter {
            params: DEFAULTS,
            editor: EDITOR,
            hz: 44_100,
            consts: Consts::new(44_100),
            zones: Zones::new(&DEFAULTS),
            dsp: Dsp::new(),
            quiet: 0,
            wake: 44_100,
        }
    }
}

impl LowLifter {
    /// Take parameters that are inside their ranges.
    fn set(&mut self, params: [i32; 15]) {
        self.params = params;
        self.zones = Zones::new(&params);
    }
}

impl Effect for LowLifter {
    /// What is sounding stays, and the controls go to the state's through
    /// their smoothers, as in FL.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN)?;
        if version != VERSION {
            return Err(StateError::Version { found: version });
        }
        if bytes.len() != LEN {
            return Err(StateError::Length {
                expected: LEN,
                found: bytes.len(),
            });
        }
        let words = bytes[4..64].as_chunks::<4>().0;
        let params: [i32; 15] = std::array::from_fn(|i| i32::from_le_bytes(words[i]));
        for (param, (value, range)) in params.iter().zip(RANGES).enumerate() {
            if *value < range.0 || *value > range.1 {
                return Err(StateError::Range {
                    param,
                    value: *value,
                });
            }
        }
        self.editor.copy_from_slice(&bytes[64..]);
        self.set(params);
        Ok(())
    }

    /// FL hands its processor the rate in whole Hz, and the processor
    /// starts over. A rate that is not a number of 1 Hz or more is not
    /// taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz >= 1.0 && hz as u32 != self.hz {
            self.hz = hz as u32;
            self.consts = Consts::new(self.hz);
            self.dsp.clear();
            self.wake = self.hz as i32;
        }
    }

    /// FL's render (`FUN_006c7a80`): once the input has stayed at or under
    /// -120 dB for a second, the blocks after are passed as they are, and
    /// the processor waits.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let loud = |v: &f32| v.to_bits() << 1 > QUIET.to_bits() << 1;
        if left.iter().chain(right.iter()).any(loud) {
            self.quiet = self.wake;
        } else {
            self.quiet = self.quiet.wrapping_sub(len as i32);
        }
        if i64::from(self.quiet) <= -(len as i64) {
            self.quiet = 0xc000_0001_u32 as i32;
            return;
        }
        for (l, r) in left.chunks_mut(CHUNK).zip(right.chunks_mut(CHUNK)) {
            self.dsp.compute(&self.zones, &self.consts, l, r);
        }
        self.dsp.flush();
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

    /// FL's flush: the processor's clear.
    fn reset(&mut self) {
        self.dsp.clear();
    }

    /// With the lookahead on, the plugin tells FL it is 10 ms late
    /// (`FUN_006c7e00`), rounded to whole frames.
    fn latency(&self) -> usize {
        if self.params[4] == 1 {
            (f64::from(self.hz) * 0.01).round_ties_even() as usize
        } else {
            0
        }
    }

    fn max_latency(&self) -> usize {
        (f64::from(self.hz) * 0.01).round_ties_even() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{every_preset, partial, presets, sine, state};

    /// A state of version 6 with these parameters.
    fn bytes(params: [i32; 15]) -> Vec<u8> {
        let mut bytes = state(&[VERSION]);
        bytes.extend(state(&params));
        bytes.extend(EDITOR);
        bytes
    }

    fn with(params: [i32; 15]) -> LowLifter {
        let mut plugin = LowLifter::default();
        plugin.set_state(&bytes(params)).unwrap();
        plugin
    }

    /// Run a plugin over a block in pieces of 441, and give what came out.
    fn run(plugin: &mut LowLifter, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (left.to_vec(), right.to_vec());
        for (l, r) in l.chunks_mut(441).zip(r.chunks_mut(441)) {
            plugin.process(l, r);
        }
        (l, r)
    }

    /// The four Low Lifters of FL's projects that the references below
    /// were taken with: the render test's, the two of Bien Duro (one
    /// shown), and one with the polarity inverted and no lookahead.
    const RENDER_TEST: [i32; 15] = DEFAULTS;
    const BIEN_DURO: [i32; 15] = [
        1, 7506, 5000, 3277, 1, 0, 6757, 5053, 9608, 10_000, 7778, 5281, 1, 1, 1,
    ];
    const NIGHTCLUB: [i32; 15] = [
        1, 10_000, 5000, 2104, 1, 1, 9002, 4413, 7395, 0, 9582, 4424, 1, 1, 1,
    ];
    const MASTERING: [i32; 15] = [
        1, 10_000, 5000, 3837, 1, 0, 3773, 2147, 4935, 10_000, 6210, 4045, 1, 0, 1,
    ];

    /// 6000 frames of two triangles with a little buzz on them, all
    /// numbers an f32 holds exactly.
    fn reference_input() -> (Vec<f32>, Vec<f32>) {
        (0..6000)
            .map(|n: i32| {
                let tri = ((n % 512) - 256).abs() as f32 / 256.0 - 0.5;
                let l = tri * 0.75 + ((n * 37 % 256) - 128) as f32 / 2048.0;
                let tri = ((n % 1024) - 512).abs() as f32 / 512.0 - 0.5;
                let r = tri * 0.5 + ((n * 97 % 128) - 64) as f32 / 1024.0;
                (l, r)
            })
            .unzip()
    }

    /// FNV-1a over the bits of the left side and then the right.
    fn fnv(left: &[f32], right: &[f32]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for v in left.iter().chain(right) {
            hash ^= u64::from(v.to_bits());
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        hash
    }

    #[test]
    fn it_comes_out_as_ildsp_s_own_code_makes_it_to_the_bit() {
        // `ILRDLowFreqEnhancer::compute` of `ildsp_x64.dll` run under an
        // emulator at 44.1 kHz, 441 frames a call, its zones set to what
        // `zone` gives: the hash of all it put out, and three frames of
        // each side.
        let (left, right) = reference_input();
        for (params, hash, l, r) in [
            (
                RENDER_TEST,
                0xd486_08e3_55ae_5be1,
                [0xbea1_7e9f, 0xbe8a_6c5d, 0xbe92_4781],
                [0x3e1b_1f49, 0xbe0e_be45, 0x3d5b_7214],
            ),
            (
                BIEN_DURO,
                0x9184_c813_3fd0_f3a2,
                [0xbdb6_6bd8, 0x3eac_e62b, 0x3e40_e2c4],
                [0x3ba6_21fc, 0xbd3d_32b4, 0x3e39_e996],
            ),
            (
                NIGHTCLUB,
                0xc5bc_8df9_00d5_29df,
                [0xbba7_d04c, 0xbe36_4f36, 0xbe80_e00a],
                [0x3b37_ce03, 0x3ea2_d9dd, 0x3ece_0f80],
            ),
            (
                MASTERING,
                0x8d07_224e_c6d4_b9df,
                [0xbb1b_a195, 0xbc21_3e91, 0xbcc9_f855],
                [0xbb26_f8f3, 0x3d72_3eeb, 0x3d9d_adb5],
            ),
        ] {
            let mut plugin = with(params);
            let (out_l, out_r) = run(&mut plugin, &left, &right);
            let at = [1000, 4000, 5999];
            assert_eq!(at.map(|n| out_l[n].to_bits()), l, "{params:?}");
            assert_eq!(at.map(|n| out_r[n].to_bits()), r, "{params:?}");
            assert_eq!(fnv(&out_l, &out_r), hash, "{params:?}");
        }
    }

    #[test]
    fn the_wheels_come_to_the_doc_s_zones() {
        // The ends of every curve, and the render test's wheels: 0 dB of
        // low gain, -6 dB of slope, 80 Hz, 0 for the diffuser and +5 dB
        // of high gain.
        assert_eq!(zone(2, 0), -120.0);
        assert_eq!(zone(2, 10_000), 12.0);
        assert!(zone(2, 4999).abs() < 0.01);
        assert_eq!((zone(3, 0), zone(3, 10_000)), (-60.0, 0.0));
        assert!((zone(3, 7210) + 6.0).abs() < 0.001);
        assert_eq!((zone(6, 0), zone(6, 10_000)), (0.0, 18.0));
        assert!((zone(6, 5000) - 9.0).abs() < 1e-6);
        assert_eq!(zone(8, 5000), 0.0);
        assert!((zone(8, 0) + 0.99).abs() < 1e-7 && (zone(8, 10_000) - 0.99).abs() < 1e-7);
        assert_eq!((zone(9, 0), zone(9, 10_000)), (-120.0, 5.0));
        assert_eq!((zone(10, 0), zone(10, 10_000)), (20.0, 120.0));
        assert!((zone(10, 7750) - 80.0).abs() < 0.01);
        assert_eq!(zone(11, 7750), zone(10, 7750));
        // The mix wheels go over as they are, the boxes as 0 or 1.
        assert_eq!((zone(1, 2500), zone(7, 10_000)), (0.25, 1.0));
        assert_eq!((zone(0, 1), zone(4, 1), zone(14, 0)), (1.0, 1.0, 0.0));
        // Each curve is `a + (b - a) n^(1/c)`, to the last bits of
        // Delphi's `Ln` and `Exp`.
        for (param, (a, b, c)) in CURVES
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((i, (*c)?)))
        {
            for wheel in [1, 37, 1234, 5000, 8888, 9999] {
                let n = f64::from(wheel as f32 / 10_000.0);
                let want = a + (b - a) * n.powf(1.0 / c);
                assert!(
                    (f64::from(zone(param, wheel)) - want).abs() < 1e-4,
                    "{param} {wheel}"
                );
            }
        }
        // FL's own results where an f64 power does not round the same:
        // pre gain at 112 is 0.20159999, not 0.2016.
        assert_eq!(zone(6, 112).to_bits(), 0x3e4e_703a);
    }

    #[test]
    fn delphi_s_ln_and_exp_are_the_maths_ones_to_an_ulp() {
        for x in [1e-4_f64, 0.0123, 0.4999, 0.5, 0.7, 0.9, 1.0] {
            assert!((delphi::ln(x) - x.ln()).abs() <= 2.0 * f64::EPSILON * x.ln().abs().max(1.0));
        }
        for x in [-18.4_f64, -9.21, -1.0, -0.69, -0.1, -1e-9] {
            let e = x.exp();
            assert!((delphi::exp(x) - e).abs() <= 2.0 * f64::EPSILON * e, "{x}");
        }
        assert_eq!(delphi::ln(0.0), f64::NEG_INFINITY);
        assert_eq!(
            (delphi::exp(f64::NEG_INFINITY), delphi::exp(0.0)),
            (0.0, 1.0)
        );
    }

    #[test]
    fn the_runtime_s_maths_are_the_f64_ones_rounded_but_where_fl_s_differ() {
        for i in 1..2000 {
            let x = i as f32 * 3.0e-5;
            assert!(
                msvc::tan(x)
                    .to_bits()
                    .abs_diff((f64::from(x).tan() as f32).to_bits())
                    <= 1
            );
            let y = -6.0 + i as f32 * 3.4e-3;
            let p = 10f64.powf(f64::from(y)) as f32;
            assert!(msvc::pow10(y).to_bits().abs_diff(p.to_bits()) <= 1, "{y}");
            let x = -(i as f32) * 2.0e-5;
            let e = f64::from(x).exp() as f32;
            assert!(msvc::exp(x).to_bits().abs_diff(e.to_bits()) <= 1, "{x}");
        }
        // Under 2^-13 `tanf` is `x + x^3 / 3`, and under 2^-27 `x`.
        let x = 1.0e-4_f32;
        let small = f64::from(x).mul_add(f64::from(x) * f64::from(x) / 3.0, 0.0);
        assert!((f64::from(msvc::tan(x)) - (f64::from(x) + small)).abs() < 1e-12);
        assert_eq!(msvc::tan(1.0e-9), 1.0e-9);
        // What the emulated runtime gives where the f64 power rounds the
        // other way: 10^-0.5632547 is 0.27336648, not 0.27336651.
        let y = -0.563_254_7_f32;
        assert_eq!(msvc::pow10(y), 0.273_366_48);
        assert_ne!(msvc::pow10(y), 10f64.powf(f64::from(y)) as f32);
        assert_eq!((msvc::pow10(0.0), msvc::pow10(1.0)), (1.0, 10.0));
        assert_eq!(msvc::exp(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn the_rate_gives_the_doc_s_constants() {
        let c = Consts::new(44_100);
        let g = (std::f64::consts::PI * 20.0 / 44_100.0).tan();
        assert!((f64::from(c.take) - g / (1.0 + g)).abs() < 1e-7);
        assert_eq!(c.take2, c.take + c.take);
        assert!((f64::from(c.fall) - (-144.447_86_f64 / 44_100.0).exp()).abs() < 1e-7);
        assert_eq!(c.quarter, 11_025.0);
        // The rotations' delays at 80 Hz: 275, 113, 86 and 72 frames.
        let back = c.delays.map(|d| (d / 80.0 - 1.0) as i32);
        assert_eq!(back, [274, 113, 86, 72]);
        // The processor counts with no more than 192 kHz.
        assert_eq!(Consts::new(384_000), Consts::new(192_000));
    }

    #[test]
    fn dry_the_plugin_is_an_allpass_and_late_by_the_lookahead() {
        // Dry/wet at 0 leaves the dry path: the allpasses the wet one
        // goes through, so every tone comes out as loud as it went in,
        // 441 frames late with the lookahead on.
        let mut dry = DEFAULTS;
        dry[1] = 0;
        for turns in [60.0, 150.0, 1000.0, 9000.0] {
            let tone = sine(turns / 44_100.0 * 4096.0 / 4096.0, 32_768);
            let mut plugin = with(dry);
            let (l, r) = run(&mut plugin, &tone, &tone);
            // The last 4410 frames, ten periods of 100 Hz and more of the
            // rest, are whole periods of every tone here.
            let level = partial(&l[l.len() - 4410..], turns / 44_100.0);
            assert!((level - 1.0).abs() < 2e-3, "{turns}: {level}");
            // The right side divides where the left multiplies, so the
            // two differ in their last bits.
            let apart = l.iter().zip(&r).map(|(l, r)| (l - r).abs());
            assert!(apart.fold(0.0, f32::max) < 1e-6);
        }
        // A click after two seconds for the smoothers to settle on the
        // dry signal alone, over a hiss just loud enough to keep the
        // processor running.
        let click = |plugin: &mut LowLifter| {
            let hiss: Vec<f32> = (0..88_200)
                .map(|n| if n % 2 == 0 { 2e-6 } else { -2e-6 })
                .collect();
            run(plugin, &hiss, &hiss);
            let mut click = vec![0.0; 2000];
            click[0] = 1.0;
            click[1..].iter_mut().step_by(2).for_each(|v| *v = 2e-6);
            run(plugin, &click, &click).0
        };
        let loudest = |s: &[f32]| s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let mut plugin = with(dry);
        assert_eq!(plugin.latency(), 441);
        let l = click(&mut plugin);
        assert!(loudest(&l[..441]) < 1e-5 && loudest(&l[441..]) > 0.1);
        // Switched off is the same as dry.
        let mut off = DEFAULTS;
        off[0] = 0;
        assert!(click(&mut with(off)) == l);
        // With no lookahead it is on time.
        let mut on_time = dry;
        on_time[4] = 0;
        let mut plugin = with(on_time);
        assert_eq!(plugin.latency(), 0);
        assert!(loudest(&click(&mut plugin)[..20]) > 0.1);
    }

    #[test]
    fn a_bass_tone_gets_its_second_and_third_harmonics() {
        // 50 Hz under the 80 Hz crossover, with the slope at 0 dB: the
        // shaper adds T2 and T3 of it as loud as itself, over the gain of
        // 1 + 2 s. The high band is off and the diffuser dry.
        let turns = 50.0 / 44_100.0;
        let tone: Vec<f32> = sine(turns, 44_100 * 2).iter().map(|v| v * 0.5).collect();
        let mut params = DEFAULTS;
        params[3] = 10_000;
        params[13] = 0;
        let mut plugin = with(params);
        let (l, _) = run(&mut plugin, &tone, &tone);
        let tail = &l[l.len() - 8820..];
        let (first, second, third) = (
            partial(tail, turns),
            partial(tail, 2.0 * turns),
            partial(tail, 3.0 * turns),
        );
        assert!(second > 0.02 && third > 0.02, "{first} {second} {third}");
        // Dry, there are none.
        params[1] = 0;
        let mut plugin = with(params);
        let (l, _) = run(&mut plugin, &tone, &tone);
        let tail = &l[l.len() - 8820..];
        assert!(partial(tail, 2.0 * turns) < 1e-3 && partial(tail, 3.0 * turns) < 1e-3);
    }

    #[test]
    fn a_second_of_quiet_input_stops_the_processor_and_passes_the_input() {
        let mut plugin = with(BIEN_DURO);
        let tone = sine(100.0 / 44_100.0, 4410);
        run(&mut plugin, &tone, &tone);
        // At -120 dB: the plugin runs on for a second, 100 blocks, and its
        // tail fades.
        let quiet = vec![1e-6; 441];
        for _ in 0..100 {
            let (l, _) = run(&mut plugin, &quiet, &quiet);
            assert!(l != quiet);
        }
        let before = plugin.clone();
        let (l, r) = run(&mut plugin, &quiet, &quiet);
        assert!(l == quiet && r == quiet);
        assert!(plugin.dsp == before.dsp);
        // A sample over -120 dB wakes it.
        let mut loud = quiet.clone();
        loud[300] = 2e-6;
        let (l, _) = run(&mut plugin, &loud, &loud);
        assert!(l != loud);
        // A new plugin starts asleep until it hears something.
        let mut plugin = with(BIEN_DURO);
        let (l, _) = run(&mut plugin, &[0.0; 441], &[0.0; 441]);
        assert!(l.iter().all(|v| *v == 0.0));
        assert_eq!(plugin.dsp, with(BIEN_DURO).dsp);
    }

    #[test]
    fn a_flush_and_a_new_rate_start_the_processor_over() {
        let mut plugin = with(NIGHTCLUB);
        let fresh = plugin.clone();
        let (left, right) = reference_input();
        let first = run(&mut plugin, &left, &right);
        assert_ne!(plugin.dsp, fresh.dsp);
        plugin.reset();
        assert_eq!(plugin.dsp, fresh.dsp);
        assert_eq!(run(&mut plugin, &left, &right), first);

        plugin.set_sample_rate(48_000.0);
        assert_eq!(plugin.consts, Consts::new(48_000));
        assert_eq!(plugin.dsp, fresh.dsp);
        assert_eq!(plugin.latency(), 480);
        run(&mut plugin, &left, &right);
        let before = plugin.clone();
        plugin.set_sample_rate(48_000.0);
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(0.0);
        assert_eq!(plugin, before);
    }

    #[test]
    fn a_moved_wheel_asks_for_no_memory_and_is_the_same_plugin_as_its_state() {
        let mut plugin = LowLifter::default();
        let memory = |p: &LowLifter| {
            let side = &p.dsp.sides[0];
            (
                side.high.as_ptr(),
                side.turns[0].line.as_ptr(),
                side.bands[3].line.as_ptr(),
            )
        };
        let before = memory(&plugin);
        let moves = [0.0, 2500.4, -3.0, 20_000.0, 0.0, 7.0, f32::NAN];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
            plugin.process(&mut [0.25; 200], &mut [-0.25; 200]);
        }
        plugin.set_param(15, 1.0);
        assert_eq!((plugin.param(15), plugin.param_count()), (0.0, 15));
        assert_eq!(
            [0, 1, 2, 3, 4, 5, 6].map(|i| plugin.param(i)),
            [0.0, 2500.0, 0.0, 10_000.0, 0.0, 1.0, 0.0]
        );
        assert_eq!(memory(&plugin), before);
        let mut params = DEFAULTS;
        params[..7].copy_from_slice(&[0, 2500, 0, 10_000, 0, 1, 0]);
        let mut same = with(params);
        same.dsp = plugin.dsp.clone();
        same.quiet = plugin.quiet;
        assert_eq!(plugin, same);
        assert_eq!(with(DEFAULTS), LowLifter::default());
    }

    #[test]
    fn only_a_state_of_version_6_and_82_bytes_loads() {
        let mut plugin = with(BIEN_DURO);
        let before = plugin.clone();
        for len in [3, 81, 83] {
            let mut wrong = bytes(DEFAULTS);
            wrong.resize(len, 0);
            assert_eq!(
                plugin.set_state(&wrong),
                Err(StateError::Length {
                    expected: 82,
                    found: len
                })
            );
        }
        for version in [5, 7] {
            let mut other = bytes(DEFAULTS);
            other[0] = version;
            assert_eq!(
                plugin.set_state(&other),
                Err(StateError::Version {
                    found: i32::from(version)
                })
            );
        }
        for (param, value) in [(0, 2), (1, 10_001), (2, -1), (14, -1)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&bytes(params)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin, before);
        // The editor's bytes are kept.
        let mut other = bytes(DEFAULTS);
        other[70] = 9;
        plugin.set_state(&other).unwrap();
        assert_eq!(plugin.editor[6], 9);
    }

    #[test]
    fn the_states_fl_ships_load() {
        let Some(own) = presets(NAME) else { return };
        // Seven presets of its own, all of version 6.
        assert_eq!(own.len(), 7);
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert!(all.len() >= own.len());
        for preset in own.iter().chain(&all) {
            let mut plugin = LowLifter::default();
            plugin.set_state(&preset.state).unwrap();
            let (l, r) = run(&mut plugin, &reference_input().0, &reference_input().1);
            let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak.is_finite() && peak < 8.0, "{}: {peak}", preset.file);
        }
    }
}
