//! Transporter's engine: `TransientLooperFaust` of FL's
//! `Shared/ildsp_x64.dll`, a Faust program (`transientDetectionLooper.dsp`,
//! compiled `-single -ftz 0`) behind a small C++ wrapper,
//! `TransientLooperImpl`. `docs/plugins/transporter.md` section 4 maps it.
//!
//! The Faust object is kept as FL lays it out: [`Looper::s`] holds its
//! words, each at [`o`] of its byte offset in FL's object, so that the
//! code below reads as the DLL's `compute` (`0x18018c4e0`) and can be held
//! to the emulator (`tools/plugstate/transient_looper_emu.py`) zone by
//! zone. The two loop buffers (`0x1610` and `0xea96b8`, 20 seconds each)
//! and the last 1024-frame ring (`0x1d4f6b8`) are vectors of their own.
//!
//! What it does, per frame:
//!
//! 1. A 10 Hz one-pole glides the bypass; its square crossfades the wet
//!    sum with the dry input.
//! 2. The dry input waits 5 ms (lookahead on) in a 1024-frame ring.
//! 3. The transient source (left, right or their sum over sqrt 2) goes
//!    through a tree of seven state-variable splits (48 Hz to 8.6 kHz)
//!    into eight bands. Each band's power runs through four peak
//!    followers (fast attack, 289/fs release) and, beside them, four that
//!    follow slowly while the band is not rising past the sensitivity;
//!    the log of the fast over the slow is compared with the sensitivity,
//!    and the log of the slow one rising by half a decade marks an onset.
//!    The count of bands that did (its lowest bit, as FL's code has it),
//!    20 ms after the last, is the transient.
//! 4. Two loops, A and B, record the input into 20-second rings. On a
//!    transient once the loop has run its minimum length (or on a trigger,
//!    or the tempo's beat, and unless ignored by chance or in turn), a
//!    loop's start becomes its last end and its end the write point; it
//!    then plays from there, reversed, an octave up or down by chance or
//!    in turn, with a 4-pole fade at every jump.
//! 5. The two loops are panned against each other (spread, or an auto-pan
//!    sine at 20/fs turns a frame) and summed under the dry input.

use crate::low_lifter::msvc;

/// The bytes of FL's object this keeps in [`Looper::s`]: the first part
/// up to the first loop buffer, then the part between the two buffers.
const HEAD: usize = 0x1610;
const MID: usize = 0xea7610;
const MID_END: usize = 0xea96b8;
const WORDS: usize = HEAD / 4 + (MID_END - MID) / 4;

/// The word of [`Looper::s`] that holds FL's byte offset `off`.
const fn o(off: usize) -> usize {
    if off < HEAD {
        off / 4
    } else {
        (off - MID) / 4 + HEAD / 4
    }
}

/// The words `instanceClear` (`0x18018b5d0`) zeroes, as byte ranges, but
/// for the last ring, which is [`Looper::ring`].
const CLEARED: [(usize, usize); 37] = [
    (0x8, 0x10),
    (0x2c, 0x1038),
    (0x1050, 0x1058),
    (0x1060, 0x1068),
    (0x10c0, 0x10c8),
    (0x10cc, 0x10ec),
    (0x10f0, 0x1110),
    (0x1114, 0x112c),
    (0x1138, 0x1158),
    (0x115c, 0x11e4),
    (0x11f8, 0x1200),
    (0x1204, 0x12bc),
    (0x12e4, 0x12ec),
    (0x12f0, 0x1310),
    (0x1314, 0x13cc),
    (0x13e0, 0x13e8),
    (0x13ec, 0x14ac),
    (0x14b0, 0x14c0),
    (0x14c4, 0x14cc),
    (0x14d4, 0x14dc),
    (0x14e0, 0x14e8),
    (0x14ec, 0x1514),
    (0x1524, 0x152c),
    (0x1530, 0x1538),
    (0x1540, 0x1550),
    (0x1554, 0x155c),
    (0x1564, 0x1574),
    (0x1578, 0x15a0),
    (0x15a4, 0x15ac),
    (0x15b4, 0x15bc),
    (0x15c0, 0x15e8),
    (0x15ec, 0x15f4),
    (0x15fc, 0x160c),
    (0xea7614, 0xea9614),
    (0xea9620, 0xea9680),
    (0xea9684, 0xea96ac),
    (0xea96b0, 0xea96b8),
];

/// The zones (the Faust program's controls) by their offsets, as
/// `buildUserInterface` (`0x18018bbf0`) names them.
pub(crate) mod zone {
    pub const BYPASS: usize = 0x28;
    pub const LOOKAHEAD: usize = 0x1038;
    pub const INPUT_LEVEL: usize = 0x104c;
    pub const FLUSH: usize = 0x105c;
    pub const LOOP_MODE: usize = 0x106c;
    pub const MIN_LENGTH_A: usize = 0x1070;
    pub const SOURCE: usize = 0x10bc;
    pub const SENSITIVITY: usize = 0x1158;
    pub const TRIGGER_A: usize = 0x14d0;
    pub const TRIGGER_AB: usize = 0x14dc;
    pub const TRIGGER_B: usize = 0x14e8;
    pub const FREEZE: usize = 0x152c;
    pub const TEMPO_MULTIPLIER: usize = 0x1538;
    pub const DETERMINISTIC: usize = 0x1550;
    pub const IGNORE: usize = 0x155c;
    pub const BOUNCING_RATE: usize = 0x1574;
    pub const REVERSE: usize = 0x15a0;
    pub const OCTAVE: usize = 0x15ac;
    pub const OCTAVE_UP: usize = 0x15b0;
    pub const LOOP_LEVEL_A: usize = 0x15e8;
    pub const SPREAD: usize = 0x15f4;
    pub const STEREO: usize = 0xea7610;
    pub const AB_RATIO: usize = 0xea9614;
    pub const LOOP_LEVEL_B: usize = 0xea96ac;
}

/// `FLT_EPSILON`, the floor of the lookahead and the levels' divisors.
const EPS: f32 = 1.192_092_9e-7;
/// `2 sqrt 2`, a state-variable split's high-pass feedback.
const SQRT8: f32 = 2.828_427;
/// 2^-32, a random word to a number between -0.5 and 0.5.
const TWO_M32: f32 = 2.328_306_4e-10;

/// `logf` of the runtime (`0x1801c6910`). Taken as the f64 logarithm rounded once,
/// which the runtime's agrees with but for the last bit now and then; it
/// only meets a comparison with a threshold.
fn logf(x: f32) -> f32 {
    f64::from(x).ln() as f32
}

/// `fmodf`, which the runtime works out exactly, as this does.
fn fmodf(x: f32, y: f32) -> f32 {
    x % y
}

/// The floor the Faust code takes with a truncation and the sign bit.
fn floorf(x: f32) -> f32 {
    let i = x as i32;
    if i == i32::MIN || i as f32 == x {
        x
    } else {
        (i - i32::from(x.is_sign_negative())) as f32
    }
}

/// `cvttss2si` into a word, as the code turns a 0-or-more float to bits.
fn trunc(x: f32) -> u32 {
    x as i32 as u32
}

fn gi(s: &[f32], off: usize) -> i32 {
    s[o(off)].to_bits() as i32
}

fn si(s: &mut [f32], off: usize, v: i32) {
    s[o(off)] = f32::from_bits(v as u32);
}

/// The 65536 sines of `classInit` (`0x18018bab0`): `sinf(i * 2 pi /
/// 65536)` as f32, for the auto-pan.
fn sines() -> &'static [f32] {
    static SINES: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    SINES.get_or_init(|| {
        (0..65536)
            .map(|i| f64::from(i as f32 * 9.587_38e-5).sin() as f32)
            .collect()
    })
}

/// The per-frame recursions FL keeps twice, as `[now, before]`: the end
/// of a frame copies each `now` word into its `before` (`0x18018fb..`).
const CARRIED: &[(usize, usize)] = &[
    (0xc, 0x8),
    (0x1054, 0x1050),
    (0x1064, 0x1060),
    (0x10c4, 0x10c0),
    (0x10d0, 0x10cc),
    (0x10d8, 0x10d4),
    (0x10e0, 0x10dc),
    (0x10e8, 0x10e4),
    (0x10f4, 0x10f0),
    (0x10fc, 0x10f8),
    (0x1104, 0x1100),
    (0x110c, 0x1108),
    (0x1118, 0x1114),
    (0x1120, 0x111c),
    (0x1128, 0x1124),
    (0x113c, 0x1138),
    (0x1144, 0x1140),
    (0x114c, 0x1148),
    (0x1154, 0x1150),
    (0x1160, 0x115c),
    (0x1168, 0x1164),
    (0x1170, 0x116c),
    (0x1178, 0x1174),
    (0x1180, 0x117c),
    (0x1188, 0x1184),
    (0x1190, 0x118c),
    (0x1198, 0x1194),
    (0x11a0, 0x119c),
    (0x11a8, 0x11a4),
    (0x11b0, 0x11ac),
    (0x11b8, 0x11b4),
    (0x11c0, 0x11bc),
    (0x11c8, 0x11c4),
    (0x11d0, 0x11cc),
    (0x11d8, 0x11d4),
    (0x11e0, 0x11dc),
    (0x11fc, 0x11f8),
    (0x1208, 0x1204),
    (0x1210, 0x120c),
    (0x1218, 0x1214),
    (0x1220, 0x121c),
    (0x1228, 0x1224),
    (0x1230, 0x122c),
    (0x1238, 0x1234),
    (0x1240, 0x123c),
    (0x1248, 0x1244),
    (0x1250, 0x124c),
    (0x1258, 0x1254),
    (0x1260, 0x125c),
    (0x1268, 0x1264),
    (0x1270, 0x126c),
    (0x1278, 0x1274),
    (0x1280, 0x127c),
    (0x1288, 0x1284),
    (0x1290, 0x128c),
    (0x1298, 0x1294),
    (0x12a0, 0x129c),
    (0x12a8, 0x12a4),
    (0x12b0, 0x12ac),
    (0x12b8, 0x12b4),
    (0x12e8, 0x12e4),
    (0x12f4, 0x12f0),
    (0x12fc, 0x12f8),
    (0x1304, 0x1300),
    (0x130c, 0x1308),
    (0x1318, 0x1314),
    (0x1320, 0x131c),
    (0x1328, 0x1324),
    (0x1330, 0x132c),
    (0x1338, 0x1334),
    (0x1340, 0x133c),
    (0x1348, 0x1344),
    (0x1350, 0x134c),
    (0x1358, 0x1354),
    (0x1360, 0x135c),
    (0x1368, 0x1364),
    (0x1370, 0x136c),
    (0x1378, 0x1374),
    (0x1380, 0x137c),
    (0x1388, 0x1384),
    (0x1390, 0x138c),
    (0x1398, 0x1394),
    (0x13a0, 0x139c),
    (0x13a8, 0x13a4),
    (0x13b0, 0x13ac),
    (0x13b8, 0x13b4),
    (0x13c0, 0x13bc),
    (0x13c8, 0x13c4),
    (0x13e4, 0x13e0),
    (0x13f0, 0x13ec),
    (0x13f8, 0x13f4),
    (0x1400, 0x13fc),
    (0x1408, 0x1404),
    (0x1410, 0x140c),
    (0x1418, 0x1414),
    (0x1420, 0x141c),
    (0x1428, 0x1424),
    (0x1430, 0x142c),
    (0x1438, 0x1434),
    (0x1440, 0x143c),
    (0x1448, 0x1444),
    (0x1450, 0x144c),
    (0x1458, 0x1454),
    (0x1460, 0x145c),
    (0x1468, 0x1464),
    (0x1470, 0x146c),
    (0x1478, 0x1474),
    (0x1480, 0x147c),
    (0x1488, 0x1484),
    (0x1490, 0x148c),
    (0x1498, 0x1494),
    (0x14a0, 0x149c),
    (0x14a8, 0x14a4),
    (0x14b4, 0x14b0),
    (0x14bc, 0x14b8),
    (0x14c8, 0x14c4),
    (0x14d8, 0x14d4),
    (0x14e4, 0x14e0),
    (0x14f0, 0x14ec),
    (0x14f8, 0x14f4),
    (0x1500, 0x14fc),
    (0x1508, 0x1504),
    (0x1510, 0x150c),
    (0x1528, 0x1524),
    (0x1534, 0x1530),
    (0x1544, 0x1540),
    (0x154c, 0x1548),
    (0x1558, 0x1554),
    (0x1568, 0x1564),
    (0x1570, 0x156c),
    (0x157c, 0x1578),
    (0x1584, 0x1580),
    (0x158c, 0x1588),
    (0x1594, 0x1590),
    (0x159c, 0x1598),
    (0x15a8, 0x15a4),
    (0x15b8, 0x15b4),
    (0x15c4, 0x15c0),
    (0x15cc, 0x15c8),
    (0x15d4, 0x15d0),
    (0x15dc, 0x15d8),
    (0x15e4, 0x15e0),
    (0x15f0, 0x15ec),
    (0x1600, 0x15fc),
    (0x1608, 0x1604),
    (0xea9624, 0xea9620),
    (0xea962c, 0xea9628),
    (0xea9634, 0xea9630),
    (0xea963c, 0xea9638),
    (0xea9644, 0xea9640),
    (0xea964c, 0xea9648),
    (0xea9654, 0xea9650),
    (0xea965c, 0xea9658),
    (0xea9664, 0xea9660),
    (0xea966c, 0xea9668),
    (0xea9674, 0xea9670),
    (0xea967c, 0xea9678),
    (0xea9688, 0xea9684),
    (0xea9690, 0xea968c),
    (0xea9698, 0xea9694),
    (0xea96a0, 0xea969c),
    (0xea96a8, 0xea96a4),
    (0xea96b4, 0xea96b0),
];

/// What a block's frames share, worked out from the zones once a block as
/// `compute` does before its loop.
struct Block {
    bypass: f32,
    /// The lookahead in frames (as a float, and truncated).
    ahead: f32,
    ahead_frames: i32,
    input_gain: f32,
    /// The 4-pole fade's pole and its complement.
    fade_pole: f32,
    fade_rest: f32,
    /// The write point's modulus: the buffer, or 1 while flushing.
    modulus: i32,
    trigger_a: f32,
    trigger_ab: f32,
    trigger_b: f32,
    sensitivity: f32,
    span: f32,
    min_length: f32,
    source: f32,
    adaptive: bool,
    bouncing: u32,
    deterministic: i32,
    freeze: f32,
    tempo_multiplier: f32,
    ignore: f32,
    keep_share: f32,
    bounce: f32,
    rate: f32,
    reverse: f32,
    octave: f32,
    octave_up: f32,
    level_a: f32,
    spread: f32,
    spread_gain: f32,
    stereo: i32,
    spread_step: f32,
    ratio_inverse: f32,
    min_length_b: f32,
    level_b: f32,
}

/// The Faust program, with its zones, state and buffers.
pub(crate) struct Looper {
    s: Vec<f32>,
    /// Loop A's and loop B's recordings: 20 seconds at the rate.
    a: Vec<f32>,
    b: Vec<f32>,
    /// Loop B's lookahead ring (`0x1d4f6b8`).
    ring: Vec<f32>,
}

impl Looper {
    /// A looper at `rate`, as `TransientLooperImpl` makes one: `init`,
    /// then the controls' defaults.
    pub(crate) fn new(rate: i32) -> Self {
        let mut looper = Looper {
            s: vec![0.0; WORDS],
            a: Vec::new(),
            b: Vec::new(),
            ring: vec![0.0; 1024],
        };
        looper.set_rate(rate);
        looper.reset_zones();
        looper.clear();
        looper
    }

    /// A zone's value.
    #[cfg(test)]
    pub(crate) fn zone(&self, off: usize) -> f32 {
        self.s[o(off)]
    }

    pub(crate) fn set_zone(&mut self, off: usize, value: f32) {
        self.s[o(off)] = value;
    }

    /// `instanceConstants` (`0x18018afd0`): the coefficients at `rate`,
    /// and both loops emptied and sized to 20 seconds. This allocates.
    pub(crate) fn set_rate(&mut self, rate: i32) {
        let s = &mut self.s;
        si(s, 0x10, rate);
        let r = rate as f32;
        let c0 = if r <= 1.0 { 1.0 } else { r };
        let c0 = if c0 < 192_000.0 { c0 } else { 192_000.0 };
        let c1 = 1.0 / c0;
        s[o(0x14)] = c0;
        let one_pole = |s: &mut [f32], at: usize, arg: f32| {
            let g = msvc::tan(arg);
            s[o(at)] = g;
            let k = g / (g + 1.0);
            s[o(at + 4)] = k;
            s[o(at + 8)] = k + k;
        };
        s[o(0x18)] = c1 * 31.415_926;
        let smooth10 = s[o(0x18)];
        one_pole(s, 0x1c, smooth10);
        s[o(0x103c)] = c0 * 0.005;
        one_pole(s, 0x1040, c1 * 62.831_852);
        let p = 1.0 / (c1 * 1.805_598_4 + 1.0);
        s[o(0x1058)] = c0 * 20.0;
        s[o(0x1074)] = p;
        s[o(0x1078)] = 1.0 - p;
        s[o(0x107c)] = c0 * 0.02 - 1.0;
        // The seven state-variable splits: g, g + sqrt 2, g (g + sqrt 2)
        // + 1, g over that, twice that, and 2 g.
        for (arg, at, twice_g) in [
            (149.440_17, 0x1080, 0x1110),
            (355.430_63, 0x1094, 0x10ec),
            (2_010.619_3, 0x10a8, 0x10c8),
            (845.361_27, 0x11e4, 0x1200),
            (4_782.085_4, 0x12bc, 0x1310),
            (11373.78, 0x12d0, 0x12ec),
            (27051.56, 0x13cc, 0x13e8),
        ] {
            let g = msvc::tan(c1 * arg);
            let a = g + std::f32::consts::SQRT_2;
            let b = a * g + 1.0;
            let c = g / b;
            s[o(at)] = g;
            s[o(at + 4)] = a;
            s[o(at + 8)] = b;
            s[o(at + 12)] = c;
            s[o(at + 16)] = c + c;
            s[o(twice_g)] = g + g;
        }
        s[o(0x112c)] = 1.0 / (c1 * 288.895_72 + 1.0);
        s[o(0x1130)] = c1 * 14.444_787;
        s[o(0x1134)] = 1.0 / (c1 * 14_444.787 + 1.0);
        s[o(0x14ac)] = c0 * 0.01;
        s[o(0x14c0)] = msvc::exp(-smooth10);
        s[o(0x15f8)] = c1 * 20.0;
        s[o(0x1560)] = c0 + c0;
        let len = (c0 * 20.0) as usize + 2;
        self.a = vec![0.0; len];
        self.b = vec![0.0; len];
    }

    /// `instanceResetUserInterface` (`0x18018b530`): every control at its
    /// default.
    pub(crate) fn reset_zones(&mut self) {
        let s = &mut self.s;
        for at in [
            zone::BYPASS,
            zone::LOOKAHEAD,
            zone::INPUT_LEVEL,
            zone::FLUSH,
            zone::LOOP_MODE,
            zone::SOURCE,
            zone::TRIGGER_A,
            zone::TRIGGER_AB,
            zone::TRIGGER_B,
            zone::FREEZE,
            zone::DETERMINISTIC,
            zone::IGNORE,
            zone::BOUNCING_RATE,
            zone::REVERSE,
            zone::OCTAVE,
            zone::OCTAVE_UP,
            zone::LOOP_LEVEL_A,
            zone::SPREAD,
            zone::STEREO,
            zone::LOOP_LEVEL_B,
        ] {
            s[o(at)] = 0.0;
        }
        s[o(zone::MIN_LENGTH_A)] = 0.5;
        s[o(zone::SENSITIVITY)] = 0.5;
        s[o(zone::TEMPO_MULTIPLIER)] = 21.0;
        s[o(zone::AB_RATIO)] = 3.0;
    }

    /// `instanceClear` (`0x18018b5d0`): every recursion and ring at 0. The
    /// loops keep what they hold.
    pub(crate) fn clear(&mut self) {
        for (from, to) in CLEARED {
            self.s[o(from)..o(to)].fill(0.0);
        }
        self.ring.fill(0.0);
    }

    /// Empty both loops, as the wrapper does when a flush has faded out.
    pub(crate) fn empty_loops(&mut self) {
        self.a.fill(0.0);
        self.b.fill(0.0);
    }

    fn block(&self) -> Block {
        let s = &self.s;
        let z = |off: usize| s[o(off)];
        let ahead = if z(zone::LOOKAHEAD) as i32 != 0 {
            z(0x103c)
        } else {
            EPS
        };
        let pole = 1.0 / (14.444_787 / ahead + 1.0);
        let modulus = if z(zone::FLUSH) <= 0.5 {
            z(0x1058)
        } else {
            1.0
        };
        let mode = z(zone::LOOP_MODE);
        let span = 1.0 - z(zone::SENSITIVITY);
        let spread = z(zone::SPREAD);
        let ratio_inverse = 1.0 / z(zone::AB_RATIO);
        Block {
            bypass: z(zone::BYPASS),
            ahead,
            ahead_frames: ahead as i32,
            input_gain: msvc::pow10(z(zone::INPUT_LEVEL) * 0.05),
            fade_pole: pole,
            fade_rest: 1.0 - pole,
            modulus: modulus as i32,
            trigger_a: z(zone::TRIGGER_A),
            trigger_ab: z(zone::TRIGGER_AB),
            trigger_b: z(zone::TRIGGER_B),
            sensitivity: span * span * 6.5,
            span: z(0x1560),
            min_length: z(zone::MIN_LENGTH_A),
            source: z(zone::SOURCE),
            adaptive: mode == 2.0,
            bouncing: u32::from(mode == 1.0),
            deterministic: z(zone::DETERMINISTIC) as i32,
            freeze: z(zone::FREEZE),
            tempo_multiplier: z(zone::TEMPO_MULTIPLIER),
            ignore: z(zone::IGNORE),
            keep_share: (1.0 - z(zone::IGNORE)) * z(0x1560),
            bounce: msvc::exp(z(zone::BOUNCING_RATE) * -0.5),
            rate: z(0x14),
            reverse: z(zone::REVERSE),
            octave: z(zone::OCTAVE),
            octave_up: z(zone::OCTAVE_UP),
            level_a: msvc::pow10(z(zone::LOOP_LEVEL_A) * 0.05),
            spread,
            spread_gain: if spread < 0.0 { 1.0 } else { spread },
            stereo: z(zone::STEREO) as i32,
            spread_step: spread * z(0x15f8),
            ratio_inverse,
            min_length_b: ratio_inverse * z(zone::MIN_LENGTH_A),
            level_b: msvc::pow10(z(zone::LOOP_LEVEL_B) * 0.05),
        }
    }

    /// `compute` (`0x18018c4e0`) over one block: `ins` are the input's
    /// left and right, the transient source's left and right and the
    /// beat; `outs` the main pair and the loops A and B.
    pub(crate) fn compute(&mut self, ins: [&[f32]; 5], outs: [&mut [f32]; 4]) {
        let k = self.block();
        let [o0, o1, o2, o3] = outs;
        for i in 0..o0.len() {
            let frame = self.frame(&k, [ins[0][i], ins[1][i], ins[2][i], ins[3][i], ins[4][i]]);
            o0[i] = frame[0];
            o1[i] = frame[1];
            o2[i] = frame[2];
            o3[i] = frame[3];
        }
    }

    /// One frame of `compute`'s loop, transcribed from the DLL.
    #[allow(
        unused_assignments,
        clippy::assign_op_pattern,
        clippy::too_many_lines,
        clippy::collapsible_if,
        clippy::collapsible_else_if,
        clippy::neg_cmp_op_on_partial_ord
    )]
    fn frame(&mut self, k: &Block, input: [f32; 5]) -> [f32; 4] {
        let s = &mut self.s[..];
        let (f31, f32c) = (1.0f32, 2.0f32);
        let f83init = SQRT8;
        let mut f83 = f83init;
        let (f2, f3, f6, f7, f78) = (
            k.trigger_a,
            k.trigger_ab,
            k.source,
            k.trigger_b,
            k.sensitivity,
        );
        let (mut f60, mut f61, mut f62, mut f63, mut f64, mut f65, mut f66, mut f67) =
            (0f32, 0f32, 0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
        let (mut f79, mut f80, mut f81, mut f82, mut f86, mut f89, mut f91) =
            (0f32, 0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
        let (mut f93, mut f95, mut f96, mut f97, mut f98, mut f99, mut f100, mut f33) =
            (0f32, 0f32, 0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
        let (mut u41, mut u44, mut u50, mut u51) = (0u32, 0u32, 0u32, 0u32);
        let (mut b28, mut b29) = (false, false);

        // The bypass glide, and the dry input into its lookahead ring.
        si(s, 0x8, 1);
        let mut f85 = s[o(0x30)];
        let f84d = (f31 - f85) - k.bypass;
        s[o(0x2c)] = f84d * s[o(0x24)] + f85;
        f85 = f84d * s[o(0x20)] + f85;
        f85 = f85 * f85;
        let mut f84 = input[0];
        let iota = gi(s, 0x34);
        s[o(0x38) + (iota as u32 & 0x3ff) as usize] = f84;
        f60 = s[o(0x1054)];
        let f17 = s[o(0x38) + (iota.wrapping_sub(k.ahead_frames) as u32 & 0x3ff) as usize];
        let i43 = gi(s, 0xc)
            .wrapping_add(gi(s, 0x1064))
            .wrapping_rem(k.modulus);
        s[o(0x1050)] = s[o(0x1048)] * (k.input_gain - f60) + f60;
        si(s, 0x1060, i43);
        f60 = s[o(0x1044)] * (k.input_gain - f60) + f60;
        f61 = i43 as f32;
        s[o(0x1068)] = f61;
        f91 = input[3];
        f89 = input[2];
        if f6 < f32c {
            if f6 < f31 {
                f91 = f89;
            }
        } else {
            f91 = (f91 + f89) * std::f32::consts::FRAC_1_SQRT_2;
        }

        // The band splits and the onset detector.
        f89 = s[o(0x10c4)];
        f80 = s[o(0x10d0)];
        f79 = f91 - (s[o(0x10ac)] * f89 + f80);
        s[o(0x10c0)] = s[o(0x10b8)] * f79 + f89;
        f89 = s[o(0x10b4)] * f79 + f89;
        f79 = s[o(0x10d8)];
        s[o(0x10cc)] = s[o(0x10c8)] * f89 + f80;
        f91 = f91 - f89 * f83;
        f81 = s[o(0x10f4)];
        f96 = s[o(0x10e0)];
        f80 = (s[o(0x10a8)] * f89 + f80) - (f79 * s[o(0x10ac)] + f96);
        f95 = s[o(0x10b4)] * f80 + f79;
        f89 = s[o(0x10e8)];
        s[o(0x10d4)] = s[o(0x10b8)] * f80 + f79;
        f98 = s[o(0x10a8)] * f95 + f96;
        f80 = s[o(0x1104)];
        s[o(0x10dc)] = s[o(0x10c8)] * f95 + f96;
        f79 = f98 - (s[o(0x1098)] * f89 + f81);
        s[o(0x10e4)] = s[o(0x10a4)] * f79 + f89;
        f89 = s[o(0x10a0)] * f79 + f89;
        f79 = s[o(0x10fc)];
        s[o(0x10f0)] = s[o(0x10ec)] * f89 + f81;
        f100 = f98 - f89 * f83;
        f96 = s[o(0x1118)];
        f81 = (s[o(0x1094)] * f89 + f81) - (f79 * s[o(0x1098)] + f80);
        f95 = s[o(0x10a0)] * f81 + f79;
        f89 = s[o(0x110c)];
        s[o(0x10f8)] = s[o(0x10a4)] * f81 + f79;
        f99 = s[o(0x1094)] * f95 + f80;
        f79 = s[o(0x1128)];
        s[o(0x1100)] = s[o(0x10ec)] * f95 + f80;
        f80 = f99 - (s[o(0x1084)] * f89 + f96);
        s[o(0x1108)] = s[o(0x1090)] * f80 + f89;
        f89 = s[o(0x108c)] * f80 + f89;
        f80 = s[o(0x1120)];
        s[o(0x1114)] = s[o(0x1110)] * f89 + f96;
        f81 = (s[o(0x1080)] * f89 + f96) - (f80 * s[o(0x1084)] + f79);
        f96 = s[o(0x108c)] * f81 + f80;
        s[o(0x111c)] = s[o(0x1090)] * f81 + f80;
        f81 = s[o(0x1080)] * f96 + f79;
        s[o(0x1124)] = s[o(0x1110)] * f96 + f79;
        f80 = f81 * f81;
        if f80 <= s[o(0x113c)] {
            f79 = s[o(0x112c)];
        } else {
            f79 = s[o(0x1134)];
        }
        f79 = (f31 - f79) * f80 + s[o(0x113c)] * f79;
        s[o(0x1138)] = f79;
        if f79 <= s[o(0x1144)] {
            f96 = s[o(0x112c)];
        } else {
            f96 = s[o(0x1134)];
        }
        f79 = (f31 - f96) * f79 + s[o(0x1144)] * f96;
        s[o(0x1140)] = f79;
        if f79 <= s[o(0x114c)] {
            f96 = s[o(0x112c)];
        } else {
            f96 = s[o(0x1134)];
        }
        f95 = (f31 - f96) * f79 + s[o(0x114c)] * f96;
        s[o(0x1148)] = f95;
        f96 = 0.001;
        f79 = EPS;
        if f95 <= s[o(0x1154)] {
            f62 = s[o(0x112c)];
        } else {
            f62 = s[o(0x1134)];
        }
        f86 = (f31 - f62) * f95 + s[o(0x1154)] * f62;
        f95 = s[o(0x1160)];
        f62 = 1.5 - f95;
        s[o(0x1150)] = f86;
        f62 = f62 + f78;
        f93 = s[o(0x1044)] * f62 + f95;
        s[o(0x115c)] = s[o(0x1048)] * f62 + f95;
        f95 = s[o(0x1188)];
        if s[o(0x1188)] <= f79 {
            f95 = f79;
        }
        f95 = (f86).sqrt() * (f31 / f95);
        if f95 <= f96 {
            f95 = f96;
        }
        f86 = logf(f95);
        f62 = 8_388_608.0;
        f95 = 20.0;
        if f93 < f86 {
            f95 = 8_388_608.0;
        }
        f95 = f31 / (f95 * s[o(0x1130)] + f31);
        f86 = f95;
        if f80 <= s[o(0x1168)] {
            f86 = s[o(0x112c)];
        }
        f86 = (f31 - f86) * f80 + s[o(0x1168)] * f86;
        s[o(0x1164)] = f86;
        f80 = f95;
        if f86 <= s[o(0x1170)] {
            f80 = s[o(0x112c)];
        }
        f86 = (f31 - f80) * f86 + s[o(0x1170)] * f80;
        s[o(0x116c)] = f86;
        f80 = f95;
        if f86 <= s[o(0x1178)] {
            f80 = s[o(0x112c)];
        }
        f80 = (f31 - f80) * f86 + s[o(0x1178)] * f80;
        s[o(0x1174)] = f80;
        if f80 <= s[o(0x1180)] {
            f95 = s[o(0x112c)];
        }
        f80 = (f31 - f95) * f80 + s[o(0x1180)] * f95;
        s[o(0x117c)] = f80;
        f80 = (f80).sqrt();
        s[o(0x1184)] = f80;
        if f80 <= f96 {
            f80 = f96;
        }
        f96 = logf(f80);
        f80 = 0.5;
        f81 = (f99 - f89 * f83) - f81;
        s[o(0x118c)] = f96;
        f83 = s[o(0x1190)];
        f81 = f81 * f81;
        if f81 <= s[o(0x1198)] {
            f89 = s[o(0x112c)];
        } else {
            f89 = s[o(0x1134)];
        }
        f89 = (f31 - f89) * f81 + s[o(0x1198)] * f89;
        s[o(0x1194)] = f89;
        if f89 <= s[o(0x11a0)] {
            f95 = s[o(0x112c)];
        } else {
            f95 = s[o(0x1134)];
        }
        f89 = (f31 - f95) * f89 + s[o(0x11a0)] * f95;
        s[o(0x119c)] = f89;
        if f89 <= s[o(0x11a8)] {
            f95 = s[o(0x112c)];
        } else {
            f95 = s[o(0x1134)];
        }
        f95 = (f31 - f95) * f89 + s[o(0x11a8)] * f95;
        s[o(0x11a4)] = f95;
        f89 = 0.001;
        if f95 <= s[o(0x11b0)] {
            f86 = s[o(0x112c)];
        } else {
            f86 = s[o(0x1134)];
        }
        f86 = (f31 - f86) * f95 + s[o(0x11b0)] * f86;
        s[o(0x11ac)] = f86;
        f95 = s[o(0x11d8)];
        if s[o(0x11d8)] <= f79 {
            f95 = f79;
        }
        f79 = (f86).sqrt() * (f31 / f95);
        if f79 <= f89 {
            f79 = f89;
        }
        f95 = logf(f79);
        f79 = 20.0;
        if f93 < f95 {
            f79 = f62;
        }
        f79 = f31 / (f79 * s[o(0x1130)] + f31);
        f95 = f79;
        if f81 <= s[o(0x11b8)] {
            f95 = s[o(0x112c)];
        }
        f95 = (f31 - f95) * f81 + s[o(0x11b8)] * f95;
        s[o(0x11b4)] = f95;
        f81 = f79;
        if f95 <= s[o(0x11c0)] {
            f81 = s[o(0x112c)];
        }
        f95 = (f31 - f81) * f95 + s[o(0x11c0)] * f81;
        s[o(0x11bc)] = f95;
        f81 = f79;
        if f95 <= s[o(0x11c8)] {
            f81 = s[o(0x112c)];
        }
        f81 = (f31 - f81) * f95 + s[o(0x11c8)] * f81;
        s[o(0x11c4)] = f81;
        if f81 <= s[o(0x11d0)] {
            f79 = s[o(0x112c)];
        }
        f79 = (f31 - f79) * f81 + s[o(0x11d0)] * f79;
        s[o(0x11cc)] = f79;
        f79 = (f79).sqrt();
        s[o(0x11d4)] = f79;
        if f79 <= f89 {
            f79 = f89;
        }
        f63 = logf(f79);
        f66 = SQRT8;
        s[o(0x11dc)] = f63;
        f79 = s[o(0x11e0)];
        f81 = s[o(0x11fc)];
        f95 = s[o(0x1208)];
        f62 = f100 - (s[o(0x11e8)] * f81 + f99 + f95);
        s[o(0x11f8)] = s[o(0x11f4)] * f62 + f81;
        f81 = s[o(0x11f0)] * f62 + f81;
        s[o(0x1204)] = s[o(0x1200)] * f81 + f95;
        f62 = s[o(0x1210)];
        f86 = s[o(0x1218)];
        f95 = (s[o(0x11e4)] * f81 + f95) - (f62 * s[o(0x11e8)] + f86);
        f97 = s[o(0x11f0)] * f95 + f62;
        s[o(0x120c)] = s[o(0x11f4)] * f95 + f62;
        f62 = s[o(0x11e4)] * f97 + f86;
        f95 = f62 * f62;
        s[o(0x1214)] = s[o(0x1200)] * f97 + f86;
        if f95 <= s[o(0x1220)] {
            f86 = s[o(0x112c)];
        } else {
            f86 = s[o(0x1134)];
        }
        f86 = (f31 - f86) * f95 + s[o(0x1220)] * f86;
        s[o(0x121c)] = f86;
        if f86 <= s[o(0x1228)] {
            f97 = s[o(0x112c)];
        } else {
            f97 = s[o(0x1134)];
        }
        f86 = (f31 - f97) * f86 + s[o(0x1228)] * f97;
        s[o(0x1224)] = f86;
        if f86 <= s[o(0x1230)] {
            f97 = s[o(0x112c)];
        } else {
            f97 = s[o(0x1134)];
        }
        f97 = (f31 - f97) * f86 + s[o(0x1230)] * f97;
        s[o(0x122c)] = f97;
        f86 = EPS;
        if f97 <= s[o(0x1238)] {
            f64 = s[o(0x112c)];
        } else {
            f64 = s[o(0x1134)];
        }
        f64 = (f31 - f64) * f97 + s[o(0x1238)] * f64;
        s[o(0x1234)] = f64;
        f97 = s[o(0x1260)];
        if s[o(0x1260)] <= f86 {
            f97 = f86;
        }
        f97 = (f64).sqrt() * (f31 / f97);
        if f97 <= f89 {
            f97 = f89;
        }
        f65 = logf(f97);
        f82 = 8_388_608.0;
        f64 = 20.0;
        f97 = 20.0;
        if f93 < f65 {
            f97 = 8_388_608.0;
        }
        f97 = f31 / (f97 * s[o(0x1130)] + f31);
        f65 = f97;
        if f95 <= s[o(0x1240)] {
            f65 = s[o(0x112c)];
        }
        f65 = (f31 - f65) * f95 + s[o(0x1240)] * f65;
        s[o(0x123c)] = f65;
        f95 = f97;
        if f65 <= s[o(0x1248)] {
            f95 = s[o(0x112c)];
        }
        f65 = (f31 - f95) * f65 + s[o(0x1248)] * f95;
        s[o(0x1244)] = f65;
        f95 = f97;
        if f65 <= s[o(0x1250)] {
            f95 = s[o(0x112c)];
        }
        f95 = (f31 - f95) * f65 + s[o(0x1250)] * f95;
        s[o(0x124c)] = f95;
        if f95 <= s[o(0x1258)] {
            f97 = s[o(0x112c)];
        }
        f95 = (f31 - f97) * f95 + s[o(0x1258)] * f97;
        s[o(0x1254)] = f95;
        f95 = (f95).sqrt();
        s[o(0x125c)] = f95;
        if f95 <= f89 {
            f95 = f89;
        }
        f95 = logf(f95);
        s[o(0x1264)] = f95;
        f62 = (f100 - (f81 * f66 + f99)) - f62;
        f62 = f62 * f62;
        b28 = 0.5 < f95 - s[o(0x1268)];
        if f62 <= s[o(0x1270)] {
            f81 = s[o(0x112c)];
        } else {
            f81 = s[o(0x1134)];
        }
        f81 = (f31 - f81) * f62 + s[o(0x1270)] * f81;
        s[o(0x126c)] = f81;
        if f81 <= s[o(0x1278)] {
            f95 = s[o(0x112c)];
        } else {
            f95 = s[o(0x1134)];
        }
        f81 = (f31 - f95) * f81 + s[o(0x1278)] * f95;
        s[o(0x1274)] = f81;
        if f81 <= s[o(0x1280)] {
            f95 = s[o(0x112c)];
        } else {
            f95 = s[o(0x1134)];
        }
        f81 = (f31 - f95) * f81 + f95 * s[o(0x1280)];
        s[o(0x127c)] = f81;
        if f81 <= s[o(0x1288)] {
            f95 = s[o(0x112c)];
        } else {
            f95 = s[o(0x1134)];
        }
        f95 = (f31 - f95) * f81 + f95 * s[o(0x1288)];
        s[o(0x1284)] = f95;
        f81 = s[o(0x12b0)];
        if s[o(0x12b0)] <= f86 {
            f81 = f86;
        }
        f81 = (f95).sqrt() * (f31 / f81);
        if f81 <= f89 {
            f81 = f89;
        }
        f81 = logf(f81);
        if f93 < f81 {
            f64 = f82;
        }
        f81 = f31 / (f64 * s[o(0x1130)] + f31);
        f95 = f81;
        if f62 <= s[o(0x1290)] {
            f95 = s[o(0x112c)];
        }
        f99 = (f31 - f95) * f62 + f95 * s[o(0x1290)];
        s[o(0x128c)] = f99;
        f95 = f81;
        if f99 <= s[o(0x1298)] {
            f95 = s[o(0x112c)];
        }
        f99 = (f31 - f95) * f99 + f95 * s[o(0x1298)];
        s[o(0x1294)] = f99;
        f95 = f81;
        if f99 <= s[o(0x12a0)] {
            f95 = s[o(0x112c)];
        }
        f95 = (f31 - f95) * f99 + f95 * s[o(0x12a0)];
        s[o(0x129c)] = f95;
        if f95 <= s[o(0x12a8)] {
            f81 = s[o(0x112c)];
        }
        f81 = (f31 - f81) * f95 + f81 * s[o(0x12a8)];
        s[o(0x12a4)] = f81;
        f81 = (f81).sqrt();
        s[o(0x12ac)] = f81;
        if f81 <= f89 {
            f81 = f89;
        }
        f95 = logf(f81);
        s[o(0x12b4)] = f95;
        f99 = 0.5;
        f89 = s[o(0x12e8)];
        f81 = s[o(0x12f4)];
        b29 = 0.5 < f95 - s[o(0x12b8)];
        f95 = f91 - (s[o(0x12d4)] * f89 + f98 + f81);
        s[o(0x12e4)] = f95 * s[o(0x12e0)] + f89;
        f89 = f95 * s[o(0x12dc)] + f89;
        s[o(0x12f0)] = f89 * s[o(0x12ec)] + f81;
        f95 = s[o(0x1304)];
        f91 = f91 - (f89 * f66 + f98);
        f98 = s[o(0x12fc)];
        f89 = (f89 * s[o(0x12d0)] + f81) - (f98 * s[o(0x12d4)] + f95);
        f81 = f89 * s[o(0x12dc)] + f98;
        s[o(0x12f8)] = f89 * s[o(0x12e0)] + f98;
        f62 = f81 * s[o(0x12d0)] + f95;
        s[o(0x1300)] = f81 * s[o(0x12ec)] + f95;
        f89 = s[o(0x130c)];
        f81 = s[o(0x1318)];
        f95 = f62 - (s[o(0x12c0)] * f89 + f81);
        s[o(0x1308)] = f95 * s[o(0x12cc)] + f89;
        f89 = f95 * s[o(0x12c8)] + f89;
        s[o(0x1314)] = f89 * s[o(0x1310)] + f81;
        f95 = s[o(0x1320)];
        f98 = s[o(0x1328)];
        f81 = (f89 * s[o(0x12bc)] + f81) - (f95 * s[o(0x12c0)] + f98);
        f100 = f81 * s[o(0x12c8)] + f95;
        s[o(0x131c)] = f81 * s[o(0x12cc)] + f95;
        f95 = f100 * s[o(0x12bc)] + f98;
        f81 = f95 * f95;
        s[o(0x1324)] = f100 * s[o(0x1310)] + f98;
        if f81 <= s[o(0x1330)] {
            f98 = s[o(0x112c)];
        } else {
            f98 = s[o(0x1134)];
        }
        f98 = (f31 - f98) * f81 + f98 * s[o(0x1330)];
        s[o(0x132c)] = f98;
        if f98 <= s[o(0x1338)] {
            f100 = s[o(0x112c)];
        } else {
            f100 = s[o(0x1134)];
        }
        f98 = (f31 - f100) * f98 + f100 * s[o(0x1338)];
        s[o(0x1334)] = f98;
        if f98 <= s[o(0x1340)] {
            f100 = s[o(0x112c)];
        } else {
            f100 = s[o(0x1134)];
        }
        f86 = (f31 - f100) * f98 + f100 * s[o(0x1340)];
        s[o(0x133c)] = f86;
        f100 = 0.001;
        f98 = EPS;
        if f86 <= s[o(0x1348)] {
            f97 = s[o(0x112c)];
        } else {
            f97 = s[o(0x1134)];
        }
        f97 = (f31 - f97) * f86 + f97 * s[o(0x1348)];
        s[o(0x1344)] = f97;
        f86 = s[o(0x1370)];
        if s[o(0x1370)] <= f98 {
            f86 = f98;
        }
        f86 = (f97).sqrt() * (f31 / f86);
        if f86 <= f100 {
            f86 = f100;
        }
        f97 = logf(f86);
        f86 = 20.0;
        if f93 < f97 {
            f86 = 8_388_608.0;
        }
        f86 = f31 / (f86 * s[o(0x1130)] + f31);
        f97 = f86;
        if f81 <= s[o(0x1350)] {
            f97 = s[o(0x112c)];
        }
        f97 = (f31 - f97) * f81 + f97 * s[o(0x1350)];
        s[o(0x134c)] = f97;
        f81 = f86;
        if f97 <= s[o(0x1358)] {
            f81 = s[o(0x112c)];
        }
        f97 = (f31 - f81) * f97 + f81 * s[o(0x1358)];
        s[o(0x1354)] = f97;
        f81 = f86;
        if f97 <= s[o(0x1360)] {
            f81 = s[o(0x112c)];
        }
        f81 = (f31 - f81) * f97 + f81 * s[o(0x1360)];
        s[o(0x135c)] = f81;
        if f81 <= s[o(0x1368)] {
            f86 = s[o(0x112c)];
        }
        f81 = (f31 - f86) * f81 + f86 * s[o(0x1368)];
        s[o(0x1364)] = f81;
        f81 = (f81).sqrt();
        s[o(0x136c)] = f81;
        if f81 <= f100 {
            f81 = f100;
        }
        f81 = logf(f81);
        s[o(0x1374)] = f81;
        f95 = (f62 - f89 * f66) - f95;
        f89 = s[o(0x1378)];
        f95 = f95 * f95;
        if f95 <= s[o(0x1380)] {
            f86 = s[o(0x112c)];
        } else {
            f86 = s[o(0x1134)];
        }
        f86 = (f31 - f86) * f95 + f86 * s[o(0x1380)];
        s[o(0x137c)] = f86;
        if f86 <= s[o(0x1388)] {
            f66 = s[o(0x112c)];
        } else {
            f66 = s[o(0x1134)];
        }
        f86 = (f31 - f66) * f86 + f66 * s[o(0x1388)];
        s[o(0x1384)] = f86;
        if f86 <= s[o(0x1390)] {
            f66 = s[o(0x112c)];
        } else {
            f66 = s[o(0x1134)];
        }
        f86 = (f31 - f66) * f86 + s[o(0x1390)] * f66;
        s[o(0x138c)] = f86;
        if f86 <= s[o(0x1398)] {
            f66 = s[o(0x112c)];
        } else {
            f66 = s[o(0x1134)];
        }
        f66 = (f31 - f66) * f86 + s[o(0x1398)] * f66;
        s[o(0x1394)] = f66;
        f86 = s[o(0x13c0)];
        if s[o(0x13c0)] <= f98 {
            f86 = f98;
        }
        f98 = (f66).sqrt() * (f31 / f86);
        if f98 <= f100 {
            f98 = f100;
        }
        f100 = logf(f98);
        f98 = 20.0;
        if f93 < f100 {
            f98 = 8_388_608.0;
        }
        f98 = f31 / (f98 * s[o(0x1130)] + f31);
        f100 = f98;
        if f95 <= s[o(0x13a0)] {
            f100 = s[o(0x112c)];
        }
        f100 = (f31 - f100) * f95 + s[o(0x13a0)] * f100;
        s[o(0x139c)] = f100;
        f95 = f98;
        if f100 <= s[o(0x13a8)] {
            f95 = s[o(0x112c)];
        }
        f100 = (f31 - f95) * f100 + s[o(0x13a8)] * f95;
        s[o(0x13a4)] = f100;
        f95 = f98;
        if f100 <= s[o(0x13b0)] {
            f95 = s[o(0x112c)];
        }
        f100 = (f31 - f95) * f100 + s[o(0x13b0)] * f95;
        s[o(0x13ac)] = f100;
        f95 = 0.001;
        if f100 <= s[o(0x13b8)] {
            f98 = s[o(0x112c)];
        }
        f98 = (f31 - f98) * f100 + s[o(0x13b8)] * f98;
        s[o(0x13b4)] = f98;
        f98 = (f98).sqrt();
        s[o(0x13bc)] = f98;
        if f98 <= f95 {
            f98 = f95;
        }
        f64 = logf(f98);
        s[o(0x13c4)] = f64;
        f98 = s[o(0x13c8)];
        f100 = s[o(0x13e4)];
        f86 = s[o(0x13f0)];
        f66 = f91 - (s[o(0x13d0)] * f100 + f62 + f86);
        s[o(0x13e0)] = s[o(0x13dc)] * f66 + f100;
        f100 = s[o(0x13d8)] * f66 + f100;
        f82 = f100 * SQRT8;
        s[o(0x13ec)] = s[o(0x13e8)] * f100 + f86;
        f66 = s[o(0x13f8)];
        f97 = s[o(0x1400)];
        f100 = (s[o(0x13cc)] * f100 + f86) - (f66 * s[o(0x13d0)] + f97);
        f86 = s[o(0x13d8)] * f100 + f66;
        s[o(0x13f4)] = s[o(0x13dc)] * f100 + f66;
        f66 = s[o(0x13cc)] * f86 + f97;
        f100 = f66 * f66;
        s[o(0x13fc)] = s[o(0x13e8)] * f86 + f97;
        if f100 <= s[o(0x1408)] {
            f86 = s[o(0x112c)];
        } else {
            f86 = s[o(0x1134)];
        }
        f86 = (f31 - f86) * f100 + s[o(0x1408)] * f86;
        s[o(0x1404)] = f86;
        if f86 <= s[o(0x1410)] {
            f97 = s[o(0x112c)];
        } else {
            f97 = s[o(0x1134)];
        }
        f86 = (f31 - f97) * f86 + s[o(0x1410)] * f97;
        s[o(0x140c)] = f86;
        if f86 <= s[o(0x1418)] {
            f97 = s[o(0x112c)];
        } else {
            f97 = s[o(0x1134)];
        }
        f97 = (f31 - f97) * f86 + s[o(0x1418)] * f97;
        s[o(0x1414)] = f97;
        f86 = EPS;
        if f97 <= s[o(0x1420)] {
            f65 = s[o(0x112c)];
        } else {
            f65 = s[o(0x1134)];
        }
        f65 = (f31 - f65) * f97 + s[o(0x1420)] * f65;
        s[o(0x141c)] = f65;
        f97 = s[o(0x1448)];
        if s[o(0x1448)] <= f86 {
            f97 = f86;
        }
        f97 = (f65).sqrt() * (f31 / f97);
        if f97 <= f95 {
            f97 = f95;
        }
        f67 = logf(f97);
        f33 = 8_388_608.0;
        f65 = 20.0;
        f97 = 20.0;
        if f93 < f67 {
            f97 = 8_388_608.0;
        }
        f97 = f31 / (f97 * s[o(0x1130)] + f31);
        f67 = f97;
        if f100 <= s[o(0x1428)] {
            f67 = s[o(0x112c)];
        }
        f67 = (f31 - f67) * f100 + s[o(0x1428)] * f67;
        s[o(0x1424)] = f67;
        f100 = f97;
        if f67 <= s[o(0x1430)] {
            f100 = s[o(0x112c)];
        }
        f67 = (f31 - f100) * f67 + s[o(0x1430)] * f100;
        s[o(0x142c)] = f67;
        f100 = f97;
        if f67 <= s[o(0x1438)] {
            f100 = s[o(0x112c)];
        }
        f100 = (f31 - f100) * f67 + s[o(0x1438)] * f100;
        s[o(0x1434)] = f100;
        if f100 <= s[o(0x1440)] {
            f97 = s[o(0x112c)];
        }
        f100 = (f31 - f97) * f100 + s[o(0x1440)] * f97;
        s[o(0x143c)] = f100;
        f100 = (f100).sqrt();
        s[o(0x1444)] = f100;
        if f100 <= f95 {
            f100 = f95;
        }
        f100 = logf(f100);
        s[o(0x144c)] = f100;
        f66 = (f91 - (f82 + f62)) - f66;
        f91 = s[o(0x1450)];
        f66 = f66 * f66;
        if f66 <= s[o(0x1458)] {
            f62 = s[o(0x112c)];
        } else {
            f62 = s[o(0x1134)];
        }
        f62 = (f31 - f62) * f66 + s[o(0x1458)] * f62;
        s[o(0x1454)] = f62;
        if f62 <= s[o(0x1460)] {
            f97 = s[o(0x112c)];
        } else {
            f97 = s[o(0x1134)];
        }
        f62 = (f31 - f97) * f62 + s[o(0x1460)] * f97;
        s[o(0x145c)] = f62;
        if f62 <= s[o(0x1468)] {
            f97 = s[o(0x112c)];
        } else {
            f97 = s[o(0x1134)];
        }
        f62 = (f31 - f97) * f62 + s[o(0x1468)] * f97;
        s[o(0x1464)] = f62;
        if f62 <= s[o(0x1470)] {
            f97 = s[o(0x112c)];
        } else {
            f97 = s[o(0x1134)];
        }
        f97 = (f31 - f97) * f62 + s[o(0x1470)] * f97;
        s[o(0x146c)] = f97;
        f62 = s[o(0x1498)];
        if s[o(0x1498)] <= f86 {
            f62 = f86;
        }
        f62 = (f97).sqrt() * (f31 / f62);
        if f62 <= f95 {
            f62 = f95;
        }
        f62 = logf(f62);
        if f93 < f62 {
            f65 = f33;
        }
        f62 = f31 / (f65 * s[o(0x1130)] + f31);
        f93 = f62;
        if f66 <= s[o(0x1478)] {
            f93 = s[o(0x112c)];
        }
        f66 = (f31 - f93) * f66 + s[o(0x1478)] * f93;
        s[o(0x1474)] = f66;
        f93 = f62;
        if f66 <= s[o(0x1480)] {
            f93 = s[o(0x112c)];
        }
        f66 = (f31 - f93) * f66 + s[o(0x1480)] * f93;
        s[o(0x147c)] = f66;
        f93 = f62;
        if f66 <= s[o(0x1488)] {
            f93 = s[o(0x112c)];
        }
        f93 = (f31 - f93) * f66 + s[o(0x1488)] * f93;
        s[o(0x1484)] = f93;
        if f93 <= s[o(0x1490)] {
            f62 = s[o(0x112c)];
        }
        f62 = (f31 - f62) * f93 + s[o(0x1490)] * f62;
        s[o(0x148c)] = f62;
        f62 = (f62).sqrt();
        s[o(0x1494)] = f62;
        if f62 <= f95 {
            f62 = f95;
        }
        f95 = logf(f62);
        s[o(0x149c)] = f95;
        u50 = (u32::from(f99 < f95 - s[o(0x14a0)])
            + u32::from(f99 < f81 - f89)
            + u32::from(b29)
            + u32::from(b28)
            + u32::from(f80 < f63 - f79)
            + u32::from(f80 < f96 - f83)
            + u32::from(f99 < f100 - f91)
            + u32::from(f99 < f64 - f98))
            & u32::from(s[o(0x107c)] <= gi(s, 0x14a8) as f32);
        u44 = 0;
        u41 = u44;
        if u50 == 0 {
            u41 = (gi(s, 0x14a8).wrapping_add(1)) as u32;
        }
        si(s, 0x14a4, u41 as i32);
        u41 = gi(s, 0x14bc) as u32;
        b28 = (gi(s, 0x14b4) as f32) < s[o(0x14ac)];
        u51 = u44;
        if b28 && (u50 as i32) < (u41 as i32) {
            u51 = (gi(s, 0x14b4).wrapping_add(1)) as u32;
        }
        si(s, 0x14b0, u51 as i32);
        u51 = u50;
        if b28 && (u50 as i32) < (u41 as i32) {
            u51 = u41;
        }
        si(s, 0x14b8, u51 as i32);
        f83 = s[o(0x14c0)] * s[o(0x14c8)] + u51 as i32 as f32;
        if f31 <= f83 {
            f83 = f31;
        }
        s[o(0x14c4)] = f83;
        s[o(0x14cc)] = f83;
        s[o(0x14d4)] = f2;
        f91 = f2 - s[o(0x14d8)];
        s[o(0x14e0)] = f3;
        f83 = f3 - s[o(0x14e4)];
        u41 = trunc(f32::from(u8::from(0.0 < f83)) * f83);
        s[o(0x14ec)] = f7;
        f89 = f7 - s[o(0x14f0)];
        f83 = s[o(0x1074)];
        u51 = trunc(f32::from(u8::from(0.0 < f91)) * f91) | u41;
        f91 = s[o(0x1078)];
        u41 = trunc(f32::from(u8::from(0.0 < f89)) * f89) | u41;
        f89 = (u41 | u51 | u50) as i32 as f32 * f91 + f83 * s[o(0x14f8)];
        s[o(0x14f4)] = f89;
        f89 = f83 * s[o(0x1500)] + f89 * f91;
        s[o(0x14fc)] = f89;
        f89 = f91 * f89 + f83 * s[o(0x1508)];
        s[o(0x1504)] = f89;
        f83 = f91 * f89 + f83 * s[o(0x1510)];
        s[o(0x150c)] = f83;
        s[o(0x1514)] = f83;

        // The onsets' period, and loop A's length.
        let n = s[o(0x1058)];
        let mut period = s[o(0x14)] * f83;
        if period <= EPS {
            period = EPS;
        }
        s[o(0x1518)] = 1.0 / period;
        let mut near = (k.min_length - s[o(0x1518)]).abs();
        if 2.0 <= near {
            near = 2.0;
        }
        s[o(0x151c)] = near;
        let len_a = s[o(0x14)] * if k.adaptive { near } else { k.min_length };
        s[o(0x1520)] = fmodf(fmodf(f61 - len_a, n) + n, n);
        let len_a = len_a - f31;

        // Loop A: does it take a new loop on this frame?
        let mut u40 = if gi(s, 0x1528) as f32 >= len_a {
            u50
        } else {
            0
        };
        si(
            s,
            0x1524,
            if u40 == 0 {
                gi(s, 0x1528).wrapping_add(1)
            } else {
                0
            },
        );
        let freeze_a = if u40 == 0 { s[o(0x1534)] } else { k.freeze };
        s[o(0x1530)] = freeze_a;
        let beat = k.tempo_multiplier * input[4];
        let beat = beat - floorf(beat);
        s[o(0x153c)] = beat;
        s[o(0x1540)] = beat;
        let tempo = if beat - s[o(0x1544)] < 0.0 {
            k.bouncing
        } else {
            0
        };
        let u36 = u32::from(gi(s, 0x154c) as f32 >= len_a) & tempo;
        si(
            s,
            0x1548,
            if u36 == 0 {
                gi(s, 0x154c).wrapping_add(1)
            } else {
                0
            },
        );
        let lcg = |x: i32| x.wrapping_mul(0x41c6_4e6d).wrapping_sub(0x2c24_19bb);
        let r1 = lcg(gi(s, 0x1558));
        let r2 = lcg(r1);
        let r3 = lcg(r2);
        let r4 = lcg(r3);
        let r5 = lcg(r4);
        let r6 = lcg(r5);
        let r7 = lcg(r6);
        let r8 = lcg(r7);
        si(s, 0x1554, r8);
        let chance = |r: i32| r as f32 * TWO_M32 + 0.5;
        let turn = fmodf(f61, k.span);
        let keep_turn = u32::from(k.keep_share > turn);
        let deterministic = k.deterministic != 0;
        let keep_a = if deterministic {
            keep_turn
        } else {
            u32::from(chance(r2) > k.ignore)
        };
        if freeze_a as i32 != 0 {
            u40 = 0;
        }
        let take_a = (u40 | u36 | u51) & keep_a;
        let (start_a, end_a) = if take_a != 0 {
            (s[o(0x1570)], f61)
        } else {
            (s[o(0x1568)], s[o(0x1570)])
        };
        s[o(0x1564)] = start_a;
        s[o(0x156c)] = end_a;
        let held = gi(s, 0x157c);
        let i42 = if held == 0 {
            gi(s, 0x1584).wrapping_add(1)
        } else {
            0
        };
        let mut length = if take_a != 0 {
            k.rate * k.min_length
        } else if held != 0 {
            s[o(0x158c)] * k.bounce
        } else {
            s[o(0x158c)]
        };
        if !(length < n) {
            length = n;
        }
        si(s, 0x1578, i32::from(i42 as f32 >= length));
        si(s, 0x1580, i42);
        s[o(0x1588)] = length;
        if k.bouncing == 0 {
            length = fmodf(fmodf(end_a - start_a, n) + n, n);
        }
        if !(length > 48.0) {
            length = 48.0;
        }
        s[o(0x1590)] = length;
        s[o(0x1598)] = fmodf((f31 - take_a as f32) * s[o(0x159c)] + f31, length);
        let reverse_turn = u32::from(k.span * k.reverse > turn);
        let octave_turn = k.span * k.octave > turn;
        let up_turn = k.span * k.octave_up > turn;
        let (reverse_a, speed_a) = if take_a != 0 {
            let reverse = if deterministic {
                reverse_turn
            } else {
                u32::from(k.reverse > chance(r8))
            };
            let octave = if deterministic {
                octave_turn
            } else {
                k.octave > chance(r6)
            };
            let speed = if octave {
                let up = if deterministic {
                    up_turn
                } else {
                    k.octave_up > chance(r4)
                };
                if up { 2.0 } else { 0.5 }
            } else {
                1.0
            };
            (reverse, speed)
        } else {
            (gi(s, 0x15a8) as u32, s[o(0x15b8)])
        };
        si(s, 0x15a4, reverse_a as i32);
        s[o(0x15b4)] = speed_a;
        let phase_a = s[o(0x1598)];
        let step = speed_a * phase_a;
        let at = if reverse_a != 0 {
            fmodf(end_a - step, n) + n
        } else {
            step + start_a
        };
        let read_a = fmodf(at, n);
        s[o(0x15bc)] = read_a;
        let jump = u32::from((f61 - read_a).abs() < 2.0)
            | u32::from(length != s[o(0x1594)] || phase_a - s[o(0x159c)] < 0.0);
        s[o(0x15c0)] = (f31 - jump as f32) * s[o(0x15c4)] + f31;
        let quiet = f32::from(u8::from(k.ahead >= s[o(0x15c0)]));
        let (p, q) = (k.fade_pole, k.fade_rest);
        s[o(0x15c8)] = (f31 - quiet) * q + p * s[o(0x15cc)];
        s[o(0x15d0)] = p * s[o(0x15d4)] + s[o(0x15c8)] * q;
        s[o(0x15d8)] = p * s[o(0x15dc)] + s[o(0x15d0)] * q;
        s[o(0x15e0)] = p * s[o(0x15e4)] + s[o(0x15d8)] * q;
        let fade_a = s[o(0x15e0)];
        let level = s[o(0x15f0)];
        s[o(0x15ec)] = (k.level_a - level) * s[o(0x1048)] + level;
        let gain_a = ((k.level_a - level) * s[o(0x1044)] + level) * fade_a;

        // The pan between the loops: the spread, or a sine turning.
        let turned = if gi(s, 0xc) == 1 {
            s[o(0x1600)] - k.spread_step
        } else {
            0.0
        };
        let turned = turned - floorf(turned);
        s[o(0x15fc)] = turned;
        let wobble = if k.spread < 0.0 {
            sines()[((turned * 65536.0) as i32).clamp(0, 65535) as usize]
        } else {
            f31
        };
        let pan = s[o(0x1608)];
        s[o(0x1604)] = (wobble - pan) * s[o(0x1048)] + pan;
        let pan = (((wobble - pan) * s[o(0x1044)] + pan) * k.spread_gain + f31) * 0.5;
        s[o(0x160c)] = pan;

        // Both loops record; loop A plays.
        let right = input[1];
        s[o(0xea7614) + (iota as u32 & 0x3ff) as usize] = right;
        let mid = (right + f84) * std::f32::consts::FRAC_1_SQRT_2;
        if k.stereo == 0 {
            f84 = mid;
        }
        let near_b = near * k.ratio_inverse;
        let write = f61 as i32 as usize;
        if let Some(slot) = self.a.get_mut(write) {
            *slot = f84;
        }
        let played = self.a.get(read_a as i32 as usize).copied().unwrap_or(0.0);
        s[o(0xea8614) + (iota as u32 & 0x3ff) as usize] = played;
        let loop_a = s[o(0xea8614) + (iota.wrapping_sub(k.ahead_frames) as u32 & 0x3ff) as usize];
        s[o(0xea9618)] = near_b;
        let len_b = if k.adaptive { near_b } else { k.min_length_b } * s[o(0x14)];
        s[o(0xea961c)] = fmodf(fmodf(f61 - len_b, n) + n, n);
        let len_b = len_b - f31;

        // Loop B, as loop A.
        let mut v40 = if gi(s, 0xea9624) as f32 >= len_b {
            u50
        } else {
            0
        };
        si(
            s,
            0xea9620,
            if v40 == 0 {
                gi(s, 0xea9624).wrapping_add(1)
            } else {
                0
            },
        );
        let freeze_b = if v40 == 0 { s[o(0xea962c)] } else { k.freeze };
        s[o(0xea9628)] = freeze_b;
        let v44 = u32::from(gi(s, 0xea9634) as f32 >= len_b) & tempo;
        si(
            s,
            0xea9630,
            if v44 == 0 {
                gi(s, 0xea9634).wrapping_add(1)
            } else {
                0
            },
        );
        let keep_b = if deterministic {
            keep_turn
        } else {
            u32::from(chance(r1) > k.ignore)
        };
        if freeze_b as i32 != 0 {
            v40 = 0;
        }
        let take_b = (v40 | v44 | u41) & keep_b;
        let (start_b, end_b) = if take_b != 0 {
            (s[o(0xea9644)], f61)
        } else {
            (s[o(0xea963c)], s[o(0xea9644)])
        };
        s[o(0xea9638)] = start_b;
        s[o(0xea9640)] = end_b;
        let held = gi(s, 0xea964c);
        let j = if held == 0 {
            gi(s, 0xea9654).wrapping_add(1)
        } else {
            0
        };
        let mut length = if take_b != 0 {
            k.rate * k.min_length_b
        } else if held != 0 {
            s[o(0xea965c)] * k.bounce
        } else {
            s[o(0xea965c)]
        };
        if !(length < n) {
            length = n;
        }
        si(s, 0xea9648, i32::from(j as f32 >= length));
        si(s, 0xea9650, j);
        s[o(0xea9658)] = length;
        if k.bouncing == 0 {
            length = fmodf(fmodf(end_b - start_b, n) + n, n);
        }
        if !(length > 48.0) {
            length = 48.0;
        }
        s[o(0xea9660)] = length;
        s[o(0xea9668)] = fmodf((f31 - take_b as f32) * s[o(0xea966c)] + f31, length);
        let (reverse_b, speed_b) = if take_b != 0 {
            let reverse = if deterministic {
                reverse_turn
            } else {
                u32::from(k.reverse > chance(r7))
            };
            let octave = if deterministic {
                octave_turn
            } else {
                k.octave > chance(r5)
            };
            let speed = if octave {
                let up = if deterministic {
                    up_turn
                } else {
                    k.octave_up > chance(r3)
                };
                if up { 2.0 } else { 0.5 }
            } else {
                1.0
            };
            (reverse, speed)
        } else {
            (gi(s, 0xea9674) as u32, s[o(0xea967c)])
        };
        si(s, 0xea9670, reverse_b as i32);
        s[o(0xea9678)] = speed_b;
        let phase_b = s[o(0xea9668)];
        let step = speed_b * phase_b;
        let at = if reverse_b != 0 {
            fmodf(end_b - step, n) + n
        } else {
            step + start_b
        };
        let read_b = fmodf(at, n);
        s[o(0xea9680)] = read_b;
        let jump = u32::from((f61 - read_b).abs() < 2.0)
            | u32::from(length != s[o(0xea9664)] || phase_b - s[o(0xea966c)] < 0.0);
        s[o(0xea9684)] = (f31 - jump as f32) * s[o(0xea9688)] + f31;
        let quiet = f32::from(u8::from(k.ahead >= s[o(0xea9684)]));
        s[o(0xea968c)] = (f31 - quiet) * q + p * s[o(0xea9690)];
        s[o(0xea9694)] = p * s[o(0xea9698)] + s[o(0xea968c)] * q;
        s[o(0xea969c)] = p * s[o(0xea96a0)] + s[o(0xea9694)] * q;
        s[o(0xea96a4)] = p * s[o(0xea96a8)] + s[o(0xea969c)] * q;
        let fade_b = s[o(0xea96a4)];
        let level = s[o(0xea96b4)];
        s[o(0xea96b0)] = (k.level_b - level) * s[o(0x1048)] + level;
        let gain_b = ((k.level_b - level) * s[o(0x1044)] + level) * fade_b;
        let recorded = if k.stereo == 0 { mid } else { right };
        if let Some(slot) = self.b.get_mut(write) {
            *slot = recorded;
        }
        let played = self.b.get(read_b as i32 as usize).copied().unwrap_or(0.0);
        self.ring[(iota as u32 & 0x3ff) as usize] = played;
        let loop_b = self.ring[(iota.wrapping_sub(k.ahead_frames) as u32 & 0x3ff) as usize];

        // The mix.
        let dry_r = s[o(0xea7614) + (iota.wrapping_sub(k.ahead_frames) as u32 & 0x3ff) as usize];
        let left = (pan * gain_a * loop_a + f60 * f17 + (f31 - pan) * gain_b * loop_b) * f85
            + f17 * (f31 - f85);
        let right = (loop_a * gain_a * (f31 - pan) + dry_r * f60 + gain_b * pan * loop_b) * f85
            + dry_r * (f31 - f85);
        let out = [left, right, f85 * fade_a * loop_a, f85 * fade_b * loop_b];

        // The frame ends: the counter moves on and every recursion keeps
        // what it has for the next.
        si(s, 0x34, iota.wrapping_add(1));
        for &(before, now) in CARRIED {
            s[o(before)] = s[o(now)];
        }
        s[o(0x30)] = s[o(0x2c)];
        out
    }
}

/// The lengths `SetParam` (`0x18018fc50`) reads a tempo ratio's index
/// in (`0x1803dd5b0`), in bars: 1/1 to 1/32, the dotted five, the
/// triplet five, the quintuplet five, and 0 for Off.
const RATIOS: [f32; 22] = [
    1.0,
    0.5,
    0.25,
    0.125,
    0.0625,
    0.03125,
    1.5,
    0.75,
    0.375,
    0.1875,
    0.09375,
    0.333_333_34,
    0.166_666_67,
    0.083_333_336,
    0.041_666_664,
    0.020_833_332,
    0.2,
    0.1,
    0.05,
    0.025,
    0.0125,
    0.0,
];

/// The most frames `render` takes at once (`0x1d50738`).
const MAX_BLOCK: usize = 0x4000;

/// `TransientLooperImpl` (vtable `0x1803e0100`): the Faust program
/// behind the interface Transporter's shell calls, which turns the
/// shell's parameters into zones, flushes the loops and fades the output.
pub(crate) struct TransientLooper {
    looper: Looper,
    /// The rate the shell set (`0x1d50740`), -1 before.
    rate: f32,
    /// The main output's fade per side (`0x1d5075c`) and its pole.
    fade: [f32; 2],
    fade_pole: f32,
    /// A flush is waiting for the fade (`0x1d50768`), and Bypass.
    flushing: bool,
    bypassed: bool,
    /// The last values of the three update buttons (`0x1d5076c`..).
    updates: [f32; 3],
    /// The loop mode as set (`0x1d50778`): 3 is Tempo.
    mode: u64,
    /// Bouncing rate, tempo and minimum length as last set
    /// (`0x1d50780`..`0x1d50788`).
    bounce: f32,
    tempo: f32,
    min_length: f32,
    /// The tempo ratio's length and trigger indexes (`0x1d50790`,
    /// `0x1d50798`).
    length: u64,
    trigger: u64,
    lookahead: bool,
    /// The five inputs and four outputs of one block.
    ins: [Vec<f32>; 5],
    outs: [Vec<f32>; 4],
}

impl TransientLooper {
    /// As `TransientLooper_CreateInstance` (`0x1801abe00`) makes it:
    /// the program at 44.1 kHz with its defaults.
    pub(crate) fn new() -> Self {
        TransientLooper {
            looper: Looper::new(44_100),
            rate: -1.0,
            fade: [0.0; 2],
            fade_pole: 0.0,
            flushing: false,
            bypassed: false,
            updates: [0.0; 3],
            mode: 0,
            bounce: 0.0,
            tempo: 60.0,
            min_length: 0.0,
            length: 0,
            trigger: 0,
            lookahead: false,
            ins: std::array::from_fn(|_| vec![0.0; MAX_BLOCK]),
            outs: std::array::from_fn(|_| vec![0.0; MAX_BLOCK]),
        }
    }

    /// Set the rate (`0x1801ac0e0`): the constants anew, both loops
    /// emptied, the program cleared, the fade's pole, then lookahead on,
    /// both sides as the source and stereo. This allocates.
    pub(crate) fn set_rate(&mut self, rate: f32) {
        self.looper.set_rate(rate as i32);
        self.looper.clear();
        self.rate = rate;
        let at = if rate.is_nan() || rate < 1.0 {
            1.0
        } else {
            rate
        };
        self.fade_pole = msvc::exp((f64::from(1.0 / at) * -94.247_779_607_693_79) as f32);
        self.set_param(23, 1.0);
        self.set_param(24, 2.0);
        self.set_param(25, 1.0);
    }

    /// The frames the output is late (`0x1801ac210`): the 5 ms lookahead
    /// when it is on.
    pub(crate) fn latency(&self) -> usize {
        if self.lookahead {
            (self.rate * 0.005) as usize
        } else {
            0
        }
    }

    /// The most [`TransientLooper::latency`] comes to at the rate.
    pub(crate) fn max_latency(&self) -> usize {
        (self.rate.max(0.0) * 0.005) as usize
    }

    /// `FPD_Flush` (`0x1801ab530`): the program's `instanceClear`. The
    /// loops keep their sound.
    pub(crate) fn clear(&mut self) {
        self.looper.clear();
    }

    /// `SetParam` (`0x18018fc50`), with the values the shell sends.
    pub(crate) fn set_param(&mut self, index: usize, value: f32) {
        let l = &mut self.looper;
        let rounded = |v: f32| v.round() as u64;
        let tempo_length = |tempo: f32, length: u64| {
            240.0 / tempo * RATIOS.get(length as usize).copied().unwrap_or(0.0)
        };
        let up = |last: &mut f32| {
            let edge = value > *last;
            *last = value;
            f32::from(u8::from(edge))
        };
        match index {
            0 => l.set_zone(zone::INPUT_LEVEL, value),
            1 => l.set_zone(zone::LOOP_LEVEL_A, value),
            2 => l.set_zone(zone::LOOP_LEVEL_B, value),
            3 => l.set_zone(zone::REVERSE, value),
            4 => l.set_zone(zone::OCTAVE, value),
            5 => l.set_zone(zone::OCTAVE_UP, value),
            6 => l.set_zone(zone::SPREAD, value),
            7 => {
                self.min_length = value;
                if self.mode != 3 {
                    l.set_zone(zone::MIN_LENGTH_A, value);
                }
            }
            8 => l.set_zone(zone::AB_RATIO, value),
            9 => l.set_zone(zone::SENSITIVITY, value),
            10 => {
                self.bounce = value;
                if self.mode != 3 {
                    l.set_zone(zone::BOUNCING_RATE, value);
                }
            }
            11 => l.set_zone(zone::FREEZE, value),
            12 => {
                if !self.flushing {
                    self.flushing = value > 0.5;
                }
            }
            13 => {
                self.mode = rounded(value);
                if self.mode == 3 {
                    l.set_zone(zone::LOOP_MODE, 1.0);
                    l.set_zone(zone::BOUNCING_RATE, 0.0);
                    l.set_zone(zone::MIN_LENGTH_A, tempo_length(self.tempo, self.length));
                } else {
                    l.set_zone(zone::LOOP_MODE, self.mode as f32);
                    l.set_zone(zone::BOUNCING_RATE, self.bounce);
                    l.set_zone(zone::MIN_LENGTH_A, self.min_length);
                }
            }
            14 => {
                self.bypassed = value > 0.5;
                l.set_zone(zone::BYPASS, value);
            }
            15 => l.set_zone(zone::TRIGGER_A, up(&mut self.updates[0])),
            16 => l.set_zone(zone::TRIGGER_B, up(&mut self.updates[1])),
            17 => l.set_zone(zone::TRIGGER_AB, up(&mut self.updates[2])),
            18 => l.set_zone(zone::DETERMINISTIC, f32::from(u8::from(value > 0.5))),
            19 => {
                self.length = rounded(value);
                if self.mode == 3 {
                    l.set_zone(zone::MIN_LENGTH_A, tempo_length(self.tempo, self.length));
                }
            }
            20 => {
                self.trigger = rounded(value);
                let ratio = RATIOS.get(self.trigger as usize).copied().unwrap_or(0.0);
                let multiplier = if ratio == 0.0 { 0.0 } else { 0.25 / ratio };
                l.set_zone(zone::TEMPO_MULTIPLIER, multiplier);
            }
            21 => l.set_zone(zone::IGNORE, value),
            22 => {
                self.tempo = value;
                if self.mode == 3 {
                    l.set_zone(zone::MIN_LENGTH_A, tempo_length(value, self.length));
                }
            }
            23 => {
                self.lookahead = value > 0.5;
                l.set_zone(zone::LOOKAHEAD, f32::from(u8::from(self.lookahead)));
            }
            // The source falls through to Mono/Stereo in FL's switch, so
            // setting it sets stereo too, by the same value.
            24 | 25 => {
                if index == 24 {
                    let source = match rounded(value) {
                        0 => 0.0,
                        1 => 1.0,
                        _ => 2.0,
                    };
                    l.set_zone(zone::SOURCE, source);
                }
                l.set_zone(zone::STEREO, f32::from(u8::from(value > 0.5)));
            }
            _ => {}
        }
    }

    /// `render` (`0x180192070`): `left` and `right` in place, the
    /// transient source `side`, the beat (the place in a 4-beat bar,
    /// as the shell fills it), and the loops' second output into `aux`.
    /// Blocks over [`MAX_BLOCK`] are taken in parts.
    pub(crate) fn render(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        side: Option<(&[f32], &[f32])>,
        beat: f32,
        aux: (&mut [f32], &mut [f32]),
    ) {
        let len = left.len().min(right.len());
        let mut at = 0;
        while at < len {
            let n = (len - at).min(MAX_BLOCK);
            let (l, r) = (&mut left[at..at + n], &mut right[at..at + n]);
            self.ins[0][..n].copy_from_slice(l);
            self.ins[1][..n].copy_from_slice(r);
            match side {
                Some((sl, sr)) => {
                    self.ins[2][..n].copy_from_slice(&sl[at..at + n]);
                    self.ins[3][..n].copy_from_slice(&sr[at..at + n]);
                }
                None => {
                    self.ins[2][..n].copy_from_slice(l);
                    self.ins[3][..n].copy_from_slice(r);
                }
            }
            self.ins[4][..n].fill(beat);
            self.block(n);
            let target = f32::from(u8::from(!self.flushing || self.bypassed));
            for (k, out) in [l, r].into_iter().enumerate() {
                for (sample, wet) in out.iter_mut().zip(&self.outs[k][..n]) {
                    let fade = (self.fade[k] - target) * self.fade_pole + target;
                    self.fade[k] = fade;
                    *sample = fade * fade * wet;
                }
            }
            aux.0[at..at + n].copy_from_slice(&self.outs[2][..n]);
            aux.1[at..at + n].copy_from_slice(&self.outs[3][..n]);
            at += n;
        }
    }

    /// One block of the program, with the flush: once the fade has gone
    /// under 1e-7 both loops are emptied and the program runs a block
    /// with Flush down, which brings its write point back to 0.
    fn block(&mut self, n: usize) {
        if !self.flushing {
            self.looper.set_zone(zone::FLUSH, 0.0);
        } else if 1.0e-7 > self.fade[0] {
            self.looper.empty_loops();
            self.looper.set_zone(zone::FLUSH, 1.0);
            self.flushing = false;
        }
        let [i0, i1, i2, i3, i4] = &self.ins;
        let [o0, o1, o2, o3] = &mut self.outs;
        self.looper.compute(
            [&i0[..n], &i1[..n], &i2[..n], &i3[..n], &i4[..n]],
            [&mut o0[..n], &mut o1[..n], &mut o2[..n], &mut o3[..n]],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emulator's test input (`gen.py` beside the doc's notes): a
    /// decaying noise burst every quarter second over two slow sines, the
    /// same on the transient source, and a beat that runs through a bar
    /// every 2 seconds. Five channels.
    fn input(len: usize) -> [Vec<f32>; 5] {
        let mut seed = 12345u32;
        let mut chans: [Vec<f32>; 5] = Default::default();
        for i in 0..len {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let noise = f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
            let env = (-((i % 11025) as f64) / 800.0).exp();
            let l = (0.5 * env * noise + 0.1 * (i as f64 * 0.05).sin()) as f32;
            let r = (0.4 * env * noise + 0.1 * (i as f64 * 0.031).sin()) as f32;
            let beat = ((i as f64 / 22050.0) % 4.0 / 4.0) as f32;
            for (k, v) in [l, r, l, r, beat].into_iter().enumerate() {
                chans[k].push(v);
            }
        }
        chans
    }

    fn fnv(outs: &[Vec<f32>; 4]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for i in 0..outs[0].len() {
            for out in outs {
                hash ^= u64::from(out[i].to_bits());
                hash = hash.wrapping_mul(0x100_0000_01b3);
            }
        }
        hash
    }

    #[test]
    fn the_program_is_fl_s_to_the_bit() {
        // `transient_looper_emu.py` at 44.1 kHz on two seconds of the
        // input, with the zones set: the hash of all four outputs, and
        // a frame of them, at the block length given.
        let len = 88_200;
        let chans = input(len);
        let base = [
            (zone::LOOKAHEAD, 1.0),
            (zone::SOURCE, 2.0),
            (zone::STEREO, 1.0),
        ];
        type Case<'a> = (&'a [(usize, f32)], usize, u64, [f32; 4]);
        let cases: [Case; 5] = [
            (
                &[
                    (zone::REVERSE, 0.5),
                    (zone::OCTAVE, 0.5),
                    (zone::OCTAVE_UP, 0.5),
                    (zone::IGNORE, 0.3),
                    (zone::MIN_LENGTH_A, 0.2),
                    (zone::AB_RATIO, 2.0),
                    (zone::SENSITIVITY, 0.8),
                ],
                256,
                0xc119_cd80_887b_fc61,
                [0.102_085_404, 0.079_313_755, 0.157_047_82, -0.084_795_22],
            ),
            (
                &[
                    (zone::SOURCE, 0.0),
                    (zone::REVERSE, 0.5),
                    (zone::OCTAVE, 0.5),
                    (zone::OCTAVE_UP, 0.5),
                    (zone::IGNORE, 0.3),
                    (zone::DETERMINISTIC, 1.0),
                    (zone::MIN_LENGTH_A, 0.1),
                ],
                300,
                0x04de_299b_7248_ec05,
                [0.043_614_655, 0.020_842_988, -0.005_161_724, -0.039_528_42],
            ),
            (
                &[
                    (zone::LOOP_MODE, 2.0),
                    (zone::SPREAD, 0.7),
                    (zone::SENSITIVITY, 1.0),
                    (zone::AB_RATIO, 5.5),
                ],
                128,
                0xaf8b_0a21_6e8b_3820,
                [
                    0.007_423_476_3,
                    -0.036_972_553,
                    -0.053_902_622,
                    -0.084_795_22,
                ],
            ),
            (
                &[
                    (zone::LOOP_MODE, 1.0),
                    (zone::TEMPO_MULTIPLIER, 1.0),
                    (zone::BOUNCING_RATE, 0.0),
                    (zone::MIN_LENGTH_A, 0.25),
                ],
                441,
                0x8596_a51b_69f7_25f0,
                [0.123_379_92, 0.100_608_274, 0.064_495_85, 0.050_346_23],
            ),
            (
                &[
                    (zone::INPUT_LEVEL, -0.8),
                    (zone::LOOP_LEVEL_A, -6.5),
                    (zone::LOOP_LEVEL_B, -6.0),
                    (zone::REVERSE, 0.4969),
                    (zone::LOOP_MODE, 1.0),
                    (zone::MIN_LENGTH_A, 1.98),
                    (zone::AB_RATIO, 4.0),
                    (zone::SENSITIVITY, 1.0),
                    (zone::BOUNCING_RATE, 0.0),
                    (zone::DETERMINISTIC, 1.0),
                    (zone::TEMPO_MULTIPLIER, 0.0),
                ],
                512,
                0x2d1c_288c_e87e_4c49,
                [0.060_168_777, 0.039_400_71, 0.0, 5.118_072_3e-5],
            ),
        ];
        for (zones, block, hash, last) in cases {
            let mut looper = Looper::new(44_100);
            for &(at, value) in base.iter().chain(zones) {
                looper.set_zone(at, value);
            }
            let mut outs: [Vec<f32>; 4] = std::array::from_fn(|_| vec![0.0; len]);
            let mut at = 0;
            while at < len {
                let n = block.min(len - at);
                let ins = std::array::from_fn(|k| &chans[k][at..at + n]);
                let [a, b, c, d] = &mut outs;
                looper.compute(
                    ins,
                    [
                        &mut a[at..at + n],
                        &mut b[at..at + n],
                        &mut c[at..at + n],
                        &mut d[at..at + n],
                    ],
                );
                at += n;
            }
            assert_eq!(outs.each_ref().map(|out| out[len - 1]), last, "{zones:?}");
            assert_eq!(fnv(&outs), hash, "{zones:?}");
        }
    }

    #[test]
    fn a_new_program_has_its_controls_defaults() {
        let looper = Looper::new(48_000);
        assert_eq!(looper.zone(zone::MIN_LENGTH_A), 0.5);
        assert_eq!(looper.zone(zone::SENSITIVITY), 0.5);
        assert_eq!(looper.zone(zone::AB_RATIO), 3.0);
        assert_eq!(looper.zone(zone::TEMPO_MULTIPLIER), 21.0);
        assert_eq!(looper.zone(zone::LOOKAHEAD), 0.0);
        // 20 seconds of each loop, the lookahead 5 ms.
        assert_eq!(looper.a.len(), 960_002);
        assert_eq!(looper.zone(0x103c), 240.0);
    }

    #[test]
    fn the_wrapper_maps_tempo_mode_and_fades_a_flush_out() {
        let mut engine = TransientLooper::new();
        engine.set_rate(44_100.0);
        assert_eq!(engine.latency(), 220);
        assert_eq!(engine.looper.zone(zone::STEREO), 1.0);
        engine.set_param(7, 0.3);
        engine.set_param(22, 120.0);
        engine.set_param(19, 2.0);
        engine.set_param(13, 3.0);
        // Tempo mode is Bouncing with no bounce, a quarter bar at 120 BPM.
        assert_eq!(engine.looper.zone(zone::LOOP_MODE), 1.0);
        assert_eq!(engine.looper.zone(zone::MIN_LENGTH_A), 0.5);
        engine.set_param(13, 0.0);
        assert_eq!(engine.looper.zone(zone::MIN_LENGTH_A), 0.3);
        engine.set_param(20, 2.0);
        assert_eq!(engine.looper.zone(zone::TEMPO_MULTIPLIER), 1.0);
        engine.set_param(20, 21.0);
        assert_eq!(engine.looper.zone(zone::TEMPO_MULTIPLIER), 0.0);
        // The update buttons trigger on the way up only.
        engine.set_param(15, 1.0);
        assert_eq!(engine.looper.zone(zone::TRIGGER_A), 1.0);
        engine.set_param(15, 1.0);
        assert_eq!(engine.looper.zone(zone::TRIGGER_A), 0.0);
        // The source also sets stereo, by its own value.
        engine.set_param(24, 0.0);
        assert_eq!(engine.looper.zone(zone::STEREO), 0.0);
        engine.set_param(25, 1.0);

        // A held tone comes through, fading in from silence.
        let n = 4096;
        let tone: Vec<f32> = (0..n).map(|i| (i as f32 * 0.03).sin() * 0.5).collect();
        let (mut l, mut r) = (tone.clone(), tone.clone());
        let (mut al, mut ar) = (vec![0.0; n], vec![0.0; n]);
        engine.render(&mut l, &mut r, None, 0.0, (&mut al, &mut ar));
        assert_eq!(l[0], 0.0);
        assert!(l[2000..].iter().any(|v| v.abs() > 0.3));
        // A flush fades it out, empties the loops and fades back in.
        engine.set_param(12, 1.0);
        let mut quiet = false;
        for _ in 0..8 {
            let (mut l, mut r) = (tone.clone(), tone.clone());
            engine.render(&mut l, &mut r, None, 0.0, (&mut al, &mut ar));
            quiet |= l.iter().all(|v| v.abs() < 1e-3);
        }
        assert!(quiet);
        assert!(!engine.flushing);
        assert!(engine.fade[0] > 0.9);
    }

    /// Run a file of five interleaved channels through the looper, as the
    /// emulator does: `TL_IN`, `TL_OUT`, `TL_ZONES` (offset=value,...),
    /// `TL_BLOCK`, `TL_DUMP`.
    #[test]
    #[ignore]
    fn run_file() {
        let input = std::env::var("TL_IN").unwrap();
        let out = std::env::var("TL_OUT").unwrap();
        let block: usize = std::env::var("TL_BLOCK").map_or(512, |b| b.parse().unwrap());
        let dump: i64 = std::env::var("TL_DUMP").map_or(-1, |b| b.parse().unwrap());
        let rate: i32 = std::env::var("TL_RATE").map_or(44100, |b| b.parse().unwrap());
        let mut looper = Looper::new(rate);
        if let Ok(zones) = std::env::var("TL_ZONES") {
            for pair in zones.split(',') {
                let (off, value) = pair.split_once('=').unwrap();
                let off = usize::from_str_radix(off.trim_start_matches("0x"), 16).unwrap();
                looper.set_zone(off, value.parse().unwrap());
            }
        }
        let bytes = std::fs::read(input).unwrap();
        let xs: Vec<f32> = bytes
            .chunks(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let n = xs.len() / 5;
        let chans: Vec<Vec<f32>> = (0..5)
            .map(|k| xs[k..].iter().step_by(5).copied().collect())
            .collect();
        let mut outs = vec![vec![0.0f32; n]; 4];
        let mut at = 0;
        while at < n {
            if at as i64 == dump {
                let raw: Vec<u8> = looper.s.iter().flat_map(|v| v.to_le_bytes()).collect();
                std::fs::write(format!("{out}.state"), raw).unwrap();
            }
            let m = block.min(n - at);
            let [a, b, c, d] = &mut outs[..] else {
                unreachable!()
            };
            looper.compute(
                [
                    &chans[0][at..at + m],
                    &chans[1][at..at + m],
                    &chans[2][at..at + m],
                    &chans[3][at..at + m],
                    &chans[4][at..at + m],
                ],
                [
                    &mut a[at..at + m],
                    &mut b[at..at + m],
                    &mut c[at..at + m],
                    &mut d[at..at + m],
                ],
            );
            at += m;
        }
        let mut raw = Vec::new();
        for i in 0..n {
            for ch in &outs {
                raw.extend(ch[i].to_le_bytes());
            }
        }
        std::fs::write(out, raw).unwrap();
    }
}
