//! Vocodex: a channel vocoder of 5 to 100 bands, as `docs/plugins/vocodex.md`
//! has it.
//!
//! Two signals go in. The modulator is analysed: each band of it is a
//! bandpass, and a follower makes an envelope of what comes out. The
//! carrier is the sound that is heard: the same bands of it, each times its
//! envelope, summed. Beside that sum the plugin passes some of the
//! modulator through a highpass and some of the carrier through a lowpass,
//! can add noise to the carrier, and ends with Soundgoodizer.
//!
//! Each input comes from a number the state saves: 0 is the plugin's own
//! insert, a number above is a sidechain input, the inserts that send to
//! the plugin's insert counted from 1, and the carrier's -1 is no external
//! input at all, for the built-in synthesizer. A number the host hands no
//! block for is the own insert, as in FL. FL's default is the modulator
//! from sidechain 1 and the carrier from the insert itself. With L-R
//! encoding on, the own input's left side is the modulator and its right
//! side the carrier.
//!
//! The state is a version, 13 envelopes, 20 parameters as i32, 7 more i32,
//! 21 switch bytes, the carrier synthesizer's preset name and the keys held
//! on its keyboard.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Wet level | 0 to 2000 | 1000 |
//! | 1 Soundgoodizing level | 0 to 1000 | 150 |
//! | 2 Modulator pass-through level | 0 to 16000 | 12800 |
//! | 3 Modulator pass-through highpass | 0 to 65536 | 55000 |
//! | 4 Carrier pass-through level | 0 to 16000 | 0 |
//! | 5 Carrier pass-through lowpass | 0 to 65536 | 28000 |
//! | 6 Carrier added noise level | 0 to 12800 | 0 |
//! | 7 Carrier added noise highpass | 0 to 65536 | 55000 |
//! | 8 Hold time | 0 to 10000 | 1666 |
//! | 9 Attack time | 0 to 10000 | 1115 |
//! | 10 Attack curve | 0 to 2000 | 0 |
//! | 11 Release time | 0 to 10000 | 3500 |
//! | 12 Release curve | 0 to 2000 | 0 |
//! | 13 Freeze envelope | 0 to 1 | 0 |
//! | 14 Band width | 5 to 1000 | 349 |
//! | 15 Filter flatness | 0 to 100 | 100 |
//! | 16 Modulator band width multiplier | 0 to 4000 | 2000 |
//! | 17 Modulator pitch shift | -1200 to 1200 | 0 |
//! | 18 Modulator unison shift | 0 to 1000 | 250 |
//! | 19 Modulator unison panning | -256 to 256 | 128 |
//!
//! FL runs the plugin in its draft mode while it plays and in full quality
//! when it renders with HQ on. This plays full quality, whatever the
//! state's draft switch says: every band at the rate it is handed, the
//! bands that reach past 11050 Hz at twice that.

use crate::soundgoodizer::Soundgoodizer;
use crate::sytrus::Sytrus;
use crate::{Effect, Fidelity, Generator, NoteOn, Sidechain, StateError, knob_value};
use std::f64::consts::{LN_2, PI};
use std::path::{Path, PathBuf};

/// The plugin's event 201 name.
pub const NAME: &str = "Vocodex";

/// The range of each parameter's control.
const RANGES: [(i32, i32); 20] = [
    (0, 2000),
    (0, 1000),
    (0, 16_000),
    (0, 65_536),
    (0, 16_000),
    (0, 65_536),
    (0, 12_800),
    (0, 65_536),
    (0, 10_000),
    (0, 10_000),
    (0, 2000),
    (0, 10_000),
    (0, 2000),
    (0, 1),
    (5, 1000),
    (0, 100),
    (0, 4000),
    (-1200, 1200),
    (0, 1000),
    (-256, 256),
];

/// What FL's default preset has.
const DEFAULTS: [i32; 20] = [
    1000, 150, 12_800, 55_000, 0, 28_000, 0, 55_000, 1666, 1115, 0, 3500, 0, 0, 349, 100, 2000, 0,
    250, 128,
];

/// The version word FL writes: 3 in the low half for the envelope layout
/// and 8 in the high half, which is the one it reads.
#[cfg(test)]
const SAVED_VERSION: i32 = 0x0008_0003;

/// The switches at `0x18c..0x194` of the plugin object, in the state in
/// this order.
const LR_ENCODING: usize = 0;
const MIN_TIMES: usize = 1;
const CONTOUR_TO_MOD: usize = 2;
const SUBTRACTIVE: usize = 4;
const CONTOUR_TO_NOISE: usize = 5;
const CONTOUR_FROM_CAR: usize = 6;
const CONTOUR_FROM_CONSONANTS: usize = 7;

/// The 13 envelopes, in the order the state has them.
const GAIN_MULTIPLIER: usize = 0;
const PANNING: usize = 1;
const GAIN_OFFSET: usize = 2;
const NOISE_LEVEL: usize = 3;
const PASS_THROUGH: usize = 4;
const HOLD: usize = 5;
const ATTACK: usize = 6;
const RELEASE: usize = 7;
const DISTRIBUTION: usize = 8;
const WIDTH: usize = 9;
const PITCH: usize = 10;
const SATURATION_MIX: usize = 11;
const MAPS: usize = 13;
/// The envelopes that read from -0.5 to 0.5, centred on their middle.
const CENTRED: [usize; 3] = [PANNING, GAIN_OFFSET, PITCH];

/// The note a band's octave counts from: `440 * 2^(-33/12)`, the C at 65.4
/// Hz.
const C2: f64 = 65.406_391_325_149_66;
/// A band whose filters reach past this, two of their widths above the
/// centre, is filtered at twice the rate.
const OVERSAMPLE_ABOVE: f64 = 11_050.0;
/// How far each unison voice's filters move, in units of the unison shift,
/// for 1 to 4 voices beside the first, voice 0 first.
const UNISON: [[f32; 5]; 5] = [
    [0.0; 5],
    [-1.0, 1.0, 0.0, 0.0, 0.0],
    [-1.0, 0.0, 1.0, 0.0, 0.0],
    [-1.0, 1.0 / 3.0, -1.0 / 3.0, 1.0, 0.0],
    [-1.0, 0.5, 0.0, -0.5, 1.0],
];
/// The alpha of each of the two stages of a pass-through filter, over
/// `sin w`.
const PASS_K: [f64; 2] = [0.356_510_361_215_121, 0.943_792_598_854_838];
/// The allpass coefficients of the two branches of the 2x upsampler.
const HALFBAND: [f64; 6] = [
    0.147_377_113_601_046_6,
    0.039_151_597_734_460_045,
    0.482_468_542_769_700_14,
    0.302_646_848_328_493_4,
    0.883_005_025_769_373_1,
    0.674_615_918_546_963_9,
];
/// What the plugin treats as silence.
const QUIET: f32 = 1e-6;
/// The plugin works through a block in pieces of at most this many frames.
const CHUNK: usize = 512;

/// One point of an envelope as the state saves it: its distance from the
/// point before, its height, its tension and four bytes of shape.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Point {
    dx: f64,
    y: f32,
    tension: f32,
    shape: [u8; 4],
}

/// One of the plugin's envelopes: a line over 0 to 1.
#[derive(Debug, Clone, PartialEq)]
struct Map {
    points: Vec<Point>,
    /// The 20 bytes after the points: loop and sustain marks.
    tail: [u8; 20],
}

impl Map {
    /// A flat line at `y`.
    fn flat(y: f32) -> Self {
        Map::line(y, y)
    }

    /// A straight line from `a` at 0 to `b` at 1.
    fn line(a: f32, b: f32) -> Self {
        let point = |dx, y, shape| Point {
            dx,
            y,
            tension: 0.0,
            shape,
        };
        let mut tail = [0xff; 20];
        tail[..4].fill(0);
        Map {
            points: vec![point(0.0, a, [0; 4]), point(1.0, b, [0, 0, 0, 2])],
            tail,
        }
    }

    /// The height at `x`, less `center`. Between two points the line is
    /// straight, which is what FL draws for a point of tension 0.
    fn at(&self, x: f64, center: f64) -> f64 {
        let Some(first) = self.points.first() else {
            return 0.0;
        };
        let mut x0 = first.dx;
        let mut y0 = f64::from(first.y);
        if x <= x0 {
            return y0 - center;
        }
        for point in &self.points[1..] {
            let x1 = x0 + point.dx;
            let y1 = f64::from(point.y);
            if x < x1 {
                let t = (x - x0) / point.dx;
                return y0 + t * (y1 - y0) - center;
            }
            (x0, y0) = (x1, y1);
        }
        y0 - center
    }

    /// Whether every point is joined to the one before by a straight line.
    fn straight(&self) -> bool {
        let curved = |p: &Point| p.tension != 0.0 && p.shape[3] != 2;
        !self.points.iter().skip(1).any(curved)
    }

    /// Read one envelope: its kind, which is 3, its version, the number of
    /// points, the points and the tail.
    fn read(bytes: &mut Reader<'_>) -> Result<Self, StateError> {
        let kind = bytes.i32()?;
        if kind != 3 {
            return Err(bytes.wrong());
        }
        let version = bytes.i32()?;
        if !(0..4).contains(&version) {
            return Err(bytes.wrong());
        }
        let count = bytes.i32()?;
        if !(0..=100_000).contains(&count) {
            return Err(bytes.wrong());
        }
        let mut points = Vec::with_capacity(count as usize);
        let mut last = 0.5f32;
        for _ in 0..count {
            let (dx, y) = if version >= 2 {
                (bytes.f64()?, bytes.f64()? as f32)
            } else {
                (f64::from(bytes.f32()?), bytes.f32()?)
            };
            let y = if y.is_nan() { last } else { y };
            last = y;
            let mut rest = [0u8; 8];
            if version >= 1 {
                rest.copy_from_slice(bytes.take(8)?);
            } else {
                rest[..4].copy_from_slice(bytes.take(4)?);
            }
            let tension = f32::from_le_bytes(rest[..4].try_into().unwrap());
            let shape = rest[4..].try_into().unwrap();
            points.push(Point {
                dx,
                y,
                tension,
                shape,
            });
        }
        let tail = bytes.take(20)?.try_into().unwrap();
        if points.is_empty() {
            points.push(Map::flat(0.0).points[0]);
        }
        Ok(Map { points, tail })
    }

    #[cfg(test)]
    fn write(&self, out: &mut Vec<u8>) {
        out.extend(3i32.to_le_bytes());
        out.extend(3i32.to_le_bytes());
        out.extend((self.points.len() as i32).to_le_bytes());
        for point in &self.points {
            out.extend(point.dx.to_le_bytes());
            out.extend(f64::from(point.y).to_le_bytes());
            out.extend(point.tension.to_le_bytes());
            out.extend(point.shape);
        }
        out.extend(self.tail);
    }
}

/// A cursor over a state.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], StateError> {
        let taken = self.bytes.get(self.at..self.at + n).ok_or(self.wrong())?;
        self.at += n;
        Ok(taken)
    }

    /// The error for a state that ends early or holds what FL never writes.
    fn wrong(&self) -> StateError {
        StateError::Length {
            expected: self.at.max(1182),
            found: self.bytes.len(),
        }
    }

    fn i32(&mut self) -> Result<i32, StateError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, StateError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f64(&mut self) -> Result<f64, StateError> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// Everything the state holds.
#[derive(Debug, Clone, PartialEq)]
struct State {
    maps: [Map; MAPS],
    params: [i32; 20],
    /// 0 for the peak follower, 1 for RMS.
    hold_mode: i32,
    /// Filter stages a band: 1 to 4.
    order: i32,
    /// Modulator unison voices beside the first: 0 to 4.
    unison: i32,
    bands: i32,
    /// 0 with the draft switch on.
    draft: i32,
    modulator: i32,
    carrier: i32,
    switches: [u8; 8],
    /// Whether each envelope follows the modulator's pitch shift.
    follows: [u8; 13],
    /// The carrier synthesizer's preset, empty for none.
    tone: String,
    /// The keys held on the synthesizer's keyboard.
    keys: Vec<i32>,
}

impl Default for State {
    fn default() -> Self {
        let map = FreqMap::new();
        let low = map.place(440.0 * (-29.0f64 / 12.0).exp2()) as f32;
        let high = map.place(20_000.0) as f32;
        let maps = std::array::from_fn(|n| match n {
            NOISE_LEVEL | PASS_THROUGH | SATURATION_MIX => Map::flat(0.0),
            DISTRIBUTION => Map::line(low, high),
            12 => Map::line(0.0, 1.0),
            _ => Map::flat(0.5),
        });
        State {
            maps,
            params: DEFAULTS,
            hold_mode: 0,
            order: 2,
            unison: 0,
            bands: 47,
            draft: 0,
            modulator: 1,
            carrier: 0,
            switches: [0, 1, 1, 1, 1, 1, 0, 1],
            follows: [0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0],
            tone: String::new(),
            keys: vec![60],
        }
    }
}

impl State {
    fn switch(&self, which: usize) -> bool {
        self.switches[which] != 0
    }

    /// Read a state as `SaveRestoreState` (`FUN_0084efb0`) does. `None` for
    /// a version FL reads nothing of.
    fn read(bytes: &[u8]) -> Result<Option<State>, StateError> {
        let mut at = Reader { bytes, at: 0 };
        let mut version = at.i32()?;
        if version <= 1 || version >= 0x0008_0004 {
            return Ok(None);
        }
        if version > 0xffff {
            version >>= 16;
        }
        let mut state = State::default();
        for map in &mut state.maps {
            *map = Map::read(&mut at)?;
        }
        let word = |at: &mut Reader<'_>| at.i32();
        if version < 3 {
            // Seven parameters, the eighth left at 58000, then the rest.
            for param in 0..7 {
                state.params[param] = word(&mut at)?;
            }
            state.params[7] = 58_000;
            for param in 8..20 {
                state.params[param] = word(&mut at)?;
            }
            if state.params[3] == 62_000 {
                state.params[3] = 58_000;
            }
        } else {
            for param in &mut state.params {
                *param = word(&mut at)?;
            }
        }
        for (param, (min, max)) in state.params.iter_mut().zip(RANGES) {
            *param = (*param).clamp(min, max);
        }
        // 36 bytes from 0x170: 7 words and 8 switches, of which a version
        // under 4 has one byte less and one under 5 three more less.
        let mut extra = [0u8; 36];
        let len = match version {
            ..4 => 32,
            4 => 33,
            _ => 36,
        };
        extra[..len].copy_from_slice(at.take(len)?);
        let word = |n: usize| i32::from_le_bytes(extra[n * 4..n * 4 + 4].try_into().unwrap());
        state.hold_mode = word(0).clamp(0, 1);
        state.order = word(1).clamp(1, 4);
        state.unison = word(2).clamp(0, 4);
        state.bands = word(3).clamp(5, 100);
        state.draft = word(4);
        state.modulator = word(5).max(0);
        state.carrier = word(6).max(-1);
        state.switches.copy_from_slice(&extra[28..36]);
        if len < 36 {
            // What FL sets before it reads a short block.
            state.switches[5..].fill(0);
        }
        if len < 33 {
            state.switches[SUBTRACTIVE] = 1;
        }
        state.follows = if version >= 4 {
            at.take(13)?.try_into().unwrap()
        } else {
            [0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0]
        };
        let len = usize::from(at.take(1)?[0]);
        state.tone = String::from_utf8_lossy(at.take(len)?).into_owned();
        let count = at.i32()?;
        if !(0..=128).contains(&count) {
            return Err(at.wrong());
        }
        state.keys = (0..count).map(|_| at.i32()).collect::<Result<_, _>>()?;
        Ok(Some(state))
    }

    /// The state as FL writes it.
    #[cfg(test)]
    fn write(&self) -> Vec<u8> {
        let mut out = SAVED_VERSION.to_le_bytes().to_vec();
        for map in &self.maps {
            map.write(&mut out);
        }
        for param in self.params {
            out.extend(param.to_le_bytes());
        }
        let words = [
            self.hold_mode,
            self.order,
            self.unison,
            self.bands,
            self.draft,
            self.modulator,
            self.carrier,
        ];
        for word in words {
            out.extend(word.to_le_bytes());
        }
        out.extend(self.switches);
        out.extend(self.follows);
        out.push(self.tone.len().min(255) as u8);
        out.extend(&self.tone.as_bytes()[..self.tone.len().min(255)]);
        out.extend((self.keys.len() as i32).to_le_bytes());
        for key in &self.keys {
            out.extend(key.to_le_bytes());
        }
        out
    }
}

/// How the band distribution's 0 to 1 maps to Hz (`FUN_0062cb20`, read by
/// `FUN_0062ccc0` and `FUN_0062cd90`): from 65.4 Hz to 21096 Hz, below a
/// knee at `knee` straight in Hz and above it exponential.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FreqMap {
    min: f64,
    max: f64,
    knee: f64,
    inv_log: f64,
    rate: f64,
    scale: f64,
    span: f64,
}

impl FreqMap {
    fn new() -> Self {
        let min = 440.0 * (-33.0f64 / 12.0).exp2();
        let max = 440.0 * (67.0f64 / 12.0).exp2();
        let knee = 2f64.ln() / (max * 2.0 / min).ln();
        let ratio = max / min;
        let log = (ratio + 1.0).ln();
        FreqMap {
            min,
            max,
            knee,
            inv_log: 1.0 / log,
            rate: log / (1.0 - knee),
            scale: ratio / (max - min),
            span: 1.0 / ratio,
        }
    }

    fn hz(&self, x: f64) -> f64 {
        if x >= self.knee {
            (((x - self.knee) * self.rate).exp() - 1.0) * (self.max - self.min) * self.span
                + self.min
        } else {
            x * self.min / self.knee
        }
    }

    fn place(&self, hz: f64) -> f64 {
        if hz >= self.min {
            self.knee + ((hz - self.min) * self.scale + 1.0).ln() * self.inv_log * (1.0 - self.knee)
        } else {
            hz * self.knee / self.min
        }
    }
}

/// The five coefficients of a biquad: `y = b0 x + b1 x1 + b2 x2 - a1 y1 -
/// a2 y2`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Coefs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefs {
    /// `FUN_006260c0` for the three kinds the plugin uses, from alpha and
    /// `cos w`.
    fn bandpass(alpha: f64, cos: f64) -> Self {
        let d = 1.0 / (alpha + 1.0);
        Coefs {
            b0: d * alpha,
            b1: 0.0,
            b2: d * -alpha,
            a1: d * cos * -2.0,
            a2: d * (1.0 - alpha),
        }
    }

    fn lowpass(alpha: f64, cos: f64) -> Self {
        let d = 1.0 / (alpha + 1.0);
        let b1 = d * (1.0 - cos);
        Coefs {
            b0: b1 * 0.5,
            b1,
            b2: b1 * 0.5,
            a1: d * cos * -2.0,
            a2: d * (1.0 - alpha),
        }
    }

    fn highpass(alpha: f64, cos: f64) -> Self {
        let d = 1.0 / (alpha + 1.0);
        let b1 = -(d * (cos + 1.0));
        Coefs {
            b0: -b1 * 0.5,
            b1,
            b2: -b1 * 0.5,
            a1: d * cos * -2.0,
            a2: d * (1.0 - alpha),
        }
    }

    /// A band's filter stage (`FUN_00626690`): a bandpass at `hz`,
    /// `octaves` wide.
    fn band(hz: f32, octaves: f32, step: f64) -> Self {
        let w = f64::from(hz) * step;
        let (sin, cos) = w.sin_cos();
        let alpha = sin * (LN_2 * 0.5 * f64::from(octaves) * w / sin).sinh();
        Coefs::bandpass(alpha, cos)
    }

    /// Run `x` through, with the state `z` as `[x1, x2, y1, y2]`, in the
    /// order of FL's sums.
    fn step(&self, z: &mut [f64; 4], x: f64) -> f64 {
        let y =
            x * self.b0 + (((z[0] * self.b1 - z[3] * self.a2) + z[1] * self.b2) - z[2] * self.a1);
        *z = [x, z[0], y, z[2]];
        y
    }
}

/// One biquad of a band, on both sides.
#[derive(Debug, Clone, PartialEq, Default)]
struct Stage {
    hz: f32,
    octaves: f32,
    coefs: Coefs,
    z: [[f64; 4]; 2],
}

/// A pass-through or noise filter: two stages, or none.
#[derive(Debug, Clone, PartialEq, Default)]
struct Pass {
    on: bool,
    coefs: [Coefs; 2],
    z: [[[f64; 4]; 2]; 2],
}

impl Pass {
    /// The two stages at the wheel's `40 * 375^(v / 65536)` Hz
    /// (`FUN_00853de0`), or none at `off`.
    fn tune(&mut self, wheel: i32, off: i32, high: bool, step: f64) {
        self.on = wheel != off;
        let hz = (40.0 * 375f64.powf(f64::from(wheel) / 65_536.0)) as f32;
        let (sin, cos) = (f64::from(hz) * step).sin_cos();
        for (coefs, k) in self.coefs.iter_mut().zip(PASS_K) {
            *coefs = if high {
                Coefs::highpass(sin * k, cos)
            } else {
                Coefs::lowpass(sin * k, cos)
            };
        }
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.on {
            return;
        }
        for (coefs, z) in self.coefs.iter().zip(&mut self.z) {
            for v in left.iter_mut() {
                *v = coefs.step(&mut z[0], f64::from(*v)) as f32;
            }
            for v in right.iter_mut() {
                *v = coefs.step(&mut z[1], f64::from(*v)) as f32;
            }
        }
    }

    fn clear(&mut self) {
        self.z = Default::default();
    }
}

/// A run of one-pole stages that rise and fall at their own speeds
/// (`FUN_00606730`, run by `FUN_00606660`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Smoother {
    count: usize,
    /// What each stage keeps of its way when its input is above it and
    /// when it is not.
    up: [f64; 4],
    down: [f64; 4],
    y: [f64; 4],
}

impl Smoother {
    /// `attack` and `release` in frames, each spread over as many stages as
    /// its curve, the last of them the fraction left.
    fn tune(&mut self, attack: f64, attack_curve: f64, release: f64, release_curve: f64) {
        let ca = attack_curve.max(1.0);
        let cr = release_curve.max(1.0);
        let na = round(ca + 0.499_999_999_999_999_94).max(1) as usize;
        let nr = round(cr + 0.499_999_999_999_999_94).max(1) as usize;
        self.count = na.max(nr).min(4);
        let (mut a, mut r) = (attack / ca.sqrt(), release / cr.sqrt());
        for stage in 0..self.count {
            if stage + 1 >= na {
                a = if stage < na {
                    a * (1.0 - (na as f64 - ca))
                } else {
                    0.0
                };
            }
            if stage + 1 >= nr {
                r = if stage < nr {
                    r * (1.0 - (nr as f64 - cr))
                } else {
                    0.0
                };
            }
            self.up[stage] = crate::limiter::coef(a);
            self.down[stage] = crate::limiter::coef(r);
        }
    }

    fn run(&mut self, values: &mut [f32]) {
        for v in values {
            let mut x = *v;
            for stage in 0..self.count {
                let y = self.y[stage];
                let keep = if f64::from(x) <= y {
                    self.down[stage]
                } else {
                    self.up[stage]
                };
                self.y[stage] = f64::from(x) + (y - f64::from(x)) * keep;
                x = self.y[stage] as f32;
            }
            *v = x;
        }
    }
}

/// The envelope follower of a modulator band: the peak of the last frames
/// (`TPeakLookahead2`, `FUN_00788010`) or their RMS (`TRMSLookahead`,
/// `FUN_00788330`).
#[derive(Debug, Clone, PartialEq, Default)]
struct Follower {
    rms: bool,
    window: usize,
    /// For the peak: the frames that may still be the highest, oldest
    /// first, as their number and value, in a ring. For the RMS: the
    /// squares of the window in a ring.
    ring: Vec<(u64, f32)>,
    head: usize,
    len: usize,
    count: u64,
    sum: f64,
}

impl Follower {
    fn new(rms: bool, window: usize) -> Self {
        let window = window.max(1);
        Follower {
            rms,
            window,
            ring: vec![(0, 0.0); window],
            head: 0,
            len: 0,
            count: 0,
            sum: 0.0,
        }
    }

    /// A new window, as long as the ring at most. A change starts the
    /// follower anew, as FL's does.
    fn set_window(&mut self, window: usize) {
        let window = window.clamp(1, self.ring.len().max(1));
        if window != self.window {
            self.window = window;
            self.clear();
        }
    }

    fn clear(&mut self) {
        self.ring.fill((0, 0.0));
        (self.head, self.len, self.count, self.sum) = (0, 0, 0, 0.0);
    }

    /// One frame of a band's two sides.
    fn step(&mut self, left: f32, right: f32) -> f32 {
        let cap = self.ring.len();
        if self.rms {
            let sq = left * left + right * right;
            let at = (self.count % self.window as u64) as usize;
            self.sum += f64::from(sq) - f64::from(self.ring[at].1);
            self.ring[at].1 = sq;
            self.count += 1;
            let mean = (self.sum / (self.window * 2) as f64) as f32;
            return mean.max(0.0).sqrt();
        }
        let x = left.abs().max(right.abs());
        self.count += 1;
        // Drop what this frame is above, then what has left the window.
        while self.len > 0 {
            let back = (self.head + self.len - 1) % cap;
            if self.ring[back].1 > x {
                break;
            }
            self.len -= 1;
        }
        let back = (self.head + self.len) % cap;
        self.ring[back] = (self.count, x);
        self.len += 1;
        while self.ring[self.head].0 + self.window as u64 <= self.count {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
        }
        self.ring[self.head].1
    }
}

/// One set of filters of a band: a modulator voice, or the carrier.
#[derive(Debug, Clone, PartialEq, Default)]
struct Set {
    /// The band's centre in octaves over [`C2`].
    octave: f64,
    /// What the modulator's filters are moved by.
    ratio: f64,
    width: f64,
    octaves: f64,
    gain: f32,
    pan: f32,
    threshold: f32,
    /// The left and right gain of the envelope.
    sides: [f32; 2],
    /// 2 for a band filtered at twice the rate.
    rate: usize,
    stages: Vec<Stage>,
    follower: Follower,
    smoother: Smoother,
    /// The last envelope, which a frozen one stays at.
    last: f32,
}

/// One band.
#[derive(Debug, Clone, PartialEq, Default)]
struct Band {
    /// The modulator voices, then the carrier.
    sets: Vec<Set>,
    pass: f64,
    offset: f32,
    hold: f64,
    attack: f64,
    release: f64,
}

/// The 2x upsampler of one input (`FUN_0077ab60`): two branches of three
/// allpasses, the second branch's output first.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Upsampler {
    /// Per side, the last input and output of each of the six allpasses.
    x: [[f64; 6]; 2],
    y: [[f64; 6]; 2],
}

impl Upsampler {
    fn step(&mut self, side: usize, input: f32) -> [f32; 2] {
        let (x, y) = (&mut self.x[side], &mut self.y[side]);
        let mut pass = |at: usize, v: f64| {
            let out = x[at] + (v - y[at]) * HALFBAND[at];
            (x[at], y[at]) = (v, out);
            out
        };
        let v = f64::from(input);
        let a = pass(0, v);
        let b = pass(1, v);
        let a = pass(2, a);
        let b = pass(3, b);
        let a = pass(4, a);
        let b = pass(5, b);
        [b as f32, a as f32]
    }
}

/// The sibilance detector (`TZXLookahead`, `FUN_00788680`): how often the
/// modulator crosses zero in the last 8 ms, above 90 Hz.
#[derive(Debug, Clone, PartialEq, Default)]
struct Consonants {
    coefs: Coefs,
    z: [f64; 4],
    ring: Vec<u8>,
    at: usize,
    crossings: i32,
    last: f32,
    /// What is added before the count: -42 dB, so that a quiet input does
    /// not cross.
    dc: f32,
}

impl Consonants {
    fn new(hz: f64) -> Self {
        let w = 90.0 * 2.0 * PI / hz;
        let (sin, cos) = w.sin_cos();
        let window = round(8.0 * 0.001 * hz).max(1) as usize;
        Consonants {
            coefs: Coefs::highpass(sin, cos),
            ring: vec![0; window],
            dc: (f64::from((10f64.ln() * 0.05) as f32 * -42.0)).exp() as f32,
            ..Default::default()
        }
    }

    fn step(&mut self, mono: f32) -> f32 {
        let v = self.coefs.step(&mut self.z, f64::from(mono)) as f32 + self.dc;
        self.at = (self.at + 1) % self.ring.len();
        let crossed = u8::from(v * self.last < 0.0);
        self.last = v;
        self.crossings += i32::from(crossed) - i32::from(self.ring[self.at]);
        self.ring[self.at] = crossed;
        let v = self.crossings as f32 * (0.05 / 8.0) as f32;
        (v * v).min(1.0)
    }

    fn clear(&mut self) {
        self.z = [0.0; 4];
        self.ring.fill(0);
        (self.at, self.crossings, self.last) = (0, 0, 0.0);
    }
}

/// The contour: the level of the consonants, of the carrier or of both, as
/// a peak of the last 12 ms smoothed.
#[derive(Debug, Clone, PartialEq, Default)]
struct Contour {
    consonants: Consonants,
    peak: Follower,
    smoother: Smoother,
}

/// A gain that moves to a new value over the next block.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Ramp {
    now: f32,
    target: f32,
}

impl Ramp {
    /// The gain at each frame of a block of `n`, which ends at the target.
    fn at(&self, i: usize, n: usize) -> f32 {
        if self.now == self.target {
            self.target
        } else {
            self.now + (self.target - self.now) * (i + 1) as f32 / n as f32
        }
    }
}

/// The plugin's working buffers, [`CHUNK`] long or twice that.
#[derive(Debug, Clone, PartialEq, Default)]
struct Work {
    modulator: [Vec<f32>; 2],
    carrier: [Vec<f32>; 2],
    /// The carrier with the noise on it.
    noisy: [Vec<f32>; 2],
    contour: Vec<f32>,
    /// The two inputs at their rate and at twice it, modulator first.
    native: [[Vec<f64>; 2]; 2],
    double: [[Vec<f64>; 2]; 2],
    /// A band's filtered input, and its envelope.
    band: [Vec<f64>; 2],
    scratch: [Vec<f64>; 2],
    envelope: Vec<f32>,
    stereo: [Vec<f32>; 2],
    /// The sum of the bands.
    sum: [Vec<f64>; 2],
    pass: [Vec<f32>; 2],
}

impl Work {
    fn new() -> Self {
        let one = || vec![0.0f32; CHUNK];
        let wide = || vec![0.0f64; 2 * CHUNK];
        Work {
            modulator: [one(), one()],
            carrier: [one(), one()],
            noisy: [one(), one()],
            contour: one(),
            native: [[wide(), wide()], [wide(), wide()]],
            double: [[wide(), wide()], [wide(), wide()]],
            band: [wide(), wide()],
            scratch: [wide(), wide()],
            envelope: one(),
            stereo: [one(), one()],
            sum: [wide(), wide()],
            pass: [one(), one()],
        }
    }
}

/// Vocodex.
#[derive(Debug)]
pub struct Vocodex {
    state: State,
    hz: f64,
    map: FreqMap,
    bands: Vec<Band>,
    contour: Option<Contour>,
    mod_pass: Pass,
    car_pass: Pass,
    noise_pass: Pass,
    wet: Ramp,
    mod_level: Ramp,
    car_level: Ramp,
    noise: f32,
    seed: u32,
    /// Frames of silence after which the plugin sleeps, `None` for never,
    /// and the count down to it.
    sleep_after: Option<i64>,
    countdown: i64,
    ups: [Upsampler; 2],
    work: Work,
    soundgoodizer: Box<Soundgoodizer>,
    synth: Option<Box<Sytrus>>,
    /// FL's `Plugins/Fruity` directory, when the host said.
    fruity: Option<PathBuf>,
    mod_side: Sidechain,
    car_side: Sidechain,
}

impl Default for Vocodex {
    fn default() -> Self {
        let mut plugin = Vocodex {
            state: State::default(),
            hz: 44_100.0,
            map: FreqMap::new(),
            bands: Vec::new(),
            contour: None,
            mod_pass: Pass::default(),
            car_pass: Pass::default(),
            noise_pass: Pass::default(),
            wet: Ramp::default(),
            mod_level: Ramp::default(),
            car_level: Ramp::default(),
            noise: 0.0,
            seed: 0x2545_f491,
            sleep_after: None,
            countdown: 0,
            ups: Default::default(),
            work: Work::new(),
            soundgoodizer: Box::default(),
            synth: None,
            fruity: None,
            mod_side: Sidechain::default(),
            car_side: Sidechain::default(),
        };
        plugin.build();
        plugin
    }
}

/// Delphi's `Round`: to the nearest whole number, a half to the even one.
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// A level knob of FL's: `(base^v - 1) / (base - 1)`, and 1 exactly at 1.
fn level(v: f32, base: f32) -> f32 {
    if v == 1.0 {
        1.0
    } else {
        ((v * base.ln()).exp() - 1.0) / (base - 1.0)
    }
}

/// `2^(6 v - 3)`: an envelope's 1/8 to 8 times.
fn scale(v: f64) -> f64 {
    f64::from(2f32.powf((v * 6.0) as f32) * 0.125)
}

/// The event 213 bytes of an `.fst` preset.
fn fst_state(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.get(..4)? != b"FLhd" {
        return None;
    }
    let header = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let mut at = 8 + header;
    if bytes.get(at..at + 4)? != b"FLdt" {
        return None;
    }
    at += 8;
    while at < bytes.len() {
        let id = *bytes.get(at)?;
        at += 1;
        let len = match id {
            0..64 => 1,
            64..128 => 2,
            128..192 => 4,
            _ => {
                let (mut len, mut shift) = (0usize, 0);
                loop {
                    let byte = *bytes.get(at)?;
                    at += 1;
                    len |= usize::from(byte & 0x7f) << shift;
                    shift += 7;
                    if byte & 0x80 == 0 {
                        break len;
                    }
                }
            }
        };
        let data = bytes.get(at..at + len)?;
        at += len;
        if id == 213 {
            return Some(data.to_vec());
        }
    }
    None
}

impl Vocodex {
    /// The sets, filters, followers and buffers for the state and the
    /// rate. This asks for memory.
    fn build(&mut self) {
        let state = &self.state;
        let sets = state.unison as usize + 2;
        let rms = state.hold_mode == 1;
        let order = state.order as usize;
        self.bands = (0..state.bands)
            .map(|_| Band {
                sets: (0..sets)
                    .map(|_| Set {
                        stages: vec![Stage::default(); order],
                        follower: Follower::new(rms, 1),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })
            .collect();
        let contour = (state.switch(CONTOUR_TO_MOD) || state.switch(CONTOUR_TO_NOISE))
            && (state.switch(CONTOUR_FROM_CAR) || state.switch(CONTOUR_FROM_CONSONANTS));
        self.contour = contour.then(|| {
            let mut smoother = Smoother::default();
            smoother.tune(self.hz * 0.016, 3.0, self.hz * 0.044, 3.0);
            Contour {
                consonants: Consonants::new(self.hz),
                peak: Follower::new(false, round(self.hz * 0.012) as usize),
                smoother,
            }
        });
        // The followers' windows are what the parameters ask for now.
        let windows = self.tune();
        for (band, windows) in self.bands.iter_mut().zip(windows) {
            for (set, window) in band.sets.iter_mut().zip(windows) {
                set.follower = Follower::new(rms, window);
            }
        }
        self.reset();
    }

    /// Work everything out from the parameters and the envelopes, as
    /// `FUN_00851dd0` and the parameter handlers do. Hands back the window
    /// each modulator follower wants. Allocates only for that list, which
    /// [`Vocodex::retune`] drops.
    fn tune(&mut self) -> Vec<Vec<usize>> {
        let mut windows = Vec::with_capacity(self.bands.len());
        self.tune_into(&mut |band, set, window| {
            if set == 0 {
                windows.push(Vec::new());
            }
            if let Some(list) = windows.get_mut(band) {
                list.push(window);
            }
        });
        windows
    }

    /// [`Vocodex::tune`] without the list: the followers take their new
    /// windows as far as their rings reach.
    fn retune(&mut self) {
        self.tune_into(&mut |_, _, _| {});
    }

    fn tune_into(&mut self, windows: &mut dyn FnMut(usize, usize, usize)) {
        let state = &self.state;
        let hz = self.hz;
        let step = 2.0 * PI / hz;
        let p = |n: usize| state.params[n];
        let n_bands = state.bands as usize;
        let order = state.order as usize;
        let unison = state.unison as usize;
        let rms = state.hold_mode == 1;
        let flat = f64::from(p(15) as f32 * 0.01);
        let bw_knob =
            f64::from(((5f64.ln() * f64::from(p(14) as f32 * 0.001)).exp() - 1.0) as f32 * 0.5);
        let follows = |map: usize| usize::from(state.follows[map] != 0);
        let map_at = |map: usize, x: f64| {
            let center = if CENTRED.contains(&map) { 0.5 } else { 0.0 };
            state.maps[map].at(x, center)
        };
        // The gain every band starts from.
        let mut g = if rms { 1.5 } else { 1.0 };
        if order > 1 {
            g /= order as f64 - flat * (order - 1) as f64;
        }
        g *= (4.0 / (n_bands as f64).sqrt()).sqrt() * f64::from(2f32.powi(order as i32 - 1)) * 3.0;
        let carrier = unison + 1;
        for set_at in 0..=carrier {
            let is_mod = set_at < carrier;
            // FL's k: -unison for the first voice, 0 for the last, 1 for
            // the carrier.
            let k = set_at as i64 - unison as i64;
            for b in 0..n_bands {
                let band = &mut self.bands[b];
                let x_band = b as f64 / (n_bands - 1) as f64;
                let hz_band = self.map.hz(map_at(DISTRIBUTION, x_band)).max(20.0);
                let mut x = [self.map.place(hz_band), 0.0];
                let set = &mut band.sets[set_at];
                if is_mod {
                    let mut pitch = map_at(PITCH, x[0]) * -2.0;
                    if unison > 0 {
                        pitch += f64::from(UNISON[unison][(-k) as usize] * p(18) as f32 * 0.001);
                    }
                    set.ratio = ((f64::from(p(17)) * (-1.0 / 1200.0) + pitch) * LN_2).exp();
                    x[1] = self.map.place(hz_band * set.ratio);
                    let t = (map_at(GAIN_MULTIPLIER, x[follows(GAIN_MULTIPLIER)]) * 2.0) as f32;
                    let gain_mult = f64::from(level3(t));
                    set.pan = (map_at(PANNING, x[follows(PANNING)]) * 2.0) as f32;
                    let offset = map_at(GAIN_OFFSET, x[follows(GAIN_OFFSET)]) * 2.0;
                    let size = (1025f64.ln() * f64::from(offset.abs() as f32)).exp();
                    band.offset = ((size - 1.0) as f32 / 1024.0) * offset.signum() as f32;
                    if offset == 0.0 {
                        band.offset = 0.0;
                    }
                    band.hold = scale(map_at(HOLD, x[follows(HOLD)]));
                    band.attack = scale(map_at(ATTACK, x[follows(ATTACK)]));
                    band.release = scale(map_at(RELEASE, x[follows(RELEASE)]));
                    let through = (map_at(PASS_THROUGH, x[follows(PASS_THROUGH)]) * 2.0) as f32;
                    band.pass = f64::from(level3(through) * 0.5);
                    let noise = map_at(NOISE_LEVEL, x[1]) as f32;
                    set.threshold =
                        ((1025f64.ln() * f64::from(noise)).exp() - 1.0) as f32 * (1.0 / 1024.0);
                    set.width = f64::from(2f32.powf(p(16) as f32 * 0.001) * 0.25);
                    set.gain = (g * gain_mult) as f32;
                    if state.follows[WIDTH] != 0 {
                        x[0] = x[1];
                    }
                } else {
                    set.gain = g as f32;
                    set.ratio = 1.0;
                    set.width = 1.0;
                }
                let width = f64::from(2f32.powf((map_at(WIDTH, x[0]) * 4.0) as f32));
                set.width *= bw_knob * 0.25 * width;
                set.octave = (hz_band / C2).ln() * (1.0 / LN_2);
            }
            // The widths, from the distance to the bands either side.
            for b in 0..n_bands {
                let octave = |b: usize| self.bands[b].sets[set_at].octave;
                let here = octave(b);
                let mut d = 0.0;
                if b > 0 {
                    d += (octave(b - 1) - here).abs();
                }
                if b + 1 < n_bands {
                    d += (octave(b + 1) - here).abs();
                    if b > 0 {
                        d *= 0.5;
                    }
                }
                let band = &mut self.bands[b];
                let set = &mut band.sets[set_at];
                set.octaves = (d * set.width).clamp(1.0 / 60.0, 8.0);
                let s = (1.0 / set.octaves).sqrt();
                set.gain = (f64::from(set.gain) * s) as f32;
                let inverse = 1.0 / set.gain.max(1.525_878_9e-5);
                set.threshold *= inverse;
                if state.switch(SUBTRACTIVE) {
                    set.threshold = -set.threshold;
                }
                if is_mod {
                    band.pass *= g * s * 0.25;
                }
                for (j, stage) in set.stages.iter_mut().enumerate() {
                    let mut octaves = (set.octaves * order as f64).min(2.0) as f32;
                    let mut off = 0.0;
                    if order > 1 {
                        octaves = (f64::from(octaves) / (flat * 0.5 + 1.0)) as f32;
                        let spread = 1.0 / (order - 1) as f64;
                        off = (j as f64 * spread - 0.5) * flat * f64::from(octaves);
                        if order == 3 && j != 1 {
                            octaves *= 0.5;
                        } else if order == 4 && (j == 0 || j == 3) {
                            octaves = (f64::from(octaves) * 0.363_636_363_636_363_65) as f32;
                        }
                    }
                    stage.octaves = octaves;
                    stage.hz = (C2 * set.ratio * ((set.octave + off) * LN_2).exp()) as f32;
                }
            }
        }
        // The times.
        let time = |wheel: i32| {
            f64::from(((501f64.ln() * f64::from(wheel as f32 * 1e-4)).exp() - 1.0) as f32)
                * 2.0
                * 0.001
        };
        let attack = time(p(9));
        let release = time(p(11)) * hz * 2.0;
        let hold = time(p(8)) * hz * 0.25;
        let attack_curve = f64::from(2f32.powf(p(10) as f32 * 0.001));
        let release_curve = f64::from(2f32.powf(p(12) as f32 * 0.001));
        let mut slack = 0f32;
        for (b, band) in self.bands.iter_mut().enumerate() {
            let carrier_gain = band.sets[carrier].gain;
            band.offset *= carrier_gain;
            for set in &mut band.sets {
                let top = set
                    .stages
                    .iter()
                    .map(|st| ((f64::from(st.octaves * 2.0)) * LN_2).exp() * f64::from(st.hz));
                let top = top.fold(0.0, f64::max);
                set.rate = 1 + usize::from(top > OVERSAMPLE_ABOVE);
            }
            let first_carrier = band.sets[carrier].stages[0].clone();
            for (set_at, set) in band.sets[..carrier].iter_mut().enumerate() {
                let k = set_at as i64 - unison as i64;
                if unison > 0 {
                    let place = (b as i64 - k).rem_euclid(unison as i64 + 1) as f64;
                    let spread = (place / unison as f64 - 0.5)
                        * f64::from(p(19))
                        * f64::from(1.0f32 / 128.0);
                    set.pan = (f64::from(set.pan) + spread) as f32;
                }
                let gain = f64::from(set.gain) * f64::from(carrier_gain) / (unison + 1) as f64;
                set.sides = pan_law(set.pan.clamp(-1.0, 1.0), gain as f32);
                let mut least_hold = 0.0;
                if state.switch(MIN_TIMES) {
                    let st = &set.stages[0];
                    let low = (f64::from(-3.0 - st.octaves) * LN_2).exp() * f64::from(st.hz);
                    least_hold = hz / low.clamp(40.0, 16_000.0);
                    if rms {
                        least_hold *= 2.0;
                    }
                }
                let held = (hold * band.hold).max(least_hold);
                let window = round(held).max(1) as usize;
                windows(b, set_at, window);
                set.follower.set_window(window);
                let mut least = 0.0;
                if state.switch(MIN_TIMES) {
                    let st = &first_carrier;
                    let low = (f64::from(-st.octaves) * LN_2).exp() * f64::from(st.hz);
                    least = hz / low.clamp(40.0, 800.0);
                }
                let a = (attack * hz * band.attack).max(least);
                let r = (release * band.release).max(least);
                set.smoother.tune(a, attack_curve, r, release_curve);
                slack = slack.max((held + r) as f32);
            }
            for set in &mut band.sets {
                let rate = set.rate as f32;
                for stage in &mut set.stages {
                    let at = (f64::from(stage.hz / rate)).min(hz * 0.4) as f32;
                    stage.coefs = Coefs::band(at, stage.octaves, step);
                }
            }
        }
        // The mixer.
        let frozen = p(13) != 0;
        let from_car = state.switch(CONTOUR_FROM_CAR);
        self.wet.target = level(p(0) as f32 * 0.001, 7.0);
        let mut mod_level = level(p(2) as f32 * (1.0 / 12_800.0), 11.0);
        if state.switch(CONTOUR_TO_MOD) && from_car {
            mod_level = if frozen { 0.0 } else { mod_level * 4.0 };
        }
        self.mod_level.target = mod_level;
        self.car_level.target = level(p(4) as f32 * (1.0 / 12_800.0), 11.0);
        let mut noise = level(p(6) as f32 * (1.0 / 12_800.0), 7.0) * 0.5;
        if state.switch(CONTOUR_TO_NOISE) && from_car {
            noise *= 4.0;
        }
        self.noise = noise;
        self.mod_pass.tune(p(3), 0, true, step);
        self.car_pass.tune(p(5), 65_536, false, step);
        self.noise_pass.tune(p(7), 0, true, step);
        self.soundgoodizer.set_param(5, p(1) as f32);
        self.sleep_after =
            (!frozen && self.synth.is_none()).then(|| round(hz * 0.5 + f64::from(slack) + 16.0));
    }

    /// Load the synthesizer the state names, when the host has said where
    /// FL keeps it, and hold its keys. Reads a file.
    fn load_synth(&mut self) {
        self.synth = None;
        let Some(fruity) = &self.fruity else {
            return;
        };
        if self.state.tone.is_empty() {
            return;
        }
        let path = fruity
            .join("Effects")
            .join("Vocodex")
            .join("Synthesizer")
            .join("Data")
            .join("Presets")
            .join(format!("{}.fst", self.state.tone));
        let Some(bytes) = std::fs::read(path).ok().and_then(|b| fst_state(&b)) else {
            return;
        };
        let mut synth = Box::new(Sytrus::default());
        synth.set_sample_rate(self.hz as f32);
        if synth.set_state(&bytes).is_err() {
            return;
        }
        self.synth = Some(synth);
        self.hold_keys();
    }

    /// Play the held keys on the synthesizer (`FUN_00850aa0`): at a volume
    /// of 0.5, in the middle.
    fn hold_keys(&mut self) {
        let Some(synth) = &mut self.synth else {
            return;
        };
        synth.all_notes_off();
        synth.reset();
        for (voice, key) in self.state.keys.iter().enumerate() {
            synth.note_on(NoteOn {
                voice: voice as u32,
                key: (*key).clamp(0, 127) as i16,
                velocity: 0.5,
                pan: 0.0,
                fine_pitch: 0.0,
                release: 0.5,
                mod_x: 0.0,
                mod_y: 0.0,
                slide: false,
                color: 0,
                porta: false,
            });
        }
    }

    /// One piece of a block, at most [`CHUNK`] frames.
    fn chunk(&mut self, at: usize, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let state = &self.state;
        let w = &mut self.work;
        // The two inputs.
        let side = |side| frames(side, at, n);
        if state.switch(LR_ENCODING) {
            w.modulator[0][..n].copy_from_slice(left);
            w.modulator[1][..n].copy_from_slice(left);
            w.carrier[0][..n].copy_from_slice(right);
            w.carrier[1][..n].copy_from_slice(right);
        } else {
            let own = (&*left, &*right);
            let m = if state.modulator > 0 {
                side(&self.mod_side).unwrap_or(own)
            } else {
                own
            };
            w.modulator[0][..n].copy_from_slice(m.0);
            w.modulator[1][..n].copy_from_slice(m.1);
            if state.carrier < 0 {
                w.carrier[0][..n].fill(0.0);
                w.carrier[1][..n].fill(0.0);
            } else {
                let c = if state.carrier > 0 {
                    side(&self.car_side).unwrap_or(own)
                } else {
                    own
                };
                w.carrier[0][..n].copy_from_slice(c.0);
                w.carrier[1][..n].copy_from_slice(c.1);
            }
        }
        // Silence on both inputs long enough puts the plugin to sleep: its
        // own input passes as it is.
        if let Some(after) = self.sleep_after {
            let loud = |v: &f32| v.abs() > QUIET;
            let heard = w
                .carrier
                .iter()
                .chain(&w.modulator)
                .any(|s| s[..n].iter().any(loud));
            if heard {
                self.countdown = after;
            } else {
                self.countdown -= n as i64;
            }
            if self.countdown <= -(n as i64) {
                self.wet.now = self.wet.target;
                self.mod_level.now = self.mod_level.target;
                self.car_level.now = self.car_level.target;
                return;
            }
        }
        if let Some(synth) = &mut self.synth {
            let [l, r] = &mut w.carrier;
            synth.process(&mut l[..n], &mut r[..n]);
        }
        let frozen = state.params[13] != 0;
        // The contour.
        if let Some(contour) = &mut self.contour {
            let from_cons = state.switch(CONTOUR_FROM_CONSONANTS);
            let from_car = state.switch(CONTOUR_FROM_CAR);
            for i in 0..n {
                let (mut l, mut r) = if from_cons {
                    let c = if frozen {
                        0.0
                    } else {
                        contour
                            .consonants
                            .step(w.modulator[0][i] + w.modulator[1][i])
                    };
                    (c, c)
                } else {
                    (1.0, 1.0)
                };
                if from_car {
                    l *= w.carrier[0][i];
                    r *= w.carrier[1][i];
                }
                w.contour[i] = contour.peak.step(l, r);
            }
            contour.smoother.run(&mut w.contour[..n]);
        }
        let contour = self.contour.is_some();
        // The noise on the carrier.
        let noisy = self.noise != 0.0;
        if noisy {
            let a = self.noise;
            for i in 0..n {
                for side in 0..2 {
                    self.seed ^= self.seed << 13;
                    self.seed ^= self.seed >> 17;
                    self.seed ^= self.seed << 5;
                    let v = (self.seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                    w.noisy[side][i] = v * a;
                }
            }
            let [l, r] = &mut w.noisy;
            self.noise_pass.run(&mut l[..n], &mut r[..n]);
            for side in 0..2 {
                for i in 0..n {
                    let shaped = if contour && state.switch(CONTOUR_TO_NOISE) {
                        w.noisy[side][i] * w.contour[i]
                    } else {
                        w.noisy[side][i]
                    };
                    w.noisy[side][i] = shaped + w.carrier[side][i];
                }
            }
        }
        // Both inputs at their rate and at twice it.
        for (which, up) in self.ups.iter_mut().enumerate() {
            let source = match (which, noisy) {
                (0, _) => &w.modulator,
                (_, true) => &w.noisy,
                _ => &w.carrier,
            };
            for (side, input) in source.iter().enumerate() {
                let native = &mut w.native[which][side];
                let double = &mut w.double[which][side];
                for (i, &v) in input[..n].iter().enumerate() {
                    native[i] = f64::from(v);
                    let [a, b] = up.step(side, v);
                    double[2 * i] = f64::from(a);
                    double[2 * i + 1] = f64::from(b);
                }
            }
        }
        // The bands.
        let unison = state.unison as usize;
        let carrier = unison + 1;
        let subtract = state.switch(SUBTRACTIVE);
        w.sum[0][..n].fill(0.0);
        w.sum[1][..n].fill(0.0);
        for band in &mut self.bands {
            for set_at in 0..=carrier {
                let set = &mut band.sets[set_at];
                let input = usize::from(set_at == carrier);
                filter(
                    set,
                    &w.native[input],
                    &w.double[input],
                    &mut w.band,
                    &mut w.scratch,
                    n,
                );
                if set_at == carrier {
                    break;
                }
                if band.pass != 0.0 {
                    for side in 0..2 {
                        for i in 0..n {
                            w.sum[side][i] += w.band[side][i] * band.pass;
                        }
                    }
                }
                for i in 0..n {
                    let (l, r) = (w.band[0][i] as f32, w.band[1][i] as f32);
                    w.envelope[i] = set.follower.step(l, r);
                }
                let threshold = set.threshold;
                if threshold != 0.0 {
                    for v in &mut w.envelope[..n] {
                        *v = if subtract {
                            (*v + threshold).max(0.0)
                        } else if *v < threshold {
                            0.0
                        } else {
                            *v
                        };
                    }
                }
                if frozen {
                    w.envelope[..n].fill(set.last);
                } else {
                    set.smoother.run(&mut w.envelope[..n]);
                    set.last = w.envelope[n - 1];
                }
                let [gl, gr] = set.sides;
                for i in 0..n {
                    let e = w.envelope[i];
                    if set_at == 0 {
                        w.stereo[0][i] = e * gl;
                        w.stereo[1][i] = e * gr;
                    } else {
                        w.stereo[0][i] += e * gl;
                        w.stereo[1][i] += e * gr;
                    }
                }
            }
            if band.offset != 0.0 {
                for side in 0..2 {
                    for v in &mut w.stereo[side][..n] {
                        *v += band.offset;
                        if band.offset < 0.0 {
                            *v = v.max(0.0);
                        }
                    }
                }
            }
            // `w.band` holds the carrier's band now.
            for side in 0..2 {
                for i in 0..n {
                    w.sum[side][i] += f64::from(w.stereo[side][i]) * w.band[side][i];
                }
            }
        }
        // The mix: the carrier and the modulator passed through, then the
        // bands.
        if self.car_level.target != 0.0 || self.car_level.now != 0.0 {
            for side in 0..2 {
                w.pass[side][..n].copy_from_slice(&w.carrier[side][..n]);
            }
            let [l, r] = &mut w.pass;
            self.car_pass.run(&mut l[..n], &mut r[..n]);
            for i in 0..n {
                let g = self.car_level.at(i, n);
                left[i] = w.pass[0][i] * g;
                right[i] = w.pass[1][i] * g;
            }
        } else {
            left.fill(0.0);
            right.fill(0.0);
        }
        if self.mod_level.target != 0.0 || self.mod_level.now != 0.0 {
            for side in 0..2 {
                w.pass[side][..n].copy_from_slice(&w.modulator[side][..n]);
            }
            let [l, r] = &mut w.pass;
            self.mod_pass.run(&mut l[..n], &mut r[..n]);
            let shaped = contour && state.switch(CONTOUR_TO_MOD);
            for i in 0..n {
                let g = self.mod_level.at(i, n);
                let c = if shaped { w.contour[i] } else { 1.0 };
                left[i] += w.pass[0][i] * c * g;
                right[i] += w.pass[1][i] * c * g;
            }
        }
        for i in 0..n {
            let g = self.wet.at(i, n);
            left[i] += w.sum[0][i] as f32 * g;
            right[i] += w.sum[1][i] as f32 * g;
        }
        self.wet.now = self.wet.target;
        self.mod_level.now = self.mod_level.target;
        self.car_level.now = self.car_level.target;
        if state.params[1] > 0 {
            self.soundgoodizer.process(left, right);
        }
    }
}

/// Frames `at..at + n` of a sidechain block, when it is that long.
fn frames(side: &Sidechain, at: usize, n: usize) -> Option<(&[f32], &[f32])> {
    Some((side.left.get(at..at + n)?, side.right.get(at..at + n)?))
}

/// `level` with base 3 and the gain envelope's halving: `(3^t - 1) / 2`.
fn level3(t: f32) -> f32 {
    if t == 1.0 {
        1.0
    } else {
        ((t * 3f32.ln()).exp() - 1.0) * 0.5
    }
}

/// The plugin's pan law (`FUN_0062c860`): the far side full, the near one
/// turned down.
fn pan_law(pan: f32, gain: f32) -> [f32; 2] {
    if pan.is_sign_negative() {
        [gain, (pan + 1.0) * gain]
    } else {
        [(1.0 - pan) * gain, gain]
    }
}

/// Run a band's input through one set of its filters into `out`: at the
/// rate, or at twice it and then back down by the mean of each pair
/// (`FUN_00855a60`, `FUN_0062d3a0`).
fn filter(
    set: &mut Set,
    native: &[Vec<f64>; 2],
    double: &[Vec<f64>; 2],
    out: &mut [Vec<f64>; 2],
    scratch: &mut [Vec<f64>; 2],
    n: usize,
) {
    for side in 0..2 {
        if set.rate == 1 {
            out[side][..n].copy_from_slice(&native[side][..n]);
            for stage in &mut set.stages {
                let z = &mut stage.z[side];
                for v in &mut out[side][..n] {
                    *v = stage.coefs.step(z, *v);
                }
            }
        } else {
            scratch[side][..2 * n].copy_from_slice(&double[side][..2 * n]);
            for stage in &mut set.stages {
                let z = &mut stage.z[side];
                for v in &mut scratch[side][..2 * n] {
                    *v = stage.coefs.step(z, *v);
                }
            }
            for i in 0..n {
                out[side][i] = (scratch[side][2 * i] + scratch[side][2 * i + 1]) * 0.5;
            }
        }
    }
}

impl Effect for Vocodex {
    /// FL reads nothing of a version of 1 or less, or of `0x80004` or more,
    /// and keeps what it has; so does this.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let Some(state) = State::read(bytes)? else {
            return Ok(());
        };
        self.state = state;
        let mut sg = [0u8; 12];
        sg[..4].copy_from_slice(&3i32.to_le_bytes());
        sg[8..].copy_from_slice(&self.state.params[1].to_le_bytes());
        // Preset A, which is what `Data/SG.dat` holds.
        self.soundgoodizer.set_state(&sg)?;
        self.load_synth();
        self.build();
        Ok(())
    }

    /// The bands are laid out anew for the rate. Asks for memory.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.hz = f64::from(hz);
            self.soundgoodizer.set_sample_rate(hz);
            if let Some(synth) = &mut self.synth {
                synth.set_sample_rate(hz);
            }
            self.build();
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if let Some(synth) = &mut self.synth {
            synth.set_tempo(bpm);
        }
    }

    /// Where the carrier synthesizer's presets are: `Effects/Vocodex` next
    /// to the generators. Reads the one the state names.
    fn set_resource_dir(&mut self, dir: &Path) {
        self.fruity = dir.parent().map(Path::to_path_buf);
        self.load_synth();
        self.build();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let mut at = 0;
        while at < len {
            let n = (len - at).min(CHUNK);
            self.chunk(at, &mut left[at..at + n], &mut right[at..at + n]);
            at += n;
        }
        self.mod_side.clear();
        self.car_side.clear();
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.state.params.get(index).map_or(0.0, |v| *v as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.state.params[index] = knob_value(value, *range);
            self.retune();
        }
    }

    fn reset(&mut self) {
        for band in &mut self.bands {
            for set in &mut band.sets {
                for stage in &mut set.stages {
                    stage.z = Default::default();
                }
                set.follower.clear();
                set.smoother.y = [0.0; 4];
                set.last = 0.0;
            }
        }
        if let Some(contour) = &mut self.contour {
            contour.consonants.clear();
            contour.peak.clear();
            contour.smoother.y = [0.0; 4];
        }
        self.mod_pass.clear();
        self.car_pass.clear();
        self.noise_pass.clear();
        self.ups = Default::default();
        self.countdown = self.sleep_after.unwrap_or(0);
        self.wet.now = self.wet.target;
        self.mod_level.now = self.mod_level.target;
        self.car_level.now = self.car_level.target;
        self.soundgoodizer.reset();
        self.hold_keys();
    }

    fn fidelity(&self) -> Fidelity {
        let state = &self.state;
        if !state.tone.is_empty() && self.synth.is_none() {
            Fidelity::Approximate("the carrier synthesizer's preset was not found, so it is silent")
        } else if self.synth.is_some() {
            Fidelity::Approximate("the carrier synthesizer is this crate's Sytrus")
        } else if !state.maps[SATURATION_MIX].points.iter().all(|p| p.y == 0.0) {
            Fidelity::Approximate("band saturation is left out")
        } else if !state.maps.iter().all(Map::straight) {
            Fidelity::Approximate("a curved envelope segment is drawn straight")
        } else if self.noise != 0.0 {
            Fidelity::Approximate("the carrier's added noise is another generator's")
        } else {
            Fidelity::Exact
        }
    }

    /// The Soundgoodizer at the end, when it is on.
    fn latency(&self) -> usize {
        if self.state.params[1] > 0 {
            self.soundgoodizer.latency()
        } else {
            0
        }
    }

    fn max_latency(&self) -> usize {
        self.soundgoodizer.max_latency()
    }

    /// The modulator's input when it is a sidechain input, else the
    /// carrier's.
    fn sidechain_input(&self) -> usize {
        let [m, c] = self.sidechain_inputs();
        if m > 0 { m } else { c }
    }

    fn sidechain_inputs(&self) -> [usize; 2] {
        let state = &self.state;
        if state.switch(LR_ENCODING) {
            return [0, 0];
        }
        [
            state.modulator.max(0) as usize,
            state.carrier.max(0) as usize,
        ]
    }

    fn set_sidechain(&mut self, index: usize, left: &[f32], right: &[f32]) {
        if index == 0 {
            return;
        }
        let [m, c] = self.sidechain_inputs();
        if index == m {
            self.mod_side.keep(left, right);
        }
        if index == c {
            self.car_side.keep(left, right);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    const HZ: f32 = 48_000.0;

    /// A Vocodex at 48 kHz with no Soundgoodizer and no modulator passed
    /// through, so that only the bands are heard.
    fn bands_only() -> Vocodex {
        let mut plugin = Vocodex::default();
        plugin.set_sample_rate(HZ);
        plugin.set_param(1, 0.0);
        plugin.set_param(2, 0.0);
        plugin
    }

    /// A tone of the harmonics of `hz` up to 8 kHz, each at 1/k.
    fn buzz(hz: f64, len: usize) -> Vec<f32> {
        let harmonics = (8000.0 / hz) as usize;
        (0..len)
            .map(|n| {
                let t = n as f64 / f64::from(HZ);
                let sum: f64 = (1..=harmonics)
                    .map(|k| (std::f64::consts::TAU * hz * k as f64 * t).sin() / k as f64)
                    .sum();
                (sum * 0.3) as f32
            })
            .collect()
    }

    /// Run `carrier` through with `modulator` on sidechain 1, in blocks of
    /// 480, and hand back the left output.
    fn vocode(plugin: &mut Vocodex, carrier: &[f32], modulator: Option<&[f32]>) -> Vec<f32> {
        let (mut left, mut right) = (carrier.to_vec(), carrier.to_vec());
        for (at, (l, r)) in left.chunks_mut(480).zip(right.chunks_mut(480)).enumerate() {
            if let Some(m) = modulator {
                let m = &m[at * 480..at * 480 + l.len()];
                plugin.set_sidechain(1, m, m);
            }
            plugin.process(l, r);
        }
        left
    }

    #[test]
    fn the_frequency_map_runs_from_c2_to_21_khz_and_back() {
        let map = FreqMap::new();
        assert!((map.hz(1.0) - map.max).abs() < 1e-6);
        assert!((map.hz(map.knee) - C2).abs() < 1e-9);
        assert!((map.hz(0.0)).abs() < 1e-12);
        for hz in [30.0, 65.4, 100.0, 1000.0, 15_000.0] {
            assert!((map.hz(map.place(hz)) - hz).abs() < 1e-6, "{hz}");
        }
        // The default distribution, from E2 to 20 kHz, as the corpus has
        // it.
        let state = State::default();
        let points = &state.maps[DISTRIBUTION].points;
        assert!((points[0].y - 0.142_937_1).abs() < 1e-7);
        assert!((points[1].y - 0.991_756_4).abs() < 1e-7);
    }

    #[test]
    fn a_band_filter_passes_its_centre_and_cuts_a_decade_away() {
        let step = 2.0 * PI / f64::from(HZ);
        let c = Coefs::band(1000.0, 0.5, step);
        let (num, den) = ([c.b0, c.b1, c.b2], [1.0, c.a1, c.a2]);
        let at = |hz: f64| testing::response(&num, &den, hz / f64::from(HZ));
        assert!((at(1000.0) - 1.0).abs() < 1e-9);
        assert!(at(100.0) < 0.1 && at(10_000.0) < 0.1);
        // Half an octave wide: down 3 dB a quarter octave either side,
        // near enough for the bilinear transform at 1 kHz.
        let edge = at(1000.0 * 2f64.powf(0.25));
        assert!((edge - 0.5f64.sqrt()).abs() < 0.02, "{edge}");
    }

    #[test]
    fn the_pass_through_filters_are_a_highpass_and_a_lowpass() {
        let step = 2.0 * PI / f64::from(HZ);
        let mut high = Pass::default();
        // 55000 is 40 * 375^(55000/65536), about 5.8 kHz.
        high.tune(55_000, 0, true, step);
        let mut low = Pass::default();
        low.tune(28_000, 65_536, false, step);
        let gain = |pass: &Pass, hz: f64| {
            pass.coefs
                .iter()
                .map(|c| testing::response(&[c.b0, c.b1, c.b2], &[1.0, c.a1, c.a2], hz / 48_000.0))
                .product::<f64>()
        };
        assert!(gain(&high, 15_000.0) > 0.9 && gain(&high, 1000.0) < 0.01);
        assert!(gain(&low, 100.0) > 0.9 && gain(&low, 5000.0) < 0.01);
        let mut off = Pass::default();
        off.tune(0, 0, true, step);
        assert!(!off.on);
    }

    #[test]
    fn the_upsampler_keeps_a_low_tone_and_its_level() {
        let mut up = Upsampler::default();
        let mut out = Vec::new();
        for v in testing::sine(0.01, 2000) {
            out.extend(up.step(0, v));
        }
        // At twice the rate the tone turns half as often a sample.
        let level = testing::partial(&out[2000..4000], 0.005);
        assert!((level - 1.0).abs() < 0.01, "{level}");
    }

    #[test]
    fn the_peak_follower_holds_the_highest_of_its_window() {
        let mut follower = Follower::new(false, 3);
        let input = [0.1f32, 0.5, 0.2, 0.3, 0.1, 0.0, 0.0, 0.9];
        let out: Vec<f32> = input.iter().map(|v| follower.step(*v, -*v * 0.5)).collect();
        assert_eq!(out, [0.1, 0.5, 0.5, 0.5, 0.3, 0.3, 0.1, 0.9]);
        let mut rms = Follower::new(true, 2);
        // A frame of 1 on both sides: (1 + 1) / (2 * 2), halved again by
        // the window not being full yet.
        assert!((rms.step(1.0, 1.0) - 0.5f32.sqrt()).abs() < 1e-6);
        assert!((rms.step(1.0, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_smoother_of_curve_1_is_one_stage_that_gets_99_percent_there() {
        let mut smoother = Smoother::default();
        smoother.tune(100.0, 1.0, 400.0, 1.0);
        // FL rounds 1.5 to 2 stages, and the second keeps nothing.
        assert_eq!(smoother.count, 2);
        assert_eq!((smoother.up[1], smoother.down[1]), (0.0, 0.0));
        let mut up = vec![1.0f32; 100];
        smoother.run(&mut up);
        assert!((up[99] - 0.99).abs() < 1e-4, "{}", up[99]);
        let mut down = vec![0.0f32; 400];
        smoother.run(&mut down);
        assert!((down[399] - 0.0099).abs() < 1e-4, "{}", down[399]);
        // Curve 2 is two stages, each over the root of 2 of the time.
        smoother.tune(100.0, 2.0, 100.0, 1.0);
        assert_eq!(smoother.count, 2);
        assert!((smoother.up[0] - crate::limiter::coef(100.0 / 2f64.sqrt())).abs() < 1e-12);
        assert!((smoother.up[1] - smoother.up[0]).abs() < 1e-12);
        assert_eq!(smoother.down[1], 0.0);
    }

    #[test]
    fn the_carrier_takes_the_modulator_s_spectrum() {
        let len = 48_000;
        let carrier = buzz(100.0, len);
        let modulator = testing::sine(1000.0 / f64::from(HZ), len);
        let mut plugin = bands_only();
        let out = vocode(&mut plugin, &carrier, Some(&modulator));
        let tail = &out[len - 4800..];
        let at = |hz: f64| testing::partial(tail, hz / f64::from(HZ));
        // The harmonics near 1 kHz come through, those far from it do not.
        let near = at(1000.0);
        assert!(near > 0.01, "{near}");
        for far in [200.0, 300.0, 4000.0, 6000.0] {
            assert!(at(far) < near * 0.05, "{far}: {} against {near}", at(far));
        }
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 4.0));
    }

    #[test]
    fn a_silent_modulator_silences_the_bands() {
        let len = 24_000;
        let carrier = buzz(100.0, len);
        let silence = vec![0.0f32; len];
        let mut plugin = bands_only();
        let out = vocode(&mut plugin, &carrier, Some(&silence));
        let peak = out[len / 2..].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak < 1e-6, "{peak}");
    }

    #[test]
    fn with_no_sidechain_block_the_modulator_is_the_own_input() {
        // The default modulator is sidechain 1; a host that has none hands
        // no block, and the plugin vocodes its input by itself.
        let len = 24_000;
        let carrier = buzz(150.0, len);
        let mut plugin = bands_only();
        assert_eq!(plugin.sidechain_inputs(), [1, 0]);
        assert_eq!(plugin.sidechain_input(), 1);
        let out = vocode(&mut plugin, &carrier, None);
        let level = testing::partial(&out[len - 4800..], 150.0 / f64::from(HZ));
        assert!(level > 0.01, "{level}");
    }

    #[test]
    fn l_r_encoding_takes_the_modulator_from_the_left() {
        let mut state = State::default();
        state.switches[LR_ENCODING] = 1;
        state.params[1] = 0;
        state.params[2] = 0;
        let mut plugin = Vocodex::default();
        plugin.set_state(&state.write()).unwrap();
        plugin.set_sample_rate(HZ);
        assert_eq!(plugin.sidechain_inputs(), [0, 0]);
        let len = 24_000;
        let mut left = testing::sine(1000.0 / f64::from(HZ), len);
        let mut right = buzz(100.0, len);
        for (l, r) in left.chunks_mut(480).zip(right.chunks_mut(480)) {
            plugin.process(l, r);
        }
        let near = testing::partial(&left[len - 4800..], 1000.0 / f64::from(HZ));
        let far = testing::partial(&left[len - 4800..], 3000.0 / f64::from(HZ));
        assert!(near > 0.01 && far < near * 0.05, "{near} {far}");
    }

    #[test]
    fn a_written_state_reads_back_and_a_bad_one_is_turned_down() {
        let state = State {
            tone: "PWM".to_owned(),
            keys: vec![51, 73, 63, 66],
            carrier: -1,
            ..State::default()
        };
        let bytes = state.write();
        assert_eq!(State::read(&bytes).unwrap(), Some(state));
        assert!(State::read(&bytes[..bytes.len() - 1]).is_err());
        // A version FL reads nothing of leaves the plugin as it was.
        let mut plugin = Vocodex::default();
        assert_eq!(plugin.set_state(&1i32.to_le_bytes()), Ok(()));
        assert_eq!(plugin.state, State::default());
    }

    #[test]
    fn the_parameters_are_the_knobs_of_fl_s_default() {
        let mut plugin = Vocodex::default();
        assert_eq!(plugin.param_count(), 20);
        let params: Vec<f32> = (0..20).map(|n| plugin.param(n)).collect();
        assert_eq!(params, DEFAULTS.map(|v| v as f32));
        plugin.set_param(17, -5000.0);
        assert_eq!(plugin.param(17), -1200.0);
        assert_eq!(plugin.param(20), 0.0);
    }

    #[test]
    fn every_vocodex_state_in_fl_s_files_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the Vocodex states skipped");
            return;
        };
        let presets = testing::every_preset(&root, NAME);
        for saved in &presets {
            let state = State::read(&saved.state).unwrap().unwrap();
            if saved.file == NAME {
                assert_eq!(state, State::default());
            }
            eprintln!("preset {}: {} bytes", saved.file, saved.state.len());
        }
        let mut inputs = Vec::new();
        let saved = testing::projects(&root);
        let saved: Vec<_> = saved.iter().filter(|s| s.plugin == NAME).collect();
        assert_eq!(saved.len(), 12);
        let fruity = root.parent().unwrap().join("Plugins/Fruity");
        for saved in saved {
            let state = State::read(&saved.state).unwrap().unwrap();
            assert_eq!(state.write(), saved.state, "{}", saved.file);
            let mut plugin = Vocodex::default();
            plugin.set_sample_rate(HZ);
            plugin.set_state(&saved.state).unwrap();
            if fruity.is_dir() {
                plugin.set_resource_dir(&fruity.join("Generators"));
            }
            inputs.push((saved.file.clone(), plugin.sidechain_inputs()));
            let len = 24_000;
            let carrier = buzz(110.0, len);
            let modulator = testing::sine(700.0 / f64::from(HZ), len);
            let out = vocode(&mut plugin, &carrier, Some(&modulator));
            let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!(out.iter().all(|v| v.is_finite()), "{}", saved.file);
            assert!(peak > 1e-3 && peak < 8.0, "{}: {peak}", saved.file);
            eprintln!("{}: peak {peak:.3} {:?}", saved.file, plugin.fidelity());
        }
        let find = |file: &str| inputs.iter().find(|(f, _)| f.starts_with(file)).unwrap().1;
        assert_eq!(find("Olbaid"), [2, 1]);
        assert_eq!(find("JuiceBass"), [2, 1]);
        assert_eq!(find("SeamlessR - Bass"), [1, 2]);
        assert_eq!(find("Treben - Gimbal"), [0, 2]);
        assert_eq!(find("Vocodex demo"), [1, 0]);
        assert_eq!(find("Ookay"), [0, 0]);
    }
}
