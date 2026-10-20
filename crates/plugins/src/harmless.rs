//! Harmless: an additive synthesiser, as `docs/plugins/harmless.md` reads
//! it out of `Harmless_x64.dll`.
//!
//! The state is a version (12 now), the 100 parameters as little-endian
//! i32, seven selectors (filter type, resonance type, phaser type,
//! compression preset, timbre colour and two quality switches), an
//! optional drawn timbre of 524 levels and 524 phases, and a flag byte.
//! Versions 4 to 12 load; [`State::parse`] has the corrections FL makes to
//! the older ones.
//!
//! # A voice
//!
//! A voice is a bank of up to 528 sines ([`dsp::additive::SineBank`]):
//! partial 0 is the filter's self-oscillation, 1 to 3 the three sub
//! partials an octave below, a fifth above and a ninth above the note,
//! and 4 on the harmonics. Every control tick (88 samples at 44.1 kHz)
//! the plugin works out each partial's frequency and level:
//!
//! 1. The timbre: the selected colour's level and phase for each harmonic
//!    (a saw by default), weighted by the six timbre EQ bands, raised
//!    cosines an octave wide on the harmonic number.
//! 2. The harmonic mask, twelve levels repeated up the harmonics and mixed
//!    in by the mask mix and the LFO.
//! 3. The filter: a response curve laid over the spectrum in octaves from
//!    the cutoff, which moves with the key, the filter envelope, the LFO
//!    and Mod X; then the resonance curve on top, and the phaser's comb.
//! 4. The pluck, which lets each partial die away at a rate that grows
//!    with its harmonic number.
//! 5. The harmonizer, which adds the spectrum again at multiples of each
//!    harmonic, at the place the selector puts it in this chain.
//! 6. The low-harmonic protection, which pulls the first harmonics back to
//!    their unfiltered level, and the sub levels.
//!
//! The bank glides each partial to its new level in 10 ms. The voice is
//! then shaped by a linear attack, an exponential decay to silence (or
//! none at a decay of 1000) and an exponential release, panned, and
//! summed. Unison makes up to nine voices of a note with their own pitch,
//! pan, phase and attack.
//!
//! Each voice then takes the stereo tremolo; after the voices come the
//! chorus, delay, reverb and the compression, which is a Maximus loaded from
//! `Harmless/Data/Compression/<n>.fst` under the directory
//! [`Generator::set_resource_dir`] is handed. The chorus, delay and reverb
//! are FL's own classes from [`crate::fl_fx`], set from Harmless's tables.
//!
//! # What is not FL's
//!
//! - The effects are told a host block of 256 frames, which sets when
//!   they sleep after the last voice. The delay keeps its time down to 20
//!   BPM and is only made when the state loads with it on.
//! - Every note is of FL's first colour: the colour groups of legato and
//!   portamento, the colours that turn them round and the filter notes are
//!   not built.
//! - The noise resonance and the per-note LFO value draw from a
//!   generator of their own: FL's is a Mersenne twister seeded from the
//!   performance counter (`FUN_0074bcf0`), so no two runs of FL agree
//!   either.
//! - Slide notes bend nothing (FL slides the host levels of what sounds).

use crate::autogun_engine::rand_uniform;
use crate::fl_fx::{
    Delay32, DelayKernel, FlangerSettings, FruityReverb, ReverbSettings, StereoFlanger,
};
use crate::three_osc::pan_law;
use crate::{Effect, Fidelity, Generator, NoteOn, StateError, knob_value, maximus::Maximus, parts};
use dsp::additive::{ShapeTable, SineBank};
use std::f64::consts::{LN_2, PI, TAU};
use std::path::Path;

/// The plugin's event 201 name.
pub const NAME: &str = "Harmless";

/// Where the compression presets are, from the directory
/// [`Generator::set_resource_dir`] is handed.
pub const COMPRESSION: &str = "Harmless/Data/Compression";

/// How many parameters the plugin has.
pub const PARAMS: usize = 100;

/// The partials a voice has: the self-oscillation, three subs and 524
/// harmonics.
pub const PARTIALS: usize = 528;

/// The levels and phases a drawn timbre has.
pub const IMAGE: usize = 524;

/// The voices the plugin keeps, unison copies counted.
const VOICES: usize = 96;

/// The most notes legato keeps held under the one that sounds.
const HELD: usize = 64;

/// A voice whose envelope falls under -80 dB after its attack is over.
const END_LEVEL: f32 = 1e-4;

/// The most samples worked on at once.
const CHUNK: usize = 256;

/// The host block the post chain's classes are told of (FL's
/// `FPD_SetBlockSize`, this+0x2d4): the chunk the plugin renders in.
const BLOCK: usize = CHUNK;

/// The slowest tempo the delay keeps its time at; slower ones cut it to
/// what the ring holds.
const SLOWEST_TEMPO: f64 = 20.0;

/// The highest frequency a harmonic is given at the first quality, in Hz.
const LIMIT_HZ: f64 = 17_000.0;

/// The lowest the filter goes before its curve adds to it: 440 Hz 31
/// semitones down.
fn filter_floor_hz() -> f64 {
    440.0 * (-31.0f64 / 12.0).exp2()
}

/// The control range of every parameter, from the editor's controls.
pub const RANGES: [(i32, i32); PARAMS] = {
    let mut r = [(0, 1); PARAMS];
    r[0] = (-1000, 1000);
    r[1] = (0, 1024);
    r[2] = (0, 2048);
    r[3] = (0, 2048);
    r[4] = (0, 2048);
    let mut i = 5;
    while i <= 10 {
        r[i] = (-1024, 1024);
        i += 1;
    }
    while i <= 22 {
        r[i] = (-1000, 1000);
        i += 1;
    }
    r[23] = (0, 1000);
    r[24] = (-1000, 1000);
    r[25] = (0, 1000);
    r[26] = (0, 1000);
    r[27] = (0, 1000);
    r[28] = (0, 1000);
    r[31] = (0, 1000);
    r[36] = (0, 2000);
    r[38] = (-2400, 2400);
    r[39] = (-2400, 2400);
    r[40] = (-1000, 1000);
    r[41] = (0, 1000);
    r[42] = (-1000, 1000);
    r[45] = (-1000, 1000);
    r[48] = (0, 1000);
    r[49] = (0, 1000);
    r[50] = (-1000, 1000);
    r[51] = (-1000, 1000);
    r[52] = (-1000, 1000);
    r[54] = (0, 8192);
    r[56] = (0, 1000);
    r[57] = (-2000, 2000);
    r[58] = (-1000, 1000);
    r[59] = (0, 1000);
    r[61] = (-2400, 2400);
    r[62] = (0, 2048);
    r[65] = (1, 9);
    r[66] = (0, 128);
    r[67] = (0, 256);
    r[68] = (0, 128);
    r[69] = (0, 128);
    r[70] = (0, 2);
    r[71] = (0, 1000);
    r[72] = (0, 2);
    r[73] = (0, 10_000);
    r[74] = (-1000, 1000);
    r[75] = (0, 999);
    r[76] = (-1000, 1000);
    r[77] = (-8000, 8000);
    r[78] = (-2000, 2000);
    r[79] = (0, 1000);
    r[80] = (0, 1000);
    r[81] = (-1000, 1000);
    r[83] = (0, 1024);
    r[84] = (0, 4);
    r[85] = (0, 3);
    r[86] = (0, 13);
    r[88] = (0, 1000);
    r[89] = (-8000, 8000);
    r[90] = (0, 8);
    r[91] = (0, 1000);
    r[92] = (0, 3);
    r[93] = (0, 1000);
    r[94] = (0, 900);
    r[95] = (0, 384);
    r[96] = (0, 6);
    r[97] = (0, 1000);
    r[98] = (0, 1000);
    r
};

/// A new Harmless: what FL's `Default.fst` preset for it holds (version 10).
pub const DEFAULTS: [i32; PARAMS] = [
    1000, 256, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 1000, 0, 1000, 0, 1000, 0, 1000, 0, 1000, 0, 0,
    0, 0, 0, 1000, 250, 0, 0, 1000, 0, 0, 0, 0, 1000, 1, 0, 0, 0, 0, 0, 0, 0, 400, 0, 1, 0, 500, 0,
    -300, 500, 0, 4096, 0, 500, 0, 0, 0, 0, 0, 0, 0, 0, 1, 128, 128, 64, 0, 2, 0, 0, 5000, 0, 500,
    0, 3000, 0, 0, 500, 0, 0, 256, 0, 3, 0, 1, 0, 3000, 0, 500, 0, 500, 300, 144, 0, 500, 500, 0,
];

/// The selectors of `Default.fst`: filter type 4, and the checkbox on.
pub const DEFAULT_SELECTORS: [i32; 7] = [4, 0, 0, 0, 0, 1, 0];

/// How many values each selector has.
const SELECTORS: [i32; 7] = [17, 9, 10, 5, 12, 2, 3];

/// The plugin's state.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub version: i32,
    pub params: [i32; PARAMS],
    /// Filter type, resonance type, phaser type, compression preset (0
    /// off), timbre colour, a checkbox, and the quality (tick and the
    /// harmonics' limit).
    pub selectors: [i32; 7],
    /// The drawn timbre: 524 levels, then 524 phases in turns.
    pub image: Option<Vec<f32>>,
    pub flag: u8,
}

impl Default for State {
    fn default() -> Self {
        State {
            version: 12,
            params: DEFAULTS,
            selectors: DEFAULT_SELECTORS,
            image: None,
            flag: 1,
        }
    }
}

fn read_i32(bytes: &[u8], at: &mut usize) -> Result<i32, StateError> {
    let word = bytes.get(*at..*at + 4).ok_or(StateError::Length {
        expected: *at + 4,
        found: bytes.len(),
    })?;
    *at += 4;
    Ok(i32::from_le_bytes(word.try_into().unwrap()))
}

impl State {
    /// Read a state as `SaveRestoreState` (`FUN_0077adb0`) does. A value
    /// outside its control's range is kept inside it, as the controls FL
    /// pushes it through do.
    pub fn parse(bytes: &[u8]) -> Result<State, StateError> {
        let mut at = 0;
        let version = read_i32(bytes, &mut at)?;
        if version > 12 {
            return Err(StateError::Version { found: version });
        }
        let mut state = State {
            version,
            ..State::default()
        };
        if version < 2 {
            return Ok(state);
        }
        let count = if version < 9 { 99 } else { 100 };
        for p in 0..count {
            state.params[p] = read_i32(bytes, &mut at)?;
        }
        if version < 9 {
            state.params[99] = 0;
        }
        if version < 5 && state.params[86] > 9 {
            // An LFO shape was put in at 10.
            state.params[86] += 1;
        }
        if version < 6 {
            state.params[61] = (f64::from(state.params[61]) * 1.2).round_ties_even() as i32;
        }
        if version > 3 {
            for s in 0..7 {
                state.selectors[s] = read_i32(bytes, &mut at)?;
            }
            if version < 5 && state.selectors[3] == 3 {
                // A compression preset was put in at 3.
                state.selectors[3] = 4;
            }
        }
        if version > 7 {
            let n = read_i32(bytes, &mut at)?;
            if n > 0 {
                let n = n as usize;
                let end = at + n * 8;
                let block = bytes.get(at..end).ok_or(StateError::Length {
                    expected: end,
                    found: bytes.len(),
                })?;
                let mut image: Vec<f32> = block
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|w| f32::from_le_bytes(*w))
                    .collect();
                if version < 10 {
                    // The phases started four floats later.
                    image.copy_within(n + 4.., n);
                }
                image.resize(2 * IMAGE, 0.0);
                state.image = Some(image);
                at = end;
            }
        }
        if version > 11 {
            state.flag = *bytes.get(at).ok_or(StateError::Length {
                expected: at + 1,
                found: bytes.len(),
            })?;
            at += 1;
        }
        if at != bytes.len() {
            return Err(StateError::Length {
                expected: at,
                found: bytes.len(),
            });
        }
        for (value, (min, max)) in state.params.iter_mut().zip(RANGES) {
            *value = (*value).clamp(min, max);
        }
        for (value, count) in state.selectors.iter_mut().zip(SELECTORS) {
            *value = (*value).clamp(0, count - 1);
        }
        Ok(state)
    }
}

/// The plugin helper's level curve: `(11^x - 1) / 10`.
fn level(x: f64) -> f64 {
    if x == 1.0 {
        1.0
    } else {
        ((x * 11f64.ln()).exp() - 1.0) * 0.1
    }
}

/// The level curve as the effect setters work it, in f32: `v == 1` is 1,
/// else `(11^v - 1)` times 0.1 as an f32.
fn fl_level(v: f32) -> f32 {
    if v == 1.0 {
        1.0
    } else {
        let e = f64::from(v * 11f64.ln() as f32).exp();
        ((e - 1.0) * f64::from(0.1f32)) as f32
    }
}

/// A reverb at a rate with room for the longest predelay of the table,
/// 31 / 384 s.
fn reverb_for(hz: i32) -> FruityReverb {
    FruityReverb::new(hz, (31.0 * f64::from(hz) / 384.0) as usize + 1)
}

/// The volume curve of the main, timbre EQ and compression wheels:
/// `(3^x - 1) / 2`.
fn level3(x: f64) -> f64 {
    if x == 1.0 {
        1.0
    } else {
        ((x * 3f64.ln()).exp() - 1.0) * 0.5
    }
}

/// FL's `Round`, to the even one at a half.
fn round(x: f64) -> f64 {
    x.round_ties_even()
}

/// The part of a time after its whole turns, with the sign of the time.
fn frac(x: f64) -> f64 {
    x - x.trunc()
}

/// `exp(ln(0.01) / samples)`: what a one-pole that falls 40 dB in this
/// many samples keeps of itself a sample.
fn fall_40db(samples: f64) -> f64 {
    if samples > 0.0 {
        (0.01f64.ln() / samples.max(1.0)).exp()
    } else {
        0.0
    }
}

/// The level and the phase in turns of one point of a timbre colour
/// (`FUN_007873c0`), at harmonic index `n` (0 the fundamental).
pub fn timbre_point(colour: i32, n: f64, image: Option<&[f32]>) -> (f32, f64) {
    let odd = (n.trunc() as i64) & 1 == 1;
    let quarter_odd = (n.trunc() as i64) & 3 == 3;
    match colour {
        1 => ((0.5f64.sqrt() / (n * 0.5 + 1.0)) as f32, 0.0),
        2 => ((0.5 / (n * 0.25 + 1.0)) as f32, 0.0),
        5 if odd => ((1.1 / ((n + 1.0) * n)) as f32, 0.25),
        5 => ((1.1 / (n + 1.0)) as f32, 0.0),
        6 if odd => ((1.1 / (n + 1.0) * (1.0 / n.powf(0.2))) as f32, 0.25),
        6 => ((1.1 / (n + 1.0)) as f32, 0.0),
        7 => {
            let level = ((1.0 / (n + 1.0)) * 7f64.ln()).exp() - 1.0;
            (
                (level * 0.291_666_666_666_666_7) as f32,
                if odd { 0.75 } else { 0.0 },
            )
        }
        8 => {
            let mut level = 0.5 / (n + 1.0);
            if odd {
                level += 0.5 / (n * 0.5 + 1.0);
            }
            (level as f32, 0.0)
        }
        9 => {
            let mut level = 1.0 / ((n + 1.0) * 3.0);
            if odd {
                level += 1.0 / ((n * 0.5 + 1.0) * 3.0);
            }
            if quarter_odd {
                level += 1.0 / ((n * 0.25 + 1.0) * 3.0);
            }
            (level as f32, 0.0)
        }
        10 => {
            let mut level = 0.5 / (n + 1.0);
            if odd {
                level += 0.25 / (n * 0.5 + 1.0);
            }
            if quarter_odd {
                level += 0.125 / (n * 0.25 + 1.0);
            }
            (level as f32, 0.0)
        }
        11 => match image {
            Some(image) => {
                let i = (round(n) as usize).min(IMAGE - 1);
                (image[i], f64::from(image[i + IMAGE]))
            }
            None => (0.0, 0.0),
        },
        _ => {
            let level = (1.0 / (n + 1.0)) as f32;
            let phase = match colour {
                3 if odd => 0.25,
                4 if (n.trunc() as i64) & 3 == 1 => 0.5,
                _ => 0.0,
            };
            (level, phase)
        }
    }
}

/// `10^(-72 x / 20) * (1 - x^16)`: the -72 dB slope the filter and
/// resonance curves fall along.
fn slope_db(x: f64) -> f64 {
    let gain = ((x * -72.0) as f32 * (10f64.ln() / 20.0) as f32) as f64;
    let x4 = x * x * x * x;
    gain.exp() * (1.0 - x4 * x4 * x4 * x4)
}

fn raised(x: f64, turns: f64) -> f64 {
    ((x * turns).cos() + 1.0) * 0.5
}

/// One of the shape curves of `FUN_0077ca30`: the 17 filter types
/// (0..16), the 9 resonance types (100..108) and the 10 phaser types
/// (200..209).
pub fn shape(kind: i32) -> ShapeTable {
    let step = |len: usize| 1.0 / (len as f64 - 1.0);
    // The distance from the middle of a table read round, 0 at its ends.
    let tent = |i: usize, len: usize| {
        let at = ((i + len / 2) % len) as f64 * 2.0 / len as f64;
        1.0 - (1.0 - at).abs()
    };
    let flat_band = |len: usize| {
        let mut t = ShapeTable::new(len, 0.5, 1.0, 1.0, |_| 1.0);
        let m = len / 3;
        let s = 0.5 / m as f64;
        for i in 0..m {
            let v = (1.0 - raised(1.0 - i as f64 * s, TAU)).powi(2) as f32;
            t.set(i, v);
            t.set(len - 1 - i, v);
        }
        t
    };
    // The sine of a turn, read a quarter (or three) along.
    let wave =
        |i: usize, len: usize, shift: usize| (((i + shift) % len) as f64 * TAU / len as f64).sin();
    let mut table = match kind {
        0 => {
            let s = step(2048);
            ShapeTable::new(2048, 0.0, 1.0, 0.0, |i| slope_db(i as f64 * s))
        }
        1 => ShapeTable::new(4096, 0.5, 1.0, 1.0, |i| slope_db(1.0 - tent(i, 4096))),
        2 => ShapeTable::new(4096, 0.5, 1.0, 1.0, |i| 1.0 - slope_db(1.0 - tent(i, 4096))),
        3 => {
            let s = step(2048);
            ShapeTable::new(2048, 1.0, 0.0, 1.0, |i| slope_db(1.0 - i as f64 * s))
        }
        4 => {
            let s = step(1024);
            ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| {
                (1.0 - raised(1.0 - i as f64 * s, PI)).powi(2)
            })
        }
        5 => {
            let s = step(2048);
            ShapeTable::new(2048, 0.5, 1.0, 1.0, |i| {
                (1.0 - raised(1.0 - i as f64 * s, TAU)).powi(2)
            })
        }
        6 => {
            let s = step(2048);
            ShapeTable::new(2048, 0.5, 1.0, 1.0, |i| {
                raised(1.0 - i as f64 * s, TAU).powi(2)
            })
        }
        7 => {
            let s = step(1024);
            ShapeTable::new(1024, 1.0, 0.0, 1.0, |i| {
                raised(1.0 - i as f64 * s, PI).powi(2)
            })
        }
        8 => {
            let s = step(682);
            ShapeTable::new(682, 0.0, 1.0, 0.0, |i| {
                -((1.0 - i as f64 * s * 0.5) * PI).cos()
            })
        }
        9 => {
            let s = step(1024);
            ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| {
                let d = 1.0 - i as f64 * s;
                f64::from((((d as f32) as f64 * 1001f64.ln()).exp() - 1.0) as f32) * 0.00075
                    + d * 0.25
            })
        }
        10 => {
            let s = step(1365);
            ShapeTable::new(1365, 0.0, 1.0, 0.0, |i| {
                let a = ((1.0 - i as f64 * s * 0.5) * PI).cos() + 1.0;
                slope_db(a)
            })
        }
        11 => {
            let s = step(1024);
            ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| {
                let x = 1.0 - i as f64 * s;
                (1.0 - raised(x, 3.0 * PI)) * (1.0 - raised(x, PI))
            })
        }
        12 => {
            let s = step(4096);
            ShapeTable::new(4096, 0.5, 1.0, 0.0, |i| {
                if i < 2048 {
                    (1.0 - ((((1.0 - i as f64 * s) * TAU).cos() + 1.0) * 0.25)).powi(2)
                } else {
                    slope_db((i - 2048) as f64 * s * 2.0)
                }
            })
        }
        13 => {
            let s = step(1024);
            ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| {
                let r = 1.0 - raised(1.0 - i as f64 * s, PI);
                let db = (r * 18.0 - 18.0) as f32 * (10f64.ln() / 20.0) as f32;
                f64::from(db).exp()
            })
        }
        14 => {
            let s = step(4096);
            ShapeTable::new(4096, 0.5, 1.0, 1.0, |i| {
                raised(1.0 - i as f64 * s, 8.0 * PI).powi(2)
            })
        }
        15 => {
            let s = step(4096);
            ShapeTable::new(4096, 0.5, 1.0, 1.0, |i| {
                let x = 1.0 - i as f64 * s;
                let a = raised(x, 12.0 * PI);
                let b = 1.0 - raised(x, TAU);
                a * a * b * b + b * 0.125
            })
        }
        16 => flat_band(2048),
        100 | 108 => {
            let s = step(1024);
            let mut t = ShapeTable::new(1024, 0.5, 1.0, 0.0, |i| {
                (1.0 - raised(1.0 - i as f64 * s, TAU)).powi(3)
            });
            t.normalise_rms(7.0);
            t
        }
        101 => {
            let s = step(682);
            let mid = 682 / 2;
            let mut t = ShapeTable::new(682, 0.5, 1.0, 0.0, |i| {
                let window = 1.0 - raised(1.0 - i as f64 * s, TAU);
                let d = ((i as f64 - mid as f64) * 2.0) * s * TAU;
                let sinc = if i == mid { 1.0 } else { d.sin() / d };
                sinc * window
            });
            t.normalise_rms(9.0);
            t
        }
        102 | 104 => {
            let len = if kind == 102 { 341 } else { 2048 };
            let mut t = flat_band(len);
            t.below = 1.0;
            t.above = 0.0;
            t.normalise_rms(if kind == 102 { 10.0 } else { 5.0 });
            t
        }
        103 => {
            let mut t = ShapeTable::new(512, 0.5, 1.0, 0.0, |i| slope_db(1.0 - tent(i, 512)));
            t.normalise_rms(18.0);
            t
        }
        105 => {
            let s = step(1536);
            let mut t = ShapeTable::new(1536, 0.5, 1.0, 0.0, |i| {
                (1.0 - raised(1.0 - i as f64 * s, 4.0 * PI)).powi(4)
            });
            t.normalise_rms(5.0);
            t
        }
        106 => ShapeTable::new(768, 0.5, 1.0, 0.0, |i| {
            let u = 1.0 - tent(i, 768);
            -(u * u)
        }),
        107 => {
            let s = step(1024);
            let mid = 512;
            let mut t = ShapeTable::new(1024, 0.5, 1.0, 0.0, |i| {
                let c = 1.0 - raised(1.0 - i as f64 * s, TAU);
                let d = ((i as f64 - mid as f64) * 2.0) * s * TAU;
                let sinc = if i == mid { 1.0 } else { d.sin() / d };
                -sinc * f64::from((c as f32).powf(0.125))
            });
            t.normalise_rms(10.0);
            t
        }
        200 => {
            let mut t = ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| wave(i, 1024, 768) + 1.0);
            t.normalise_rms(1.0);
            t
        }
        201 => {
            let mut t = ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| {
                let u = 1.0 - tent(i, 1024);
                u * u
            });
            t.normalise_rms(1.0);
            t
        }
        202 => {
            let mut t = ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| (i as f64 * PI / 1024.0).sin());
            t.normalise_rms(1.0);
            t
        }
        203 => {
            let mut t = ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| {
                (wave(i, 1024, 768) * 0.5 + 0.5).powi(6)
            });
            t.normalise_rms(1.0);
            t
        }
        204 => {
            let mut t = ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| slope_db(1.0 - tent(i, 1024)));
            t.normalise_rms(1.0);
            t
        }
        205 => {
            let mut t = ShapeTable::new(1024, 0.0, 1.0, 0.0, |i| {
                let s = (wave(i, 1024, 768) * 0.5 + 0.5).powi(2);
                slope_db(1.0 - s * s)
            });
            t.normalise_rms(1.0);
            t
        }
        206 => {
            let mut t = ShapeTable::new(2048, 0.0, 1.0, 0.0, |i| {
                let a = 1.0 - (wave(i, 2048, 512) * 0.5 + 0.5);
                let s2 = wave(i, 2048, 1536) * 0.5 + 0.5;
                slope_db(a) + slope_db(1.0 - s2 * s2) * 4.0
            });
            t.normalise_rms(1.0);
            t.period = 1024.0;
            t
        }
        207 => {
            let len = 2048;
            let s = step(len);
            let mut t = ShapeTable::zeroed(len, 0.0, 1.0, 0.0);
            for i in 0..len {
                let v = (((i * 4) as f64 * s + 0.75) * TAU).sin() + 1.0;
                t.set((i + len / 8) % len, (v * (1.0 - i as f64 * s)) as f32);
            }
            t.normalise_rms(1.0);
            t.period = 512.0;
            t
        }
        208 => {
            let len = 2048;
            let mut t = ShapeTable::zeroed(len, 0.0, 1.0, 0.0);
            for i in 0..len / 2 {
                let s = (((i + len * 3 / 8) % len) as f64 * 4.0 * PI / len as f64).sin();
                let e = ((1.0 - s.abs()) as f32 as f64 * 1_000_001f64.ln()).exp();
                let f = ((1.0 - (e - 1.0) as f32 / 1_000_000.0) as f64 * s.signum() + 1.0) as f32;
                t.set(i + len / 4, f * f * f);
            }
            t.normalise_rms(1.0);
            t
        }
        209 => {
            let mut t = ShapeTable::new(2048, 0.0, 1.0, 0.0, |i| wave(i, 2048, 1536));
            t.normalise_rms(1.0);
            t
        }
        _ => ShapeTable::new(4, 0.0, 1.0, 0.0, |_| 1.0),
    };
    table.flush();
    table
}

/// What is the same for every Harmless: the harmonic ratios with no
/// detune, the timbre EQ bands and the protection weights
/// (`FUN_0077b450`).
#[derive(Debug, Clone, PartialEq)]
struct Tables {
    bands: Vec<[f32; PARTIALS]>,
    protect: [f32; 8],
    lfo_harmonics: [f32; 12],
}

/// The ratio of each partial to the note with no detune: the
/// self-oscillation's 1, the three subs and the harmonics.
fn base_ratio(k: usize) -> f64 {
    match k {
        0 => 1.0,
        1 => 0.5,
        2 => 1.5,
        3 => 2.5,
        _ => (k - 3) as f64,
    }
}

impl Tables {
    fn new() -> Self {
        let mut bands = vec![[0.0f32; PARTIALS]; 6];
        for (b, band) in bands.iter_mut().enumerate() {
            for (k, w) in band.iter_mut().enumerate() {
                let x = base_ratio(k).log2() - b as f64;
                let below = b == 0 && x < 0.0;
                let above = b == 5 && x > 0.0;
                *w = if below || above {
                    1.0
                } else {
                    ((x.clamp(-1.0, 1.0) * PI).cos() + 1.0) as f32 * 0.5
                };
            }
        }
        let mut protect = [0.0; 8];
        let unit = 1.0 / base_ratio(8).log2();
        for (k, w) in protect.iter_mut().enumerate() {
            let r = base_ratio(k).log2().max(0.0);
            let c = ((r * unit * PI).cos() + 1.0) * 0.5;
            *w = c.powi(5) as f32;
        }
        let mut lfo_harmonics = [0.0; 12];
        for (i, c) in lfo_harmonics.iter_mut().enumerate() {
            let n = (i + 1) as f64;
            let s = (n * PI / 12.0).sin();
            *c = (-11.148_862_085_948_322 / n * (s / n) * (s / n)) as f32;
        }
        Tables {
            bands,
            protect,
            lfo_harmonics,
        }
    }
}

/// What the parameters, the selectors and the rate come to.
#[derive(Debug, Clone, PartialEq)]
struct Derived {
    rate: f64,
    tempo: f64,
    /// Samples between control updates.
    tick: usize,
    /// The highest harmonic frequency, in Hz.
    limit_hz: f64,
    /// Every partial's level and phase of the colour, and the levels
    /// after the timbre EQ.
    raw: [f32; PARTIALS],
    phase: [f64; PARTIALS],
    timbre: [f32; PARTIALS],
    /// Each partial's ratio to the note with the detune, and the
    /// grittiness offsets.
    ratio: [f64; PARTIALS],
    grit_table: [f64; PARTIALS],
    /// Self-oscillation, then the three subs.
    subs: [f32; 4],
    mask: [f32; 12],
    phase_spread: f32,
    protect_gain: f32,
    eq: [f32; 6],
    mask_mix: f32,
    mask_lfo: f32,
    tremolo: f32,
    attack: f64,
    decay: f64,
    release: f64,
    pluck: f64,
    main_volume: f32,
    main_pitch: f32,
    grit: f32,
    vibrato: f32,
    pitch_lfo: f32,
    porta: f32,
    fenv_attack: f32,
    fenv_decay: f32,
    fenv_tension: [Tension; 2],
    fenv_amount: f32,
    /// The filter's range in log2 Hz.
    fmin: f32,
    fmax: f32,
    frange: f32,
    fcut: f32,
    fwidth: f32,
    keytrack: f32,
    flfo: f32,
    res_offset: f32,
    /// The noise resonance's hold in ms (0x720) and the part of it a tick
    /// is (0x718).
    noise_len: f64,
    noise_rate: f64,
    res_ratio: f32,
    res_scale: f32,
    res_bias: f32,
    unison: usize,
    unison_gain: f32,
    fat_pitch: f32,
    fat_phase: f32,
    phaser_width: f32,
    phaser_width_lfo: f32,
    phaser_offset_lfo: f32,
    phaser_speed: f64,
    sfx_width: f32,
    sfx_width_lfo: f32,
    sfx_strength: f32,
    lfo_attack: f32,
    lfo_speed: f64,
    lfo_key_offset: f32,
    declick: f64,
    smoothing: f32,
    cutoff_glide: f32,
    res_glide: f32,
    vib_rate: f32,
    vib_attack: f32,
    pluck_rate: f64,
    pluck_start: i64,
    /// The tremolo's rate in turns a sample (0x7a4), the frames a new
    /// voice fades it in over (0x7ac) and the fade's step (0x7a8).
    tremolo_rate: f32,
    tremolo_fade: i32,
    tremolo_fade_step: f32,
    /// The most a host gain moves a frame (`TAudioRenderer` +0x34).
    ramp_cap: f32,
}

/// A curve of the filter envelope: `FUN_005fd4d0`'s `(t, 1 / |t|,
/// ln(|t| + 1))`, straight at 0.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Tension {
    t: f32,
    inv: f32,
    ln: f32,
}

impl Tension {
    fn new(value: i32) -> Self {
        let v = f64::from(value) / 1000.0;
        let t = (v.signum() * ((v.abs() * 1_000_001f64.ln()).exp() - 1.0)) as f32;
        if t == 0.0 {
            return Tension::default();
        }
        Tension {
            t,
            inv: 1.0 / t.abs(),
            ln: (f64::from(t.abs()) + 1.0).ln() as f32,
        }
    }

    /// The curve at `x` from 0 to 1.
    fn at(&self, x: f32) -> f32 {
        let x = f64::from(x);
        if self.t == 0.0 {
            x as f32
        } else if self.t > 0.0 {
            (1.0 - (((1.0 - x) * f64::from(self.ln)).exp() - 1.0) * f64::from(self.inv)) as f32
        } else {
            ((((x * f64::from(self.ln)).exp()) - 1.0) * f64::from(self.inv)) as f32
        }
    }

    /// Where on the curve a value is.
    fn inverse(&self, y: f32) -> f32 {
        let y = f64::from(y);
        if self.t == 0.0 {
            y as f32
        } else if self.t > 0.0 {
            (1.0 - ((1.0 - y) * f64::from(self.t.abs()) + 1.0).ln() / f64::from(self.ln)) as f32
        } else {
            ((y * f64::from(self.t.abs()) + 1.0).ln() / f64::from(self.ln)) as f32
        }
    }
}

impl Derived {
    fn new(rate: f64, tempo: f64) -> Self {
        Derived {
            rate,
            tempo,
            tick: 88,
            limit_hz: LIMIT_HZ,
            raw: [0.0; PARTIALS],
            phase: [0.0; PARTIALS],
            timbre: [0.0; PARTIALS],
            ratio: [0.0; PARTIALS],
            grit_table: [0.0; PARTIALS],
            subs: [0.0; 4],
            mask: [0.0; 12],
            phase_spread: 1.0,
            protect_gain: 0.0,
            eq: [1.0; 6],
            mask_mix: 0.5,
            mask_lfo: 0.0,
            tremolo: 0.0,
            attack: 0.0,
            decay: 0.0,
            release: 0.0,
            pluck: -4.0,
            main_volume: 0.75,
            main_pitch: -1200.0,
            grit: 0.0,
            vibrato: 0.0,
            pitch_lfo: 0.0,
            porta: 0.0,
            fenv_attack: 1.0,
            fenv_decay: 0.0,
            fenv_tension: [Tension::default(); 2],
            fenv_amount: 0.0,
            fmin: 0.0,
            fmax: 1.0,
            frange: 1.0,
            fcut: 0.5,
            fwidth: 512.0,
            keytrack: 1.0,
            flfo: 0.0,
            res_offset: 0.0,
            noise_len: 2.0,
            noise_rate: 0.0,
            res_ratio: 1.0,
            res_scale: 1024.0,
            res_bias: -0.125,
            unison: 1,
            unison_gain: 1.0,
            fat_pitch: 0.0,
            fat_phase: 0.5,
            phaser_width: 0.5,
            phaser_width_lfo: 0.0,
            phaser_offset_lfo: 0.0,
            phaser_speed: 0.0,
            sfx_width: 0.5,
            sfx_width_lfo: 0.0,
            sfx_strength: 0.0,
            lfo_attack: 1.0,
            lfo_speed: 0.0,
            lfo_key_offset: 1200.0,
            declick: 0.0,
            smoothing: 0.0,
            cutoff_glide: 0.0,
            res_glide: 0.0,
            vib_rate: 0.0,
            vib_attack: 0.0,
            pluck_rate: 0.0,
            pluck_start: 0,
            tremolo_rate: 0.0,
            tremolo_fade: 0,
            tremolo_fade_step: 0.0,
            ramp_cap: 0.001,
        }
    }
}

/// The post effects' settings tables, read out of the DLL's data.
/// Chorus (`0x7ee73c + 0x20 t`): taps a side, then five settings.
const CHORUS: [[i32; 6]; 9] = [
    [0; 6],
    [4, 512, 768, 8, 128, -32],
    [5, 1024, 856, 0, 128, 68],
    [7, 512, 512, 128, 128, 54],
    [6, 561, 854, 0, 0, 38],
    [1, 136, 312, 0, 0, 128],
    [2, 86, 632, 0, 0, 0],
    [1, 42, 944, 0, 0, -22],
    [9, 81, 170, 0, 0, -66],
];

/// Delay (`0x7ee67c + 8 t`): the stereo offset in ms (the right side when
/// positive) and the kind (0 echo, 2 ping-pong).
const DELAY: [[i32; 2]; 4] = [[0, 0], [0, 0], [1, 2], [-20, 2]];

/// Reverb (`0x7ee67c + 0x20 t`): eight settings a type. The fifth from
/// the start is the decay in tenths of a second, the third the predelay
/// in 384ths of a second.
const REVERB: [[i32; 8]; 7] = [
    [0; 8],
    [75, 40, 0, 40, 100, 15, 40, 2],
    [0, 115, 18, 40, 100, 22, 50, 2],
    [34, 21, 27, 90, 100, 45, 31, 3],
    [400, 60, 31, 60, 65, 23, 37, 1],
    [100, 36, 0, 35, 100, 6, 36, 2],
    [100, 60, 0, 17, 100, 5, 30, 2],
];

/// Harmonizer (`0x7ee634 + 0x10 t`): start, add, times, spread.
const HARMONIZER: [[i32; 4]; 5] = [
    [-1, 0, 2, 5],
    [0, 0, 2, 5],
    [1, 0, 2, 5],
    [-1, 1, 1, 10],
    [0, 2, 1, 10],
];

/// The unison pitch spread of each type (`0x7ee5c8 + 0x24 t`).
const FAT_PITCH: [[f32; 9]; 3] = [
    [
        0.877, 1.069, 0.929, 1.181, 1.031, 1.129, 0.887, 1.117, 0.977,
    ],
    [1.0; 9],
    [
        0.890_471_9,
        1.076_426_3,
        0.911_577_05,
        1.102_535_8,
        1.034_126_2,
        1.166_861_2,
        0.853_970_9,
        1.206_272_6,
        1.0,
    ],
];

/// A one-pole of FL's envelope helper (`FUN_005f5f60`, `FUN_005f5fe0`):
/// what is left of the way to the input after a sample.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct OnePole {
    coef: f32,
    y: f32,
}

impl OnePole {
    fn set(&mut self, samples: f64) {
        self.coef = fall_40db(samples) as f32;
    }

    fn hold(&mut self) {
        self.coef = 1.0;
    }

    fn run(&mut self, buf: &mut [f32]) {
        let mut y = self.y;
        for x in buf.iter_mut() {
            y = (y - *x) * self.coef + *x;
            *x = y;
        }
        self.y = if y.abs() > dsp::additive::SILENT {
            y
        } else {
            0.0
        };
    }
}

/// One sounding voice: a note, or one unison copy of it.
#[derive(Debug, Clone, PartialEq)]
struct Voice {
    active: bool,
    id: u32,
    /// Its order of starting, for taking the oldest.
    age: u64,
    /// The note's cents from C5.
    pitch: f32,
    velocity: f32,
    vel01: f32,
    pan: f32,
    mod_x: f32,
    mod_y: f32,
    release_value: f32,
    /// What the note's glides take their time by (`voice+0x24`).
    porta_scale: f32,
    /// The first unison voice of its note: FL's voice record that owns the
    /// others (`+0xa0` 0).
    lead: bool,
    /// The glide (`voice+0x4c..+0x7c`): what it puts on the note's pan,
    /// volume, cents, Mod X, Mod Y and velocity, what it adds to them a
    /// tick, how many ticks it has left (below 0 for one that falls by
    /// `slide_coef` a tick for good).
    slide: [f32; 6],
    slide_step: [f32; 6],
    slide_ticks: i32,
    slide_coef: f32,
    /// The frames left of the tremolo's fade in (`voice+0x80`).
    trem_fade: i32,
    /// The noise resonance's seed (`voice+0x1a8`) and how far through
    /// its hold it is (`+0x1b0`).
    noise_seed: i32,
    noise_time: f64,
    phase_offset: f32,
    pan_offset: f32,
    pitch_offset: f32,
    variance: f32,
    attack_scale: f32,
    // Control state.
    countdown: usize,
    first: bool,
    lfo_phase: f64,
    lfo_att: f32,
    vib_att: f32,
    random: f32,
    fenv_pos: f32,
    fenv_inc: f32,
    fenv_decaying: bool,
    cutoff: f32,
    res_mt: f32,
    last_cents: f32,
    f0_hz: f32,
    log2_f0: f32,
    count: usize,
    pluck_on: bool,
    pluck_tick: i64,
    pluck_ready: usize,
    pluck_cum: [f32; PARTIALS],
    pluck_factor: [f32; PARTIALS],
    levels: [f32; PARTIALS + 12],
    hz: [f32; PARTIALS],
    log2_hz: [f32; PARTIALS],
    alias: [f32; PARTIALS],
    scratch: [f32; PARTIALS + 12],
    bank: SineBank,
    // Envelope.
    state: u8,
    pos: i64,
    end: i64,
    slope: f64,
    env: OnePole,
    declick: OnePole,
    last_ramp: f32,
    gains: [f32; 2],
    mono: [f32; CHUNK],
    env_buf: [f32; CHUNK],
}

impl Voice {
    fn new() -> Self {
        Voice {
            active: false,
            id: 0,
            age: 0,
            pitch: 0.0,
            velocity: 1.0,
            vel01: 0.78,
            pan: 0.0,
            mod_x: 0.0,
            mod_y: 0.0,
            release_value: 0.5,
            porta_scale: 1.0,
            lead: false,
            slide: [0.0; 6],
            slide_step: [0.0; 6],
            slide_ticks: 0,
            slide_coef: 1.0,
            trem_fade: 0,
            noise_seed: 0,
            noise_time: 0.0,
            phase_offset: 0.5,
            pan_offset: 0.0,
            pitch_offset: 0.0,
            variance: 0.0,
            attack_scale: 1.0,
            countdown: 0,
            first: true,
            lfo_phase: 0.0,
            lfo_att: 0.0,
            vib_att: 0.0,
            random: 0.0,
            fenv_pos: 0.0,
            fenv_inc: 0.0,
            fenv_decaying: false,
            cutoff: 0.0,
            res_mt: f32::MAX,
            last_cents: f32::NAN,
            f0_hz: 0.0,
            log2_f0: 0.0,
            count: 0,
            pluck_on: false,
            pluck_tick: 0,
            pluck_ready: 0,
            pluck_cum: [1.0; PARTIALS],
            pluck_factor: [1.0; PARTIALS],
            levels: [0.0; PARTIALS + 12],
            hz: [0.0; PARTIALS],
            log2_hz: [0.0; PARTIALS],
            alias: [0.0; PARTIALS],
            scratch: [0.0; PARTIALS + 12],
            bank: SineBank::new(PARTIALS),
            state: 0,
            pos: 0,
            end: 0,
            slope: 0.0,
            env: OnePole::default(),
            declick: OnePole::default(),
            last_ramp: 0.0,
            gains: [0.0; 2],
            mono: [0.0; CHUNK],
            env_buf: [0.0; CHUNK],
        }
    }
}

/// What a note brings a voice and keeps when legato hands the voice on
/// (the first 0x28 bytes of FL's voice record and its `TVoiceParams`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Note {
    id: u32,
    /// When it started, for the newest held note.
    age: u64,
    pitch: f32,
    velocity: f32,
    vel01: f32,
    pan: f32,
    mod_x: f32,
    mod_y: f32,
    release: f32,
    porta_scale: f32,
}

impl Note {
    /// The levels a glide works on (`FUN_00787ec0`): pan, volume, cents,
    /// Mod X, Mod Y and velocity.
    fn levels(&self, to_volume: bool) -> [f32; 6] {
        let volume = if to_volume { self.velocity } else { 1.0 };
        [
            self.pan, volume, self.pitch, self.mod_x, self.mod_y, self.vel01,
        ]
    }
}

impl Voice {
    fn note(&self, age: u64) -> Note {
        Note {
            id: self.id,
            age,
            pitch: self.pitch,
            velocity: self.velocity,
            vel01: self.vel01,
            pan: self.pan,
            mod_x: self.mod_x,
            mod_y: self.mod_y,
            release: self.release_value,
            porta_scale: self.porta_scale,
        }
    }

    fn take_note(&mut self, note: &Note) {
        self.id = note.id;
        self.pitch = note.pitch;
        self.velocity = note.velocity;
        self.vel01 = note.vel01;
        self.pan = note.pan;
        self.mod_x = note.mod_x;
        self.mod_y = note.mod_y;
        self.release_value = note.release;
        self.porta_scale = note.porta_scale;
    }

    /// Glide from `from` to the voice's own note (`FUN_00784630`): the
    /// glide takes on the difference, and moves it off over `porta`
    /// ticks times the note's scale, a hundredth a time when that is
    /// above 0 and in a straight line over one tick more than it when not.
    /// A new note's glide (`fresh`) leaves its pan, volume and velocity
    /// alone, and `wrap` takes octaves off the cents until they are 1250
    /// or less.
    fn glide(&mut self, from: &[f32; 6], to: &[f32; 6], wrap: bool, fresh: bool, porta: f32) {
        for k in [0, 1, 5] {
            self.slide[k] = if fresh {
                0.0
            } else {
                self.slide[k] + from[k] - to[k]
            };
        }
        for k in 2..5 {
            self.slide[k] = self.slide[k] + from[k] - to[k];
        }
        if wrap && self.slide[2] != 0.0 {
            let octave = 1200f32.copysign(self.slide[2]);
            while f64::from(self.slide[2]).abs() > 1250.0 {
                self.slide[2] -= octave;
            }
        }
        let ticks = porta * self.porta_scale;
        if ticks > 0.0 {
            self.slide_ticks = i32::MIN + 1;
            self.slide_coef = (0.01f64.ln() / f64::from(ticks)).exp() as f32;
        } else {
            let n = 1 - round(f64::from(ticks)) as i32;
            self.slide_ticks = n;
            let by = (-1.0 / f64::from(n)) as f32;
            for k in 0..6 {
                self.slide_step[k] = self.slide[k] * by;
            }
        }
    }

    /// A tick of the glide (`FUN_00784a10`, at the top of the tick).
    fn step_slide(&mut self) {
        if self.slide_ticks < 0 {
            for x in &mut self.slide {
                *x *= self.slide_coef;
            }
        } else if self.slide_ticks > 0 {
            for k in 0..6 {
                self.slide[k] += self.slide_step[k];
            }
            self.slide_ticks -= 1;
        }
    }
}

/// The post chain's three FL classes and what Harmless's setters
/// (`FUN_00789570`, `FUN_00789080`, `FUN_00789280`) make of the state for
/// them. A class that is off is FL's freed object: turning it on again
/// starts it fresh.
#[derive(Debug, Clone, PartialEq)]
struct Effects {
    /// The sleep countdown (0x7b8) and what voices set it to (0x7bc):
    /// 4 ms plus the chorus's ring. The chorus only turns its LFOs and the
    /// delay takes no input while it is under 1.
    sleep: i32,
    hold: i32,
    chorus_on: bool,
    chorus: StereoFlanger,
    /// The dry gain (0x7c8); the wet is in the flanger's voices.
    chorus_dry: f32,
    delay_on: bool,
    delay: Delay32,
    kernel: DelayKernel,
    /// The stereo offset of each side in frames (0x7dc, 0x7e0), and the
    /// input gain (0x7e4).
    delay_offsets: [usize; 2],
    delay_gain: f32,
    reverb_on: bool,
    reverb: FruityReverb,
    /// The wet gain (0x7f8).
    reverb_wet: f32,
}

/// Harmless.
pub struct Harmless {
    state: State,
    tables: Tables,
    d: Box<Derived>,
    filter: ShapeTable,
    resonance: ShapeTable,
    phaser: ShapeTable,
    voices: Vec<Voice>,
    next_age: u64,
    sample_count: u64,
    global_lfo: f64,
    vibrato_phase: f32,
    phaser_phase: f64,
    seed: u32,
    /// The notes held under the one that sounds, in legato: FL's voice
    /// records of type 2.
    held: [Note; HELD],
    held_len: usize,
    /// The note whose voices answer for the notes (FL's voice at 0x340)
    /// and the levels the last one left (0x348..0x35c), which a glide
    /// starts from: `None` is FL's 0x7fffffff in its cents.
    lead: Option<u32>,
    lead_age: u64,
    last: Option<[f32; 6]>,
    /// The tremolo's depth as it glides to the wheel's (0x7a0), and its
    /// gains for the block, left and right side by side (set 0 +0x30).
    trem_depth: f32,
    trem: [f32; 2 * CHUNK],
    compression: Option<Box<Maximus>>,
    compression_states: [Option<Vec<u8>>; 4],
    missing_compression: bool,
    fx: Box<Effects>,
    left: [f32; CHUNK],
    right: [f32; CHUNK],
    mono_in: [f32; CHUNK],
    wet_left: [f32; CHUNK],
    wet_right: [f32; CHUNK],
}

impl Default for Harmless {
    fn default() -> Self {
        let mut plugin = Harmless {
            state: State::default(),
            tables: Tables::new(),
            d: Box::new(Derived::new(44_100.0, 120.0)),
            filter: shape(0),
            resonance: shape(100),
            phaser: shape(200),
            voices: (0..VOICES).map(|_| Voice::new()).collect(),
            next_age: 0,
            sample_count: 0,
            global_lfo: 0.0,
            vibrato_phase: 0.0,
            phaser_phase: 0.0,
            seed: 0x1234_5678,
            held: [Note::default(); HELD],
            held_len: 0,
            lead: None,
            lead_age: 0,
            last: None,
            trem_depth: 0.0,
            trem: [0.0; 2 * CHUNK],
            compression: None,
            compression_states: [None, None, None, None],
            missing_compression: false,
            fx: Box::new(Effects {
                sleep: 0,
                hold: 0,
                chorus_on: false,
                chorus: StereoFlanger::default(),
                chorus_dry: 1.0,
                delay_on: false,
                delay: Delay32::default(),
                kernel: DelayKernel::Straight,
                delay_offsets: [0; 2],
                delay_gain: 0.0,
                reverb_on: false,
                reverb: reverb_for(44_100),
                reverb_wet: 0.0,
            }),
            left: [0.0; CHUNK],
            right: [0.0; CHUNK],
            mono_in: [0.0; CHUNK],
            wet_left: [0.0; CHUNK],
            wet_right: [0.0; CHUNK],
        };
        plugin.load();
        plugin
    }
}

impl Harmless {
    /// The state the plugin has now.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Everything that follows from a new state or rate: the shape
    /// curves, the effect lines and the derived settings. This allocates.
    fn load(&mut self) {
        let s = &self.state.selectors;
        self.filter = shape(s[0]);
        self.resonance = shape(s[1] + 100);
        self.phaser = shape(s[2] + 200);
        self.build_effects();
        self.derive();
        self.build_compression();
    }

    fn param(&self, i: usize) -> i32 {
        self.state.params[i]
    }

    fn on(&self, i: usize) -> bool {
        self.state.params[i] > 0
    }

    /// What `ProcessParam` and the image rebuilds make of every parameter
    /// (`FUN_0077f630`, `FUN_00787990`, `FUN_00787b40`, `FUN_00787ce0`,
    /// `FUN_0077c170`, `FUN_00788060`, `FUN_0077f170`). Allocates nothing.
    fn derive(&mut self) {
        let p = self.state.params;
        let sr = self.d.rate;
        let quality = self.state.selectors[6].clamp(0, 2) as usize;
        let colour = self.state.selectors[4];
        let image = self.state.image.as_deref();
        let tables = &self.tables;
        let d = &mut *self.d;
        let k = |v: i32| f64::from(v) / 1000.0;

        let base = [88.0, 44.0, 8.0][quality];
        d.tick = ((round(base / 4.0 * sr / 44_100.0) * 4.0) as usize).max(4);
        d.limit_hz = LIMIT_HZ;

        // The colour, its EQ and the detune.
        d.raw[0] = 1.0;
        d.phase[0] = 0.0;
        for i in 0..IMAGE {
            let (level, phase) = timbre_point(colour, i as f64, image);
            d.raw[i + 4] = level;
            d.phase[i + 4] = phase;
        }
        d.raw[1] = d.raw[4];
        d.phase[1] = d.phase[4];
        for (slot, n) in [(2, 0.5), (3, 1.5)] {
            let (level, phase) = timbre_point(colour, n, image);
            d.raw[slot] = level;
            d.phase[slot] = phase;
        }
        for (b, eq) in d.eq.iter_mut().enumerate() {
            *eq = level3(f64::from(p[5 + b] + 1024) / 1024.0) as f32;
        }
        for i in 0..PARTIALS {
            let weight: f32 = (0..6).map(|b| tables.bands[b][i] * d.eq[b]).sum();
            d.timbre[i] = d.raw[i] * weight;
        }
        let detune = (f64::from(p[39]) / 1200.0).exp2();
        d.ratio[0] = 1.0;
        d.ratio[1] = 0.5;
        d.ratio[2] = detune * 0.5 + 1.0;
        d.ratio[3] = detune * 1.5 + 1.0;
        for i in 4..PARTIALS {
            d.ratio[i] = 1.0 + (i - 4) as f64 * detune;
        }
        for i in 0..PARTIALS {
            d.grit_table[i] = ((d.ratio[i] - 1.0) * 0.418_879_020_478_639_06).sin();
        }
        d.grit_table[0] = 0.0;
        d.limit_hz = LIMIT_HZ / detune * if quality == 1 { 1.5 } else { 1.0 };
        if quality == 2 {
            d.limit_hz = f64::INFINITY;
        }

        d.phase_spread = (k(p[0]).signum() * k(p[0]) * k(p[0])) as f32;
        d.protect_gain = level(f64::from(p[1]) / 1024.0) as f32;
        d.subs = [
            (f64::from(p[62]) * 0.003_906_25) as f32,
            p[2] as f32 / 1024.0,
            p[3] as f32 / 1024.0,
            p[4] as f32 / 1024.0,
        ];
        for (j, m) in d.mask.iter_mut().enumerate() {
            let v = k(p[11 + j]);
            *m = (level(v.abs()) * v.signum()) as f32;
        }
        d.mask_mix = k(p[23]) as f32;
        d.mask_lfo = k(p[24]) as f32;
        d.tremolo = p[25] as f32 * 0.001f32;
        d.attack = sr * (((k(p[26]) * 101f64.ln()).exp() - 1.0) as f32 as f64) * 0.05;
        d.decay = if p[27] < 1000 {
            sr * (((k(p[27]) * 1001f64.ln()).exp() - 1.0) as f32 as f64 * 0.11995 + 0.05)
        } else {
            -4.0
        };
        d.release = sr * (((k(p[28]) * 101f64.ln()).exp() - 1.0) as f32 as f64 * 0.0249 + 0.01);
        d.pluck = if p[31] < 1000 {
            ((k(p[31]) * 1001f64.ln()).exp() - 1.0) as f32 as f64 * 0.11975
        } else {
            -4.0
        };
        d.main_volume = (level3(k(p[36])) * 0.75) as f32;
        d.main_pitch = (p[38] - 1200) as f32;
        d.grit = {
            let v = k(p[40]);
            let e = ((v.abs() as f32 as f64 * 65f64.ln()).exp() - 1.0) as f32;
            (f64::from(e * v.signum() as f32) * -0.0125) as f32
        };
        d.vibrato = (f64::from(p[41]) * 0.075) as f32;
        d.pitch_lfo = {
            let v = k(p[42]);
            let e = ((v.abs() as f32 as f64 * 101f64.ln()).exp() - 1.0) as f32;
            (f64::from(e * v.signum() as f32) * 12.0) as f32
        };
        // The glide time in ticks, signed (`ProcessParam` 45, 0x548): above
        // 0 a glide falls to a hundredth in it, at or below 0 it runs
        // straight over one tick more than its size.
        d.porta = {
            let v = p[45];
            let e =
                ((f64::from(v.unsigned_abs() as f32 * 0.001f32) * 41f64.ln()).exp() - 1.0) as f32;
            let sr = (f64::from(v.signum()) * sr.round()) as f32;
            (f64::from(sr * e) / (d.tick as f64 * 26.666_666_666_666_668)) as f32
        };
        d.fenv_attack = if p[48] < 1000 {
            let e = ((k(p[48]) * 1001f64.ln()).exp() - 1.0) as f32;
            (1.0 / (f64::from((sr as f32) * e) * 0.12 + 1.0)) as f32
        } else {
            0.0
        };
        d.fenv_decay = if p[49] < 1000 {
            let e = ((k(p[49]) * 1001f64.ln()).exp() - 1.0) as f32;
            (-1.0 / (sr * f64::from(e) * 0.12 + 1.0)) as f32
        } else {
            0.0
        };
        d.fenv_tension = [Tension::new(p[50]), Tension::new(p[51])];

        // The filter's range follows its curve (FUN_0077c170).
        d.fwidth = (((1.0 - f64::from(p[56]) * 0.004) * LN_2).exp() * 1024.0) as f32;
        let reach = |ext: f32| {
            let e = (682.666_666_666_666_6 / f64::from(d.fwidth)) as f32 * ext;
            e - e.signum() * e.min(1.0)
        };
        d.fmin = (filter_floor_hz().log2() - f64::from(reach(self.filter.below))) as f32;
        d.fmax = (LIMIT_HZ.log2() + f64::from(reach(self.filter.above))) as f32;
        d.frange = d.fmax - d.fmin;
        d.fcut = (f64::from(d.fmin) + f64::from(p[54]) / 8192.0 * f64::from(d.frange)) as f32;
        d.fenv_amount = k(p[52]) as f32 * if p[47] > 0 { 1.0 } else { d.frange };
        d.keytrack = k(p[57]) as f32;
        d.flfo = (k(p[58]) * 6.0) as f32;
        d.res_offset = (f64::from(p[61]) / 1200.0) as f32;
        // `ProcessParam` 61: the noise holds `(201^x - 1) 2.49 + 2.0` ms,
        // x the wheel from 0 to 1.
        let x = (f64::from(p[61] + 2400) * 0.000_208_333_333_333_333_35) as f32;
        let e = ((f64::from(x) * 201f64.ln()).exp() - 1.0) as f32;
        d.noise_len = f64::from(e) * 2.490_022_675_736_961_2 + 1.995_464_852_607_709_7;
        d.noise_rate = d.tick as f64 / (d.noise_len * 0.001 * sr.round());
        d.res_ratio = f64::from(d.res_offset).exp2() as f32;
        let (scale, bias) = match (p[60] > 0, p[63] > 0) {
            (false, false) => (1024.0, -0.125),
            (false, true) => ((-27.0f64 / 12.0).exp2() * 450_560.0, 0.875),
            (true, false) => (0.384, -1.5),
            (true, true) => ((-15.0f64 / 12.0).exp2() * 168.96, 0.5),
        };
        d.res_scale = scale as f32;
        d.res_bias = bias as f32;

        d.unison = p[65].clamp(1, 9) as usize;
        d.unison_gain = if d.unison < 2 {
            1.0
        } else {
            let x = 1.0 - 1.0 / d.unison as f64;
            1.0 - ((x as f32 as f64 * 3f64.ln()).exp() - 1.0) as f32 * 0.5
        };
        d.fat_pitch = {
            let e = ((f64::from(p[67]) / 256.0 * 41f64.ln()).exp() - 1.0) as f32;
            (f64::from(e) * 1.875) as f32
        };
        d.fat_phase = p[68] as f32 / 128.0;

        d.phaser_width = (f64::from(p[73]) * 0.0001) as f32;
        d.phaser_width_lfo = (f64::from(p[74]) * 0.0005) as f32;
        d.phaser_offset_lfo = (f64::from(p[76]) * 0.0005) as f32;
        let beat = d.tempo / (sr * 3840.0);
        d.phaser_speed = f64::from(p[77]).signum() * (k(p[77]).abs() * LN_2).exp() * beat;
        d.sfx_width = k(p[80]) as f32;
        d.sfx_width_lfo = k(p[81]) as f32;
        d.sfx_strength = ((f64::from(p[83]) / 256.0 * LN_2).exp() * 1.331_000_355_909_495_2) as f32;
        d.lfo_attack = {
            let e = ((k(p[88]) * 101f64.ln()).exp() - 1.0) as f32;
            (1.0 / (f64::from(sr as f32 * e) * 0.05 + 1.0)) as f32
        };
        d.lfo_speed = f64::from(p[89]).signum() * (k(p[89]).abs() * LN_2).exp() * beat;
        d.lfo_key_offset = (f64::from(p[89]) * 0.6 + 1200.0) as f32;

        d.declick = round(sr * 0.03);
        d.smoothing = fall_40db(sr * 0.01) as f32;
        d.cutoff_glide = fall_40db(sr * 0.1 / d.tick as f64) as f32;
        d.res_glide = fall_40db(sr * 1.25 / d.tick as f64) as f32;
        d.vib_rate = (6.0 / sr) as f32;
        d.vib_attack = (3.0 / sr) as f32;
        d.pluck_rate = 0.01f64.ln() * d.tick as f64 / sr;
        d.pluck_start = 1 - round(sr * 0.002 / d.tick as f64) as i64;
        // `FUN_0077f170`.
        d.tremolo_rate = (2.5 / sr.round()) as f32;
        d.tremolo_fade = round(sr.round() * 0.1) as i32;
        d.tremolo_fade_step = (1.0 / f64::from(d.tremolo_fade + 1)) as f32;
        d.ramp_cap = (0.001 * 44_100.0 / sr) as f32;

        self.set_effects();
    }

    /// The setters of the chorus (`FUN_00789570`), the delay
    /// (`FUN_00789080`) and the reverb (`FUN_00789280`), on the classes
    /// [`Harmless::build_effects`] made. Allocates nothing.
    fn set_effects(&mut self) {
        let p = self.state.params;
        let rate = self.d.rate;
        let hz = rate.round() as i32;
        let fx = &mut *self.fx;

        // The chorus: a table of the six flanger wheels, the mix as a dry
        // and a wet of 1 - (5^x - 1) / 4 crossing at the middle.
        fx.hold = round(f64::from(hz) * 0.004) as i32;
        let kind = p[90].clamp(0, 8) as usize;
        fx.chorus_on = kind > 0 && {
            if !fx.chorus_on {
                fx.chorus.fresh();
            }
            let mix = p[91] as f32 * 0.001;
            let law = |x: f32| 1.0 - ((f64::from(x) * 5f64.ln()).exp() - 1.0) as f32 * 0.25;
            fx.chorus_dry = law(mix);
            let settings = FlangerSettings::from_wheels(CHORUS[kind], law(1.0 - mix));
            fx.chorus.set(&settings, f64::from(hz));
            fx.hold += BLOCK as i32 + fx.chorus.length();
            true
        };

        // The delay: 192 steps a beat, the input on the level curve, and a
        // stereo offset on one side.
        let kind = p[92].clamp(0, 3) as usize;
        fx.delay_on = kind > 0 && fx.delay.capacity() > 0 && {
            if !fx.delay_on {
                fx.delay.fresh();
            }
            let [offset_ms, kernel] = DELAY[kind];
            fx.kernel = if kernel == 0 {
                DelayKernel::Straight
            } else {
                DelayKernel::PingPong
            };
            let offset = round(f64::from(offset_ms.abs()) * 0.001 * f64::from(hz)) as usize;
            let tempo = f64::from(self.d.tempo as f32);
            let time = round(f64::from(p[95] * hz) * 0.3125 / tempo) as usize;
            fx.delay.set_length(BLOCK + offset, time);
            fx.delay_offsets = if offset_ms > 0 {
                [0, offset]
            } else {
                [offset, 0]
            };
            fx.delay.set_feedback(p[94] as f32 * 0.001);
            fx.delay_gain = fl_level(p[93] as f32 * 0.001);
            true
        };

        // The reverb: a table of the class's eight settings, the wet on
        // the level curve, halved.
        let kind = p[96].clamp(0, 6) as usize;
        fx.reverb_on = kind > 0 && {
            if !fx.reverb_on {
                fx.reverb.fresh();
            }
            let t = REVERB[kind];
            fx.reverb.set(ReverbSettings {
                low_cut: t[0] as f32,
                high_cut: t[1] as f32,
                predelay: round(f64::from(t[2] * hz) / 384.0) as i32,
                room: t[3],
                diffusion: t[4],
                decay: (f64::from(t[5] * hz) / 10.0) as f32,
                damping: t[6] as f32,
                colour: t[7],
            });
            fx.reverb_wet = (f64::from(fl_level(p[97] as f32 * 0.001)) * 0.5) as f32;
            true
        };
    }

    /// The memory of the three classes at the rate. The delay has room
    /// for its longest time at 20 BPM and is only made when the state has
    /// it on. This allocates, and starts every class fresh.
    fn build_effects(&mut self) {
        let hz = self.d.rate.round() as i32;
        let fx = &mut *self.fx;
        fx.chorus.make_room(f64::from(hz));
        fx.chorus.fresh();
        fx.chorus_on = false;
        let delay = if self.state.params[92] > 0 {
            let longest = 384.0 * 0.3125 / SLOWEST_TEMPO * f64::from(hz);
            BLOCK + (0.02 * f64::from(hz)) as usize + 1 + longest as usize + 1
        } else {
            0
        };
        if fx.delay.capacity() != delay {
            fx.delay = Delay32::with_capacity(delay);
        }
        fx.delay.fresh();
        fx.delay_on = false;
        if fx.reverb.rate() != hz {
            fx.reverb = reverb_for(hz);
        }
        fx.reverb.fresh();
        fx.reverb_on = false;
        fx.sleep = 0;
    }

    /// The compression Maximus for the state's preset, from the files read
    /// in [`Generator::set_resource_dir`].
    fn build_compression(&mut self) {
        let which = self.state.selectors[3];
        self.compression = None;
        self.missing_compression = false;
        if which <= 0 {
            return;
        }
        let Some(bytes) = &self.compression_states[(which - 1) as usize] else {
            self.missing_compression = true;
            return;
        };
        let mut maximus = Box::new(Maximus::default());
        maximus.set_sample_rate(self.d.rate as f32);
        if maximus.set_state(bytes).is_err() {
            self.missing_compression = true;
            return;
        }
        maximus.set_param(5, self.state.params[98] as f32);
        self.compression = Some(maximus);
    }

    fn random(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(0x0808_8405).wrapping_add(1);
        (f64::from(self.seed) * 2.328_306_437_080_797_4e-10) as f32
    }

    /// Start the unison copies of a note (`FUN_00782940`).
    fn start(&mut self, note: NoteOn) {
        let d = &*self.d;
        let p = self.state.params;
        let host_pitch = f32::from(note.key - 60) * 100.0 + note.fine_pitch;
        let vel01 = ((f64::from(note.velocity) * 10.0 + 1.0).ln() / 11f64.ln() * 100.0 / 128.0)
            .clamp(0.0, 1.0) as f32;
        // The note's glide time scale (`FUN_00782810`).
        let porta_scale = if p[46] > 0 {
            let s = (1.0 - f64::from(vel01) * 0.85) as f32;
            s * s
        } else {
            1.0
        };
        let this = Note {
            id: note.voice,
            age: self.next_age,
            pitch: host_pitch,
            velocity: note.velocity,
            vel01,
            pan: note.pan,
            mod_x: note.mod_x,
            mod_y: note.mod_y,
            release: note.release,
            porta_scale,
        };
        // Legato (`MonoBox`; FL's notes of the 14th colour turn it round,
        // and a note here is of the first): while a note sounds, a new
        // one takes its voices over and glides them to itself.
        if self.on(44)
            && let Some(lead) = self.lead
            && self.voices.iter().any(|v| v.active && v.id == lead)
        {
            self.legato(lead, this);
            return;
        }
        let porta = (self.on(43) || note.porta) && self.last.is_some();
        let n = d.unison;
        let global_random = if self.on(87) {
            Some(self.random())
        } else {
            None
        };
        let spreads = FAT_PITCH[self.state.params[70].clamp(0, 2) as usize];
        for (i, spread_of) in spreads.iter().enumerate().take(n) {
            let r = global_random.unwrap_or_else(|| self.random());
            // A seed of its own for the noise resonance (`FUN_0074bb70`).
            self.random();
            let self_seed = self.seed as i32;
            let slot = self.free_slot();
            let age = self.next_age;
            self.next_age += 1;
            let d = &*self.d;
            let voice = &mut self.voices[slot];
            voice.active = true;
            voice.id = note.voice;
            voice.age = age;
            voice.take_note(&this);
            voice.lead = i == 0;
            voice.slide = [0.0; 6];
            voice.slide_step = [0.0; 6];
            voice.slide_ticks = 0;
            voice.trem_fade = d.tremolo_fade;
            if n == 1 {
                voice.phase_offset = 0.5;
                voice.pan_offset = 0.0;
                voice.pitch_offset = 0.0;
                voice.variance = 0.0;
                voice.attack_scale = 1.0;
            } else {
                let mut phase = i as f32 / n as f32 * d.fat_phase;
                let x = (i as f32 / (n - 1) as f32) * 2.0 - 1.0;
                voice.pan_offset = x / 128.0 * p[66] as f32;
                let mut s = x.abs();
                if i & 1 == 1 {
                    s = -s;
                }
                if n & 1 == 1 && n < i * 2 {
                    s = -s;
                }
                let spread = s * spread_of;
                voice.pitch_offset = spread * d.fat_pitch;
                if p[70] == 2 {
                    let w = f64::from(d.fat_phase * d.fat_phase).powi(4);
                    phase = ((f64::from(voice.pitch_offset) * LN_2 / 1200.0).exp() * 100_000.0 * w)
                        as f32;
                }
                voice.phase_offset = (f64::from(phase) + 0.5 - round(f64::from(phase))) as f32;
                voice.attack_scale =
                    ((f64::from(s.abs()) - 1.0) * f64::from(p[69]) * (1.0 / 128.0) * 3.0 * LN_2)
                        .exp() as f32;
                voice.variance = s * p[69] as f32 / 256.0;
            }
            voice.random = r;
            voice.noise_seed = self_seed;
            voice.noise_time = 0.0;
            voice.countdown = 0;
            voice.first = true;
            voice.lfo_phase = 0.0;
            voice.lfo_att = d.lfo_attack;
            voice.vib_att = d.vib_attack;
            voice.fenv_pos = 0.0;
            voice.fenv_inc = d.fenv_attack * d.tick as f32;
            voice.fenv_decaying = false;
            voice.cutoff = d.fcut;
            voice.res_mt = f32::MAX;
            voice.last_cents = f32::NAN;
            voice.count = 0;
            voice.pluck_on = d.pluck >= 0.0 && p[35] == 0;
            voice.pluck_ready = 0;
            voice.bank.silence();
            voice.bank.set_len(0);
            voice.bank.set_smoothing(d.smoothing);
            // The envelope.
            let mut attack = d.attack * f64::from(voice.attack_scale);
            if p[29] > 0 {
                attack *= f64::from(1.0 - (vel01 * 1.125).min(1.0));
            }
            let declick = attack.min(d.declick);
            voice.declick.set(declick);
            voice.declick.y = if attack <= 0.0 { 1.0 } else { 0.0 };
            let ramp_floor = d.rate * 0.2;
            let mut start = 1i64;
            if attack < ramp_floor {
                let left = 1.0 - attack / ramp_floor;
                start = round(ramp_floor * left * left) as i64 + 1;
                attack = ramp_floor;
            }
            voice.state = 1;
            voice.pos = start;
            voice.end = round(attack + declick) as i64 + 1;
            voice.slope = 1.0 / (attack + 1.0);
            voice.last_ramp = (start as f64 * voice.slope) as f32;
            voice.env = OnePole::default();
            voice.pluck_tick = d.pluck_start.min(-(round(d.attack / d.tick as f64) as i64));
            voice.gains = [0.0; 2];
        }
        // Portamento (`PortaBox`; the 15th colour turns it round): every
        // voice of the note glides from the last note's levels.
        let to = this.levels(self.on(37));
        if porta && let Some(from) = self.last {
            let porta_ticks = self.d.porta;
            for voice in self.voices.iter_mut() {
                if voice.active && voice.id == this.id {
                    voice.glide(&from, &to, true, true, porta_ticks);
                }
            }
        }
        self.lead = Some(this.id);
        self.lead_age = this.age;
        self.last = Some(to);
    }

    /// A new note while `lead` sounds, in legato: FL makes a voice record
    /// for the new note that sounds nothing, then `FUN_00787ef0` swaps
    /// it with the lead's (`FUN_00784560`), so the lead's voices go on
    /// under the new note and the old one waits among the held notes.
    fn legato(&mut self, lead: u32, this: Note) {
        let Some(old) = self
            .voices
            .iter()
            .find(|v| v.active && v.id == lead && v.lead)
            .or_else(|| self.voices.iter().find(|v| v.active && v.id == lead))
            .map(|v| v.note(self.lead_age))
        else {
            return;
        };
        self.next_age += 1;
        self.hold(old);
        self.hand_on(&old, this);
        self.lead_age = this.age;
    }

    /// Keep a note among the held ones; when they are full the oldest
    /// goes.
    fn hold(&mut self, note: Note) {
        if self.held_len == HELD {
            self.held.copy_within(1.., 0);
            self.held_len -= 1;
        }
        self.held[self.held_len] = note;
        self.held_len += 1;
    }

    /// Give the voices of `from` to `to` and glide them over
    /// (`FUN_00787ef0` after the swap): the glide starts from the old
    /// note's levels and the new note's become the last.
    fn hand_on(&mut self, from: &Note, to: Note) {
        let on37 = self.on(37);
        let start = from.levels(on37);
        let end = to.levels(on37);
        let porta = self.d.porta;
        for voice in self.voices.iter_mut() {
            if voice.active && voice.id == from.id {
                voice.take_note(&to);
                voice.glide(&start, &end, false, false, porta);
            }
        }
        self.lead = Some(to.id);
        self.last = Some(end);
    }

    /// The voice to use for a new one: a silent one, or the oldest.
    fn free_slot(&self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return i;
        }
        let oldest = self.voices.iter().enumerate().min_by_key(|(_, v)| v.age);
        oldest.map_or(0, |(i, _)| i)
    }

    /// Let go of a voice (`FUN_00783910`).
    fn release(voice: &mut Voice, d: &Derived) {
        if voice.state >= 6 {
            return;
        }
        voice.state = 6;
        voice.end = -1;
        // The filter envelope goes to its decay from where it is.
        if voice.fenv_inc > 0.0 {
            let value = d.fenv_tension[0].at(voice.fenv_pos.clamp(0.0, 1.0));
            voice.fenv_inc = d.fenv_decay * d.tick as f32;
            voice.fenv_decaying = true;
            voice.fenv_pos = d.fenv_tension[1].inverse(value);
        }
        // The pluck starts now if it has not (it waited for the release
        // with `PluckRel`), and no later than now if it has.
        if d.pluck >= 0.0 {
            voice.pluck_on = true;
            voice.pluck_tick = voice.pluck_tick.max(0);
        }
    }

    /// The LFO's value for a voice at a tick: bipolar and unipolar.
    fn lfo(&self, voice: &Voice, fenv: f32, cents: f32) -> (f32, f32) {
        let d = &*self.d;
        let att2 = voice.lfo_att * voice.lfo_att;
        let shape = self.param(86);
        let x = voice.lfo_phase + f64::from(voice.variance);
        let bipolar = |v: f32| (v * att2, ((f64::from(v * att2) + 1.0) * 0.5) as f32);
        match shape {
            9 => (fenv * att2, fenv * att2),
            10 => {
                let v = (voice.cutoff - d.fmin) / d.frange * att2;
                (v, v)
            }
            11 => {
                let u = voice.random * att2;
                (u * 2.0 - 1.0, u)
            }
            12 => {
                let v = (voice.vel01 + voice.slide[5]) * att2;
                (v, v)
            }
            13 => {
                let v = ((cents + d.lfo_key_offset) as f64 / 3600.0).clamp(-1.0, 1.0) as f32;
                bipolar(v)
            }
            1 => {
                let f = ((x - round(x)) * TAU) as f32;
                let v: f32 = (0..12)
                    .map(|i| (f * (i + 1) as f32).sin() * self.tables.lfo_harmonics[i])
                    .sum();
                bipolar(v)
            }
            2 => {
                let s = (((x - round(x)) + 0.5) * TAU).sin() as f32;
                let c = 1.0 - s.abs();
                let c3 = c * c * c;
                bipolar(s.signum() * (1.0 - c3 * c3))
            }
            3 => {
                let y = x - 0.5;
                bipolar(((y - round(y)) * 2.0) as f32)
            }
            4 => {
                let q = round(x * 2.0 - 0.5) as i64;
                bipolar(((q & 1) * 2 - 1) as f32)
            }
            5 => {
                let q = round(x * 3.0 - 0.5) as i64;
                bipolar((q.rem_euclid(3) - 1) as f32)
            }
            6 => {
                let y = x + 0.25;
                let t = (y - round(y - 0.5)) * 4.0 - 2.0;
                bipolar((t.abs() - 1.0) as f32)
            }
            7 => {
                let y = x - 0.5;
                let t = ((y - round(y)) + 0.5) as f32;
                bipolar(t * t * t * 2.0 - 1.0)
            }
            8 => {
                let a = (x + 0.5) * TAU;
                bipolar((((a).sin() + ((a + 7.0) * 0.2).sin()) * 0.5) as f32)
            }
            _ => bipolar((((x - round(x)) + 0.5) * TAU).sin() as f32),
        }
    }

    /// The harmonizer (`FUN_00784220`): the levels again at multiples of
    /// each harmonic.
    fn harmonize(&self, voice: &mut Voice, lfo01: f32) {
        let d = &*self.d;
        let mut width = d.sfx_width;
        if self.on(82) {
            width *= voice.vel01 + voice.slide[5];
        }
        let swing = if d.sfx_width_lfo <= 0.0 {
            d.sfx_width_lfo.max(-width)
        } else {
            d.sfx_width_lfo.min(1.0 - width)
        };
        width += swing * lfo01;
        if width <= 0.0 {
            return;
        }
        let [start, add, times, spread] = HARMONIZER[self.param(84).clamp(0, 4) as usize];
        let step_left = 1.0 / (width * spread as f32 + 1.0);
        let mix = self.param(79) as f32 / 1000.0;
        let harmonics = voice.count.saturating_sub(4);
        voice.scratch[..harmonics].copy_from_slice(&voice.levels[4..4 + harmonics]);
        let mut left = 1.0f32;
        let mut step = 2i32;
        let mut total = 1.0f32;
        loop {
            left -= step_left;
            if left <= 0.0 {
                break;
            }
            let gain = (left * left * left * d.sfx_strength).min(1.0) * mix * mix;
            let first = step + start;
            if first < harmonics as i32 {
                let count = ((harmonics as i32 - first - 1) / step + 1) as usize;
                for j in 0..count {
                    let at = first as usize + j * step as usize;
                    voice.levels[4 + at] += voice.scratch[j] * gain;
                }
            }
            total += gain;
            step = step * times + add;
        }
        let norm = 1.0 / total;
        for v in &mut voice.levels[4..4 + harmonics] {
            *v *= norm;
        }
    }

    /// One control tick of a voice (`FUN_00784a10`, first loop).
    fn control(&self, voice: &mut Voice) {
        let d = &*self.d;
        let p = &self.state.params;
        let tick = d.tick as f32;
        // Below quality 2 the curves are read at the point below
        // (`FUN_0077c450` sets each table's quality).
        let nearest = self.state.selectors[6] < 2;
        voice.step_slide();
        // The filter envelope.
        voice.fenv_pos += voice.fenv_inc;
        if voice.fenv_pos >= 1.0 && voice.fenv_inc > 0.0 {
            let over = (voice.fenv_pos - 1.0) / voice.fenv_inc;
            voice.fenv_inc = d.fenv_decay * tick;
            voice.fenv_pos = 1.0 + over * voice.fenv_inc;
            voice.fenv_decaying = true;
        }
        voice.fenv_pos = voice.fenv_pos.max(0.0);
        let curve = d.fenv_tension[usize::from(voice.fenv_decaying)];
        let fenv = curve.at(voice.fenv_pos.min(1.0));
        // The cutoff glides to the wheel.
        voice.cutoff = d.fcut + (voice.cutoff - d.fcut) * d.cutoff_glide;
        // Pitch.
        let mut cents = voice.pitch + d.main_pitch + voice.slide[2];
        if p[41] > 0 {
            let vib = (self.vibrato_phase * std::f32::consts::TAU).sin();
            cents += vib * d.vibrato * voice.vib_att * voice.vib_att;
        }
        let (lfo, lfo01) = self.lfo(voice, fenv, cents);
        let fat = if p[99] > 0 {
            voice.pitch_offset * (voice.vel01 + voice.slide[5])
        } else {
            voice.pitch_offset
        };
        cents += d.pitch_lfo * lfo + fat;

        // Frequencies, when the pitch moved.
        if cents != voice.last_cents {
            let log2_f0 = (440f64.log2() + ((f64::from(cents) * 0.01 + 60.0) - 57.0) / 12.0) as f32;
            let f0 = f64::from(log2_f0).exp2();
            voice.log2_f0 = log2_f0;
            voice.f0_hz = f0 as f32;
            let count = (round((d.limit_hz / f0).min(1e6)) as i64).max(1) as usize + 4;
            let count = ((count.min(PARTIALS) + 7) & !7).min(PARTIALS);
            let grown = count > voice.count;
            let old = voice.count;
            voice.count = count;
            voice.bank.set_len(count);
            let inv = 1.0 / d.rate;
            for k in 0..count {
                let mut f = d.ratio[k] * f0 * inv;
                if d.grit != 0.0 {
                    f += d.grit_table[k] * f0 * f64::from(d.grit) * inv;
                }
                if k > 0 {
                    voice.bank.set_freq(k, f);
                }
                let hz = (f * d.rate) as f32;
                voice.hz[k] = hz;
                voice.log2_hz[k] = hz.max(1e-6).log2();
                voice.alias[k] = alias_weight(hz);
            }
            if grown && !voice.first && old > 4 {
                // New partials join in step with the fundamental.
                let elapsed = voice.bank.phase(4) - d.phase[4];
                for k in old..count {
                    voice.bank.set_phase(k, d.phase[k] + d.ratio[k] * elapsed);
                }
            }
            voice.last_cents = cents;
        }
        let n = voice.count;

        // The mask (FUN_00783f90).
        let mut mix = d.mask_mix;
        let swing = if d.mask_lfo <= 0.0 {
            d.mask_lfo.max(-mix)
        } else {
            d.mask_lfo.min(1.0 - mix)
        };
        mix += swing * lfo01;
        voice.levels[..n].copy_from_slice(&d.timbre[..n]);
        if mix > 0.0 {
            for j in 0..n.saturating_sub(4) {
                voice.levels[4 + j] *= d.mask[j % 12] * mix + (1.0 - mix);
            }
        }
        let sfx = p[79] > 0;
        let sfx_at = p[85];
        if sfx && sfx_at == 0 {
            self.harmonize(voice, lfo01);
        }

        // The filter.
        let keyed = (f64::from(voice.log2_f0) - 7.0) as f32 * d.keytrack;
        let floor = d.fmin + keyed;
        let mut fc = voice.cutoff + keyed + (voice.mod_x + voice.slide[3]) * d.frange * 0.5;
        fc = fc.clamp(floor, d.fmax);
        let mut res_x = p[59] as f32 / 1000.0 + (voice.mod_y + voice.slide[4]) * 0.5;
        res_x = res_x.clamp(0.0, 1.0);
        let mut resonance = (level(f64::from(res_x)) * 0.25) as f32;
        let lfo_room = if d.flfo <= 0.0 {
            d.flfo.max(floor - fc)
        } else {
            (d.fmax - fc).min(d.flfo)
        };
        fc += lfo01 * lfo_room;
        let mut amount = d.fenv_amount;
        if p[53] > 0 {
            amount *= voice.vel01 + voice.slide[5];
        }
        if p[47] == 0 {
            fc += (fenv - 0.5) * amount;
        } else if amount <= 0.0 {
            fc -= (floor - fc) * fenv * amount;
        } else {
            fc += (d.fmax - fc) * fenv * amount;
        }
        let cut_hz = f64::from(fc).exp2() as f32;
        let w = d.fwidth;
        if p[55] == 0 {
            let offset = self.filter.centre - fc * w;
            for k in 0..n {
                voice.levels[k] *= clamped(&self.filter, nearest, voice.log2_hz[k], w, offset);
            }
        } else {
            let scale = w * 0.9 / cut_hz;
            let offset = self.filter.centre - w * 0.9;
            for k in 0..n {
                voice.levels[k] *= clamped(&self.filter, nearest, voice.hz[k], scale, offset);
            }
        }
        if p[64] > 0 {
            voice.res_mt = if voice.res_mt == f32::MAX {
                fc
            } else {
                fc + (voice.res_mt - fc) * d.res_glide
            };
            let moved = (fc - voice.res_mt).abs();
            resonance *= (moved * moved).min(1.0);
        }
        if resonance > 0.0 && self.state.selectors[1] == 8 {
            // Noise (resonance type 8): a partial is silent with the
            // chance `t / (t + 1)`, `t = 9999 (4 r)^3 / hold`, and the
            // rest go up by `sqrt(t + 1)` so the power stays; the draw is
            // held for the noise's hold (ippsRandUniform_Direct_32f,
            // ippsThreshold_LTVal, ippsThreshold_GTVal).
            let x = f64::from(resonance * 4.0);
            let t = (x * x * x * 9999.0 / d.noise_len) as f32;
            let top = t + 1.0;
            let keep = f64::from(top).sqrt() as f32;
            let seed = voice.noise_seed;
            let draw = &mut voice.scratch[..n];
            rand_uniform(draw, 0.0, top, &mut voice.noise_seed);
            for (level, v) in voice.levels[..n].iter_mut().zip(draw.iter()) {
                let v = if *v < t { 0.0 } else { *v };
                *level *= if v > 0.0 { keep } else { v };
            }
            voice.noise_time += d.noise_rate;
            if voice.noise_time < 1.0 {
                voice.noise_seed = seed;
            } else {
                voice.noise_time = frac(voice.noise_time);
            }
        } else if resonance > 0.0 {
            let scale = if p[63] > 0 {
                d.res_scale / voice.f0_hz
            } else {
                d.res_scale
            };
            let bias = d.res_bias - voice.log2_f0;
            let table = &self.resonance;
            for k in 0..n {
                let shape = if p[60] == 0 {
                    clamped(
                        table,
                        nearest,
                        voice.log2_hz[k],
                        scale,
                        table.centre - (fc + d.res_offset) * scale,
                    )
                } else {
                    clamped(
                        table,
                        nearest,
                        voice.hz[k],
                        scale,
                        table.centre - cut_hz * d.res_ratio * scale,
                    )
                };
                let a = ((voice.log2_hz[k] + bias) * resonance).max(0.0);
                voice.levels[k] *= (a * shape + 1.0).max(0.0);
            }
        }
        if sfx && sfx_at == 1 {
            self.harmonize(voice, lfo01);
        }

        // The phaser.
        if p[71] > 0 {
            let table = &self.phaser;
            let len = table.len() as f32;
            let kt = voice.log2_f0 * p[78] as f32 / 1000.0;
            let offset = ((-(p[75] as f32) / 1000.0 - lfo * d.phaser_offset_lfo) - voice.variance)
                * len
                - (self.phaser_phase as f32) * table.period;
            let mix = p[71] as f32 / 1000.0;
            let shift = self.state.selectors[2] == 9;
            let m = p[71] as f32 * 0.001f32;
            let bend = (f64::from(m * m * m) * LN_2 * 0.25) as f32;
            let inv_rate = (1.0 / d.rate) as f32;
            let octaves = ((d.phaser_width - 1.0) * 8.0 + lfo * d.phaser_width_lfo * 2.0) as f64;
            for k in 0..n {
                let shape = match p[72] {
                    0 => {
                        let scale = octaves.exp2() as f32 * 12.0 * table.period;
                        wrapped(table, nearest, voice.log2_hz[k], scale, offset - kt * scale)
                    }
                    1 => {
                        let scale = octaves.exp2() as f32 * 12.0 * 0.005 * table.period;
                        wrapped(table, nearest, voice.hz[k], scale, offset - kt * len)
                    }
                    _ => {
                        let width = d.phaser_width + lfo * d.phaser_width_lfo;
                        let scale = (2.0 - width) * table.period / voice.f0_hz;
                        wrapped(table, nearest, voice.hz[k], scale, offset - kt * len)
                    }
                };
                if shift {
                    // Type 9 moves the partial instead: by `2^(curve
                    // mix^3 / 4)` (`ippsExp_32f` of `curve * mix^3 ln2 /
                    // 4`, times the partial's Hz and 1/SR, into the
                    // bank), with the frequency weight over again.
                    let e = f64::from(shape * bend).exp() as f32;
                    let f = e * voice.hz[k] * inv_rate;
                    if k > 0 {
                        voice.bank.set_freq(k, f64::from(f));
                        voice.alias[k] = alias_weight((f64::from(f) * d.rate) as f32);
                    }
                } else {
                    voice.levels[k] *= shape * mix + (1.0 - mix);
                }
            }
        }
        if sfx && sfx_at == 2 {
            self.harmonize(voice, lfo01);
        }

        // The pluck (FUN_00784860).
        if voice.pluck_on {
            if voice.pluck_tick >= 0 {
                if voice.pluck_ready < n {
                    let mut time = d.pluck;
                    if p[32] > 0 {
                        time *= f64::from(voice.vel01 * voice.vel01);
                    }
                    let rate = d.pluck_rate / (time + 0.25);
                    for k in voice.pluck_ready..n {
                        let by = if p[33] > 0 {
                            f64::from(d.timbre[k].abs().max(1e-6)).recip()
                        } else {
                            d.ratio[k]
                        };
                        let factor = (by * rate).exp() as f32;
                        voice.pluck_factor[k] = factor;
                        voice.pluck_cum[k] = if voice.pluck_tick < 1 {
                            1.0
                        } else {
                            factor.powi(voice.pluck_tick as i32)
                        };
                    }
                    voice.pluck_ready = n;
                }
                for k in 0..n {
                    voice.levels[k] *= voice.pluck_cum[k];
                    voice.pluck_cum[k] *= voice.pluck_factor[k];
                }
            }
            voice.pluck_tick += 1;
        }
        if sfx && sfx_at == 3 {
            self.harmonize(voice, lfo01);
        }

        // Protection and the subs.
        for k in 0..8.min(n) {
            let l = voice.levels[k];
            voice.levels[k] = l + (d.timbre[k] - l) * self.tables.protect[k] * d.protect_gain;
        }
        for k in 0..4 {
            voice.levels[k] *= d.subs[k];
        }
        let pluck1 = if voice.pluck_on && voice.pluck_tick > 0 {
            voice.pluck_cum[1]
        } else {
            1.0
        };
        voice.levels[0] = d.subs[0] * resonance * pluck1;
        let osc_hz = f64::from(cut_hz) * f64::from(d.res_ratio);
        // FL sets the resonance partial, and starts it again, only when
        // its frequency moves (`FUN_00784a10`).
        let osc = osc_hz / d.rate;
        if voice.bank.freq(0) != osc {
            voice.bank.set_freq(0, osc);
        }
        voice.alias[0] = alias_weight(osc_hz as f32);

        // To the bank.
        for k in 0..n {
            voice.bank.set_target(k, voice.levels[k] * voice.alias[k]);
        }
        if voice.first {
            // Phases of the colour, moved on by the unison phase and the
            // spread of the first wheel.
            let spread = f64::from(d.phase_spread);
            let start = if spread >= 0.0 {
                let f0c = f64::from(voice.f0_hz) / d.rate;
                let r = frac(f0c * 277.0 * self.sample_count as f64) * spread;
                f64::from(voice.phase_offset) + r
            } else {
                f64::from(voice.phase_offset)
            };
            for k in 0..n {
                let mut phase = d.phase[k] + d.ratio[k] * start;
                if spread < 0.0 {
                    phase += f64::from(pseudo(k as u32 ^ (self.sample_count as u32))) * -spread;
                }
                voice.bank.set_phase(k, phase);
            }
            voice.bank.jump();
            voice.first = false;
        }
        voice.lfo_att = (voice.lfo_att + d.lfo_attack * tick).min(1.0);
        voice.vib_att = (voice.vib_att + d.vib_attack * tick).min(1.0);
    }

    /// Render one chunk of a voice into the stereo sum.
    fn render_voice(&self, voice: &mut Voice, left: &mut [f32], right: &mut [f32], trem: bool) {
        let d = &*self.d;
        let len = left.len();
        // The host's gains for the block, from the note's levels as the
        // block starts (`FUN_00784a10` asks `ComputeLRVol`, host slot 31):
        // a new voice starts at them, a sounding one ramps to them.
        let vol =
            d.main_volume * (voice.velocity_gain(self.on(37)) + voice.slide[1]) * d.unison_gain;
        let aim = pan_law(voice.pan + voice.slide[0] + voice.pan_offset, vol);
        if voice.first {
            voice.gains = aim;
        }
        voice.mono[..len].fill(0.0);
        let mut done = 0;
        while done < len {
            if voice.countdown == 0 {
                if self.on(87) {
                    voice.lfo_phase = self.global_lfo + d.lfo_speed * done as f64;
                }
                self.control(voice);
                voice.countdown = d.tick;
            }
            let run = voice.countdown.min(len - done);
            voice.bank.render_add(&mut voice.mono[done..done + run]);
            voice.countdown -= run;
            voice.lfo_phase += d.lfo_speed * run as f64;
            done += run;
        }
        self.envelope(voice, len);
        for (x, e) in voice.mono[..len].iter_mut().zip(&voice.env_buf[..len]) {
            *x *= *e;
        }
        // `FUN_00604960`: each side's gain glides to its aim by no more
        // than the host's cap a frame, in FL's lanes.
        let (start_l, step_l) = parts::glide(&mut voice.gains[0], aim[0], d.ramp_cap, len);
        let (start_r, step_r) = parts::glide(&mut voice.gains[1], aim[1], d.ramp_cap, len);
        let mut ramp_l = Ramp::new(start_l, step_l, len);
        let mut ramp_r = Ramp::new(start_r, step_r, len);
        // The tremolo on the voice (`FUN_00784a10`, tail): a new voice
        // fades it in over 0.1 s, `(1 - r) + r * trem`, r rising from
        // 1 / (frames + 1) a frame to 1.
        let fading = trem && voice.trem_fade >= 1;
        let fade_start = 1.0 - voice.trem_fade as f32 * d.tremolo_fade_step;
        if fading {
            voice.trem_fade -= len as i32;
        }
        let still = step_l == 0.0 && step_r == 0.0;
        if still && start_l == 0.0 && start_r == 0.0 {
            return self.end_voice(voice, len);
        }
        for i in 0..len {
            let (gl, gr) = if still {
                (start_l, start_r)
            } else {
                (ramp_l.next(i), ramp_r.next(i))
            };
            let mut sl = voice.mono[i] * gl;
            let mut sr = voice.mono[i] * gr;
            if trem {
                let (tl, tr) = (self.trem[2 * i], self.trem[2 * i + 1]);
                if fading {
                    let r = ((f64::from(fade_start) + f64::from(d.tremolo_fade_step) * i as f64)
                        as f32)
                        .min(1.0);
                    sl *= (1.0 - r) + r * tl;
                    sr *= (1.0 - r) + r * tr;
                } else {
                    sl *= tl;
                    sr *= tr;
                }
            }
            left[i] += sl;
            right[i] += sr;
        }
        self.end_voice(voice, len);
    }

    /// Whether a voice is over after its block.
    fn end_voice(&self, voice: &mut Voice, len: usize) {
        // A voice past its attack whose envelope is under -80 dB is over
        // (`FUN_00783ac0` from the voice renderer), but for the note that
        // answers for the notes: legato may hand it on yet.
        let last = voice.env_buf[len - 1];
        let lead = self.lead == Some(voice.id);
        if voice.state == 10 || (voice.state >= 3 && !lead && last.abs() < END_LEVEL) {
            voice.active = false;
        }
    }

    /// The amplitude envelope over a chunk (`FUN_00784a10`, second loop).
    fn envelope(&self, voice: &mut Voice, len: usize) {
        let d = &*self.d;
        let mut done = 0;
        while done < len {
            if voice.end <= voice.pos {
                if voice.end == -1 {
                    let release = if self.on(30) {
                        voice.release_value * 2.0
                    } else {
                        1.0
                    };
                    voice.declick.set(d.declick);
                    voice.env.set(d.release * f64::from(release));
                } else if voice.state < 9 {
                    voice.declick.set(d.rate * 0.08);
                    if d.decay >= 0.0 {
                        voice.env.set(d.decay);
                    } else {
                        voice.env.hold();
                    }
                    voice.state = voice.state.max(3);
                } else {
                    voice.state = 10;
                }
                voice.env.y = voice.last_ramp;
                voice.end = i64::MAX;
            }
            let run = ((voice.end - voice.pos).min((len - done) as i64)) as usize;
            let buf = &mut voice.env_buf[done..done + run];
            if voice.state < 3 {
                for (i, v) in buf.iter_mut().enumerate() {
                    *v = (((voice.pos + i as i64) as f64 * voice.slope) as f32).min(1.0);
                }
            } else {
                buf.fill(0.0);
                voice.env.run(buf);
            }
            voice.last_ramp = buf[run - 1];
            voice.declick.run(buf);
            voice.pos += run as i64;
            done += run;
        }
    }

    /// The stereo tremolo's gains for a block (`Gen_Render`, ahead of the
    /// voices), when there are voices for it: a tone of magnitude 1.25 at
    /// 2.5 Hz from the sample counter, `|cos| - 1` on the left and
    /// `|sin| - 1` on the right, times the depth as it glides to the wheel's by
    /// the host's cap (`FUN_00604a90`), plus 1. With no voices the depth
    /// jumps to the wheel's.
    fn tremolo(&mut self, len: usize, voices: usize) -> bool {
        let d = &*self.d;
        if d.tremolo == 0.0 && self.trem_depth == 0.0 {
            return false;
        }
        if voices < 1 {
            self.trem_depth = d.tremolo;
            return false;
        }
        let tau = TAU as f32;
        let mut phase =
            (frac(f64::from(self.sample_count as f32 * d.tremolo_rate)) * f64::from(tau)) as f32;
        if phase >= tau {
            phase -= tau;
        }
        let w = TAU * f64::from(d.tremolo_rate);
        let trem = &mut self.trem[..2 * len];
        for i in 0..len {
            let a = f64::from(phase) + w * i as f64;
            trem[2 * i] = (1.25 * a.cos()).abs() as f32 + -1.0;
            trem[2 * i + 1] = (1.25 * a.sin()).abs() as f32 + -1.0;
        }
        let (start, step) = parts::glide(&mut self.trem_depth, d.tremolo, d.ramp_cap, len);
        if step == 0.0 {
            if start == 0.0 {
                trem.fill(0.0);
            } else if start != 1.0 {
                for x in trem.iter_mut() {
                    *x *= start;
                }
            }
        } else {
            let mut ramp = Ramp::new(start, step, len);
            for i in 0..len {
                let g = ramp.next(i);
                trem[2 * i] *= g;
                trem[2 * i + 1] *= g;
            }
        }
        for x in trem.iter_mut() {
            *x += 1.0;
        }
        true
    }

    /// The chorus, delay, reverb and compression.
    fn effects(&mut self, voices: usize, left: &mut [f32], right: &mut [f32]) {
        let len = left.len();
        let d = &*self.d;
        let fx = &mut *self.fx;
        // The voices keep the classes awake: with none left, the
        // countdown runs out a block at a time.
        if voices < 1 {
            if fx.sleep > 0 {
                fx.sleep -= len as i32;
            }
        } else {
            fx.sleep = fx.hold;
        }
        let (wet_l, wet_r) = (&mut self.wet_left[..len], &mut self.wet_right[..len]);
        if fx.chorus_on {
            if fx.sleep < 1 {
                fx.chorus.skip(len);
            } else {
                for i in 0..len {
                    (wet_l[i], wet_r[i]) = fx.chorus.frame(left[i], right[i]);
                }
                if fx.chorus_dry != 1.0 {
                    for i in 0..len {
                        left[i] *= fx.chorus_dry;
                        right[i] *= fx.chorus_dry;
                    }
                }
                for i in 0..len {
                    left[i] += wet_l[i];
                    right[i] += wet_r[i];
                }
            }
        }
        if fx.delay_on {
            if fx.sleep > 0 {
                for i in 0..len {
                    wet_l[i] = left[i] * fx.delay_gain;
                    wet_r[i] = right[i] * fx.delay_gain;
                }
                fx.delay.add(0, wet_l, fx.delay_offsets[0]);
                fx.delay.add(1, wet_r, fx.delay_offsets[1]);
            }
            let cap = (44_100.0 / d.rate * 0.001) as f32;
            fx.delay.render(fx.kernel, wet_l, wet_r, cap);
            for i in 0..len {
                left[i] += wet_l[i];
                right[i] += wet_r[i];
            }
        }
        if fx.reverb_on {
            let mono = &mut self.mono_in[..len];
            for i in 0..len {
                mono[i] = left[i] + right[i];
            }
            fx.reverb.process(mono, wet_l, wet_r);
            for i in 0..len {
                left[i] += wet_l[i] * fx.reverb_wet;
                right[i] += wet_r[i] * fx.reverb_wet;
            }
        }
        if let Some(maximus) = &mut self.compression {
            maximus.process(left, right);
        }
    }
}

/// A gain FL's SSE kernels ramp across a block (`FUN_00611760`,
/// `FUN_00611550`): the even frames and the odd ones each in a lane of
/// their own that moves two steps a pair, eight frames to a round, and
/// the frames past the last round one step at a time from the even lane.
struct Ramp {
    even: f32,
    odd: f32,
    twice: f32,
    step: f32,
    rounds: usize,
}

impl Ramp {
    fn new(start: f32, step: f32, len: usize) -> Self {
        Ramp {
            even: start + 0.0,
            odd: start + step,
            twice: step + step,
            step,
            rounds: len / 8 * 8,
        }
    }

    /// The gain of `frame`, asked for in order from 0.
    fn next(&mut self, frame: usize) -> f32 {
        if frame < self.rounds {
            if frame & 1 == 0 {
                self.even
            } else {
                let odd = self.odd;
                self.even += self.twice;
                self.odd += self.twice;
                odd
            }
        } else {
            let even = self.even;
            self.even += self.step;
            even
        }
    }
}

impl Voice {
    fn velocity_gain(&self, to_volume: bool) -> f32 {
        if to_volume { self.velocity } else { 1.0 }
    }
}

/// A partial's weight by its frequency: nothing under 16 Hz or over 17
/// kHz, squared fades from 16 to 32 Hz and from 16 to 17 kHz
/// (`FUN_007453c0` and the loop after it in the voice).
/// A curve at `x * scale + offset`, the first or last point outside it
/// (`FUN_0077c540`): between the two points either side, or with
/// `nearest` the point below (`FUN_0077c470`).
fn clamped(table: &ShapeTable, nearest: bool, x: f32, scale: f32, offset: f32) -> f32 {
    if !nearest {
        return table.clamped(x, scale, offset);
    }
    let at = (x * scale + offset).max(0.0).min((table.len() - 1) as f32);
    table.get(at as usize)
}

/// A curve at `x * scale + offset` read round and round (`FUN_0077c760`):
/// between the two points either side, or with `nearest` the point toward
/// 0 (`FUN_0077c690`).
fn wrapped(table: &ShapeTable, nearest: bool, x: f32, scale: f32, offset: f32) -> f32 {
    if !nearest {
        return table.wrapped(x, scale, offset);
    }
    let i = ((x * scale + offset) as i32) & (table.len() as i32 - 1);
    table.get(i as usize)
}

fn alias_weight(hz: f32) -> f32 {
    if hz > 16_000.0 {
        if hz > 17_000.0 {
            0.0
        } else {
            let w = 1.0 - (hz - 16_000.0) / 1000.0;
            w * w
        }
    } else if hz < 32.0 {
        if hz < 16.0 {
            0.0
        } else {
            let w = 1.0 - (32.0 - hz) / 16.0;
            w * w
        }
    } else {
        1.0
    }
}

/// A number from 0 to 1 that follows from `x`.
fn pseudo(x: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    (f64::from(h) / f64::from(u32::MAX)) as f32
}

/// The event 213 bytes of an `.fst` preset, the plugin's state in it.
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

impl Generator for Harmless {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.state = State::parse(bytes)?;
        self.load();
        // `PortaBox` on forgets the last note (`FUN_00788030`).
        if self.on(43) {
            self.last = None;
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if f64::from(hz) != self.d.rate && hz > 0.0 {
            self.d.rate = f64::from(hz);
            self.load();
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 && bpm != self.d.tempo {
            self.d.tempo = bpm;
            self.derive();
        }
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        for n in 0..4 {
            let path = dir.join(COMPRESSION).join(format!("{}.fst", n + 1));
            self.compression_states[n] = std::fs::read(path).ok().and_then(|b| fst_state(&b));
        }
        self.build_compression();
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            // FL bends what sounds; here the slide note bends nothing.
            return;
        }
        self.start(note);
    }

    /// `Voice_Release` (`FUN_00783910`): a held note goes; the note that
    /// sounds hands its voices to the newest held note if there is one,
    /// with its release velocity's glide time, and lets them go if not.
    fn note_off(&mut self, voice: u32) {
        if let Some(i) = self.held[..self.held_len]
            .iter()
            .position(|h| h.id == voice)
        {
            self.held.copy_within(i + 1..self.held_len, i);
            self.held_len -= 1;
            return;
        }
        if self.lead == Some(voice) {
            self.lead = None;
            let newest = (0..self.held_len).max_by_key(|&i| self.held[i].age);
            let gone = self
                .voices
                .iter()
                .find(|v| v.active && v.id == voice)
                .map(|v| v.note(self.lead_age));
            if let (Some(i), Some(mut gone)) = (newest, gone) {
                let mut next = self.held[i];
                self.held.copy_within(i + 1..self.held_len, i);
                self.held_len -= 1;
                if self.on(46) {
                    let s = (1.0 - f64::from(gone.release.min(1.0)) * 0.85) as f32;
                    gone.porta_scale = s * s;
                } else {
                    gone.porta_scale = 1.0;
                }
                next.porta_scale = gone.porta_scale;
                self.hand_on(&gone, next);
                self.lead_age = next.age;
                return;
            }
        }
        let d = &*self.d;
        for v in self.voices.iter_mut().filter(|v| v.active && v.id == voice) {
            Harmless::release(v, d);
        }
    }

    fn all_notes_off(&mut self) {
        self.held_len = 0;
        self.lead = None;
        let d = &*self.d;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            Harmless::release(v, d);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let mut done = 0;
        let total = left.len().min(right.len());
        while done < total {
            let len = (total - done).min(CHUNK);
            let mut l = self.left;
            let mut r = self.right;
            l[..len].fill(0.0);
            r[..len].fill(0.0);
            let mut voices = std::mem::take(&mut self.voices);
            // FL's voice list: what sounds and the held notes.
            let sounding = voices.iter().filter(|v| v.active).count() + self.held_len;
            let trem = self.tremolo(len, sounding);
            for voice in voices.iter_mut().filter(|v| v.active) {
                self.render_voice(voice, &mut l[..len], &mut r[..len], trem);
            }
            self.voices = voices;
            self.effects(sounding, &mut l[..len], &mut r[..len]);
            for i in 0..len {
                left[done + i] += l[i];
                right[done + i] += r[i];
            }
            let d = &*self.d;
            self.global_lfo += d.lfo_speed * len as f64;
            self.vibrato_phase =
                frac(f64::from(self.vibrato_phase + d.vib_rate * len as f32)) as f32;
            self.phaser_phase = frac(self.phaser_phase + d.phaser_speed * len as f64);
            self.sample_count += len as u64;
            done += len;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.state.params.get(index).map_or(0.0, |v| *v as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index >= PARAMS {
            return;
        }
        let value = knob_value(value, RANGES[index]);
        if self.state.params[index] == value {
            return;
        }
        self.state.params[index] = value;
        self.derive();
        if index == 43 && value > 0 {
            self.last = None;
        }
        if index == 98
            && let Some(maximus) = &mut self.compression
        {
            maximus.set_param(5, value as f32);
        }
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.active = false;
        }
        self.held_len = 0;
        self.lead = None;
        self.last = None;
        self.load();
    }

    fn tail(&self) -> usize {
        let d = &*self.d;
        // The release falls 40 dB in its time and a voice ends at -80 dB.
        let mut tail = d.release * 2.0 + d.declick;
        let fx = &*self.fx;
        if fx.chorus_on {
            tail += f64::from(fx.hold);
        }
        if fx.delay_on {
            let time = fx.delay.delay() as f64;
            let feedback = f64::from(fx.delay.feedback());
            let repeats = if feedback > 0.0 {
                (0.001f64.ln() / feedback.ln()).min(50.0)
            } else {
                0.0
            };
            tail += time * (repeats + 1.0);
        }
        if fx.reverb_on {
            tail += f64::from(fx.reverb.settings().decay) * 1.5;
        }
        tail as usize + d.tick
    }

    fn latency(&self) -> usize {
        self.compression.as_ref().map_or(0, |m| m.latency())
    }

    fn fidelity(&self) -> Fidelity {
        if self.missing_compression {
            Fidelity::Approximate("Harmless without its compression preset")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, partial, projects};

    fn note(key: i16) -> NoteOn {
        NoteOn {
            voice: 1,
            key,
            velocity: 1.0,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            slide: false,
            color: 0,
            porta: false,
        }
    }

    fn render(plugin: &mut Harmless, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            plugin.process(a, b);
        }
        (l, r)
    }

    /// A v12 state of these parameters, no image.
    fn state_of(params: &[(usize, i32)], selectors: [i32; 7]) -> Vec<u8> {
        let mut p = DEFAULTS;
        for (i, v) in params {
            p[*i] = *v;
        }
        let mut bytes = 12i32.to_le_bytes().to_vec();
        for v in p.iter().chain(selectors.iter()) {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend(0i32.to_le_bytes());
        bytes.push(1);
        bytes
    }

    #[test]
    fn the_level_curves_meet_at_their_ends() {
        assert_eq!(level(0.0), 0.0);
        assert_eq!(level(1.0), 1.0);
        assert!((level(0.5) - (11f64.sqrt() - 1.0) / 10.0).abs() < 1e-12);
        assert_eq!(level3(1.0), 1.0);
        assert!((level3(2.0) - 4.0).abs() < 1e-12);
        // The main volume at its default is 3/4.
        let plugin = Harmless::default();
        assert!((plugin.d.main_volume - 0.75).abs() < 1e-6);
    }

    #[test]
    fn below_quality_2_a_curve_is_read_at_the_point_below() {
        let table = ShapeTable::new(8, 0.0, 1.0, 0.0, |i| i as f64 * 10.0);
        // Between points 2 and 3, a quarter of the way.
        assert_eq!(clamped(&table, false, 2.25, 1.0, 0.0), 22.5);
        assert_eq!(clamped(&table, true, 2.25, 1.0, 0.0), 20.0);
        assert_eq!(clamped(&table, true, 2.99, 1.0, 0.0), 20.0);
        // The ends outside.
        assert_eq!(clamped(&table, true, -3.0, 1.0, 0.0), 0.0);
        assert_eq!(clamped(&table, true, 30.0, 1.0, 0.0), 70.0);
        // Round and round: 10.5 is point 2, and -1.5 truncates to -1,
        // which is point 7.
        assert_eq!(wrapped(&table, true, 10.5, 1.0, 0.0), 20.0);
        assert_eq!(wrapped(&table, true, -1.5, 1.0, 0.0), 70.0);
        assert_eq!(wrapped(&table, false, 10.5, 1.0, 0.0), 25.0);
    }

    #[test]
    fn the_host_gains_ramp_in_lanes() {
        let gains: Vec<f32> = {
            let mut ramp = Ramp::new(1.0, 0.1, 11);
            (0..11).map(|i| ramp.next(i)).collect()
        };
        // Eight frames in two lanes, each moving two steps a pair.
        let (mut even, mut odd) = (1.0f32, 1.0 + 0.1f32);
        for pair in 0..4 {
            assert_eq!(gains[2 * pair], even);
            assert_eq!(gains[2 * pair + 1], odd);
            even += 0.1 + 0.1;
            odd += 0.1 + 0.1;
        }
        // The rest from the even lane, a step at a time.
        for gain in &gains[8..] {
            assert_eq!(*gain, even);
            even += 0.1;
        }
    }

    #[test]
    fn a_voice_starts_at_its_host_gains_and_then_glides_to_them() {
        let mut plugin = Harmless::default();
        plugin.set_sample_rate(44_100.0);
        let mut n = note(60);
        n.pan = -0.5;
        plugin.note_on(n);
        render(&mut plugin, 256);
        let vol = plugin.d.main_volume * plugin.d.unison_gain;
        let voice = plugin.voices.iter().find(|v| v.active).unwrap();
        assert_eq!(voice.gains, pan_law(-0.5, vol));
        // The main volume up to full: the gains move no more than
        // 0.001 * 44100 / rate a frame.
        plugin.set_param(36, 2000.0);
        let before = plugin.voices.iter().find(|v| v.active).unwrap().gains;
        render(&mut plugin, 256);
        let after = plugin.voices.iter().find(|v| v.active).unwrap().gains;
        for side in 0..2 {
            let moved = after[side] - before[side];
            assert!((moved - 0.001 * 256.0).abs() < 1e-5, "{moved}");
        }
    }

    #[test]
    fn the_timbre_colours_are_falling_series() {
        // The default colour is a saw: 1 / (n + 1).
        for n in 0..10 {
            let (level, phase) = timbre_point(0, f64::from(n), None);
            assert!((level - 1.0 / (n as f32 + 1.0)).abs() < 1e-7);
            assert_eq!(phase, 0.0);
        }
        // Colour 3 turns the even harmonics a quarter.
        assert_eq!(timbre_point(3, 1.0, None).1, 0.25);
        assert_eq!(timbre_point(3, 2.0, None).1, 0.0);
        // Colour 5 makes the even harmonics fall as 1 / n^2.
        let (even, _) = timbre_point(5, 3.0, None);
        assert!((even - (1.1 / 12.0) as f32).abs() < 1e-7);
        // A drawn one reads its own levels and phases.
        let mut image = vec![0.0; 2 * IMAGE];
        image[7] = 0.5;
        image[7 + IMAGE] = 0.75;
        assert_eq!(timbre_point(11, 7.0, Some(&image)), (0.5, 0.75));
    }

    #[test]
    fn the_timbre_bands_add_up_to_one() {
        let tables = Tables::new();
        for k in 0..PARTIALS {
            let sum: f32 = tables.bands.iter().map(|band| band[k]).sum();
            assert!((sum - 1.0).abs() < 1e-5, "partial {k}: {sum}");
        }
        // Harmonic 4 (index 7) is two octaves up: all of band 2.
        assert_eq!(tables.bands[2][7], 1.0);
        // The protection keeps the fundamental and the subs whole and lets
        // the third harmonic go.
        assert_eq!(tables.protect[4], 1.0);
        assert_eq!(tables.protect[1], 1.0);
        assert!(tables.protect[6] < 1e-3);
    }

    #[test]
    fn the_lowpass_curve_falls_72_db_over_its_length() {
        let lp = shape(0);
        assert_eq!(lp.len(), 2048);
        assert!((lp.get(0) - 1.0).abs() < 1e-6);
        let half = f64::from(lp.get(1023));
        assert!((20.0 * half.log10() + 36.0).abs() < 0.1, "{half}");
        assert_eq!(lp.get(2047), 0.0);
        // The highpass is the lowpass turned round, centred at its end.
        let hp = shape(3);
        assert_eq!(hp.centre, 2047.0);
        assert!((hp.get(2047) - 1.0).abs() < 1e-6);
        // The resonance curves are normalised to their RMS.
        let res = shape(100);
        let rms = (res
            .points()
            .iter()
            .map(|p| f64::from(*p).powi(2))
            .sum::<f64>()
            / 1024.0)
            .sqrt();
        assert!((rms - 7.0).abs() < 1e-3);
    }

    #[test]
    fn a_tension_curve_bends_and_comes_back() {
        for value in [-1000, -300, 0, 250, 1000] {
            let t = Tension::new(value);
            assert!(t.at(0.0).abs() < 1e-5, "{value}");
            assert!((t.at(1.0) - 1.0).abs() < 1e-5, "{value}");
            for x in [0.1f32, 0.5, 0.9] {
                assert!((t.inverse(t.at(x)) - x).abs() < 1e-3, "{value} {x}");
            }
        }
        assert!(Tension::new(500).at(0.5) > 0.5);
        assert!(Tension::new(-500).at(0.5) < 0.5);
    }

    #[test]
    fn a_state_round_trips_its_parameters_and_image() {
        let bytes = state_of(&[(54, 1000), (65, 3)], [4, 2, 1, 2, 0, 1, 0]);
        let state = State::parse(&bytes).unwrap();
        assert_eq!(state.params[54], 1000);
        assert_eq!(state.params[65], 3);
        assert_eq!(state.selectors, [4, 2, 1, 2, 0, 1, 0]);
        assert!(state.image.is_none());
        // A v8 state has 99 parameters, and its phases start 4 later.
        let mut v9 = 8i32.to_le_bytes().to_vec();
        for v in DEFAULTS.iter().take(99).chain([0; 7].iter()) {
            v9.extend(v.to_le_bytes());
        }
        v9.extend(524i32.to_le_bytes());
        let mut image = vec![0.0f32; 2 * IMAGE];
        image[IMAGE + 4] = 0.25;
        for f in &image {
            v9.extend(f.to_le_bytes());
        }
        let state = State::parse(&v9).unwrap();
        assert_eq!(state.params[99], 0);
        assert_eq!(state.image.as_ref().unwrap()[IMAGE], 0.25);
        // One byte short is turned down.
        assert!(State::parse(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn a_note_is_a_saw_at_its_pitch() {
        // No mask, filter wide open: the colour itself.
        let mut plugin = Harmless::default();
        let bytes = state_of(&[(23, 0), (54, 8192), (1, 0)], [0; 7]);
        plugin.set_sample_rate(48_000.0);
        plugin.set_state(&bytes).unwrap();
        plugin.note_on(note(69));
        // Key 60 is C5, 523 Hz, and the main pitch's -1200 cents put A at
        // 440 Hz.
        let (l, _) = render(&mut plugin, 48_000);
        let tail = &l[24_000..24_000 + 4800];
        let turns = 440.0 / 48_000.0;
        let h1 = partial(tail, turns);
        let h2 = partial(tail, 2.0 * turns);
        let h3 = partial(tail, 3.0 * turns);
        // The fundamental at 1, times the main volume's 3/4 and the
        // centre of the pan law.
        let want = 0.75 * std::f64::consts::FRAC_1_SQRT_2;
        assert!((h1 / want - 1.0).abs() < 0.02, "{h1}");
        assert!((h2 / h1 - 0.5).abs() < 0.02, "{}", h2 / h1);
        assert!((h3 / h1 - 1.0 / 3.0).abs() < 0.02, "{}", h3 / h1);
        assert!(l.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn a_low_cutoff_takes_the_top_off() {
        let mut open = Harmless::default();
        open.set_state(&state_of(&[(23, 0), (54, 8192)], [0; 7]))
            .unwrap();
        let mut shut = Harmless::default();
        shut.set_state(&state_of(&[(23, 0), (54, 2000)], [0; 7]))
            .unwrap();
        let turns = 440.0 / 44_100.0;
        let mut ratio = Vec::new();
        for plugin in [&mut open, &mut shut] {
            plugin.note_on(note(69));
            let (l, _) = render(plugin, 22_050);
            let tail = &l[11_025..11_025 + 4410];
            ratio.push(partial(tail, 8.0 * turns) / partial(tail, turns));
        }
        assert!(ratio[1] < ratio[0] * 0.1, "{ratio:?}");
    }

    #[test]
    fn a_note_dies_away_after_its_release() {
        let mut plugin = Harmless::default();
        plugin.note_on(note(60));
        render(&mut plugin, 4410);
        plugin.note_off(1);
        let frames = plugin.tail();
        let (l, _) = render(&mut plugin, frames + 4410);
        let end = &l[l.len() - 1000..];
        assert!(end.iter().all(|v| v.abs() < 1e-4));
        assert!(plugin.voices.iter().all(|v| !v.active));
    }

    fn with_state(params: &[(usize, i32)]) -> Harmless {
        let mut plugin = Harmless::default();
        plugin.set_sample_rate(44_100.0);
        plugin
            .set_state(&state_of(params, DEFAULT_SELECTORS))
            .unwrap();
        plugin
    }

    fn keyed(id: u32, key: i16) -> NoteOn {
        NoteOn {
            voice: id,
            ..note(key)
        }
    }

    fn sounding(plugin: &Harmless) -> Vec<&Voice> {
        plugin.voices.iter().filter(|v| v.active).collect()
    }

    #[test]
    fn the_tremolo_swings_each_side_from_nothing_to_a_quarter_over() {
        let mut plugin = with_state(&[(25, 1000)]);
        // No voices: the depth goes straight to the wheel's.
        assert!(!plugin.tremolo(256, 0));
        assert_eq!(plugin.trem_depth, 1.0);
        // A turn of the 2.5 Hz tone: 1.25 |cos| on the left, 1.25 |sin|
        // on the right.
        assert!(plugin.tremolo(CHUNK, 1));
        assert_eq!(plugin.trem[0], 1.25);
        assert!(plugin.trem[1].abs() < 1e-6);
        let w = TAU * 2.5 / 44_100.0;
        for i in 0..CHUNK {
            let a = w * i as f64;
            assert!((f64::from(plugin.trem[2 * i]) - 1.25 * a.cos().abs()).abs() < 1e-6);
            assert!((f64::from(plugin.trem[2 * i + 1]) - 1.25 * a.sin().abs()).abs() < 1e-6);
        }
    }

    #[test]
    fn the_tremolo_depth_glides_while_voices_sound() {
        let mut plugin = with_state(&[(25, 500)]);
        plugin.trem_depth = 0.0;
        assert!(plugin.tremolo(256, 1));
        // At most 0.001 a frame at 44.1 kHz.
        assert!((plugin.trem_depth - 0.256).abs() < 1e-5);
        // At sample 0 the left starts at 1 + depth (1.25 - 1) = 1, as the
        // depth starts at 0.
        assert_eq!(plugin.trem[0], 1.0);
    }

    #[test]
    fn a_new_voice_fades_the_tremolo_in_over_a_tenth_of_a_second() {
        let mut plugin = with_state(&[(25, 1000)]);
        assert_eq!(plugin.d.tremolo_fade, 4410);
        assert_eq!(plugin.d.tremolo_fade_step, (1.0f64 / 4411.0) as f32);
        plugin.note_on(note(60));
        render(&mut plugin, 512);
        let voice = sounding(&plugin)[0];
        assert_eq!(voice.trem_fade, 4410 - 512);
        render(&mut plugin, 4096);
        assert!(sounding(&plugin)[0].trem_fade < 1);
    }

    fn with_selectors(params: &[(usize, i32)], selectors: [i32; 7]) -> Harmless {
        let mut plugin = Harmless::default();
        plugin.set_sample_rate(44_100.0);
        plugin.set_state(&state_of(params, selectors)).unwrap();
        plugin
    }

    #[test]
    fn the_noise_resonance_silences_partials_and_holds_its_draw() {
        let mut selectors = DEFAULT_SELECTORS;
        selectors[1] = 8;
        let mut plugin = with_selectors(&[(59, 1000), (61, 0)], selectors);
        // The middle of the wheel holds (201^0.5 - 1) 2.49 + 2.0 ms.
        let hold = (201f64.sqrt() - 1.0) * 2.490_022_675_736_961_2 + 1.995_464_852_607_709_7;
        assert!((plugin.d.noise_len - hold).abs() < 1e-5);
        assert!((plugin.d.noise_rate * hold * 44.1 / 88.0 - 1.0).abs() < 1e-6);
        plugin.note_on(note(60));
        render(&mut plugin, 1);
        let voice = sounding(&plugin)[0];
        let seed = voice.noise_seed;
        // At full resonance t = 9999 / hold: few partials are left, and
        // those at sqrt(t + 1) times their level.
        let n = voice.count;
        let silent = voice.levels[4..n].iter().filter(|l| **l == 0.0).count();
        assert!(silent * 10 > (n - 4) * 9, "{silent} of {}", n - 4);
        // The draw holds for the hold's ticks, then moves on.
        let ticks = (1.0 / plugin.d.noise_rate) as usize;
        render(&mut plugin, 88 * (ticks - 1));
        assert_eq!(sounding(&plugin)[0].noise_seed, seed);
        render(&mut plugin, 88 * 2);
        assert_ne!(sounding(&plugin)[0].noise_seed, seed);
    }

    #[test]
    fn the_frequency_shift_phaser_bends_the_partials_a_quarter_octave_rms() {
        let mut selectors = DEFAULT_SELECTORS;
        selectors[2] = 9;
        let mut plugin = with_selectors(&[(71, 1000)], selectors);
        plugin.note_on(note(60));
        render(&mut plugin, 1);
        let voice = sounding(&plugin)[0];
        let mut bent = 0;
        for k in 4..voice.count {
            let plain = f64::from(voice.hz[k]) / 44_100.0;
            let octaves = (voice.bank.freq(k) / plain).log2();
            // The curve is a sine of RMS 1: at most sqrt 2 / 4 octave.
            assert!(octaves.abs() <= 2f64.sqrt() / 4.0 + 1e-4, "{k}: {octaves}");
            if octaves.abs() > 1e-3 {
                bent += 1;
            }
        }
        assert!(bent > 0);
    }

    #[test]
    fn portamento_falls_to_a_hundredth_in_its_time() {
        let mut plugin = with_state(&[(43, 1), (45, 400)]);
        let ticks = plugin.d.porta;
        // 44100 (41^0.4 - 1) / (88 * 26.67) ticks.
        let want = (44_100.0 * (41f64.powf(0.4) - 1.0) / (88.0 * 80.0 / 3.0)) as f32;
        assert!((ticks - want).abs() < 1e-3, "{ticks}");
        plugin.note_on(keyed(1, 60));
        render(&mut plugin, 512);
        plugin.note_on(keyed(2, 67));
        let voice = sounding(&plugin).into_iter().find(|v| v.id == 2).unwrap();
        assert_eq!(voice.slide[2], -700.0);
        let coef = voice.slide_coef;
        assert!((f64::from(coef).powf(f64::from(ticks)) - 0.01).abs() < 1e-6);
        // A tick moves it on by the factor, before the pitch is read.
        render(&mut plugin, 1);
        let voice = sounding(&plugin).into_iter().find(|v| v.id == 2).unwrap();
        assert_eq!(voice.slide[2], -700.0 * coef);
    }

    #[test]
    fn portamento_glides_an_octave_at_most_and_a_bit() {
        for (key, want) in [(73, -100.0), (84, -1200.0), (48, 1200.0), (61, -100.0)] {
            let mut plugin = with_state(&[(43, 1), (45, 400)]);
            plugin.note_on(keyed(1, 60));
            plugin.note_on(keyed(2, key));
            let voice = sounding(&plugin).into_iter().find(|v| v.id == 2).unwrap();
            assert_eq!(voice.slide[2], want, "key {key}");
        }
    }

    #[test]
    fn a_negative_glide_time_goes_in_a_straight_line() {
        let mut plugin = with_state(&[(43, 1), (45, -400)]);
        let n = 1 - plugin.d.porta.round_ties_even() as i32;
        plugin.note_on(keyed(1, 60));
        plugin.note_on(keyed(2, 62));
        let voice = sounding(&plugin).into_iter().find(|v| v.id == 2).unwrap();
        assert_eq!(voice.slide_ticks, n);
        render(&mut plugin, 88 * n as usize);
        let voice = sounding(&plugin).into_iter().find(|v| v.id == 2).unwrap();
        assert_eq!(voice.slide_ticks, 0);
        assert!(voice.slide[2].abs() < 1e-3, "{}", voice.slide[2]);
    }

    #[test]
    fn legato_hands_the_voice_on_and_back() {
        let mut plugin = with_state(&[(44, 1), (45, 400), (65, 3)]);
        plugin.note_on(keyed(1, 60));
        render(&mut plugin, 1024);
        // A second note takes the three voices over, gliding from the first.
        plugin.note_on(keyed(2, 67));
        let voices = sounding(&plugin);
        assert_eq!(voices.len(), 3);
        assert!(voices.iter().all(|v| v.id == 2 && v.slide[2] == -700.0));
        render(&mut plugin, 1024);
        // Its release gives them back to the first, still held.
        plugin.note_off(2);
        let voices = sounding(&plugin);
        assert_eq!(voices.len(), 3);
        assert!(voices.iter().all(|v| v.id == 1 && v.state < 6));
        // From where the glide had got to, so less than the fifth.
        assert!(
            (300.0..700.0).contains(&voices[0].slide[2]),
            "{}",
            voices[0].slide[2]
        );
        // Which then lets go.
        plugin.note_off(1);
        assert!(sounding(&plugin).iter().all(|v| v.state == 6));
        // A note after the release starts its own voices.
        plugin.note_on(keyed(3, 64));
        assert_eq!(sounding(&plugin).iter().filter(|v| v.id == 3).count(), 3);
    }

    #[test]
    fn a_held_note_let_go_under_legato_is_forgotten() {
        let mut plugin = with_state(&[(44, 1)]);
        plugin.note_on(keyed(1, 60));
        plugin.note_on(keyed(2, 64));
        plugin.note_off(1);
        assert_eq!(plugin.held_len, 0);
        plugin.note_off(2);
        assert!(sounding(&plugin).iter().all(|v| v.id == 2 && v.state == 6));
    }

    /// The left side of a note of 0.1 s with these parameters, and without
    /// the effect `off` turns off.
    fn with_and_without(
        params: &[(usize, i32)],
        off: usize,
        frames: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut sides = [Vec::new(), Vec::new()];
        for (side, on) in sides.iter_mut().zip([true, false]) {
            let mut moved = params.to_vec();
            if !on {
                moved.push((off, 0));
            }
            let mut plugin = Harmless::default();
            plugin
                .set_state(&state_of(&moved, DEFAULT_SELECTORS))
                .unwrap();
            plugin.note_on(note(60));
            let (mut l, _) = render(&mut plugin, 4410);
            plugin.note_off(1);
            l.extend(render(&mut plugin, frames - 4410).0);
            *side = l;
        }
        let [with, without] = sides;
        (with, without)
    }

    #[test]
    fn the_delay_echoes_in_192nds_of_a_beat() {
        // 96 steps at 120 BPM: half a beat, 11025 samples.
        let params = [(92, 1), (93, 1000), (94, 0), (95, 96)];
        let (with, without) = with_and_without(&params, 92, 20_000);
        let heard = |side: &[f32]| (0..side.len()).find(|n| side[*n].abs() > 1e-7);
        let echo: Vec<f32> = with.iter().zip(&without).map(|(a, b)| a - b).collect();
        assert_eq!(heard(&echo), heard(&without).map(|n| n + 11_025));
        // At an input of 1000 and no feedback the echo is the note itself.
        for n in 11_025..11_100 {
            assert!((with[n] - without[n] - without[n - 11_025]).abs() < 1e-5);
        }
    }

    #[test]
    fn the_chorus_mixes_on_a_5_power_law() {
        let mut plugin = Harmless::default();
        plugin
            .set_state(&state_of(&[(90, 1), (91, 500)], DEFAULT_SELECTORS))
            .unwrap();
        // Half way the dry is 1 - (sqrt 5 - 1) / 4, and so is the wet.
        let law = 1.0 - (5f32.sqrt() - 1.0) / 4.0;
        assert!((plugin.fx.chorus_dry - law).abs() < 1e-6);
        assert!(plugin.fx.chorus_on && plugin.fx.chorus.length() > 8);
        let (with, without) = with_and_without(&[(90, 1), (91, 500)], 90, 8000);
        assert!(with.iter().all(|v| v.is_finite()) && with != without);
    }

    #[test]
    fn the_reverb_rings_on_after_the_voices_have_gone() {
        let params = [(96, 4), (97, 1000), (28, 0)];
        let (with, without) = with_and_without(&params, 96, 3 * 44_100);
        assert!(with.iter().all(|v| v.is_finite()));
        // The note is short; a second on, only the reverb sounds.
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        let late = 2 * 44_100..2 * 44_100 + 4410;
        assert!(rms(&without[late.clone()]) < 1e-6);
        assert!(rms(&with[late]) > 1e-4);
    }

    #[test]
    fn every_state_fl_ships_parses() {
        let Some(root) = corpus() else {
            return;
        };
        let mut states = projects(&root);
        states.retain(|s| s.plugin.eq_ignore_ascii_case(NAME));
        let presets = every_preset(&root, NAME);
        assert!(states.len() >= 160, "{}", states.len());
        assert!(presets.len() >= 400, "{}", presets.len());
        for saved in states.iter().chain(&presets) {
            let parsed = State::parse(&saved.state);
            assert!(parsed.is_ok(), "{} {:?}", saved.file, parsed.err());
        }
    }

    #[test]
    fn every_project_state_plays_a_sane_note() {
        let Some(root) = corpus() else {
            return;
        };
        let generators = root.parent().unwrap().join("Plugins/Fruity/Generators");
        let mut states = projects(&root);
        states.retain(|s| s.plugin.eq_ignore_ascii_case(NAME));
        for saved in states {
            let mut plugin = Harmless::default();
            plugin.set_resource_dir(&generators);
            plugin.set_state(&saved.state).unwrap();
            plugin.note_on(note(60));
            let (l, r) = render(&mut plugin, 22_050);
            let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{}", saved.file);
            assert!(peak < 8.0, "{} peak {peak}", saved.file);
        }
    }
}
