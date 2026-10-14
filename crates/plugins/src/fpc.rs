//! FPC: 32 drum pads, each a MIDI note and a stack of velocity layers that
//! each play one sample file, as `docs/plugins/fpc.md` has it.
//!
//! # The state
//!
//! A version and then chunks ([`Chunks`]). FL writes version 0x1e84e403,
//! which it reads as 2000100. One chunk 2 a pad, 32 of them, and after
//! them what the plugin's window was showing. In a pad:
//!
//! ```text
//! chunk 100   16 i32: volume, pan, flags, note, output, cut, cut by, tune, colour, icon
//!             flags: 1 mute, 2 solo, 8 one layer a hit
//! chunk 0x65  the pad's name
//! chunk 0x66  8 i32: how the one layer is picked
//! chunk 0x69  a layer: chunk 300 its sample path, chunk 0x12d 7 i32: volume, pan,
//!             velocity low, velocity high, flags, tune, region; chunk 0x12e ends it
//! chunk 0x67  which envelope the next chunk 0x68 is: 0 volume, 1 pan
//! chunk 0x68  the envelope, as Love Philter and Sytrus save theirs
//! chunk 0x6a  ends the pad
//! ```
//!
//! A state older than version 1000011 has another layout that is not
//! read, and such a state is turned down. FL's files hold none.
//!
//! # The files
//!
//! A layer names its file by path and the state holds no audio. The host
//! decodes: [`Generator::sample_paths`] lists the paths and
//! [`Generator::set_sample`] takes each file. A layer whose file is not
//! there is silent, and [`Generator::fidelity`] says a file is missing.
//!
//! # A note
//!
//! The note's key picks the pads: every pad whose note it is. On each, the
//! layers whose velocity range holds the note's velocity sound, or with
//! "one layer a hit" one of them, round robin or at random. The velocity
//! is the MIDI one, which FL's host works back out of the note's gain.
//!
//! A layer plays its file from the first frame to the last and a note off
//! does not stop it. The pitch is the file's own, moved by the note's fine
//! pitch and the two tunes, 128 to the octave: which pad a note plays says
//! nothing of its pitch. Volumes go through `(11^(1.28 v / 127) - 1) / 10`,
//! so 99 is unity and 127 is 2.05, and the pad's and the layer's each go
//! through FL's pan law with their pan, which is 0.7071 a side in the
//! centre.
//!
//! Every `period` samples, 48 at 44100 Hz, the gains are worked out again
//! and move to where they should be in a straight line. That is how the
//! two envelopes of a pad are heard, and how a cut is: a pad whose "cut by"
//! group is the "cut" group of a pad that is hit fades out over one period.
//! A note off lets the envelopes go.
//!
//! # The parameters
//!
//! | index | parameter | range |
//! |---|---|---|
//! | `0x00 + bank * 0x40 + pad` | volume | 0 to 127 |
//! | `0x10 + bank * 0x40 + pad` | pan | -128 to 128 |
//! | `0x20 + bank * 0x40 + pad` | mute | 0, 1 |
//! | `0x30 + bank * 0x40 + pad` | solo | 0, 1 |
//! | `0x100 + bank * 0x40 + pad` | tune | -128 to 128 |
//!
//! with two banks of 16 pads. A state may hold a volume of 128, which the
//! knob cannot be set to.
//!
//! # What is not FL's
//!
//! - FL's resampler is not read. A file at the song's rate with no tune is
//!   copied frame by frame, as FL does it. Anything else is read through a
//!   4-point Hermite curve, and fidelity says so.
//! - The envelopes run as [`menvelope`](crate::menvelope) runs Sytrus's.
//!   What `TFPCEnvelope` does on top of that is not read, and fidelity
//!   says so for a state that has one switched on.
//! - Every output number plays into the channel. FL sends the outputs
//!   above 0 to other mixer tracks.
//! - A path that is not a file is silent. FL asks its host to find the
//!   file by its name, which brings back the four oldest presets.
//! - The envelope knobs, parameters 0x200 to 0x2ff, cannot be moved.
//! - A note's key is taken as it comes. The engine has moved it by the
//!   channel's root note, and FL picks the pad before that.
//! - The note's velocity is its gain and its pan is [`dsp::pan_linear`],
//!   as for the other generators.
//! - FL has no limit on the layers that sound. Here one past [`VOICES`]
//!   takes the place of the oldest.

use crate::menvelope::{Class, MEnvelope, Reader, Run, Track};
use crate::sampler::{self, Bank, Chunks};
use crate::{Fidelity, Generator, NoteOn, SampleFile, StateError, knob_gain, knob_value};

/// The plugin's event 201 name.
pub const NAME: &str = "FPC";

/// The pads a state has.
pub const PADS: usize = 32;

/// The most layers that sound at once.
pub const VOICES: usize = 128;

/// The version FL 2026 writes, and what it reads it as.
const WRITTEN: i32 = 0x1e84_e403;
/// Above this the strings are UTF-16.
const WIDE: i32 = 2_000_013;
/// Below this the state is the old fixed layout.
const CHUNKED: i32 = 1_000_011;

/// The parameters a host can move: the five knobs of each pad.
const PARAMS: usize = 0x180;

/// The gain of a volume knob: FL's knob curve stretched so that 127 lands
/// where velocity 128 does.
pub fn gain(volume: i32) -> f32 {
    knob_gain(1.28 * f64::from(volume) / 127.0) as f32
}

/// Cents for a tune knob, 128 steps to the octave.
fn cents(tune: i32) -> f64 {
    f64::from(tune) * 9.375
}

/// The control period at a sample rate, in samples: 48 at 44100 Hz, kept a
/// multiple of 4.
pub fn period(rate: f32) -> usize {
    let quarters = (f64::from(rate) * 48.0 / 44_100.0 / 4.0).round_ties_even();
    (quarters as usize * 4).max(4)
}

/// One layer of a pad.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// The sample's path as the state has it, empty for none.
    pub path: String,
    pub volume: i32,
    pub pan: i32,
    /// The layer sounds from this velocity up to the one before `high`,
    /// and at 127 when `high` is 127.
    pub low: i32,
    pub high: i32,
    /// The note's velocity scales the layer's volume.
    pub scaled: bool,
    pub tune: i32,
    /// The file's place in the bank.
    wave: Option<usize>,
}

impl Default for Layer {
    fn default() -> Self {
        Layer {
            path: String::new(),
            volume: 127,
            pan: 0,
            low: 0,
            high: 127,
            scaled: true,
            tune: 0,
            wave: None,
        }
    }
}

impl Layer {
    fn holds(&self, velocity: i32) -> bool {
        self.low <= velocity && (velocity < self.high || (velocity == 127 && self.high == 127))
    }
}

/// One pad.
#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    pub name: String,
    pub volume: i32,
    pub pan: i32,
    pub tune: i32,
    pub mute: bool,
    pub solo: bool,
    /// The MIDI note that hits the pad.
    pub note: i32,
    /// The output the pad plays into, 0 for the channel's own.
    pub output: i32,
    /// A hit on this pad cuts the pads whose `cut_by` is this group. -1
    /// for none.
    pub cut: i32,
    pub cut_by: i32,
    /// One layer a hit, picked by `mode`: 0 at random, 1 at random but not
    /// the one before, 2 the one before the last, 3 the one after it.
    pub pick: bool,
    pub mode: i32,
    pub layers: Vec<Layer>,
    /// The layer the last hit picked.
    last: i32,
    volume_envelope: Track,
    pan_envelope: Track,
    /// Neither envelope asks for what [`Track`] leaves out.
    plain: bool,
}

impl Pad {
    /// The pad as a new FPC has it, but for its note.
    fn new(index: usize) -> Self {
        let flat = MEnvelope {
            kind: 1,
            version: 3,
            points: Vec::new(),
            arp: Vec::new(),
            flags: 0,
            active: false,
            decay: -1,
            loop_start: -1,
            loop_end: -1,
            knobs: crate::menvelope::ENVELOPE_KNOBS,
            phase: 0,
        };
        Pad {
            name: String::new(),
            volume: 127,
            pan: 0,
            tune: 0,
            mute: false,
            solo: false,
            note: index as i32 + 60,
            output: 0,
            cut: -1,
            cut_by: -1,
            pick: false,
            mode: 1,
            layers: Vec::new(),
            last: -1,
            volume_envelope: Track::new(&flat, 0.0),
            pan_envelope: Track::new(&flat, 0.5),
            plain: true,
        }
    }

    /// Read the chunks of one pad.
    fn read(index: usize, bytes: &[u8], wide: bool, bank: &mut Bank) -> Result<Self, StateError> {
        let mut pad = Pad::new(index);
        let mut which = -1;
        for chunk in Chunks::new(bytes) {
            let (id, body) = chunk?;
            match id {
                100 => {
                    let mut words = [0; 16];
                    for (word, value) in words.iter_mut().zip(sampler::words(body)) {
                        *word = value;
                    }
                    let [volume, pan, flags, note, output, cut, cut_by, tune, ..] = words;
                    (pad.volume, pad.pan, pad.tune) = (volume, pan, tune);
                    (pad.note, pad.output) = (note, output);
                    (pad.cut, pad.cut_by) = (cut, cut_by);
                    pad.mute = flags & 1 != 0;
                    pad.solo = flags & 2 != 0;
                    pad.pick = flags & 8 != 0;
                }
                0x65 => pad.name = sampler::text(body, wide),
                0x66 => pad.mode = sampler::words(body).next().unwrap_or(1),
                0x67 => which = sampler::words(body).next().unwrap_or(-1),
                0x68 => {
                    let mut reader = Reader::new(body);
                    let envelope = MEnvelope::read(&mut reader, Class::Envelope)?;
                    reader.end()?;
                    pad.plain &= !envelope.active || envelope.plain(Class::Envelope);
                    match which {
                        0 => pad.volume_envelope = Track::new(&envelope, 0.0),
                        1 => pad.pan_envelope = Track::new(&envelope, 0.5),
                        _ => {}
                    }
                    which = -1;
                }
                0x69 => pad.layers.push(Layer::read(body, wide, bank)?),
                _ => {}
            }
        }
        Ok(pad)
    }

    /// The layers a hit at this velocity sounds: all of them, or the one
    /// picked when the pad has "one layer a hit" on. The range is by index,
    /// and each layer in it still has to hold the velocity.
    fn hit(&mut self, velocity: i32, random: &mut u32) -> std::ops::Range<usize> {
        let all = 0..self.layers.len();
        if !self.pick || self.layers.len() < 2 {
            return all;
        }
        let holds = |layer: &&Layer| layer.holds(velocity);
        let count = self.layers.iter().filter(holds).count();
        // The place among the layers in range of the one picked last.
        let in_range = |index: &usize| self.layers[*index].holds(velocity);
        let last = (0..self.layers.len())
            .filter(in_range)
            .position(|index| index as i32 == self.last);
        let mut draw = |of: usize| {
            // A linear congruential step, as good as FL's own for this.
            *random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((u64::from(*random) * of as u64) >> 32) as usize
        };
        let place = match (count, self.mode) {
            (0, _) => return all,
            (1, _) => Some(0),
            (_, 0) => Some(draw(count)),
            (_, 1) => {
                let drawn = draw(count - 1);
                match last {
                    Some(last) if last <= drawn && last < count - 1 => Some(drawn + 1),
                    _ => Some(drawn),
                }
            }
            (_, 2) => Some(last.filter(|last| *last > 0).map_or(count - 1, |l| l - 1)),
            (_, 3) => Some(last.map_or(0, |last| (last + 1) % count)),
            // A mode FL does not have keeps the layer it had.
            _ => last,
        };
        let Some(place) = place else {
            return all;
        };
        let picked = (0..self.layers.len()).filter(in_range).nth(place);
        let picked = picked.unwrap_or(0);
        self.last = picked as i32;
        picked..picked + 1
    }
}

impl Layer {
    fn read(bytes: &[u8], wide: bool, bank: &mut Bank) -> Result<Self, StateError> {
        let mut layer = Layer::default();
        for chunk in Chunks::new(bytes) {
            let (id, body) = chunk?;
            match id {
                300 => layer.path = sampler::text(body, wide),
                0x12d => {
                    let mut words = [0; 7];
                    for (word, value) in words.iter_mut().zip(sampler::words(body)) {
                        *word = value;
                    }
                    let [volume, pan, low, high, flags, tune, _] = words;
                    (layer.volume, layer.pan, layer.tune) = (volume, pan, tune);
                    (layer.low, layer.high) = (low, high);
                    layer.scaled = flags & 1 != 0;
                }
                _ => {}
            }
        }
        layer.wave = bank.place(&layer.path);
        Ok(layer)
    }
}

/// One layer that is sounding.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    /// The host's number for the note.
    note: u32,
    pad: usize,
    layer: usize,
    wave: usize,
    /// The source frame the next sample reads, and how far it moves on.
    pos: f64,
    step: f64,
    /// The gains that stay: the note's and the pad's, a side.
    fixed: (f32, f32),
    volume: Option<Run>,
    pan: Option<Run>,
    /// The gains now, where they are headed and what a sample adds.
    now: (f32, f32),
    target: (f32, f32),
    slope: (f32, f32),
    /// Samples left of the period, 0 for a voice that has not worked its
    /// gains out yet.
    left: usize,
    started: bool,
    /// Cut by another pad.
    cut: bool,
    /// Its volume envelope has played out.
    over: bool,
    /// Hit since the last block, so that the hit it came with does not cut
    /// it.
    fresh: bool,
}

/// The gains of a run of frames that moves from one gain to another, as
/// the host's SSE routine (0x0062f6f0) works them out. It takes two frames
/// at a time, the second a slope ahead of the first, and adds two slopes
/// to both after each pair. The frames past the last whole group of eight
/// go one at a time from the first lane, a slope each.
struct Ramp {
    even: (f32, f32),
    odd: (f32, f32),
    slope: (f32, f32),
    /// Frames left in pairs, and which of a pair is next.
    paired: usize,
    second: bool,
}

impl Ramp {
    fn new(start: (f32, f32), slope: (f32, f32), len: usize) -> Self {
        Ramp {
            even: start,
            odd: (start.0 + slope.0, start.1 + slope.1),
            slope,
            paired: len & !7,
            second: false,
        }
    }

    fn gains(&mut self) -> (f32, f32) {
        if self.paired == 0 {
            let gains = self.even;
            self.even = (self.even.0 + self.slope.0, self.even.1 + self.slope.1);
            return gains;
        }
        self.paired -= 1;
        if !self.second {
            self.second = true;
            return self.even;
        }
        self.second = false;
        let gains = self.odd;
        let twice = (self.slope.0 + self.slope.0, self.slope.1 + self.slope.1);
        self.even = (self.even.0 + twice.0, self.even.1 + twice.1);
        self.odd = (self.odd.0 + twice.0, self.odd.1 + twice.1);
        gains
    }
}

/// FPC.
#[derive(Debug, Clone, PartialEq)]
pub struct Fpc {
    pads: Vec<Pad>,
    bank: Bank,
    rate: f32,
    period: usize,
    voices: Vec<Voice>,
    random: u32,
    /// The most frames a file plays for at this rate.
    longest: usize,
}

impl Default for Fpc {
    fn default() -> Self {
        let mut pads: Vec<Pad> = (0..PADS).map(Pad::new).collect();
        for pad in &mut pads {
            pad.layers.push(Layer::default());
        }
        let rate = 44_100.0;
        Fpc {
            pads,
            bank: Bank::default(),
            rate,
            period: period(rate),
            voices: Vec::with_capacity(VOICES),
            random: 0x2545_f491,
            longest: 0,
        }
    }
}

impl Fpc {
    /// The 32 pads.
    pub fn pads(&self) -> &[Pad] {
        &self.pads
    }

    /// `x` units of an envelope a control period: an `x` is half a second.
    fn pace(&self) -> f64 {
        2.0 * self.period as f64 / f64::from(self.rate)
    }

    /// The pad and the knob of a parameter.
    fn knob(index: usize) -> Option<(usize, usize)> {
        let (kind, rest) = match index {
            0..0x80 => ((index & 0x3f) >> 4, index),
            0x100..0x180 => (4, index - 0x100),
            _ => return None,
        };
        // The second half of each bank's 64 is the tune's, and nothing's
        // under 0x100.
        if (rest & 0x3f) >= 0x10 && kind == 4 {
            return None;
        }
        Some(((rest >> 6) * 16 + (rest & 0xf), kind))
    }

    /// Work out again how long the longest file plays for.
    fn measure(&mut self) {
        let mut longest = 0f64;
        for pad in &self.pads {
            for layer in &pad.layers {
                let Some(wave) = layer.wave.and_then(|place| self.bank.wave(place)) else {
                    continue;
                };
                let step = sampler::step(cents(layer.tune + pad.tune), wave.rate, self.rate);
                longest = longest.max(wave.len() as f64 / step);
            }
        }
        self.longest = longest.ceil() as usize + self.period;
    }
}

/// The MIDI velocity FL's host makes of a note's gain: the inverse of its
/// velocity curve, where a gain of 1 is 100.
fn midi_velocity(gain: f32) -> i32 {
    let velocity = (10.0 * f64::from(gain.max(0.0)) + 1.0).ln() / 11f64.ln() * 100.0;
    (velocity.round_ties_even() as i32).clamp(0, 127)
}

impl Generator for Fpc {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let Some((word, rest)) = bytes.split_first_chunk::<4>() else {
            return Err(StateError::Length {
                expected: 4,
                found: bytes.len(),
            });
        };
        let written = i32::from_le_bytes(*word);
        if written > WRITTEN {
            return Err(StateError::Version { found: written });
        }
        let version = if written > 0x1f_ffff {
            written >> 8
        } else {
            written
        };
        if version < CHUNKED {
            return Err(StateError::Version { found: written });
        }
        let wide = version > WIDE;
        let mut bank = Bank::default();
        let mut pads = Vec::with_capacity(PADS);
        for chunk in Chunks::new(rest) {
            let (id, body) = chunk?;
            if id == 2 && pads.len() < PADS {
                pads.push(Pad::read(pads.len(), body, wide, &mut bank)?);
            }
        }
        for index in pads.len()..PADS {
            pads.push(Pad::new(index));
        }
        for pad in &mut pads {
            if pad.layers.is_empty() {
                pad.layers.push(Layer::default());
            }
        }
        self.pads = pads;
        self.bank = bank;
        self.voices.clear();
        self.measure();
        Ok(())
    }

    /// The notes that are sounding keep the step they have.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.period = period(hz);
        self.measure();
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let velocity = midi_velocity(note.velocity);
        let key = i32::from(note.key);
        let any_solo = self.pads.iter().any(|pad| pad.solo);
        let pace = self.pace();
        let (left, right) = dsp::pan_linear(note.pan);
        let first = self.voices.len();
        // The pads this note hit, for the cut groups.
        let mut hit = [false; PADS];
        for (index, pad) in self.pads.iter_mut().enumerate() {
            let muted = (pad.mute || any_solo) && !pad.solo;
            if pad.note != key || muted {
                continue;
            }
            let pad_gains = sampler::circular(pad.pan as f32 / 128.0, gain(pad.volume));
            for place in pad.hit(velocity, &mut self.random) {
                let layer = &pad.layers[place];
                let Some(wave) = layer.wave else {
                    continue;
                };
                let Some(file) = self.bank.wave(wave).filter(|_| layer.holds(velocity)) else {
                    continue;
                };
                // The velocity is in the note's gain, and a layer that
                // does not scale takes it back out.
                let level = if layer.scaled { note.velocity } else { 1.0 };
                let tuned = f64::from(note.fine_pitch) + cents(layer.tune + pad.tune);
                if self.voices.len() == VOICES {
                    self.voices.remove(0);
                }
                let active = |track: &Track| track.active().then(|| track.start(pace));
                self.voices.push(Voice {
                    note: note.voice,
                    pad: index,
                    layer: place,
                    wave,
                    pos: 0.0,
                    step: sampler::step(tuned, file.rate, self.rate),
                    fixed: (level * left * pad_gains.0, level * right * pad_gains.1),
                    volume: active(&pad.volume_envelope),
                    pan: active(&pad.pan_envelope),
                    now: (0.0, 0.0),
                    target: (0.0, 0.0),
                    slope: (0.0, 0.0),
                    left: 0,
                    started: false,
                    cut: false,
                    over: false,
                    fresh: true,
                });
                hit[index] = true;
            }
        }
        // A voice that was removed for room has moved the others down.
        let first = first.min(self.voices.len());
        for voice in &mut self.voices[..first] {
            let by = self.pads[voice.pad].cut_by;
            let cuts = |(index, was_hit): (usize, &bool)| *was_hit && self.pads[index].cut == by;
            if by != -1 && !voice.fresh && hit.iter().enumerate().any(cuts) {
                voice.cut = true;
            }
        }
    }

    /// The envelopes of the note's layers are let go. A layer with none
    /// plays its file out.
    fn note_off(&mut self, voice: u32) {
        let pace = self.pace();
        for sounding in self.voices.iter_mut().filter(|v| v.note == voice) {
            let pad = &self.pads[sounding.pad];
            if let Some(run) = &mut sounding.volume {
                pad.volume_envelope.release(run, pace);
            }
            if let Some(run) = &mut sounding.pan {
                pad.pan_envelope.release(run, pace);
            }
        }
    }

    fn all_notes_off(&mut self) {
        let pace = self.pace();
        for sounding in &mut self.voices {
            let pad = &self.pads[sounding.pad];
            if let Some(run) = &mut sounding.volume {
                pad.volume_envelope.release(run, pace);
            }
            if let Some(run) = &mut sounding.pan {
                pad.pan_envelope.release(run, pace);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let pace = self.pace();
        let Fpc {
            pads,
            bank,
            voices,
            period,
            ..
        } = self;
        for voice in voices.iter_mut() {
            voice.fresh = false;
            let pad = &pads[voice.pad];
            let (Some(layer), Some(wave)) = (pad.layers.get(voice.layer), bank.wave(voice.wave))
            else {
                voice.pos = f64::INFINITY;
                continue;
            };
            let frames = wave.len() as f64;
            let mut at = 0;
            while at < len && voice.pos < frames {
                if voice.left == 0 {
                    voice.left = *period;
                    let mut level = 1.0;
                    if let Some(run) = &mut voice.volume {
                        level = run.out;
                        pad.volume_envelope.advance(run, 1.0, pace);
                        if run.over() {
                            voice.over = voice.pan.is_none_or(|pan| pan.over());
                        }
                    }
                    let mut pan = layer.pan as f32 / 128.0;
                    if let Some(run) = &mut voice.pan {
                        pan += run.out * 2.0;
                        pad.pan_envelope.advance(run, 1.0, pace);
                    }
                    let own = sampler::circular(pan, gain(layer.volume));
                    voice.target = (voice.fixed.0 * own.0 * level, voice.fixed.1 * own.1 * level);
                    if !voice.started {
                        voice.started = true;
                        voice.now = voice.target;
                    }
                    if voice.cut || voice.over {
                        voice.target = (0.0, 0.0);
                    }
                    let steps = *period as f32;
                    voice.slope = (
                        (voice.target.0 - voice.now.0) / steps,
                        (voice.target.1 - voice.now.1) / steps,
                    );
                }
                // FL reads the block's frames first, so a run is cut short
                // where the file ends.
                let mut run = 0;
                let mut pos = voice.pos;
                while run < voice.left.min(len - at) && pos < frames {
                    pos += voice.step;
                    run += 1;
                }
                // FL looks at the left side's slope alone to tell a ramp
                // from a block at one gain.
                let ramped = voice.slope.0 != 0.0;
                let mut ramp = Ramp::new(voice.now, voice.slope, run);
                for n in at..at + run {
                    let (l, r) = wave.frame(voice.pos);
                    let gains = if ramped { ramp.gains() } else { voice.target };
                    left[n] += l * gains.0;
                    right[n] += r * gains.1;
                    voice.pos += voice.step;
                }
                if ramped {
                    voice.now.0 += voice.slope.0 * run as f32;
                    voice.now.1 += voice.slope.1 * run as f32;
                }
                voice.left -= run;
                at += run;
                let silent = voice.now.0.abs() < 1e-9 && voice.now.1.abs() < 1e-9;
                if (voice.cut || voice.over) && silent {
                    voice.pos = f64::INFINITY;
                }
            }
        }
        voices.retain(|voice| {
            let frames = bank.wave(voice.wave).map_or(0, |wave| wave.len());
            voice.pos < frames as f64
        });
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        let Some((pad, kind)) = Self::knob(index) else {
            return 0.0;
        };
        let pad = &self.pads[pad];
        let value = match kind {
            0 => pad.volume,
            1 => pad.pan,
            2 => i32::from(pad.mute),
            3 => i32::from(pad.solo),
            _ => pad.tune,
        };
        value as f32
    }

    /// The layers that are sounding keep the gains they started with.
    fn set_param(&mut self, index: usize, value: f32) {
        let Some((pad, kind)) = Self::knob(index) else {
            return;
        };
        let pad = &mut self.pads[pad];
        match kind {
            0 => pad.volume = knob_value(value, (0, 127)),
            1 => pad.pan = knob_value(value, (-128, 128)),
            2 => pad.mute = knob_value(value, (0, 1)) == 1,
            3 => pad.solo = knob_value(value, (0, 1)) == 1,
            _ => pad.tune = knob_value(value, (-128, 128)),
        }
        if kind == 4 {
            self.measure();
        }
    }

    fn reset(&mut self) {
        self.voices.clear();
    }

    /// As long as the longest file plays: a note off stops no layer.
    fn tail(&self) -> usize {
        self.longest.max(self.period)
    }

    fn fidelity(&self) -> Fidelity {
        if self.bank.lacking() {
            return Fidelity::Approximate("FPC: a sample file is missing, and its layer is silent");
        }
        let sounding = |pad: &&Pad| pad.layers.iter().any(|layer| layer.wave.is_some());
        let mut pads = self.pads.iter().filter(sounding);
        let tuned = |pad: &Pad| pad.tune != 0 || pad.layers.iter().any(|layer| layer.tune != 0);
        if self.bank.resampled(self.rate) || pads.clone().any(tuned) {
            return Fidelity::Approximate("FPC: a Hermite curve stands in for FL's resampler");
        }
        let enveloped =
            |pad: &Pad| pad.volume_envelope.active() || pad.pan_envelope.active() || !pad.plain;
        if pads.clone().any(enveloped) {
            return Fidelity::Approximate("FPC: its pad envelopes run as Sytrus's do");
        }
        if pads.any(|pad| pad.output != 0) {
            return Fidelity::Approximate("FPC: every output plays into the channel");
        }
        Fidelity::Exact
    }

    fn sample_paths(&self) -> Vec<String> {
        self.bank.paths()
    }

    fn set_sample(&mut self, path: &str, file: Option<SampleFile<'_>>) {
        self.bank.set(path, file);
        self.measure();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::menvelope::tests::stream;
    use crate::testing;
    use dsp::curve::Point;
    use std::f32::consts::FRAC_1_SQRT_2;

    pub(crate) fn chunk(id: i32, body: &[u8]) -> Vec<u8> {
        let mut bytes = id.to_le_bytes().to_vec();
        bytes.extend((body.len() as i64).to_le_bytes());
        bytes.extend(body);
        bytes
    }

    fn wide(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    /// A layer as a state has one: its path and its seven numbers.
    fn layer(path: &str, numbers: [i32; 7]) -> Vec<u8> {
        let mut body = chunk(300, &wide(path));
        body.extend(chunk(0x12d, &testing::state(&numbers)));
        body.extend(chunk(0x12e, &[]));
        chunk(0x69, &body)
    }

    /// A layer at unity with the whole velocity range.
    fn plain(path: &str) -> Vec<u8> {
        layer(path, [99, 0, 0, 127, 1, 0, -1])
    }

    /// An envelope of these points, switched on or off, with a point it
    /// holds at when `held` names one.
    fn envelope(points: &[(f64, f64)], active: bool, held: i32) -> Vec<u8> {
        let points: Vec<Point> = points
            .iter()
            .map(|(x, y)| Point {
                x: *x,
                y: *y,
                tension: 0.0,
                mode: 0,
            })
            .collect();
        let block = [0, -i32::from(active), -1, -1, held];
        stream(1, 3, &points, block, &[128, 128, 0, 128])
    }

    /// A pad of these ten numbers and layers, with the two envelopes.
    fn pad(numbers: [i32; 10], layers: &[Vec<u8>], volume: &[u8], pan: &[u8]) -> Vec<u8> {
        let mut words = [0; 16];
        words[..10].copy_from_slice(&numbers);
        let mut body = chunk(100, &testing::state(&words));
        body.extend(chunk(0x65, &wide("Pad")));
        body.extend(chunk(
            0x66,
            &testing::state(&[numbers[2] >> 8, 0, 0, 0, 0, 0, 0, 0]),
        ));
        for layer in layers {
            body.extend(layer);
        }
        body.extend(chunk(0x67, &testing::state(&[0, 0])));
        body.extend(chunk(0x68, volume));
        body.extend(chunk(0x67, &testing::state(&[1, 0])));
        body.extend(chunk(0x68, pan));
        body.extend(chunk(0x6a, &[]));
        chunk(2, &body)
    }

    /// A pad at unity on this note, with these layers and no envelope.
    fn simple(note: i32, layers: &[Vec<u8>]) -> Vec<u8> {
        let numbers = [99, 0, 0, note, 0, -1, -1, 0, 0, 0];
        pad(numbers, layers, &off(), &centre())
    }

    fn off() -> Vec<u8> {
        envelope(&[(0.0, 1.0), (1.25, 0.0)], false, -1)
    }

    fn centre() -> Vec<u8> {
        envelope(&[(0.0, 0.5)], false, -1)
    }

    /// A state of these pads, as FL 2026 writes one.
    fn state(pads: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = WRITTEN.to_le_bytes().to_vec();
        for pad in pads {
            bytes.extend(pad);
        }
        for index in pads.len()..PADS {
            bytes.extend(simple(index as i32 + 100, &[plain("")]));
        }
        bytes.extend(chunk(1, &wide("loop.mid")));
        bytes.extend(chunk(3, &[1]));
        bytes.extend(chunk(4, &[0; 0x20]));
        bytes
    }

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

    fn file(left: &[f32], rate: u32) -> SampleFile<'_> {
        SampleFile {
            left,
            right: None,
            rate,
        }
    }

    /// A ramp that never repeats, so that a frame says where it is from.
    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|n| (n as f32 + 1.0) / len as f32).collect()
    }

    fn kit(pads: &[Vec<u8>]) -> Fpc {
        let mut fpc = Fpc::default();
        fpc.set_state(&state(pads)).unwrap();
        fpc
    }

    fn play(fpc: &mut Fpc, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        fpc.process(&mut left, &mut right);
        (left, right)
    }

    /// What unity is: a pad and a layer at 99 in the centre, each through
    /// FL's pan law.
    fn unity() -> f32 {
        let one = gain(99) * FRAC_1_SQRT_2;
        one * one
    }

    #[test]
    fn a_pad_plays_its_file_frame_by_frame_whatever_its_note() {
        let sound = ramp(300);
        for key in [36, 60, 101] {
            let mut fpc = kit(&[simple(i32::from(key), &[plain("kick.wav")])]);
            assert_eq!(fpc.sample_paths(), ["kick.wav"]);
            fpc.set_sample("kick.wav", Some(file(&sound, 44_100)));
            assert_eq!(fpc.fidelity(), Fidelity::Exact);
            fpc.note_on(note(1, key));
            // A note off does not stop it.
            fpc.note_off(1);
            let (left, right) = play(&mut fpc, 400);
            for (n, frame) in sound.iter().enumerate() {
                let want = frame * unity();
                assert!((left[n] - want).abs() < 1e-6, "{key} {n}");
                assert_eq!(left[n], right[n]);
            }
            assert!(left[300..].iter().all(|v| *v == 0.0));
            assert!(fpc.voices.is_empty());
            // Another key hits nothing.
            fpc.note_on(note(2, key + 1));
            assert!(play(&mut fpc, 64).0.iter().all(|v| *v == 0.0));
            assert!(fpc.tail() >= 300);
        }
        assert!((gain(99) - 0.9936).abs() < 1e-3 && (gain(127) - 2.0527).abs() < 1e-3);
        assert_eq!(gain(0), 0.0);
    }

    #[test]
    fn the_fine_pitch_the_tunes_and_the_file_s_rate_set_the_step() {
        let sound = ramp(512);
        // An octave of fine pitch reads every other frame.
        let mut fpc = kit(&[simple(60, &[plain("a.wav")])]);
        fpc.set_sample("a.wav", Some(file(&sound, 44_100)));
        let mut high = note(1, 60);
        high.fine_pitch = 1200.0;
        fpc.note_on(high);
        let (left, _) = play(&mut fpc, 300);
        for n in 0..256 {
            assert!((left[n] - sound[2 * n] * unity()).abs() < 1e-6, "{n}");
        }
        assert!(left[256..].iter().all(|v| *v == 0.0));

        // A pad tune of 64 and a layer tune of 64 are an octave, and a
        // file at half the rate is an octave down again.
        let tuned = pad(
            [99, 0, 0, 60, 0, -1, -1, 64, 0, 0],
            &[layer("a.wav", [99, 0, 0, 127, 1, 64, -1])],
            &off(),
            &centre(),
        );
        let mut fpc = kit(&[tuned]);
        fpc.set_sample("a.wav", Some(file(&sound, 22_050)));
        assert!(matches!(fpc.fidelity(), Fidelity::Approximate(what) if what.contains("Hermite")));
        fpc.note_on(note(1, 60));
        let (left, _) = play(&mut fpc, 512);
        for n in 0..512 {
            assert!((left[n] - sound[n] * unity()).abs() < 1e-6, "{n}");
        }

        // Between the frames the curve goes through them.
        let mut fpc = fpc_at(&sound, 22_050, 44_100.0);
        fpc.note_on(note(1, 60));
        let (left, _) = play(&mut fpc, 1000);
        for n in 1..500 {
            assert!((left[2 * n] - sound[n] * unity()).abs() < 1e-6, "{n}");
            let between = (sound[n] + sound[n + 1]) / 2.0 * unity();
            assert!((left[2 * n + 1] - between).abs() < 1e-4, "{n}");
        }
        // And the song's rate counts as the file's does.
        let mut fpc = fpc_at(&sound, 48_000, 48_000.0);
        assert_eq!(fpc.fidelity(), Fidelity::Exact);
        fpc.note_on(note(1, 60));
        assert!((play(&mut fpc, 8).0[7] - sound[7] * unity()).abs() < 1e-6);
        assert_eq!(
            (period(44_100.0), period(48_000.0), period(8000.0)),
            (48, 52, 8)
        );
        assert_eq!(period(100.0), 4);
    }

    /// One pad on key 60 with this file, at this song rate.
    fn fpc_at(sound: &[f32], file_rate: u32, rate: f32) -> Fpc {
        let mut fpc = kit(&[simple(60, &[plain("a.wav")])]);
        fpc.set_sample_rate(rate);
        fpc.set_sample("a.wav", Some(file(sound, file_rate)));
        fpc
    }

    #[test]
    fn the_velocity_picks_the_layer_and_scales_it_when_the_layer_says_so() {
        let soft = layer("soft.wav", [99, 0, 0, 64, 1, 0, -1]);
        let hard = layer("hard.wav", [99, 0, 64, 127, 0, 0, -1]);
        let mut fpc = kit(&[simple(60, &[soft, hard])]);
        fpc.set_sample("soft.wav", Some(file(&[1.0; 64], 44_100)));
        fpc.set_sample("hard.wav", Some(file(&[-1.0; 64], 44_100)));
        // The gain FL's curve gives a velocity, and the velocity back.
        let curve = |velocity: f64| knob_gain(velocity / 100.0) as f32;
        for (velocity, midi) in [(1, 1), (63, 63), (64, 64), (100, 100), (127, 127)] {
            assert_eq!(midi_velocity(curve(f64::from(velocity))), midi);
        }
        assert_eq!(midi_velocity(1.0), 100);
        assert_eq!((midi_velocity(0.0), midi_velocity(9.0)), (0, 127));

        let mut quiet = note(1, 60);
        quiet.velocity = curve(63.0);
        fpc.note_on(quiet);
        let (left, _) = play(&mut fpc, 64);
        assert!((left[0] - curve(63.0) * unity()).abs() < 1e-6);
        // The hard layer does not scale: its velocity is taken back out.
        let mut loud = note(2, 60);
        loud.velocity = curve(64.0);
        fpc.note_on(loud);
        assert!((play(&mut fpc, 64).0[0] + unity()).abs() < 1e-6);
        // 127 is in a range that ends on 127.
        let mut top = note(3, 60);
        top.velocity = curve(127.0);
        fpc.note_on(top);
        assert!((play(&mut fpc, 64).0[0] + unity()).abs() < 1e-6);
        // A slide note sounds nothing.
        let mut slide = note(4, 60);
        slide.slide = true;
        fpc.note_on(slide);
        assert!(play(&mut fpc, 64).0.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn one_layer_a_hit_goes_round_the_layers() {
        let layers: Vec<Vec<u8>> = ["a", "b", "c"].iter().map(|name| plain(name)).collect();
        for (mode, order) in [(3, [1.0, 2.0, 3.0, 1.0]), (2, [3.0, 2.0, 1.0, 3.0])] {
            // Flag 8, and the mode in the pad's chunk 0x66.
            let numbers = [99, 0, 8 | mode << 8, 60, 0, -1, -1, 0, 0, 0];
            let mut bytes = pad(numbers, &layers, &off(), &centre());
            // The flags word holds the flags alone.
            let at = 12 + 12 + 8;
            bytes[at..at + 4].copy_from_slice(&8i32.to_le_bytes());
            let mut fpc = kit(&[bytes]);
            assert!(fpc.pads()[0].pick);
            assert_eq!(fpc.pads()[0].mode, mode);
            for (value, name) in [(1.0, "a"), (2.0, "b"), (3.0, "c")] {
                fpc.set_sample(name, Some(file(&[value; 8], 44_100)));
            }
            for (voice, want) in order.iter().enumerate() {
                fpc.note_on(note(voice as u32, 60));
                let heard = play(&mut fpc, 8).0[0] / unity();
                assert!((heard - want).abs() < 1e-5, "{mode} {voice}: {heard}");
            }
        }
        // At random, every hit is one layer, and not the one before.
        let numbers = [99, 0, 8, 60, 0, -1, -1, 0, 0, 0];
        let mut bytes = pad(numbers, &layers, &off(), &centre());
        let at = 12 + 12 + 0x40 + 12 + 6 + 12;
        bytes[at..at + 4].copy_from_slice(&1i32.to_le_bytes());
        let mut fpc = kit(&[bytes]);
        assert_eq!(fpc.pads()[0].mode, 1);
        for (value, name) in [(1.0, "a"), (2.0, "b"), (3.0, "c")] {
            fpc.set_sample(name, Some(file(&[value; 8], 44_100)));
        }
        let mut before = 0.0;
        let mut seen = [false; 3];
        for voice in 0..40 {
            fpc.note_on(note(voice, 60));
            let heard = (play(&mut fpc, 8).0[0] / unity()).round();
            assert!((1.0..=3.0).contains(&heard) && heard != before, "{heard}");
            seen[heard as usize - 1] = true;
            before = heard;
        }
        assert_eq!(seen, [true; 3]);
    }

    #[test]
    fn a_hit_cuts_the_pads_that_its_group_cuts_over_one_period() {
        // The open hat is cut by group 0, and the closed hat cuts group 0.
        let open = pad(
            [99, 0, 0, 46, 0, -1, 0, 0, 0, 0],
            &[plain("open.wav")],
            &off(),
            &centre(),
        );
        let closed = pad(
            [99, 0, 0, 42, 0, 0, -1, 0, 0, 0],
            &[plain("closed.wav")],
            &off(),
            &centre(),
        );
        let mut fpc = kit(&[open, closed]);
        fpc.set_sample("open.wav", Some(file(&[1.0; 1000], 44_100)));
        fpc.set_sample("closed.wav", Some(file(&[0.0; 1000], 44_100)));
        fpc.note_on(note(1, 46));
        let (left, _) = play(&mut fpc, 100);
        assert!(left.iter().all(|v| (v - unity()).abs() < 1e-6));
        fpc.note_on(note(2, 42));
        // 100 samples in, the open hat is 4 samples into its third period:
        // it plays that out, and the next one takes it to nothing.
        let (left, _) = play(&mut fpc, 200);
        assert!(left[..44].iter().all(|v| (v - unity()).abs() < 1e-6));
        for n in 0..48 {
            let want = unity() * (1.0 - n as f32 / 48.0);
            assert!((left[44 + n] - want).abs() < 1e-5, "{n}: {}", left[44 + n]);
        }
        assert!(left[92..].iter().all(|v| v.abs() < 1e-6));
        assert_eq!(fpc.voices.len(), 1);
        // The closed hat does not cut itself, and the open one cuts
        // nothing.
        fpc.note_on(note(3, 42));
        fpc.note_on(note(4, 46));
        play(&mut fpc, 300);
        assert_eq!(fpc.voices.len(), 3);
    }

    #[test]
    fn mute_and_solo_say_which_pads_a_note_hits() {
        let pads = |first: i32, second: i32| {
            let one = pad(
                [99, 0, first, 60, 0, -1, -1, 0, 0, 0],
                &[plain("one")],
                &off(),
                &centre(),
            );
            let two = pad(
                [99, 0, second, 60, 0, -1, -1, 0, 0, 0],
                &[plain("two")],
                &off(),
                &centre(),
            );
            let mut fpc = kit(&[one, two]);
            fpc.set_sample("one", Some(file(&[1.0; 8], 44_100)));
            fpc.set_sample("two", Some(file(&[2.0; 8], 44_100)));
            fpc.note_on(note(1, 60));
            (play(&mut fpc, 8).0[0] / unity()).round()
        };
        // Two pads on one note both sound.
        assert_eq!(pads(0, 0), 3.0);
        assert_eq!(pads(1, 0), 2.0);
        assert_eq!(pads(0, 2), 2.0);
        // A pad that is both plays, as in FL.
        assert_eq!(pads(3, 0), 1.0);
        assert_eq!(pads(1, 1), 0.0);
    }

    #[test]
    fn the_pans_go_through_fl_s_law_and_the_note_s_own_is_linear() {
        let sided = pad(
            [99, 128, 0, 60, 0, -1, -1, 0, 0, 0],
            &[layer("a", [99, -128, 0, 127, 1, 0, -1])],
            &off(),
            &centre(),
        );
        let mut fpc = kit(&[sided]);
        fpc.set_sample("a", Some(file(&[1.0; 8], 44_100)));
        fpc.note_on(note(1, 60));
        // The pad is hard right and the layer hard left: nothing.
        let (left, right) = play(&mut fpc, 8);
        assert!(left[0].abs() < 1e-6 && right[0].abs() < 1e-6);

        let mut fpc = fpc_at(&[1.0; 8], 44_100, 44_100.0);
        let mut panned = note(1, 60);
        (panned.pan, panned.velocity) = (-1.0, 0.5);
        fpc.note_on(panned);
        let (left, right) = play(&mut fpc, 8);
        assert!((left[0] - 0.5 * unity()).abs() < 1e-6 && right[0] == 0.0);
        // A stereo file keeps its sides.
        let mut fpc = kit(&[simple(60, &[plain("a")])]);
        fpc.set_sample(
            "a",
            Some(SampleFile {
                left: &[1.0; 8],
                right: Some(&[-0.5; 8]),
                rate: 44_100,
            }),
        );
        fpc.note_on(note(1, 60));
        let (left, right) = play(&mut fpc, 8);
        assert!((left[0] - unity()).abs() < 1e-6 && (right[0] + 0.5 * unity()).abs() < 1e-6);
    }

    #[test]
    fn the_volume_envelope_shapes_a_layer_and_a_note_off_lets_it_go() {
        // Down to nothing in a quarter of a second: 0.5 half seconds.
        let fall = envelope(&[(0.0, 1.0), (0.5, 0.0)], true, -1);
        let numbers = [99, 0, 0, 60, 0, -1, -1, 0, 0, 0];
        let mut fpc = kit(&[pad(numbers, &[plain("a")], &fall, &centre())]);
        fpc.set_sample("a", Some(file(&vec![1.0; 44_100], 44_100)));
        assert!(matches!(fpc.fidelity(), Fidelity::Approximate(what) if what.contains("envelope")));
        fpc.note_on(note(1, 60));
        let (left, _) = play(&mut fpc, 22_050);
        assert!((left[0] - unity()).abs() < 1e-6);
        // The gains are a period behind the envelope.
        let half = 44_100 / 8 + 48;
        assert!((left[half] / unity() - 0.5).abs() < 0.01, "{}", left[half]);
        assert!(left[44_100 / 4 + 200..].iter().all(|v| *v == 0.0));
        assert!(fpc.voices.is_empty());

        // Held on its second point until the note is let go.
        let held = envelope(&[(0.0, 1.0), (0.1, 0.5), (0.5, 0.0)], true, 1);
        let mut fpc = kit(&[pad(numbers, &[plain("a")], &held, &centre())]);
        fpc.set_sample("a", Some(file(&vec![1.0; 88_200], 44_100)));
        fpc.note_on(note(7, 60));
        let (left, _) = play(&mut fpc, 22_050);
        assert!((left[22_000] / unity() - 0.5).abs() < 1e-3);
        fpc.note_off(8);
        assert!((play(&mut fpc, 4800).0[4799] / unity() - 0.5).abs() < 1e-3);
        fpc.note_off(7);
        let (left, _) = play(&mut fpc, 22_050);
        assert!(left[5000] < 0.3 * unity() && left[5000] > 0.0);
        assert!(left[12_000..].iter().all(|v| *v == 0.0));
        assert!(fpc.voices.is_empty());

        // The pan envelope moves the layer: 1 is hard right.
        let right_side = envelope(&[(0.0, 1.0)], true, -1);
        let mut fpc = kit(&[pad(numbers, &[plain("a")], &off(), &right_side)]);
        fpc.set_sample("a", Some(file(&[1.0; 64], 44_100)));
        fpc.note_on(note(1, 60));
        let (left, right) = play(&mut fpc, 8);
        assert!(left[0].abs() < 1e-6);
        assert!((right[0] - gain(99) * gain(99) * FRAC_1_SQRT_2).abs() < 1e-6);
        fpc.all_notes_off();
        fpc.reset();
        assert!(fpc.voices.is_empty());
    }

    #[test]
    fn a_missing_file_is_silent_and_the_plugin_says_so() {
        let mut fpc = kit(&[
            simple(60, &[plain("%FLStudioData%\\gone.wav"), plain("here.wav")]),
            simple(61, &[plain("here.wav")]),
            simple(62, &[plain("")]),
        ]);
        assert_eq!(fpc.sample_paths(), ["%FLStudioData%\\gone.wav", "here.wav"]);
        let lacking =
            Fidelity::Approximate("FPC: a sample file is missing, and its layer is silent");
        assert_eq!(fpc.fidelity(), lacking);
        fpc.set_sample("here.wav", Some(file(&[1.0; 8], 44_100)));
        fpc.set_sample("%FLStudioData%\\gone.wav", None);
        fpc.set_sample("never named", Some(file(&[1.0; 8], 44_100)));
        assert_eq!(fpc.fidelity(), lacking);
        fpc.note_on(note(1, 60));
        assert!((play(&mut fpc, 8).0[0] - unity()).abs() < 1e-6);
        // A pad with no path lacks nothing.
        let mut empty = kit(&[]);
        assert!(empty.sample_paths().is_empty());
        assert_eq!(empty.fidelity(), Fidelity::Exact);
        empty.note_on(note(1, 100));
        assert!(play(&mut empty, 8).0.iter().all(|v| *v == 0.0));
        // A new FPC has its pads from key 60 up, each with an empty layer.
        let new = Fpc::default();
        assert_eq!(new.pads().len(), PADS);
        assert_eq!((new.pads()[0].note, new.pads()[31].note), (60, 91));
        assert_eq!(new.pads()[5].layers, [Layer::default()]);
        // Another output than the channel's is said.
        let mut out = kit(&[pad(
            [99, 0, 0, 60, 3, -1, -1, 0, 0, 0],
            &[plain("a")],
            &off(),
            &centre(),
        )]);
        out.set_sample("a", Some(file(&[1.0; 8], 44_100)));
        assert!(matches!(out.fidelity(), Fidelity::Approximate(what) if what.contains("output")));
    }

    #[test]
    fn the_ramp_runs_in_pairs_and_the_last_frames_one_at_a_time() {
        let slope = (0.1, -0.2);
        let mut ramp = Ramp::new((1.0, 2.0), slope, 11);
        let gains: Vec<(f32, f32)> = (0..11).map(|_| ramp.gains()).collect();
        // Eight frames in pairs: each lane moves two slopes a pair.
        let (mut even, mut odd) = ((1.0f32, 2.0f32), (1.0 + 0.1f32, 2.0 - 0.2f32));
        for pair in 0..4 {
            assert_eq!(gains[2 * pair], even);
            assert_eq!(gains[2 * pair + 1], odd);
            even = (even.0 + (0.1 + 0.1), even.1 + (-0.2 - 0.2));
            odd = (odd.0 + (0.1 + 0.1), odd.1 + (-0.2 - 0.2));
        }
        // Then three from the first lane, a slope each.
        for gain in &gains[8..11] {
            assert_eq!(*gain, even);
            even = (even.0 + slope.0, even.1 + slope.1);
        }
        // Fewer than eight frames are all one at a time.
        let mut short = Ramp::new((0.0, 0.0), (0.5, 0.5), 3);
        let gains: Vec<f32> = (0..3).map(|_| short.gains().0).collect();
        assert_eq!(gains, [0.0, 0.5, 1.0]);
    }

    /// FL moves a ramp's start on by the slope times the frames at the end
    /// of every block, so where the blocks fall moves its gains by a
    /// rounding step, and nothing else.
    #[test]
    fn the_blocks_can_fall_anywhere() {
        let sound = ramp(2000);
        let fall = envelope(&[(0.0, 1.0), (0.05, 0.0)], true, -1);
        let numbers = [99, 20, 0, 60, 0, -1, -1, 3, 0, 0];
        let pads = [pad(numbers, &[plain("a")], &fall, &centre())];
        let whole = {
            let mut fpc = kit(&pads);
            fpc.set_sample("a", Some(file(&sound, 44_100)));
            fpc.note_on(note(1, 60));
            play(&mut fpc, 3000)
        };
        let mut fpc = kit(&pads);
        fpc.set_sample("a", Some(file(&sound, 44_100)));
        fpc.note_on(note(1, 60));
        let (mut left, mut right) = (Vec::new(), Vec::new());
        for len in [1, 47, 48, 49, 7, 500, 1, 2347] {
            let (l, r) = play(&mut fpc, len);
            left.extend(l);
            right.extend(r);
        }
        assert_eq!((left.len(), right.len()), (3000, 3000));
        let near =
            |split: &[f32], one: &[f32]| split.iter().zip(one).all(|(a, b)| (a - b).abs() <= 1e-6);
        assert!(near(&left, &whole.0) && near(&right, &whole.1));
        // Where the envelope ends and the file ends are the same sample.
        let last = |side: &[f32]| side.iter().rposition(|v| *v != 0.0);
        assert_eq!(last(&left), last(&whole.0));
        assert!(whole.0.iter().any(|v| *v != 0.0));
    }

    #[test]
    fn the_parameters_are_the_five_knobs_of_each_pad() {
        let mut fpc = kit(&[pad(
            [128, -5, 3, 60, 0, -1, -1, 7, 0, 0],
            &[plain("a")],
            &off(),
            &centre(),
        )]);
        assert_eq!(fpc.param_count(), 0x180);
        let knobs = [0x00, 0x10, 0x20, 0x30, 0x100];
        let read: Vec<f32> = knobs.iter().map(|index| fpc.param(*index)).collect();
        assert_eq!(read, [128.0, -5.0, 1.0, 1.0, 7.0]);
        // Pad 17 is the second of the second bank.
        for (index, low, high) in [
            (0x41, 0.0, 127.0),
            (0x51, -128.0, 128.0),
            (0x61, 0.0, 1.0),
            (0x71, 0.0, 1.0),
            (0x141, -128.0, 128.0),
        ] {
            fpc.set_param(index, f32::MIN);
            assert_eq!(fpc.param(index), low, "{index:#x}");
            fpc.set_param(index, f32::MAX);
            assert_eq!(fpc.param(index), high, "{index:#x}");
        }
        let second = &fpc.pads()[17];
        assert_eq!((second.volume, second.pan, second.tune), (127, 128, 128));
        assert!(second.mute && second.solo);
        // Between the knobs there is nothing.
        for index in [0x80, 0xff, 0x110, 0x17f, 0x180, 0x200, 9999] {
            fpc.set_param(index, 5.0);
            assert_eq!(fpc.param(index), 0.0, "{index:#x}");
        }
    }

    #[test]
    fn a_state_fl_does_not_write_is_turned_down() {
        let good = state(&[simple(60, &[plain("a")])]);
        let mut fpc = Fpc::default();
        fpc.set_state(&good).unwrap();
        let kept = fpc.clone();
        let too_new = (WRITTEN + 1).to_le_bytes();
        let old = 1_000_010i32.to_le_bytes();
        for (bad, error) in [
            (
                &[][..],
                StateError::Length {
                    expected: 4,
                    found: 0,
                },
            ),
            (&too_new[..], StateError::Version { found: WRITTEN + 1 }),
            (&old[..], StateError::Version { found: 1_000_010 }),
        ] {
            assert_eq!(fpc.set_state(bad), Err(error));
            assert_eq!(fpc, kept);
        }
        // Cut off inside a pad, inside a layer and inside an envelope.
        for len in [10, 100, good.len() - 60, good.len() - 1] {
            assert!(matches!(
                fpc.set_state(&good[..len]),
                Err(StateError::Length { .. })
            ));
            assert_eq!(fpc, kept);
        }
        // The version alone is 32 new pads.
        fpc.set_state(&WRITTEN.to_le_bytes()).unwrap();
        assert_eq!(fpc.pads(), Fpc::default().pads());
        // An older state has its strings a byte a letter.
        let mut narrow = 2_000_013i32.to_le_bytes().to_vec();
        let mut body = chunk(0x65, b"Snare");
        let mut inner = chunk(300, b"s.wav");
        inner.extend(chunk(0x12e, &[]));
        body.extend(chunk(0x69, &inner));
        narrow.extend(chunk(2, &body));
        fpc.set_state(&narrow).unwrap();
        assert_eq!(fpc.pads()[0].name, "Snare");
        assert_eq!(fpc.sample_paths(), ["s.wav"]);
    }

    #[test]
    fn a_layer_past_the_most_takes_the_place_of_the_oldest() {
        let mut fpc = fpc_at(&[1.0; 4000], 44_100, 44_100.0);
        for voice in 0..VOICES as u32 + 5 {
            fpc.note_on(note(voice, 60));
        }
        assert_eq!(fpc.voices.len(), VOICES);
        assert_eq!(fpc.voices[0].note, 5);
        let (left, _) = play(&mut fpc, 16);
        assert!((left[0] - VOICES as f32 * unity()).abs() < 1e-3);
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, FPC's states skipped");
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        let presets = saved.len();
        saved.extend(testing::projects(&root));
        let (mut states, mut layers, mut paths) = (0, 0, 0);
        let mut enveloped = 0;
        for saved in saved {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let mut fpc = crate::generator_by_name(&saved.plugin).unwrap();
            if let Err(error) = fpc.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            let mut read = Fpc::default();
            read.set_state(&saved.state).unwrap();
            assert_eq!(read.pads().len(), PADS, "{}", saved.file);
            layers += read
                .pads()
                .iter()
                .map(|pad| pad.layers.len())
                .sum::<usize>();
            let active = |pad: &&Pad| pad.volume_envelope.active() || pad.pan_envelope.active();
            enveloped += read.pads().iter().filter(active).count();
            let named = fpc.sample_paths();
            assert!(
                named.iter().all(|path| path.contains('\\')),
                "{}",
                saved.file
            );
            paths += named.len();
            // With a click for every file, every pad that has one sounds.
            for path in &named {
                fpc.set_sample(path, Some(file(&[1.0; 64], 44_100)));
            }
            for (voice, pad) in read.pads().iter().enumerate() {
                fpc.note_on(note(voice as u32, pad.note as i16));
            }
            let (mut left, mut right) = ([0.0; 256], [0.0; 256]);
            fpc.process(&mut left, &mut right);
            assert!(left.iter().chain(&right).all(|v| v.is_finite()));
            let heard = left.iter().any(|v| *v != 0.0);
            assert_eq!(heard, !named.is_empty(), "{}", saved.file);
            states += 1;
        }
        // 85 preset states, 32 of them older ones that name the plugin one
        // byte a letter, and 25 uses in 20 projects, with 9565 layers.
        assert_eq!((presets, states, layers), (85, 110, 9565));
        eprintln!("FPC: {states} states, {paths} paths, {enveloped} pads with an envelope");
    }
}
