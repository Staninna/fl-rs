//! Fruity Love Philter: eight filter units in a chain, each with a filter,
//! a waveshaper and a pan and volume stage, and eight articulators a unit
//! that move its controls, as `docs/plugins/fruity-love-philter.md` and
//! `Fruity Love Philter_x64.dll` have it.
//!
//! # The state
//!
//! An i32 version, which is 12 in the high half of `0xc0003` or a plain 2 to
//! 11, and then a zlib stream of:
//!
//! - six i32: master level, master LFO level, follower level, attack and
//!   release, and the pattern that plays, 1 to 10;
//! - 31 bytes, 32 in version 10 and 33 from 11 on: i32 Modulation X and Y,
//!   an i32 that is 0, an f32, i32 solo, i32 HQ envelopes, i32 oversampling,
//!   and the bytes smooth, centre and step, then one for the window from
//!   version 10 on and one for MIDI from 11 on;
//! - a byte for "use unit 1 for the follower" and an i32 the editor keeps;
//! - the comment: an i32 length and bytes before version 6, and after it a
//!   byte length, with `0xff` and a u32 for a long one, of UTF-16 units;
//! - the eight units: twelve i32 parameters, the bytes mode, 0, 0, 0, an
//!   i32 type, the bytes alternate, enabled and waveshaper on, the
//!   waveshaper's curve, a byte for a bipolar curve, and eight articulators,
//!   each an i32, three f32 and fourteen envelopes: ten patterns, the LFO
//!   and the maps of Modulation X, Modulation Y and the follower
//!   ([`crate::menvelope`] has the stream);
//! - from version 12 on, a byte for the HQ waveshaper.
//!
//! # The parameters
//!
//! FL numbers 359 of them. 0 to 7 are the master level (0 to 160), the
//! master LFO level (-128 to 128), the follower's level (0 to 160), attack
//! and release (0 to 16384), the pattern (1 to 10) and Modulation X and Y
//! (0 to 1024). Every unit then has its twelve and, for each of its eight
//! articulators, the LFO's speed, tension, skew and pulse width. The last
//! unit has no "to next", so it has eleven.
//!
//! | unit parameter | range | default |
//! |---|---|---|
//! | 0 Envelope amount | 0 to 256 | 128 |
//! | 1 Cutoff | 0 to 256 | 128 |
//! | 2 Resonance | 0 to 256 | 128 |
//! | 3 Low, gain or drive | 0 to 128 | 128 |
//! | 4 Band, 5 High | 0 to 128 | 0 |
//! | 6 Waveshaper pre amp | 64 to 192 | 128 |
//! | 7 Waveshaper mix | 0 to 128 | 128 |
//! | 8 Pan | -128 to 128 | 0 |
//! | 9 Volume | 0 to 160 | 128 |
//! | 10 Input volume | -160 to 160 | 128 |
//! | 11 To next | -160 to 160 | 0 |
//!
//! # The sound
//!
//! The plugin runs oversampled: the oversampling number is how many times
//! the rate is doubled, and FL starts it at 1. Each doubling is a half band
//! filter of two chains of three allpasses, up on the way in and down on
//! the way out. The filters' corners are worked out for 44.1 kHz and scaled
//! by `44100 / rate`, with the rate the oversampled one.
//!
//! A control tick is 64 samples at 44.1 kHz and 8 with HQ envelopes, to the
//! nearest 4 at another rate. Every tick the articulators give a value: an
//! adding one (pan, cutoff, resonance) starts at 0 and adds its maps, its
//! pattern and its LFO, counted from a half, and a multiplying one (volume,
//! the three gains, waveshaper mix) starts at 1. The patterns and LFOs with
//! the tempo flag count in beats and the others in half seconds.
//!
//! A unit hears the plugin's input at its input volume and the unit
//! before it at that one's "to next". Its filter is one of twelve:
//! Chocolate is a state variable filter whose low, band and high outputs
//! are mixed, Lime a resonant low or high pass, Vanilla the eight biquads
//! of the cookbook and Mango a four pole ladder with drive. The mode is how
//! many of them run in a row, and modes 4 and 5 are two and three with the
//! earlier ones tuned up. The waveshaper follows, and the unit's output is
//! panned into the mix.
//!
//! What this does not do as FL does, and [`Effect::fidelity`] says when a
//! state asks for the first:
//!
//! - What [`MEnvelope::plain`] leaves out of an envelope.
//! - FL lines the patterns up with the song's position when it plays. Here
//!   they start with the first block and after every [`Effect::reset`],
//!   which is the same thing for a song played from its start.
//! - FL doubles the oversampling again while it renders to a file, and its
//!   waveshapers read between their steps then, as they do when the state
//!   of a version 12 says so. This goes by the state.
//! - Modulation X and Y start where the state has them, and FL's glide up
//!   to there from 0 in the first tenth of a second.

use crate::menvelope::{Class, Lfo, LfoRun, MEnvelope, Reader, Run, Track};
use crate::parts::{Follower, glide, stereo_gains};
use crate::{Effect, Fidelity, StateError, knob_gain, knob_value};
use dsp::biquad::{Biquad64, Coefficients64};
use dsp::curve::Point;
use dsp::halfband::HalfBand;
use std::f64::consts::{FRAC_PI_2, LN_2, PI, SQRT_2, TAU};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Love Philter";

const UNITS: usize = 8;
const ARTICULATORS: usize = 8;
const PATTERNS: usize = 10;

/// The newest version there is, and the most its zlib stream may come to.
const NEWEST: i32 = 12;
const ROOM: usize = 16 << 20;

/// The ranges of the eight parameters the plugin has itself.
const GLOBALS: [(i32, i32); 8] = [
    (0, 160),
    (-128, 128),
    (0, 160),
    (0, 16_384),
    (0, 16_384),
    (1, 10),
    (0, 1024),
    (0, 1024),
];
const GLOBAL_DEFAULTS: [i32; 8] = [128, 128, 128, 4096, 8192, 1, 512, 512];

/// The ranges of a unit's twelve.
const UNIT: [(i32, i32); 12] = [
    (0, 256),
    (0, 256),
    (0, 256),
    (0, 128),
    (0, 128),
    (0, 128),
    (64, 192),
    (0, 128),
    (-128, 128),
    (0, 160),
    (-160, 160),
    (-160, 160),
];
const UNIT_DEFAULTS: [i32; 12] = [128, 128, 128, 128, 0, 0, 128, 128, 0, 128, 128, 0];

/// The ranges of an LFO's speed, tension, skew and pulse width.
const LFO: [(i32, i32); 4] = [(512, 65_536), (-128, 128), (-128, 128), (-112, 112)];

/// How many parameters a unit has with its articulators' LFOs.
const PER_UNIT: usize = UNIT.len() + ARTICULATORS * LFO.len();
const PARAMS: usize = GLOBALS.len() + UNITS * PER_UNIT - 1;

/// The articulators of a unit, in the order the state has them.
const PAN: usize = 0;
const VOLUME: usize = 1;
const CUTOFF: usize = 2;
const RESONANCE: usize = 3;
const LOW: usize = 4;
const SHAPER_MIX: usize = 7;

/// The filter types.
const CHOCOLATE: i32 = 0;
const LIME_LOW: i32 = 1;
const LIME_HIGH: i32 = 2;
const VANILLA_LOW: i32 = 3;
const VANILLA_BAND: i32 = 4;
const VANILLA_STOP: i32 = 5;
const VANILLA_HIGH: i32 = 6;
const VANILLA_ALL: i32 = 7;
const VANILLA_LOW_SHELF: i32 = 8;
const VANILLA_PEAK: i32 = 9;
const VANILLA_HIGH_SHELF: i32 = 10;
const MANGO: i32 = 11;

/// The most filters a unit runs in a row.
const STAGES: usize = 3;

/// How many steps a map's table has, and a unipolar waveshaper's.
const MAP: usize = 1024;
const SHAPER: usize = 0x4000;

/// The most frames [`LovePhilter::process`] takes through at once, and the
/// most times the rate is doubled.
const CHUNK: usize = 256;
const DOUBLINGS: usize = 4;

/// What Center gives its follower, as Fruity WaveShaper's does.
const CENTER_PACE: f64 = 1.007_023_499_999_999_8;
const CENTER_KEEP: f64 = 0.96;

/// `(base^x - 1)` cut down to an f32, which is how FL's knob curves start.
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

/// The level curve of the plugin's gain knobs, which is 1 at 1 exactly.
fn level(x: f32) -> f32 {
    if x == 1.0 {
        1.0
    } else {
        knob_gain(f64::from(x)) as f32
    }
}

/// The same for a knob that goes below 0, which turns the phase over.
fn signed_level(value: i32) -> f32 {
    let x = value as f32 / 128.0;
    level(x.abs()) * value.signum() as f32
}

/// One articulator: what moves one control of a unit.
#[derive(Debug, Clone, PartialEq)]
struct Articulator {
    /// It adds what it has to its control, and multiplies otherwise.
    additive: bool,
    base: f32,
    /// How much of its pattern it gives, and of its LFO.
    amount: f32,
    lfo_amount: f32,
    patterns: Vec<Track>,
    /// Whether every pattern runs as FL's does.
    plain: bool,
    lfo: Lfo,
    lfo_knobs: [i32; 4],
    /// The tables of the three maps that are drawn.
    maps: [Option<Vec<f32>>; 3],
    run: Run,
    lfo_run: LfoRun,
    value: f32,
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

impl Articulator {
    /// The one FL makes for control `index` of a unit.
    fn new(index: usize) -> Self {
        let additive = matches!(index, PAN | CUTOFF | RESONANCE);
        let pattern = idle(1, 1.0, crate::menvelope::ENVELOPE_KNOBS);
        let lfo = idle(2, 1.0, crate::menvelope::LFO_KNOBS);
        let centre = if additive { 0.5 } else { 0.0 };
        let track = Track::new(&pattern, centre);
        let lfo = Lfo::new(&lfo);
        Articulator {
            additive,
            base: if additive { 0.0 } else { 1.0 },
            amount: 1.0,
            lfo_amount: 1.0,
            run: track.start(1.0),
            patterns: vec![track; PATTERNS],
            plain: true,
            lfo_run: lfo.start(1.0),
            lfo,
            lfo_knobs: crate::menvelope::LFO_KNOBS,
            maps: [None, None, None],
            value: if additive { 0.0 } else { 1.0 },
        }
    }

    /// Read one from a state, for control `index`.
    fn read(reader: &mut Reader<'_>, index: usize) -> Result<Self, StateError> {
        let mut articulator = Articulator::new(index);
        let centre = if articulator.additive { 0.5 } else { 0.0 };
        let _format = reader.i32()?;
        articulator.base = reader.f32()?;
        articulator.amount = reader.f32()?;
        articulator.lfo_amount = reader.f32()?;
        articulator.plain = true;
        for pattern in &mut articulator.patterns {
            let envelope = MEnvelope::read(reader, Class::Envelope)?;
            articulator.plain &= !envelope.active || envelope.plain(Class::Envelope);
            *pattern = Track::new(&envelope, centre);
        }
        let lfo = MEnvelope::read(reader, Class::Lfo)?;
        articulator.lfo_knobs = lfo.knobs;
        articulator.lfo = Lfo::new(&lfo);
        for map in &mut articulator.maps {
            let envelope = MEnvelope::read(reader, Class::Map)?;
            // A map nobody drew is a level line at 1 less the centre, and
            // FL keeps no table for one.
            let untouched = envelope.flat() == Some((1.0 - centre) as f32);
            *map = (!untouched).then(|| envelope.table(MAP, centre));
        }
        Ok(articulator)
    }

    /// Whether anything moves the control: a pattern, the LFO or a map.
    fn live(&self, pattern: usize) -> bool {
        let mapped = self.maps.iter().any(Option::is_some);
        self.patterns[pattern].active() || self.lfo.active() || mapped
    }

    /// Put the pattern and the LFO back at their start, a tick before it.
    fn restart(&mut self, pattern: usize, rates: [f64; 2]) {
        let track = &self.patterns[pattern];
        self.run = track.start_before(rates[usize::from(track.tempo)]);
        let rate = rates[usize::from(self.lfo.tempo())];
        self.lfo_run = self.lfo.start_before(rate);
        self.value = self.base;
    }

    /// Work the value out for one control tick. `places` are where
    /// Modulation X, Y and the follower are in their maps' tables.
    fn tick(&mut self, pattern: usize, places: [usize; 3], rates: [f64; 2]) {
        let mut value = self.base;
        for (map, (table, place)) in self.maps.iter().zip(places).enumerate() {
            let Some(table) = table else {
                continue;
            };
            let mut mapped = table[place];
            if map == 2 {
                mapped *= self.amount;
            }
            if self.additive {
                value += mapped;
            } else {
                value *= mapped;
            }
        }
        let track = &self.patterns[pattern];
        if track.active() {
            track.advance(&mut self.run, 1.0, rates[usize::from(track.tempo)]);
            if self.additive {
                value += self.run.out * self.amount;
            } else {
                value = value * self.run.out * self.amount;
            }
        }
        if self.lfo.active() {
            let rate = rates[usize::from(self.lfo.tempo())];
            self.lfo.advance(&mut self.lfo_run, 1.0, rate);
            if self.additive {
                value += self.lfo_run.out * self.lfo_amount;
            } else {
                value *= self.lfo_run.out * self.lfo_amount * 2.0 + 1.0;
            }
        }
        self.value = value;
    }
}

/// One filter of a unit's row, with what every type keeps of the samples
/// before.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stage {
    /// The two coefficients Chocolate and Lime glide: where the design
    /// puts them, and where the last tick left them.
    aim: [f32; 2],
    now: [f32; 2],
    /// Chocolate's low and band of each side, and Lime's low and speed.
    state: [[f64; 2]; 2],
    biquad: [Biquad64; 2],
    coefficients: Coefficients64,
    /// Mango: its four poles and its last output a side, the pole's
    /// coefficient, the feedback, the make-up gain and the drive.
    poles: [[f32; 5]; 2],
    pole: f32,
    feedback: f32,
    gain: f32,
    drive: f32,
}

impl Stage {
    /// A stage at rest. `damping` is what the second coefficient waits at.
    fn new(damping: f32) -> Self {
        Stage {
            aim: [0.0, damping],
            now: [0.5, damping],
            state: [[0.0; 2]; 2],
            biquad: [Biquad64::default(); 2],
            coefficients: Coefficients64::IDENTITY,
            poles: [[0.0; 5]; 2],
            pole: 0.0,
            feedback: 0.0,
            gain: 0.0,
            drive: 0.0,
        }
    }

    fn clear(&mut self) {
        self.state = [[0.0; 2]; 2];
        self.biquad = [Biquad64::default(); 2];
        self.poles = [[0.0; 5]; 2];
    }
}

/// Mango's saturator: `x (a^2 + 3a + 6) / (a^3 + 3a^2 + 6a + 12)` with `a`
/// the size of `x`.
fn saturate(x: f32) -> f32 {
    let size = x.abs();
    let poly = (3.0 + size) * size + 6.0;
    x * poly / (poly * size + 12.0)
}

/// One filter unit.
#[derive(Debug, Clone, PartialEq)]
struct Unit {
    params: [i32; 12],
    /// How many filters run in a row: 0 is none, and 4 and 5 are 2 and 3
    /// with the earlier ones tuned up.
    mode: u8,
    kind: i32,
    alternate: bool,
    enabled: bool,
    shaper_on: bool,
    bipolar: bool,
    shaper: MEnvelope,
    articulators: Vec<Articulator>,

    /// What the parameters come to.
    input: f32,
    from_previous: f32,
    pan: f32,
    volume: f32,
    cutoff: f32,
    resonance: f32,
    levels: [f32; 3],
    pre_amp: f32,
    shaper_mix: f32,
    /// The unit feeds the follower.
    feeds: bool,

    /// What the mode comes to: the filters in the row, whether the earlier
    /// ones are tuned up, the power that spreads a resonance or a gain
    /// over the row, and the width of a band filter.
    stages: usize,
    detune: bool,
    spread: f32,
    width: f32,
    stage: [Stage; STAGES],
    /// The waveshaper's table, with one more step at either end.
    table: Vec<i16>,

    /// The three gains of Chocolate, the waveshaper's mix and the four
    /// gains of the output, as the last tick left them.
    gains: [f32; 3],
    mix: f32,
    matrix: [f32; 4],
}

impl Unit {
    /// The unit FL makes: the first is switched on.
    fn new(index: usize) -> Self {
        Unit {
            params: UNIT_DEFAULTS,
            mode: 1,
            kind: CHOCOLATE,
            alternate: false,
            enabled: index == 0,
            shaper_on: false,
            bipolar: false,
            shaper: MEnvelope {
                points: vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)],
                arp: vec![0, 0],
                ..idle(3, 0.0, crate::menvelope::ENVELOPE_KNOBS)
            },
            articulators: (0..ARTICULATORS).map(Articulator::new).collect(),
            input: 1.0,
            from_previous: 0.0,
            pan: 0.0,
            volume: 1.0,
            cutoff: 0.5,
            resonance: 0.5,
            levels: [1.0, 0.0, 0.0],
            pre_amp: 1.0,
            shaper_mix: 1.0,
            feeds: false,
            stages: 1,
            detune: false,
            spread: 1.0,
            width: (LN_2 / 2.0) as f32,
            stage: [Stage::new(1.0); STAGES],
            table: Vec::new(),
            gains: [0.0; 3],
            mix: 0.0,
            matrix: [0.0; 4],
        }
    }

    fn read(reader: &mut Reader<'_>, index: usize) -> Result<Self, StateError> {
        let mut unit = Unit::new(index);
        for (param, value) in unit.params.iter_mut().enumerate() {
            *value = reader.i32()?;
            let (min, max) = UNIT[param];
            if *value < min || *value > max {
                let param = GLOBALS.len() + index * PER_UNIT + param;
                return Err(StateError::Range {
                    param,
                    value: *value,
                });
            }
        }
        unit.mode = reader.u8()?;
        reader.bytes(3)?;
        unit.kind = reader.i32()?;
        unit.alternate = reader.u8()? != 0;
        unit.enabled = reader.u8()? != 0;
        unit.shaper_on = reader.u8()? != 0;
        unit.shaper = MEnvelope::read(reader, Class::Map)?;
        unit.bipolar = reader.u8()? != 0;
        for (index, articulator) in unit.articulators.iter_mut().enumerate() {
            *articulator = Articulator::read(reader, index)?;
        }
        Ok(unit)
    }

    /// Bake the waveshaper's curve, when the waveshaper is on: `SHAPER`
    /// steps for a unipolar curve and twice as many for a bipolar one, the
    /// first and the last once more at either end.
    fn bake(&mut self) {
        self.table.clear();
        if !self.shaper_on {
            return;
        }
        let (steps, centre, scale) = if self.bipolar {
            (2 * SHAPER - 1, 0.5, 65_535.0)
        } else {
            (SHAPER - 1, 0.0, 32_767.0)
        };
        let step = |level: &f32| {
            let scaled = f64::from(*level * scale).round_ties_even();
            scaled.clamp(-32_768.0, 32_767.0) as i16
        };
        let levels = self.shaper.table(steps, centre);
        let steps: Vec<i16> = levels.iter().map(step).collect();
        self.table.push(steps[0]);
        self.table.extend(&steps);
        self.table.push(steps[steps.len() - 1]);
    }

    /// Work out what the mode asks of the row, and put the filters at
    /// rest.
    fn arrange(&mut self) {
        let lime = matches!(self.kind, LIME_LOW | LIME_HIGH);
        let damping = if lime { 0.1 } else { 1.0 };
        let mut last = i32::from(self.mode) - 1;
        let mut width = f32::from(self.mode);
        self.detune = last > 2;
        if self.detune {
            last -= 2;
        }
        self.stages = usize::try_from(last + 1).unwrap_or(0).min(STAGES);
        if last >= 0 {
            self.spread = if self.kind == MANGO {
                (1.0 / (f64::from(last) / 4.0 + 1.0)) as f32
            } else {
                (1.0 / f64::from(last + 1)) as f32
            };
            if self.kind == VANILLA_STOP {
                width = 1.0 / width;
            }
        }
        self.width = (f64::from(width) * (LN_2 / 2.0)) as f32;
        self.stage = [Stage::new(damping); STAGES];
    }

    /// How far a stage before the last one is tuned up: its corner is the
    /// one after it to this power.
    fn tuning(&self) -> f32 {
        if self.stages == 2 { 0.5 } else { 0.75 }
    }

    /// Design the row for a cutoff and a resonance. `scale` is `44100 /
    /// rate` and `gain` what the low slider is for a shelf, a peak or
    /// Mango's drive.
    fn design(&mut self, cutoff: f32, resonance: f32, gain: f32, scale: f32) {
        let x = f64::from(cutoff);
        let mut r = f64::from(resonance);
        let scale = f64::from(scale);
        let (stages, detune, tuning) = (self.stages, self.detune, self.tuning());
        let spread = self.spread;
        let detuned = |fc: f64| if detune { power(fc, tuning) } else { fc };
        let alternate = self.alternate;
        let wider = |r: f64| {
            if alternate { r * (x + 1.0) * 0.5 } else { r }
        };
        match self.kind {
            CHOCOLATE => {
                let mut fc = if x > 0.0 {
                    curve(51.0, x) * 0.015_900_000_929_832_46
                } else {
                    0.0
                };
                for stage in self.stage[..stages].iter_mut().rev() {
                    let turn = ((fc + 0.005) * scale).min(0.625);
                    stage.aim[0] = (turn.sin() * 2.0) as f32;
                    fc = detuned(fc);
                }
                let mut damping = if r <= 0.0 {
                    1.0
                } else if r < 1.0 {
                    1.0 - r * 0.899_999_976_158_142_1
                } else {
                    0.2
                };
                if alternate {
                    damping *= damping;
                }
                self.stage[0].aim[1] = damping as f32;
            }
            LIME_LOW | LIME_HIGH => {
                r = wider(r);
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
                for stage in self.stage[..stages].iter_mut().rev() {
                    let cos = ((fc + 0.007) * scale).min(0.4 * PI).cos();
                    stage.aim[0] = (2.0 - cos * 2.0) as f32;
                    let fall = cos - 1.0;
                    let keep = (-fall * fall * fall).sqrt() * f64::from(SQRT_2 as f32);
                    let keep = (keep / (q * fall) + 1.0) as f32;
                    stage.aim[1] = if f64::from(keep) < 0.1 { 0.1 } else { keep };
                    fc = detuned(fc);
                }
            }
            VANILLA_LOW..=VANILLA_HIGH_SHELF => {
                r = wider(r);
                let kind = self.kind;
                let (reach, offset, most) = match kind {
                    VANILLA_LOW => (0.000_664_666_644_297_540_2, 0.003, f64::INFINITY),
                    VANILLA_HIGH => (0.000_399_933_342_123_404_15, 1e-4, 0.6001),
                    _ => (0.000_399_933_342_123_404_15, 1e-4, 1.0001),
                };
                let mut fc = if x > 0.0 {
                    (curve(1501.0, x) * reach).min(most)
                } else {
                    0.0
                };
                // What the resonance knob is for the type: a Q, a width in
                // octaves or a shelf's slope.
                let shape = match kind {
                    VANILLA_BAND | VANILLA_STOP => {
                        if r <= 0.0 {
                            0.01
                        } else if r < 1.0 {
                            curve(101.0, f64::from(r as f32)) * 0.119_899_995_625_019_07 + 0.01
                        } else {
                            12.0
                        }
                    }
                    VANILLA_LOW_SHELF | VANILLA_HIGH_SHELF => {
                        let r = 1.0 - r;
                        if r <= 0.0 {
                            0.1
                        } else if r < 1.0 {
                            curve(3.0, f64::from(r as f32)) * 0.449_999_988_079_071_04 + 0.1
                        } else {
                            1.0
                        }
                    }
                    VANILLA_PEAK => {
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
                let mut a = f64::from(gain);
                if stages > 1 && kind >= VANILLA_LOW_SHELF {
                    a = (a.ln() * f64::from(spread)).exp();
                }
                let width = f64::from(self.width);
                let scale = scale * TAU;
                for stage in self.stage[..stages].iter_mut().rev() {
                    let w = ((fc + offset) * scale).clamp(0.0, 2.849_517_146_113_191);
                    let (sin, cos) = w.sin_cos();
                    stage.coefficients = match kind {
                        VANILLA_LOW => Coefficients64::low_pass(cos, sin * (0.5 / shape)),
                        VANILLA_HIGH => Coefficients64::high_pass(cos, sin * (0.5 / shape)),
                        VANILLA_ALL => Coefficients64::all_pass(cos, sin * (0.5 / shape)),
                        VANILLA_BAND => {
                            let alpha = sin * (shape * width * w / sin).sinh();
                            Coefficients64::band_pass(cos, alpha)
                        }
                        VANILLA_STOP => {
                            let alpha = sin * (shape * width * w / sin).sinh();
                            Coefficients64::notch(cos, alpha)
                        }
                        VANILLA_PEAK => Coefficients64::peak(cos, sin / (shape * 2.0), a),
                        _ => {
                            let beta =
                                a.sqrt() * ((a + 1.0 / a) * (1.0 / shape - 1.0) + 2.0).sqrt();
                            if kind == VANILLA_LOW_SHELF {
                                Coefficients64::low_shelf(cos, beta * sin, a)
                            } else {
                                Coefficients64::high_shelf(cos, beta * sin, a)
                            }
                        }
                    };
                    fc = detuned(fc);
                }
            }
            MANGO => {
                let mut fc = if x > 0.0 {
                    (curve(201.0, x) * 0.014_949_999_749_660_492).min(3.0)
                } else {
                    0.0
                };
                let drive = curve(251.0, f64::from(gain)) * 0.12;
                let driven = drive >= 0.001;
                let thin = if driven { 1.0 / drive } else { 1.0 };
                let mut feedback = 0.0;
                if r > 0.0 {
                    feedback = f64::from((drive as f32).min(1.0)) * 0.6 + 1.9;
                    if r < 1.0 {
                        feedback *= r;
                    }
                }
                if stages > 1 {
                    let grown = (feedback + 1.0).ln() * f64::from(spread);
                    feedback = grown.exp() - 1.0;
                }
                let scale = scale * 0.5;
                for stage in self.stage[..stages].iter_mut().rev() {
                    let g = ((fc + 0.01) * scale).min(0.65);
                    let g2 = g * g;
                    let turn = ((g * 1.873 * g2 + g2 * 0.4955) - g * 0.649) + 0.9988;
                    let mut pole = 1.0 - (-PI * turn * g).exp();
                    stage.feedback = (-feedback * (g2 * -3.9364 + g * 1.8409 + 0.9968)) as f32;
                    if driven {
                        stage.drive = (drive * 2.0) as f32;
                        pole *= thin;
                        let heat = (thin * 4.0).tanh() as f32;
                        if alternate {
                            stage.feedback *= heat;
                        }
                        let open = f64::from(1.0 - stage.feedback);
                        stage.gain = (f64::from(1.0 / heat) * 0.5 * open) as f32;
                    } else {
                        stage.drive = 0.0;
                        stage.gain = (f64::from(1.0 - stage.feedback) * 0.5) as f32;
                    }
                    stage.pole = pole as f32;
                    fc = detuned(fc);
                }
            }
            _ => {}
        }
    }
}

/// The envelope follower: it goes after the louder side of what it hears,
/// at one pace up and another down.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tracker {
    gain: f32,
    /// How much of the way stays after a frame, on the way up and down.
    pace: [f32; 2],
    state: f32,
    /// The most it was in this control tick.
    most: f32,
}

impl Tracker {
    fn hear(&mut self, left: &[f32], right: &[f32]) {
        let [attack, release] = self.pace;
        let mut state = self.state;
        let mut most = 0f32;
        for (l, r) in left.iter().zip(right) {
            let heard = l.abs().max(r.abs()) * self.gain;
            let pace = if heard <= state { release } else { attack };
            state = (state - heard) * pace + heard;
            if state.abs() < 5.960_464_5e-8 {
                state = 0.0;
            }
            most = most.max(state.abs());
        }
        self.state = state;
        self.most = self.most.max(most);
    }
}

/// The blocks the plugin works in, one side each: its input at the
/// oversampled rate, the mix of the units, a unit's signal and two more
/// for a unit's filter and waveshaper.
#[derive(Debug, Clone, PartialEq)]
struct Work {
    input: [Vec<f32>; 2],
    mix: [Vec<f32>; 2],
    unit: [Vec<f32>; 2],
    band: [Vec<f32>; 2],
    high: [Vec<f32>; 2],
}

impl Default for Work {
    fn default() -> Self {
        let block = || vec![0.0; CHUNK << DOUBLINGS];
        let pair = || [block(), block()];
        Work {
            input: pair(),
            mix: pair(),
            unit: pair(),
            band: pair(),
            high: pair(),
        }
    }
}

/// Fruity Love Philter. A new one has FL's defaults: its first unit on, a
/// Chocolate filter with its low output alone, its cutoff half way.
#[derive(Debug, Clone, PartialEq)]
pub struct LovePhilter {
    globals: [i32; 8],
    solo: i32,
    hq_envelopes: bool,
    /// The waveshapers read between their steps.
    hq_shaper: bool,
    /// How many times the rate is doubled.
    oversampling: usize,
    smooth: bool,
    center: bool,
    /// The follower listens to the first unit and not to the input.
    follow_unit: bool,
    units: Vec<Unit>,

    hz: f64,
    tempo: f64,
    /// `44100 / rate` at the oversampled rate, which scales every corner.
    scale: f32,
    /// The most a gain moves in a frame.
    cap: f32,
    /// A control tick in frames, and the frames the one under way has
    /// left.
    tick: usize,
    left: usize,
    /// What an envelope's `x` unit is in ticks: for one that counts in
    /// half seconds and for one that counts in beats.
    rates: [f64; 2],
    /// Modulation X and Y as they glide, and how much of the way stays
    /// after a tick.
    glide: [f32; 2],
    keep: f32,
    /// The follower, and what it listens to: 0 nothing, 1 the input, 2
    /// the first unit.
    tracker: Tracker,
    follows: u8,
    /// Where the three maps are read.
    places: [usize; 3],
    master: f32,
    master_lfo: f32,
    /// The next tick puts every gain where it belongs with no glide.
    snap: bool,
    up: [[HalfBand; 2]; DOUBLINGS],
    down: [[HalfBand; 2]; DOUBLINGS],
    offset: [Follower; 2],
    work: Work,
}

impl Default for LovePhilter {
    fn default() -> Self {
        let mut philter = LovePhilter {
            globals: GLOBAL_DEFAULTS,
            solo: -1,
            hq_envelopes: false,
            hq_shaper: false,
            oversampling: 1,
            smooth: true,
            center: false,
            follow_unit: false,
            units: (0..UNITS).map(Unit::new).collect(),
            hz: 44_100.0,
            tempo: 140.0,
            scale: 1.0,
            cap: 0.01,
            tick: 64,
            left: 0,
            rates: [0.0; 2],
            glide: [512.0; 2],
            keep: 0.0,
            tracker: Tracker {
                gain: 1.0,
                pace: [0.0; 2],
                state: 0.0,
                most: 0.0,
            },
            follows: 0,
            places: [512, 512, 0],
            master: 1.0,
            master_lfo: 1.0,
            snap: true,
            up: [[HalfBand::default(); 2]; DOUBLINGS],
            down: [[HalfBand::default(); 2]; DOUBLINGS],
            offset: [Follower::default(); 2],
            work: Work::default(),
        };
        philter.prepare();
        philter
    }
}

impl LovePhilter {
    /// The pattern that plays, from 0.
    fn pattern(&self) -> usize {
        (self.globals[5].clamp(1, PATTERNS as i32) - 1) as usize
    }

    /// The rate the filters run at.
    fn rate(&self) -> f64 {
        self.hz * (1 << self.oversampling) as f64
    }

    /// Work out the control tick and what the envelopes count by.
    fn retime(&mut self) {
        let rate = self.rate();
        self.scale = (44_100.0 / rate) as f32;
        self.cap = (f64::from(self.scale) * 0.001) as f32 * 10.0;
        let ticks = if self.hq_envelopes { 8.0 } else { 64.0 };
        let frames = (ticks * rate / 44_100.0 / 4.0).round_ties_even() * 4.0;
        self.tick = frames.max(4.0) as usize;
        let seconds = self.tick as f64 / rate;
        self.rates = [seconds * 2.0, seconds * self.tempo / 60.0];
        let spent = self.tick as f64 * 44_100.0 / (rate * 96.0);
        self.keep = (0.01f64.ln() * spent / 96.0).exp() as f32;
        // The follower's times: 4096 is 25 ms and 16384 a second.
        let unit = rate * 0.0125;
        for (pace, knob) in self.tracker.pace.iter_mut().zip(&self.globals[3..5]) {
            let knob = (f64::from(*knob) * 6.103_515_625e-5) as f32;
            let frames = curve(81.0, f64::from(knob)) * unit;
            *pace = if frames > 0.0 {
                (0.01f64.ln() / frames).exp() as f32
            } else {
                0.0
            };
        }
    }

    /// Work out everything the parameters come to.
    fn levels(&mut self) {
        self.master = level(self.globals[0] as f32 / 128.0);
        self.master_lfo = signed_level(self.globals[1]);
        self.tracker.gain = level(self.globals[2] as f32 / 128.0);
        let (master, master_lfo, solo) = (self.master, self.master_lfo, self.solo);
        let mut previous: Option<(bool, i32)> = None;
        for (index, unit) in self.units.iter_mut().enumerate() {
            let p = unit.params;
            for articulator in &mut unit.articulators {
                articulator.lfo_amount = master_lfo;
            }
            let cutoff = &mut unit.articulators[CUTOFF];
            cutoff.amount = p[0] as f32 / 128.0;
            cutoff.lfo_amount = cutoff.amount * master_lfo;
            unit.cutoff = p[1] as f32 / 256.0;
            unit.resonance = p[2] as f32 / 256.0;
            unit.levels = [p[3], p[4], p[5]].map(|v| level(v as f32 / 128.0));
            unit.pre_amp = p[6] as f32 / 128.0;
            unit.shaper_mix = p[7] as f32 / 128.0;
            unit.pan = p[8] as f32 / 128.0;
            unit.volume = if solo < 0 || solo == index as i32 {
                level(p[9] as f32 / 128.0) * master
            } else {
                0.0
            };
            unit.input = signed_level(p[10]);
            // A unit hears the one before it when both are switched on.
            unit.from_previous = match previous {
                Some((true, next)) if unit.enabled => signed_level(next),
                _ => 0.0,
            };
            previous = Some((unit.enabled, p[11]));
        }
        let mapped = |unit: &Unit| unit.articulators.iter().any(|a| a.maps[2].is_some());
        self.follows = match (self.units.iter().any(mapped), self.follow_unit) {
            (false, _) => 0,
            (true, false) => 1,
            (true, true) => 2,
        };
        let feeds = self.follows == 2;
        for (index, unit) in self.units.iter_mut().enumerate() {
            unit.feeds = feeds && index == 0 && unit.enabled;
        }
    }

    /// Make everything ready after a state: the rows, the tables, the
    /// levels and the patterns at their start.
    fn prepare(&mut self) {
        self.retime();
        for unit in &mut self.units {
            unit.arrange();
            unit.bake();
        }
        self.levels();
        self.restart();
    }

    /// Put the patterns and the LFOs back at their start and the filters
    /// at rest.
    fn restart(&mut self) {
        let (pattern, rates) = (self.pattern(), self.rates);
        for unit in &mut self.units {
            for articulator in &mut unit.articulators {
                articulator.restart(pattern, rates);
            }
            for stage in &mut unit.stage {
                stage.clear();
            }
        }
        self.left = 0;
        self.snap = true;
        self.glide = [self.globals[6] as f32, self.globals[7] as f32];
        (self.tracker.state, self.tracker.most) = (0.0, 0.0);
        self.places[2] = 0;
        self.up = [[HalfBand::default(); 2]; DOUBLINGS];
        self.down = [[HalfBand::default(); 2]; DOUBLINGS];
        self.offset = [Follower::default(); 2];
    }

    /// What starts a control tick: the maps' places and every
    /// articulator's value.
    fn control(&mut self) {
        for (glide, target) in self.glide.iter_mut().zip(&self.globals[6..8]) {
            let target = *target as f32;
            *glide = if self.smooth {
                target + (*glide - target) * self.keep
            } else {
                target
            };
        }
        let place = |value: f32| f64::from(value).round_ties_even().clamp(0.0, MAP as f64) as usize;
        self.places[0] = place(self.glide[0]);
        self.places[1] = place(self.glide[1]);
        if self.follows > 0 {
            self.places[2] = place(self.tracker.most * MAP as f32);
            self.tracker.most = 0.0;
        }
        let (pattern, places, rates) = (self.pattern(), self.places, self.rates);
        for unit in &mut self.units {
            for articulator in &mut unit.articulators {
                if articulator.live(pattern) {
                    articulator.tick(pattern, places, rates);
                } else {
                    articulator.value = articulator.base;
                }
            }
        }
    }

    /// Run `len` frames of one control tick, from `at` in the work
    /// blocks, through the units into the mix.
    fn run(&mut self, at: usize, len: usize) {
        let span = at..at + len;
        let work = &mut self.work;
        if self.follows == 1 {
            let [left, right] = &work.input;
            self.tracker.hear(&left[span.clone()], &right[span.clone()]);
        }
        let (scale, cap, snap) = (self.scale, self.cap, self.snap);
        for unit in self.units.iter_mut().filter(|unit| unit.enabled) {
            // What the unit hears: the one before it, which the block
            // still holds, and the input.
            for side in 0..2 {
                let block = &mut work.unit[side][span.clone()];
                let input = &work.input[side][span.clone()];
                let (previous, direct) = (unit.from_previous, unit.input);
                for (sample, input) in block.iter_mut().zip(input) {
                    let before = if previous == 0.0 {
                        0.0
                    } else {
                        *sample * previous
                    };
                    *sample = before + *input * direct;
                }
            }
            unit.filter(work, span.clone(), scale, cap, snap);
            if !unit.table.is_empty() {
                unit.shape(work, span.clone(), (cap, snap, self.hq_shaper));
            }
            if unit.feeds {
                let [left, right] = &work.unit;
                self.tracker.hear(&left[span.clone()], &right[span.clone()]);
            }
            let pan = unit.pan + unit.articulators[PAN].value * 2.0;
            let volume = unit.volume * unit.articulators[VOLUME].value;
            let target = stereo_gains(pan, volume, 0.0);
            if snap {
                unit.matrix = target;
            }
            let mut gains = [0.0; 4];
            let mut steps = [0.0; 4];
            for k in 0..4 {
                (gains[k], steps[k]) = glide(&mut unit.matrix[k], target[k], cap, len);
            }
            let [unit_left, unit_right] = &work.unit;
            let [mix_left, mix_right] = &mut work.mix;
            for frame in span.clone() {
                let (l, r) = (unit_left[frame], unit_right[frame]);
                mix_left[frame] += l * gains[0] + r * gains[2];
                mix_right[frame] += r * gains[1] + l * gains[3];
                for (gain, step) in gains.iter_mut().zip(steps) {
                    *gain += step;
                }
            }
        }
    }

    /// One block of no more than [`CHUNK`] frames.
    fn chunk(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len();
        let doublings = self.oversampling;
        let frames = len << doublings;
        // Up to the rate the filters run at.
        for (side, block) in [&*left, &*right].into_iter().enumerate() {
            let input = &mut self.work.input[side];
            let scratch = &mut self.work.band[side];
            input[..len].copy_from_slice(block);
            let mut now = len;
            for stage in (0..doublings).rev() {
                scratch[..now].copy_from_slice(&input[..now]);
                self.up[stage][side].up(&scratch[..now], &mut input[..now * 2]);
                self.up[stage][side].flush_subnormal();
                now *= 2;
            }
            self.work.mix[side][..frames].fill(0.0);
        }
        let mut at = 0;
        while at < frames {
            if self.left == 0 {
                self.left = self.tick;
                self.control();
            }
            let run = self.left.min(frames - at);
            self.run(at, run);
            at += run;
            self.left -= run;
            self.snap = false;
        }
        // And back down.
        for (side, block) in [&mut *left, &mut *right].into_iter().enumerate() {
            let mix = &mut self.work.mix[side];
            let scratch = &mut self.work.band[side];
            let mut now = frames;
            for stage in (0..doublings).rev() {
                scratch[..now].copy_from_slice(&mix[..now]);
                self.down[stage][side].down(&scratch[..now], &mut mix[..now / 2]);
                self.down[stage][side].flush_subnormal();
                now /= 2;
            }
            block.copy_from_slice(&mix[..len]);
            if self.center {
                let pace = CENTER_PACE / self.hz;
                let offset = &mut self.offset[side];
                for sample in block.iter_mut() {
                    let x = f64::from(*sample);
                    *sample = (x - offset.step(x, pace, CENTER_KEEP)) as f32;
                }
                offset.flush();
            }
        }
    }

    /// The parameter FL numbers `index`: where it is kept and its range.
    fn place(index: usize) -> Option<Place> {
        if index < GLOBALS.len() {
            return Some(Place::Global(index));
        }
        let index = index - GLOBALS.len();
        let (unit, mut index) = (index / PER_UNIT, index % PER_UNIT);
        if unit >= UNITS {
            return None;
        }
        // The last unit has no "to next".
        let own = if unit == UNITS - 1 { 11 } else { 12 };
        if index < own {
            return Some(Place::Unit(unit, index));
        }
        index -= own;
        let articulator = index / LFO.len();
        (articulator < ARTICULATORS).then_some(Place::Lfo(unit, articulator, index % LFO.len()))
    }
}

/// Where a parameter is kept.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Place {
    Global(usize),
    Unit(usize, usize),
    /// An LFO knob of an articulator of a unit.
    Lfo(usize, usize, usize),
}

impl Unit {
    /// Run the unit's filter over a span of its block, in place.
    fn filter(
        &mut self,
        work: &mut Work,
        span: std::ops::Range<usize>,
        scale: f32,
        cap: f32,
        snap: bool,
    ) {
        let len = span.len();
        let cutoff = self.cutoff + self.articulators[CUTOFF].value;
        let resonance = self.resonance + self.articulators[RESONANCE].value;
        let stages = self.stages;
        match self.kind {
            CHOCOLATE => {
                let mut gains = [0.0; 3];
                for (k, gain) in gains.iter_mut().enumerate() {
                    *gain = self.levels[k] * self.articulators[LOW + k].value;
                    if stages > 1 {
                        *gain = power(f64::from(*gain), self.spread) as f32;
                    }
                }
                self.design(cutoff, resonance, 0.0, scale);
                if snap {
                    self.gains = gains;
                    for stage in &mut self.stage[..stages] {
                        stage.now = stage.aim;
                    }
                }
                let mut mix = [(0.0, 0.0); 3];
                for k in 0..3 {
                    mix[k] = glide(&mut self.gains[k], gains[k], cap, len);
                }
                // The corner of every stage glides, and the damping of the
                // first. The stages after it are not resonant.
                let mut glides = [[(0f32, 0f32); 2]; STAGES];
                for (index, stage) in self.stage[..stages].iter_mut().enumerate() {
                    let moves = &mut glides[index];
                    moves[0] = glide(&mut stage.now[0], stage.aim[0], cap, len);
                    moves[1] = if index == 0 {
                        glide(&mut stage.now[1], stage.aim[1], cap, len)
                    } else {
                        (1.0, 0.0)
                    };
                    if self.alternate {
                        // The damping goes by the corner: the higher of
                        // where it glides from and where it gets to.
                        let corner = moves[0].0.max(stage.now[0]);
                        let first = if index == 0 { moves[1].0 } else { 1.0 };
                        moves[1].0 = first * scale / corner;
                    }
                }
                for (stage, moves) in self.stage[..stages].iter_mut().zip(glides) {
                    for side in 0..2 {
                        let [mut low, mut band] = stage.state[side];
                        let (mut f, f_step) = (f64::from(moves[0].0), f64::from(moves[0].1));
                        let (mut q, q_step) = (f64::from(moves[1].0), f64::from(moves[1].1));
                        let block = &mut work.unit[side][span.clone()];
                        let bands = &mut work.band[side][span.clone()];
                        let highs = &mut work.high[side][span.clone()];
                        let frames = block.iter_mut().zip(bands.iter_mut().zip(highs));
                        let (mut g, steps) = (mix.map(|m| m.0), mix.map(|m| m.1));
                        for (sample, (band_out, high_out)) in frames {
                            low += band * f;
                            let high = f64::from(*sample) - (band * q + low);
                            band += high * f;
                            (*band_out, *high_out) = (band as f32, high as f32);
                            *sample = low as f32 * g[0] + *band_out * g[1] + *high_out * g[2];
                            f += f_step;
                            q += q_step;
                            for (gain, step) in g.iter_mut().zip(steps) {
                                *gain += step;
                            }
                        }
                        stage.state[side] = [low, band].map(flushed);
                    }
                }
            }
            LIME_LOW | LIME_HIGH => {
                self.design(cutoff, resonance, 0.0, scale);
                let high = self.kind == LIME_HIGH;
                for stage in &mut self.stage[..stages] {
                    if snap {
                        stage.now = stage.aim;
                    }
                    let corner = glide(&mut stage.now[0], stage.aim[0], cap, len);
                    let keep = glide(&mut stage.now[1], stage.aim[1], cap, len);
                    for side in 0..2 {
                        let [mut low, mut speed] = stage.state[side];
                        let (mut g, mut k) = (f64::from(corner.0), f64::from(keep.0));
                        for sample in &mut work.unit[side][span.clone()] {
                            let x = f64::from(*sample);
                            speed += (x - low) * g;
                            low += speed;
                            speed *= k;
                            *sample = if high { (x - low) as f32 } else { low as f32 };
                            g += f64::from(corner.1);
                            k += f64::from(keep.1);
                        }
                        stage.state[side] = [low, speed].map(flushed);
                    }
                }
            }
            VANILLA_LOW..=VANILLA_HIGH_SHELF => {
                // A shelf's or a peak's gain: the slider's 64 is none, and
                // its 128 a factor of 8.
                let knob = (f64::from(self.params[3]) * 0.015_625) as f32;
                let gain = (curve(8.0, f64::from(knob)) * 0.125 + 0.125) as f32;
                self.design(cutoff, resonance, gain, scale);
                for stage in &mut self.stage[..stages] {
                    for side in 0..2 {
                        let filter = &mut stage.biquad[side];
                        filter
                            .process_block(&stage.coefficients, &mut work.unit[side][span.clone()]);
                        filter.flush();
                    }
                }
            }
            MANGO => {
                self.design(cutoff, resonance, self.params[3] as f32 / 128.0, scale);
                for stage in &mut self.stage[..stages] {
                    let (pole, feedback, gain, drive) =
                        (stage.pole, stage.feedback, stage.gain, stage.drive);
                    for side in 0..2 {
                        let poles = &mut stage.poles[side];
                        for sample in &mut work.unit[side][span.clone()] {
                            let x = *sample + poles[4] * feedback;
                            let last = poles[3];
                            if drive == 0.0 {
                                poles[0] += (x - poles[0]) * pole;
                                poles[1] += (poles[0] - poles[1]) * pole;
                                poles[2] += (poles[1] - poles[2]) * pole;
                                poles[3] += (poles[2] - poles[3]) * pole;
                            } else {
                                let mut hot = saturate(x * drive);
                                for state in &mut poles[..4] {
                                    *state += (hot - saturate(*state * drive)) * pole;
                                    hot = saturate(*state * drive);
                                }
                            }
                            poles[4] = poles[3] + last;
                            *sample = gain * poles[4];
                        }
                        for state in poles.iter_mut() {
                            if state.is_subnormal() {
                                *state = 0.0;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Run the waveshaper over a span of the unit's block, in place.
    ///
    /// The curve is baked into steps. Without `hq` the nearest step is the
    /// output, and with it the output is on the line between two steps.
    fn shape(&mut self, work: &mut Work, span: std::ops::Range<usize>, glides: (f32, bool, bool)) {
        let (cap, snap, hq) = glides;
        let len = span.len();
        let wanted = self.articulators[SHAPER_MIX].value * self.shaper_mix;
        let wanted = if wanted >= 0.0 { wanted.min(1.0) } else { 0.0 };
        if snap {
            self.mix = wanted;
        }
        let (start, step) = glide(&mut self.mix, wanted, cap, len);
        let table = &self.table;
        // Step `i` is at `table[i + 1]`.
        let last = (table.len() - 3) as i64;
        let at = |i: i64| f32::from(table[(i + 1) as usize]);
        let pre_amp = self.pre_amp;
        for side in 0..2 {
            let mut mix = start;
            for sample in &mut work.unit[side][span.clone()] {
                let x = *sample;
                let shaped = if self.bipolar {
                    let half = last as f32 * 0.5;
                    let place = x * (pre_amp * last as f32 * 0.5) + half;
                    let nearest = place.round_ties_even();
                    if hq {
                        let i = (nearest as i64).clamp(-1, last);
                        at(i) + (at(i + 1) - at(i)) * (place - nearest)
                    } else {
                        at((nearest as i64).clamp(0, last))
                    }
                } else {
                    // The curve is for an input from 0 up, and an input
                    // under 0 gets what its size gets, upside down.
                    let place = x * pre_amp * last as f32;
                    let nearest = place.round_ties_even();
                    let i = (nearest as i64).saturating_abs().min(last);
                    let between = if hq {
                        (at(i + 1) - at(i)) * (place - nearest)
                    } else {
                        0.0
                    };
                    if nearest < 0.0 {
                        -(at(i) - between)
                    } else {
                        at(i) + between
                    }
                };
                let shaped = shaped * 3.051_757_8e-5;
                *sample = if step == 0.0 && mix == 1.0 {
                    shaped
                } else if step == 0.0 && mix == 0.0 {
                    x
                } else {
                    x * (1.0 - mix) + shaped * mix
                };
                mix += step;
            }
        }
    }
}

/// A filter's state, with a number too small to hold in full made 0.
fn flushed(state: f64) -> f64 {
    if state.is_subnormal() { 0.0 } else { state }
}

impl Effect for LovePhilter {
    /// FL turns a version past 12 down, and so does this. This asks for
    /// memory for the envelopes and their tables.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let word = crate::state_version(bytes, 4)?;
        if word == i32::MAX || word >= 0xc0004 {
            return Err(StateError::Version { found: word });
        }
        let version = if word >= 0x10000 { word >> 16 } else { word };
        if !(0..=NEWEST).contains(&version) {
            return Err(StateError::Version { found: word });
        }
        let packed = &bytes[4..];
        let unpacked = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(packed, ROOM);
        let Ok(body) = unpacked else {
            return Err(StateError::Length {
                expected: bytes.len(),
                found: bytes.len(),
            });
        };
        let mut reader = Reader::new(&body);
        let mut next = LovePhilter {
            hz: self.hz,
            tempo: self.tempo,
            ..LovePhilter::default()
        };
        for (param, (min, max)) in GLOBALS.into_iter().enumerate() {
            let value = reader.i32()?;
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            next.globals[param] = value;
        }
        let (_unused, _glide) = (reader.i32()?, reader.f32()?);
        next.solo = reader.i32()?;
        next.hq_envelopes = reader.i32()? > 0;
        next.oversampling = reader.i32()?.clamp(0, DOUBLINGS as i32) as usize;
        next.smooth = reader.u8()? != 0;
        next.center = reader.u8()? != 0;
        let _step = reader.u8()?;
        if version >= 10 {
            let _window = reader.u8()?;
        }
        if version >= 11 {
            let _midi = reader.u8()?;
        }
        next.follow_unit = reader.u8()? != 0;
        let _wheel = reader.i32()?;
        if version < 6 {
            let len = usize::try_from(reader.i32()?).unwrap_or(usize::MAX);
            reader.bytes(len)?;
        } else {
            let mut len = usize::from(reader.u8()?);
            if len == 0xff {
                len = reader.u32()? as usize;
            }
            reader.bytes(len.saturating_mul(2))?;
        }
        for (index, unit) in next.units.iter_mut().enumerate() {
            *unit = Unit::read(&mut reader, index)?;
        }
        if version >= 12 {
            next.hq_shaper = reader.u8()? != 0;
        }
        reader.end()?;
        next.prepare();
        *self = next;
        Ok(())
    }

    /// A rate that is not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 && f64::from(hz) != self.hz {
            self.hz = f64::from(hz);
            self.retime();
            self.restart();
        }
    }

    /// The patterns and LFOs that count in beats go by this, from their
    /// next segment on.
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm;
            self.rates[1] = self.tick as f64 / self.rate() * bpm / 60.0;
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let blocks = left[..len].chunks_mut(CHUNK);
        for (left, right) in blocks.zip(right[..len].chunks_mut(CHUNK)) {
            self.chunk(left, right);
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        match Self::place(index) {
            Some(Place::Global(index)) => self.globals[index] as f32,
            Some(Place::Unit(unit, index)) => self.units[unit].params[index] as f32,
            Some(Place::Lfo(unit, articulator, knob)) => {
                self.units[unit].articulators[articulator].lfo_knobs[knob] as f32
            }
            None => 0.0,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match Self::place(index) {
            Some(Place::Global(index)) => {
                self.globals[index] = knob_value(value, GLOBALS[index]);
                if index == 3 || index == 4 {
                    self.retime();
                }
                self.levels();
            }
            Some(Place::Unit(unit, index)) => {
                self.units[unit].params[index] = knob_value(value, UNIT[index]);
                self.levels();
            }
            Some(Place::Lfo(unit, articulator, knob)) => {
                let articulator = &mut self.units[unit].articulators[articulator];
                articulator.lfo_knobs[knob] = knob_value(value, LFO[knob]);
                articulator.lfo.set_knobs(articulator.lfo_knobs);
            }
            None => {}
        }
    }

    /// Puts the patterns and the LFOs back at their start and the filters
    /// at rest.
    fn reset(&mut self) {
        self.restart();
    }

    fn fidelity(&self) -> Fidelity {
        let pattern = self.pattern();
        let odd = |unit: &Unit| {
            let live = |a: &Articulator| a.live(pattern) && !a.plain;
            unit.enabled && unit.articulators.iter().any(live)
        };
        if self.units.iter().any(odd) {
            Fidelity::Approximate(
                "a pattern's sustain knob or its arpeggiator points are left out of Love Philter",
            )
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menvelope::tests::stream;
    use crate::testing::{corpus, every_preset, partial, projects, sine};

    /// The bytes of a unit as a state has them.
    struct Saved {
        params: [i32; 12],
        mode: u8,
        kind: i32,
        alternate: bool,
        enabled: bool,
        shaper: Option<Vec<Point>>,
        /// The pattern of an articulator, switched on and looping.
        patterns: Vec<(usize, Vec<Point>)>,
        /// The LFO of an articulator, switched on.
        lfos: Vec<(usize, i32)>,
    }

    impl Saved {
        fn new(kind: i32, enabled: bool) -> Self {
            Saved {
                params: UNIT_DEFAULTS,
                mode: 1,
                kind,
                alternate: false,
                enabled,
                shaper: None,
                patterns: Vec::new(),
                lfos: Vec::new(),
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let mut bytes = Vec::new();
            for param in self.params {
                bytes.extend(param.to_le_bytes());
            }
            bytes.extend([self.mode, 0, 0, 0]);
            bytes.extend(self.kind.to_le_bytes());
            bytes.extend([
                u8::from(self.alternate),
                u8::from(self.enabled),
                u8::from(self.shaper.is_some()),
            ]);
            let line = vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)];
            let shaper = self.shaper.as_ref().unwrap_or(&line);
            bytes.extend(stream(3, 3, shaper, [0, -1, -1, -1, -1], &[]));
            bytes.push(0);
            for index in 0..ARTICULATORS {
                let additive = matches!(index, PAN | CUTOFF | RESONANCE);
                let base = if additive { 0.0f32 } else { 1.0 };
                bytes.extend(1i32.to_le_bytes());
                for value in [base, 1.0, 1.0] {
                    bytes.extend(value.to_le_bytes());
                }
                let drawn = self.patterns.iter().find(|(at, _)| *at == index);
                for pattern in 0..PATTERNS {
                    let knobs = crate::menvelope::ENVELOPE_KNOBS;
                    match drawn.filter(|_| pattern == 0) {
                        Some((_, points)) => {
                            let end = points.len() as i32 - 1;
                            bytes.extend(stream(1, 3, points, [1, -1, -1, 0, end], &knobs));
                        }
                        None => {
                            let idle = [Point::new(0.0, 1.0)];
                            bytes.extend(stream(1, 3, &idle, [0, 0, -1, -1, -1], &knobs));
                        }
                    }
                }
                let speed = self.lfos.iter().find(|(at, _)| *at == index);
                let on = if speed.is_some() { -1 } else { 0 };
                let speed = speed.map_or(40_000, |(_, speed)| *speed);
                let one = [Point::new(0.0, 1.0)];
                let block = [1, on, -1, -1, -1];
                bytes.extend(stream(2, 3, &one, block, &[speed, 0, 0, 0, 0]));
                let level = if additive { 0.5 } else { 1.0 };
                let flat = [Point::new(0.0, level), Point::new(1.0, level)];
                for _ in 0..3 {
                    bytes.extend(stream(3, 3, &flat, [0, -1, -1, -1, -1], &[]));
                }
            }
            bytes
        }
    }

    /// A state of version 12 with these units and this oversampling.
    fn state(units: &[Saved], oversampling: i32) -> Vec<u8> {
        let mut body = Vec::new();
        for global in &GLOBAL_DEFAULTS[..6] {
            body.extend(global.to_le_bytes());
        }
        for word in [512i32, 512, 0] {
            body.extend(word.to_le_bytes());
        }
        body.extend(0.9685f32.to_le_bytes());
        for word in [-1i32, 0, oversampling] {
            body.extend(word.to_le_bytes());
        }
        body.extend([1, 0, 1, 1, 1]);
        body.push(0);
        body.extend(0i32.to_le_bytes());
        body.push(0);
        for index in 0..UNITS {
            match units.get(index) {
                Some(unit) => body.extend(unit.bytes()),
                None => body.extend(Saved::new(CHOCOLATE, false).bytes()),
            }
        }
        body.push(1);
        let mut bytes = 0xc0003i32.to_le_bytes().to_vec();
        bytes.extend(miniz_oxide::deflate::compress_to_vec_zlib(&body, 2));
        bytes
    }

    /// The plugin with a state, at 44.1 kHz.
    fn loaded(units: &[Saved], oversampling: i32) -> LovePhilter {
        let mut philter = LovePhilter::default();
        philter.set_sample_rate(44_100.0);
        philter.set_state(&state(units, oversampling)).unwrap();
        philter
    }

    /// How loud a tone at `hz` comes out, of 1.
    fn gain(philter: &mut LovePhilter, hz: f64) -> f64 {
        philter.reset();
        // A whole number of periods in the block that is measured.
        let turns = (hz / 44_100.0 * 4096.0).round() / 4096.0;
        let tone = sine(turns, 3 * 4096);
        let (mut left, mut right) = (tone.clone(), tone);
        for (l, r) in left.chunks_mut(333).zip(right.chunks_mut(333)) {
            philter.process(l, r);
        }
        assert!(left.iter().all(|v| v.is_finite()));
        partial(&left[2 * 4096..], turns)
    }

    #[test]
    fn a_low_pass_passes_a_tenth_of_its_corner_and_cuts_ten_times_it() {
        for kind in [CHOCOLATE, LIME_LOW, VANILLA_LOW, MANGO] {
            for oversampling in [0, 1, 2] {
                let mut unit = Saved::new(kind, true);
                // No resonance, and a corner in the middle of the band.
                unit.params[2] = 0;
                unit.params[1] = 110;
                if kind == MANGO {
                    // No drive, which the slider is for Mango.
                    unit.params[3] = 0;
                }
                let mut philter = loaded(&[unit], oversampling);
                // The corner is where the tone is 3 to 12 dB down.
                let mut corner = 0.0;
                for step in 0..60 {
                    let hz = 50.0 * 1.1f64.powi(step);
                    if gain(&mut philter, hz) < 0.6 {
                        corner = hz;
                        break;
                    }
                }
                assert!(
                    (100.0..2500.0).contains(&corner),
                    "{kind} {oversampling}: {corner}"
                );
                let low = gain(&mut philter, corner / 10.0);
                let high = gain(&mut philter, corner * 10.0);
                assert!((low - 1.0).abs() < 0.12, "{kind} {oversampling}: {low}");
                assert!(high < 0.12, "{kind} {oversampling}: {high}");
            }
        }
    }

    #[test]
    fn a_high_pass_does_the_opposite_and_a_band_pass_peaks_between() {
        for kind in [LIME_HIGH, VANILLA_HIGH] {
            let mut unit = Saved::new(kind, true);
            unit.params[2] = 0;
            unit.params[1] = 128;
            let mut philter = loaded(&[unit], 1);
            assert!(gain(&mut philter, 30.0) < 0.1, "{kind}");
            assert!((gain(&mut philter, 15_000.0) - 1.0).abs() < 0.15, "{kind}");
        }
        let mut band = Saved::new(VANILLA_BAND, true);
        band.params[1] = 128;
        let mut philter = loaded(&[band], 1);
        let heard: Vec<f64> = [40.0, 300.0, 600.0, 1200.0, 12_000.0]
            .iter()
            .map(|hz| gain(&mut philter, *hz))
            .collect();
        let most = heard.iter().fold(0f64, |max, v| max.max(*v));
        assert!(heard[0] < 0.3 * most && heard[4] < 0.3 * most, "{heard:?}");
        assert!((most - 1.0).abs() < 0.2, "{heard:?}");
    }

    #[test]
    fn the_filters_corners_do_not_move_with_the_rate_or_the_oversampling() {
        let corner = |hz: f32, oversampling: i32| {
            let mut unit = Saved::new(VANILLA_LOW, true);
            unit.params[2] = 0;
            let mut philter = loaded(&[unit], oversampling);
            philter.set_sample_rate(hz);
            let tone = sine(1000.0 / f64::from(hz), 16_384);
            let (mut left, mut right) = (tone.clone(), tone);
            philter.process(&mut left, &mut right);
            let heard = left[8192..].iter().fold(0f32, |max, v| max.max(v.abs()));
            f64::from(heard)
        };
        let at = corner(44_100.0, 1);
        assert!(at > 0.05 && at < 0.95, "{at}");
        for (hz, oversampling) in [(44_100.0, 0), (48_000.0, 1), (96_000.0, 2), (88_200.0, 0)] {
            let other = corner(hz, oversampling);
            assert!(
                (other - at).abs() < 0.08 * at,
                "{hz} {oversampling}: {other}"
            );
        }
    }

    #[test]
    fn more_stages_cut_more_and_a_mode_of_0_leaves_the_sound_alone() {
        let cut = |mode: u8| {
            let mut unit = Saved::new(VANILLA_LOW, true);
            unit.params[2] = 0;
            unit.mode = mode;
            gain(&mut loaded(&[unit], 1), 8000.0)
        };
        let (none, one, two, three) = (cut(0), cut(1), cut(2), cut(3));
        assert!((none - 1.0).abs() < 0.05, "{none}");
        assert!(one < 0.2 && two < one * 0.5 && three < two * 0.5);
        // Modes 4 and 5 are two and three stages with the first tuned up.
        assert!(cut(4) > two && cut(4) < one);
        assert!(cut(5) > three && cut(5) < one);
    }

    #[test]
    fn a_gating_pattern_on_the_volume_opens_and_shuts_in_time() {
        let mut unit = Saved::new(CHOCOLATE, true);
        unit.mode = 0;
        // A beat on, a beat off, as steps.
        let step = |x: f64, y: f64| Point {
            mode: 2,
            ..Point::new(x, y)
        };
        let gate = vec![Point::new(0.0, 1.0), step(1.0, 0.0), step(1.0, 1.0)];
        unit.patterns.push((VOLUME, gate));
        let mut philter = loaded(&[unit], 0);
        // 120 BPM is 22050 frames a beat.
        philter.set_tempo(120.0);
        philter.reset();
        assert_eq!(philter.fidelity(), Fidelity::Exact);
        let (mut left, mut right) = (vec![0.5f32; 88_200], vec![0.5f32; 88_200]);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            philter.process(l, r);
        }
        for (beat, open) in [(0, true), (1, false), (2, true), (3, false)] {
            let middle = beat * 22_050 + 11_025;
            let heard = left[middle];
            let wanted = if open { 0.5 } else { 0.0 };
            assert!((heard - wanted).abs() < 1e-3, "beat {beat}: {heard}");
            assert_eq!(right[middle], heard);
        }
    }

    #[test]
    fn an_lfo_on_the_cutoff_sweeps_the_filter() {
        let mut unit = Saved::new(VANILLA_LOW, true);
        unit.params[2] = 0;
        unit.lfos.push((CUTOFF, 45_000));
        let mut philter = loaded(&[unit], 1);
        let tone = sine(2000.0 / 44_100.0, 4 * 44_100);
        let (mut left, mut right) = (tone.clone(), tone);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            philter.process(l, r);
        }
        // The level of every tenth of a second.
        let levels: Vec<f32> = left
            .chunks(4410)
            .map(|block| block.iter().fold(0f32, |max, v| max.max(v.abs())))
            .collect();
        let (low, high) = levels
            .iter()
            .fold((1f32, 0f32), |(low, high), v| (low.min(*v), high.max(*v)));
        assert!(high > 0.9 && low < 0.3, "{low} to {high}");
    }

    #[test]
    fn a_unit_feeds_the_next_one_and_both_are_panned_into_the_mix() {
        // The first unit sends everything on and is silent itself.
        let mut first = Saved::new(CHOCOLATE, true);
        first.mode = 0;
        first.params[9] = 0;
        first.params[11] = 128;
        let mut second = Saved::new(CHOCOLATE, true);
        second.mode = 0;
        second.params[10] = 0;
        second.params[8] = -128;
        let mut philter = loaded(&[first, second], 0);
        let (mut left, mut right) = (vec![0.25f32; 2048], vec![0.25f32; 2048]);
        philter.process(&mut left, &mut right);
        // All the way left: the left side has its own and half the right's.
        assert!(
            (left[2000] - 0.25 * SQRT_2 as f32).abs() < 1e-4,
            "{}",
            left[2000]
        );
        assert!(right[2000].abs() < 1e-6);

        // Input volume below 0 turns the phase over.
        let mut turned = Saved::new(CHOCOLATE, true);
        turned.mode = 0;
        turned.params[10] = -128;
        let mut philter = loaded(&[turned], 0);
        let (mut left, mut right) = (vec![0.25f32; 64], vec![-0.5f32; 64]);
        philter.process(&mut left, &mut right);
        assert_eq!((left[63], right[63]), (-0.25, 0.5));
    }

    #[test]
    fn the_waveshaper_draws_its_curve_and_mixes_it_in() {
        let mut unit = Saved::new(CHOCOLATE, true);
        unit.mode = 0;
        // A curve that is flat at a half: every sample comes out a half.
        unit.shaper = Some(vec![Point::new(0.0, 0.5), Point::new(1.0, 0.5)]);
        let mut all = loaded(&[unit], 0);
        let (mut left, mut right) = (vec![0.3f32, -0.3, 0.9], vec![0.0f32; 3]);
        all.process(&mut left, &mut right);
        for (heard, wanted) in left.iter().zip([0.5, -0.5, 0.5]) {
            assert!((heard - wanted).abs() < 1e-4, "{heard}");
        }
        // A straight curve gives the input back, read between its steps,
        // and an input under 0 upside down.
        let mut line = Saved::new(CHOCOLATE, true);
        line.mode = 0;
        line.shaper = Some(vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)]);
        let mut straight = loaded(&[line], 0);
        assert!(straight.hq_shaper);
        let (mut left, mut right) = (vec![0.300_02f32], vec![-0.300_02f32]);
        straight.process(&mut left, &mut right);
        assert!((left[0] - 0.300_02).abs() < 2e-5, "{}", left[0]);
        assert_eq!(right[0], -left[0]);
        // Half the mix is half the input and half the curve.
        all.set_param(GLOBALS.len() + 7, 64.0);
        all.reset();
        let (mut left, mut right) = (vec![0.3f32; 4], vec![0.0f32; 4]);
        all.process(&mut left, &mut right);
        assert!((left[3] - 0.4).abs() < 1e-4, "{}", left[3]);
    }

    #[test]
    fn the_half_band_filters_keep_the_band_and_the_default_plugin_is_a_low_pass() {
        let mut band = HalfBand::default();
        let tone = sine(0.0625, 2048);
        let mut doubled = vec![0.0; 4096];
        band.up(&tone, &mut doubled);
        let mut back = vec![0.0; 2048];
        HalfBand::default().down(&doubled, &mut back);
        assert!((partial(&doubled[2048..], 0.03125) - 1.0).abs() < 1e-3);
        assert!((partial(&back[1024..], 0.0625) - 1.0).abs() < 1e-3);
        // Nothing of the image at the old rate less the tone.
        assert!(partial(&doubled[2048..], 0.46875) < 1e-3);

        let mut philter = LovePhilter::default();
        assert_eq!(philter.fidelity(), Fidelity::Exact);
        assert_eq!(philter.latency(), 0);
        assert!((gain(&mut philter, 60.0) - 1.0).abs() < 0.1);
        assert!(gain(&mut philter, 12_000.0) < 0.1);
    }

    #[test]
    fn the_parameters_are_numbered_as_fl_numbers_them() {
        assert!(crate::by_name("fruity love philter").is_some());
        let mut philter = LovePhilter::default();
        assert_eq!(philter.param_count(), 359);
        let globals: Vec<f32> = (0..8).map(|i| philter.param(i)).collect();
        assert_eq!(globals, GLOBAL_DEFAULTS.map(|v| v as f32));
        // The second unit starts at 8 + 44, and its first LFO at 12 on.
        assert_eq!(philter.param(8 + 44 + 1), 128.0);
        assert_eq!(philter.param(8 + 44 + 12), 40_000.0);
        // The last unit has 11 of its own.
        assert_eq!(philter.param(8 + 7 * 44 + 10), 128.0);
        assert_eq!(philter.param(8 + 7 * 44 + 11), 40_000.0);
        assert_eq!(philter.param(359), 0.0);
        philter.set_param(8 + 1, 1e9);
        assert_eq!(philter.param(8 + 1), 256.0);
        philter.set_param(5, 0.0);
        assert_eq!(philter.param(5), 1.0);
        philter.set_param(8 + 12, 0.0);
        assert_eq!(philter.param(8 + 12), 512.0);
        philter.set_param(4000, 1.0);
    }

    #[test]
    fn a_state_that_is_cut_short_too_new_or_out_of_range_is_turned_down() {
        let good = state(&[Saved::new(CHOCOLATE, true)], 1);
        let mut philter = LovePhilter::default();
        assert_eq!(philter.set_state(&good), Ok(()));
        let before = philter.clone();
        assert!(philter.set_state(&good[..good.len() - 9]).is_err());
        assert!(philter.set_state(&good[..3]).is_err());
        let mut newer = good.clone();
        newer[..4].copy_from_slice(&0xd0003i32.to_le_bytes());
        assert_eq!(
            philter.set_state(&newer),
            Err(StateError::Version { found: 0xd0003 })
        );
        let mut loud = Saved::new(CHOCOLATE, true);
        loud.params[9] = 161;
        assert_eq!(
            philter.set_state(&state(&[loud], 1)),
            Err(StateError::Range {
                param: 8 + 9,
                value: 161
            })
        );
        assert_eq!(philter, before);
    }

    #[test]
    fn every_state_fl_ships_reads_to_its_last_byte_and_plays() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states of Love Philter skipped");
            return;
        };
        let mut saved = every_preset(&root, NAME);
        let shipped = saved.len();
        let named = |saved: &crate::testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        saved.extend(projects(&root).into_iter().filter(named));
        assert_eq!((shipped, saved.len()), (52, 102));
        let (mut exact, mut oversampled) = (0, 0);
        for saved in saved {
            let mut philter = LovePhilter::default();
            philter.set_sample_rate(48_000.0);
            philter.set_tempo(140.0);
            if let Err(error) = philter.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            exact += usize::from(philter.fidelity() == Fidelity::Exact);
            oversampled += usize::from(philter.oversampling > 0);
            let (mut left, mut right) = (sine(0.01, 9600), sine(0.037, 9600));
            for (l, r) in left.chunks_mut(300).zip(right.chunks_mut(300)) {
                philter.process(l, r);
            }
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 100.0),
                "{}",
                saved.file
            );
        }
        assert_eq!((exact, oversampled), (102, 84));
    }
}
