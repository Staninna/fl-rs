//! Fruity Envelope Controller: a channel whose notes run eight
//! articulators, each an envelope, an LFO and four maps, whose values are
//! eight controller outputs. It has no sound.
//! `docs/plugins/fruity-envelope-controller.md` reads it from
//! `Fruity Envelope Controller_x64.dll`.
//!
//! Parameter `11 a + k` is knob `k` of articulator `a`; 88 and 89 are the
//! Mod X and Mod Y wheels.
//!
//! | k | knob | range |
//! |---|---|---|
//! | 0 | base level, in 256ths | 0 to 256 |
//! | 1 | envelope amount | -256 to 256 |
//! | 2 | LFO amount | -256 to 256 |
//! | 3 to 6 | envelope attack, decay, sustain, release | as Sytrus's |
//! | 7 to 10 | LFO speed, tension, skew, pulse width | as Sytrus's |
//!
//! # Notes and ticks
//!
//! A note (`TriggerVoice`, `0x8af660`) takes every articulator whose key
//! range holds its key from the note that had it, and starts its envelope
//! and LFO. Once a tick (`NewTick`, `0x8afec0`) each articulator a note
//! holds works its value out: its base, plus the envelope times its
//! amount, then the LFO (added, or multiplied in as `lfo * amount * 2 +
//! 1`), the key, Mod X and Mod Y maps, and the velocity and random maps
//! from the note's start, each added in an articulator that sums and
//! multiplied in one that does not. The value, kept within 0 to 1, is the
//! articulator's output, sent where it moved. An articulator whose
//! envelope and LFO have both played out lets its note go and keeps its
//! last output.

use crate::menvelope::{Class, Lfo, LfoRun, MEnvelope, Reader, Run, Track};
use crate::{Fidelity, Generator, NoteOn, StateError, knob_value};
use dsp::curve::{Curve, Point};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Envelope Controller";

const SLOTS: usize = 8;
const KNOBS: usize = 11;
const PARAMS: usize = SLOTS * KNOBS + 2;
/// The maps after the envelope and the LFO: key, velocity, Mod X, Mod Y
/// and random, numbered 2 to 6 as FL numbers an articulator's parts.
const KEY: usize = 2;
const VELOCITY: usize = 3;
const MOD_X: usize = 4;
const MOD_Y: usize = 5;
const RANDOM: usize = 6;
const MAPS: usize = 5;
/// An output not sent since the last flush.
const UNSENT: f32 = -1.0;
/// An output of 1, in the units FL sends.
const FULL: f64 = 1_073_741_824.0;

/// The range of knob `k` of an articulator. The envelope and LFO knobs
/// are not in the editor's form; they are taken to be Sytrus's.
const RANGES: [(i32, i32); KNOBS] = [
    (0, 256),
    (-256, 256),
    (-256, 256),
    (0, 256),
    (0, 256),
    (-128, 128),
    (0, 256),
    (512, 65_536),
    (-128, 128),
    (-128, 128),
    (-112, 112),
];
/// Mod X and Mod Y.
const MOD_RANGE: (i32, i32) = (0, 65_536);
const DEFAULTS: [i32; KNOBS] = [0, 256, 128, 128, 128, 0, 128, 40_000, 0, 0, 0];

/// A `TECArticulator` (`0x8acac0`): one output.
#[derive(Debug, Clone, PartialEq)]
struct Articulator {
    /// It sums (`+8` is 1, "bipolar sum") rather than multiplies.
    sums: bool,
    /// Base level, envelope amount and LFO amount: `+0x20`, `+0x24`,
    /// `+0x28`.
    levels: [f32; 3],
    envelope: Track,
    lfo: Lfo,
    /// The key, velocity, Mod X, Mod Y and random maps; `None` for one at
    /// its default.
    maps: [Option<Curve>; MAPS],
    /// A map that is flat, by its value less the centre.
    flat: [Option<f32>; MAPS],
    /// The parts a note's tick runs through, in order: `+0x60`.
    order: [u8; 8],
    parts: usize,
    /// The Mod X and Mod Y maps that are not flat: `+0x80`.
    idle: [u8; 2],
    idles: usize,
    /// What the output starts from: `+0xa4`, the identity of the way it
    /// combines with the flat Mod X and Mod Y maps in it.
    rest: f32,
    /// The identity `+0xa0`.
    identity: f32,
    /// The keys it takes, `+0x58..=+0x5c`.
    keys: (i32, i32),
    /// Plain envelopes, which [`Track`] runs as FL does.
    plain: bool,
    global: bool,
}

/// The note an articulator is held by, and where it is.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Held {
    voice: u32,
    run: Run,
    lfo: LfoRun,
    /// The velocity and random maps, read when the note started: `+0xd4`.
    fixed: f32,
    /// The note's key, as the key map reads it.
    key: f32,
    mods: [f32; 2],
}

fn idle_envelope(kind: u32, level: f64, knobs: [i32; 4]) -> MEnvelope {
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
    fn new() -> Self {
        let mut articulator = Articulator {
            sums: false,
            levels: [0.0, 1.0, 0.5],
            envelope: Track::new(
                &idle_envelope(1, 1.0, crate::menvelope::ENVELOPE_KNOBS),
                0.0,
            ),
            lfo: Lfo::new(&idle_envelope(2, 1.0, crate::menvelope::LFO_KNOBS)),
            maps: [const { None }; MAPS],
            flat: [None; MAPS],
            order: [0; 8],
            parts: 0,
            idle: [0; 2],
            idles: 0,
            rest: 1.0,
            identity: 1.0,
            keys: (0, 256),
            plain: true,
            global: false,
        };
        articulator.build();
        articulator
    }

    fn centre(&self) -> f64 {
        if self.sums { 0.5 } else { 0.0 }
    }

    fn combine(&self, a: f32, b: f32) -> f32 {
        if self.sums { a + b } else { a * b }
    }

    /// Read one from a stream: its version, its three levels, the
    /// envelope, the LFO and `maps` maps from the key map on (`0x7dfaf0`,
    /// `0x8b1100`).
    fn read(reader: &mut Reader<'_>, sums: bool, maps: usize) -> Result<Self, StateError> {
        let mut articulator = Articulator::new();
        articulator.sums = sums;
        let centre = articulator.centre();
        let _version = reader.i32()?;
        for level in &mut articulator.levels {
            *level = reader.f32()?;
        }
        let envelope = MEnvelope::read(reader, Class::Envelope)?;
        articulator.plain = envelope.plain(Class::Envelope);
        articulator.envelope = Track::new(&envelope, centre);
        let lfo = MEnvelope::read(reader, Class::Lfo)?;
        articulator.global = lfo.active && lfo.flags & 2 != 0;
        articulator.lfo = Lfo::new(&lfo);
        // A map is at its default when it is the identity line of the way
        // the articulator combines: 1 for a product and the centre for a
        // sum.
        let identity = (1.0 - centre) as f32;
        for at in 0..maps {
            let map = MEnvelope::read(reader, Class::Map)?;
            let flat = map.flat();
            if flat == Some(identity) {
                continue;
            }
            articulator.flat[at] = flat.map(|level| (f64::from(level) - centre) as f32);
            articulator.maps[at] = Some(Curve::new(&map.points));
        }
        articulator.build();
        Ok(articulator)
    }

    /// Which parts a tick runs through and what the output starts from:
    /// `0x8b11c0`.
    fn build(&mut self) {
        self.identity = if self.sums { 0.0 } else { 1.0 };
        self.rest = self.identity;
        self.parts = 0;
        if self.envelope.active() {
            self.order[self.parts] = 0;
            self.parts += 1;
        }
        if self.lfo.active() {
            self.order[self.parts] = 1;
            self.parts += 1;
        }
        for part in [KEY, MOD_X, MOD_Y] {
            if self.maps[part - KEY].is_some() {
                self.order[self.parts] = part as u8;
                self.parts += 1;
            }
        }
        self.idles = 0;
        for part in [MOD_X, MOD_Y] {
            match (self.flat[part - KEY], self.maps[part - KEY].is_some()) {
                (Some(level), true) => self.rest = self.combine(self.rest, level),
                (None, true) => {
                    self.idle[self.idles] = part as u8;
                    self.idles += 1;
                }
                // A map at its default is flat at the identity.
                (_, false) => self.rest = self.combine(self.rest, self.identity),
            }
        }
    }

    /// Map `part` at `x`, less its centre: `0x7df500`.
    fn map(&self, part: usize, x: f64) -> f32 {
        match &self.maps[part - KEY] {
            Some(curve) => (curve.value_at(x) - self.centre()) as f32,
            None => (f64::from(1.0 - self.centre() as f32) - self.centre()) as f32,
        }
    }

    /// Set knob `k` from parameter `value` (`ProcessParam`, `0x8ae740`).
    fn set_knob(&mut self, k: usize, knobs: &[i32]) {
        match k {
            0..3 => self.levels[k] = knobs[k] as f32 * 0.003_906_25,
            3..7 => self
                .envelope
                .set_knobs([knobs[3], knobs[4], knobs[5], knobs[6]]),
            _ => self
                .lfo
                .set_knobs([knobs[7], knobs[8], knobs[9], knobs[10]]),
        }
    }
}

/// Fruity Envelope Controller, as a [`Generator`] that makes no sound.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvelopeController {
    params: [i32; PARAMS],
    articulators: [Articulator; SLOTS],
    held: [Option<Held>; SLOTS],
    /// What each output was sent as, `+0xe0`.
    sent: [f32; SLOTS],
    /// The outputs run on without a note, `+0x2a1`.
    free: bool,
    tempo: f32,
    ppq: u16,
    /// Half seconds and beats a tick.
    rates: [f64; 2],
    /// FL's random generator for the random map, `0x2f4`.
    seed: u32,
    outputs: [f32; SLOTS],
    changed: bool,
}

impl Default for EnvelopeController {
    fn default() -> Self {
        let mut params = [0; PARAMS];
        for slot in params.as_chunks_mut::<KNOBS>().0.iter_mut() {
            *slot = DEFAULTS;
        }
        params[88] = 32_768;
        params[89] = 32_768;
        let mut plugin = EnvelopeController {
            params,
            articulators: std::array::from_fn(|_| Articulator::new()),
            held: [None; SLOTS],
            sent: [0.0; SLOTS],
            free: false,
            tempo: 120.0,
            ppq: 96,
            rates: [0.0; 2],
            seed: 0x12b_d6aa,
            outputs: [0.0; SLOTS],
            changed: false,
        };
        plugin.apply();
        plugin.timing();
        plugin
    }
}

impl EnvelopeController {
    /// Every parameter again, as FL's loader does (`0x8ad310`).
    fn apply(&mut self) {
        for (slot, articulator) in self.articulators.iter_mut().enumerate() {
            let knobs = &self.params[slot * KNOBS..(slot + 1) * KNOBS];
            for k in [0, 1, 2, 3, 7] {
                articulator.set_knob(k, knobs);
            }
        }
    }

    /// The rates the envelopes count by: `0x8afde0`, with the tempo from
    /// `FPE_Tempo` and the PPQ from `FPD_SetTimeSig`.
    fn timing(&mut self) {
        let beats = (1.0 / f64::from(self.ppq.max(1))) as f32;
        let seconds = beats * 120.0 / self.tempo;
        self.rates = [f64::from(seconds), f64::from(beats)];
    }

    /// The Mod X or Mod Y wheel, from 0 to 1.
    fn wheel(&self, part: usize) -> f32 {
        (f64::from(self.params[88 + part - MOD_X]) * 1.525_878_906_25e-5) as f32
    }

    /// The next of FL's random numbers, from 0 to 1. FL's generator
    /// (`0x8ab9e0`) is not read; a Park-Miller step stands in.
    fn random(&mut self) -> f64 {
        self.seed = ((u64::from(self.seed) * 48_271) % 0x7fff_ffff) as u32;
        f64::from(self.seed) / f64::from(0x7fff_ffffu32)
    }
}

impl Generator for EnvelopeController {
    /// The state FL's `SaveRestoreState` (`0x8ad310`) writes: a version
    /// (`0x60003`, whose high word counts from FL 6 on), 90 parameters (44
    /// before version 6), the articulator the editor shows, its part, the
    /// option bits, a word for the editor, then for each of the 8
    /// articulators (4 before version 6) its flags (from version 2), its
    /// stream with 5 maps (2 before version 6) and its key range.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let mut reader = Reader::new(bytes);
        let raw = reader.u32()?;
        let version = if raw > 0xffff { raw >> 16 } else { raw } as i32;
        if version > 6 {
            return Err(StateError::Version { found: version });
        }
        let old = version < 6;
        let mut params = Self::default().params;
        let count = if old { 44 } else { 88 + 2 };
        for param in params.iter_mut().take(count) {
            *param = reader.i32()?;
        }
        let _page = reader.i32()?;
        if version > 0 {
            let _part = reader.i32()?;
        }
        let options = reader.u32()?;
        let _editor = reader.i32()?;
        let (slots, maps) = if old { (4, 2) } else { (SLOTS, MAPS) };
        let mut articulators: [Articulator; SLOTS] = std::array::from_fn(|_| Articulator::new());
        for articulator in articulators.iter_mut().take(slots) {
            let flags = if version > 1 { reader.u32()? } else { 0 };
            *articulator = Articulator::read(&mut reader, flags & 1 != 0, maps)?;
            articulator.keys = (reader.i32()?, reader.i32()?);
        }
        reader.end()?;
        for (slot, articulator) in articulators.iter_mut().enumerate() {
            let knobs = &params[slot * KNOBS..(slot + 1) * KNOBS];
            for k in [0, 1, 2, 3, 7] {
                articulator.set_knob(k, knobs);
            }
        }
        self.params = params;
        self.articulators = articulators;
        self.free = options & 4 != 0;
        self.timing();
        self.reset();
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm as f32;
        self.timing();
    }

    fn set_ppq(&mut self, ppq: u16) {
        self.ppq = ppq;
        self.timing();
    }

    fn note_on(&mut self, note: NoteOn) {
        // `FinalLevels.Pitch` in cents from C5.
        let pitch = f32::from(note.key - 60) * 100.0 + note.fine_pitch;
        let mut key = (pitch + 6000.0) * 0.01;
        if key <= 0.0 {
            key = 0.0;
        }
        let at = f64::from(key).round_ties_even() as i32;
        let ceiling = 20.551_182_f32;
        let scaled = f64::from((note.velocity * 10.0).min(ceiling) + 1.0).ln();
        let velocity = (scaled / f64::from(ceiling + 1.0).ln()) as f32 * 0.007_812_5 * 127.0;
        for slot in 0..SLOTS {
            let articulator = &self.articulators[slot];
            if at < articulator.keys.0 || at > articulator.keys.1 {
                continue;
            }
            let rates = self.rates;
            let rate = rates[usize::from(articulator.envelope.tempo)];
            let lfo_rate = rates[usize::from(articulator.lfo.tempo())];
            let mut fixed = articulator.map(VELOCITY, f64::from(velocity));
            if articulator.maps[RANDOM - KEY].is_some() {
                let x = self.random();
                let articulator = &self.articulators[slot];
                fixed = articulator.combine(fixed, articulator.map(RANDOM, x));
            }
            let articulator = &self.articulators[slot];
            self.held[slot] = Some(Held {
                voice: note.voice,
                run: articulator.envelope.start(rate),
                lfo: articulator.lfo.start(lfo_rate),
                fixed,
                key: ((f64::from(pitch) + 6000.0) * 8.333_333_333_333_333e-5) as f32,
                mods: [
                    note.mod_x * 0.5 + self.wheel(MOD_X),
                    note.mod_y * 0.5 + self.wheel(MOD_Y),
                ],
            });
        }
    }

    /// `Voice_Release` (`0x8afca0`) lets go of the envelope and the LFO of
    /// every articulator the note holds.
    fn note_off(&mut self, voice: u32) {
        for (articulator, held) in self.articulators.iter().zip(&mut self.held) {
            let Some(held) = held.as_mut().filter(|held| held.voice == voice) else {
                continue;
            };
            let rate = self.rates[usize::from(articulator.envelope.tempo)];
            articulator.envelope.release(&mut held.run, rate);
            let rate = self.rates[usize::from(articulator.lfo.tempo())];
            articulator.lfo.release(&mut held.lfo, rate);
        }
    }

    fn all_notes_off(&mut self) {
        let voices = self.held.map(|held| held.map(|held| held.voice));
        for voice in voices.into_iter().flatten() {
            self.note_off(voice);
        }
    }

    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        for held in self.held.iter_mut().flatten() {
            if held.voice == voice {
                held.key = ((f64::from(pitch) + 6000.0) * 8.333_333_333_333_333e-5) as f32;
            }
        }
    }

    fn tick(&mut self, _ppq_tick: u64, _tempo: f64) {
        for slot in 0..SLOTS {
            let articulator = &self.articulators[slot];
            let mut value = articulator.rest;
            match &mut self.held[slot] {
                Some(held) => {
                    let mut moving = articulator.levels[0];
                    for part in &articulator.order[..articulator.parts] {
                        let part = usize::from(*part);
                        match part {
                            0 => {
                                let rate = self.rates[usize::from(articulator.envelope.tempo)];
                                articulator.envelope.advance(&mut held.run, 1.0, rate);
                                moving += held.run.out * articulator.levels[1];
                            }
                            1 => {
                                let rate = self.rates[usize::from(articulator.lfo.tempo())];
                                articulator.lfo.advance(&mut held.lfo, 1.0, rate);
                                let lfo = held.lfo.out * articulator.levels[2];
                                moving = if articulator.sums {
                                    moving + lfo
                                } else {
                                    moving * (lfo * 2.0 + 1.0)
                                };
                            }
                            _ => {
                                let x = match part {
                                    KEY => held.key,
                                    _ => held.mods[part - MOD_X],
                                };
                                let mapped = articulator.map(part, f64::from(x));
                                moving = articulator.combine(moving, mapped);
                            }
                        }
                    }
                    moving = articulator.combine(moving, articulator.identity);
                    moving = articulator.combine(moving, held.fixed);
                    value = articulator.combine(value, moving);
                    if held.run.over() && held.lfo.over() {
                        self.held[slot] = None;
                    }
                }
                None if self.free => {
                    value = articulator.combine(value, articulator.levels[0]);
                    for part in &articulator.idle[..articulator.idles] {
                        let part = usize::from(*part);
                        let mapped = articulator.map(part, f64::from(self.wheel(part)));
                        value = articulator.combine(value, mapped);
                    }
                }
                None => continue,
            }
            // FL clamps the bits as an integer: a negative single is 0.
            let bits = value.to_bits() as i32;
            let value = if bits < 0 {
                0.0
            } else if bits <= 0x3f80_0000 {
                value
            } else {
                1.0
            };
            if self.sent[slot] != value {
                self.sent[slot] = value;
                let sent = (f64::from(value) * FULL).round_ties_even();
                self.outputs[slot] = (sent / FULL) as f32;
                self.changed = true;
            }
        }
    }

    fn controller_outputs(&self) -> &[f32] {
        &self.outputs
    }

    fn controller_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// `Gen_Render` is Delphi's abstract method and FL never calls it.
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index >= PARAMS {
            return;
        }
        if index >= SLOTS * KNOBS {
            self.params[index] = knob_value(value, MOD_RANGE);
            return;
        }
        let (slot, k) = (index / KNOBS, index % KNOBS);
        self.params[index] = knob_value(value, RANGES[k]);
        let knobs = &self.params[slot * KNOBS..(slot + 1) * KNOBS];
        self.articulators[slot].set_knob(k, knobs);
    }

    /// FL's flush (`FPD_Flush`, `0x8ace10`): every output is sent again
    /// on the next tick. The notes go too, as the host kills them.
    fn reset(&mut self) {
        self.sent = [UNSENT; SLOTS];
        self.held = [None; SLOTS];
    }

    fn tail(&self) -> usize {
        0
    }

    fn fidelity(&self) -> Fidelity {
        let articulators = self.articulators.iter();
        if articulators.clone().any(|a| !a.plain) {
            Fidelity::Approximate("an envelope's sustain knob is left out")
        } else if articulators.clone().any(|a| a.global) {
            Fidelity::Approximate("a global LFO runs once for every note")
        } else if articulators.clone().any(|a| a.maps[RANDOM - KEY].is_some()) {
            Fidelity::Approximate("the random map draws from a generator of its own")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menvelope::tests::stream;

    fn point(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    /// An articulator's stream: an envelope of these points, an idle LFO
    /// and default maps.
    fn articulator(sums: bool, envelope: &[Point], active: bool, maps: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        if sums {
            bytes.extend(1u32.to_le_bytes());
        } else {
            bytes.extend(0u32.to_le_bytes());
        }
        bytes.extend(3i32.to_le_bytes());
        for level in [0.0f32, 1.0, 0.5] {
            bytes.extend(level.to_le_bytes());
        }
        let block = [0, i32::from(active), -1, -1, -1];
        bytes.extend(stream(1, 3, envelope, block, &[128, 128, 0, 128]));
        let lfo = [point(0.0, 1.0)];
        bytes.extend(stream(
            2,
            3,
            &lfo,
            [0, 0, -1, -1, -1],
            &[40_000, 0, 0, 0, 0],
        ));
        let identity = if sums { 0.5 } else { 1.0 };
        for _ in 0..maps {
            let map = [point(0.0, identity), point(1.0, identity)];
            bytes.extend(stream(3, 3, &map, [0, 0, -1, -1, -1], &[]));
        }
        bytes.extend(0i32.to_le_bytes());
        bytes.extend(256i32.to_le_bytes());
        bytes
    }

    fn state(params: &[i32; PARAMS], slots: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = 0x60003u32.to_le_bytes().to_vec();
        bytes.extend(params.iter().flat_map(|p| p.to_le_bytes()));
        for word in [0i32, 0, 0, 0] {
            bytes.extend(word.to_le_bytes());
        }
        for slot in slots {
            bytes.extend(slot);
        }
        bytes
    }

    fn params(base: i32, amount: i32) -> [i32; PARAMS] {
        let mut params = EnvelopeController::default().params;
        for slot in 0..SLOTS {
            params[slot * KNOBS] = base;
            params[slot * KNOBS + 1] = amount;
        }
        params
    }

    #[test]
    fn a_note_runs_the_envelope_into_the_output() {
        // A ramp from 0 to 1 over one half second, then held at 1.
        let ramp = [point(0.0, 0.0), point(1.0, 1.0)];
        let slots: Vec<_> = (0..SLOTS)
            .map(|slot| articulator(true, &ramp, slot == 0, MAPS))
            .collect();
        let mut plugin = EnvelopeController::default();
        // A base of 0 and the envelope's full amount, summed.
        plugin.set_state(&state(&params(0, 256), &slots)).unwrap();
        plugin.set_tempo(120.0);
        plugin.set_ppq(96);
        plugin.tick(0, 120.0);
        assert!(!plugin.controller_changed(), "nothing moves without a note");
        plugin.note_on(NoteOn::default());
        plugin.tick(0, 120.0);
        // Half a second is 96 ticks at 120 BPM; a tick in, the summing
        // articulator is at 1/96 of the way less its 0.5 centre, plus its
        // velocity map, which is at its default 0, and clamped to 0.
        assert!(plugin.controller_changed());
        assert_eq!(plugin.outputs[0], 0.0);
        for _ in 0..200 {
            plugin.tick(0, 120.0);
        }
        // At the top: 1 less the centre.
        assert!(
            (plugin.outputs[0] - 0.5).abs() < 1e-6,
            "{}",
            plugin.outputs[0]
        );
        // The others run no envelope, and give their base and the identity.
        assert_eq!(plugin.outputs[1], 0.0);
    }

    #[test]
    fn a_product_scales_its_base_by_the_envelope() {
        let ramp = [point(0.0, 0.0), point(1.0, 1.0)];
        let slots: Vec<_> = (0..SLOTS)
            .map(|_| articulator(false, &ramp, true, MAPS))
            .collect();
        let mut plugin = EnvelopeController::default();
        // Base 128/256 plus the envelope at full amount, times a velocity
        // map at its identity 1.
        plugin.set_state(&state(&params(128, 256), &slots)).unwrap();
        plugin.note_on(NoteOn::default());
        // A quarter of the way up after 24 ticks of the 96.
        for _ in 0..24 {
            plugin.tick(0, 120.0);
        }
        assert!(
            (plugin.outputs[0] - 0.75).abs() < 1e-6,
            "{}",
            plugin.outputs[0]
        );
        for _ in 0..24 {
            plugin.tick(0, 120.0);
        }
        assert!((plugin.outputs[0] - 1.0).abs() < 1e-6);
        assert_eq!(plugin.outputs[0], plugin.outputs[7]);
        // A note let go of runs out, and the output stays where it was.
        plugin.note_off(0);
        for _ in 0..100 {
            plugin.tick(0, 120.0);
        }
        assert!(plugin.held.iter().all(Option::is_none));
        assert!((plugin.outputs[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_key_out_of_range_takes_nothing() {
        let ramp = [point(0.0, 0.0), point(1.0, 1.0)];
        let mut slots: Vec<_> = (0..SLOTS)
            .map(|_| articulator(false, &ramp, true, MAPS))
            .collect();
        // Keys 70 to 80 only for the first.
        let len = slots[0].len();
        slots[0][len - 8..len - 4].copy_from_slice(&70i32.to_le_bytes());
        slots[0][len - 4..].copy_from_slice(&80i32.to_le_bytes());
        let mut plugin = EnvelopeController::default();
        plugin.set_state(&state(&params(128, 256), &slots)).unwrap();
        plugin.note_on(NoteOn::default());
        assert!(plugin.held[0].is_none());
        assert!(plugin.held[1].is_some());
    }

    #[test]
    fn parameters_stay_inside_their_wheels() {
        let mut plugin = EnvelopeController::default();
        assert_eq!(plugin.param_count(), 90);
        for (k, (low, high)) in RANGES.iter().enumerate() {
            let index = 3 * KNOBS + k;
            plugin.set_param(index, f32::MIN);
            assert_eq!(plugin.param(index), *low as f32);
            plugin.set_param(index, f32::MAX);
            assert_eq!(plugin.param(index), *high as f32);
        }
        plugin.set_param(89, 1e9);
        assert_eq!(plugin.param(89), 65_536.0);
        assert_eq!(plugin.param(90), 0.0);
    }

    #[test]
    fn a_state_of_another_length_is_refused() {
        let ramp = [point(0.0, 0.0), point(1.0, 1.0)];
        let slots: Vec<_> = (0..SLOTS)
            .map(|_| articulator(false, &ramp, true, MAPS))
            .collect();
        let mut bytes = state(&params(128, 256), &slots);
        let mut plugin = EnvelopeController::default();
        assert!(plugin.set_state(&bytes).is_ok());
        bytes.push(0);
        assert!(plugin.set_state(&bytes).is_err());
        assert!(plugin.set_state(&bytes[..100]).is_err());
    }

    #[test]
    fn every_state_in_fl_s_projects_loads_and_plays() {
        let Some(root) = crate::testing::corpus() else {
            return;
        };
        let saved = crate::testing::projects(&root);
        let mine = saved.iter().filter(|s| s.plugin.eq_ignore_ascii_case(NAME));
        let mut count = 0;
        for saved in mine {
            let mut plugin = EnvelopeController::default();
            plugin.set_state(&saved.state).unwrap();
            plugin.note_on(NoteOn::default());
            for _ in 0..400 {
                plugin.tick(0, 120.0);
                assert!(plugin.outputs.iter().all(|o| (0.0..=1.0).contains(o)));
            }
            plugin.note_off(0);
            for _ in 0..400 {
                plugin.tick(0, 120.0);
            }
            count += 1;
        }
        assert_eq!(count, 4);
    }
}
