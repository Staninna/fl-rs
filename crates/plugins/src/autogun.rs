//! Autogun, FL's "metallic timbre generator", as `docs/plugins/autogun.md`
//! reads it out of `Autogun_x64.dll`.
//!
//! The DLL wraps no other plugin and ships no patches: every one of its
//! presets is the state itself, 49 wheels and a few switches. The sound is
//! made from two numbers, the timbre seeds, which FL's random generator
//! turns into the level of up to 32767 partials of a very low fundamental
//! (about half a hertz at C5). Pre-decay and fullness shape those levels,
//! an impulse curve colours them, and each partial then dies away over its
//! own time on a decay curve. An inverse FFT makes a wavetable of them
//! every 50 ms frame, and a voice crossfades from one table to the next,
//! read with FL's 6-point interpolator once for every unison copy. After
//! the voices come a three-band EQ, FL's chorus, delay and reverb classes
//! and "magic dust", a Soundgoodizer: the Maximus state in
//! `Autogun/Data/SG.dat` with its mix on the wheel.
//!
//! The timbre engine is in `autogun_engine.rs`. What this file adds is the
//! state, the parameters, the voice (articulators, levels, pan, declick,
//! the 1 to 3 stage filter) and the post chain.
//!
//! Ogun (`ogun.rs`, `docs/plugins/ogun.md`) plays on this file too: its
//! DLL is the same code less the random presets, as [`Kind`] says.
//!
//! Approximated, and said in [`Generator::fidelity`]: nothing is null
//! tested; IPP's `exp` and its CPU-specific random generators are taken to
//! give what their generic paths give; a timbre seed 2 of 0, which draws a
//! new seed for every note from a counter FL keeps per process, counts from
//! 0 here; the qualities above the default keep its interpolator and leave
//! out the oversampled output filter; portamento and
//! the velocity-to-envelope mode 2 are left out.

// The voice and effect kernels index their channels and stages on purpose,
// as FL's do.
#![allow(clippy::needless_range_loop)]

use crate::autogun_engine::{self as eng, CHUNK, Engine, Setup, Timbre, Work};
use crate::fl_fx::{
    Delay32, DelayKernel, FlangerSettings, FruityReverb, ReverbSettings, StereoFlanger,
};
use crate::harmor_art::{EnvRun, Envelope};
use crate::maximus::Maximus;
use crate::menvelope::{Class, Lfo, LfoRun, MEnvelope, Reader};
use crate::{Effect, Fidelity, Generator, NoteOn, StateError, knob_value};
use dsp::curve::Curve;
use std::f64::consts::{LN_2, PI, TAU};
use std::path::Path;

/// The plugin's event 201 name.
pub const NAME: &str = "Autogun";

/// The Soundgoodizer state "magic dust" loads, under the generators
/// directory.
pub const SG_STATE: &str = "Autogun/Data/SG.dat";

/// Automatable parameters: the REC indexes 0 to 48 (the DFM tags).
pub const PARAMS: usize = 49;

/// How many notes sound at once. FL leaves this to the host; a new note
/// past it takes the oldest voice.
pub const VOICES: usize = 16;

/// Each parameter's range, from the editor's wheels.
pub const RANGES: [(i32, i32); PARAMS] = [
    (0, 256),
    (-24, 24),
    (-50, 50),
    (0, 512),
    (0, 512),
    (0, 1000),
    (0, 10_000),
    (0, 10_000),
    (-1000, 1000),
    (0, 1000),
    (0, 9999),
    (0, 9999),
    (1, 9),
    (0, 128),
    (16, 128),
    (0, 256),
    (0, 128),
    (0, 256),
    (0, 256),
    (0, 9),
    (0, 1024),
    (0, 1024),
    (0, 128),
    (0, 128),
    (-128, 128),
    (-128, 128),
    (0, 127),
    (0, 384),
    (-512, 512),
    (-128, 128),
    (0, 2),
    (19, 3000),
    (5, 221),
    (0, 384),
    (1, 100),
    (0, 100),
    (1, 200),
    (5, 221),
    (0, 4),
    (-128, 128),
    (-1800, 1800),
    (-1800, 1800),
    (-1800, 1800),
    (0, 65_536),
    (0, 65_536),
    (0, 65_536),
    (0, 65_536),
    (0, 65_536),
    (0, 65_536),
];

/// What the constructor leaves the parameters at: the editor's wheel
/// values, and the master level at 256.
pub const DEFAULTS: [i32; PARAMS] = [
    256, 0, 0, 256, 256, 100, 4000, 2000, -500, 300, 1, 0, 1, 128, 128, 128, 64, 128, 128, 0, 512,
    768, 8, 128, -32, 64, 64, 144, 0, 64, 0, 75, 40, 0, 40, 100, 15, 40, 2, 25, 0, 0, 0, 8950,
    33145, 49305, 17500, 17500, 17500,
];

/// Articulators: volume, filter frequency, filter resonance, pitch and
/// unison pitch run per tick; unison phase, pre-decay, decay time, release
/// time, fullness, morphing, rotation and EQ shifting are read once.
const ARTICULATORS: usize = 13;
const RUN: usize = 5;
/// Which articulators are bipolar: they add, about 0.5 (`DAT_0087edc8`).
const BIPOLAR: u16 = 0x1a0e;
/// The four timbre curves: forced harmonics, impulse, decay and release
/// EQ.
const SHAPES: usize = 4;
/// Which curves a comb fills with 1 and not 0 (`DAT_00886d58`, from
/// curve 8 of the editor's list, so bit `13 + s - 8` for curve `s`).
const COMB_ONES: u16 = 0x0180;
/// A map baked for the mod X and Y wheels has this many steps.
const MAP_STEPS: usize = 512;

/// The newest state version, 19 (`0x00130003`). Every older one back to 0
/// is read as `SaveRestoreState` reads it.
const NEWEST: i32 = 19;
/// The block at plugin+0x12c: 49 parameters and the switches after them.
const BLOCK: usize = 0x100;
/// Articulators an old state leaves out, which FL resets: before version 4
/// the unison pitch and phase (`DAT_0087d7a0` = 0x30), before 10 rotation
/// and EQ shifting (`DAT_0087d7a1` = 0x18, from articulator 8).
const BEFORE_4: u8 = 0x30;
const BEFORE_10: u8 = 0x18;

/// The two plugins this file plays. Their DLLs share the code from the
/// state loader to the post chain. Autogun is Ogun with random presets: it
/// saves the random-preset seed, has magic dust and puts its master level
/// a quarter lower.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Autogun,
    Ogun,
}

/// The switches at plugin+0x208, by their offset from it.
const MOD_SMOOTH: usize = 0;
const KILL_SAME: usize = 1;
const KEY_PITCH: usize = 3;
const KEY_SEED: usize = 4;
const VEL_ENV: usize = 5;
const DECLICK: usize = 6;
const RELEASE_ON_ENV: usize = 7;
const PORTA: usize = 10;
const LOG_CURVES: usize = 11;

/// The unison copies' detune shares (`DAT_008ff730`).
const DETUNE: [f32; 9] = [
    0.877, 1.069, 0.929, 1.181, 1.031, 1.129, 0.887, 1.117, 0.977,
];

/// FL's `Round`.
fn round(x: f64) -> f64 {
    x.round_ties_even()
}

/// One articulator as the state has it (`TOpArticulator` over
/// `TArticulator`, `FUN_0087a4d0`): a base, envelope and LFO amounts, the
/// envelope, the LFO and the keyboard, velocity, mod X and mod Y maps.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtState {
    pub base: f32,
    pub env_amount: f32,
    pub lfo_amount: f32,
    pub(crate) envelope: MEnvelope,
    pub(crate) lfo: MEnvelope,
    pub(crate) maps: [MEnvelope; 4],
}

/// Autogun's event 213 (`SaveRestoreState` @ `0x0087cb90`): a version
/// word and a zlib stream of the block at plugin+0x12c, three switches,
/// the EQ band types, the info text, the random-preset seed, 13
/// articulators and 4 curves.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub version: i32,
    pub params: [i32; PARAMS],
    /// Filter type (0 low, 1 band, 2 notch, 3 high pass), its stages (0
    /// off to 3) and the alternate resonance switch.
    pub filter_type: i32,
    pub filter_stages: i32,
    pub filter_alt: i32,
    /// The richness wheel plus 5: the table is `2^order` samples.
    pub order: i32,
    pub reverb_on: bool,
    pub reverb_tempo: bool,
    /// Quality, 0 to 4 (1 is the default).
    pub quality: i32,
    /// The switches at plugin+0x208..+0x213.
    pub flags: [u8; 12],
    /// The harmonic comb of each curve.
    pub combs: [i32; SHAPES],
    pub to_target: bool,
    pub normalise: bool,
    pub weighted: bool,
    pub spare: u8,
    /// Magic dust, 0 to 1000.
    pub magic: i32,
    pub delay_tempo: bool,
    pub delay_on: bool,
    pub spare2: u8,
    pub eq_types: [i32; 3],
    pub info: String,
    /// The random-preset generator's seed (version 19).
    pub seed: i32,
    pub arts: Vec<ArtState>,
    pub(crate) shapes: Vec<MEnvelope>,
}

impl State {
    /// Read an Autogun event 213. A version FL would not read is turned
    /// down.
    pub fn parse(bytes: &[u8]) -> Result<State, StateError> {
        State::parse_as(bytes, Kind::Autogun)
    }

    /// Read an event 213 as this plugin's DLL does. An old state is a
    /// shorter block read over the constructor's, and leaves out
    /// articulators and curves that FL then resets.
    pub fn parse_as(bytes: &[u8], kind: Kind) -> Result<State, StateError> {
        let raw = crate::state_version(bytes, 8)?;
        if raw == i32::MAX || raw >= 0x13_0004 {
            return Err(StateError::Version { found: raw });
        }
        let version = if raw > 0xffff { raw >> 16 } else { raw };
        debug_assert!(version <= NEWEST);
        let body = miniz_oxide::inflate::decompress_to_vec_zlib(&bytes[4..]).map_err(|_| {
            StateError::Length {
                expected: bytes.len() + 1,
                found: bytes.len(),
            }
        })?;
        let mut stream = Reader::new(&body);
        // Before version 8 there is no fine pitch, and FL sets it to 0.
        let mut block = default_block();
        let first = if version < 8 { 8 } else { 12 };
        block[..first].copy_from_slice(stream.bytes(first)?);
        block[first..12].fill(0);
        let rest = block_rest(version);
        block[12..12 + rest].copy_from_slice(stream.bytes(rest)?);
        let mut r = Reader::new(&block);
        let mut params = [0; PARAMS];
        for p in &mut params {
            *p = r.i32()?;
        }
        let filter_type = r.i32()?;
        let filter_stages = r.i32()?;
        let filter_alt = r.i32()?;
        let order = r.i32()?;
        let switches = r.bytes(4)?;
        let quality = r.i32()?;
        let mut flags = [0u8; 12];
        flags.copy_from_slice(r.bytes(12)?);
        let mut combs = [0; SHAPES];
        for c in &mut combs {
            *c = r.i32()?;
        }
        let more = r.bytes(4)?;
        let magic = r.i32()?;
        r.end()?;
        let mut r = stream;
        let three = r.bytes(3)?;
        let mut eq_types = [0; 3];
        for t in &mut eq_types {
            *t = r.i32()?;
        }
        let info = if version < 18 {
            let len = usize::try_from(r.i32()?).unwrap_or(0);
            r.bytes(len)?.iter().map(|b| char::from(*b)).collect()
        } else {
            let mut len = usize::from(r.u8()?);
            if len == 0xff {
                len = r.u32()? as usize;
            }
            let raw = r.bytes(2 * len)?;
            let (pairs, _) = raw.as_chunks::<2>();
            let units: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
            String::from_utf16_lossy(&units)
        };
        let seed = if kind == Kind::Autogun && version > 18 {
            r.i32()?
        } else {
            0
        };
        let mut arts = Vec::with_capacity(ARTICULATORS);
        for k in 0..ARTICULATORS {
            let reset = (version < 4 && k < 8 && (BEFORE_4 >> k) & 1 != 0)
                || (version < 10 && (8..16).contains(&k) && (BEFORE_10 >> (k - 8)) & 1 != 0);
            if reset {
                arts.push(blank_art(k));
                continue;
            }
            let _format = r.u32()?;
            let base = r.f32()?;
            let env_amount = r.f32()?;
            let lfo_amount = r.f32()?;
            let envelope = MEnvelope::read(&mut r, Class::Envelope)?;
            let lfo = MEnvelope::read(&mut r, Class::Lfo)?;
            let maps = [
                MEnvelope::read(&mut r, Class::Map)?,
                MEnvelope::read(&mut r, Class::Map)?,
                MEnvelope::read(&mut r, Class::Map)?,
                MEnvelope::read(&mut r, Class::Map)?,
            ];
            arts.push(ArtState {
                base,
                env_amount,
                lfo_amount,
                envelope,
                lfo,
                maps,
            });
        }
        let mut shapes = Vec::with_capacity(SHAPES);
        for s in 0..SHAPES {
            // Before version 5 there are no forced harmonics; before 6 a
            // curve counted in partials and had no log axis.
            if version < 5 && s == eng::FORCED {
                shapes.push(blank_shape(s));
                continue;
            }
            let mut shape = MEnvelope::read(&mut r, Class::Map)?;
            if version < 6 {
                old_curve(&mut shape, order);
                flags[LOG_CURVES] = 1;
            }
            shapes.push(shape);
        }
        r.end()?;
        // Before 14 the curves were never weighted (`0x226`).
        let weighted = version >= 14 && more[2] != 0;
        Ok(State {
            version,
            params,
            filter_type,
            filter_stages,
            filter_alt,
            order,
            reverb_on: switches[0] != 0,
            reverb_tempo: switches[1] != 0,
            quality,
            flags,
            combs,
            to_target: more[0] != 0,
            normalise: more[1] != 0,
            weighted,
            spare: more[3],
            magic,
            delay_tempo: three[0] != 0,
            delay_on: three[1] != 0,
            spare2: three[2],
            eq_types,
            info,
            seed,
            arts,
            shapes,
        })
    }

    fn flag(&self, which: usize) -> u8 {
        self.flags[which]
    }
}

/// `TArticulator`'s combination of two values by its mode.
fn combine(additive: bool, a: f64, b: f64) -> f64 {
    if additive { a + b } else { a * b }
}

/// One articulator made ready.
#[derive(Debug, Clone, PartialEq)]
struct Art {
    additive: bool,
    centre: f64,
    base: f32,
    env_amount: f32,
    lfo_amount: f32,
    key: Curve,
    key_from: f64,
    vel: Curve,
    vel_from: f64,
    /// The mod X and mod Y maps baked in 513 steps less their centre, or
    /// none when the map does nothing (`FUN_00885c80`).
    mods: [Option<Vec<f32>>; 2],
    envelope: Envelope,
    lfo: Lfo,
    env_on: bool,
    lfo_on: bool,
}

impl Art {
    fn new(state: &ArtState, index: usize) -> Self {
        let additive = BIPOLAR & (1 << index) != 0;
        let centre = if additive { 0.5 } else { 0.0 };
        let neutral = if additive { 0.0 } else { 1.0 };
        let mods = [2, 3].map(|m| {
            let map = &state.maps[m];
            match map.flat() {
                Some(level) if (f64::from(level) - centre - neutral).abs() < 1e-9 => None,
                _ => Some(map.table(MAP_STEPS, centre)),
            }
        });
        let from = |m: &MEnvelope| m.points.first().map_or(0.0, |p| p.x.max(0.0));
        Art {
            additive,
            centre,
            base: state.base,
            env_amount: state.env_amount,
            lfo_amount: state.lfo_amount,
            key: Curve::new(&state.maps[0].points),
            key_from: from(&state.maps[0]),
            vel: Curve::new(&state.maps[1].points),
            vel_from: from(&state.maps[1]),
            mods,
            envelope: Envelope::new(&state.envelope, centre),
            lfo: Lfo::new(&state.lfo),
            env_on: state.envelope.active && !state.envelope.points.is_empty(),
            lfo_on: state.lfo.active,
        }
    }

    fn live(&self) -> bool {
        self.env_on || self.lfo_on
    }

    /// `FUN_0078d5f0`: the keyboard map at `key / 120` with the velocity
    /// map, as an f32.
    fn note(&self, key: f32, velocity: f32) -> f64 {
        let k = self.key.value_at(self.key_from + f64::from(key) / 120.0) - self.centre;
        let v = self.vel.value_at(self.vel_from + f64::from(velocity)) - self.centre;
        f64::from(combine(self.additive, k, v) as f32)
    }

    /// The mod X and mod Y maps at their places on top of `value`.
    fn mods(&self, value: f64, places: [usize; 2]) -> f64 {
        let mut value = value;
        for (map, place) in self.mods.iter().zip(places) {
            if let Some(map) = map {
                value = combine(self.additive, value, f64::from(map[place.min(MAP_STEPS)]));
            }
        }
        value
    }
}

/// Where a voice is in one of the first five articulators.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Run {
    env: EnvRun,
    lfo: Option<LfoRun>,
    value: f64,
}

/// One biquad stage, FL's direct form I in f64 over a stereo pair
/// (`FUN_00626ec0`): `b0 b1 b2 a1 a2` and the two pairs of history.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Biquad {
    c: [f64; 5],
    x1: [f64; 2],
    x2: [f64; 2],
    y1: [f64; 2],
    y2: [f64; 2],
}

impl Biquad {
    /// `FUN_006264d0` for the types Autogun uses: 1 low pass, 2 band pass,
    /// 3 notch, 4 high pass, 6 low shelf, 7 peak, 8 high shelf. `a` is the
    /// shelf or peak gain and `alpha` the width term (for the shelves the
    /// factor that times `sin` is `2 sqrt(A) alpha`).
    fn design(&mut self, kind: i32, a: f64, alpha: f64, sin: f64, cos: f64) {
        self.c = match kind {
            1..=4 => {
                let d = 1.0 / (alpha + 1.0);
                let a1 = d * cos * -2.0;
                let a2 = d * (1.0 - alpha);
                match kind {
                    1 => {
                        let b1 = d * (1.0 - cos);
                        [b1 * 0.5, b1, b1 * 0.5, a1, a2]
                    }
                    2 => [d * alpha, 0.0, -(d * alpha), a1, a2],
                    3 => [d, a1, d, a1, a2],
                    _ => {
                        let b1 = -(d * (cos + 1.0));
                        let b0 = -b1 * 0.5;
                        [b0, b1, b0, a1, a2]
                    }
                }
            }
            6 => {
                let dm = (a - 1.0) * cos;
                let beta = alpha * sin;
                let d = 1.0 / (a + 1.0 + dm + beta);
                [
                    d * a * ((a + 1.0 - dm) + beta),
                    d * a * 2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    d * a * ((a + 1.0 - dm) - beta),
                    d * ((a - 1.0) + (a + 1.0) * cos) * -2.0,
                    d * ((a + 1.0 + dm) - beta),
                ]
            }
            7 => {
                let d = 1.0 / (alpha / a + 1.0);
                let a1 = d * cos * -2.0;
                [
                    d * (alpha * a + 1.0),
                    a1,
                    d * (1.0 - alpha * a),
                    a1,
                    d * (1.0 - alpha / a),
                ]
            }
            8 => {
                let dm = (a - 1.0) * cos;
                let beta = alpha * sin;
                let d = 1.0 / ((a + 1.0 - dm) + beta);
                [
                    d * a * (a + 1.0 + dm + beta),
                    d * a * -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    d * a * ((a + 1.0 + dm) - beta),
                    d * ((a - 1.0) - (a + 1.0) * cos) * 2.0,
                    d * ((a + 1.0 - dm) - beta),
                ]
            }
            _ => self.c,
        };
    }

    /// Run over interleaved stereo frames in place.
    fn run(&mut self, frames: &mut [f32]) {
        let [b0, b1, b2, a1, a2] = self.c;
        for pair in frames.as_chunks_mut::<2>().0 {
            for s in 0..2 {
                let x = f64::from(pair[s]);
                let y = x * b0 + ((self.x1[s] * b1 - self.y2[s] * a2) + self.x2[s] * b2)
                    - self.y1[s] * a1;
                self.x2[s] = self.x1[s];
                self.x1[s] = x;
                self.y2[s] = self.y1[s];
                self.y1[s] = y;
                pair[s] = y as f32;
            }
        }
        let history = self
            .x1
            .iter_mut()
            .chain(&mut self.x2)
            .chain(&mut self.y1)
            .chain(&mut self.y2);
        for v in history {
            if v.abs() < 1e-30 {
                *v = 0.0;
            }
        }
    }

    fn clear(&mut self) {
        self.x1 = [0.0; 2];
        self.x2 = [0.0; 2];
        self.y1 = [0.0; 2];
        self.y2 = [0.0; 2];
    }
}

/// One note (`TriggerVoice` @ `0x00882a90`, the 0x10c0-byte voice).
#[derive(Debug, Clone, PartialEq)]
struct Voice {
    id: u32,
    active: bool,
    age: u64,
    /// 1 playing, 5 let go, 9 ending, 10 over (+0x10).
    state: i32,
    /// 9 when every running articulator is over, less while one runs
    /// (+0x14).
    over: i32,
    /// Samples to the next control tick; below 0 before the first.
    countdown: i32,
    first: bool,
    key: i32,
    note: f32,
    /// The levels the host gave (`FinalLevels`).
    pan: f32,
    volume: f32,
    pitch: f32,
    fcut: f32,
    fres: f32,
    velocity: f32,
    release_velocity: f32,
    /// The timbre's pitch in cents and the unison detune last set.
    cents: f32,
    detune: f32,
    /// Each articulator's value at the note (+0x80).
    notes: [f64; ARTICULATORS],
    runs: [Run; RUN],
    live: [bool; RUN],
    mods: [f32; 2],
    places: [usize; 2],
    /// Gains: the targets, where the ramp ends, where it is, its steps.
    target: [f32; 4],
    end: [f32; 4],
    gains: [f32; 4],
    steps: [f32; 4],
    declick: i32,
    ramp: f32,
    ramp_step: f32,
    held: [f32; 2],
    filters: [Biquad; 3],
    let_go: bool,
    release_on_env: bool,
    timbre: Timbre,
}

/// The post chain's classes and their front end.
#[derive(Debug, Clone, PartialEq)]
struct Effects {
    chorus: StereoFlanger,
    chorus_on: bool,
    chorus_dry: f32,
    /// Samples the chorus and delay stay awake for with no voices.
    hold: i32,
    sleep: i32,
    delay: Delay32,
    delay_on: bool,
    kernel: DelayKernel,
    delay_offsets: [usize; 2],
    delay_gain: f32,
    reverb: FruityReverb,
    reverb_on: bool,
    reverb_wet: f32,
    /// The EQ bands, and whether each is on.
    bands: [Biquad; 3],
    band_on: [bool; 3],
}

/// What follows from the state, the rate and the tempo.
#[derive(Debug, Clone, PartialEq)]
struct Derived {
    rate: f64,
    tempo: f64,
    /// Control tick and frame in samples, declick length.
    tick: i32,
    hop: i32,
    declick: i32,
    /// Mod X / Y smoothing a tick.
    smooth: f32,
    /// Articulator rates: half seconds and beats a tick.
    rates: [f64; 2],
    master: f32,
    cents: i32,
    fullness: f32,
    predecay: f32,
    decay: f32,
    release: f32,
    morph: f32,
    seeds: [i32; 2],
    cutoff: f32,
    resonance: f32,
    /// The filter's type, its stages less 1, the width factor of its bands
    /// and the root its resonance takes.
    filter_kind: u8,
    filter_last: i32,
    filter_width: f32,
    filter_root: f32,
    filter_alt: i32,
    /// The filter runs at twice the rate under 88.2 kHz, and what a radian
    /// a sample at 44.1 kHz is then.
    doubled: bool,
    filter_scale: f32,
}

/// Autogun, or Ogun when made with [`Autogun::new`] of [`Kind::Ogun`].
pub struct Autogun {
    kind: Kind,
    state: State,
    d: Derived,
    arts: Vec<Art>,
    engine: Engine,
    work: Work,
    voices: Vec<Voice>,
    fx: Box<Effects>,
    sg_state: Option<Vec<u8>>,
    sg: Option<Box<Maximus>>,
    missing_sg: bool,
    /// The counter a seed 2 of 0 draws a new seed from (+0x244).
    counter: i32,
    clock: u64,
    /// A voice's frames, interleaved, and twice that for the filter.
    vbuf: Vec<f32>,
    vbuf2: Vec<f32>,
    left: Vec<f32>,
    right: Vec<f32>,
    wet_left: Vec<f32>,
    wet_right: Vec<f32>,
    mono: Vec<f32>,
}

/// The default state: the parameters at their defaults, every
/// articulator and curve idle.
fn blank_state() -> State {
    let arts = (0..ARTICULATORS).map(blank_art).collect();
    State {
        version: NEWEST,
        params: DEFAULTS,
        filter_type: 0,
        filter_stages: 0,
        filter_alt: -1,
        order: 14,
        reverb_on: false,
        reverb_tempo: false,
        quality: 1,
        flags: [1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0],
        combs: [0; SHAPES],
        to_target: false,
        normalise: false,
        weighted: false,
        spare: 0,
        magic: 430,
        delay_tempo: true,
        delay_on: false,
        spare2: 0,
        eq_types: [0, 1, 2],
        info: String::new(),
        seed: 0,
        arts,
        shapes: (0..SHAPES).map(blank_shape).collect(),
    }
}

/// Articulator `k` as the constructor makes it and as `FUN_0087ed10`
/// resets it: every map flat at the value that changes nothing.
fn blank_art(k: usize) -> ArtState {
    let knobs = crate::menvelope::ENVELOPE_KNOBS;
    let idle = |kind: u32, level: f64| crate::harmor_state::idle(kind, level, knobs);
    let centre = if BIPOLAR & (1 << k) != 0 { 0.5 } else { 1.0 };
    ArtState {
        base: 0.0,
        env_amount: 1.0,
        lfo_amount: 1.0,
        envelope: idle(1, 0.0),
        lfo: crate::harmor_state::idle(2, 0.5, crate::menvelope::LFO_KNOBS),
        maps: [
            idle(3, centre),
            idle(3, centre),
            idle(3, centre),
            idle(3, centre),
        ],
    }
}

/// The constructor's curve `s` (`FUN_0078cc00`): no forced harmonics, the
/// three EQs flat at their middle, which is a gain of 1.
fn blank_shape(s: usize) -> MEnvelope {
    let level = if s == eng::FORCED { 0.0 } else { 0.5 };
    crate::harmor_state::idle(3, level, crate::menvelope::ENVELOPE_KNOBS)
}

/// The block as the constructor leaves it and `SaveRestoreState` resets
/// it before a read (+0x20f..+0x213, the combs, +0x224, +0x225, +0x227 and
/// magic dust at 430): an old state overwrites only the front of it.
fn default_block() -> [u8; BLOCK] {
    let s = blank_state();
    let mut b = Vec::with_capacity(BLOCK);
    for v in s.params {
        b.extend(v.to_le_bytes());
    }
    for v in [s.filter_type, s.filter_stages, s.filter_alt, s.order] {
        b.extend(v.to_le_bytes());
    }
    b.extend([u8::from(s.reverb_on), u8::from(s.reverb_tempo), 0, 0]);
    b.extend(s.quality.to_le_bytes());
    b.extend(s.flags);
    for v in s.combs {
        b.extend(v.to_le_bytes());
    }
    b.extend([
        u8::from(s.to_target),
        u8::from(s.normalise),
        u8::from(s.weighted),
        s.spare,
    ]);
    b.extend(s.magic.to_le_bytes());
    let mut block = [0; BLOCK];
    block.copy_from_slice(&b);
    block
}

/// How much of the block after its first 12 bytes a version saves
/// (`SaveRestoreState`): each newer one added to its end.
fn block_rest(version: i32) -> usize {
    let mut rest = if version < 1 { 0xf3 } else { 0xf4 };
    for (before, less) in [(2, 3), (7, 1), (9, 16), (11, 1), (12, 1), (13, 6)] {
        if version < before {
            rest -= less;
        }
    }
    rest
}

/// A curve saved before version 6 (`SaveRestoreState`, the `v < 6` part):
/// its `x` counted partials of the richness it was saved at, so it is
/// stretched to `(2^(order-1) - 1) / 32767` of its length and held at its
/// last level to the end, then read again at 16 points for the log axis,
/// point `i` at `i / 15` with the value at `2^i / 32767`.
fn old_curve(shape: &mut MEnvelope, order: i32) {
    let partials = 1i32.wrapping_shl(((order - 1) & 31) as u32).wrapping_sub(1);
    let scale = f64::from(partials) / 32_767.0;
    for p in &mut shape.points {
        p.x *= scale;
    }
    let last = shape.points.last().map_or(0.0, |p| p.y as f32);
    add_point(&mut shape.points, 1.0, last);
    let curve = Curve::new(&shape.points);
    let from = shape.points.first().map_or(0.0, |p| p.x.max(0.0));
    let mut points = Vec::with_capacity(16);
    for i in 0..16 {
        let y = curve.value_at(from + f64::from(1u32 << i) / 32_767.0) as f32;
        add_point(&mut points, f64::from(i) / 15.0, y);
    }
    shape.arp = vec![0; points.len()];
    shape.points = points;
}

/// `FUN_00786c60`: a straight point at `x` from the start of a one unit
/// long map, before the first point after it, with its `x` and the next
/// point's kept relative as FL keeps them (in f32 steps).
fn add_point(points: &mut Vec<dsp::curve::Point>, x: f64, y: f32) {
    let x = x.clamp(0.0, 1.0);
    let (mut index, mut before) = (points.len(), 0.0);
    if let Some(first) = points.first() {
        let mut at = first.x;
        before = at;
        for (i, p) in points.iter().enumerate().skip(1) {
            let next = at + p.x;
            if x < next {
                index = i;
                break;
            }
            at = next;
            before = at;
        }
    }
    let step = ((x - before) as f32).max(0.0);
    points.insert(index, dsp::curve::Point::new(f64::from(step), f64::from(y)));
    if let Some(next) = points.get_mut(index + 1) {
        next.x -= f64::from(step);
    }
}

/// The settings of a quality (`FUN_0087e750`): interpolator (1 linear, 2
/// the 6-point, 6 a longer one), oversampling bits, frame length in ms and
/// the control tick at 44.1 kHz. A quality past 4 changes nothing, so the
/// constructor's, which are quality 1's, stay.
fn quality(q: i32) -> (u32, u32, i32, i32) {
    match q {
        0 => (1, 0, 50, 96),
        2 => (2, 1, 50, 48),
        3 => (2, 2, 25, 24),
        4 => (6, 2, 25, 8),
        _ => (2, 0, 50, 96),
    }
}

/// The master level curve `(3^x - 1) / 2`, 1 at 1 (`FUN_00882700`).
fn level3(x: f32) -> f32 {
    if x == 1.0 {
        1.0
    } else {
        ((f64::from(x * 3f64.ln() as f32).exp() - 1.0) * 0.5) as f32
    }
}

/// The delay level curve `(11^x - 1) / 10`, 1 at 1.
fn level11(x: f32) -> f32 {
    if x == 1.0 {
        1.0
    } else {
        ((f64::from(x * 11f64.ln() as f32).exp() - 1.0) * f64::from(0.1f32)) as f32
    }
}

/// A seed wheel's seed (ProcessParam for REC 10 and 11): -1 at 0.
fn wheel_seed(value: i32) -> i32 {
    if value >= 1 {
        eng::mix(value - 0x20cf, 0x1cfc)
    } else {
        value - 1
    }
}

/// `IntPower`.
fn powi(x: f64, n: i32) -> f64 {
    let mut result = 1.0;
    for _ in 0..n.unsigned_abs() {
        result *= x;
    }
    if n < 0 { 1.0 / result } else { result }
}

/// FL's pan law (`FUN_0062cce0`): cos and sin of `(1 + pan) pi / 4`
/// times a gain.
fn pan_law(pan: f32, gain: f32) -> (f32, f32) {
    let angle = f64::from(1.0 + pan) * (PI / 4.0);
    (
        (angle.cos() * f64::from(gain)) as f32,
        (angle.sin() * f64::from(gain)) as f32,
    )
}

/// `FUN_00617ae0`: a voice's four gains for a pan and a level: left,
/// right, right into left, left into right.
fn stereo_pan(pan: f32, vol: f32) -> [f32; 4] {
    if pan == 0.0 {
        return [vol, vol, 0.0, 0.0];
    }
    if vol == 0.0 {
        return [0.0; 4];
    }
    let (l, r) = pan_law(pan, vol * std::f32::consts::SQRT_2);
    let (into_left, into_right) = if pan <= 0.0 {
        (1.0 - r / vol, 0.0)
    } else {
        (0.0, 1.0 - l / vol)
    };
    [
        l * (1.0 - into_left * 0.5),
        r * (1.0 - into_right * 0.5),
        l * into_left * 0.5,
        r * into_right * 0.5,
    ]
}

/// `FUN_0062cc10`: a gain's ramp from where the last one ended to
/// `target` over `ticks` samples, no steeper than `cap` a sample.
fn ramp(step: &mut f32, end: &mut f32, now: &mut f32, target: f32, cap: f32, ticks: i32) {
    let ticks = ticks.max(1) as f32;
    *now = *end;
    *step = target - *now;
    if *step != 0.0 {
        *end = target;
        *step /= ticks;
        if cap < step.abs() {
            *step = cap.copysign(*step);
            *end = *now + *step * ticks;
            if end.abs() < 5.960_464_5e-8 {
                *end = 0.0;
                *step = -*now / ticks;
            }
        }
    }
}

/// The velocity, 0 to 1, under a note's velocity gain: FL's
/// `FPV_GetVelocity`, as `harmor.rs` works it back.
fn velocity_of(gain: f32) -> f32 {
    let x = ((f64::from(gain) * 10.0 + 1.0).ln() / 11f64.ln()) * 100.0 / 128.0;
    x.clamp(0.0, 1.0) as f32
}

impl Default for Autogun {
    fn default() -> Self {
        Autogun::new(Kind::Autogun)
    }
}

impl Autogun {
    /// The plugin of this kind at its defaults, which both DLLs' constructors
    /// share.
    pub fn new(kind: Kind) -> Self {
        let state = blank_state();
        let rate = 44_100.0;
        let engine = Engine::new(state.order as u32, 0, rate);
        let work = Work::new(&engine);
        let mut plugin = Autogun {
            kind,
            arts: Vec::new(),
            d: Derived {
                rate,
                tempo: 140.0,
                tick: 96,
                hop: 2205,
                declick: 96,
                smooth: 0.0,
                rates: [0.0; 2],
                master: 1.0,
                cents: 0,
                fullness: 0.0,
                predecay: 0.0,
                decay: 0.0,
                release: 0.0,
                morph: 0.0,
                seeds: [-1, -1],
                cutoff: 0.5,
                resonance: 0.5,
                filter_kind: 0,
                filter_last: -1,
                filter_width: 1.0,
                filter_root: 1.0,
                filter_alt: 0,
                doubled: true,
                filter_scale: 0.5,
            },
            engine,
            work,
            voices: Vec::new(),
            fx: Box::new(Effects {
                chorus: StereoFlanger::default(),
                chorus_on: false,
                chorus_dry: 1.0,
                hold: 0,
                sleep: 0,
                delay: Delay32::default(),
                delay_on: false,
                kernel: DelayKernel::Straight,
                delay_offsets: [0, 0],
                delay_gain: 1.0,
                reverb: FruityReverb::new(44_100, 1),
                reverb_on: false,
                reverb_wet: 0.0,
                bands: [Biquad::default(); 3],
                band_on: [false; 3],
            }),
            sg_state: None,
            sg: None,
            missing_sg: false,
            counter: 0,
            clock: 0,
            vbuf: vec![0.0; 2 * CHUNK],
            vbuf2: vec![0.0; 4 * CHUNK],
            left: vec![0.0; CHUNK],
            right: vec![0.0; CHUNK],
            wet_left: vec![0.0; CHUNK],
            wet_right: vec![0.0; CHUNK],
            mono: vec![0.0; CHUNK],
            state,
        };
        plugin.load();
        plugin
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Everything that follows from a new state or rate: the engine and its
    /// curves, the articulators, the voices' buffers and the effects. This
    /// allocates.
    fn load(&mut self) {
        let rate = self.d.rate;
        let (_, bits, _, _) = quality(self.state.quality);
        let order = self.state.order.clamp(6, 20) as u32;
        let bits = if order < 10 && rate < 88_200.0 {
            bits.max(1)
        } else {
            bits
        };
        if self.engine.order != order || self.engine.bits != bits {
            self.engine = Engine::new(order, bits, rate);
            self.work = Work::new(&self.engine);
        } else {
            self.engine.set_rate(rate);
        }
        self.engine.to_target = self.state.to_target;
        self.engine.normalise = self.state.normalise;
        self.bake_curves();
        self.arts = self
            .state
            .arts
            .iter()
            .enumerate()
            .map(|(k, a)| Art::new(a, k))
            .collect();
        let engine = &self.engine;
        self.voices.retain(|v| v.timbre.fits(engine));
        while self.voices.len() < VOICES {
            let voice = self.new_voice();
            self.voices.push(voice);
        }
        for v in &mut self.voices {
            v.active = false;
        }
        self.build_effects();
        self.derive();
        self.build_sg();
    }

    fn new_voice(&self) -> Voice {
        let idle = crate::harmor_state::idle(1, 0.0, crate::menvelope::ENVELOPE_KNOBS);
        let env = Envelope::new(&idle, 0.0).start(1.0);
        Voice {
            id: 0,
            active: false,
            age: 0,
            state: 10,
            over: 9,
            countdown: 0,
            first: true,
            key: 60,
            note: 60.0,
            pan: 0.0,
            volume: 1.0,
            pitch: 0.0,
            fcut: 0.0,
            fres: 0.0,
            velocity: 0.78,
            release_velocity: 0.5,
            cents: 0.0,
            detune: 0.0,
            notes: [0.0; ARTICULATORS],
            runs: [Run {
                env,
                lfo: None,
                value: 0.0,
            }; RUN],
            live: [false; RUN],
            mods: [256.0; 2],
            places: [256; 2],
            target: [0.0; 4],
            end: [0.0; 4],
            gains: [0.0; 4],
            steps: [0.0; 4],
            declick: -1,
            ramp: 0.0,
            ramp_step: 0.0,
            held: [0.0; 2],
            filters: [Biquad::default(); 3],
            let_go: false,
            release_on_env: false,
            timbre: Timbre::new(&self.engine),
        }
    }

    /// `FUN_0087e900` and `FUN_00886960` for the four curves: each read at
    /// every partial's place, mapped, weighted, combed and handed to the
    /// engine.
    fn bake_curves(&mut self) {
        let n = self.engine.partials;
        let wide = (n + n / 2).min(0x8000);
        let log = self.state.flag(LOG_CURVES) != 0;
        let place = |i: usize| {
            if log {
                ((i + 1) as f64).log2() / 15.0
            } else {
                i as f64 / 32_767.0
            }
        };
        let weighted = self.state.weighted;
        let to_target = self.state.to_target;
        for s in 0..SHAPES {
            let shape = &self.state.shapes[s];
            if s == eng::FORCED && shape.flat() == Some(0.0) {
                self.engine.set_curve(s, None);
                continue;
            }
            let count = if s == eng::FORCED { n } else { wide };
            let curve = Curve::new(&shape.points);
            let from = shape.points.first().map_or(0.0, |p| p.x.max(0.0));
            let weigh = weighted && s + 13 <= usize::from(to_target) + 14;
            let mut values = Vec::with_capacity(count);
            for i in 0..count {
                let x = from + place(i) + 5.960_464_477_539_063e-8;
                let v = (curve.value_at(x) as f32).clamp(0.0, 1.0);
                let mut v = if s == eng::FORCED {
                    v
                } else if v * 2.0 == 1.0 {
                    1.0
                } else {
                    let e = f64::from(v * 2.0 * 3f64.ln() as f32).exp();
                    ((e - 1.0) * 0.5) as f32
                };
                if weigh {
                    v *= (1.0 / ((i + 1) as f64).sqrt()) as f32;
                }
                values.push(v);
            }
            let comb = self.state.combs[s];
            if comb > 0 {
                let fill = if COMB_ONES & (1 << (s + 13 - 8)) != 0 {
                    1.0
                } else {
                    0.0
                };
                let comb = comb as usize;
                for (i, v) in values.iter_mut().enumerate() {
                    let hit = if comb < 5 {
                        i % (comb + 1) == comb
                    } else {
                        i % (comb - 2) > 0
                    };
                    if hit {
                        *v = fill;
                    }
                }
            }
            self.engine.set_curve(s, Some(&values));
        }
    }

    /// The parameters' and the switches' derived values: ProcessParam's
    /// setters (`0x00880670`) and the rate setter (`FUN_0087c6d0`). This
    /// allocates nothing.
    fn derive(&mut self) {
        let p = self.state.params;
        let rate = self.d.rate;
        let (_, _, hop_ms, tick_base) = quality(self.state.quality);
        let d = &mut self.d;
        d.hop = round(f64::from(hop_ms) * 0.001 * rate) as i32;
        let nominal = f64::from(tick_base) * rate / 44_100.0;
        let tick = (round(nominal / 4.0) as i32 * 4).max(4);
        d.tick = tick;
        let ratio = f64::from(tick * tick_base) / (nominal * 96.0);
        d.smooth = ((0.01f64.ln() * ratio) / 96.0).exp() as f32;
        let seconds = ratio * 96.0 / 44_100.0;
        d.rates = [seconds * 2.0, seconds * f64::from(d.tempo as f32) / 60.0];
        let host = 44_100.0 / rate;
        d.declick = round(96.0 / (host * host)) as i32;
        // Ogun's master level is `level3` itself (`FUN_0086a570`); Autogun
        // takes a quarter of it (`FUN_00882700`).
        let master = level3(p[0] as f32 * 0.007_812_5);
        d.master = match self.kind {
            Kind::Autogun => (f64::from(master) * 0.25) as f32,
            Kind::Ogun => master,
        };
        d.cents = p[1] * 100 + p[2];
        d.fullness = p[8] as f32 * 0.001;
        d.predecay = p[5] as f32 * 0.001;
        d.decay = if p[6] == 10_000 {
            -1.0
        } else {
            (f64::from(p[6]) * 0.0001) as f32
        };
        d.release = (f64::from(p[7]) * 0.0001) as f32;
        d.morph = p[9] as f32 * 0.001;
        d.seeds = [wheel_seed(p[10]), wheel_seed(p[11])];
        d.cutoff = p[17] as f32 * 0.003_906_25;
        d.resonance = p[18] as f32 * 0.003_906_25;
        d.filter_kind = self.state.filter_type.clamp(0, 3) as u8;
        d.filter_last = self.state.filter_stages.clamp(0, 3) - 1;
        let stages = (d.filter_last + 1).max(1) as f32;
        let width = if d.filter_kind == 2 {
            1.0 / stages
        } else {
            stages
        };
        d.filter_width = (f64::from(width) * (LN_2 / 2.0)) as f32;
        d.filter_root = 1.0 / stages;
        d.filter_alt = self.state.filter_alt;
        d.doubled = rate < 88_200.0;
        d.filter_scale = host as f32 * if d.doubled { 0.5 } else { 1.0 };
        self.set_effects();
    }

    /// The setters of the chorus (`FUN_00882460`), the delay
    /// (`FUN_00881f60`), the reverb (`FUN_00882230`) and the EQ
    /// (`FUN_00882820`), on the classes [`Autogun::build_effects`] made.
    /// This allocates nothing.
    fn set_effects(&mut self) {
        let p = self.state.params;
        let rate = self.d.rate;
        let hz = round(rate) as i32;
        let tempo = f64::from(self.d.tempo as f32);
        let fx = &mut *self.fx;

        fx.hold = hz * 100 / 44_100;
        fx.chorus_on = p[19] >= 1 && {
            if !fx.chorus_on {
                fx.chorus.fresh();
            }
            let mix = p[25].abs() as f32 * 0.007_812_5;
            let law = |x: f32| 1.0 - ((f64::from(x) * 5f64.ln()).exp() - 1.0) as f32 * 0.25;
            fx.chorus_dry = law(mix);
            let sign = if p[25] < 0 { -1.0 } else { 1.0 };
            let wheels = [p[19], p[20], p[21], p[22], p[23], p[24]];
            let settings = FlangerSettings::from_wheels(wheels, sign * law(1.0 - mix));
            fx.chorus.set(&settings, f64::from(hz));
            fx.hold += CHUNK as i32 + fx.chorus.length();
            true
        };

        fx.delay_on = self.state.delay_on && fx.delay.capacity() > 0 && {
            if !fx.delay_on {
                fx.delay.fresh();
            }
            let x = (f64::from(p[28].abs()) * 0.001_953_125) as f32;
            let spread = ((f64::from(x) * 1001f64.ln()).exp() - 1.0) as f32 * 50.0;
            let offset = round(f64::from(spread * hz as f32 / 100_000.0)) as usize;
            let time = if self.state.delay_tempo {
                f64::from(p[27] * hz) * 0.3125 / tempo
            } else {
                f64::from(p[27] * hz) / 384.0
            };
            fx.delay.set_length(CHUNK + offset, round(time) as usize);
            fx.delay_offsets = if p[28] > 0 { [0, offset] } else { [offset, 0] };
            fx.delay.set_feedback(p[26] as f32 * 0.007_812_5);
            let v = p[29] as f32 * 0.007_812_5;
            fx.delay_gain = match p[29].signum() {
                0 => 0.0,
                s => level11(v.abs()) * s as f32,
            };
            fx.kernel = match p[30] {
                1 => DelayKernel::Swapped,
                2 => DelayKernel::PingPong,
                _ => DelayKernel::Straight,
            };
            true
        };

        fx.reverb_on = self.state.reverb_on && {
            if !fx.reverb_on {
                fx.reverb.fresh();
            }
            let predelay = if self.state.reverb_tempo {
                f64::from(p[33] * hz) * 0.3125 / tempo
            } else {
                f64::from(p[33] * hz) / 384.0
            };
            let off = |v: i32, none: i32| if v == none { 0.0 } else { v as f32 };
            fx.reverb.set(ReverbSettings {
                low_cut: off(p[31], 19),
                high_cut: off(p[32], 221),
                predelay: round(predelay) as i32,
                room: p[34],
                diffusion: p[35],
                decay: (f64::from(p[36] * hz) / 10.0) as f32,
                damping: off(p[37], 221),
                colour: p[38],
            });
            fx.reverb_wet = p[39] as f32 * 0.003_906_25;
            true
        };

        for b in 0..3 {
            let gain = p[40 + b] as f32 * 0.01;
            fx.band_on[b] = p[40 + b] != 0;
            let x = (f64::from(p[43 + b]) * 1.525_878_906_25e-5) as f32;
            let grown = (f64::from(x) * 101f64.ln()).exp();
            let freq = (f64::from((grown - 1.0) as f32) * 159.9 + 10.0) as f32;
            let width = (f64::from(p[46 + b]) * 1.525_878_906_25e-5) as f32;
            let freq = freq.min((0.4999 * rate) as f32);
            let (sin, cos) = (f64::from(freq) * TAU / rate).sin_cos();
            let kind = self.state.eq_types[b].clamp(0, 2) + 6;
            let a = 10f64.powf(f64::from(gain) * 0.025);
            let alpha = if kind == 7 {
                let q = (f64::from(width) * 1001f64.ln()).exp() - 1.0;
                sin / ((f64::from(q as f32) * 0.0999 + 0.1) * 2.0)
            } else {
                ((a * a + 1.0) / (f64::from(width) * 1.9 + 0.1) - (a - 1.0) * (a - 1.0))
                    .max(0.0)
                    .sqrt()
            };
            fx.bands[b].design(kind, a, alpha, sin, cos);
        }
    }

    /// The memory of the effect classes at the rate, with room for the
    /// longest delay and predelay down to 20 BPM. This allocates.
    fn build_effects(&mut self) {
        let hz = round(self.d.rate) as i32;
        let fx = &mut *self.fx;
        fx.chorus.make_room(f64::from(hz));
        fx.chorus.fresh();
        fx.chorus_on = false;
        let slowest = (384.0 * 0.3125 / 20.0 * f64::from(hz)).max(f64::from(hz));
        let delay = if self.state.delay_on {
            CHUNK + (0.5 * f64::from(hz)) as usize + 2 + slowest as usize + 1
        } else {
            0
        };
        if fx.delay.capacity() != delay {
            fx.delay = Delay32::with_capacity(delay);
        }
        fx.delay.fresh();
        fx.delay_on = false;
        let predelay = if self.state.reverb_on {
            slowest as usize + 1
        } else {
            1
        };
        fx.reverb = FruityReverb::new(hz, predelay);
        fx.reverb.fresh();
        fx.reverb_on = false;
        fx.sleep = 0;
        for band in &mut fx.bands {
            band.clear();
        }
    }

    /// Magic dust: the Soundgoodizer of `SG.dat` with its mix on the
    /// wheel. This allocates.
    fn build_sg(&mut self) {
        self.sg = None;
        self.missing_sg = false;
        // Ogun's `Gen_Render` (`0x0086d450`) has no magic dust.
        if self.kind == Kind::Ogun || self.state.magic <= 0 {
            return;
        }
        let Some(bytes) = &self.sg_state else {
            self.missing_sg = true;
            return;
        };
        let mut maximus = Box::new(Maximus::default());
        maximus.set_sample_rate(self.d.rate as f32);
        if maximus.set_state(bytes).is_err() {
            self.missing_sg = true;
            return;
        }
        maximus.set_param(5, self.state.magic as f32);
        self.sg = Some(maximus);
    }

    /// The voice a new note takes: a free one, or the oldest.
    fn free_slot(&self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return i;
        }
        (0..self.voices.len())
            .min_by_key(|i| self.voices[*i].age)
            .unwrap_or(0)
    }

    /// TriggerVoice (`0x00882a90`).
    fn start(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let pitch = f32::from(note.key - 60) * 100.0 + note.fine_pitch;
        let note_f = (pitch * 0.01 + 60.0).max(0.0);
        let key = round(f64::from(note_f)) as i32;
        let vel = velocity_of(note.velocity);
        if self.state.flag(KILL_SAME) != 0 {
            for v in self.voices.iter_mut().filter(|v| v.active) {
                if v.key == key && v.state < 9 {
                    v.state = 9;
                }
            }
        }
        let slot = self.free_slot();
        self.clock += 1;
        let Autogun {
            state,
            d,
            arts,
            engine,
            voices,
            counter,
            clock,
            ..
        } = self;
        let p = state.params;
        let flags = state.flags;
        let vel_env = flags[VEL_ENV];
        let attack = if vel_env != 0 { 2.0 - vel * 2.0 } else { 0.0 };

        let voice = &mut voices[slot];
        voice.id = note.voice;
        voice.active = true;
        voice.age = *clock;
        voice.state = 1;
        voice.over = 9;
        voice.countdown = -d.tick;
        voice.first = true;
        voice.key = key;
        voice.note = note_f;
        voice.pan = note.pan.clamp(-1.0, 1.0);
        voice.volume = note.velocity;
        voice.pitch = pitch;
        voice.fcut = note.mod_x;
        voice.fres = note.mod_y;
        voice.velocity = vel;
        voice.release_velocity = note.release;
        voice.let_go = false;
        voice.mods = [p[3] as f32, p[4] as f32];
        let hosts = [voice.fcut, voice.fres];
        voice.places = [0, 1]
            .map(|m| round(f64::from(voice.mods[m] + hosts[m] * 256.0)).clamp(0.0, 512.0) as usize);
        voice.gains = [0.0; 4];
        voice.steps = [0.0; 4];
        voice.end = [0.0; 4];
        for f in &mut voice.filters {
            f.clear();
        }

        for k in 0..ARTICULATORS {
            if k == 8 {
                continue;
            }
            let art = &arts[k];
            let mut value = art.note(note_f, vel);
            if k < RUN {
                if k == 3 {
                    value *= 4.0;
                    if flags[KEY_PITCH] == 0 {
                        value -= f64::from(pitch) * (1.0 / 1200.0);
                    }
                }
                let mut env = art.envelope.start(d.rates[usize::from(art.envelope.tempo)]);
                if vel_env & 1 != 0 && art.env_on {
                    env.scale_attack(attack);
                }
                let lfo = art
                    .lfo_on
                    .then(|| art.lfo.start(d.rates[usize::from(art.lfo.tempo())]));
                let mut start = f64::from(art.base);
                if art.env_on {
                    start += f64::from(env.out) * f64::from(art.env_amount);
                }
                if let Some(l) = &lfo {
                    let swing = f64::from(l.out) * f64::from(art.lfo_amount);
                    start = if art.additive {
                        start + swing
                    } else {
                        start * (swing * 2.0 + 1.0)
                    };
                }
                let live = art.live();
                voice.live[k] = live;
                voice.runs[k] = Run {
                    env,
                    lfo,
                    value: if live {
                        combine(art.additive, start, value)
                    } else {
                        value
                    },
                };
            } else {
                value = art.mods(value, voice.places);
            }
            voice.notes[k] = value;
        }
        voice.release_on_env =
            flags[RELEASE_ON_ENV] != 0 && arts[0].env_on && state.arts[0].envelope.loop_end >= 0;
        voice.declick = if flags[DECLICK] == 0 { -1 } else { d.declick };
        if voice.declick > 0 {
            voice.ramp = 0.0;
            voice.ramp_step = (1.0 / f64::from(voice.declick)) as f32;
            voice.held = [0.0; 2];
        }
        voice.detune = (f64::from(p[15]) * voice.runs[4].value) as f32;
        let spread = detune_cents(voice.detune);
        let lean = 1.0 - f64::from(p[16] as f32 * 0.007_812_5) * voice.notes[5];
        let grown = (f64::from(lean as f32) * 101f64.ln()).exp();
        let phase_spread = powi(
            f64::from(1.0 - (grown - 1.0) as f32 * 0.01),
            engine.order as i32,
        );
        let unison_vol = p[14] as f32 * 0.007_812_5;

        let seed1 = if flags[KEY_SEED] != 0 {
            eng::mix(p[10] + key - 0x1f88, 0x1e42)
        } else {
            d.seeds[0]
        };
        let seed2 = if d.seeds[1] < 0 {
            let s = eng::mix(*counter - 0x200e, 0x1dbc);
            *counter = counter.wrapping_add(1);
            s
        } else {
            d.seeds[1]
        };
        let samples = |ms: f32| f64::from((f64::from(ms) * 0.001 * round(d.rate)) as f32);
        let decay = if d.decay >= 0.0 {
            let grown = (f64::from(d.decay) * 21f64.ln()).exp();
            let ms = f64::from((grown - 1.0) as f32) * 997.5 * f64::from(voice.notes[7] as f32);
            samples((ms + 50.0) as f32)
        } else {
            -1.0
        };
        let fullness = (f64::from(d.fullness) + voice.notes[9] * 2.0) as f32;
        voice.cents = (f64::from(pitch + d.cents as f32) + voice.runs[3].value * 1200.0) as f32;
        let ratio = (f64::from(voice.cents) * (1.0 / 1200.0) * LN_2).exp();
        let copies = p[12].clamp(1, 9) as usize;
        let setup = Setup {
            hop: d.hop,
            predecay: f64::from(d.predecay) * voice.notes[6],
            decay,
            fullness: f64::from(fullness.clamp(-1.0, 1.0)),
            seed1,
            seed2,
            morph: (f64::from(d.morph) * voice.notes[10]) as f32,
            ratio,
            copies,
            shift: (voice.notes[12] as f32).clamp(-0.5, 0.5),
            rotation: voice.notes[11] as f32,
            linear: quality(state.quality).0 == 1,
        };
        voice.timbre.start(engine, &setup);
        let root = if copies < 3 {
            1.0
        } else {
            (1.0 / (copies as f64).sqrt()) as f32
        };
        for i in 0..copies {
            if copies == 1 {
                voice.timbre.copies[i].detune = 0.0;
                voice.timbre.set_copy(i, 1.0, 0.0, 0.3, 0.3);
                continue;
            }
            let c = copies as f32;
            let phase = (i as f32 / c - ((c - 1.0) / c) * 0.5) * phase_spread as f32;
            let place = (i as f64 / (copies - 1) as f64) as f32 * 2.0 - 1.0;
            let mut lean = place.abs();
            let gain = if copies == 2 {
                0.3
            } else {
                (root * unison_vol + root * 2.0 * (1.0 - lean) * (1.0 - unison_vol)) * 0.3
            };
            let (l, r) = pan_law(place * 0.007_812_5 * p[13] as f32, gain);
            if i % 2 == 1 {
                lean = -lean;
            }
            if copies % 2 == 1 && copies < 2 * i {
                lean = -lean;
            }
            let detune = lean * DETUNE[i];
            let ratio = (f64::from(detune * (f64::from(spread) * (LN_2 / 1200.0)) as f32)).exp();
            voice.timbre.copies[i].detune = detune;
            voice.timbre.set_copy(i, ratio, f64::from(phase), l, r);
        }
    }

    /// Voice_Release (`FUN_00884450`): the timbre's release, unless the
    /// volume envelope's end lets it go, and the running envelopes'.
    fn release(&mut self, slot: usize) {
        let Autogun {
            d,
            arts,
            engine,
            work,
            voices,
            ..
        } = self;
        let voice = &mut voices[slot];
        if voice.state < 5 {
            voice.state = 5;
        }
        if !voice.release_on_env {
            let velocity = voice.release_velocity;
            release_timbre(arts, d, engine, work, voice, velocity);
        }
        for k in 0..RUN {
            let art = &arts[k];
            if art.env_on {
                art.envelope.release(&mut voice.runs[k].env, 1.0);
            }
        }
    }

    /// The voices and the post chain over one chunk into `self.left` and
    /// `self.right` (`Gen_Render` @ `0x00885600`).
    fn chunk(&mut self, len: usize) {
        self.left[..len].fill(0.0);
        self.right[..len].fill(0.0);
        let sounding = self.voices.iter().filter(|v| v.active).count();
        for slot in 0..self.voices.len() {
            if self.voices[slot].active {
                self.render_voice(slot, len);
            }
        }
        self.effects(sounding, len);
    }

    /// One voice over `len` frames into the sum (`FUN_00884890`).
    fn render_voice(&mut self, slot: usize, len: usize) {
        let Autogun {
            state,
            d,
            arts,
            engine,
            work,
            voices,
            vbuf,
            vbuf2,
            left,
            right,
            ..
        } = self;
        let p = &state.params;
        let voice = &mut voices[slot];
        if voice.state >= 10 {
            voice.active = false;
            return;
        }
        let mut done = 0;
        while done < len {
            if voice.countdown < 1 {
                tick(voice, arts, d, p, state.flag(MOD_SMOOTH) != 0);
                if voice.release_on_env && voice.runs[0].env.over() {
                    release_timbre(arts, d, engine, work, voice, 0.5);
                }
            }
            let cents =
                (f64::from(voice.pitch + d.cents as f32) + voice.runs[3].value * 1200.0) as f32;
            if cents != voice.cents {
                voice.cents = cents;
                let ratio = (f64::from(cents) * (1.0 / 1200.0) * LN_2).exp();
                voice.timbre.set_pitch(engine, ratio);
            }
            let detune = (f64::from(p[15]) * voice.runs[4].value) as f32;
            if detune != voice.detune {
                voice.detune = detune;
                let spread = f64::from(detune_cents(detune)) * (LN_2 / 1200.0);
                for copy in &mut voice.timbre.copies[..voice.timbre.count] {
                    copy.ratio = f64::from(copy.detune * spread as f32).exp();
                }
            }
            let n = (voice.countdown as usize).min(len - done).max(1);
            let frames = &mut vbuf[..2 * n];
            voice.timbre.render(engine, work, frames);
            if voice.timbre.quiet < 1 && voice.state < 9 {
                voice.state = 9;
            }
            if voice.declick > 0 {
                voice.declick -= n as i32;
                let m = if voice.declick < 0 {
                    (n as i32 + voice.declick).max(0) as usize
                } else {
                    n
                };
                let (mut c, step) = (voice.ramp, voice.ramp_step);
                let [mut hl, mut hr] = voice.held;
                for pair in frames[..2 * m].as_chunks_mut::<2>().0 {
                    hl += c * (pair[0] - hl);
                    pair[0] = hl;
                    hr += c * (pair[1] - hr);
                    pair[1] = hr;
                    c += step;
                }
                voice.held = [hl, hr];
                voice.ramp += step * m as f32;
            }
            if d.filter_last >= 0 {
                filter_coefficients(d, voice);
                let stages = (d.filter_last + 1) as usize;
                if d.doubled {
                    let up = &mut vbuf2[..4 * n];
                    for j in 0..n {
                        up[4 * j] = frames[2 * j];
                        up[4 * j + 1] = frames[2 * j + 1];
                        up[4 * j + 2] = frames[2 * j];
                        up[4 * j + 3] = frames[2 * j + 1];
                    }
                    for f in &mut voice.filters[..stages] {
                        f.run(up);
                    }
                    for j in 0..n {
                        frames[2 * j] = up[4 * j] * 0.5 + up[4 * j + 2] * 0.5;
                        frames[2 * j + 1] = up[4 * j + 1] * 0.5 + up[4 * j + 3] * 0.5;
                    }
                } else {
                    for f in &mut voice.filters[..stages] {
                        f.run(frames);
                    }
                }
            }
            let mut g = voice.gains;
            let s = voice.steps;
            let (out_l, out_r) = (&mut left[done..done + n], &mut right[done..done + n]);
            for j in 0..n {
                let (l, r) = (frames[2 * j], frames[2 * j + 1]);
                out_l[j] += l * g[0] + r * g[2];
                out_r[j] += r * g[1] + l * g[3];
                for k in 0..4 {
                    g[k] += s[k];
                }
            }
            for k in 0..4 {
                voice.gains[k] += voice.steps[k] * n as f32;
            }
            voice.first = false;
            voice.countdown -= n as i32;
            done += n;
        }
        if voice.state < 9 && voice.over < 9 {
            return;
        }
        let still = voice.gains.iter().chain(&voice.steps).all(|v| *v == 0.0);
        if voice.state < 10 && still {
            voice.state = 10;
        }
        if voice.state > 9 {
            voice.active = false;
        }
    }

    /// EQ, chorus, delay, reverb and magic dust on the sum.
    fn effects(&mut self, voices: usize, len: usize) {
        let d = &self.d;
        let fx = &mut *self.fx;
        let (left, right) = (&mut self.left[..len], &mut self.right[..len]);
        for b in 0..3 {
            if fx.band_on[b] {
                let band = &mut fx.bands[b];
                let mut pair = [0.0f32; 2];
                for j in 0..len {
                    pair[0] = left[j];
                    pair[1] = right[j];
                    band.run(&mut pair);
                    left[j] = pair[0];
                    right[j] = pair[1];
                }
            }
        }
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
                fx.delay.add(0, left, fx.delay_offsets[0]);
                fx.delay.add(1, right, fx.delay_offsets[1]);
            }
            let cap = (44_100.0 / d.rate * 0.001) as f32;
            fx.delay.render(fx.kernel, wet_l, wet_r, cap);
            for i in 0..len {
                left[i] += wet_l[i] * fx.delay_gain;
                right[i] += wet_r[i] * fx.delay_gain;
            }
        }
        if fx.reverb_on {
            let mono = &mut self.mono[..len];
            for i in 0..len {
                mono[i] = left[i] + right[i];
            }
            fx.reverb.process(mono, wet_l, wet_r);
            for i in 0..len {
                left[i] += wet_l[i] * fx.reverb_wet;
                right[i] += wet_r[i] * fx.reverb_wet;
            }
        }
        if let Some(sg) = &mut self.sg {
            sg.process(left, right);
        }
    }
}

/// How far the unison copies spread, in cents a unit of detune share:
/// `(11^(x / 256) - 1) * 5` (`FUN_008846e0`).
fn detune_cents(amount: f32) -> f32 {
    ((f64::from(amount * 0.003_906_25) * 11f64.ln()).exp() - 1.0) as f32 * 5.0
}

/// `FUN_00884200`: let the timbre go, once, over the release time the
/// release articulator gives the release velocity.
fn release_timbre(
    arts: &[Art],
    d: &Derived,
    engine: &mut Engine,
    work: &mut Work,
    voice: &mut Voice,
    velocity: f32,
) {
    if voice.let_go {
        return;
    }
    voice.let_go = true;
    let art = &arts[8];
    let value = art.mods(art.note(voice.note, velocity.clamp(0.0, 1.0)), voice.places);
    let grown = (f64::from(d.release) * 21f64.ln()).exp();
    let ms = f64::from((grown - 1.0) as f32) * 997.5 * f64::from(value as f32) + 50.0;
    let samples = f64::from((f64::from(ms as f32) * 0.001 * round(d.rate)) as f32);
    voice.timbre.release(engine, work, samples);
}

/// One control tick of a voice (`FUN_00884890`, its first part): the mod
/// wheels, the five running articulators and the gains' ramps.
fn tick(voice: &mut Voice, arts: &[Art], d: &Derived, p: &[i32; PARAMS], smooth: bool) {
    let dt = if voice.countdown < 0 {
        voice.countdown = -voice.countdown;
        0.0
    } else {
        voice.countdown = d.tick;
        1.0
    };
    voice.over = 9;
    let hosts = [voice.fcut, voice.fres];
    for m in 0..2 {
        let wheel = p[3 + m] as f32;
        voice.mods[m] = if smooth {
            wheel + (voice.mods[m] - wheel) * d.smooth
        } else {
            wheel
        };
        let at = f64::from(voice.mods[m] + hosts[m] * 256.0);
        voice.places[m] = round(at).clamp(0.0, 512.0) as usize;
    }
    for k in 0..RUN {
        let art = &arts[k];
        let run = &mut voice.runs[k];
        run.value = art.mods(voice.notes[k], voice.places);
        if !voice.live[k] {
            continue;
        }
        if art.env_on {
            let rate = d.rates[usize::from(art.envelope.tempo)];
            art.envelope.advance(&mut run.env, dt, rate);
            let e = f64::from(run.env.out) * f64::from(art.env_amount);
            run.value = if art.additive {
                run.value + e
            } else {
                run.value * e
            };
            if !run.env.over() {
                voice.over = voice.over.min(1);
            }
        }
        if let Some(lfo) = &mut run.lfo {
            let rate = d.rates[usize::from(art.lfo.tempo())];
            art.lfo.advance(lfo, dt, rate);
            let l = f64::from(lfo.out) * f64::from(art.lfo_amount);
            run.value = if art.additive {
                run.value + l
            } else {
                run.value * (l * 2.0 + 1.0)
            };
            voice.over = voice.over.min(1);
        }
    }
    let level = voice.volume * d.master;
    voice.target = if voice.state < 9 {
        stereo_pan(voice.pan, (f64::from(level) * voice.runs[0].value) as f32)
    } else {
        [0.0; 4]
    };
    if voice.first {
        voice.end = voice.target;
    }
    let cap = (0.001 * 44_100.0 / d.rate) as f32;
    for g in 0..4 {
        ramp(
            &mut voice.steps[g],
            &mut voice.end[g],
            &mut voice.gains[g],
            voice.target[g],
            cap,
            voice.countdown,
        );
    }
}

/// `FUN_0087fe50`: the voice filter's coefficients from the cutoff and
/// resonance wheels and articulators, on every stage: the cutoff on an
/// exponential curve, the resonance as a Q (low and high pass, rooted
/// over the stages) or a bandwidth in octaves (band pass and notch).
fn filter_coefficients(d: &Derived, voice: &mut Voice) {
    let cut = f64::from(d.cutoff) + voice.runs[1].value;
    let mut res = f64::from(d.resonance) + voice.runs[2].value;
    if d.filter_alt != 0 {
        res = res * (cut + 1.0) * 0.5;
    }
    let scale = f64::from(d.filter_scale) * TAU;
    let grow = |x: f64, base: f64| ((f64::from(x as f32) * f64::ln(base)).exp() - 1.0) as f32;
    let kind = [1, 2, 3, 4][usize::from(d.filter_kind)];
    let stages = (d.filter_last + 1).max(1) as usize;
    let clamp_w = |w: f64| w.clamp(0.0, 2.849_517_15);
    match d.filter_kind {
        0 | 3 => {
            let (factor, offset, top) = if d.filter_kind == 0 {
                (0.000_664_666_644, 0.003, f64::INFINITY)
            } else {
                (0.000_399_933_342, 0.0001, 0.6001)
            };
            let f = if cut > 0.0 {
                (f64::from(grow(cut, 1501.0)) * factor).min(top)
            } else {
                0.0
            };
            let mut q = if res <= 0.0 {
                0.1
            } else if res >= 1.0 {
                50.0
            } else {
                f64::from(grow(res, 2001.0)) * 0.024_950_001_4 + 0.1
            };
            if d.filter_last > 0 {
                q = f64::from((q as f32).powf(d.filter_root));
            }
            let q = 0.5 / q;
            let w = clamp_w((f + offset) * scale);
            let (sin, cos) = w.sin_cos();
            for f in &mut voice.filters[..stages] {
                f.design(kind, 0.0, sin * q, sin, cos);
            }
        }
        _ => {
            let f = if cut > 0.0 {
                (f64::from(grow(cut, 1501.0)) * 0.000_399_933_342).min(1.0001)
            } else {
                0.0
            };
            let bw = if res <= 0.0 {
                0.01
            } else if res >= 1.0 {
                12.0
            } else {
                f64::from(grow(res, 101.0)) * 0.119_899_996 + 0.01
            };
            let w = clamp_w((f + 0.0001) * scale);
            let (sin, cos) = w.sin_cos();
            let alpha = if sin > 0.0 {
                sin * (bw * f64::from(d.filter_width) * w / sin).sinh()
            } else {
                0.0
            };
            for f in &mut voice.filters[..stages] {
                f.design(kind, 0.0, alpha, sin, cos);
            }
        }
    }
}

impl Generator for Autogun {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.state = State::parse_as(bytes, self.kind)?;
        self.load();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz > 0.0 && f64::from(hz) != self.d.rate {
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
        if self.kind == Kind::Autogun {
            self.sg_state = std::fs::read(dir.join(SG_STATE)).ok();
            self.build_sg();
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        self.start(note);
    }

    fn note_off(&mut self, voice: u32) {
        for slot in 0..self.voices.len() {
            let v = &self.voices[slot];
            if v.active && v.id == voice && v.state < 5 {
                self.release(slot);
            }
        }
    }

    fn all_notes_off(&mut self) {
        for slot in 0..self.voices.len() {
            let v = &self.voices[slot];
            if v.active && v.state < 5 {
                self.release(slot);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let total = left.len().min(right.len());
        let mut done = 0;
        while done < total {
            let len = (total - done).min(CHUNK);
            self.chunk(len);
            for i in 0..len {
                left[done + i] += self.left[i];
                right[done + i] += self.right[i];
            }
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
        if self.state.params[index] != value {
            self.state.params[index] = value;
            self.derive();
        }
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.active = false;
        }
        let fx = &mut *self.fx;
        fx.chorus.fresh();
        fx.chorus_on = false;
        fx.delay.fresh();
        fx.delay_on = false;
        fx.reverb.fresh();
        fx.reverb_on = false;
        fx.sleep = 0;
        for band in &mut fx.bands {
            band.clear();
        }
        self.derive();
    }

    fn tail(&self) -> usize {
        let d = &self.d;
        // The strongest partials take four times the release to fall 40 dB,
        // and a voice ends 100 ms under -80 dB.
        let grown = (f64::from(d.release) * 21f64.ln()).exp();
        let release = ((grown - 1.0) * 997.5 + 50.0) * 0.001 * d.rate;
        let mut tail = release * 4.0 + 0.1 * d.rate + f64::from(d.hop) * 2.0;
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
        tail as usize + d.tick as usize
    }

    fn latency(&self) -> usize {
        self.sg.as_ref().map_or(0, |m| m.latency())
    }

    fn fidelity(&self) -> Fidelity {
        let s = &self.state;
        let ogun = self.kind == Kind::Ogun;
        let say =
            |autogun, ogun_text| Fidelity::Approximate(if ogun { ogun_text } else { autogun });
        if self.missing_sg {
            Fidelity::Approximate("Autogun without its magic dust state")
        } else if (2..=4).contains(&s.quality) {
            say(
                "Autogun quality other than normal",
                "Ogun quality other than normal",
            )
        } else if s.params[11] == 0 && s.params[9] > 0 {
            say(
                "Autogun timbre seed 2 drawn from a counter",
                "Ogun timbre seed 2 drawn from a counter",
            )
        } else if s.flag(PORTA) != 0 || s.flag(VEL_ENV) & 2 != 0 {
            say(
                "Autogun portamento or velocity envelope mode",
                "Ogun portamento or velocity envelope mode",
            )
        } else {
            say(
                "Autogun: timbre engine not null-tested",
                "Ogun: timbre engine not null-tested",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, projects};

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
            porta: false,
        }
    }

    /// A small engine, so that an unoptimised test build is quick.
    fn small() -> Autogun {
        let mut plugin = Autogun::default();
        plugin.state.order = 11;
        plugin.state.params[11] = 77;
        plugin.load();
        plugin.set_sample_rate(48_000.0);
        plugin
    }

    fn render(plugin: &mut Autogun, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        plugin.process(&mut l, &mut r);
        (l, r)
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn a_note_sounds_and_dies_away_after_its_release() {
        let mut plugin = small();
        plugin.note_on(note(60));
        let (l, r) = render(&mut plugin, 24_000);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
        let level = rms(&l[4800..]);
        assert!(level > 0.005 && level < 2.0, "{level}");
        plugin.note_off(1);
        let tail = plugin.tail();
        let (l, _) = render(&mut plugin, tail);
        assert!(rms(&l[l.len() - 4800..]) < 1e-4);
        assert!(plugin.voices.iter().all(|v| !v.active));
    }

    #[test]
    fn the_same_seed_makes_the_same_sound_and_another_another() {
        let mut a = small();
        let mut b = small();
        a.note_on(note(60));
        b.note_on(note(60));
        assert_eq!(render(&mut a, 4800), render(&mut b, 4800));
        let mut c = small();
        c.set_param(11, 78.0);
        c.note_on(note(60));
        assert_ne!(render(&mut a, 4800).0, render(&mut c, 4800).0);
    }

    #[test]
    fn an_old_block_is_a_front_of_the_new_one() {
        assert_eq!(block_rest(NEWEST), BLOCK - 12);
        assert_eq!(block_rest(13), BLOCK - 12);
        // Version 0 stops after the seventh switch.
        assert_eq!(12 + block_rest(0), 0xe3);
        // What a short state leaves is the constructor's, and the reset
        // switches are FL's.
        let block = default_block();
        assert_eq!(block[0xd8], 1);
        assert_eq!(&block[0xe3..0xe8], &[0, 0, 1, 0, 0]);
        assert_eq!(i32::from_le_bytes(block[0xfc..].try_into().unwrap()), 430);
    }

    #[test]
    fn an_old_curve_is_stretched_to_its_partials_and_read_on_a_log_axis() {
        // A ramp over the partials of order 11 (1023 of them), saved before
        // version 6: it rises to 1 at partial 1023 and holds.
        let mut shape = blank_shape(eng::IMPULSE);
        shape.points = vec![
            dsp::curve::Point::new(0.0, 0.0),
            dsp::curve::Point::new(1.0, 1.0),
        ];
        old_curve(&mut shape, 11);
        assert_eq!(shape.points.len(), 16);
        let curve = Curve::new(&shape.points);
        // Point i sits at i / 15 and holds the ramp at partial 2^i.
        for i in 0..16 {
            let want = (f64::from(1u32 << i) / 1023.0).min(1.0);
            let got = curve.value_at(f64::from(i) / 15.0);
            assert!((got - want).abs() < 1e-4, "{i}: {got} {want}");
        }
    }

    #[test]
    fn the_seed_wheels_mix_as_processparam_does() {
        assert_eq!(wheel_seed(0), -1);
        assert_eq!(wheel_seed(1201), eng::mix(1201 - 0x20cf, 0x1cfc));
    }

    #[test]
    fn the_pan_law_is_flat_in_the_middle_and_moves_the_far_side_over() {
        assert_eq!(stereo_pan(0.0, 0.8), [0.8, 0.8, 0.0, 0.0]);
        // Hard left: both sides into the left, 3 dB down each.
        let [l, r, rl, lr] = stereo_pan(-1.0, 1.0);
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert!((l - half).abs() < 1e-4 && r.abs() < 1e-6, "{l} {r}");
        assert!(lr == 0.0 && (rl - half).abs() < 1e-4, "{rl}");
    }

    #[test]
    fn a_ramp_is_no_steeper_than_its_cap() {
        let (mut step, mut end, mut now) = (0.0, 0.0, 0.0);
        ramp(&mut step, &mut end, &mut now, 1.0, 0.001, 100);
        assert!((step - 0.001).abs() < 1e-9 && (end - 0.1).abs() < 1e-6);
        ramp(&mut step, &mut end, &mut now, 1.0, 1.0, 100);
        assert!((end - 1.0).abs() < 1e-6 && (now - 0.1).abs() < 1e-6);
    }

    #[test]
    fn the_low_pass_passes_the_bottom_and_cuts_the_top() {
        let mut f = Biquad::default();
        let w = TAU * 1000.0 / 48_000.0;
        let (sin, cos) = w.sin_cos();
        f.design(1, 0.0, sin * (0.5 / 0.707), sin, cos);
        let [b0, b1, b2, a1, a2] = f.c;
        let gain = |hz: f64| crate::testing::response(&[b0, b1, b2], &[1.0, a1, a2], hz / 48_000.0);
        assert!((gain(50.0) - 1.0).abs() < 0.01);
        assert!(gain(10_000.0) < 0.02);
        // The peak and the shelves are flat at 0 dB.
        for kind in [6, 7, 8] {
            f.design(kind, 1.0, 0.5, sin, cos);
            let [b0, b1, b2, a1, a2] = f.c;
            let g = crate::testing::response(&[b0, b1, b2], &[1.0, a1, a2], 0.1);
            assert!((g - 1.0).abs() < 1e-9, "{kind} {g}");
        }
    }

    #[test]
    fn every_state_fl_ships_parses_to_the_last_byte() {
        let Some(root) = corpus() else { return };
        let mut saved = projects(&root);
        saved.extend(every_preset(&root, NAME));
        let ours: Vec<_> = saved
            .iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        assert!(ours.len() >= 10, "{}", ours.len());
        for s in ours {
            let state = State::parse(&s.state).unwrap_or_else(|e| panic!("{}: {e}", s.file));
            assert_eq!(state.arts.len(), ARTICULATORS);
            assert_eq!(state.quality, 1, "{}", s.file);
        }
    }

    /// The templates' impulse EQ is drawn in bins 1/64 of the curve apart:
    /// 512 partials of 0.511 Hz, so a C5 (MIDI 60) has its harmonics at
    /// 261.6 Hz, moved by the state's coarse pitch, and not between them.
    #[test]
    fn the_drawn_bins_are_the_harmonics_of_the_note() {
        let Some(root) = corpus() else { return };
        let saved = projects(&root);
        let breakbeat = |s: &&crate::testing::Saved| {
            s.file.contains("Breakbeat") && s.plugin.eq_ignore_ascii_case(NAME)
        };
        let Some(s) = saved.iter().find(breakbeat) else {
            panic!("no Breakbeat")
        };
        let mut plugin = Autogun::default();
        plugin.set_sample_rate(48_000.0);
        plugin.set_state(&s.state).unwrap();
        plugin.note_on(note(60));
        let (l, _) = render(&mut plugin, 48_000);
        // The state's coarse pitch moves the note.
        let hz = 261.626 * (f64::from(plugin.state.params[1]) / 12.0).exp2();
        let turns = |f: f64| f / 48_000.0;
        let on: f64 = (1..=4)
            .map(|h| crate::testing::partial(&l[24_000..], turns(hz * f64::from(h))))
            .sum();
        let off: f64 = (1..=4)
            .map(|h| crate::testing::partial(&l[24_000..], turns(hz * (f64::from(h) + 0.5))))
            .sum();
        eprintln!("harmonics {on:.5}, between {off:.5}");
        assert!(on > 4.0 * off, "{on} {off}");
    }

    #[test]
    fn every_project_state_plays_a_sane_note() {
        let Some(root) = corpus() else { return };
        let gens = root.join("../Plugins/Fruity/Generators");
        for s in projects(&root)
            .iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
        {
            let mut plugin = Autogun::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_state(&s.state).unwrap();
            plugin.set_resource_dir(&gens);
            assert!(!plugin.missing_sg, "{}", s.file);
            plugin.note_on(note(60));
            let (l, r) = render(&mut plugin, 24_000);
            assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{}", s.file);
            let level = rms(&l).max(rms(&r));
            let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
            eprintln!("{}: rms {level:.4} peak {peak:.4}", s.file);
            assert!(level > 0.003 && level < 2.0, "{}: {level}", s.file);
        }
    }
}
