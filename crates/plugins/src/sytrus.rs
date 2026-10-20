//! Sytrus, FL's FM synth, as `docs/plugins/sytrus.md` reads it out of
//! `Sytrus_x64.dll`: six operators with drawn harmonics and a shaped
//! waveform, an FM and a ring modulation matrix between them, three
//! filters of Love Philter's family, and an articulator on every control
//! with the envelope, the LFO and the six maps of `TOpArticulator`.
//!
//! The state is read as FL writes it, to the last byte, for every version
//! from 8 to 25. The sound is an approximation: the operators, the matrix,
//! the filters, the envelopes and the output levels follow what the voice
//! kernel does, with no band limiting of the wavetables, no unison
//! sub-voices, no plucked (Karplus-Strong) operators, no waveshaper on the
//! filters and no delay, chorus, reverb or EQ after the voices. A state
//! that asks for one of those says so in [`Generator::fidelity`], and
//! every state says the operators are approximate.
//!
//! A host moves FL's 597 REC parameters, numbered and ranged as
//! `ProcessParam` has them (section 6 of the doc): the main volume, LFO
//! level and pitch, the quick ADSR, Mod X and Mod Y, which the voices
//! glide to, the operators' and filters' wheels and every articulator's
//! knobs are heard; the unison, chorus, delay, EQ and portamento ones are
//! kept and read back.

// The kernels index their rows and operators on purpose: the workspace
// tests build unoptimised, and an index loop is what FL's kernel does.
#![allow(clippy::needless_range_loop)]

use crate::menvelope::{Class, Lfo, LfoRun, MEnvelope, Reader, Run, Track};
use crate::three_osc::pan_law;
use crate::{Fidelity, Generator, NoteOn, StateError, knob_gain};
use dsp::biquad::{Biquad64, Coefficients64};
use dsp::curve::{Curve, Point};
use std::f64::consts::{FRAC_PI_2, LN_2, PI, TAU};

pub const NAME: &str = "Sytrus";

/// How many notes sound at once.
pub const VOICES: usize = 32;

const OPERATORS: usize = 6;
const FILTERS: usize = 3;
const PARTS: usize = 10;
/// The matrix has a row for every operator and every filter.
const ROWS: usize = 9;
/// Columns 0 to 5 are the operators that modulate or feed the row, and
/// then the row's pan, its send to the effects and its output.
const PAN: usize = 6;
const SEND: usize = 7;
const OUT: usize = 8;
/// The most articulators a part has, and how many each kind has.
const ARTICULATORS: usize = 8;
const COUNTS: [usize; 3] = [6, 8, 2];
/// A wavetable's samples a cycle, and one more to read past the end.
const TABLE: usize = 2048;
/// The harmonics an operator draws.
const HARMONICS: usize = 256;
/// A Mod X or Mod Y map is baked into this many steps.
const MAP: usize = 512;
/// The most frames rendered at once.
const CHUNK: usize = 256;
/// What FL calls C5, the note at pitch 0, in Hz.
const C5: f64 = 523.251_130_601_197_3;
const PPQ: f64 = 96.0;
/// An FM cell at full level moves the carrier's phase by this many cycles
/// for a modulator at 1, and a cell on the diagonal, which is feedback, by
/// 35/128 of a cycle.
const FM_DEPTH: f32 = 4.0;
const SELF_FM: f32 = 35.0 / 128.0;
/// The ratio wheel's 20000 is the base pitch.
const RATIO_UNIT: f64 = 20_000.0;
const STAGES: usize = 3;
/// A note with no envelope fades out over this long after its release.
const FADE: f64 = 0.01;
/// The oldest state read: the operator block and the articulator stream
/// before version 8 are another format.
const OLDEST: i32 = 8;
const NEWEST: i32 = 25;

/// Filter types, as Love Philter numbers them. 12 is Sytrus's phaser.
const SVF: i32 = 0;
const LADDER_LOW: i32 = 1;
const LADDER_HIGH: i32 = 2;
const LOW: i32 = 3;
const BAND: i32 = 4;
const STOP: i32 = 5;
const HIGH: i32 = 6;
const ALL: i32 = 7;
const LOW_SHELF: i32 = 8;
const PEAK: i32 = 9;
const HIGH_SHELF: i32 = 10;
const COMB: i32 = 11;
const PHASER: i32 = 12;

/// FL's REC parameters: `ProcessParam` (`0x867930`) takes indexes under
/// 0x255, and its decoder (`0x868c90`) walks the parts' REC bases.
const PARAMS: usize = 597;
/// The first parameter of operator `p` is `20 + 51 p`: three wheels, then
/// six articulators of eight knobs.
const OP_PARAM: usize = 20;
const OP_STRIDE: usize = 51;
/// The filters' first parameters and their wheels: filter 3 has no "to
/// next" wheel.
const FILTER_PARAMS: [usize; FILTERS] = [326, 400, 474];
const FILTER_WHEELS: [usize; FILTERS] = [10, 10, 9];
/// The main part's: 9 wheels, three delay units of five, then two
/// articulators.
const MAIN_PARAM: usize = 547;
const MAIN_WHEELS: usize = 24;
/// From here the decoder reads `k = index - 547` as a late global: 40 to
/// 48 the EQ, 49 portamento.
const LATE_PARAM: usize = MAIN_PARAM + 40;

/// What a REC parameter is, as the decoder has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Param {
    /// One of the first 20, or a late global (40 to 49).
    Global(usize),
    /// Wheel `k` of a part.
    Wheel { part: usize, k: usize },
    /// Knob `j` of articulator `art` of a part.
    Knob { part: usize, art: usize, j: usize },
}

impl Param {
    fn of(index: usize) -> Option<Param> {
        if index >= PARAMS {
            return None;
        }
        if index < OP_PARAM {
            return Some(Param::Global(index));
        }
        if index >= LATE_PARAM {
            return Some(Param::Global(index - MAIN_PARAM));
        }
        let part = (0..PARTS).rev().find(|p| param_base(*p).0 <= index)?;
        let (base, wheels) = param_base(part);
        let k = index - base;
        Some(if k < wheels {
            Param::Wheel { part, k }
        } else {
            let (art, j) = ((k - wheels) / 8, (k - wheels) % 8);
            Param::Knob { part, art, j }
        })
    }

    /// The control's Min, Max and DefaultValue (the editor form's, which
    /// `0x867930` rescales a 0 to 1 by under REC_FromMIDI).
    fn control(self) -> (i32, i32, i32) {
        match self {
            Param::Global(k) => match k {
                0 => (0, 160, 128),
                1 => (-128, 128, 128),
                2 => (-24, 24, 0),
                3..=10 => match (k - 3) % 4 {
                    2 => (-128, 128, 0),
                    _ => (0, 256, 128),
                },
                11 => (1, 9, 1),
                12 => (0, 128, 128),
                13 => (16, 128, 100),
                14 => (0, 256, 128),
                15 => (0, 96, 0),
                16 => (0, 128, 16),
                17 => (0, 192, 0),
                18 | 19 => (0, 512, 256),
                40..=42 => (-1800, 1800, 0),
                43 => (0, 65_536, 8950),
                44 => (0, 65_536, 33_145),
                45 => (0, 65_536, 49_305),
                46..=48 => (0, 65_536, 17_500),
                _ => (0, 1, 0),
            },
            Param::Wheel { part, k } if part < OPERATORS => match k {
                0 => (0, 256, 0),
                1 => (-256, 256, 256),
                _ => (0, 4800, 1200),
            },
            Param::Wheel { part, k } if part < ROWS => match k {
                0 => (0, 5, 1),
                1..=3 => (0, 256, [128, 128, 0][k - 1]),
                4 => (0, 128, 128),
                5 | 6 => (0, 128, 0),
                7 => (64, 192, 128),
                8 => (0, 128, 0),
                _ => (-256, 256, 0),
            },
            Param::Wheel { k, .. } => match k {
                0 => (-128, 128, 0),
                1 => (0, 9, 4),
                2 => (0, 1024, 512),
                3 => (0, 1024, 768),
                4 => (0, 128, 8),
                5 => (0, 128, 128),
                6 => (-128, 128, -32),
                7 => (-128, 128, 128),
                8 => (0, 128, 128),
                _ => match (k - 9) % 5 {
                    0 => (0, 127, 64),
                    1 => (0, 384, 144),
                    2 => (-512, 512, 0),
                    3 => (-128, 128, 64),
                    _ => (0, 2, 0),
                },
            },
            Param::Knob { j, .. } => match j {
                0 | 1 | 3 => (0, 256, KNOBS[j]),
                2 | 5 | 6 => (-128, 128, KNOBS[j]),
                4 => (512, 65_536, KNOBS[j]),
                _ => (-112, 112, KNOBS[j]),
            },
        }
    }
}

/// A part's first REC parameter and how many wheels it has before its
/// articulators.
fn param_base(part: usize) -> (usize, usize) {
    match part {
        0..OPERATORS => (OP_PARAM + OP_STRIDE * part, 3),
        OPERATORS..ROWS => (
            FILTER_PARAMS[part - OPERATORS],
            FILTER_WHEELS[part - OPERATORS],
        ),
        _ => (MAIN_PARAM, MAIN_WHEELS),
    }
}

/// Where a REC parameter that is not an articulator knob keeps its value
/// in [`State::params`]: the index itself.
fn param_index(part: usize, k: usize) -> usize {
    param_base(part).0 + k
}

/// `(base^x - 1)` cut down to an f32, how FL's knob curves start.
fn curve(base: f64, x: f64) -> f64 {
    f64::from(((x * base.ln()).exp() - 1.0) as f32)
}

/// FL's power of two floats: 0 stays 0.
fn power(x: f64, y: f32) -> f64 {
    if x <= 0.0 {
        0.0
    } else {
        f64::from((f64::from(x as f32).ln() * f64::from(y)).exp() as f32)
    }
}

/// The level curve of a level cell or a wheel: `(11^x - 1) / 10` of its
/// size, 1 at 1 exactly, with its sign.
fn level(value: i32, scale: f32) -> f32 {
    let x = value as f32 / scale;
    let size = if x.abs() == 1.0 {
        1.0
    } else {
        knob_gain(f64::from(x.abs())) as f32
    };
    if value == 0 { 0.0 } else { size.copysign(x) }
}

/// The bend of a level from 0 to 1 by a tension `k`: the curve FL's
/// tension knobs share.
fn bend(x: f64, k: f64) -> f64 {
    if k == 0.0 {
        return x;
    }
    let (inverse, log) = (1.0 / k.abs(), (k.abs() + 1.0).ln());
    if k > 0.0 {
        1.0 - (((1.0 - x) * log).exp() - 1.0) * inverse
    } else {
        ((x * log).exp() - 1.0) * inverse
    }
}

/// What a tension or skew wheel's -128..128 comes to as a bend: `t = (21^a
/// - 1) / 20` and then `top^t - 1`, with the wheel's sign.
fn tension(wheel: i32, top: f64) -> f64 {
    if wheel == 0 {
        return 0.0;
    }
    let a = f64::from(wheel.unsigned_abs() as f32 / 128.0);
    let t = f64::from((curve(21.0, a) * 0.05) as f32);
    (f64::from(((t * top.ln()).exp() - 1.0) as f32)).copysign(f64::from(wheel))
}

/// An envelope of one point, switched off.
fn idle(kind: u32, level: f64, knobs: [i32; 4]) -> MEnvelope {
    MEnvelope {
        kind,
        version: 3,
        points: vec![Point::new(0.0, level)],
        arp: vec![0],
        flags: 0,
        active: false,
        decay: -1,
        loop_start: -1,
        loop_end: -1,
        knobs,
        phase: 0,
    }
}

/// Whether articulator `index` of a part of this kind adds to its control
/// (pan, pitch, phase, cutoff and resonance) and multiplies it otherwise.
fn additive(kind: usize, index: usize) -> bool {
    match kind {
        0 => matches!(index, 0 | 3 | 4),
        1 => matches!(index, 0 | 2 | 3),
        _ => index == 0,
    }
}

/// A new articulator's eight knobs.
const KNOBS: [i32; 8] = [128, 128, 0, 128, 40_000, 0, 0, 0];

/// The quick ADSR at rest.
const ADSR: [i32; 4] = [128, 128, 0, 128];

/// An envelope's attack, decay, sustain and release knobs with the main
/// panel's quick ADSR over them: the times scaled by `knob / 128`, the
/// sustain offset.
fn scaled(knobs: [i32; 4], adsr: [i32; 4]) -> [i32; 4] {
    if adsr == ADSR {
        return knobs;
    }
    let scale = |knob: i32, by: i32| (f64::from(knob) * f64::from(by) / 128.0) as i32;
    [
        scale(knobs[0], adsr[0]),
        scale(knobs[1], adsr[1]),
        knobs[2] + adsr[2],
        scale(knobs[3], adsr[3]),
    ]
}

/// One articulator: what moves one control of a part.
#[derive(Debug, Clone, PartialEq)]
struct Articulator {
    additive: bool,
    base: f32,
    amount: f32,
    lfo_amount: f32,
    envelope: Track,
    lfo: Lfo,
    /// The key, velocity and random maps, which are read once a note.
    key: Option<Curve>,
    velocity: Option<Curve>,
    random: Option<Curve>,
    /// The Mod X and Mod Y maps, baked into tables.
    mod_x: Option<Vec<f32>>,
    mod_y: Option<Vec<f32>>,
    /// The envelope's attack, decay, sustain and release knobs and the
    /// LFO's speed, tension, skew and pulse width, as the stream has them:
    /// the eight REC parameters of the articulator.
    knobs: [i32; 8],
}

/// Where one voice is in an [`Articulator`].
#[derive(Debug, Clone, Copy, PartialEq)]
struct ArtRun {
    run: Run,
    lfo: LfoRun,
    /// What the key, velocity and random maps came to for the note.
    fixed: f32,
    value: f32,
}

impl Articulator {
    fn new(additive: bool) -> Self {
        let centre = if additive { 0.5 } else { 0.0 };
        Articulator {
            additive,
            base: 0.0,
            amount: 1.0,
            lfo_amount: 1.0,
            envelope: Track::new(&idle(1, 1.0, crate::menvelope::ENVELOPE_KNOBS), centre),
            lfo: Lfo::new(&idle(2, 1.0, crate::menvelope::LFO_KNOBS)),
            key: None,
            velocity: None,
            random: None,
            mod_x: None,
            mod_y: None,
            knobs: KNOBS,
        }
    }

    fn centre(&self) -> f64 {
        if self.additive { 0.5 } else { 0.0 }
    }

    /// Read one `TOpArticulator`: its format, its three levels, the
    /// envelope, the LFO and the key, velocity, Mod X, Mod Y, random and
    /// unison maps. `adsr` scales the envelope's quick knobs, for the main
    /// panel's attack, decay, sustain and release.
    fn read(reader: &mut Reader<'_>, additive: bool, adsr: [i32; 4]) -> Result<Self, StateError> {
        let mut articulator = Articulator::new(additive);
        let centre = articulator.centre();
        let _format = reader.i32()?;
        articulator.base = reader.f32()?;
        articulator.amount = reader.f32()?;
        articulator.lfo_amount = reader.f32()?;
        let mut envelope = MEnvelope::read(reader, Class::Envelope)?;
        articulator.knobs[..4].copy_from_slice(&envelope.knobs);
        envelope.knobs = scaled(envelope.knobs, adsr);
        articulator.envelope = Track::new(&envelope, centre);
        let lfo = MEnvelope::read(reader, Class::Lfo)?;
        articulator.knobs[4..].copy_from_slice(&lfo.knobs);
        articulator.lfo = Lfo::new(&lfo);
        // A map nobody drew is a level line at the identity, and nothing
        // is kept for one.
        let identity = (1.0 - centre) as f32;
        let mut maps = Vec::with_capacity(6);
        for _ in 0..6 {
            let map = MEnvelope::read(reader, Class::Map)?;
            maps.push((map.flat() != Some(identity)).then_some(map));
        }
        let drawn = |map: &Option<MEnvelope>| map.as_ref().map(|m| Curve::new(&m.points));
        let table = |map: &Option<MEnvelope>| map.as_ref().map(|m| m.table(MAP, centre));
        articulator.key = drawn(&maps[0]);
        articulator.velocity = drawn(&maps[1]);
        articulator.mod_x = table(&maps[2]);
        articulator.mod_y = table(&maps[3]);
        articulator.random = drawn(&maps[4]);
        Ok(articulator)
    }

    /// Set knob `j` (REC order: envelope attack, decay, sustain, release,
    /// LFO speed, tension, skew, pulse width), with the main panel's quick
    /// ADSR `adsr` over the envelope's. This allocates nothing.
    fn set_knob(&mut self, j: usize, value: i32, adsr: [i32; 4]) {
        self.knobs[j] = value;
        self.rescale(adsr);
        if j >= 4 {
            let [_, _, _, _, speed, tension, skew, width] = self.knobs;
            self.lfo.set_knobs([speed, tension, skew, width]);
        }
    }

    /// Take the envelope knobs through the quick ADSR anew.
    fn rescale(&mut self, adsr: [i32; 4]) {
        let [attack, decay, sustain, release, ..] = self.knobs;
        self.envelope
            .set_knobs(scaled([attack, decay, sustain, release], adsr));
    }

    fn combine(&self, a: f32, b: f32) -> f32 {
        if self.additive { a + b } else { a * b }
    }

    /// Whether an envelope or an LFO moves the control while a note
    /// sounds.
    fn live(&self) -> bool {
        self.envelope.active() || self.lfo.active()
    }

    /// A run at the start of a note. `key`, `velocity` and `random` are
    /// where the note is in those maps, from 0 to 1.
    fn start(&self, key: f64, velocity: f64, random: f64, rates: [f64; 2]) -> ArtRun {
        let centre = self.centre();
        let mut fixed = if self.additive { 0.0 } else { 1.0 };
        for (map, at) in [
            (&self.key, key),
            (&self.velocity, velocity),
            (&self.random, random),
        ] {
            if let Some(curve) = map {
                fixed = self.combine(fixed, (curve.value_at(at) - centre) as f32);
            }
        }
        let rate = rates[usize::from(self.envelope.tempo)];
        let lfo_rate = rates[usize::from(self.lfo.tempo())];
        ArtRun {
            run: self.envelope.start(rate),
            lfo: self.lfo.start(lfo_rate),
            fixed,
            value: fixed,
        }
    }

    /// Work the value out for one control tick. `places` are where Mod X
    /// and Mod Y are in their tables.
    fn tick(&self, run: &mut ArtRun, places: [usize; 2], rates: [f64; 2]) {
        let mut value = run.fixed;
        if self.live() {
            let mut moving = self.base;
            if self.envelope.active() {
                let rate = rates[usize::from(self.envelope.tempo)];
                self.envelope.advance(&mut run.run, 1.0, rate);
                moving += run.run.out * self.amount;
            }
            if self.lfo.active() {
                let rate = rates[usize::from(self.lfo.tempo())];
                self.lfo.advance(&mut run.lfo, 1.0, rate);
                if self.additive {
                    moving += run.lfo.out * self.lfo_amount;
                } else {
                    moving *= run.lfo.out * self.lfo_amount * 2.0 + 1.0;
                }
            }
            value = self.combine(moving, run.fixed);
        }
        for (table, place) in [(&self.mod_x, places[0]), (&self.mod_y, places[1])] {
            if let Some(table) = table {
                value = self.combine(value, table[place.min(MAP)]);
            }
        }
        run.value = value;
    }

    fn release(&self, run: &mut ArtRun, rates: [f64; 2]) {
        if self.envelope.active() {
            let rate = rates[usize::from(self.envelope.tempo)];
            self.envelope.release(&mut run.run, rate);
        }
    }
}

/// One operator, as the state has it and as its wavetable comes out.
#[derive(Debug, Clone, PartialEq)]
struct Operator {
    /// The ratio wheel, in 1/20000ths of the base pitch, and the offset
    /// wheel in 1/100 Hz.
    ratio: i32,
    offset: i32,
    /// The noise wheel's level.
    noise: f32,
    /// The phase wheel, in cycles; the volume wheel, where 256 is 1; the
    /// pitch envelope's range in cents.
    phase: f32,
    volume: f32,
    range: f32,
    /// `TABLE + 1` samples of one cycle at peak 1, or nothing for an
    /// operator that draws no harmonic.
    table: Vec<f32>,
    free_run: bool,
    plucked: bool,
    /// The harmonics filter wheel, where 128 leaves every harmonic.
    filter: i32,
    articulators: Vec<Articulator>,
}

/// The shape of one cycle at phase `p` in half cycles (0 to 2): a sine
/// that the shape wheel takes to a triangle, a saw, a square and a pulse.
fn shape_at(p: f64, shape: f64) -> f64 {
    let pulse = (shape - 2.0).max(0.0);
    if pulse > 0.0 {
        return if p >= 1.0 - pulse * 0.75 { -1.0 } else { 1.0 };
    }
    let sine = (p * PI).sin();
    let mix = shape.clamp(0.0, 1.0);
    if mix <= 0.0 {
        return sine;
    }
    let rise = 1.0 - mix;
    let flat = (shape - 1.0).clamp(0.0, 1.0);
    let mut q = p + rise / 2.0;
    while q >= 2.0 {
        q -= 2.0;
    }
    let line = if q >= rise {
        let fall = 1.0 - (q - rise) * (2.0 / (2.0 - rise));
        if flat != 0.0 {
            let plateau = if q >= 1.0 { -flat } else { flat };
            plateau + fall * (1.0 - flat)
        } else {
            fall
        }
    } else {
        (q / rise) * 2.0 - 1.0
    };
    sine * (1.0 - mix) + line * mix
}

/// Build an operator's cycle from its harmonics and its shape wheels, as
/// `TSytrusShape` does: every drawn harmonic is a shaped wave at its
/// level and phase, the sum is brought to peak 1, and the tension wheel
/// bends it.
fn build_table(
    levels: &[f32],
    phases: &[f32],
    wheels: [i32; 5],
    half: [bool; 2],
    abs: bool,
) -> Vec<f32> {
    if levels.iter().all(|level| *level == 0.0) {
        return Vec::new();
    }
    let [shape, tension, skew, sine, _filter] = wheels;
    let shape = f64::from(shape as f32 / 128.0);
    let skew = tension_of(skew, 1e9);
    let sine = if sine > 0 {
        f64::from(sine) * 0.036_815_538_909_255_39
    } else {
        0.0
    };
    let mut table = vec![0f32; TABLE + 1];
    let mut period = 2.0;
    if half[0] {
        period = 1.0;
    }
    if half[1] {
        period *= 2.0;
    }
    for (h, level) in levels.iter().enumerate() {
        if *level == 0.0 {
            continue;
        }
        let mut step = f64::from((h + 1) as u32) * 2.0 / TABLE as f64;
        if half[0] {
            step /= 2.0;
        }
        if half[1] {
            step *= 2.0;
        }
        let mut p = f64::from(phases.get(h).copied().unwrap_or(0.0)) * 2.0;
        for sample in table.iter_mut().take(TABLE) {
            while p >= period {
                p -= period;
            }
            let mut value = 0.0;
            if p <= 2.0 {
                let mut at = p;
                if skew != 0.0 {
                    let side = if p <= 1.0 { 1.0 - p } else { p - 1.0 };
                    let bent = bend(side, skew);
                    at = if p <= 1.0 { 1.0 - bent } else { bent + 1.0 };
                }
                value = shape_at(at, shape);
                if sine > 0.0 {
                    value = (value * sine).sin();
                }
            }
            *sample += (value * f64::from(*level)) as f32;
            p += step;
        }
    }
    let peak = table.iter().fold(0f32, |peak, v| peak.max(v.abs()));
    if peak > 0.0 {
        for sample in &mut table {
            *sample /= peak;
        }
    }
    let tension = tension_of(tension, 1e18);
    if tension != 0.0 {
        for sample in &mut table {
            let bent = bend(f64::from(sample.abs()), tension) as f32;
            *sample = bent.copysign(*sample);
        }
    }
    if abs {
        for sample in &mut table {
            *sample = sample.abs();
        }
    }
    table[TABLE] = table[0];
    table
}

/// [`tension`] by another name, for a wheel.
fn tension_of(wheel: i32, top: f64) -> f64 {
    tension(wheel, top)
}

/// One filter, as the state has it.
#[derive(Debug, Clone, PartialEq)]
struct Filter {
    /// How many filters run in a row, and whether the earlier ones are
    /// tuned up.
    stages: usize,
    detune: bool,
    kind: i32,
    cutoff: f32,
    resonance: f32,
    /// The low, band and high (or gain) sliders' levels.
    levels: [f32; 3],
    /// The gain slider as a shelf's or a peak's gain.
    gain: f32,
    /// The waveshaper is on with some mix.
    shaper: bool,
    /// The waveshaper's switch, without the mix.
    shaper_on: bool,
    to_next: f32,
    articulators: Vec<Articulator>,
}

/// What one tick's cutoff and resonance come to for a filter's row.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Design {
    kind: i32,
    stages: usize,
    /// The SVF's corner and damping, or the ladder's corner and keep, a
    /// stage.
    a: [f32; STAGES],
    b: [f32; STAGES],
    coefficients: [Coefficients64; STAGES],
}

impl Design {
    const REST: Design = Design {
        kind: SVF,
        stages: 0,
        a: [0.0; STAGES],
        b: [1.0; STAGES],
        coefficients: [Coefficients64::IDENTITY; STAGES],
    };
}

impl Filter {
    /// Take wheel `k` of the header, as `0x869e80` does. The envelope
    /// wheel (1) is the cutoff articulator's amount, and the "to next"
    /// wheel (9) wants the rows routed anew; the waveshaper's pre-amp (7)
    /// is not built.
    fn set_wheel(&mut self, k: usize, value: i32) {
        match k {
            0 => {
                let order = value - 1;
                self.detune = order > 2;
                let last = if self.detune { order - 2 } else { order };
                self.stages = usize::try_from(last + 1).unwrap_or(0).min(STAGES);
            }
            2 => self.cutoff = value as f32 / 256.0,
            3 => self.resonance = value as f32 / 256.0,
            4..=6 => {
                self.levels[k - 4] = level(value, 128.0);
                if k == 4 {
                    let knob = (f64::from(value) * 0.015_625) as f32;
                    self.gain = (curve(8.0, f64::from(knob)) * 0.125 + 0.125) as f32;
                }
            }
            8 => self.shaper = self.shaper_on && value != 0,
            9 => self.to_next = level(value, 256.0),
            _ => {}
        }
    }

    /// Design the row for a cutoff and a resonance from 0 to 1, as Love
    /// Philter's `design` does (`docs/plugins/fruity-love-philter.md`).
    /// `scale` is `44100 / rate`.
    fn design(&self, cutoff: f32, resonance: f32, scale: f32) -> Design {
        let mut design = Design::REST;
        design.kind = self.kind;
        design.stages = self.stages;
        let mut stages = self.stages;
        let x = f64::from(cutoff);
        let mut r = f64::from(resonance);
        if matches!(self.kind, COMB | PHASER) {
            // The comb's resonance is its feedback and the phaser has
            // none: the low pass that stands in for them is one mild
            // stage.
            stages = stages.min(1);
            r = 0.5;
        }
        let scale = f64::from(scale);
        let tuning = if stages == 2 { 0.5 } else { 0.75 };
        let detune = self.detune;
        let detuned = |fc: f64| if detune { power(fc, tuning) } else { fc };
        let spread = if stages > 1 {
            (1.0 / stages as f64) as f32
        } else {
            1.0
        };
        match self.kind {
            SVF => {
                let mut fc = if x > 0.0 {
                    curve(51.0, x) * 0.015_900_000_929_832_46
                } else {
                    0.0
                };
                for stage in (0..stages).rev() {
                    let turn = ((fc + 0.005) * scale).min(0.625);
                    design.a[stage] = (turn.sin() * 2.0) as f32;
                    fc = detuned(fc);
                }
                let damping = if r <= 0.0 {
                    1.0
                } else if r < 1.0 {
                    1.0 - r * 0.899_999_976_158_142_1
                } else {
                    0.2
                };
                design.b[0] = damping as f32;
            }
            LADDER_LOW | LADDER_HIGH => {
                let mut fc = if x > 0.0 {
                    (curve(76.0, x) * 0.013_24).min(1.6)
                } else {
                    0.0
                };
                let mut q = if r <= 0.0 {
                    1.0
                } else if r < 1.0 {
                    curve(1001.0, f64::from(r as f32)) * 0.049 + 1.0
                } else {
                    50.0
                };
                if stages > 1 {
                    q = power(q, spread);
                }
                let scale = scale * FRAC_PI_2;
                for stage in (0..stages).rev() {
                    let cos = ((fc + 0.007) * scale).min(0.4 * PI).cos();
                    design.a[stage] = (2.0 - cos * 2.0) as f32;
                    let fall = cos - 1.0;
                    let keep = (-fall * fall * fall).sqrt() * f64::from(std::f32::consts::SQRT_2);
                    let keep = (keep / (q * fall) + 1.0) as f32;
                    design.b[stage] = if f64::from(keep) < 0.1 { 0.1 } else { keep };
                    fc = detuned(fc);
                }
            }
            _ => {
                // The comb and the phaser are not built: a low pass stands
                // in for them.
                let kind = if (LOW..=HIGH_SHELF).contains(&self.kind) {
                    self.kind
                } else {
                    LOW
                };
                let (reach, offset, most) = match kind {
                    LOW => (0.000_664_666_644_297_540_2, 0.003, f64::INFINITY),
                    HIGH => (0.000_399_933_342_123_404_15, 1e-4, 0.6001),
                    _ => (0.000_399_933_342_123_404_15, 1e-4, 1.0001),
                };
                let mut fc = if x > 0.0 {
                    (curve(1501.0, x) * reach).min(most)
                } else {
                    0.0
                };
                let shape = match kind {
                    BAND | STOP => {
                        if r <= 0.0 {
                            0.01
                        } else if r < 1.0 {
                            curve(101.0, f64::from(r as f32)) * 0.119_899_995_625_019_07 + 0.01
                        } else {
                            12.0
                        }
                    }
                    LOW_SHELF | HIGH_SHELF => {
                        let r = 1.0 - r;
                        if r <= 0.0 {
                            0.1
                        } else if r < 1.0 {
                            curve(3.0, f64::from(r as f32)) * 0.449_999_988_079_071_04 + 0.1
                        } else {
                            1.0
                        }
                    }
                    PEAK => {
                        let r = 1.0 - r;
                        if r <= 0.0 {
                            0.1
                        } else if r < 1.0 {
                            curve(2001.0, f64::from(r as f32)) * 0.049_949_999_898_672_104 + 0.1
                        } else {
                            100.0
                        }
                    }
                    _ => {
                        let q = if r <= 0.0 {
                            0.1
                        } else if r < 1.0 {
                            curve(2001.0, f64::from(r as f32)) * 0.024_950_001_388_788_223 + 0.1
                        } else {
                            50.0
                        };
                        if stages > 1 { power(q, spread) } else { q }
                    }
                };
                let mut a = f64::from(self.gain);
                if stages > 1 && kind >= LOW_SHELF {
                    a = (a.ln() * f64::from(spread)).exp();
                }
                let mut width = f64::from(stages.max(1) as u32);
                if kind == STOP {
                    width = 1.0 / width;
                }
                let width = width * (LN_2 / 2.0);
                let scale = scale * TAU;
                for stage in (0..stages).rev() {
                    let w = ((fc + offset) * scale).clamp(0.0, 2.849_517_146_113_191);
                    let (sin, cos) = w.sin_cos();
                    design.coefficients[stage] = match kind {
                        LOW => Coefficients64::low_pass(cos, sin * (0.5 / shape)),
                        HIGH => Coefficients64::high_pass(cos, sin * (0.5 / shape)),
                        ALL => Coefficients64::all_pass(cos, sin * (0.5 / shape)),
                        BAND => {
                            let alpha = sin * (shape * width * w / sin).sinh();
                            Coefficients64::band_pass(cos, alpha)
                        }
                        STOP => {
                            let alpha = sin * (shape * width * w / sin).sinh();
                            Coefficients64::notch(cos, alpha)
                        }
                        PEAK => Coefficients64::peak(cos, sin / (shape * 2.0), a),
                        _ => {
                            let beta =
                                a.sqrt() * ((a + 1.0 / a) * (1.0 / shape - 1.0) + 2.0).sqrt();
                            if kind == LOW_SHELF {
                                Coefficients64::low_shelf(cos, beta * sin, a)
                            } else {
                                Coefficients64::high_shelf(cos, beta * sin, a)
                            }
                        }
                    };
                    fc = detuned(fc);
                }
                design.kind = kind;
                design.stages = stages;
            }
        }
        design
    }
}

/// Everything a state comes to.
#[derive(Debug, Clone, PartialEq)]
struct State {
    version: i32,
    /// The main volume's gain and the main pitch in cents.
    main_volume: f32,
    main_pitch: f32,
    /// The unison order: 1 is one voice.
    unison: i32,
    /// The FX send level, as a gain.
    fx_send: f32,
    mod_x: i32,
    mod_y: i32,
    /// Mod X and Mod Y glide to their wheels (`ModSmoothBtn`, u8 0x440).
    mod_smooth: bool,
    /// The matrix as levels: `fm[carrier][modulator]` in cycles of the
    /// carrier for a modulator at 1, `rm[carrier][modulator]`, the
    /// operators' sends into each filter, and every row's pan (-0.5 to
    /// 0.5), send and output gains.
    fm: [[f32; OPERATORS]; OPERATORS],
    rm: [[f32; OPERATORS]; OPERATORS],
    to_filter: [[f32; OPERATORS]; FILTERS],
    pan: [f32; ROWS],
    send: [f32; ROWS],
    out: [f32; ROWS],
    operators: Vec<Operator>,
    filters: Vec<Filter>,
    main: Vec<Articulator>,
    /// A delay, the reverb, the chorus or the EQ is on.
    effects: bool,

    /// Which rows sound: they reach the output themselves or through a
    /// filter or a modulation.
    active: [bool; ROWS],
    /// Who each operator modulates, by FM and by RM: the carrier and the
    /// level.
    fm_targets: Vec<Vec<(usize, f32)>>,
    rm_targets: Vec<Vec<(usize, f32)>>,
    /// Who feeds each filter: the row and the level.
    filter_inputs: Vec<Vec<(usize, f32)>>,
    /// Every REC parameter but the articulator knobs, which the
    /// articulators keep, at its own index.
    params: [i32; PARAMS],
}

impl State {
    /// The blank template: one operator at the base pitch, out at full.
    fn blank() -> Self {
        let operator = |articulators| Operator {
            ratio: RATIO_UNIT as i32,
            offset: 0,
            noise: 0.0,
            phase: 0.0,
            volume: 1.0,
            range: 1200.0,
            table: Vec::new(),
            free_run: false,
            plucked: false,
            filter: 128,
            articulators,
        };
        let articulators = |kind: usize| -> Vec<Articulator> {
            (0..COUNTS[kind])
                .map(|index| Articulator::new(additive(kind, index)))
                .collect()
        };
        let mut levels = [0f32; HARMONICS];
        levels[0] = 1.0;
        let sine = build_table(
            &levels,
            &[0.0; HARMONICS],
            [0, 0, 0, 0, 128],
            [false; 2],
            false,
        );
        let mut operators: Vec<Operator> =
            (0..OPERATORS).map(|_| operator(articulators(0))).collect();
        for operator in &mut operators {
            operator.table = sine.clone();
        }
        let filters = (0..FILTERS)
            .map(|_| Filter {
                stages: 1,
                detune: false,
                kind: SVF,
                cutoff: 0.5,
                resonance: 0.0,
                levels: [1.0, 0.0, 0.0],
                gain: 8.0,
                shaper: false,
                shaper_on: false,
                to_next: 0.0,
                articulators: articulators(1),
            })
            .collect();
        let mut out = [0.0; ROWS];
        out[0] = 1.0;
        let mut state = State {
            version: NEWEST,
            main_volume: 1.0,
            main_pitch: 0.0,
            unison: 1,
            fx_send: 1.0,
            mod_x: 256,
            mod_y: 256,
            mod_smooth: true,
            fm: [[0.0; OPERATORS]; OPERATORS],
            rm: [[0.0; OPERATORS]; OPERATORS],
            to_filter: [[0.0; OPERATORS]; FILTERS],
            pan: [0.0; ROWS],
            send: [0.0; ROWS],
            out,
            operators,
            filters,
            main: articulators(2),
            effects: false,
            active: [false; ROWS],
            // Room for every target, so that routing anew allocates
            // nothing.
            fm_targets: (0..OPERATORS)
                .map(|_| Vec::with_capacity(OPERATORS))
                .collect(),
            rm_targets: (0..OPERATORS)
                .map(|_| Vec::with_capacity(OPERATORS))
                .collect(),
            filter_inputs: (0..FILTERS).map(|_| Vec::with_capacity(ROWS)).collect(),
            params: std::array::from_fn(|index| match Param::of(index) {
                Some(param @ (Param::Global(_) | Param::Wheel { .. })) => param.control().2,
                _ => 0,
            }),
        };
        state.route();
        state
    }

    /// Read a state: its version and the zlib stream after it.
    fn read(bytes: &[u8]) -> Result<Self, StateError> {
        let raw = crate::state_version(bytes, 8)?;
        if raw == i32::MAX || raw >= 0x1a_0004 {
            return Err(StateError::Version { found: raw });
        }
        let version = if raw >= 0x1_0000 { raw >> 16 } else { raw };
        if !(OLDEST..=NEWEST).contains(&version) {
            return Err(StateError::Version { found: version });
        }
        let body = miniz_oxide::inflate::decompress_to_vec_zlib(&bytes[4..]).map_err(|_| {
            StateError::Length {
                expected: bytes.len() + 1,
                found: bytes.len(),
            }
        })?;
        let mut reader = Reader::new(&body);
        let state = State::read_body(&mut reader, version)?;
        reader.end()?;
        Ok(state)
    }

    fn read_body(reader: &mut Reader<'_>, version: i32) -> Result<Self, StateError> {
        let mut state = State::blank();
        state.version = version;
        // The main block: the globals, the two matrices, the quick ADSRs,
        // the unison, the chorus and the Mod X / Mod Y wheels, then the
        // mute flags of the matrix cells.
        let globals = [reader.i32()?, reader.i32()?, reader.i32()?];
        let mut fm = [0i32; ROWS * ROWS];
        let mut rm = [0i32; ROWS * ROWS];
        for cell in &mut fm {
            *cell = reader.i32()?;
        }
        for cell in &mut rm {
            *cell = reader.i32()?;
        }
        let mut adsr = [0i32; 8];
        for knob in &mut adsr {
            *knob = reader.i32()?;
        }
        let mut unison = [0i32; 7];
        for knob in &mut unison {
            *knob = reader.i32()?;
        }
        let mut main = [0i32; 9];
        for knob in &mut main {
            *knob = reader.i32()?;
        }
        let _send_target = reader.i32()?;
        state.mod_x = reader.i32()?;
        state.mod_y = reader.i32()?;
        let _smoothing = reader.f32()?;
        reader.bytes(0x440 - 0x430)?;
        let flags = reader.bytes(4)?;
        state.mod_smooth = flags[0] != 0;
        reader.bytes(8)?;
        let mutes = reader.bytes(2 * ROWS * ROWS)?;
        for (cell, mute) in fm.iter_mut().zip(mutes) {
            if *mute != 0 {
                *cell = 0;
            }
        }
        for (cell, mute) in rm.iter_mut().zip(&mutes[ROWS * ROWS..]) {
            if *mute != 0 {
                *cell = 0;
            }
        }
        let params = &mut state.params;
        params[..3].copy_from_slice(&globals);
        params[3..11].copy_from_slice(&adsr);
        params[11..18].copy_from_slice(&unison);
        params[18] = state.mod_x;
        params[19] = state.mod_y;
        params[MAIN_PARAM..MAIN_PARAM + 9].copy_from_slice(&main);
        state.main_volume = level(globals[0], 128.0);
        state.main_pitch = globals[2] as f32 * 100.0;
        state.unison = unison[0];
        state.fx_send = level(main[8], 128.0);
        for row in 0..ROWS {
            let cell = |col: usize| fm[row * ROWS + col];
            state.pan[row] = cell(PAN) as f32 / 512.0;
            state.send[row] = level(cell(SEND), 256.0);
            state.out[row] = level(cell(OUT), 256.0);
            for col in 0..OPERATORS {
                if row < OPERATORS {
                    let depth = if row == col { SELF_FM } else { FM_DEPTH };
                    state.fm[row][col] = level(cell(col), 256.0) * depth;
                    state.rm[row][col] = level(rm[row * ROWS + col], 256.0);
                } else {
                    state.to_filter[row - OPERATORS][col] = level(cell(col), 256.0);
                }
            }
        }

        if version > 3 {
            reader.i32()?;
        }
        if version > 5 {
            reader.u8()?;
        }
        if version > 6 {
            reader.u8()?;
        }
        if version > 12 {
            reader.u8()?;
        }
        if version > 13 {
            let count = match version {
                14 => 1,
                15..=17 => 5,
                18 => 8,
                _ => 9,
            };
            let flags = reader.bytes(count)?;
            // 0x4f8, the portamento switch, is REC 596.
            if let Some(porta) = flags.get(1) {
                state.params[PARAMS - 1] = i32::from(*porta);
            }
        }
        let mut effects = false;
        for unit in 0..3 {
            let delay = reader.bytes(23)?;
            effects |= delay[20] != 0;
            for (j, word) in delay[..20].as_chunks::<4>().0.iter().enumerate() {
                state.params[param_index(9, 9 + 5 * unit + j)] = i32::from_le_bytes(*word);
            }
        }
        if version >= 10 {
            let reverb = reader.bytes(if version == 10 { 37 } else { 38 })?;
            effects |= reverb[36] != 0;
        }
        if version > 2 {
            let eq = reader.i32()?;
            effects |= eq != 0;
            let size = if version < 14 { 12 } else { 16 };
            let bands = reader.bytes(3 * size)?;
            // Level, frequency and width of each band: REC 587 + 3 field
            // + band.
            for band in 0..3 {
                for field in 0..3 {
                    let at = band * size + 4 * field;
                    let word = [bands[at], bands[at + 1], bands[at + 2], bands[at + 3]];
                    state.params[LATE_PARAM + 3 * field + band] = i32::from_le_bytes(word);
                }
            }
        }
        // The chorus is on with an order above 0.
        effects |= main[1] > 0;
        state.effects = effects;
        if version < 23 {
            let len = reader.i32()?;
            let len = usize::try_from(len).map_err(|_| StateError::Length {
                expected: reader.at,
                found: reader.bytes.len(),
            })?;
            reader.bytes(len)?;
        } else {
            let mut len = usize::from(reader.u8()?);
            if len == 0xff {
                len = reader.i32()?.try_into().map_err(|_| StateError::Length {
                    expected: reader.at,
                    found: reader.bytes.len(),
                })?;
            }
            reader.bytes(2 * len)?;
        }

        for part in 0..PARTS {
            let mut header = [0i32; 10];
            for word in &mut header {
                *word = reader.i32()?;
            }
            let kind = match part {
                0..OPERATORS => 0,
                OPERATORS..ROWS => 1,
                _ => 2,
            };
            // An operator's and a filter's wheels are the header's first
            // words; the main part's are at 0x3fc.
            if kind < 2 {
                let (base, wheels) = param_base(part);
                state.params[base..base + wheels].copy_from_slice(&header[..wheels]);
            }
            if kind == 0 {
                let operator = &mut state.operators[part];
                operator.phase = header[0] as f32 / 256.0;
                operator.volume = header[1] as f32 / 256.0;
                operator.range = header[2] as f32;
                let harmonics = if version < 17 { 128 } else { HARMONICS };
                operator.ratio = reader.i32()?;
                operator.offset = reader.i32()?;
                let noise = reader.i32()?;
                operator.noise = level(noise, 128.0).abs();
                let mut levels = vec![0f32; harmonics];
                let mut phases = vec![0f32; harmonics];
                for value in &mut levels {
                    *value = reader.f32()?;
                }
                for value in &mut phases {
                    *value = reader.f32()?;
                }
                let mut wheels = [0i32; 5];
                for wheel in &mut wheels {
                    *wheel = reader.i32()?;
                }
                let flags = reader.bytes(8)?;
                let half = [flags[0] != 0, flags[1] != 0];
                operator.free_run = flags[3] != 0;
                operator.plucked = flags[6] != 0;
                operator.filter = wheels[4];
                operator.table = build_table(&levels, &phases, wheels, half, flags[2] != 0);
            } else if kind == 1 {
                let index = part - OPERATORS;
                let filter = &mut state.filters[index];
                for k in [0, 2, 3, 4, 5, 6, 9] {
                    filter.set_wheel(k, header[k]);
                }
                let block = reader.bytes(if version < 9 { 0x29 } else { 0x30 })?;
                let word = |at: usize| {
                    block
                        .get(at..at + 4)
                        .map_or(0, |w| i32::from_le_bytes([w[0], w[1], w[2], w[3]]))
                };
                filter.kind = word(0x2c).clamp(0, PHASER);
                filter.shaper_on = word(0x28) != 0;
                filter.set_wheel(8, header[8]);
                let _shaper = MEnvelope::read(reader, Class::Map)?;
                if version >= 12 {
                    reader.u8()?;
                }
            }
            for index in 0..COUNTS[kind] {
                let articulator =
                    Articulator::read(reader, additive(kind, index), state.adsr(part, index))?;
                match kind {
                    0 => state.operators[part].articulators[index] = articulator,
                    1 => state.filters[part - OPERATORS].articulators[index] = articulator,
                    _ => state.main[index] = articulator,
                }
            }
            if kind == 1 {
                // The envelope wheel is the cutoff articulator's amount.
                let cutoff = &mut state.filters[part - OPERATORS].articulators[2];
                cutoff.amount = header[1] as f32 / 128.0;
            }
        }
        if version > 23 {
            reader.u8()?;
        }
        if version > 24 {
            reader.u8()?;
        }
        state.route();
        Ok(state)
    }

    /// The main panel's quick ADSR over the envelope of articulator `index`
    /// of `part`: group 0 (REC 3 to 6) over every volume articulator,
    /// group 1 (7 to 10) over the cutoff and resonance of filters 1 and 2.
    fn adsr(&self, part: usize, index: usize) -> [i32; 4] {
        let filter = (OPERATORS..OPERATORS + 2).contains(&part);
        let group = if filter && matches!(index, 2 | 3) {
            1
        } else if index == 1 {
            0
        } else {
            return ADSR;
        };
        let at = 3 + 4 * group;
        [
            self.params[at],
            self.params[at + 1],
            self.params[at + 2],
            self.params[at + 3],
        ]
    }

    /// Work out which rows sound and who feeds whom.
    fn route(&mut self) {
        let mut active = [false; ROWS];
        for row in 0..ROWS {
            active[row] = self.out[row] != 0.0 || self.send[row] != 0.0;
        }
        for f in (0..FILTERS).rev() {
            if f + 1 < FILTERS && active[OPERATORS + f + 1] && self.filters[f].to_next != 0.0 {
                active[OPERATORS + f] = true;
            }
        }
        for f in 0..FILTERS {
            if active[OPERATORS + f] {
                for op in 0..OPERATORS {
                    if self.to_filter[f][op] != 0.0 {
                        active[op] = true;
                    }
                }
            }
        }
        for _ in 0..OPERATORS {
            for carrier in 0..OPERATORS {
                if !active[carrier] {
                    continue;
                }
                for modulator in 0..OPERATORS {
                    if self.fm[carrier][modulator] != 0.0 || self.rm[carrier][modulator] != 0.0 {
                        active[modulator] = true;
                    }
                }
            }
        }
        for op in 0..OPERATORS {
            if self.operators[op].table.is_empty() && self.operators[op].noise == 0.0 {
                active[op] = false;
            }
        }
        self.active = active;
        for modulator in 0..OPERATORS {
            self.fm_targets[modulator].clear();
            self.rm_targets[modulator].clear();
            for carrier in 0..OPERATORS {
                if !active[carrier] {
                    continue;
                }
                if self.fm[carrier][modulator] != 0.0 {
                    self.fm_targets[modulator].push((carrier, self.fm[carrier][modulator]));
                }
                if self.rm[carrier][modulator] != 0.0 {
                    self.rm_targets[modulator].push((carrier, self.rm[carrier][modulator]));
                }
            }
        }
        for f in 0..FILTERS {
            self.filter_inputs[f].clear();
            for op in 0..OPERATORS {
                if active[op] && self.to_filter[f][op] != 0.0 {
                    self.filter_inputs[f].push((op, self.to_filter[f][op]));
                }
            }
            if f > 0 && active[OPERATORS + f - 1] && self.filters[f - 1].to_next != 0.0 {
                self.filter_inputs[f].push((OPERATORS + f - 1, self.filters[f - 1].to_next));
            }
        }
    }

    fn articulators(&self, part: usize) -> &[Articulator] {
        match part {
            0..OPERATORS => &self.operators[part].articulators,
            OPERATORS..ROWS => &self.filters[part - OPERATORS].articulators,
            _ => &self.main,
        }
    }

    fn articulators_mut(&mut self, part: usize) -> &mut [Articulator] {
        match part {
            0..OPERATORS => &mut self.operators[part].articulators,
            OPERATORS..ROWS => &mut self.filters[part - OPERATORS].articulators,
            _ => &mut self.main,
        }
    }

    /// What REC parameter `index` is at: what `0x867930` reads back.
    fn param(&self, index: usize) -> Option<i32> {
        Some(match Param::of(index)? {
            Param::Knob { part, art, j } => self.articulators(part).get(art)?.knobs[j],
            _ => self.params[index],
        })
    }

    /// Set REC parameter `index`, inside its control's range, and do what
    /// `0x867930` does after it stores one. Allocates nothing: routing
    /// anew pushes into lists that have room for every target.
    fn set_param(&mut self, index: usize, value: f32) {
        let Some(param) = Param::of(index) else {
            return;
        };
        let (min, max, _) = param.control();
        let value = crate::knob_value(value, (min, max));
        match param {
            Param::Knob { part, art, j } => {
                let adsr = self.adsr(part, art);
                if let Some(articulator) = self.articulators_mut(part).get_mut(art) {
                    articulator.set_knob(j, value, adsr);
                }
                return;
            }
            _ => self.params[index] = value,
        }
        match param {
            // `0x86a1d0`: the main volume and pitch, and the LFO level,
            // which every articulator takes as its LFO amount and a
            // filter's cutoff as its envelope amount times it.
            Param::Global(0) => self.main_volume = level(value, 128.0),
            Param::Global(1) => {
                let lfo = level(value, 128.0);
                for part in 0..PARTS {
                    for articulator in self.articulators_mut(part) {
                        articulator.lfo_amount = lfo;
                    }
                }
                for filter in &mut self.filters {
                    let cutoff = &mut filter.articulators[2];
                    cutoff.lfo_amount = cutoff.amount * lfo;
                }
            }
            Param::Global(2) => self.main_pitch = value as f32 * 100.0,
            // The quick ADSR scales the envelopes it is over.
            Param::Global(3..=10) => {
                for part in 0..PARTS {
                    for art in 0..self.articulators(part).len() {
                        let adsr = self.adsr(part, art);
                        self.articulators_mut(part)[art].rescale(adsr);
                    }
                }
            }
            Param::Global(11) => self.unison = value,
            // `0x86a3e0`; the voices glide to them a tick at a time.
            Param::Global(18) => self.mod_x = value,
            Param::Global(19) => self.mod_y = value,
            // `0x86a730` and `0x869e30`.
            Param::Wheel { part, k } if part < OPERATORS => {
                let operator = &mut self.operators[part];
                match k {
                    0 => operator.phase = value as f32 / 256.0,
                    1 => operator.volume = value as f32 / 256.0,
                    _ => operator.range = value as f32,
                }
            }
            // `0x869e80`, and `0x865120` routes the rows anew after the
            // "to next" wheel.
            Param::Wheel { part, k } if part < ROWS => {
                let lfo = level(self.params[1], 128.0);
                let filter = &mut self.filters[part - OPERATORS];
                filter.set_wheel(k, value);
                if k == 1 {
                    let cutoff = &mut filter.articulators[2];
                    cutoff.amount = value as f32 / 128.0;
                    cutoff.lfo_amount = cutoff.amount * lfo;
                }
                if k == 9 {
                    self.route();
                }
            }
            Param::Wheel { k: 8, .. } => self.fx_send = level(value, 128.0),
            // The unison, the chorus, the delays, the EQ and portamento
            // are kept but not built.
            _ => {}
        }
    }

    /// Whether any envelope of a sounding part is on.
    fn any_envelope(&self) -> bool {
        (0..PARTS).any(|part| {
            (part == ROWS || self.active[part])
                && self.articulators(part).iter().any(|a| a.envelope.active())
        })
    }
}

/// One operator as a voice runs it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OpRun {
    /// The phase in cycles, and what it moves by a sample.
    phase: f64,
    increment: f64,
    /// The pitch of the note for the operator, in cycles a sample, before
    /// its pitch articulator.
    pitch: f64,
    /// The FM input and the ring modulation product gathered for the next
    /// sample.
    fm: f32,
    rm: f32,
    /// The modulation level and the volume, and what they move by a
    /// sample.
    modulation: f32,
    modulation_step: f32,
    volume: f32,
    volume_step: f32,
    /// The phase articulator at the last tick.
    phase_art: f32,
}

/// One filter as a voice runs it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FilterRun {
    design: Design,
    /// The SVF's low and band, or the ladder's low and speed, a stage.
    state: [[f64; 2]; STAGES],
    biquad: [Biquad64; STAGES],
    gains: [f32; 3],
    volume: f32,
    volume_step: f32,
}

impl FilterRun {
    /// A row at rest.
    fn rest() -> Self {
        FilterRun {
            design: Design::REST,
            state: [[0.0; 2]; STAGES],
            biquad: [Biquad64::default(); STAGES],
            gains: [0.0; 3],
            volume: 0.0,
            volume_step: 0.0,
        }
    }

    /// Run one sample through the row.
    fn run(&mut self, x: f32) -> f32 {
        let design = &self.design;
        let mut sample = f64::from(x);
        match design.kind {
            SVF => {
                let gains = self.gains.map(f64::from);
                for stage in 0..design.stages {
                    let [mut low, mut band] = self.state[stage];
                    let f = f64::from(design.a[stage]);
                    let q = f64::from(design.b[stage]);
                    low += band * f;
                    let high = sample - (band * q + low);
                    band += high * f;
                    sample = low * gains[0] + band * gains[1] + high * gains[2];
                    self.state[stage] = [flushed(low), flushed(band)];
                }
            }
            LADDER_LOW | LADDER_HIGH => {
                let high = design.kind == LADDER_HIGH;
                for stage in 0..design.stages {
                    let [mut low, mut speed] = self.state[stage];
                    let g = f64::from(design.a[stage]);
                    let k = f64::from(design.b[stage]);
                    speed += (sample - low) * g;
                    low += speed;
                    speed *= k;
                    sample = if high { sample - low } else { low };
                    self.state[stage] = [flushed(low), flushed(speed)];
                }
            }
            _ => {
                for stage in 0..design.stages {
                    sample = self.biquad[stage].process(&design.coefficients[stage], sample);
                }
            }
        }
        sample as f32
    }
}

/// A filter's state, with a number too small to hold in full made 0.
fn flushed(state: f64) -> f64 {
    if state.is_subnormal() { 0.0 } else { state }
}

/// One sounding note.
#[derive(Debug, Clone, PartialEq)]
struct Voice {
    id: u32,
    /// The note's pitch in cents from C5, without and with the main pitch.
    key_pitch: f32,
    pitch: f32,
    velocity: f32,
    pan: f32,
    /// Mod X and Mod Y as they glide to the wheels, the note's own on top
    /// of them, and where the two come to in the tables.
    mods: [f32; 2],
    offsets: [f32; 2],
    places: [usize; 2],
    operators: [OpRun; OPERATORS],
    filters: [FilterRun; FILTERS],
    articulators: [ArtRun; PARTS * ARTICULATORS],
    /// Every row's gain into the left and the right, and the steps.
    gains: [[f32; 2]; ROWS],
    gain_steps: [[f32; 2]; ROWS],
    released: bool,
    /// The fade of a note with no envelope, from 1 to 0 after its release.
    fade: f32,
    over: bool,
    /// The next tick is the first.
    first: bool,
    ticked: bool,
    seed: u32,
}

/// The two rates an envelope can count by: half seconds and beats a
/// tick.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Timing {
    rate: f32,
    tick: f64,
    rates: [f64; 2],
    /// `44100 / rate`.
    scale: f32,
    /// What a fade loses a sample.
    fade_step: f32,
    /// What is left a tick of the way Mod X and Mod Y have to go: 1 % in
    /// 0.209 s (`0x8658b7`).
    mod_glide: f32,
}

/// Where a Mod X or Mod Y map is read: the wheel and the note's own, in
/// 512ths, to the nearest step.
fn place(wheel: f32, offset: f32) -> usize {
    (f64::from(wheel + offset * 256.0).round_ties_even()).clamp(0.0, MAP as f64) as usize
}

/// What an operator's phase moves by a sample at `pitch` cents from C5,
/// before its pitch articulator (`0x872210`).
fn op_pitch(operator: &Operator, pitch: f32, rate: f32) -> f64 {
    let base = C5 * (f64::from(pitch) / 1200.0).exp2() / f64::from(rate);
    base * f64::from(operator.ratio) / RATIO_UNIT
        + f64::from(operator.offset) / (100.0 * f64::from(rate))
}

impl Voice {
    fn new(note: &NoteOn, state: &State, timing: Timing, seed: &mut u32) -> Self {
        let key_pitch = f32::from(note.key - 60) * 100.0 + note.fine_pitch;
        let pitch = key_pitch + state.main_pitch;
        let key = f64::from((pitch / 100.0 + 60.0) / 120.0).clamp(0.0, 1.0);
        // FL's velocity of a note is the host's gain turned back into 0..1
        // of 128.
        let velocity = ((f64::from(note.velocity) * 10.0 + 1.0).ln() / 11f64.ln() * 100.0 / 128.0)
            .clamp(0.0, 1.0);
        *seed = seed.wrapping_mul(0x0808_8405).wrapping_add(1);
        let random = f64::from(*seed) / f64::from(u32::MAX);
        let mut articulators = [ArtRun {
            run: state.main[0].envelope.start(1.0),
            lfo: state.main[0].lfo.start(1.0),
            fixed: 0.0,
            value: 0.0,
        }; PARTS * ARTICULATORS];
        for part in 0..PARTS {
            for (index, articulator) in state.articulators(part).iter().enumerate() {
                articulators[part * ARTICULATORS + index] =
                    articulator.start(key, velocity, random, timing.rates);
            }
        }
        let mut operators = [OpRun {
            phase: 0.0,
            increment: 0.0,
            pitch: 0.0,
            fm: 0.0,
            rm: 0.0,
            modulation: 1.0,
            modulation_step: 0.0,
            volume: 0.0,
            volume_step: 0.0,
            phase_art: 0.0,
        }; OPERATORS];
        for (op, run) in operators.iter_mut().enumerate() {
            let operator = &state.operators[op];
            run.pitch = op_pitch(operator, pitch, timing.rate);
            run.phase = f64::from(operator.phase);
            if operator.free_run {
                *seed = seed.wrapping_mul(0x0808_8405).wrapping_add(1);
                run.phase += f64::from(*seed) / f64::from(u32::MAX);
            }
            run.phase -= run.phase.floor();
        }
        let mods = [state.mod_x as f32, state.mod_y as f32];
        let offsets = [note.mod_x, note.mod_y];
        Voice {
            id: note.voice,
            key_pitch,
            pitch,
            velocity: note.velocity,
            pan: note.pan,
            mods,
            offsets,
            places: [place(mods[0], offsets[0]), place(mods[1], offsets[1])],
            operators,
            filters: [FilterRun::rest(); FILTERS],
            articulators,
            gains: [[0.0; 2]; ROWS],
            gain_steps: [[0.0; 2]; ROWS],
            released: false,
            fade: 1.0,
            over: false,
            first: true,
            ticked: true,
            seed: *seed,
        }
    }

    fn release(&mut self, state: &State, timing: Timing) {
        if self.released {
            return;
        }
        self.released = true;
        for part in 0..PARTS {
            for (index, articulator) in state.articulators(part).iter().enumerate() {
                let run = &mut self.articulators[part * ARTICULATORS + index];
                articulator.release(run, timing.rates);
            }
        }
    }

    /// Start a control tick of `len` frames: move every articulator on and
    /// aim the levels at what they come to.
    fn tick(&mut self, state: &State, timing: Timing, len: usize) {
        let len = len.max(1) as f32;
        let snap = self.first;
        self.first = false;
        // The main pitch retunes the operators (`0x872290` calls `0x872210`
        // when the note's pitch and it come to another sum).
        let pitch = self.key_pitch + state.main_pitch;
        if pitch != self.pitch {
            self.pitch = pitch;
            for (run, operator) in self.operators.iter_mut().zip(&state.operators) {
                run.pitch = op_pitch(operator, pitch, timing.rate);
            }
        }
        // Mod X and Mod Y glide to the wheels a tick at a time, or jump
        // with smoothing off.
        for (i, wheel) in [state.mod_x, state.mod_y].into_iter().enumerate() {
            let wheel = wheel as f32;
            self.mods[i] = if state.mod_smooth {
                wheel + (self.mods[i] - wheel) * timing.mod_glide
            } else {
                wheel
            };
            self.places[i] = place(self.mods[i], self.offsets[i]);
        }
        for part in 0..PARTS {
            if part < ROWS && !state.active[part] {
                continue;
            }
            for (index, articulator) in state.articulators(part).iter().enumerate() {
                let run = &mut self.articulators[part * ARTICULATORS + index];
                articulator.tick(run, self.places, timing.rates);
            }
        }
        let value =
            |part: usize, index: usize| self.articulators[part * ARTICULATORS + index].value;
        for op in 0..OPERATORS {
            if !state.active[op] {
                continue;
            }
            let operator = &state.operators[op];
            let volume = value(op, 1) * operator.volume;
            let modulation = value(op, 2).clamp(0.0, 1.0);
            let factor =
                (f64::from(value(op, 3)) * 2.0 * f64::from(operator.range) / 1200.0).exp2();
            let phase_art = value(op, 4);
            let run = &mut self.operators[op];
            let delta = if snap {
                0.0
            } else {
                f64::from(phase_art - run.phase_art) / f64::from(len)
            };
            run.phase_art = phase_art;
            run.increment = (run.pitch * factor + delta).clamp(-0.5, 0.5);
            if snap {
                run.volume = volume;
                run.modulation = modulation;
                run.volume_step = 0.0;
                run.modulation_step = 0.0;
                run.rm = modulation;
            } else {
                run.volume_step = (volume - run.volume) / len;
                run.modulation_step = (modulation - run.modulation) / len;
            }
        }
        for f in 0..FILTERS {
            if !state.active[OPERATORS + f] {
                continue;
            }
            let part = OPERATORS + f;
            let filter = &state.filters[f];
            let cutoff = filter.cutoff + value(part, 2);
            let resonance = filter.resonance + value(part, 3);
            let run = &mut self.filters[f];
            run.design = filter.design(cutoff, resonance, timing.scale);
            for k in 0..3 {
                run.gains[k] = filter.levels[k] * value(part, 4 + k);
            }
            let volume = value(part, 1);
            if snap {
                run.volume = volume;
                run.volume_step = 0.0;
            } else {
                run.volume_step = (volume - run.volume) / len;
            }
        }
        for row in 0..ROWS {
            if !state.active[row] || (state.out[row] == 0.0 && state.send[row] == 0.0) {
                continue;
            }
            let pan = self.pan * 0.5 + 0.5 + state.pan[row] + value(row, 0);
            let pan = (pan * 2.0 - 1.0).clamp(-1.0, 1.0);
            // What goes to the effects comes back dry here, at half, for
            // the effects' own returns.
            let gain = self.velocity
                * state.main_volume
                * (state.out[row] + state.send[row] * state.fx_send * 0.5);
            let aim = pan_law(pan, gain);
            for side in 0..2 {
                if snap {
                    self.gains[row][side] = aim[side];
                    self.gain_steps[row][side] = 0.0;
                } else {
                    self.gain_steps[row][side] = (aim[side] - self.gains[row][side]) / len;
                }
            }
        }
        if self.released {
            let mut over = true;
            for part in 0..PARTS {
                if part < ROWS && !state.active[part] {
                    continue;
                }
                for (index, articulator) in state.articulators(part).iter().enumerate() {
                    let run = &self.articulators[part * ARTICULATORS + index];
                    if articulator.envelope.active() && !run.run.over() {
                        over = false;
                    }
                }
            }
            if state.any_envelope() {
                self.over = over;
            } else if self.fade <= 0.0 {
                self.over = true;
            }
        }
    }

    /// Render `len` frames into `scratch`, a block of [`CHUNK`] a row, and
    /// add them to the output.
    fn render(
        &mut self,
        state: &State,
        timing: Timing,
        scratch: &mut [f32],
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let len = left.len().min(right.len()).min(CHUNK);
        let fading = self.released && !state.any_envelope();
        for i in 0..len {
            for op in 0..OPERATORS {
                if !state.active[op] {
                    continue;
                }
                let operator = &state.operators[op];
                let run = self.operators[op];
                let x = run.phase + f64::from(run.fm * run.modulation);
                let mut y = 0.0;
                if !operator.table.is_empty() {
                    let pos = (x - x.floor()) * TABLE as f64;
                    let index = (pos as usize).min(TABLE - 1);
                    let frac = (pos - index as f64) as f32;
                    let table = &operator.table;
                    y = table[index] + (table[index + 1] - table[index]) * frac;
                }
                if operator.noise > 0.0 {
                    self.seed = self.seed.wrapping_mul(0x0808_8405).wrapping_add(1);
                    let white = (self.seed >> 8) as f32 * (1.0 / 8_388_608.0) - 1.0;
                    y = operator.noise * white + y * (1.0 - operator.noise);
                }
                let gain = run.rm + 1.0 - run.modulation;
                y *= gain;
                let out = y * run.volume;
                scratch[op * CHUNK + i] = out;
                let targets = &state.rm_targets[op];
                for k in 0..targets.len() {
                    let (carrier, depth) = targets[k];
                    self.operators[carrier].rm *= depth * y + 1.0 - depth.abs();
                }
                let targets = &state.fm_targets[op];
                for k in 0..targets.len() {
                    let (carrier, depth) = targets[k];
                    self.operators[carrier].fm += depth * out;
                }
                let run = &mut self.operators[op];
                run.fm = 0.0;
                run.rm = run.modulation;
                run.phase += run.increment;
                if run.phase >= 1.0 {
                    run.phase -= 1.0;
                } else if run.phase < 0.0 {
                    run.phase += 1.0;
                }
                run.volume += run.volume_step;
                run.modulation += run.modulation_step;
            }
        }
        for f in 0..FILTERS {
            let part = OPERATORS + f;
            if !state.active[part] {
                continue;
            }
            let inputs = &state.filter_inputs[f];
            let run = &mut self.filters[f];
            for i in 0..len {
                let mut x = 0.0;
                for k in 0..inputs.len() {
                    let (row, gain) = inputs[k];
                    x += scratch[row * CHUNK + i] * gain;
                }
                x *= run.volume;
                run.volume += run.volume_step;
                scratch[part * CHUNK + i] = run.run(x);
            }
        }
        let mut fade = self.fade;
        for row in 0..ROWS {
            if !state.active[row] || (state.out[row] == 0.0 && state.send[row] == 0.0) {
                continue;
            }
            let [mut l, mut r] = self.gains[row];
            let [ls, rs] = self.gain_steps[row];
            fade = self.fade;
            for i in 0..len {
                let sample = scratch[row * CHUNK + i] * fade;
                left[i] += sample * l;
                right[i] += sample * r;
                l += ls;
                r += rs;
                if fading {
                    fade = (fade - timing.fade_step).max(0.0);
                }
            }
            self.gains[row] = [l, r];
        }
        if fading {
            self.fade = fade;
        }
    }
}

/// The plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct Sytrus {
    state: State,
    rate: f32,
    tempo: f64,
    ppq: f64,
    voices: Vec<Voice>,
    to_tick: usize,
    tick_part: f64,
    scratch: Vec<f32>,
    seed: u32,
}

impl Default for Sytrus {
    fn default() -> Self {
        Sytrus {
            state: State::blank(),
            rate: 44_100.0,
            tempo: 120.0,
            ppq: PPQ,
            voices: Vec::with_capacity(VOICES),
            to_tick: 0,
            tick_part: 0.0,
            scratch: vec![0.0; ROWS * CHUNK],
            seed: 0x1234_5678,
        }
    }
}

impl Sytrus {
    fn tick_len(&self) -> f64 {
        (f64::from(self.rate) * 60.0 / (self.tempo * self.ppq)).max(1.0)
    }

    fn timing(&self) -> Timing {
        let tick = self.tick_len();
        let seconds = tick / f64::from(self.rate);
        Timing {
            rate: self.rate,
            tick,
            rates: [seconds * 2.0, seconds * self.tempo / 60.0],
            scale: (44_100.0 / f64::from(self.rate)) as f32,
            fade_step: (1.0 / (FADE * f64::from(self.rate))) as f32,
            mod_glide: 0.01f64.powf(seconds * 44_100.0 / (96.0 * 96.0)) as f32,
        }
    }
}

impl Generator for Sytrus {
    /// The notes that sound are cut: a new state routes the operators
    /// anew.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.state = State::read(bytes)?;
        self.voices.clear();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.rate = hz;
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm;
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        if ppq > 0 {
            self.ppq = f64::from(ppq);
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        if self.voices.len() == VOICES {
            self.voices.remove(0);
        }
        let timing = self.timing();
        let voice = Voice::new(&note, &self.state, timing, &mut self.seed);
        self.voices.push(voice);
    }

    fn note_off(&mut self, voice: u32) {
        let timing = self.timing();
        for sounding in self.voices.iter_mut().filter(|v| v.id == voice) {
            sounding.release(&self.state, timing);
        }
    }

    fn all_notes_off(&mut self) {
        let timing = self.timing();
        for sounding in &mut self.voices {
            sounding.release(&self.state, timing);
        }
    }

    /// The voice's next tick retunes its operators to it, as it does for
    /// the main pitch.
    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        for sounding in self.voices.iter_mut().filter(|v| v.id == voice) {
            sounding.key_pitch = pitch;
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let timing = self.timing();
        let mut at = 0;
        while at < len {
            if self.to_tick == 0 {
                self.tick_part += timing.tick;
                self.to_tick = (self.tick_part as usize).max(1);
                self.tick_part -= self.to_tick as f64;
                for voice in &mut self.voices {
                    voice.ticked = true;
                }
            }
            let run = (len - at).min(self.to_tick).min(CHUNK);
            let (state, scratch, to_tick) = (&self.state, &mut self.scratch[..], self.to_tick);
            let block = at..at + run;
            self.voices.retain_mut(|voice| {
                if voice.ticked {
                    voice.tick(state, timing, to_tick);
                    voice.ticked = false;
                }
                if voice.over {
                    return false;
                }
                let (l, r) = (&mut left[block.clone()], &mut right[block.clone()]);
                voice.render(state, timing, scratch, l, r);
                true
            });
            at += run;
            self.to_tick -= run;
        }
    }

    /// FL's 597 REC parameters, numbered as `0x868c90` numbers them.
    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.state.param(index).unwrap_or(0) as f32
    }

    /// The notes that sound hear it from their next tick.
    fn set_param(&mut self, index: usize, value: f32) {
        self.state.set_param(index, value);
    }

    fn reset(&mut self) {
        self.voices.clear();
        (self.to_tick, self.tick_part) = (0, 0.0);
    }

    /// The longest release among the envelopes that are on, and the fade
    /// of a note without one.
    fn tail(&self) -> usize {
        let timing = self.timing();
        let mut ticks = 1.0;
        for part in 0..PARTS {
            for articulator in self.state.articulators(part) {
                let track = &articulator.envelope;
                if track.active() {
                    let rate = timing.rates[usize::from(track.tempo)];
                    ticks = f64::max(ticks, track.release_length() / rate + 2.0);
                }
            }
        }
        let frames = ticks.min(1e6) * timing.tick;
        frames.ceil() as usize + (FADE * f64::from(self.rate)).ceil() as usize + 1
    }

    fn fidelity(&self) -> Fidelity {
        let state = &self.state;
        let active_ops = (0..OPERATORS).filter(|op| state.active[*op]);
        let active_filters = (0..FILTERS).filter(|f| state.active[OPERATORS + f]);
        if state.unison > 1 {
            Fidelity::Approximate("Sytrus unison")
        } else if active_ops.clone().any(|op| state.operators[op].plucked) {
            Fidelity::Approximate("Sytrus plucked operator")
        } else if active_filters.clone().any(|f| state.filters[f].shaper) {
            Fidelity::Approximate("Sytrus waveshaper")
        } else if active_filters
            .clone()
            .any(|f| matches!(state.filters[f].kind, COMB | PHASER))
        {
            Fidelity::Approximate("Sytrus comb or phaser filter")
        } else if state.effects && state.send.iter().any(|s| *s != 0.0) {
            Fidelity::Approximate("Sytrus effects")
        } else {
            Fidelity::Approximate("Sytrus operators")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, partial};

    const RATE: f32 = 48_000.0;

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
            color: 0,
            porta: false,
        }
    }

    /// A plugin with the blank template, at 48 kHz and 120 BPM.
    fn blank() -> Sytrus {
        let mut sytrus = Sytrus::default();
        sytrus.set_sample_rate(RATE);
        sytrus.set_tempo(120.0);
        sytrus
    }

    /// Play one note for `len` frames, in blocks of 512, and return the
    /// left side.
    fn play(sytrus: &mut Sytrus, key: i16, len: usize) -> Vec<f32> {
        sytrus.note_on(note(1, key));
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            sytrus.process(l, r);
        }
        left
    }

    fn rms(sound: &[f32]) -> f64 {
        let sum: f64 = sound.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
        (sum / sound.len() as f64).sqrt()
    }

    #[test]
    fn a_note_follows_the_pitch_the_host_moves_it_to() {
        crate::testing::follows_note_pitch(|| Box::new(Sytrus::default()), 44_100.0);
    }
    #[test]
    fn a_sine_operator_at_full_level_is_a_sine_at_the_note_s_frequency() {
        let mut sytrus = blank();
        let turns = f64::from(C5 as f32) / f64::from(RATE);
        let whole = (4096.0 * turns).round() / turns;
        let len = whole.round() as usize;
        let sound = play(&mut sytrus, 60, 8192 + len);
        let steady = &sound[8192..8192 + len];
        // Centred, so each side has 1 / sqrt 2 of the operator's 1.
        let at_pitch = partial(steady, turns);
        assert!((at_pitch - 0.707).abs() < 0.03, "{at_pitch}");
        assert!(partial(steady, turns * 2.0) < 0.01);
        assert!(partial(steady, turns * 3.0) < 0.01);
        // An octave up turns twice as fast.
        let mut sytrus = blank();
        let sound = play(&mut sytrus, 72, 8192 + len);
        assert!(partial(&sound[8192..8192 + len], turns * 2.0) > 0.68);

        // Let go, the note fades out in 10 ms and the voice is gone.
        sytrus.all_notes_off();
        let (mut left, mut right) = (vec![0.0; 2048], vec![0.0; 2048]);
        sytrus.process(&mut left, &mut right);
        assert!(left[..100].iter().any(|v| v.abs() > 0.1));
        assert!(left[1000..].iter().all(|v| *v == 0.0));
        assert!(sytrus.voices.is_empty());
        assert!(sytrus.tail() > 480);
    }

    #[test]
    fn the_rec_parameters_are_numbered_as_the_decoder_walks_the_parts() {
        let of = |index| Param::of(index).unwrap();
        assert_eq!(of(18), Param::Global(18));
        assert_eq!(of(19), Param::Global(19));
        // Operator 2's volume wheel, then its first articulator's attack.
        assert_eq!(of(72), Param::Wheel { part: 1, k: 1 });
        assert_eq!(
            of(74),
            Param::Knob {
                part: 1,
                art: 0,
                j: 0
            }
        );
        // The filters' cutoffs, and filter 2's last knob.
        assert_eq!(of(328), Param::Wheel { part: 6, k: 2 });
        assert_eq!(of(476), Param::Wheel { part: 8, k: 2 });
        assert_eq!(
            of(473),
            Param::Knob {
                part: 7,
                art: 7,
                j: 7
            }
        );
        // Filter 3 has no "to next" wheel.
        assert_eq!(
            of(483),
            Param::Knob {
                part: 8,
                art: 0,
                j: 0
            }
        );
        // The main part: chorus, delay unit 1, its articulators.
        assert_eq!(of(550), Param::Wheel { part: 9, k: 3 });
        assert_eq!(of(556), Param::Wheel { part: 9, k: 9 });
        assert_eq!(
            of(571),
            Param::Knob {
                part: 9,
                art: 0,
                j: 0
            }
        );
        assert_eq!(
            of(586),
            Param::Knob {
                part: 9,
                art: 1,
                j: 7
            }
        );
        // The late globals: the EQ and portamento.
        assert_eq!(of(587), Param::Global(40));
        assert_eq!(of(596), Param::Global(49));
        assert_eq!(Param::of(597), None);

        let mut sytrus = blank();
        assert_eq!(sytrus.param_count(), 597);
        let read: Vec<f32> = [0, 2, 13, 18, 72, 328, 556, 571, 575, 591]
            .iter()
            .map(|i| sytrus.param(*i))
            .collect();
        assert_eq!(
            read,
            [
                128.0, 0.0, 100.0, 256.0, 256.0, 128.0, 64.0, 128.0, 40_000.0, 33_145.0
            ]
        );
        // Each inside its control.
        for (index, value, kept) in [
            (18, 999.0, 512.0),
            (0, 999.0, 160.0),
            (2, -99.0, -24.0),
            (476, -5.0, 0.0),
            (326, 9.0, 5.0),
            (333, 0.0, 64.0),
            (575, 0.0, 512.0),
            (578, 999.0, 112.0),
            (560, 9.0, 2.0),
            (587, -9999.0, -1800.0),
            (596, 7.0, 1.0),
        ] {
            sytrus.set_param(index, value);
            assert_eq!(sytrus.param(index), kept, "{index}");
        }
        assert_eq!(sytrus.state.main[0].knobs[4], 512);
        assert_eq!(sytrus.state.main_pitch, -2400.0);
    }

    #[test]
    fn every_state_s_rec_parameters_read_back_what_it_holds() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Sytrus's parameters skipped");
            return;
        };
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        for saved in testing::projects(&root).into_iter().filter(ours) {
            let mut sytrus = Sytrus::default();
            if sytrus.set_state(&saved.state).is_err() {
                continue;
            }
            let state = sytrus.state.clone();
            // Setting each to what it reads changes nothing. The LFO level
            // and the filters' envelope wheels are left out: they set LFO
            // amounts, which the state keeps as FL last worked them out,
            // to a bit or so.
            let amounts = [1, 327, 401, 475];
            for index in (0..PARAMS).filter(|index| !amounts.contains(index)) {
                let value = sytrus.param(index);
                let (min, max, _) = Param::of(index).unwrap().control();
                if (min as f32..=max as f32).contains(&value) {
                    sytrus.set_param(index, value);
                    assert_eq!(sytrus.param(index), value, "{index}");
                }
            }
            assert_eq!(sytrus.state.mod_x, state.mod_x);
            assert_eq!(sytrus.state.filters, state.filters, "{}", saved.file);
            assert_eq!(sytrus.state.out, state.out);
            assert_eq!(sytrus.state.active, state.active);
        }
    }

    #[test]
    fn mod_x_glides_to_its_wheel_and_the_cutoff_and_main_pitch_are_heard() {
        let mut sytrus = blank();
        sytrus.note_on(note(1, 60));
        let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
        sytrus.process(&mut left, &mut right);
        assert_eq!(sytrus.voices[0].places, [256, 256]);
        // 1 % of the way is left 0.209 s on.
        sytrus.set_param(18, 0.0);
        let len = (0.209 * RATE) as usize;
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        sytrus.process(&mut left, &mut right);
        let left_over = sytrus.voices[0].mods[0];
        assert!((left_over - 2.56).abs() < 0.6, "{left_over}");
        // Without smoothing it jumps.
        sytrus.state.mod_smooth = false;
        sytrus.set_param(18, 512.0);
        sytrus.process(&mut vec![0.0; 512], &mut vec![0.0; 512]);
        assert_eq!(sytrus.voices[0].places[0], 512);

        // Operator 1 through filter 1, a low pass, to the output.
        let mut sytrus = blank();
        let state = &mut sytrus.state;
        state.out = [0.0; ROWS];
        state.out[OPERATORS] = 1.0;
        state.to_filter[0][0] = 1.0;
        state.filters[0].kind = LOW;
        state.route();
        sytrus.set_param(328, 256.0);
        let open = rms(&play(&mut sytrus, 72, 8192)[4096..]);
        sytrus.set_param(328, 0.0);
        let (mut left, mut right) = (vec![0.0; 8192], vec![0.0; 8192]);
        sytrus.process(&mut left, &mut right);
        let shut = rms(&left[4096..]);
        assert!(open > 0.3 && shut < open * 0.1, "{open} {shut}");

        // The main pitch retunes a note that sounds.
        let mut sytrus = blank();
        let turns = f64::from(C5 as f32) / f64::from(RATE);
        let whole = (4096.0 * turns * 2.0).round() / (turns * 2.0);
        let len = whole.round() as usize;
        play(&mut sytrus, 60, 4096);
        sytrus.set_param(2, 12.0);
        let (mut left, mut right) = (vec![0.0; 2048 + len], vec![0.0; 2048 + len]);
        sytrus.process(&mut left, &mut right);
        assert!(partial(&left[2048..], turns * 2.0) > 0.68);
    }

    #[test]
    fn a_modulator_at_index_0_leaves_the_carrier_alone_and_at_more_bends_it() {
        let alone = play(&mut blank(), 60, 8192);
        // Operator 2 modulates operator 1 by nothing.
        let mut sytrus = blank();
        sytrus.state.fm[0][1] = 0.0;
        sytrus.state.route();
        assert!(!sytrus.state.active[1]);
        let same = play(&mut sytrus, 60, 8192);
        assert_eq!(testing::bits(&alone), testing::bits(&same));

        // A modulator an octave up at an index of half a cycle takes the
        // carrier's own partial down and puts sidebands two octaves apart
        // around it.
        let mut sytrus = blank();
        sytrus.state.fm[0][1] = 0.5;
        sytrus.state.operators[1].ratio = 40_000;
        sytrus.state.route();
        assert!(sytrus.state.active[1]);
        assert_eq!(sytrus.state.fm_targets[1], vec![(0, 0.5)]);
        let bent = play(&mut sytrus, 60, 8192);
        let turns = f64::from(C5 as f32) / f64::from(RATE);
        let carrier = partial(&bent[4096..], turns);
        assert!(carrier < 0.6, "{carrier}");
        assert!(partial(&bent[4096..], turns * 3.0) > 0.05);
        assert!(bent.iter().all(|v| v.is_finite() && v.abs() < 1.0));
        // And it is the phase-modulated sine itself, to the sample window:
        // the same RMS as the ideal wave over the same frames.
        let ideal: f64 = (0..4096)
            .map(|n| {
                let t = turns * n as f64;
                (TAU * (t + 0.5 * (TAU * 2.0 * t).sin())).sin().powi(2)
            })
            .sum::<f64>();
        let ideal = (ideal / 4096.0).sqrt() * f64::from(std::f32::consts::FRAC_1_SQRT_2);
        assert!(
            (rms(&bent[4096..]) - ideal).abs() < 0.01,
            "{} {ideal}",
            rms(&bent[4096..])
        );

        // Ring modulation by a modulator at the same pitch is a cosine of
        // twice the pitch and a constant.
        let mut sytrus = blank();
        sytrus.state.rm[0][1] = 1.0;
        sytrus.state.route();
        let ring = play(&mut sytrus, 60, 8192);
        assert!(partial(&ring[4096..], turns) < 0.05);
        assert!(partial(&ring[4096..], turns * 2.0) > 0.3);
    }

    #[test]
    fn a_low_pass_at_a_cutoff_passes_a_tenth_of_it_and_cuts_ten_times_it() {
        // The corner of a Vanilla low pass is `(fc + 0.003) * 44100` Hz,
        // and this cutoff puts it at 1 kHz.
        let filter = Filter {
            stages: 1,
            detune: false,
            kind: LOW,
            cutoff: 0.0,
            resonance: 0.0,
            levels: [1.0, 0.0, 0.0],
            gain: 1.0,
            shaper: false,
            shaper_on: false,
            to_next: 0.0,
            articulators: Vec::new(),
        };
        let fc: f64 = 1000.0 / 44_100.0 - 0.003;
        let x = (fc / 0.000_664_666_644_297_540_2 + 1.0).ln() / 1501f64.ln();
        let gain = |hz: f64| {
            let mut run = FilterRun::rest();
            run.design = filter.design(x as f32, 0.5, 44_100.0 / RATE);
            let tone = testing::sine(hz / f64::from(RATE), 48_000);
            let out: Vec<f32> = tone.iter().map(|v| run.run(*v)).collect();
            rms(&out[24_000..]) / rms(&tone[24_000..])
        };
        assert!((gain(100.0) - 1.0).abs() < 0.1, "{}", gain(100.0));
        assert!(gain(10_000.0) < 0.05, "{}", gain(10_000.0));
        // The state variable filter does the same with its low output.
        let svf = Filter {
            kind: SVF,
            ..filter.clone()
        };
        let mut run = FilterRun::rest();
        run.gains = [1.0, 0.0, 0.0];
        run.design = svf.design(0.5, 0.0, 44_100.0 / RATE);
        let low = testing::sine(50.0 / f64::from(RATE), 48_000);
        let out: Vec<f32> = low.iter().map(|v| run.run(*v)).collect();
        assert!((rms(&out[24_000..]) / rms(&low[24_000..]) - 1.0).abs() < 0.1);
        let high = testing::sine(20_000.0 / f64::from(RATE), 48_000);
        let out: Vec<f32> = high.iter().map(|v| run.run(*v)).collect();
        assert!(rms(&out[24_000..]) / rms(&high[24_000..]) < 0.1);
    }

    #[test]
    fn a_volume_envelope_takes_its_beat_from_the_tempo() {
        // Up over one beat to full, then held.
        let points = [Point::new(0.0, 0.0), Point::new(1.0, 1.0)];
        let envelope = MEnvelope {
            points: points.to_vec(),
            arp: vec![0, 0],
            flags: 1,
            active: true,
            loop_end: 1,
            ..idle(1, 1.0, crate::menvelope::ENVELOPE_KNOBS)
        };
        let rise_at = |bpm: f64| {
            let mut sytrus = blank();
            sytrus.set_tempo(bpm);
            sytrus.state.operators[0].articulators[1].envelope = Track::new(&envelope, 0.0);
            assert!(sytrus.state.any_envelope());
            let sound = play(&mut sytrus, 60, 96_000);
            let level = |second: f64| rms(&sound[(second * 48_000.0) as usize..][..2400]);
            (level(0.25), level(0.75), level(1.5))
        };
        // At 120 BPM a beat is half a second: half way up at a quarter.
        let (quarter, after, late) = rise_at(120.0);
        assert!((quarter / late - 0.5).abs() < 0.08, "{quarter} {late}");
        assert!((after / late - 1.0).abs() < 0.05, "{after} {late}");
        // At 60 BPM the same beat takes a second.
        let (quarter, after, late) = rise_at(60.0);
        assert!((quarter / late - 0.25).abs() < 0.08, "{quarter} {late}");
        assert!((after / late - 0.75).abs() < 0.08, "{after} {late}");
    }

    #[test]
    fn the_wavetable_is_shaped_by_the_wheels() {
        let mut levels = [0f32; HARMONICS];
        levels[0] = 1.0;
        let none = [0.0; HARMONICS];
        let sine = build_table(&levels, &none, [0, 0, 0, 0, 128], [false; 2], false);
        assert_eq!(sine.len(), TABLE + 1);
        assert!((sine[TABLE / 4] - 1.0).abs() < 1e-5);
        assert!(sine[TABLE / 2].abs() < 1e-5);
        assert_eq!(sine[TABLE], sine[0]);
        // A full shape wheel is a square at 50%.
        let square = build_table(&levels, &none, [256, 0, 0, 0, 128], [false; 2], false);
        assert!(
            (square[TABLE / 4] - 1.0).abs() < 1e-5 && (square[3 * TABLE / 4] + 1.0).abs() < 1e-5
        );
        // Half way it has odd harmonics a sine has not.
        let tri = build_table(&levels, &none, [128, 0, 0, 0, 128], [false; 2], false);
        assert!(partial(&tri[..TABLE], 3.0 / TABLE as f64) > 0.02);
        assert!(partial(&sine[..TABLE], 3.0 / TABLE as f64) < 1e-4);
        // Abs folds it, and no harmonic is no table.
        let folded = build_table(&levels, &none, [0, 0, 0, 0, 128], [false; 2], true);
        assert!(folded.iter().all(|v| *v >= 0.0));
        assert!(build_table(&none, &none, [0, 0, 0, 0, 128], [false; 2], false).is_empty());
        // The second harmonic alone turns twice a cycle.
        let mut second = [0f32; HARMONICS];
        second[1] = 1.0;
        let table = build_table(&second, &none, [0, 0, 0, 0, 128], [false; 2], false);
        assert!(partial(&table[..TABLE], 2.0 / TABLE as f64) > 0.99);
    }

    #[test]
    fn a_state_that_is_cut_short_too_new_or_too_old_is_turned_down() {
        let mut sytrus = blank();
        assert!(matches!(
            sytrus.set_state(&[1, 2]),
            Err(StateError::Length { .. })
        ));
        let newer = 0x1a_0004i32.to_le_bytes();
        assert_eq!(
            sytrus.set_state(&newer),
            Err(StateError::Version { found: 0x1a_0004 })
        );
        let mut old = 7i32.to_le_bytes().to_vec();
        old.extend([0; 8]);
        assert_eq!(
            sytrus.set_state(&old),
            Err(StateError::Version { found: 7 })
        );
        // A version with nothing after it, or with bytes that are no zlib.
        let mut bare = 24i32.to_le_bytes().to_vec();
        assert!(matches!(
            sytrus.set_state(&bare),
            Err(StateError::Length { .. })
        ));
        bare.extend([1, 2, 3]);
        assert!(matches!(
            sytrus.set_state(&bare),
            Err(StateError::Length { .. })
        ));
        // A zlib stream of too few bytes.
        let short = miniz_oxide::deflate::compress_to_vec_zlib(&[0; 100], 6);
        let mut state = (24i32 << 16).to_le_bytes().to_vec();
        state.extend(short);
        assert!(matches!(
            sytrus.set_state(&state),
            Err(StateError::Length { .. })
        ));
        assert_eq!(sytrus.param(18), 256.0);
        assert_eq!(sytrus.fidelity(), Fidelity::Approximate("Sytrus operators"));
    }

    #[test]
    fn every_preset_from_version_8_and_every_state_in_fl_s_projects_loads() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Sytrus's states skipped");
            return;
        };
        let presets = testing::every_preset(&root, NAME);
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects: Vec<_> = testing::projects(&root).into_iter().filter(ours).collect();
        assert_eq!(projects.len(), 268);
        let (mut loaded, mut old, mut heard) = (0, 0, 0);
        let mut versions = std::collections::BTreeMap::new();
        let mut fidelity = std::collections::BTreeMap::new();
        for (from_project, saved) in presets
            .iter()
            .map(|s| (false, s))
            .chain(projects.iter().map(|s| (true, s)))
        {
            let mut sytrus = blank();
            let raw = i32::from_le_bytes(saved.state[..4].try_into().unwrap());
            let version = if raw >= 0x1_0000 { raw >> 16 } else { raw };
            match sytrus.set_state(&saved.state) {
                Ok(()) => {}
                Err(StateError::Version { found }) if found == version && found < OLDEST => {
                    assert!(!from_project, "{}", saved.file);
                    old += 1;
                    continue;
                }
                Err(error) => panic!("{} (version {version}): {error}", saved.file),
            }
            loaded += 1;
            *versions.entry(version).or_insert(0) += 1;
            let said = match sytrus.fidelity() {
                Fidelity::Exact => "exact",
                Fidelity::Approximate(what) => what,
            };
            *fidelity.entry(said).or_insert(0) += 1;
            // Every state plays: three notes on, a block, and off.
            for (id, key) in [(1, 48), (2, 60), (3, 67)] {
                sytrus.note_on(note(id, key));
            }
            let (mut left, mut right) = (vec![0.0; 2048], vec![0.0; 2048]);
            sytrus.process(&mut left, &mut right);
            sytrus.all_notes_off();
            sytrus.process(&mut left, &mut right);
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 32.0),
                "{}",
                saved.file
            );
            let peak = sound.fold(0f32, |peak, v| peak.max(v.abs()));
            heard += usize::from(peak > 1e-3);
            if saved.file == "Default" {
                assert!(peak > 0.1, "the blank template is silent");
            }
            assert!(sytrus.tail() > 0);
        }
        eprintln!(
            "Sytrus: {loaded} states loaded, {heard} of them heard in the first 2048 frames, \
             {old} presets older than version 8; {versions:?}; {fidelity:?}"
        );
        // The 268 states in the projects and every preset from version 8
        // on; the presets before it (229 in Sytrus's own directory, 252
        // across FL's files) are the old articulator stream.
        assert_eq!(loaded, presets.len() + 268 - old);
        assert!(old >= 229, "{old}");
        assert_eq!(fidelity.get("exact"), None);
    }
}
