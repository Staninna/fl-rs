//! Kepler: FL's Juno-60, as `docs/plugins/kepler.md` reads it out of
//! `Kepler_x64.dll` and its `engine_x64.dll`.
//!
//! The shell is a Delphi panel that hands everything to the engine, which
//! is Kepler Exo's: 258 parameters from 0 to 1, of which a "Kepler-1"
//! state sets 36 and leaves the rest at the engine's defaults. So DCO 2,
//! the second LFO and envelope, the mod matrix, EQ, saturation, delay,
//! reverb and the other filter types never sound here, and are not built.
//!
//! # The state
//!
//! A shell header of 12 bytes, then the engine's stream: an i32 length and
//! "Kepler-1", an i32 version (1 to 4), an i32 count and that many f32,
//! the parameters in Kepler-1's order ([`KEPLER1`]). From version 2: a
//! byte, the velocity to volume (engine parameter 0x49) and the sub
//! oscillator's phase (0x0b) as f32. From version 3: three bytes, the last
//! two of which turn the chorus noise on and the oversampling from 2 to 4.
//!
//! # A note
//!
//! FL's note becomes a MIDI note: the key, and a velocity of
//! `int(ln(10 * gain + 1) * 41.714)` out of 127, with its pan and fine pitch
//! as expressions. Eight voices, each the free one that has rested longest
//! or else the oldest. Every frame the voice works out its two envelopes,
//! its pitch and its cutoff; then for each of 2 (or 4) oversampled steps its
//! DCO, a leaky integrator of band-limited steps (saw, pulse, sub), and the
//! engine's recorded noise go through a four-pole ladder with a rational
//! tanh in every stage, into the VCA and the pan. A polyphase halfband
//! takes the sum down to the host's rate, then the highpass, the chorus
//! and the main volume follow.
//!
//! The arpeggiator steps through the held notes up, up and down, or down,
//! over one, two or four octaves, at a rate in seconds or in beats.
//!
//! # FL's files
//!
//! The noise is a recording of 682245 samples in the engine DLL's data, and
//! LFO 1 a recorded wave of 2170. [`Generator::set_resource_dir`] reads
//! both from [`DLL`]. Without them the noise is silent and LFO 1 is a
//! sine, and [`Generator::fidelity`] says so.
//!
//! What is not FL's:
//!
//! - The step table is normalised by an exact reciprocal; FL's uses
//!   `rcpps` and one Newton step, which differ by CPU.
//! - The crossfade of 15 ms when the chorus mode changes is left out.
//! - FL's `FPE_MIDI_Vol` and `FPE_MIDI_Pan` are not read: the engine's
//!   output gain stays at the 6.309 it starts with, and the host puts the
//!   channel's volume on after.
//! - The song position the LFO and arpeggiator sync to is counted from the
//!   first frame the plugin plays.
//! - Hold, the chorus noise and automation smoothing are not built.

use crate::{Fidelity, Generator, NoteOn, StateError};
use std::f64::consts::PI;
use std::path::Path;

/// The plugin's event 201 name.
pub const NAME: &str = "Kepler";

/// The engine's DLL, from the directory [`Generator::set_resource_dir`] is
/// handed.
pub const DLL: &str = "Kepler/engine_x64.dll";

/// The engine's parameters.
pub const PARAMS: usize = 0x102;

/// Kepler-1's 36 parameters, as engine parameters (`FUN_180016300`).
pub const KEPLER1: [usize; 36] = [
    0x55, 0x59, 0x58, 0x56, 0x57, 0x30, 0x31, 0x32, 0x33, 0x51, 0x00, 0x05, 0x06, 0x07, 0x08, 0x09,
    0x0a, 0x22, 0x24, 0x25, 0x26, 0x2b, 0x2c, 0x2d, 0x27, 0x2f, 0x3d, 0x3e, 0x3f, 0x40, 0x45, 0x52,
    0x53, 0x48, 0x34, 0x35,
];

/// Where the noise and LFO 1's wave are in the engine DLL's image.
const NOISE_AT: u64 = 0x1_8010_5cc0;
const NOISE_LEN: usize = 0xa6905;
const WAVE_AT: u64 = 0x1_8025_2ed0;
const WAVE_LEN: usize = 2170;

/// The voices and the taps of a band-limited step.
const VOICES: usize = 8;
const TAPS: usize = 32;
const ROWS: usize = 1025;

/// 11-point curves of the engine, each read between its points at 10 v.
const DECAY: [f32; 11] = [
    0.0015, 0.0027, 0.043, 0.141, 0.328, 0.651, 1.144, 2.053, 3.956, 8.466, 10.124,
];
const CUTOFF: [f32; 11] = [
    2.0,
    2.459_431_6,
    core::f32::consts::LOG2_10,
    4.321_928,
    5.683_696_3,
    6.954_196_5,
    8.543_032,
    10.471_675,
    12.388_018,
    15.163_492,
    15.609_64,
];
const LFO_DCO: [f32; 11] = [
    0.0,
    0.001_163_933,
    0.008_127_881,
    0.015_058_375,
    0.021_955_734,
    0.028_820_276,
    0.052_592_173,
    0.078_186_356,
    0.120_573_66,
    0.177_280_77,
    0.213_883_88,
];
const RELEASE: [f32; 11] = [
    0.001, 0.002, 0.02, 0.112, 0.321, 0.572, 1.063, 1.964, 3.38, 6.91, 10.967,
];
const LFO_CUTOFF: [f32; 11] = [
    0.0,
    0.091_396_33,
    0.380_383_43,
    0.662_003_5,
    0.968_460_2,
    1.237_315_9,
    1.482_488_9,
    1.857_351_2,
    2.275_535,
    2.664_406,
    2.82,
];
const ATTACK: [f32; 11] = [
    0.003, 0.004, 0.013, 0.05, 0.128, 0.243, 0.401, 0.7, 1.3, 2.403, 3.049,
];
const LEVEL: [f32; 11] = [
    0.0, 0.01, 0.02, 0.05, 0.09, 0.14, 0.23, 0.46, 0.68, 0.92, 1.0,
];
const ENV_CUTOFF: [f32; 11] = [
    0.0,
    0.389_397_68,
    1.681_163_8,
    2.887_912_8,
    4.016_843,
    5.039_211,
    6.068_635,
    7.287_973,
    8.475_396,
    8.956_633,
    8.972_286,
];
const LFO_DELAY: [f32; 11] = [
    0.0, 0.229, 0.447, 0.547, 0.833, 1.0, 2.074, 4.775, 5.612, 6.774, 6.706,
];
const ARP_RATE: [f32; 11] = [
    0.811, 0.754, 0.575, 0.41, 0.211, 0.144, 0.102, 0.077, 0.053, 0.03, 0.021,
];
const LFO_RATE: [f32; 11] = [
    7.442_222,
    5.807_355,
    4.349_882,
    3.656_347,
    3.113_062_6,
    2.671_194_8,
    2.046_293_7,
    1.309_328,
    0.851_477_5,
    0.283_792_97,
    0.0,
];

/// The arpeggiator's steps in 96ths of a beat when it keeps time.
const ARP_SYNC: [i32; 13] = [96, 72, 64, 48, 36, 32, 24, 18, 16, 12, 9, 8, 6];

/// The halfband's twelve allpass coefficients, first branch then second.
const HALFBAND: [[f32; 6]; 2] = [
    [
        0.036_681_503,
        0.274_631_77,
        0.561_099,
        0.769_741_83,
        0.892_260_8,
        0.962_094_55,
    ],
    [
        0.136_547_63,
        0.423_138_62,
        0.677_540_06,
        0.839_889_65,
        0.931_542,
        0.987_816_4,
    ],
];

/// The Qs of the 16-pole Butterworth the step table is the impulse of.
const BUTTERWORTH: [f64; 8] = [
    0.50241929, 0.52249861, 0.56694403, 0.64682178, 0.78815462, 1.0606777, 1.7224471, 5.1011486,
];

/// A curve read at `v` (`FUN_180022840` and the rest): between points
/// `int(10 v)` and the next, the last point from 10 on.
fn curve(table: &[f32; 11], v: f32) -> f32 {
    let x = v * 10.0;
    let i = x as i32;
    if i < 10 {
        let f = x - i as f32;
        let i = i.max(0) as usize;
        (1.0 - f) * table[i] + f * table[i + 1]
    } else {
        table[10]
    }
}

/// The engine's defaults (`FUN_18001d900`).
fn default(index: usize) -> f32 {
    match index {
        3 | 8 | 10 | 0xf | 0x11 | 0x17 | 0x20 | 0x21 | 0x25 | 0x28 | 0x49 | 0x5a | 0x9a => 1.0,
        0xb
        | 0xd
        | 0x12
        | 0x2f
        | 0x30
        | 0x3e
        | 0x42
        | 0x43
        | 0x48
        | 0x4a
        | 0x53
        | 0x5c..=0x63
        | 0x65..=0x6d
        | 0x95
        | 0x96
        | 0x100
        | 0x101 => 0.5,
        0xe | 0x36 | 0x3f => 0.33,
        _ => 0.0,
    }
}

/// The rational tanh of the filter: Pade 7/6 on [-5, 5].
fn tanh(x: f32) -> f32 {
    let x = x.clamp(-5.0, 5.0);
    let x2 = x * x;
    (((x2 + 378.0) * x2 + 17_325.0) * x2 + 135_135.0) * x
        / (((x2 * 28.0 + 3150.0) * x2 + 62_370.0) * x2 + 135_135.0)
}

/// The step table (`FUN_180014a20`, `FUN_180015130`): 1025 rows of 32
/// taps, row `r` for a step `r / 1024` of a sample after the start of one.
pub fn step_table() -> Vec<[f32; TAPS]> {
    // The impulse response of the cascade at 1024 times the rate, from
    // 3/4 of 2048 samples on.
    let (half, len) = (2048usize, 32_768usize);
    let skip = 3 * half / 4;
    let (sin, cos) = (PI / half as f64).sin_cos();
    let stages: Vec<[f64; 5]> = BUTTERWORTH
        .iter()
        .map(|q| {
            let alpha = 0.5 / q * sin;
            let a0 = alpha + 1.0;
            let b = 1.0 - cos;
            [
                b * 0.5 / a0,
                b / a0,
                b * 0.5 / a0,
                -2.0 * cos / a0,
                (1.0 - alpha) / a0,
            ]
        })
        .collect();
    let mut state = [[0.0f64; 4]; 8];
    let mut ir = vec![0.0f32; len + skip];
    let mut peak = 0.0f64;
    for (n, out) in ir.iter_mut().enumerate() {
        let mut x = if n == 0 { 1.0 } else { 0.0 };
        for (c, s) in stages.iter().zip(state.iter_mut()) {
            let y = c[0] * x + c[1] * s[0] + c[2] * s[1] - c[3] * s[2] - c[4] * s[3];
            *s = [x, s[0], y, s[2]];
            x = y;
        }
        peak = peak.max(x);
        *out = x as f32;
    }
    let gain = 0.5f32 / peak as f32;
    let mut table: Vec<f32> = ir[skip..].iter().map(|v| v * gain).collect();
    for (i, v) in table[len - 1024..].iter_mut().enumerate() {
        *v *= 1.0 - i as f32 / 1024.0;
    }
    (0..ROWS)
        .map(|r| {
            std::array::from_fn(|j| {
                let at = 1024 * j as i64 - r as i64;
                if at < 0 { 0.0 } else { table[at as usize] }
            })
        })
        .collect()
}

/// The engine DLL's noise and LFO wave.
#[derive(Debug, Clone, PartialEq)]
pub struct Data {
    pub noise: Vec<i16>,
    /// 2171 points, the last the first again, from -1 to 1.
    pub wave: Vec<f32>,
}

impl Data {
    /// From the bytes of `engine_x64.dll`. `None` for another file.
    pub fn from_dll(bytes: &[u8]) -> Option<Self> {
        let i16s = |at: u64, n: usize| -> Option<Vec<i16>> {
            let raw = image(bytes, at, 2 * n)?;
            Some(
                raw.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| i16::from_le_bytes(*b))
                    .collect(),
            )
        };
        let noise = i16s(NOISE_AT, NOISE_LEN)?;
        let raw = i16s(WAVE_AT, WAVE_LEN)?;
        // Centred between its extremes and scaled to them
        // (`FUN_180013d30`).
        let scaled: Vec<f32> = raw.iter().map(|s| f32::from(*s) * 3.051_757_8e-5).collect();
        let high = scaled.iter().fold(0.0f32, |m, v| m.max(*v));
        let low = scaled.iter().fold(0.0f32, |m, v| m.min(*v));
        let mid = (low + high) * 0.5;
        let scale = 1.0 / (high - mid);
        if !scale.is_finite() {
            return None;
        }
        let mut wave: Vec<f32> = scaled.iter().map(|v| (v - mid) * scale).collect();
        wave.push(wave[0]);
        Some(Data { noise, wave })
    }
}

/// `len` bytes at an address of a 64-bit DLL's image.
fn image(bytes: &[u8], address: u64, len: usize) -> Option<&[u8]> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?));
    let header = u32_at(0x3c)? as usize;
    let optional = header.checked_add(24)?;
    if bytes.get(header..header + 4)? != b"PE\0\0" || u16_at(optional)? != 0x20b {
        return None;
    }
    let base = u64::from_le_bytes(*bytes.get(optional + 24..)?.first_chunk()?);
    let rva = address.checked_sub(base)?;
    let table = optional + usize::from(u16_at(header + 20)?);
    (0..usize::from(u16_at(header + 6)?)).find_map(|k| {
        let at = table + 40 * k;
        let (start, size, raw) = (u32_at(at + 12)?, u32_at(at + 16)?, u32_at(at + 20)?);
        let into = rva.checked_sub(u64::from(start))?;
        let end = into.checked_add(len as u64)?;
        if end > u64::from(size) {
            return None;
        }
        let from = raw as usize + into as usize;
        bytes.get(from..from + len)
    })
}

/// The DCO (`FUN_180014c00`, `FUN_180014f80`, `FUN_180014da0`,
/// `FUN_180014d00`).
#[derive(Debug, Clone, Copy)]
struct Dco {
    /// Where in a turn the sub oscillator flips, and the pulse falls.
    sub_at: f32,
    width: f32,
    saw: f32,
    pulse: f32,
    sub: f32,
    /// The saw and pulse levels as the last turn began.
    saw_then: f32,
    pulse_then: f32,
    inc: f64,
    phase: f64,
    integral: f32,
    leak: f32,
    ring: [f32; TAPS],
    at: usize,
    fell: bool,
    flipped: bool,
    sub_state: f32,
    out: f32,
}

impl Default for Dco {
    fn default() -> Self {
        Dco {
            sub_at: 0.5,
            width: 0.0,
            saw: 1.0,
            pulse: 0.0,
            sub: 1.0,
            saw_then: 0.0,
            pulse_then: 0.0,
            inc: 0.01,
            phase: 0.0,
            integral: 0.5,
            leak: 0.999,
            ring: [0.0; TAPS],
            at: 0,
            fell: false,
            flipped: false,
            sub_state: 1.0,
            out: 0.0,
        }
    }
}

impl Dco {
    /// A step of `amount` at `frac` of the sample (`FUN_180014d00`).
    fn step(&mut self, table: &[[f32; TAPS]], frac: f32, amount: f32) {
        let row = (frac.clamp(0.0, 1.0) * 1024.0) as usize;
        let taps = &table[row.min(ROWS - 1)];
        for (k, slot) in self.ring.iter_mut().enumerate() {
            *slot += taps[(k + TAPS - self.at) % TAPS] * amount;
        }
    }

    /// The edges of this sample (`FUN_180014da0`), `old` the phase before.
    fn edges(&mut self, table: &[[f32; TAPS]], old: f32) {
        let inc = self.inc as f32;
        if !self.fell && f64::from(self.width) <= self.phase {
            let x = self.width.max(old);
            self.fell = true;
            self.step(table, (x - old) / inc, -self.pulse_then);
        }
        if !self.flipped && f64::from(self.sub_at) <= self.phase {
            self.flipped = true;
            self.sub_state = -self.sub_state;
            self.step(table, (self.sub_at - old) / inc, self.sub_state * self.sub);
        }
        let turns = self.phase as i32;
        if turns != 0 {
            self.saw_then = self.saw;
            self.pulse_then = self.pulse;
            self.phase -= f64::from(turns);
            let frac = (1.0 - self.phase / self.inc) as f32;
            self.step(table, frac, self.saw + self.pulse);
            self.fell = false;
            self.flipped = false;
            if f64::from(self.width) <= self.phase {
                self.fell = true;
                self.step(table, (self.width + 1.0 - old) / inc, -self.pulse_then);
            }
            if f64::from(self.sub_at) <= self.phase {
                self.flipped = true;
                self.sub_state = -self.sub_state;
                self.step(
                    table,
                    (self.sub_at + 1.0 - old) / inc,
                    self.sub_state * self.sub,
                );
            }
        }
    }

    /// One oversampled step (`FUN_180014f80`, free running).
    fn next(&mut self, table: &[[f32; TAPS]]) -> f32 {
        let old = self.phase;
        self.phase += self.inc;
        self.edges(table, old as f32);
        let y = self.integral - self.inc as f32 * self.saw;
        let y = (y + self.ring[self.at]) * self.leak;
        self.integral = y;
        self.out = (y + y) - self.saw;
        self.ring[self.at] = 0.0;
        self.at = (self.at + 1) & (TAPS - 1);
        self.out
    }
}

/// One lane of filter type 0 (`FUN_180013260`), and its coefficients.
#[derive(Debug, Clone, Copy, Default)]
struct Ladder {
    cutoff: f32,
    res: f32,
    input: f32,
    out: f32,
    g: f32,
    h: f32,
    k: f32,
    gain: f32,
    norm: f32,
    s: [f32; 4],
}

impl Ladder {
    /// The coefficients of a frame, at `pi_t` (pi over the oversampled
    /// rate) and `most` Hz.
    fn design(&mut self, pi_t: f32, most: f32) {
        let x = self.cutoff.min(most).max(20.0) * pi_t;
        let g = (2.471_688_5 - x * x * 0.189_759_69) * x / (2.467_401 - x * x);
        self.h = 1.0 / (g + 1.0);
        self.g = self.h * g;
        let g4 = self.g * self.g * self.g * self.g;
        self.k = self.res * 4.2;
        self.gain = self.res + 1.0;
        self.norm = 1.0 / (self.k * g4 + 1.0);
    }

    /// One oversampled step, with the output of the fourth pole.
    fn run(&mut self) {
        let (g, h) = (self.g, self.h);
        let [s1, s2, s3, s4] = self.s;
        let fb = ((g * h * s1 + s2 * h) * g + s3 * h) * g + s4 * h;
        let u = (self.gain * self.input - fb * self.k) * 0.364_298_73;
        let v1 = tanh(u) * 2.745 * self.norm;
        let t = tanh(v1 - s1) * g;
        self.s[0] = t + t + s1;
        let y1 = g * v1 + h * s1;
        let t = tanh(y1 - s2) * g;
        self.s[1] = t + t + s2;
        let y2 = g * y1 + s2 * h;
        let t = tanh(y2 - s3) * g;
        self.s[2] = t + t + s3;
        let y3 = g * y2 + s3 * h;
        let t = tanh(y3 - s4) * g;
        self.s[3] = t + t + s4;
        self.out = g * y3 + s4 * h;
    }
}

/// One voice (`FUN_180021e80` and the rest).
#[derive(Debug, Clone, Copy)]
struct Voice {
    /// The key, -1 for none, and the frames since the voice started.
    key: i32,
    age: i32,
    gate: bool,
    env: f32,
    stage: u8,
    decayed: f32,
    /// The velocity gain, its target and the VCA's scale of it.
    gain: f32,
    target: f32,
    scaled: f32,
    glide: f32,
    to: f32,
    fine: f32,
    pan: (f32, f32),
    panned: (f32, f32),
    fresh: bool,
    vca: f32,
    noise_at: usize,
    dco: Dco,
    ladder: Ladder,
}

impl Voice {
    fn new(k: usize) -> Self {
        Voice {
            key: -1,
            age: 0,
            gate: false,
            env: 0.0,
            stage: 2,
            decayed: 1.0,
            gain: 0.0,
            target: 0.0,
            scaled: 0.0,
            glide: 0.0,
            to: 0.0,
            fine: 0.0,
            pan: (1.0, 1.0),
            panned: (0.0, 0.0),
            fresh: true,
            vca: 0.0,
            noise_at: (k as f64 * 85_280.625) as usize,
            dco: Dco::default(),
            ladder: Ladder::default(),
        }
    }
}

/// The halfband decimator (`FUN_18001cd80`): per lane (left and right of
/// each branch) the last two inputs of every one of its six allpasses.
#[derive(Debug, Clone, Copy, Default)]
struct Halfband {
    /// `y[k]` is the output of allpass `k`, `y[0]` the input; each the
    /// sample before and the one before that.
    y: [[[f32; 2]; 7]; 4],
}

impl Halfband {
    fn lane(&mut self, lane: usize, x: f32) -> f32 {
        let coefs = &HALFBAND[lane / 2];
        let h = &mut self.y[lane];
        let mut input = x;
        for (k, c) in coefs.iter().enumerate() {
            let y = (input - h[k + 1][1]) * c + h[k][1];
            h[k] = [input, h[k][0]];
            input = y;
        }
        // The last allpass's history.
        h[6] = [input, h[6][0]];
        input
    }

    /// One pair in, the output of its second sample.
    fn push(&mut self, l: f32, r: f32) -> (f32, f32) {
        let late = (self.y[2][6][0], self.y[3][6][0]);
        let a = (self.lane(0, l), self.lane(1, r));
        self.lane(2, l);
        self.lane(3, r);
        ((a.0 + late.0) * 0.5, (a.1 + late.1) * 0.5)
    }
}

/// A one-pole of `FUN_1800025c0`: `y = b0 x + b1 x1 + c y1`.
#[derive(Debug, Clone, Copy, Default)]
struct OnePole {
    b0: f32,
    b1: f32,
    c: f32,
    x1: f32,
    y1: f32,
}

impl OnePole {
    fn set(&mut self, hz: f32, pi_t: f32, high: bool) {
        let t = tanf(hz * pi_t);
        let a = 1.0 / (t + 1.0);
        if high {
            (self.b0, self.b1) = (a, -a);
        } else {
            (self.b0, self.b1) = (a * t, a * t);
        }
        self.c = -(a * (t - 1.0));
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.c * self.y1;
        (self.x1, self.y1) = (x, y);
        y
    }
}

fn tanf(x: f32) -> f32 {
    f64::from(x).tan() as f32
}

/// The chorus (`FUN_180001770`, `FUN_180001a60`).
struct Chorus {
    mode: i32,
    sine: bool,
    inc: f32,
    phase: f32,
    offset: f32,
    center: f32,
    depth: f32,
    smooth: f32,
    delay: (f32, f32),
    input: [OnePole; 2],
    output: [OnePole; 2],
    /// Two lowpass biquads per side: b0, b1, b2 and the feedback taps.
    biquad: [f32; 5],
    bq: [[[f32; 4]; 2]; 2],
    lines: [Vec<f32>; 2],
    at: [usize; 2],
    mix: f32,
    level: f32,
    rate: f32,
}

impl Chorus {
    fn new(rate: f32) -> Self {
        let mut chorus = Chorus {
            mode: 0,
            sine: false,
            inc: 9.07e-6,
            phase: 0.0,
            offset: 0.5,
            center: 0.00108,
            depth: 0.0,
            smooth: 0.0,
            delay: (0.01, 0.01),
            input: [OnePole::default(); 2],
            output: [OnePole::default(); 2],
            biquad: [0.0; 5],
            bq: [[[0.0; 4]; 2]; 2],
            lines: [vec![0.0; 0x8000], vec![0.0; 0x8000]],
            at: [0; 2],
            mix: 0.0,
            level: 0.0,
            rate,
        };
        chorus.set_rate(rate, 50.0);
        chorus
    }

    /// The filters for a rate (`FUN_18001d300`), the input highpass at
    /// `high` Hz.
    fn set_rate(&mut self, rate: f32, high: f32) {
        self.rate = rate;
        let pi_t = (1.0 / f64::from(rate)) as f32 * std::f32::consts::PI;
        for side in 0..2 {
            self.input[side].set(high, pi_t, true);
            self.output[side].set(7000.0, pi_t, false);
        }
        // A cookbook lowpass at 7 kHz and Q 0.707 (`FUN_180002690` type 0).
        let w = (7000.0 * std::f32::consts::TAU) / rate;
        let (sin, cos) = f64::from(w).sin_cos();
        let (sin, cos) = (sin as f32, cos as f32);
        let alpha = (0.5 / 0.707) * sin;
        let a0 = 1.0 / (alpha + 1.0);
        let b1 = 1.0 - cos;
        let b0 = b1 * 0.5;
        self.biquad = [
            a0 * b0,
            a0 * b1,
            a0 * b0,
            -(a0 * (cos * -2.0)),
            -(a0 * (1.0 - alpha)),
        ];
        self.smooth = 1.0 - 0.1f64.powf(20.0 / f64::from(rate)) as f32;
    }

    /// A new mode, from the frame's switch (`FUN_18001b940`).
    fn set_mode(&mut self, mode: i32) {
        self.mode = mode;
        if mode == 0 {
            return;
        }
        if mode == 3 {
            self.inc = 8.0 / self.rate;
            self.sine = true;
            self.offset = 0.028_6;
            self.center = 0.002_85;
            self.depth = 0.000_52;
        } else {
            self.inc = if mode == 1 { 0.4 } else { 0.6 } / self.rate;
            self.sine = false;
            self.offset = 0.5;
            self.center = 0.001_08;
            self.depth = 0.004_15;
        }
        let rate = self.rate;
        self.set_rate(rate, 0.5);
    }

    fn lfo(&self, phase: f32) -> f32 {
        if self.sine {
            ((f64::from(phase * std::f32::consts::TAU).sin() as f32) + 1.0) * 0.5
        } else if phase < 0.25 {
            phase + phase + 0.5
        } else if phase < 0.75 {
            (1.5 - phase) - phase
        } else {
            ((phase - 0.75) + phase) - 0.75
        }
    }

    fn read(line: &[f32], at: usize, delay: f32) -> f32 {
        let mask = line.len() - 1;
        let i = delay as i32;
        let f = delay - i as f32;
        let pos = (at as i32 - i) as usize;
        let get = |k: usize| line[pos.wrapping_sub(k) & mask];
        let (y1, y2, y0, y3) = (get(1), get(2), get(0), get(3));
        let c1 = (y2 - y0) * 0.5;
        let d = y1 - y2;
        let c2 = d + c1;
        let c3 = (y3 - y1) * 0.5 + c2 + d;
        ((c3 * f - (c3 + c2)) * f + c1) * f + y1
    }

    fn run(&mut self, l: &mut f32, r: &mut f32) {
        let m = (*l + *r) * 0.5;
        (*l, *r) = (m, m);
        let p = self.phase;
        let q = (p + self.offset) - (p + self.offset) as i32 as f32;
        let (a, b) = (self.lfo(p), self.lfo(q));
        self.delay.0 += ((self.depth * a + self.center) - self.delay.0) * self.smooth;
        self.delay.1 += ((self.depth * b + self.center) - self.delay.1) * self.smooth;
        self.phase = (p + self.inc) - (p + self.inc) as i32 as f32;
        let ins = [self.input[0].run(*l), self.input[1].run(*r)];
        let mut wet = [0.0f32; 2];
        for side in 0..2 {
            let mut x = ins[side];
            for stage in 0..2 {
                let s = &mut self.bq[side][stage];
                let c = &self.biquad;
                let y = c[3] * s[2] + c[4] * s[3] + c[2] * s[1] + c[0] * x + c[1] * s[0];
                *s = [x, s[0], y, s[2]];
                x = y;
            }
            let mask = self.lines[side].len() - 1;
            self.lines[side][self.at[side]] = x;
            self.at[side] = (self.at[side] + 1) & mask;
            let delay = if side == 0 {
                self.delay.0
            } else {
                self.delay.1
            } * self.rate;
            wet[side] = Self::read(&self.lines[side], self.at[side], delay);
        }
        let wet = [self.output[0].run(wet[0]), self.output[1].run(wet[1])];
        self.mix += (0.5 - self.mix) * self.smooth;
        self.level += (1.0 - self.level) * self.smooth;
        let (w, d) = (self.mix.sqrt(), (1.0 - self.mix).sqrt());
        *l = (wet[0] * w + d * *l) * self.level;
        *r = (wet[1] * w + d * *r) * self.level;
    }
}

/// The arpeggiator (`FUN_180012680` and the rest).
#[derive(Debug, Clone)]
struct Arp {
    on: bool,
    mode: i32,
    octaves: i32,
    /// The held notes, as played and sorted, with their velocities.
    played: Vec<(i32, f32)>,
    up: Vec<(i32, f32)>,
    down: Vec<(i32, f32)>,
    index: i32,
    octave: i32,
    going: i32,
    sounding: i32,
    fire: bool,
    pending: u8,
    step: f64,
    gate: f64,
    next: f64,
    start: f64,
    last: f64,
    delta: f64,
    total: f64,
}

impl Arp {
    fn new() -> Self {
        Arp {
            on: false,
            mode: 0,
            octaves: 1,
            played: Vec::with_capacity(128),
            up: Vec::with_capacity(128),
            down: Vec::with_capacity(128),
            index: -1,
            octave: 0,
            going: 1,
            sounding: -1,
            fire: true,
            pending: 0,
            step: 0.5,
            gate: 0.5,
            next: 0.0,
            start: 0.0,
            last: 0.0,
            delta: 0.0,
            total: 0.0,
        }
    }

    fn restart(&mut self) {
        self.fire = true;
        self.index = -1;
        if self.mode == 2 {
            self.octave = self.octaves - 1;
            self.going = 2;
        } else {
            self.octave = 0;
            self.going = 1;
        }
    }

    fn add(&mut self, key: i32, velocity: f32, first: bool, synced: bool) -> bool {
        if self.up.iter().any(|(k, _)| *k == key) {
            return false;
        }
        if self.played.len() < self.played.capacity() {
            self.played.push((key, velocity));
            let at = self
                .up
                .iter()
                .position(|(k, _)| *k > key)
                .unwrap_or(self.up.len());
            self.up.insert(at, (key, velocity));
            let at = self
                .down
                .iter()
                .position(|(k, _)| *k <= key)
                .unwrap_or(self.down.len());
            self.down.insert(at, (key, velocity));
        }
        if first {
            self.pending = if synced { 2 } else { 1 };
            self.restart();
        }
        true
    }

    fn remove(&mut self, key: i32) {
        self.played.retain(|(k, _)| *k != key);
        self.up.retain(|(k, _)| *k != key);
        self.down.retain(|(k, _)| *k != key);
    }

    /// The next note (`FUN_180012d60`), modes 0 to 2.
    fn advance(&mut self) -> (i32, f32) {
        let (n_up, n_down) = (self.up.len() as i32, self.down.len() as i32);
        self.index += 1;
        if self.going == 2 {
            if n_down <= self.index {
                self.octave -= 1;
                self.index = 0;
                if self.octave < 0 {
                    if self.mode == 1 {
                        self.going = 1;
                        if n_up < 2 {
                            self.octave = (self.octaves - 1).min(1);
                        } else {
                            self.index = 1;
                            self.octave = 0;
                        }
                    } else {
                        self.octave = self.octaves - 1;
                    }
                }
            }
        } else if n_up <= self.index {
            self.octave += 1;
            self.index = 0;
            if self.octave >= self.octaves {
                if self.mode == 1 {
                    self.going = 2;
                    if n_down < 2 {
                        self.octave = (self.octaves - 2).max(0);
                    } else {
                        self.index = 1;
                        self.octave = self.octaves - 1;
                    }
                } else {
                    self.octave = 0;
                }
            }
        }
        let list = if self.going == 2 {
            &self.down
        } else {
            &self.up
        };
        let (key, velocity) = list
            .get(self.index.max(0) as usize)
            .copied()
            .unwrap_or((0, 0.0));
        ((key + self.octave * 12).clamp(0, 127), velocity)
    }
}

/// Kepler.
pub struct Kepler {
    params: [f32; PARAMS],
    version: i32,
    count: usize,
    oversample: usize,
    noise_on: bool,
    rate: f32,
    tempo: f64,
    seconds: f64,
    beats: f64,
    data: Option<Data>,
    table: Vec<[f32; TAPS]>,
    voices: [Voice; VOICES],
    held: Vec<(u32, i32)>,
    rng: u32,
    // LFO 1.
    lfo_phase: f64,
    lfo_inc: f64,
    lfo_cycles: f64,
    lfo_beat: f64,
    lfo_delay: f32,
    lfo_delay_inc: f32,
    lfo_gate: f32,
    lfo_tri: f32,
    lfo: f32,
    arp: Arp,
    halfband: [Halfband; 2],
    high: OnePole,
    high_r: OnePole,
    chorus: Chorus,
    volume: (f32, f32),
    smoothed: (f32, f32),
    smooth: f32,
    outer: f32,
}

impl Default for Kepler {
    fn default() -> Self {
        let mut kepler = Kepler {
            params: std::array::from_fn(default),
            version: 4,
            count: 36,
            oversample: 2,
            noise_on: false,
            rate: 44_100.0,
            tempo: 120.0,
            seconds: 0.0,
            beats: 0.0,
            data: None,
            table: step_table(),
            voices: std::array::from_fn(Voice::new),
            held: Vec::with_capacity(128),
            rng: 0,
            lfo_phase: 0.0,
            lfo_inc: 0.0,
            lfo_cycles: 0.0,
            lfo_beat: 0.0,
            lfo_delay: 1.0,
            lfo_delay_inc: 0.0,
            lfo_gate: 1.0,
            lfo_tri: 0.0,
            lfo: 0.0,
            arp: Arp::new(),
            halfband: [Halfband::default(); 2],
            high: OnePole::default(),
            high_r: OnePole::default(),
            chorus: Chorus::new(44_100.0),
            volume: (0.0, 0.0),
            smoothed: (0.0, 0.0),
            smooth: 0.0,
            outer: 6.309,
        };
        kepler.prepare();
        kepler
    }
}

impl Kepler {
    /// An engine parameter, from 0 to 1.
    pub fn param_of(&self, index: usize) -> f32 {
        self.params.get(index).copied().unwrap_or(0.0)
    }

    fn os_rate(&self) -> f64 {
        f64::from(self.rate) * self.oversample as f64
    }

    /// Everything the rate and the parameters give (`FUN_18001d300`,
    /// `FUN_18001d700`, `FUN_180019a20`).
    fn prepare(&mut self) {
        let rate = f64::from(self.rate);
        self.smooth = 1.0 - 0.1f64.powf(400.0 / rate) as f32;
        let mode = (self.params[0x45] * 3.0 + 0.5) as i32;
        if mode != self.chorus.mode || self.chorus.rate != self.rate {
            if self.chorus.rate != self.rate {
                self.chorus.set_rate(self.rate, 50.0);
            }
            self.chorus.set_mode(mode);
        }
        // The highpass.
        let hz = (f64::from(self.params[0x24]).powf(1.6) as f32) * 2680.0 + 20.0;
        let pi_t = (1.0 / rate) as f32 * std::f32::consts::PI;
        self.high.set(hz, pi_t, true);
        self.high_r.set(hz, pi_t, true);
        // The main volume and pan.
        let v = self.params[0x48];
        let pan = self.params[0x4a].clamp(0.0, 1.0);
        let g = ((0.327_543_14 - v * 1.392_291_4) * v - 0.349_678_93) * v;
        self.volume = ((1.0 - pan).sqrt() * g, pan.sqrt() * g);
        // LFO 1.
        let r = self.params[0x30];
        let hz = (2f64.powf(f64::from(-curve(&LFO_RATE, r))) as f32) * 21.739_13;
        self.lfo_inc = f64::from(hz * (1.0 / self.rate));
        let ticks = (((2f64.powf(f64::from(r * 7.0)) as f32) - 1.0) * 48.370_08 + 1.1) as i32;
        self.lfo_cycles = 192.0 / f64::from(ticks.max(1));
        let t = curve(&LFO_DELAY, self.params[0x32]);
        self.lfo_delay_inc = if t > 0.0 { 1.0 / (t * self.rate) } else { 1.0 };
        let manual = self.params[0x33] >= 0.5;
        self.lfo_gate = if manual && self.params[0x51] < 0.5 {
            0.0
        } else {
            1.0
        };
        // The arpeggiator.
        self.arp.on = self.params[0x55] >= 0.5;
        let m = self.params[0x59];
        self.arp.mode = if m < 0.333 {
            0
        } else if m < 0.666 {
            1
        } else {
            2
        };
        self.arp.octaves = 1 << ((self.params[0x58] * 2.0 + 0.5) as i32 & 0x1f);
        self.arp.step = if self.params[0x57] >= 0.5 {
            let at = (self.params[0x56] * 12.0 + 0.5) as usize;
            f64::from(ARP_SYNC[at.min(12)]) * (1.0 / 96.0)
        } else {
            f64::from(curve(&ARP_RATE, self.params[0x56]))
        };
        self.arp.gate = self.arp.step * f64::from(self.params[0x5a]);
    }

    fn synced(&self) -> bool {
        self.params[0x57] >= 0.5
    }

    /// A voice for a key (`FUN_180018680`).
    fn pick(&self, key: i32) -> (usize, bool) {
        if let Some(v) = self.voices.iter().position(|v| v.key == key) {
            return (v, true);
        }
        let mut best: Option<usize> = None;
        for (k, v) in self.voices.iter().enumerate() {
            if !v.gate && best.is_none_or(|b| self.voices[b].age < v.age) {
                best = Some(k);
            }
        }
        let oldest = || {
            (0..VOICES)
                .max_by_key(|k| (self.voices[*k].age, std::cmp::Reverse(*k)))
                .unwrap_or(0)
        };
        (best.unwrap_or_else(oldest), false)
    }

    /// A voice starts (`FUN_1800188a0`, `FUN_180022280`).
    fn start(&mut self, key: i32, velocity: f32, pan: f32, fine: f32) {
        let (slot, same) = self.pick(key);
        self.rng = self.rng.wrapping_mul(0xbb3_8435).wrapping_add(0x3619_636b);
        let vtv = self.params[0x49];
        let v = &mut self.voices[slot];
        v.gate = true;
        v.fresh = !same;
        v.to = (key - 69) as f32 * 0.083_333_336;
        v.glide = v.to;
        let curve = ((velocity * velocity * (velocity * 0.984_35)
            - 0.231_573 * velocity * velocity)
            + 0.247_223 * velocity)
            * vtv
            + (1.0 - vtv);
        v.target = curve;
        v.scaled = curve * 1.025_641_1;
        v.key = key;
        v.age = 0;
        v.stage = 0;
        v.decayed = 1.0;
        v.fine = fine * 0.000_833_333_35;
        let p = ((pan + 1.0) * 0.5).clamp(0.0, 1.0);
        v.pan = ((1.0 - p).sqrt() * 1.414_427_2, p.sqrt() * 1.414_427_2);
    }

    /// A voice lets go (`FUN_180018b30`).
    fn release(&mut self, key: i32) {
        for v in self.voices.iter_mut() {
            if v.gate && v.key == key {
                v.gate = false;
                v.stage = 2;
                v.target = 0.0;
            }
        }
    }

    fn key_on(&mut self, key: i32, velocity: f32, pan: f32, fine: f32) {
        let first = self.held.is_empty();
        if first && self.params[0x33] < 0.5 {
            self.lfo_delay = 0.0;
            self.lfo_beat = self.beats;
            if self.params[0x34] >= 0.5 {
                self.lfo_phase = f64::from(self.params[0x35].clamp(0.0, 0.9999));
            }
        }
        if self.arp.on {
            let synced = self.synced();
            self.arp.add(key, velocity, first, synced);
        } else {
            self.start(key, velocity, pan, fine);
        }
    }

    fn key_off(&mut self, key: i32) {
        if self.arp.on {
            self.arp.remove(key);
            if self.arp.up.is_empty() && self.arp.sounding >= 0 {
                let sounding = self.arp.sounding;
                self.release(sounding);
                self.arp.sounding = -1;
            }
        } else {
            self.release(key);
        }
    }

    /// The arpeggiator's clock for one frame (`FUN_180019210`).
    fn clock(&mut self) {
        if !self.arp.on || self.arp.up.is_empty() {
            return;
        }
        let now = if self.synced() {
            self.beats
        } else {
            self.seconds
        };
        if self.arp.last < now {
            self.arp.delta = now - self.arp.last;
        }
        self.arp.last = now;
        self.arp.total += self.arp.delta;
        loop {
            while self.arp.fire {
                if self.arp.pending != 0 {
                    self.arp.last = now;
                    self.arp.total = now;
                    if self.arp.pending == 2 {
                        let n = ((now - 0.001) / self.arp.step).round();
                        self.arp.start = n * self.arp.step;
                        self.arp.next = self.arp.start;
                    } else {
                        self.arp.start = now;
                        self.arp.next = now;
                    }
                    self.arp.pending = 0;
                }
                if self.arp.total < self.arp.next {
                    return;
                }
                let (key, velocity) = self.arp.advance();
                self.arp.sounding = key;
                self.start(key, velocity, 0.0, 0.0);
                self.arp.fire = false;
            }
            if self.arp.total < self.arp.next + self.arp.gate {
                return;
            }
            self.arp.next += self.arp.step;
            if self.arp.sounding >= 0 {
                let sounding = self.arp.sounding;
                self.release(sounding);
                self.arp.sounding = -1;
            }
            self.arp.fire = true;
        }
    }

    /// One frame of the host's rate (`FUN_18001b940`).
    fn frame(&mut self) -> (f32, f32) {
        let p = self.params;
        // LFO 1: the triangle, and the recorded wave the voices hear.
        let ph = self.lfo_phase as f32;
        self.lfo_tri = if ph < 0.25 {
            ph + ph
        } else if ph < 0.75 {
            0.5 - (((ph - 0.25) + ph) - 0.25)
        } else {
            ((ph - 0.75) + ph) - 1.25
        };
        let wave = match &self.data {
            Some(data) => {
                let x = ph * 2170.0;
                let i = (x as usize).min(WAVE_LEN - 1);
                let f = x - i as f32;
                ((1.0 - f) * data.wave[i] + f * data.wave[i + 1]) * 0.5
            }
            None => (f64::from(ph) * std::f64::consts::TAU).sin() as f32 * 0.5,
        };
        if p[0x31] >= 0.5 {
            let d = (self.beats - self.lfo_beat) * self.lfo_cycles;
            self.lfo_beat = self.beats;
            if (0.0..1.0).contains(&d) {
                self.lfo_inc = d;
            }
        }
        self.lfo_phase += self.lfo_inc;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
        }
        let ramp = self.lfo_delay * self.lfo_gate;
        self.lfo_delay = (self.lfo_delay + self.lfo_delay_inc).min(1.0);
        self.lfo = ramp * wave;
        let lfo_uni = self.lfo_tri + 0.5;

        // Each voice's modulation (`FUN_180023ab0`).
        let os = self.os_rate();
        let inc_scale = 1.0 / os;
        let pi_t = (1.0 / os) as f32 * std::f32::consts::PI;
        let most = (os * 0.4995) as f32;
        let rate = self.rate;
        let attack = {
            let t = curve(&ATTACK, p[0x3d]);
            if t != 0.0 {
                (1.0 - 0.411_764_722_381_908_1f64.powf(1.0 / (f64::from(rate) * f64::from(t))))
                    as f32
            } else {
                1.0
            }
        };
        let decay = (0.1f64.powf(f64::from(1.0 / (curve(&DECAY, p[0x3e]) * rate)))) as f32;
        let sustain = p[0x3f];
        let release = (0.1f64.powf(f64::from(1.0 / (curve(&RELEASE, p[0x40]) * rate)))) as f32;
        let pw_amount = p[5];
        let pw_source = p[6];
        let leak_most = 0.37f64.powf(10.0 / f64::from(rate)) as f32;
        let lfo_dco = curve(&LFO_DCO, p[0]) * if p[1] >= 0.5 { 10.0 } else { 1.0 };
        let octave = ((p[0x53] * 2.0 + 0.5) as i32) as f32 - 1.0;
        let base = 2f64.powf(f64::from(curve(&CUTOFF, p[0x25]))) as f32;
        let env_cut = curve(&ENV_CUTOFF, p[0x2c]);
        let lfo_cut = curve(&LFO_CUTOFF, p[0x2d]);
        let polarity = if p[0x2b] < 0.5 { 1.0 } else { -1.0 };
        let res = p[0x26].clamp(0.0, 1.0);
        let vel_smooth = 1.0 - 0.1f64.powf(400.0 / f64::from(rate)) as f32;
        let sub = if p[9] < 0.5 {
            0.0
        } else {
            curve(&LEVEL, p[10]) * 1.111_111_2
        };
        let noise = curve(&LEVEL, p[0x22]).max(0.01);
        for v in self.voices.iter_mut() {
            match v.stage {
                0 => {
                    v.env += (1.7 - v.env) * attack;
                    if v.env >= 1.0 {
                        v.env = 1.0;
                        v.stage = 1;
                    }
                }
                1 => {
                    v.decayed *= decay;
                    v.env = v.decayed * (1.0 - sustain) + sustain;
                }
                _ => {
                    v.env *= release;
                    if v.env < 1e-30 {
                        v.env = 0.0;
                    }
                }
            }
            let source = if pw_source < 0.333 {
                lfo_uni
            } else if pw_source < 0.666 {
                1.0
            } else {
                v.env
            };
            v.dco.width = 0.458 - source * 0.432 * pw_amount;
            v.glide = v.to;
            let pitch = octave + v.glide + v.fine;
            let lfo2 = self.lfo + self.lfo;
            let pitch = lfo2 * lfo_dco + pitch;
            let inc = (2f64.powf(f64::from(pitch) * 1.0005) * 440.0 * inc_scale).clamp(0.0, 0.25);
            v.dco.inc = inc;
            v.dco.leak = leak_most.min(1.0 - inc as f32 * 0.267_949);
            let keys = (v.key - 24) as f32 * 0.083_333_336 * p[0x27];
            let c = keys + polarity * v.env * env_cut + lfo2 * lfo_cut;
            let hz = (2f64.powf(f64::from(c)) as f32 * base).clamp(5.0, 30_000.0);
            v.ladder.cutoff = hz;
            v.ladder.res = res;
            v.age += 1;
            v.gain += (v.target - v.gain) * vel_smooth;
            if v.gain.abs() < 1e-30 {
                v.gain = 0.0;
            }
            v.dco.sub_at = p[0xb] * 0.99;
            v.dco.saw = if p[8] < 0.5 { 0.0 } else { 1.0 };
            v.dco.pulse = if p[7] < 0.5 { 0.0 } else { 1.0 };
            v.dco.sub = sub;
            v.ladder.design(pi_t, most);
        }

        // The oversampled steps.
        let level = p[0x20];
        let gate_mode = p[0x2f] >= 0.5;
        let pan_smooth = 1.0 - 0.1f64.powf(200.0 / f64::from(rate)) as f32;
        let mut l = [0.0f32; 4];
        let mut r = [0.0f32; 4];
        for (ls, rs) in l.iter_mut().zip(r.iter_mut()).take(self.oversample) {
            for v in self.voices.iter_mut() {
                let osc = v.dco.next(&self.table);
                let n = match &self.data {
                    Some(data) => {
                        let s = data.noise[v.noise_at];
                        v.noise_at += 1;
                        if v.noise_at > NOISE_LEN - 1 {
                            v.noise_at = 0;
                        }
                        f32::from(s) * 8.631_591_5e-5
                    }
                    None => 0.0,
                };
                v.ladder.input = (n * noise + osc * level) * 0.27;
                v.ladder.run();
            }
            let (mut sl, mut sr) = (0.0f32, 0.0f32);
            for v in self.voices.iter_mut() {
                let g = if gate_mode {
                    v.vca = v.gain;
                    v.gain
                } else {
                    let e = (v.env - 0.025).max(0.0) * v.scaled;
                    v.vca = e;
                    e
                };
                if v.fresh {
                    v.panned = v.pan;
                    v.fresh = false;
                } else {
                    v.panned.0 += (v.pan.0 - v.panned.0) * pan_smooth;
                    v.panned.1 += (v.pan.1 - v.panned.1) * pan_smooth;
                }
                let out = v.ladder.out;
                sr += out * v.panned.1 * g;
                sl += out * v.panned.0 * g;
            }
            *ls = sl;
            *rs = sr;
        }

        // Down to the host's rate.
        let mut n = self.oversample;
        let mut stage = 0;
        while n > 1 {
            for k in 0..n {
                let (a, b) = self.halfband[stage].push(l[k], r[k]);
                l[k >> 1] = a;
                r[k >> 1] = b;
            }
            n >>= 1;
            stage += 1;
        }
        let (mut left, mut right) = (l[0], r[0]);

        if p[0x9a] >= 0.5 {
            left = self.high.run(left);
            right = self.high_r.run(right);
        }
        if self.chorus.mode != 0 {
            self.chorus.run(&mut left, &mut right);
        }
        self.smoothed.0 += (self.volume.0 - self.smoothed.0) * self.smooth;
        self.smoothed.1 += (self.volume.1 - self.smoothed.1) * self.smooth;
        left *= self.smoothed.0;
        right *= self.smoothed.1;
        (left * self.outer, right * self.outer)
    }
}

impl Generator for Kepler {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let short = StateError::Length {
            expected: 189,
            found: bytes.len(),
        };
        let mut at = 12;
        let mut take = |n: usize| -> Result<&[u8], StateError> {
            let part = bytes.get(at..at + n).ok_or(short)?;
            at += n;
            Ok(part)
        };
        let i32_of = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap_or([0; 4]));
        let len = i32_of(take(4)?);
        let name = take(usize::try_from(len).map_err(|_| short)?)?;
        if name != b"Kepler-1\0" {
            return Err(StateError::Version {
                found: i32::from_le_bytes(name.first_chunk().copied().unwrap_or([0; 4])),
            });
        }
        let version = i32_of(take(4)?);
        if !(1..=4).contains(&version) {
            return Err(StateError::Version { found: version });
        }
        let mut params: [f32; PARAMS] = std::array::from_fn(default);
        let mut count = 0;
        let mut flags = [0u8; 3];
        if version > 1 {
            count = usize::try_from(i32_of(take(4)?)).map_err(|_| short)?;
            for i in 0..count {
                let value = f32::from_le_bytes(take(4)?.try_into().unwrap_or([0; 4]));
                if i < KEPLER1.len() {
                    params[KEPLER1[i]] = value;
                }
            }
            take(1)?;
            params[0x49] = f32::from_le_bytes(take(4)?.try_into().unwrap_or([0; 4]));
            params[0xb] = f32::from_le_bytes(take(4)?.try_into().unwrap_or([0; 4]));
        }
        if version > 2 {
            flags.copy_from_slice(take(3)?);
        }
        if at != bytes.len() {
            return Err(StateError::Length {
                expected: at,
                found: bytes.len(),
            });
        }
        // Before version 4, an LFO rate of 0.5 or more is read anew
        // (the end of `FUN_18001f170`).
        if version < 4 && params[0x31] >= 0.5 {
            let x = (2f64.powf(f64::from(params[0x30] * 7.0)) as f32 - 1.0) * 48.370_08 + 1.1;
            let ticks = (9216.0 / (x as i32).max(1) as f32) as i32 - 1;
            let y = (f64::from(ticks as f32 * 0.020_673_936 + 1.0)).log2() as f32;
            params[0x30] = 1.0 - y * 0.206_099_3;
        }
        self.params = params;
        self.version = version;
        self.count = count;
        self.noise_on = flags[1] != 0;
        self.oversample = if flags[2] != 0 { 4 } else { 2 };
        self.voices = std::array::from_fn(Voice::new);
        self.halfband = [Halfband::default(); 2];
        self.prepare();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.voices = std::array::from_fn(Voice::new);
        self.chorus = Chorus::new(hz);
        self.prepare();
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm;
    }

    /// Reads the noise and the LFO's wave out of the engine's DLL.
    fn set_resource_dir(&mut self, dir: &Path) {
        let read = std::fs::read(dir.join(DLL)).ok();
        if let Some(data) = read.as_deref().and_then(Data::from_dll) {
            self.data = Some(data);
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let key = i32::from(note.key);
        if !(0..128).contains(&key) {
            return;
        }
        let v = ((f64::from(note.velocity) * 10.0 + 1.0).ln() as f32 * 41.714_26).clamp(0.0, 127.0);
        let velocity = (v as i32) as f32 * 0.007_874_016;
        if self.held.len() < self.held.capacity() {
            self.held.retain(|(_, k)| *k != key);
            self.held.push((note.voice, key));
        }
        self.key_on(key, velocity, note.pan, note.fine_pitch);
    }

    fn note_off(&mut self, voice: u32) {
        if let Some(at) = self.held.iter().position(|(v, _)| *v == voice) {
            let (_, key) = self.held.remove(at);
            self.key_off(key);
        }
    }

    fn all_notes_off(&mut self) {
        while let Some((_, key)) = self.held.pop() {
            self.key_off(key);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let step = 1.0 / f64::from(self.rate);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.clock();
            let (a, b) = self.frame();
            *l += a;
            *r += b;
            self.seconds += step;
            self.beats += step * self.tempo * (1.0 / 60.0);
        }
    }

    fn param_count(&self) -> usize {
        KEPLER1.len()
    }

    fn param(&self, index: usize) -> f32 {
        KEPLER1.get(index).map_or(0.0, |p| self.params[*p])
    }

    /// From 0 to 1, as the state holds it.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(p) = KEPLER1.get(index) {
            self.params[*p] = if value.is_nan() {
                0.0
            } else {
                value.clamp(0.0, 1.0)
            };
            self.prepare();
        }
    }

    fn reset(&mut self) {
        self.held.clear();
        self.arp.played.clear();
        self.arp.up.clear();
        self.arp.down.clear();
        self.arp.sounding = -1;
        for v in self.voices.iter_mut() {
            v.gate = false;
            v.stage = 2;
            v.env = 0.0;
            v.gain = 0.0;
            v.target = 0.0;
        }
    }

    /// The release down to a ten-thousandth, and a tenth of a second for
    /// the chorus and the filters.
    fn tail(&self) -> usize {
        let t = f64::from(curve(&RELEASE, self.params[0x40]));
        ((4.0 * t + 0.1) * f64::from(self.rate)) as usize
    }

    fn fidelity(&self) -> Fidelity {
        if self.data.is_none() {
            Fidelity::Approximate("Kepler noise and LFO wave not available")
        } else if self.params[0x52] >= 0.5 {
            Fidelity::Approximate("Kepler hold not built")
        } else if self.noise_on {
            Fidelity::Approximate("Kepler chorus noise not built")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn note(voice: u32, key: i16) -> NoteOn {
        NoteOn {
            voice,
            key,
            velocity: 1.0,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            slide: false,
            porta: false,
        }
    }

    /// A state of version 4 with these Kepler-1 parameters and the
    /// defaults elsewhere.
    fn state(set: &[(usize, f32)]) -> Vec<u8> {
        let plugin = Kepler::default();
        let mut values: Vec<f32> = (0..KEPLER1.len()).map(|i| plugin.param(i)).collect();
        for &(i, v) in set {
            values[i] = v;
        }
        let mut stream = Vec::new();
        stream.extend(9i32.to_le_bytes());
        stream.extend(b"Kepler-1\0");
        stream.extend(4i32.to_le_bytes());
        stream.extend((values.len() as i32).to_le_bytes());
        for v in values {
            stream.extend(v.to_le_bytes());
        }
        stream.push(0);
        stream.extend(1f32.to_le_bytes());
        stream.extend(0.5f32.to_le_bytes());
        stream.extend([0, 0, 0]);
        let mut bytes = vec![0; 4];
        bytes.extend((stream.len() as u32).to_le_bytes());
        bytes.extend([0; 4]);
        bytes.extend(stream);
        bytes
    }

    fn with(set: &[(usize, f32)]) -> Kepler {
        let mut plugin = Kepler::default();
        plugin.set_state(&state(set)).unwrap();
        plugin
    }

    fn play(plugin: &mut Kepler, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        for (l, r) in left.chunks_mut(128).zip(right.chunks_mut(128)) {
            plugin.process(l, r);
        }
        (left, right)
    }

    fn crossings(sound: &[f32]) -> usize {
        sound
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count()
    }

    fn rms(sound: &[f32]) -> f32 {
        (sound.iter().map(|v| v * v).sum::<f32>() / sound.len() as f32).sqrt()
    }

    #[test]
    fn a_state_sets_kepler_1_s_parameters_in_its_order() {
        let plugin = with(&[(19, 0.25), (30, 1.0), (33, 0.75)]);
        assert_eq!(plugin.param_of(0x25), 0.25);
        assert_eq!(plugin.param_of(0x45), 1.0);
        assert_eq!(plugin.param_of(0x48), 0.75);
        assert_eq!(plugin.param(19), 0.25);
        assert_eq!(plugin.param_count(), 36);
        // What the state leaves out keeps the engine's default.
        assert_eq!(plugin.param_of(0x9a), 1.0);
        assert_eq!(plugin.param_of(0x28), 1.0);
        assert_eq!(plugin.param_of(0x0e), 0.33);
    }

    #[test]
    fn a_state_that_is_not_kepler_1_s_is_refused() {
        let mut plugin = Kepler::default();
        let good = state(&[]);
        assert!(plugin.set_state(&good[..good.len() - 1]).is_err());
        let mut long = good.clone();
        long.push(0);
        assert!(plugin.set_state(&long).is_err());
        let mut named = good.clone();
        named[16] = b'X';
        assert!(plugin.set_state(&named).is_err());
        let mut version = good;
        version[25..29].copy_from_slice(&9i32.to_le_bytes());
        assert!(plugin.set_state(&version).is_err());
        assert!(plugin.set_state(&[]).is_err());
    }

    #[test]
    fn the_last_byte_turns_the_oversampling_to_4() {
        let mut bytes = state(&[]);
        let last = bytes.len() - 1;
        bytes[last] = 1;
        let mut plugin = Kepler::default();
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.oversample, 4);
        plugin.note_on(note(0, 57));
        let (left, _) = play(&mut plugin, 8820);
        assert!(left.iter().all(|v| v.is_finite()));
        assert!(rms(&left[4410..]) > 0.01);
    }

    #[test]
    fn a_curve_runs_straight_between_its_points() {
        let table = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 20.0];
        assert_eq!(curve(&table, 0.0), 0.0);
        assert!((curve(&table, 0.25) - 2.5).abs() < 1e-6);
        assert!((curve(&table, 0.95) - 14.5).abs() < 1e-5);
        assert_eq!(curve(&table, 1.0), 20.0);
        assert_eq!(curve(&table, 2.0), 20.0);
        assert_eq!(curve(&table, -1.0), table[0]);
    }

    #[test]
    fn the_filter_s_tanh_is_tanh_and_holds_at_5() {
        for i in -300..=300 {
            let x = i as f32 / 100.0;
            assert!((tanh(x) - x.tanh()).abs() < 2e-3, "{x}");
        }
        assert_eq!(tanh(9.0), tanh(5.0));
        assert_eq!(tanh(-9.0), -tanh(5.0));
    }

    #[test]
    fn every_row_of_the_step_table_adds_up_to_the_same_step() {
        let table = step_table();
        assert_eq!(table.len(), ROWS);
        let sums: Vec<f32> = table.iter().map(|row| row.iter().sum()).collect();
        // The peak of the impulse is 0.5 at 1024 times the rate, so a
        // row is one sample's worth of the step.
        for sum in &sums {
            assert!(
                (sum - sums[0]).abs() < 1e-2 * sums[0].abs(),
                "{sum} {}",
                sums[0]
            );
        }
        assert!(sums[0] > 0.0);
        // A row late by a whole sample is the first row a tap later.
        for j in 1..TAPS {
            assert!((table[1024][j] - table[0][j - 1]).abs() < 1e-6);
        }
    }

    #[test]
    fn the_halfband_passes_the_bass_and_stops_the_top() {
        let gain = |turns: f64| {
            let mut band = Halfband::default();
            let x = testing::sine(turns, 8192);
            let mut out = Vec::new();
            for pair in x.chunks(2) {
                band.push(pair[0], pair[0]);
                out.push(band.push(pair[1], pair[1]).0);
            }
            rms(&out[1024..]) / rms(&x)
        };
        assert!((gain(0.01) - 1.0).abs() < 0.01);
        assert!(gain(0.4) < 0.01);
    }

    #[test]
    fn the_ladder_passes_below_its_cutoff_and_rings_at_full_resonance() {
        let pi_t = std::f32::consts::PI / 88_200.0;
        let run = |cutoff: f32, res: f32, turns: f64| {
            let mut ladder = Ladder {
                cutoff,
                res,
                ..Ladder::default()
            };
            ladder.design(pi_t, 44_000.0);
            let x = testing::sine(turns, 16_384);
            let out: Vec<f32> = x
                .iter()
                .map(|v| {
                    ladder.input = v * 0.1;
                    ladder.run();
                    ladder.out
                })
                .collect();
            rms(&out[8192..]) / (rms(&x[8192..]) * 0.1)
        };
        assert!((run(5000.0, 0.0, 100.0 / 88_200.0) - 1.0).abs() < 0.02);
        // Four poles: two octaves above the cutoff is down by 24 dB or so.
        assert!(run(1000.0, 0.0, 4000.0 / 88_200.0) < 0.08);
        // Resonance lifts the cutoff above the passband.
        assert!(run(1000.0, 0.7, 1000.0 / 88_200.0) > run(1000.0, 0.0, 1000.0 / 88_200.0));
    }

    #[test]
    fn a_note_plays_its_pitch_and_dies_after_its_release() {
        // Saw only, filter open, no chorus.
        let mut plugin = with(&[(13, 0.0), (14, 1.0), (15, 0.0), (19, 1.0), (30, 0.0)]);
        plugin.note_on(note(1, 69));
        let (left, right) = play(&mut plugin, 44_100);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        // 440 Hz, through the highpass at 20 Hz.
        let turns = crossings(&left[4410..]) as f32 / (39_690.0 / 44_100.0);
        assert!((turns - 440.0).abs() < 4.0, "{turns}");
        assert!(rms(&left[4410..]) > 0.05);
        // Centred.
        assert!((rms(&left) - rms(&right)).abs() < 1e-4);
        plugin.note_off(1);
        let tail = plugin.tail();
        let (left, _) = play(&mut plugin, tail);
        let end = &left[left.len() - 441..];
        assert!(rms(end) < 1e-3, "{}", rms(end));
    }

    #[test]
    fn an_octave_up_doubles_the_pitch() {
        let saw = [(13, 0.0), (14, 1.0), (15, 0.0), (19, 1.0), (30, 0.0)];
        let count = |key: i16| {
            let mut plugin = with(&saw);
            plugin.note_on(note(1, key));
            let (left, _) = play(&mut plugin, 44_100);
            crossings(&left[4410..])
        };
        let (low, high) = (count(45), count(57));
        assert!(
            (high as f32 / low as f32 - 2.0).abs() < 0.03,
            "{low} {high}"
        );
    }

    #[test]
    fn the_cutoff_takes_the_top_off() {
        let bright = |cutoff: f32| {
            let mut plugin = with(&[(19, cutoff), (20, 0.0), (22, 0.0), (30, 0.0)]);
            plugin.note_on(note(1, 45));
            let (left, _) = play(&mut plugin, 22_050);
            let d: Vec<f32> = left.windows(2).map(|w| w[1] - w[0]).collect();
            rms(&d[4410..]) / rms(&left[4410..])
        };
        assert!(bright(0.3) < bright(1.0) * 0.5);
    }

    #[test]
    fn the_arpeggiator_goes_up_then_down() {
        let mut arp = Arp::new();
        arp.mode = 1;
        arp.octaves = 2;
        for (i, key) in [64, 60, 67].into_iter().enumerate() {
            arp.add(key, 1.0, i == 0, false);
        }
        let keys: Vec<i32> = (0..12).map(|_| arp.advance().0).collect();
        assert_eq!(keys, [60, 64, 67, 72, 76, 79, 76, 72, 67, 64, 60, 64]);
        arp.mode = 2;
        arp.restart();
        let keys: Vec<i32> = (0..7).map(|_| arp.advance().0).collect();
        assert_eq!(keys, [79, 76, 72, 67, 64, 60, 79]);
        arp.mode = 0;
        arp.octaves = 1;
        arp.restart();
        let keys: Vec<i32> = (0..4).map(|_| arp.advance().0).collect();
        assert_eq!(keys, [60, 64, 67, 60]);
    }

    #[test]
    fn the_arpeggiator_plays_one_note_at_a_time_at_its_rate() {
        // Arp on, up, one octave, a step of 1/16 at 120 beats a minute.
        let mut plugin = with(&[(0, 1.0), (1, 0.0), (2, 0.0), (3, 9.0 / 12.0), (4, 1.0)]);
        plugin.set_tempo(120.0);
        plugin.note_on(note(1, 60));
        plugin.note_on(note(2, 64));
        let (left, _) = play(&mut plugin, 44_100);
        assert!(left.iter().all(|v| v.is_finite()));
        assert!(rms(&left) > 0.01);
        assert!(plugin.voices.iter().filter(|v| v.gate).count() <= 1);
        // Eight steps of 1/8 of a second in the second.
        assert_eq!(plugin.arp.step, 12.0 / 96.0);
        plugin.all_notes_off();
        assert!(plugin.voices.iter().all(|v| !v.gate));
    }

    #[test]
    fn eight_voices_and_a_ninth_takes_the_oldest() {
        let mut plugin = Kepler::default();
        for k in 0..8 {
            plugin.note_on(note(k, 48 + k as i16));
            play(&mut plugin, 10);
        }
        assert!(plugin.voices.iter().all(|v| v.gate));
        plugin.note_on(note(8, 70));
        assert!(plugin.voices.iter().any(|v| v.key == 70));
        assert!(!plugin.voices.iter().any(|v| v.key == 48));
    }

    #[test]
    fn the_engine_s_data_is_read_out_of_its_dll() {
        assert!(Data::from_dll(b"MZ").is_none());
        assert!(Data::from_dll(&[]).is_none());
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Kepler's DLL skipped");
            return;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        let data = Data::from_dll(&std::fs::read(dir.join(DLL)).unwrap()).unwrap();
        assert_eq!(data.noise.len(), NOISE_LEN);
        assert_eq!(data.wave.len(), WAVE_LEN + 1);
        assert_eq!(data.wave[WAVE_LEN], data.wave[0]);
        assert!(data.wave.iter().all(|v| (-1.001..=1.001).contains(v)));
        let noise: Vec<f32> = data
            .noise
            .iter()
            .map(|v| f32::from(*v) * 8.6316e-5)
            .collect();
        assert!(rms(&noise) > 0.01 && rms(&noise) < 2.0, "{}", rms(&noise));
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Kepler's states skipped");
            return;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects: Vec<_> = testing::projects(&root).into_iter().filter(ours).collect();
        assert_eq!(projects.len(), 4);
        let presets = testing::every_preset(&root, NAME);
        assert!(presets.len() >= 277, "{}", presets.len());
        for (n, saved) in projects.iter().chain(&presets).enumerate() {
            let mut plugin = Kepler::default();
            plugin
                .set_state(&saved.state)
                .unwrap_or_else(|e| panic!("{}: {e:?}", saved.file));
            if n >= projects.len() + 40 && n % 8 != 0 {
                continue;
            }
            plugin.set_resource_dir(&dir);
            plugin.note_on(note(1, 48));
            plugin.note_on(note(2, 55));
            let (left, right) = play(&mut plugin, 8820);
            let peak = left
                .iter()
                .chain(&right)
                .fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak.is_finite() && peak < 8.0, "{}: {peak}", saved.file);
        }
        // The projects' channels sound.
        for saved in &projects {
            let mut plugin = Kepler::default();
            plugin.set_state(&saved.state).unwrap();
            plugin.set_resource_dir(&dir);
            assert_eq!(plugin.fidelity(), Fidelity::Exact, "{}", saved.file);
            plugin.note_on(note(1, 60));
            let (left, _) = play(&mut plugin, 44_100);
            assert!(rms(&left) > 1e-3, "{}", saved.file);
        }
    }
}
