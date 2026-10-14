//! DirectWave: a sampler of 128 programs, each a set of zones that map a
//! sample file onto a range of keys and velocities, as
//! `docs/plugins/directwave.md` has it.
//!
//! # The state
//!
//! A version and then chunks ([`Chunks`]). FL writes version 0x25 or 0x26
//! and reads 0x24 to 0x26 this way. At the top level:
//!
//! ```text
//! chunk 6      16 bytes of flags
//! chunk 0x206  7 bytes of settings, among them the interpolation
//! chunk 5      a part, one of 16 MIDI channels: the program it plays, its volume, pan and wheels
//! chunk 1      a program, 128 of them: a header, its name, its effects, 100 groups and its zones
//! ```
//!
//! A zone is a chunk 3 in its program: its keys and velocities, the file
//! by path, the loop, the tune, two filters, an envelope for the volume,
//! two for the matrix, two LFOs and 16 slots of a modulation matrix. A zone
//! may carry its samples (chunk 0x205), which are then played instead of the
//! file. A state older than version 0x24 is a fixed layout; FL ships two
//! such presets, both without a zone, and those are read as empty.
//!
//! # A note
//!
//! The note goes to part 0 and so to that part's program. Every zone
//! whose key range holds the key and whose velocity range holds the MIDI
//! velocity starts a voice, one zone of a group when the group says so.
//!
//! A voice reads its file at `2^(cents / 1200) * file rate / rate` frames
//! a sample, in 32.32 fixed point, and draws a straight line between two
//! frames. The cents are the key's distance from the zone's root, times
//! the zone's key tracking, plus its tune and what the matrix adds. It
//! loops, plays once, holds the loop until the note is let go, or goes back
//! and forth. Two biquads of FL's own cookbook, each two stages deep, filter
//! it, and an ADSR envelope and the gains of zone, program and part shape
//! it.
//!
//! DirectWave works in steps of at most 256 samples. The matrix, the
//! envelopes for the matrix, the LFOs and the pitch are worked out once a
//! step, the volume envelope and the gains every sample.
//!
//! # What is not FL's
//!
//! - The three effects a program sends to, its program LFOs, the zone
//!   effects (ring modulator, decimator, bit crusher, phaser), filter types
//!   6 and 7, time stretching and the high quality interpolation are not
//!   built. Fidelity says so where a state asks for one of them.
//! - FL's `rand` stream, which the LFOs' random shapes, the random source
//!   and the filters' denormal guard draw on, starts here at the start of
//!   the plugin. In FL it is shared with whatever else ran on the thread.
//! - FL's block sizes are not known. The steps start where the engine's
//!   blocks start, so a smoothed value moves a step at a time from other
//!   places than in FL.
//! - Every note plays on part 0 and every part plays into the channel.
//! - A path that is not a file is silent.

use crate::sampler::{self, Bank, Chunks, Wave};
use crate::{Fidelity, Generator, NoteOn, SampleFile, StateError};
use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};

/// The plugin's event 201 name.
pub const NAME: &str = "DirectWave";

/// The voices FL keeps, and the most that sound at once.
pub const VOICES: usize = 256;

const PARTS: usize = 16;
const PROGRAMS: usize = 128;
const SLOTS: usize = 16;
const GROUPS: usize = 100;
/// A group's pick that plays every zone it reaches, and one that plays none.
const ALL: i32 = -1;
const NONE: i32 = -2;
/// How many notes' keys are kept for their note offs.
const NOTES: usize = 1024;
/// The longest step DirectWave works in.
const STEP: usize = 256;
/// The newest version FL reads, and the oldest it reads as chunks.
const NEWEST: i32 = 0x26;
const CHUNKED: i32 = 0x24;

/// `1 / 32767`, what a 16-bit frame is multiplied by.
const SHORT: f32 = 3.051_851e-5;

/// A field of a chunk, zero where the chunk is shorter, as FL reads one
/// into a cleared buffer.
fn bytes<const N: usize>(body: &[u8], at: usize) -> [u8; N] {
    let mut out = [0; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = body.get(at + i).copied().unwrap_or(0);
    }
    out
}

fn f32_at(body: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(bytes(body, at))
}

fn i32_at(body: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(bytes(body, at))
}

fn u8_at(body: &[u8], at: usize) -> u8 {
    body.get(at).copied().unwrap_or(0)
}

/// How much a time knob moves an envelope a tick (`FUN_180011020`): its
/// fourth power times 10 s, plus 1 ms, at `per_second` ticks a second.
fn pace(x: f32, per_second: f32) -> f32 {
    if x <= 0.0 {
        return 1.0;
    }
    1.0 / (x * x * x * x * per_second * 10.0 + per_second * 0.001)
}

/// The cutoff of a filter knob in Hz (`FUN_180010f30`): 127 semitones from
/// 13.75 Hz.
fn cutoff(x: f32) -> f32 {
    2f32.powf(x * 127.0 / 12.0 - 5.0) * 440.0
}

/// MSVC's `rand`, which FL's copy draws from.
#[derive(Debug, Clone, PartialEq)]
struct Rand(u32);

impl Rand {
    fn next(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as i32
    }
}

/// One of the four envelopes of a voice (`FUN_180013f20`): it rises by
/// the attack, falls by the decay to the sustain and by the release to
/// nothing, and what it gives is its level squared.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Envelope {
    /// 0 off, 1 attack, 2 decay, 3 sustain, 4 release.
    state: i32,
    level: f32,
}

impl Envelope {
    fn start(&mut self) {
        *self = Envelope {
            state: 1,
            level: 0.0,
        };
    }

    /// `FUN_180013f10`.
    fn release(&mut self) {
        if self.state > 0 {
            self.state = 4;
        }
    }

    fn next(&mut self, attack: f32, decay: f32, sustain: f32, release: f32) -> f32 {
        match self.state {
            4 => {
                self.level -= release;
                if self.level <= 0.0 {
                    *self = Envelope::default();
                }
            }
            2 => {
                self.level -= decay;
                if self.level <= sustain {
                    self.level = sustain;
                    self.state = 3;
                }
            }
            1 => {
                self.level += attack;
                if 1.0 <= self.level {
                    self.level = 1.0;
                    self.state = 2;
                }
            }
            _ => {}
        }
        self.level * self.level
    }
}

/// The running part of an LFO (`FUN_180025df0`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Lfo {
    phase: f32,
    /// How far the fade in has got, to 1.
    fade: f32,
    /// The random value of this turn, and the smoothed one.
    held: f32,
    smooth: f32,
}

impl Lfo {
    /// `FUN_180025fa0`: a phase from 0 to 1 sets where it starts, and 1 or
    /// more leaves it where the voice that had it before left it.
    fn start(&mut self, phase: f32) {
        self.fade = 0.0;
        if phase < 1.0 {
            self.phase = TAU * phase;
        }
    }

    fn next(&mut self, shape: u8, step: f32, fade: f32, rand: &mut Rand) -> f32 {
        let value = match shape {
            0 => self.phase.sin(),
            1 => self.phase.sin().abs(),
            2 if self.phase < PI => self.phase * (2.0 / PI) - 1.0,
            2 => 3.0 - self.phase * (2.0 / PI),
            3 if self.phase < PI => -1.0,
            3 => 1.0,
            4 => self.phase * (1.0 / PI) - 1.0,
            5 => 1.0 - self.phase * (1.0 / PI),
            6 => self.held,
            7 => {
                let before = self.smooth;
                self.smooth += (self.held - self.smooth) * step;
                before
            }
            _ => 0.0,
        };
        self.phase += step;
        if TAU <= self.phase {
            let drawn = rand.next() - 0x4000;
            self.phase -= TAU;
            self.held = drawn as f32 * 6.103_515_6e-5;
        }
        self.fade += fade;
        if 1.0 < self.fade {
            self.fade = 1.0;
        }
        self.fade * value
    }
}

/// A biquad's coefficients, `b0, b1, b2, a1, a2` with `a0` divided out.
type Coefficients = [f32; 5];

/// FL's cookbook filters (`FUN_180003a10`): 1 lowpass, 2 highpass, 3
/// bandpass, 4 notch, 5 allpass, anything else a wire.
fn coefficients(kind: u8, hz: f32, q: f32, rate: f32) -> Coefficients {
    if !(1..=5).contains(&kind) {
        return [1.0, 0.0, 0.0, 0.0, 0.0];
    }
    let w = TAU * hz / rate;
    let (sin, cos) = (w.sin(), w.cos());
    let alpha = sin / (q + q);
    let norm = 1.0 / (alpha + 1.0);
    let a1 = cos * -2.0 * norm;
    let a2 = (1.0 - alpha) * norm;
    match kind {
        1 => {
            let b0 = (1.0 - cos) * 0.5 * norm;
            [b0, (1.0 - cos) * norm, b0, a1, a2]
        }
        2 => {
            let b0 = (cos + 1.0) * 0.5 * norm;
            [b0, -(cos + 1.0) * norm, b0, a1, a2]
        }
        3 => [sin * 0.5 * norm, 0.0, -sin * 0.5 * norm, a1, a2],
        4 => [norm, a1, norm, a1, a2],
        _ => [norm * (1.0 - alpha), a1, (alpha + 1.0) * norm, a1, a2],
    }
}

/// Two biquad stages on one side (`FUN_180014dd0`), and the clip after
/// them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Biquads([f32; 8]);

impl Biquads {
    fn run(&mut self, x: f32, clip: f32, c: &Coefficients) -> f32 {
        let s = &mut self.0;
        let a = ((x * c[0] + s[2] * c[1] + c[2] * s[3]) - s[0] * c[3]) - s[1] * c[4];
        (s[3], s[2], s[1], s[0]) = (s[2], x, s[0], a);
        let b = ((a * c[0] + s[6] * c[1] + c[2] * s[7]) - s[4] * c[3]) - c[4] * s[5];
        (s[7], s[6], s[5], s[4]) = (s[6], a, s[4], b);
        if 0.0 < clip {
            let top = 1.0 - clip;
            return b.min(top).max(-top);
        }
        b
    }
}

/// The limiter after the filters (`FUN_180003980`).
fn limit(left: &mut f32, right: &mut f32, level: &mut f32, attack: f32, release: f32) {
    let peak = left.abs().max(right.abs());
    let pace = if peak <= *level { release } else { attack };
    *level = (*level - peak) * pace + peak;
    if 1.0 < *level {
        let gain = level.powf(-1.0);
        *left *= gain;
        *right *= gain;
    }
}

/// One of a zone's two filters.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Filter {
    /// 0 none, 1 lowpass, 2 highpass, 3 bandpass, 4 notch, 5 allpass, 6
    /// and 7 two more that are not built.
    pub kind: u8,
    /// From 0 to 1, 127 semitones up from 13.75 Hz.
    pub cutoff: f32,
    pub resonance: f32,
    /// The output is held inside `1 - clip`, when it is above 0.
    pub clip: f32,
    rest: [u8; 3],
    extra: f32,
}

/// One of a zone's two LFOs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LfoKnobs {
    /// 0 sine, 1 rectified sine, 2 triangle, 3 square, 4 saw up, 5 saw
    /// down, 6 random, 7 smoothed random.
    pub shape: u8,
    /// Its rate is a fraction of a beat.
    pub synced: bool,
    /// `20 * rate^2` Hz.
    pub rate: f32,
    /// Where a voice starts it, 1 for where the last left it.
    pub phase: f32,
    /// How long it takes to fade in.
    pub fade: f32,
}

/// One slot of a zone's modulation matrix: a source, a destination and an
/// amount, which goes in as `(2 (amount - 0.5))^3`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Slot {
    pub source: i16,
    pub destination: i16,
    pub amount: f32,
}

/// One zone.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub name: String,
    pub path: String,
    pub root: u8,
    pub low_key: u8,
    pub high_key: u8,
    pub low_velocity: u8,
    pub high_velocity: u8,
    /// A new voice of another zone in this group releases this one. 0 for
    /// none.
    pub cut: u8,
    /// The filters run side by side, and one after the other when not.
    pub parallel: bool,
    pub volume: f32,
    /// From 0 on the left to 1 on the right.
    pub pan: f32,
    /// The group it belongs to, of 100.
    pub group: i32,
    /// The file as the state saw it last: frames, channels, slices, rate
    /// and bits.
    pub frames: i32,
    pub channels: i32,
    pub slices: i32,
    pub rate: f32,
    pub bits: i32,
    /// 0 and 1 once (1 ignores the note off), 2 loop, 3 loop until the note
    /// off, 4 back and forth.
    pub mode: i32,
    pub loop_start: i32,
    pub loop_end: i32,
    pub start: i32,
    /// 0.5 for none, 2400 cents a unit.
    pub pitch: f32,
    pub coarse: i8,
    pub fine: i8,
    /// Cents a key, 100 for a chromatic zone.
    pub keytrack: i16,
    /// Stretching and its knobs; not built.
    pub stretch: u8,
    stretch_rest: (u8, [f32; 3]),
    /// The two sends, the third, and the dry level.
    pub sends: [f32; 4],
    pub filters: [Filter; 2],
    /// Plays at the tempo, not built.
    pub synced: bool,
    /// Starts at the slice its key picks.
    pub sliced: bool,
    /// Attack, decay, sustain, release.
    pub amp: [f32; 4],
    /// Ring modulator, decimator, bit crusher and phaser: on, and two knobs
    /// each. Not built.
    pub effects: [(u8, f32, f32); 4],
    pub envelopes: [[f32; 4]; 2],
    pub lfos: [LfoKnobs; 2],
    pub slots: [Slot; SLOTS],
    /// The file's or the held samples' place in the bank.
    wave: Option<usize>,
}

impl Default for Zone {
    fn default() -> Self {
        Zone {
            name: String::new(),
            path: String::new(),
            root: 60,
            low_key: 0,
            high_key: 127,
            low_velocity: 0,
            high_velocity: 127,
            cut: 0,
            parallel: false,
            volume: 1.0,
            pan: 0.5,
            group: 0,
            frames: 0,
            channels: 0,
            slices: 0,
            rate: 44_100.0,
            bits: 32,
            mode: 0,
            loop_start: 0,
            loop_end: 0,
            start: 0,
            pitch: 0.5,
            coarse: 0,
            fine: 0,
            keytrack: 100,
            stretch: 0,
            stretch_rest: (0, [0.0; 3]),
            sends: [0.0, 0.0, 0.0, 1.0],
            filters: [Filter::default(); 2],
            synced: false,
            sliced: false,
            amp: [0.0, 0.5, 1.0, 0.25],
            effects: [(0, 0.0, 0.0); 4],
            envelopes: [[0.0, 0.5, 0.5, 0.25]; 2],
            lfos: [LfoKnobs::default(); 2],
            slots: [Slot::default(); SLOTS],
            wave: None,
        }
    }
}

impl Zone {
    /// Whether a key and a MIDI velocity reach the zone.
    fn reaches(&self, note: i32, velocity: i32) -> bool {
        let keys = i32::from(self.low_key) <= note && note <= i32::from(self.high_key);
        let velocity = velocity as f32;
        let (low, high) = (f32::from(self.low_velocity), f32::from(self.high_velocity));
        keys && low <= velocity && velocity <= high
    }

    /// The group it belongs to, as an index into a program's groups.
    fn group_index(&self) -> usize {
        usize::try_from(self.group).unwrap_or(0).min(GROUPS - 1)
    }

    /// Read a chunk 3 (`FUN_18003f840`). Samples it carries go into the
    /// bank, and the path of one that carries none is asked for.
    fn read(body: &[u8], bank: &mut Bank) -> Result<Self, StateError> {
        let mut zone = Zone::default();
        let (mut filters, mut envelopes, mut lfos, mut slots) = (0, 0, 0, 0);
        let mut extra = 0;
        let mut held = None;
        for chunk in Chunks::new(body) {
            let (id, b) = chunk?;
            match id {
                500 => {
                    (zone.root, zone.low_key, zone.high_key) =
                        (u8_at(b, 0), u8_at(b, 1), u8_at(b, 2));
                    (zone.low_velocity, zone.high_velocity) = (u8_at(b, 3), u8_at(b, 4));
                    zone.cut = u8_at(b, 5);
                    zone.parallel = u8_at(b, 8) != 0;
                    (zone.volume, zone.pan) = (f32_at(b, 9), f32_at(b, 13));
                    zone.group = i32_at(b, 17);
                    extra = i32_at(b, 21);
                }
                0x1f5 => zone.name = sampler::text(b, false),
                0x1f6 => zone.path = sampler::text(b, false),
                0x1f7 => {
                    zone.frames = i32_at(b, 0);
                    (zone.channels, zone.slices) = (i32_at(b, 8), i32_at(b, 12));
                    (zone.rate, zone.mode) = (f32_at(b, 16), i32_at(b, 20));
                    (zone.loop_start, zone.loop_end) = (i32_at(b, 24), i32_at(b, 28));
                    let bits = if b.len() < 0x28 { 32 } else { i32_at(b, 36) };
                    zone.bits = bits;
                    let start = i32_at(b, 32);
                    zone.start = if i64::from(zone.frames) < i64::from(start) {
                        0
                    } else {
                        start
                    };
                }
                0x1f8 => {
                    zone.pitch = f32_at(b, 0);
                    zone.coarse = u8_at(b, 4) as i8;
                    zone.fine = u8_at(b, 5) as i8;
                    zone.keytrack = i16::from_le_bytes(bytes(b, 6));
                }
                0x1f9 => {
                    zone.stretch = u8_at(b, 0);
                    zone.stretch_rest = (u8_at(b, 1), [f32_at(b, 2), f32_at(b, 6), f32_at(b, 10)]);
                }
                0x1fa => {
                    for (i, send) in zone.sends.iter_mut().enumerate() {
                        *send = f32_at(b, 4 * i);
                    }
                }
                0x1fb if filters < 2 => {
                    zone.filters[filters] = Filter {
                        kind: u8_at(b, 0),
                        rest: [u8_at(b, 1), u8_at(b, 2), u8_at(b, 3)],
                        cutoff: f32_at(b, 4),
                        resonance: f32_at(b, 8),
                        clip: f32_at(b, 12),
                        extra: f32_at(b, 16),
                    };
                    filters += 1;
                }
                0x1fc => (zone.synced, zone.sliced) = (u8_at(b, 0) != 0, u8_at(b, 1) != 0),
                0x1fd => {
                    for (i, knob) in zone.amp.iter_mut().enumerate() {
                        *knob = f32_at(b, 4 * i);
                    }
                }
                0x1fe..=0x201 => {
                    zone.effects[(id - 0x1fe) as usize] = (u8_at(b, 0), f32_at(b, 1), f32_at(b, 5));
                }
                0x202 if envelopes < 2 => {
                    for (i, knob) in zone.envelopes[envelopes].iter_mut().enumerate() {
                        *knob = f32_at(b, 4 * i);
                    }
                    envelopes += 1;
                }
                0x203 if lfos < 2 => {
                    zone.lfos[lfos] = LfoKnobs {
                        shape: u8_at(b, 0),
                        synced: u8_at(b, 1) != 0,
                        rate: f32_at(b, 4),
                        phase: f32_at(b, 8),
                        fade: f32_at(b, 12),
                    };
                    lfos += 1;
                }
                0x204 if slots < SLOTS => {
                    zone.slots[slots] = Slot {
                        source: i16::from_le_bytes(bytes(b, 0)),
                        destination: i16::from_le_bytes(bytes(b, 2)),
                        amount: f32_at(b, 4),
                    };
                    slots += 1;
                }
                0x205 => held = Some(b),
                4 => break,
                _ => {}
            }
        }
        zone.wave = match held {
            Some(b) => held_wave(&zone, extra, b).map(|wave| bank.hold(wave)),
            None => bank.place(&zone.path),
        };
        Ok(zone)
    }
}

impl Bank {
    /// Put a decoded file in a place of its own, for a state that carries
    /// its samples. It has no path, so no host is asked for it.
    pub(crate) fn hold(&mut self, wave: Wave) -> usize {
        self.paths.push(String::new());
        self.slots.push(sampler::Slot::Ready(wave));
        self.paths.len() - 1
    }
}

/// The samples a zone carries in its chunk 0x205: each channel's frames
/// with `extra` more around them, the first 256 of which come before the
/// sound, in 16-bit or float as the zone's bits say. A channel the chunk
/// is too short for is silent.
fn held_wave(zone: &Zone, extra: i32, b: &[u8]) -> Option<Wave> {
    let width = if zone.bits == 16 { 2 } else { 4 };
    let frames = usize::try_from(zone.frames).ok().filter(|f| *f > 0)?;
    let channels = usize::try_from(zone.channels)
        .ok()
        .filter(|c| (1..=2).contains(c))?;
    let stride = frames + usize::try_from(extra).unwrap_or(0);
    let side = |channel: usize| -> Vec<f32> {
        let from = channel * stride * width + 256 * width;
        (0..frames)
            .map(|frame| {
                let at = from + frame * width;
                if width == 2 {
                    f32::from(i16::from_le_bytes(bytes(b, at))) / 32_768.0
                } else {
                    f32_at(b, at)
                }
            })
            .collect()
    };
    let full = |channel: usize| b.len() >= (channel + 1) * stride * width;
    let left = if full(0) { side(0) } else { vec![0.0; frames] };
    let right = (channels == 2).then(|| if full(1) { side(1) } else { vec![0.0; frames] });
    Some(Wave {
        left,
        right,
        rate: zone.rate.max(1.0) as u32,
    })
}

/// One of a program's 100 groups.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Group {
    /// 0 every zone, 1 round robin, 2 at random, 3 at random but not the
    /// one before.
    pub mode: u8,
    /// How likely the group is to sound at all.
    pub chance: f32,
    pub fade: f32,
    /// The zone of the group's list that played last.
    last: i32,
}

impl Default for Group {
    fn default() -> Self {
        Group {
            mode: 0,
            chance: 1.0,
            fade: 0.0,
            last: 0,
        }
    }
}

/// One program.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub name: String,
    pub path: String,
    /// 0 polyphonic, 1 mono, 2 legato.
    pub poly: u8,
    /// The glide only slides from a note that is held.
    pub legato: bool,
    pub volume: f32,
    pub glide: f32,
    /// Two waveshapers on what the program's voices make: a `tanh` and a
    /// sine, each on and with its drive.
    pub shapers: [(bool, f32); 2],
    /// The three effects it sends to: on, and their bytes. Not built.
    pub effects: [(bool, Vec<u8>); 3],
    /// Its own two LFOs, for sources 5 and 6. Not built.
    lfos: [Vec<u8>; 2],
    /// The four knobs, sources 16 to 19.
    pub knobs: [f32; 4],
    pub groups: Vec<Group>,
    pub zones: Vec<Zone>,
    rest: Vec<u8>,
}

impl Default for Program {
    fn default() -> Self {
        Program {
            name: String::new(),
            path: String::new(),
            poly: 0,
            legato: false,
            volume: 1.0,
            glide: 0.0,
            shapers: [(false, 0.0); 2],
            effects: [
                (false, Vec::new()),
                (false, Vec::new()),
                (false, Vec::new()),
            ],
            lfos: [Vec::new(), Vec::new()],
            knobs: [0.0; 4],
            groups: vec![Group::default(); GROUPS],
            zones: Vec::new(),
            rest: Vec::new(),
        }
    }
}

impl Program {
    /// Read a chunk 1 (`FUN_18003f130`): the index in chunk 100 says which
    /// program it is.
    fn read(body: &[u8], bank: &mut Bank) -> Result<(usize, Self), StateError> {
        let mut program = Program::default();
        let mut index = None;
        let mut count = 0;
        let (mut lfos, mut knobs) = (0, 0);
        for chunk in Chunks::new(body) {
            let (id, b) = chunk?;
            if index.is_none() && id != 100 && id != 2 {
                break;
            }
            match id {
                2 => break,
                100 => {
                    let at = usize::try_from(i32_at(b, 0)).ok().filter(|i| *i < PROGRAMS);
                    index = Some(at.unwrap_or(PROGRAMS));
                    (program.poly, program.legato) = (u8_at(b, 4), u8_at(b, 5) != 0);
                    (program.volume, program.glide) = (f32_at(b, 6), f32_at(b, 10));
                    count = usize::try_from(i32_at(b, 14)).unwrap_or(0);
                    program.rest = b.get(18..).unwrap_or_default().to_vec();
                }
                0x66 => program.name = sampler::text(b, false),
                0x67 => program.path = sampler::text(b, false),
                0x68 => {
                    program.shapers = [
                        (u8_at(b, 0) != 0, f32_at(b, 2)),
                        (u8_at(b, 1) != 0, f32_at(b, 6)),
                    ]
                }
                0x69..=0x6b => {
                    program.effects[(id - 0x69) as usize] = (u8_at(b, 0) != 0, b.to_vec())
                }
                0x6c if lfos < 2 => {
                    program.lfos[lfos] = b.to_vec();
                    lfos += 1;
                }
                0x6d if knobs < 4 && b.len() == 4 => {
                    program.knobs[knobs] = f32_at(b, 0);
                    knobs += 1;
                }
                0x6e => {
                    let at = i32_at(b, 0);
                    if (1..GROUPS as i32).contains(&at) {
                        let group = &mut program.groups[at as usize];
                        group.mode = u8_at(b, 4);
                        if group.mode > 3 {
                            group.mode = 0;
                        }
                        (group.chance, group.fade) = (f32_at(b, 5), f32_at(b, 9));
                    }
                }
                3 if program.zones.len() < count => program.zones.push(Zone::read(b, bank)?),
                _ => {}
            }
        }
        Ok((index.unwrap_or(PROGRAMS), program))
    }

    /// Whether any zone sends to an effect of the program that is on.
    fn sends(&self) -> bool {
        let on = |i: usize| self.effects[i].0;
        let send = |zone: &Zone| (0..3).any(|i| on(i) && zone.sends[i] > 0.0);
        self.zones.iter().any(send)
    }
}

/// One of the 16 parts, the MIDI channels.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub program: u8,
    pub output: u8,
    /// Cents at a full bend.
    pub bend_range: i32,
    /// From -1 to 1.
    pub bend: f32,
    pub wheel: f32,
    pub volume: f32,
    /// From 0 on the left to 1 on the right.
    pub pan: f32,
    pub expression: f32,
    pub sustain: f32,
    /// Its last note, and what each key holds.
    last: i32,
    keys: [Key; 128],
}

impl Default for Part {
    fn default() -> Self {
        Part {
            program: 0,
            output: 0,
            bend_range: 200,
            bend: 0.0,
            wheel: 0.0,
            volume: 100.0 / 127.0,
            pan: 0.5,
            expression: 1.0,
            sustain: 0.0,
            last: 0,
            keys: [Key::default(); 128],
        }
    }
}

impl Part {
    /// A chunk 5's part from offset 0x208 on (`FUN_18003eb30`).
    fn read(&mut self, b: &[u8]) {
        (self.program, self.output) = (u8_at(b, 0x208), u8_at(b, 0x209));
        self.bend_range = i32_at(b, 0x214);
        (self.bend, self.wheel) = (f32_at(b, 0x234), f32_at(b, 0x238));
        (self.volume, self.pan) = (f32_at(b, 0x23c), f32_at(b, 0x240));
        (self.expression, self.sustain) = (f32_at(b, 0x244), f32_at(b, 0x248));
        self.last = i32_at(b, 4);
    }
}

/// A key a part holds: the MIDI velocity, FL's voice and its levels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Key {
    velocity: i32,
    id: u32,
    levels: Levels,
}

/// What FL's host keeps a DirectWave voice at: the note's volume, its pan
/// from -1 to 1 and cents on top of its key.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Levels {
    volume: f32,
    pan: f32,
    cents: f32,
}

/// One voice, as FL keeps 256 of them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Voice {
    /// Started and not yet worked out once.
    fresh: bool,
    amp: Envelope,
    envelopes: [Envelope; 2],
    lfos: [Lfo; 2],
    part: usize,
    program: usize,
    zone: usize,
    note: i32,
    /// The note before it, for the glide, and where the glide is.
    previous: i32,
    glide: f32,
    /// A note of the part was held when it started.
    legato: bool,
    id: u32,
    cut: u8,
    levels: Levels,
    random: f32,
    follower: f32,
    /// The file has played out.
    ended: bool,
    /// Its zone plays once and takes no note off.
    once: bool,
    /// Let go before it started.
    waiting: bool,
    /// The read position, 32.32 fixed point, and its step.
    position: i64,
    step: i64,
    low: i32,
    high: i32,
    length: i32,
    /// 2 once, 1 after the first time round a loop.
    looped: i32,
    backwards: bool,
    gains: (f32, f32),
    cutoffs: (f32, f32),
    limiter: f32,
    /// Filter, side.
    filters: [[Biquads; 2]; 2],
}

impl Voice {
    fn frame(&self) -> u32 {
        (self.position >> 32) as u32
    }

    fn fraction(&self) -> u32 {
        self.position as u32
    }

    fn set_frame(&mut self, frame: u32, keep: bool) {
        let fraction = if keep { self.position & 0xffff_ffff } else { 0 };
        self.position = (i64::from(frame) << 32) | fraction;
    }

    /// `FUN_180033f00`: let go, unless the zone plays once and `forced` is
    /// clear. FL drops a voice that has not started. A note off for one
    /// waits until it has, as the engine sends a step's note off on the
    /// sample of its note on.
    fn release(&mut self, forced: bool) {
        if (1..=3).contains(&self.amp.state) && (!self.once || forced) {
            self.amp.release();
            self.envelopes[0].release();
            self.envelopes[1].release();
        }
        if self.fresh && forced {
            self.fresh = false;
            self.amp.state = 0;
        }
        self.waiting = self.fresh;
    }
}

/// A decoded file as a voice reads it: 256 frames of nothing on either
/// side, and a 16-bit file read back as FL holds it.
struct Source<'a> {
    left: &'a [f32],
    right: Option<&'a [f32]>,
    short: bool,
    /// The file's own rate.
    rate: f32,
}

impl Source<'_> {
    fn at(&self, side: &[f32], frame: u32) -> f32 {
        let value = side.get(frame as usize).copied().unwrap_or(0.0);
        if self.short {
            (value * 32_768.0) as i32 as f32 * SHORT
        } else {
            value
        }
    }

    /// The raw 16-bit value, for the stereo readers that scale last.
    fn raw(&self, side: &[f32], frame: u32) -> f32 {
        let value = side.get(frame as usize).copied().unwrap_or(0.0);
        (value * 32_768.0) as i32 as f32
    }

    /// `FUN_1800332d0` and `FUN_180033550`, and with `wrap` the frame the
    /// line goes to from the last of a loop (`FUN_1800333a0`,
    /// `FUN_180033670`).
    fn read(&self, frame: u32, fraction: u32, wrap: Option<u32>) -> (f32, f32) {
        let f = (f64::from(fraction) * 2.328_306_436_538_696_3e-10) as f32;
        let mono = |side: &[f32]| {
            let s0 = self.at(side, frame);
            let s1 = self.at(side, wrap.unwrap_or(frame + 1));
            (s1 - s0) * f + s0
        };
        let Some(right) = self.right else {
            let value = mono(self.left);
            return (value, value);
        };
        if wrap.is_some() {
            return (mono(self.left), mono(right));
        }
        let stereo = |side: &[f32]| {
            let g = 1.0 - f;
            if self.short {
                (self.raw(side, frame) * g + self.raw(side, frame + 1) * f) * SHORT
            } else {
                g * self.at(side, frame) + f * self.at(side, frame + 1)
            }
        };
        (stereo(self.left), stereo(right))
    }
}

/// Whether every frame of a file is a 16-bit value, so that FL holds it
/// as one.
fn is_short(wave: &Wave) -> bool {
    let fits = |v: &f32| {
        let x = v * 32_768.0;
        x.fract() == 0.0 && (-32_768.0..32_768.0).contains(&x)
    };
    wave.left.iter().all(fits) && wave.right.as_deref().is_none_or(|r| r.iter().all(fits))
}

/// What a step has for every voice.
struct Step {
    rate: f32,
    /// Steps a second.
    per_second: f32,
    /// Radians an LFO turns a step at a rate knob of 1.
    lfo: f32,
    /// Steps a sixteenth.
    beat: f32,
    /// Smoothing a sample, 5 ms.
    smooth: f32,
    attack: f32,
    release: f32,
}

/// DirectWave.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectWave {
    parts: Vec<Part>,
    programs: Vec<Program>,
    bank: Bank,
    /// Which places in the bank hold 16-bit sound.
    short: Vec<bool>,
    /// The flags of chunk 6 and the settings of chunk 0x206.
    flags: u32,
    settings: [u8; 7],
    voices: Vec<Voice>,
    /// The key of each of the last notes, by the host's number.
    notes: Vec<(u32, i32)>,
    rate: f32,
    tempo: f32,
    rand: Rand,
    /// A step's sound before the volume, and a part's.
    scratch: [Vec<f32>; 4],
}

impl Default for DirectWave {
    fn default() -> Self {
        DirectWave {
            parts: vec![Part::default(); PARTS],
            programs: vec![Program::default(); PROGRAMS],
            bank: Bank::default(),
            short: Vec::new(),
            flags: 0,
            settings: [1, 0, 0, 0, 0, 0, 0],
            voices: vec![Voice::default(); VOICES],
            notes: vec![(u32::MAX, 0); NOTES],
            rate: 44_100.0,
            tempo: 120.0,
            rand: Rand(1),
            scratch: std::array::from_fn(|_| vec![0.0; STEP]),
        }
    }
}

impl DirectWave {
    /// The 128 programs.
    pub fn programs(&self) -> &[Program] {
        &self.programs
    }

    /// The 16 parts.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// The flag DirectWave saves as bit 1 of chunk 6, which squares the
    /// volume and takes the zone's into the matrix's base.
    fn squared(&self) -> bool {
        self.flags & 2 != 0
    }

    /// Bit 0, which keeps a volume the velocity multiplies inside 0 to 1.
    fn clamped(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Read a state older than 0x24: FL's two presets of it have no zone,
    /// and one with zones is turned down.
    fn read_fixed(version: i32, rest: &[u8]) -> Result<(), StateError> {
        let program = match version {
            ..0x20 => 0x1b4,
            0x20..0x23 => 0x1c4,
            _ => 0x15ac,
        };
        let at = 4 + 0x2700;
        let expected = at + PROGRAMS * program;
        if rest.len() < expected {
            return Err(StateError::Length {
                expected: expected + 4,
                found: rest.len() + 4,
            });
        }
        let zones = (0..PROGRAMS).map(|i| i32_at(rest, at + i * program + 0x130));
        if zones.into_iter().any(|count| count > 0) {
            return Err(StateError::Version { found: version });
        }
        Ok(())
    }

    /// The state's zones and the bank's places, in a new plugin.
    fn read(state: &[u8]) -> Result<Self, StateError> {
        let Some((word, rest)) = state.split_first_chunk::<4>() else {
            return Err(StateError::Length {
                expected: 4,
                found: state.len(),
            });
        };
        let version = i32::from_le_bytes(*word);
        if version > NEWEST {
            return Err(StateError::Version { found: version });
        }
        let mut plugin = DirectWave::default();
        if version < CHUNKED {
            Self::read_fixed(version, rest)?;
            return Ok(plugin);
        }
        for chunk in Chunks::new(rest) {
            let (id, b) = chunk?;
            match id {
                6 => plugin.flags = u32::from_le_bytes(bytes(b, 0)),
                0x206 => plugin.settings = bytes(b, 0),
                5 => {
                    let at = i32_at(b, 0);
                    if let Some(part) = usize::try_from(at)
                        .ok()
                        .and_then(|i| plugin.parts.get_mut(i))
                    {
                        part.read(b);
                    }
                }
                1 => {
                    let (index, program) = Program::read(b, &mut plugin.bank)?;
                    if let Some(slot) = plugin.programs.get_mut(index) {
                        *slot = program;
                    }
                }
                _ => {}
            }
        }
        plugin.short = vec![false; plugin.bank.slots.len()];
        for (place, short) in plugin.short.iter_mut().enumerate() {
            *short = plugin.bank.wave(place).is_some_and(is_short);
        }
        Ok(plugin)
    }

    /// The program a part plays.
    fn program_of(&self, part: usize) -> usize {
        usize::from(self.parts[part].program).min(PROGRAMS - 1)
    }

    /// A free voice: off and not just started.
    fn free(&self) -> Option<usize> {
        let free = |voice: &Voice| voice.amp.state == 0 && !voice.fresh;
        self.voices.iter().position(free)
    }

    /// `FUN_180024f90`: start what a key at this velocity plays on a part.
    fn start(&mut self, part: usize, note: i32, velocity: i32, id: u32, levels: Levels) {
        let program = self.program_of(part);
        let held = self.parts[part].keys.iter().any(|key| key.velocity > 0);
        // How many zones of each group the key and the velocity reach, and
        // which of them a group that plays one of them picks. Nothing is
        // allocated: this runs on a note on.
        let mut counts = [0i32; GROUPS];
        for zone in &self.programs[program].zones {
            if zone.reaches(note, velocity) {
                counts[zone.group_index()] += 1;
            }
        }
        let mut picks = [ALL; GROUPS];
        for (group, &count) in counts.iter().enumerate() {
            if count == 0 {
                continue;
            }
            let g = self.programs[program].groups[group];
            if g.chance < 1.0 {
                // FL draws from `rand_s` for this; its stream is not FL's.
                let drawn = self.rand.next() as f32 / 32_768.0;
                if g.chance < drawn {
                    picks[group] = NONE;
                    continue;
                }
            }
            if count == 1 {
                continue;
            }
            let mut draw = |of: i32| {
                let drawn = self.rand.next();
                drawn / (0x7fff / of).max(1)
            };
            let picked = match g.mode {
                1 => {
                    let next = g.last + 1;
                    if next < count { next } else { 0 }
                }
                2 => draw(count),
                3 if g.last >= 0 => {
                    let drawn = draw(count - 1);
                    if g.last <= drawn { drawn + 1 } else { drawn }
                }
                3 => draw(count),
                _ => continue,
            };
            if !(0..count).contains(&picked) {
                continue;
            }
            picks[group] = picked;
            self.programs[program].groups[group].last = picked;
        }
        let poly = self.programs[program].poly;
        let last = self.parts[part].last;
        // Group by group, and in a group zone by zone, as FL starts them.
        for (group, &pick) in picks.iter().enumerate() {
            let mut place = -1;
            for index in 0..self.programs[program].zones.len() {
                let zone = &self.programs[program].zones[index];
                if !zone.reaches(note, velocity) || zone.group_index() != group {
                    continue;
                }
                place += 1;
                if pick != ALL && pick != place {
                    continue;
                }
                let (cut, mode) = (zone.cut, zone.mode);
                if cut != 0 {
                    let cuts = |v: &&mut Voice| v.cut == cut && (1..=3).contains(&v.amp.state);
                    for voice in self.voices.iter_mut().filter(cuts) {
                        voice.release(true);
                    }
                }
                if poly == 1 {
                    for voice in self.voices.iter_mut().filter(|v| v.part == part) {
                        voice.ended = true;
                        voice.release(true);
                    }
                }
                if poly == 2 {
                    let sounding =
                        |v: &&mut Voice| v.part == part && (1..=3).contains(&v.amp.state);
                    if let Some(voice) = self.voices.iter_mut().find(sounding) {
                        (voice.previous, voice.note, voice.id) = (last, note, id);
                        voice.levels = levels;
                        continue;
                    }
                }
                let Some(slot) = self.free() else {
                    continue;
                };
                let voice = &mut self.voices[slot];
                (voice.part, voice.program, voice.zone) = (part, program, index);
                (voice.previous, voice.note, voice.id) = (last, note, id);
                (voice.legato, voice.cut, voice.levels) = (held, cut, levels);
                voice.once = mode == 1;
                voice.waiting = false;
                voice.fresh = true;
                voice.ended = false;
            }
        }
        let part = &mut self.parts[part];
        part.keys[note as usize] = Key {
            velocity,
            id,
            levels,
        };
        part.last = note;
    }

    /// Work out one voice for a step of `len` samples into the part's two
    /// buffers (`FUN_180030090`).
    #[allow(clippy::too_many_arguments)]
    fn render(
        voice: &mut Voice,
        zone: &Zone,
        program: &Program,
        part: &Part,
        source: Option<Source<'_>>,
        step: &Step,
        flags: (bool, bool),
        rand: &mut Rand,
        buffers: &mut [Vec<f32>],
        len: usize,
    ) {
        let (squared, clamped) = flags;
        let Some(source) = source else {
            voice.amp.state = 0;
            voice.fresh = false;
            return;
        };
        let frames = source.left.len() as i32;
        if frames == 0 {
            voice.amp.state = 0;
            voice.fresh = false;
            return;
        }
        if voice.fresh {
            voice.position = 0;
            voice.follower = 0.0;
            voice.envelopes[0].start();
            voice.envelopes[1].start();
            voice.lfos[0].start(zone.lfos[0].phase);
            voice.lfos[1].start(zone.lfos[1].phase);
            voice.amp.start();
            voice.random = rand.next() as f32 * 3.051_757_8e-5;
            voice.glide = voice.previous as f32;
            if program.glide <= 0.0 || (program.legato && !voice.legato) {
                voice.glide = voice.note as f32;
            }
        }
        if voice.amp.state < 1 {
            return;
        }
        if frames as u32 <= voice.frame() {
            voice.set_frame(frames as u32, true);
        }
        let per = step.per_second;
        let e = &zone.envelopes;
        let env = [
            voice.envelopes[0].next(
                pace(e[0][0], per),
                pace(e[0][1], per),
                e[0][2],
                pace(e[0][3], per),
            ),
            voice.envelopes[1].next(
                pace(e[1][0], per),
                pace(e[1][1], per),
                e[1][2],
                pace(e[1][3], per),
            ),
        ];
        let mut lfo = [0.0; 2];
        for (i, knobs) in zone.lfos.iter().enumerate() {
            let rate = knobs.rate * knobs.rate;
            let mut turn = step.lfo * rate;
            if knobs.synced {
                let beats = (f64::from(rate) * 127.5 + 1.0) as i32;
                turn = (f64::from(TAU) / (f64::from(beats) * f64::from(step.beat))) as f32;
            }
            let fade = pace(knobs.fade, per);
            lfo[i] = voice.lfos[i].next(knobs.shape, turn, fade, rand);
        }

        // The matrix's destinations: 1 pitch, 2 volume, 3 pan, 4 to 6 the
        // start and the loop, 7 to 12 the filters, 13 to 16 the volume's
        // envelope, 17 to 24 the zone effects, 25 to 28 the sends and the
        // dry level, 29 to 31 stretching, 32 on the slots' amounts.
        let mut d = [0f32; 48];
        d[1] = zone.pitch;
        d[2] = if squared { zone.volume } else { 1.0 };
        let pan = voice.levels.pan * 0.5 + zone.pan;
        d[3] = if 0.0 <= pan { pan.min(1.0) } else { 0.0 };
        let targets = |n: i16| zone.slots.iter().any(|s| s.destination == n);
        let (moves_start, moves_low, moves_high) = (targets(4), targets(5), targets(6));
        if moves_start {
            d[4] = zone.start as f32 / frames as f32;
        }
        if moves_low {
            d[5] = zone.loop_start as f32 / frames as f32;
        }
        if moves_high {
            d[6] = zone.loop_end as f32 / frames as f32;
        }
        let [f1, f2] = zone.filters;
        d[7..13].copy_from_slice(&[
            f1.cutoff,
            f1.resonance,
            f1.clip,
            f2.cutoff,
            f2.resonance,
            f2.clip,
        ]);
        d[13..17].copy_from_slice(&zone.amp);
        for (i, (_, a, b)) in zone.effects.iter().enumerate() {
            d[17 + 2 * i] = *a;
            d[18 + 2 * i] = *b;
        }
        d[25..29].copy_from_slice(&zone.sends);
        d[29..32].copy_from_slice(&zone.stretch_rest.1);
        for (i, slot) in zone.slots.iter().enumerate() {
            d[32 + i] = slot.amount;
        }
        let mut follows = false;
        let value = |source: i16, follows: &mut bool| -> Option<f32> {
            Some(match source {
                0 | 7 | 8 => 0.0,
                1 => voice.note as f32 / 127.0,
                2 => voice.levels.volume,
                3 => part.wheel,
                4 => part.bend,
                9 => env[0],
                10 => env[1],
                11 => lfo[0],
                12 => lfo[1],
                13 => {
                    *follows = true;
                    voice.follower
                }
                14 => voice.frame() as f32 / frames as f32,
                15 => voice.random,
                16..=19 => program.knobs[(source - 16) as usize],
                _ => return None,
            })
        };
        let amount = |a: f32| {
            let x = (a - 0.5) + (a - 0.5);
            x * x * x
        };
        for i in (0..SLOTS).rev() {
            let slot = zone.slots[i];
            let target = slot.destination as u16 as i32;
            if target < 32 || target - i as i32 > 31 || target >= 48 {
                continue;
            }
            let mut x = d[target as usize];
            if let Some(v) = value(slot.source, &mut follows) {
                x += amount(d[32 + i]) * v;
            }
            d[target as usize] = x.clamp(0.0, 1.0);
        }
        for i in 0..SLOTS {
            let slot = zone.slots[i];
            let target = slot.destination as u16 as i32;
            if (target > 31 && target - (i as i32) < 32) || target >= 48 {
                continue;
            }
            let mut x = d[target as usize];
            let a = amount(d[32 + i]);
            if slot.source == 2 && !squared && target == 2 {
                x = x * a * voice.levels.volume;
            } else if let Some(v) = value(slot.source, &mut follows) {
                x += a * v;
            }
            if slot.source != 2 || clamped || target != 2 {
                x = if 0.0 <= x { x.min(1.0) } else { 0.0 };
            }
            d[target as usize] = x;
        }

        let frames_f = frames as f32;
        let start = if moves_start {
            (d[4] * frames_f) as i32
        } else {
            zone.start
        };
        let low = if moves_low {
            (d[5] * frames_f) as i32
        } else {
            zone.loop_start
        };
        let high = if moves_high {
            (d[6] * frames_f) as i32
        } else {
            zone.loop_end
        };
        (voice.low, voice.high) = (low.min(high), low.max(high));
        voice.length = voice.high - voice.low;

        // The glide moves after the pitch is taken.
        let note = voice.note as f32;
        let glide = voice.glide;
        let span = ((voice.note - voice.previous) as f32).abs();
        let moved = pace(program.glide, per) * span;
        if note < glide {
            voice.glide = (glide - moved).max(note);
        } else if glide < note {
            voice.glide = (glide + moved).min(note);
        }
        let mut cents = (glide - f32::from(zone.root)) * f32::from(zone.keytrack);
        cents += f32::from(zone.coarse) * 100.0;
        cents += f32::from(zone.fine);
        cents += d[1] * 2400.0 - 1200.0;
        cents += part.bend_range as f32 * part.bend;
        cents += voice.levels.cents;
        let ratio = 2f64.powf(f64::from(cents) / 1200.0) * f64::from(source.rate);
        voice.step = (ratio / f64::from(step.rate) * 4_294_967_296.0) as i64;

        let mut amp = d[2];
        if !squared {
            amp *= zone.volume;
        }
        amp *= program.volume;
        amp *= part.volume;
        amp *= part.expression;
        let mut left = ((1.0 - part.pan) * (1.0 - d[3])).sqrt();
        let mut right = (part.pan * d[3]).sqrt();
        if squared {
            left = left * 2.0 * amp;
            right = right * 2.0 * amp;
        }
        let target = (left * amp, right * amp);
        let release = if voice.ended { 0.25 } else { d[16] };
        let (attack, decay) = (pace(d[13], step.rate), pace(d[14], step.rate));
        let (sustain, release) = (d[15], pace(release, step.rate));

        if voice.fresh {
            voice.limiter = 1.0;
            voice.filters = Default::default();
            voice.cutoffs = (d[7], d[10]);
            voice.gains = target;
            voice.set_frame(start as u32, false);
            voice.backwards = false;
            voice.looped = 0;
            if zone.sliced && zone.slices > 0 && voice.note >= i32::from(zone.root) {
                let slice = (voice.note - i32::from(zone.root)) % zone.slices;
                let at = f64::from(slice) * (f64::from(frames) / f64::from(zone.slices));
                voice.set_frame(at as i64 as i32 as u32, false);
            }
            voice.fresh = false;
            if voice.waiting {
                voice.waiting = false;
                voice.release(false);
            }
        }

        let [left_buf, right_buf, part_left, part_right] = buffers else {
            return;
        };
        let once = zone.mode < 2 || (zone.mode == 3 && voice.amp.state > 3);
        let wrap = |voice: &Voice| {
            (voice.frame() == (voice.high as u32).wrapping_sub(1) && voice.looped < 2)
                .then_some(voice.low as u32)
        };
        if once {
            voice.looped = 2;
            for n in 0..len {
                (left_buf[n], right_buf[n]) = source.read(voice.frame(), voice.fraction(), None);
                voice.position = voice.position.wrapping_add(voice.step);
                if frames as u32 <= voice.frame() {
                    voice.set_frame(frames as u32, false);
                    voice.ended = true;
                    voice.amp.release();
                }
            }
        } else if zone.mode == 2 || zone.mode == 3 {
            for n in 0..len {
                let to = wrap(voice);
                (left_buf[n], right_buf[n]) = source.read(voice.frame(), voice.fraction(), to);
                voice.position = voice.position.wrapping_add(voice.step);
                if voice.high as u32 <= voice.frame() {
                    let mut at = voice.low;
                    if voice.length > 0 {
                        let r = (voice.frame() as i32).wrapping_sub(voice.low) % voice.length;
                        at = if r < 0 { r + voice.length } else { r } + voice.low;
                    }
                    voice.set_frame(at as u32, true);
                    voice.looped = 1;
                }
            }
        } else if zone.mode == 4 {
            for n in 0..len {
                (left_buf[n], right_buf[n]) = source.read(voice.frame(), voice.fraction(), None);
                let mut back = voice.backwards;
                if !back {
                    voice.position = voice.position.wrapping_add(voice.step);
                    if voice.high as u32 <= voice.frame() {
                        voice.backwards = true;
                        back = true;
                    }
                }
                if back {
                    let before = voice.position;
                    voice.position = before.wrapping_sub(voice.step);
                    if voice.frame() <= voice.low as u32 || voice.position < 0 {
                        voice.backwards = false;
                    }
                }
            }
        } else {
            left_buf[..len].fill(0.0);
            right_buf[..len].fill(0.0);
        }

        if follows {
            for n in 0..len {
                let peak = left_buf[n].abs().max(right_buf[n].abs());
                if voice.follower < peak {
                    voice.follower = peak;
                }
                voice.follower *= step.smooth;
            }
        }

        if f1.kind != 0 || f2.kind != 0 {
            let smooth = (-1.0 / (per * 0.005)).exp();
            let q1 = FRAC_1_SQRT_2 / (1.01 - d[8]);
            let q2 = FRAC_1_SQRT_2 / (1.01 - d[11]);
            let c1 = coefficients(f1.kind, cutoff(voice.cutoffs.0), q1, step.rate);
            let c2 = coefficients(f2.kind, cutoff(voice.cutoffs.1), q2, step.rate);
            voice.cutoffs.0 = (voice.cutoffs.0 - d[7]) * smooth + d[7];
            voice.cutoffs.1 = (voice.cutoffs.1 - d[10]) * smooth + d[10];
            let [[l1, r1], [l2, r2]] = &mut voice.filters;
            for n in 0..len {
                let guard = rand.next() as f32 * 1e-14;
                let (xl, xr) = (guard + left_buf[n], guard + right_buf[n]);
                let (mut yl, mut yr) = (l1.run(xl, d[9], &c1), r1.run(xr, d[9], &c1));
                if zone.parallel {
                    yl += l2.run(xl, d[12], &c2);
                    yr += r2.run(xr, d[12], &c2);
                } else {
                    yl = l2.run(yl, d[12], &c2);
                    yr = r2.run(yr, d[12], &c2);
                }
                limit(
                    &mut yl,
                    &mut yr,
                    &mut voice.limiter,
                    step.attack,
                    step.release,
                );
                (left_buf[n], right_buf[n]) = (yl, yr);
            }
        }

        let dry = d[28];
        for n in 0..len {
            let level = voice.amp.next(attack, decay, sustain, release);
            let l = level * voice.gains.0 * left_buf[n];
            let r = level * voice.gains.1 * right_buf[n];
            voice.gains.0 = (voice.gains.0 - target.0) * step.smooth + target.0 + 1e-14;
            voice.gains.1 = (voice.gains.1 - target.1) * step.smooth + target.1 + 1e-14;
            if 1.0 <= dry {
                part_left[n] += l;
                part_right[n] += r;
            } else {
                part_left[n] += l * dry;
                part_right[n] += r * dry;
            }
        }
    }

    /// One step of every part (`FUN_18002b130`), added to the output.
    fn run(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len();
        let per_second = self.rate / len as f32;
        let step = Step {
            rate: self.rate,
            per_second,
            lfo: TAU * 20.0 / per_second,
            beat: (f64::from(per_second) * 15.0 / f64::from(self.tempo)) as f32,
            smooth: (-1.0 / (self.rate * 0.005)).exp(),
            attack: (-1000.0 / self.rate).exp(),
            release: (-10.0 / self.rate).exp(),
        };
        let flags = (self.squared(), self.clamped());
        let DirectWave {
            parts,
            programs,
            bank,
            short,
            voices,
            rand,
            scratch,
            ..
        } = self;
        for (index, part) in parts.iter().enumerate() {
            let program = &programs[usize::from(part.program).min(PROGRAMS - 1)];
            if program.zones.is_empty() {
                continue;
            }
            let [_, _, pl, pr] = scratch;
            pl[..len].fill(0.0);
            pr[..len].fill(0.0);
            for voice in voices.iter_mut() {
                if voice.part != index || (voice.amp.state < 1 && !voice.fresh) {
                    continue;
                }
                let Some(zone) = program.zones.get(voice.zone) else {
                    voice.amp.state = 0;
                    voice.fresh = false;
                    continue;
                };
                let source = zone.wave.and_then(|place| {
                    let wave = bank.wave(place)?;
                    Some(Source {
                        left: &wave.left,
                        right: wave.right.as_deref(),
                        short: short.get(place).copied().unwrap_or(false),
                        rate: wave.rate as f32,
                    })
                });
                Self::render(
                    voice, zone, program, part, source, &step, flags, rand, scratch, len,
                );
            }
            let [_, _, pl, pr] = scratch;
            for (i, shaper) in program.shapers.iter().enumerate() {
                if !shaper.0 {
                    continue;
                }
                let gain = 1.0 - shaper.1 * 0.75;
                let drive = 1.0 / (gain * gain * gain);
                let shape = |x: f32| {
                    if i == 0 {
                        (drive * x).tanh() * gain
                    } else {
                        (drive * x).sin() * gain
                    }
                };
                for x in pl[..len].iter_mut().chain(pr[..len].iter_mut()) {
                    *x = shape(*x);
                }
            }
            for n in 0..len {
                left[n] += pl[n];
                right[n] += pr[n];
            }
        }
    }
}

/// The MIDI velocity FL's host hands DirectWave for a note's gain, in
/// 128ths, the inverse of its velocity curve where a gain of 1 is 100.
fn velocity_of(gain: f32) -> f32 {
    ((10.0 * f64::from(gain.max(0.0)) + 1.0).ln() / 11f64.ln() * 100.0 / 128.0) as f32
}

impl Generator for DirectWave {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let read = Self::read(bytes)?;
        *self = DirectWave {
            rate: self.rate,
            tempo: self.tempo,
            ..read
        };
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm as f32;
    }

    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let key = i32::from(note.key).clamp(0, 127);
        let velocity = ((velocity_of(note.velocity) * 127.0) as i32).clamp(1, 127);
        let id = note.voice;
        self.notes[id as usize % NOTES] = (id, key);
        let levels = Levels {
            volume: note.velocity,
            pan: note.pan,
            cents: note.fine_pitch,
        };
        self.start(0, key, velocity, id, levels);
    }

    /// `FUN_180024e30`: the voices of the note let go, and a mono program
    /// goes back to the lowest key still held.
    fn note_off(&mut self, id: u32) {
        let (known, key) = self.notes[id as usize % NOTES];
        if known != id {
            return;
        }
        let part = 0;
        let program = self.program_of(part);
        self.parts[part].keys[key as usize] = Key::default();
        if self.parts[part].sustain > 0.5 {
            return;
        }
        let matches = |v: &&mut Voice| v.note == key && v.part == part && v.id == id;
        for voice in self.voices.iter_mut().filter(matches) {
            voice.release(false);
        }
        if self.programs[program].poly > 0 && key == self.parts[part].last {
            let held = self.parts[part].keys.iter().position(|k| k.velocity > 0);
            if let Some(next) = held {
                let held = self.parts[part].keys[next];
                self.start(part, next as i32, held.velocity, held.id, held.levels);
            }
        }
    }

    fn all_notes_off(&mut self) {
        for voice in &mut self.voices {
            voice.release(false);
        }
        for part in &mut self.parts {
            part.keys = [Key::default(); 128];
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let most = if self.settings[5] != 0 { 8 } else { STEP };
        let mut at = 0;
        while at < len {
            let n = most.min(len - at);
            self.run(&mut left[at..at + n], &mut right[at..at + n]);
            at += n;
        }
    }

    fn param(&self, _index: usize) -> f32 {
        0.0
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}

    fn reset(&mut self) {
        for voice in &mut self.voices {
            (voice.amp.state, voice.fresh) = (0, false);
        }
        for part in &mut self.parts {
            part.keys = [Key::default(); 128];
        }
    }

    /// A release of up to 10 s, or a file that plays once to its end at
    /// two octaves down.
    fn tail(&self) -> usize {
        let mut longest = 10.01f64;
        for zone in self.programs.iter().flat_map(|p| &p.zones) {
            if zone.mode == 1 && zone.rate > 0.0 {
                longest = longest.max(f64::from(zone.frames) / f64::from(zone.rate) * 4.0);
            }
        }
        (longest * f64::from(self.rate)) as usize
    }

    fn fidelity(&self) -> Fidelity {
        let programs = || self.programs.iter().filter(|p| !p.zones.is_empty());
        let zones = || programs().flat_map(|p| &p.zones);
        if self.bank.lacking() {
            return Fidelity::Approximate(
                "DirectWave: a sample file is missing, and its zone is silent",
            );
        }
        if programs().any(Program::sends) {
            return Fidelity::Approximate("DirectWave: a program's send effects are left out");
        }
        if zones().any(|z| z.filters.iter().any(|f| f.kind > 5)) {
            return Fidelity::Approximate("DirectWave: filter types 6 and 7 are left out");
        }
        if zones().any(|z| z.effects.iter().any(|e| e.0 != 0)) {
            return Fidelity::Approximate(
                "DirectWave: a zone's ring modulator, decimator, crusher or phaser is left out",
            );
        }
        if zones().any(|z| z.stretch == 1 || z.synced) {
            return Fidelity::Approximate("DirectWave: a zone's time stretching is left out");
        }
        let program_lfo = |z: &Zone| z.slots.iter().any(|s| s.source == 5 || s.source == 6);
        if zones().any(program_lfo) {
            return Fidelity::Approximate("DirectWave: the program LFOs are left out");
        }
        if self.settings[1..5] != [0; 4] {
            return Fidelity::Approximate("DirectWave: its high quality interpolation is left out");
        }
        if self.parts[0].output != 0 {
            return Fidelity::Approximate("DirectWave: every output plays into the channel");
        }
        Fidelity::Exact
    }

    fn sample_paths(&self) -> Vec<String> {
        self.bank.paths()
    }

    fn set_sample(&mut self, path: &str, file: Option<SampleFile<'_>>) {
        self.bank.set(path, file);
        self.short.resize(self.bank.slots.len(), false);
        if let Some(place) = self.bank.find(path) {
            self.short[place] = self.bank.wave(place).is_some_and(is_short);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fpc::tests::chunk;
    use crate::testing;

    /// A zone of a test state, as its chunks say it.
    #[derive(Clone)]
    struct Spec {
        path: &'static str,
        keys: (u8, u8, u8),
        velocities: (u8, u8),
        mode: i32,
        lp: (i32, i32),
        amp: [f32; 4],
        filter: (u8, f32, f32),
        slots: Vec<(i16, i16, f32)>,
        held: Option<(i32, Vec<f32>)>,
    }

    impl Default for Spec {
        fn default() -> Self {
            Spec {
                path: "a.wav",
                keys: (60, 0, 127),
                velocities: (0, 127),
                mode: 0,
                lp: (0, 0),
                amp: [0.0, 0.5, 1.0, 0.25],
                filter: (0, 0.5, 0.0),
                slots: vec![(2, 2, 1.0)],
                held: None,
            }
        }
    }

    fn f32s(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn zone(spec: &Spec) -> Vec<u8> {
        let (root, low, high) = spec.keys;
        let mut head = vec![
            root,
            low,
            high,
            spec.velocities.0,
            spec.velocities.1,
            0,
            0,
            0,
            0,
        ];
        head.extend(f32s(&[1.0, 0.5]));
        head.extend(0i32.to_le_bytes());
        head.extend(512i32.to_le_bytes());
        let mut body = chunk(500, &head);
        let mut path = spec.path.as_bytes().to_vec();
        path.push(0);
        body.extend(chunk(0x1f6, &path));
        let frames = spec.held.as_ref().map_or(0, |(_, v)| v.len() as i64);
        let mut file = frames.to_le_bytes().to_vec();
        file.extend(1i32.to_le_bytes());
        file.extend(0i32.to_le_bytes());
        file.extend(44_100f32.to_le_bytes());
        let bits = match &spec.held {
            Some((2, _)) => 16,
            _ => 32,
        };
        for word in [spec.mode, spec.lp.0, spec.lp.1, 0, bits] {
            file.extend(word.to_le_bytes());
        }
        body.extend(chunk(0x1f7, &file));
        let mut pitch = 0.5f32.to_le_bytes().to_vec();
        pitch.extend([0, 0]);
        pitch.extend(100i16.to_le_bytes());
        body.extend(chunk(0x1f8, &pitch));
        let mut sends = f32s(&[0.5, 0.5, 0.35, 1.0]);
        sends.extend([0; 32]);
        body.extend(chunk(0x1fa, &sends));
        let (kind, cut, res) = spec.filter;
        for kind in [kind, 0] {
            let mut filter = vec![kind, 0, 0, 0];
            filter.extend(f32s(&[cut, res, 0.0, 0.0]));
            body.extend(chunk(0x1fb, &filter));
        }
        body.extend(chunk(0x1fd, &f32s(&spec.amp)));
        for _ in 0..2 {
            body.extend(chunk(0x202, &f32s(&[0.0, 0.5, 0.5, 0.25])));
        }
        for slot in 0..SLOTS {
            let (source, target, amount) = spec.slots.get(slot).copied().unwrap_or((0, 0, 0.5));
            let mut b = source.to_le_bytes().to_vec();
            b.extend(target.to_le_bytes());
            b.extend(amount.to_le_bytes());
            body.extend(chunk(0x204, &b));
        }
        if let Some((width, values)) = &spec.held {
            let mut raw = Vec::new();
            let pad = |raw: &mut Vec<u8>, n: usize| raw.extend(vec![0; n * *width as usize]);
            pad(&mut raw, 256);
            for v in values {
                if *width == 2 {
                    raw.extend(((v * 32_768.0) as i16).to_le_bytes());
                } else {
                    raw.extend(v.to_le_bytes());
                }
            }
            pad(&mut raw, 256);
            body.extend(chunk(0x205, &raw));
        }
        body.extend(chunk(4, &[]));
        chunk(3, &body)
    }

    fn program(index: i32, poly: u8, zones: &[Spec]) -> Vec<u8> {
        let mut head = index.to_le_bytes().to_vec();
        head.extend([poly, 0]);
        head.extend(f32s(&[1.0, 0.0]));
        head.extend((zones.len() as i32).to_le_bytes());
        head.extend([0, 0]);
        head.extend([0; 10]);
        let mut body = chunk(100, &head);
        body.extend(chunk(0x66, b"Test\0"));
        body.extend(chunk(0x68, &[0; 10]));
        for zone in zones {
            body.extend(self::zone(zone));
        }
        body.extend(chunk(2, &[]));
        chunk(1, &body)
    }

    fn state(poly: u8, zones: &[Spec]) -> Vec<u8> {
        let mut bytes = 0x25i32.to_le_bytes().to_vec();
        bytes.extend(chunk(6, &[0; 16]));
        bytes.extend(chunk(0x206, &[1, 0, 0, 0, 0, 0, 0]));
        for part in 0..16i32 {
            let mut b = vec![0; 0x274];
            b[..4].copy_from_slice(&part.to_le_bytes());
            b[0x214..0x218].copy_from_slice(&200i32.to_le_bytes());
            for (at, v) in [(0x23c, 100.0 / 127.0), (0x240, 0.5), (0x244, 1.0)] {
                b[at..at + 4].copy_from_slice(&f32::to_le_bytes(v));
            }
            bytes.extend(chunk(5, &b));
        }
        bytes.extend(program(0, poly, zones));
        bytes
    }

    fn note(voice: u32, key: i16, velocity: f32) -> NoteOn {
        NoteOn {
            voice,
            key,
            velocity,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            slide: false,
            porta: false,
        }
    }

    fn file(left: &[f32]) -> SampleFile<'_> {
        SampleFile {
            left,
            right: None,
            rate: 44_100,
        }
    }

    fn ramp(len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| (n as f32 + 1.0) / (len as f32 + 1.0))
            .collect()
    }

    fn play(dw: &mut DirectWave, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        dw.process(&mut left, &mut right);
        (left, right)
    }

    fn kit(poly: u8, zones: &[Spec]) -> DirectWave {
        let mut dw = DirectWave::default();
        dw.set_state(&state(poly, zones)).unwrap();
        dw
    }

    /// What a centred note at full velocity comes out at: the pan law's
    /// half and the part's volume of 100.
    fn unity() -> f32 {
        0.5 * (100.0 / 127.0)
    }

    #[test]
    fn a_zone_plays_its_file_at_its_root_frame_by_frame_and_an_octave_up_twice_as_fast() {
        let sound = ramp(600);
        let mut dw = kit(0, &[Spec::default()]);
        assert_eq!(dw.sample_paths(), ["a.wav"]);
        assert!(matches!(dw.fidelity(), Fidelity::Approximate(what) if what.contains("missing")));
        dw.set_sample("a.wav", Some(file(&sound)));
        assert_eq!(dw.fidelity(), Fidelity::Exact);
        dw.note_on(note(1, 60, 1.0));
        let (left, right) = play(&mut dw, 700);
        for n in 0..512 {
            assert!(
                (left[n] - sound[n] * unity()).abs() < 1e-6,
                "{n}: {}",
                left[n]
            );
            assert_eq!(left[n], right[n]);
        }
        // The step the file ends in starts the release, a fast one, from
        // its first sample on.
        let fade = |n: usize| left[n] / (sound[n] * unity());
        assert!(fade(513) < fade(512) && fade(512) < 1.0 && fade(599) > 0.9);
        assert!(left[600..].iter().all(|v| *v == 0.0));
        dw.note_on(note(2, 72, 1.0));
        let (left, _) = play(&mut dw, 300);
        for n in 0..256 {
            assert!((left[n] - sound[2 * n] * unity()).abs() < 1e-6, "{n}");
        }
        // Halfway between two frames is halfway between their values.
        dw.note_on(note(3, 48, 1.0));
        let (left, _) = play(&mut dw, 21);
        assert!((left[21 - 1] - (sound[10] * unity())).abs() < 1e-6);
        let between = (sound[1] + sound[2]) / 2.0 * unity();
        assert!((left[3] - between).abs() < 1e-6, "{} {between}", left[3]);
    }

    #[test]
    fn a_loop_goes_round_and_a_one_shot_takes_no_note_off() {
        let sound = ramp(16);
        let looped = Spec {
            mode: 2,
            lp: (4, 8),
            ..Spec::default()
        };
        let mut dw = kit(0, &[looped]);
        dw.set_sample("a.wav", Some(file(&sound)));
        dw.note_on(note(1, 60, 1.0));
        let (left, _) = play(&mut dw, 40);
        let want: Vec<usize> = (0..8).chain((0..32).map(|n| 4 + n % 4)).collect();
        for (n, frame) in want.iter().enumerate() {
            assert!((left[n] - sound[*frame] * unity()).abs() < 1e-6, "{n}");
        }
        // The release takes the level down over 0.25^4 * 10 s + 1 ms.
        dw.note_off(1);
        let (left, _) = play(&mut dw, 2000);
        assert!(left[0] > 0.0 && left[1760] < left[0] * 1e-4);
        assert!(left[1800..].iter().all(|v| *v == 0.0));
        assert!(dw.voices.iter().all(|v| v.amp.state == 0));

        let once = Spec {
            mode: 1,
            ..Spec::default()
        };
        let long = vec![0.3; 4000];
        let mut dw = kit(0, &[once]);
        dw.set_sample("a.wav", Some(file(&long)));
        dw.note_on(note(1, 60, 1.0));
        dw.note_off(1);
        let (left, _) = play(&mut dw, 4100);
        assert!((left[3839] - 0.3 * unity()).abs() < 1e-6);
        assert!(left[3999] > 0.0 && left[4000..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_velocity_picks_the_zone_and_scales_the_volume() {
        let soft = Spec {
            path: "soft.wav",
            velocities: (0, 63),
            ..Spec::default()
        };
        let hard = Spec {
            path: "hard.wav",
            velocities: (64, 127),
            ..Spec::default()
        };
        let mut dw = kit(0, &[soft, hard]);
        // Not a 16-bit value, which FL would scale by 32768 / 32767.
        dw.set_sample("soft.wav", Some(file(&[0.3; 64])));
        dw.set_sample("hard.wav", Some(file(&[-0.3; 64])));
        // FL's gain for a MIDI velocity, and what DirectWave makes of it:
        // 128ths times 127, so 64 is 63.
        let gain = |velocity: f64| crate::knob_gain(velocity / 100.0) as f32;
        assert_eq!((velocity_of(gain(64.0)) * 127.0) as i32, 63);
        assert_eq!((velocity_of(gain(65.0)) * 127.0) as i32, 64);
        let fresh = dw.clone();
        dw.note_on(note(1, 60, gain(64.0)));
        let (left, _) = play(&mut dw, 8);
        assert!(
            (left[0] - 0.3 * gain(64.0) * unity()).abs() < 1e-6,
            "{}",
            left[0]
        );
        let mut dw = fresh;
        dw.note_on(note(2, 60, gain(65.0)));
        let (left, _) = play(&mut dw, 8);
        assert!(
            (left[0] + 0.3 * gain(65.0) * unity()).abs() < 1e-6,
            "{}",
            left[0]
        );
        // Keys outside the zones' ranges play nothing.
        let mut dw = kit(
            0,
            &[Spec {
                keys: (60, 50, 70),
                ..Spec::default()
            }],
        );
        dw.set_sample("a.wav", Some(file(&[1.0; 64])));
        dw.note_on(note(1, 71, 1.0));
        assert!(play(&mut dw, 8).0.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_envelope_rises_falls_and_squares_its_level() {
        let mut env = Envelope::default();
        env.start();
        assert_eq!(env.next(0.5, 0.25, 0.5, 0.1), 0.25);
        assert_eq!(env.next(0.5, 0.25, 0.5, 0.1), 1.0);
        assert_eq!((env.next(0.5, 0.25, 0.5, 0.1), env.state), (0.75 * 0.75, 2));
        assert_eq!((env.next(0.5, 0.25, 0.5, 0.1), env.state), (0.25, 3));
        assert_eq!(env.next(0.5, 0.25, 0.5, 0.1), 0.25);
        env.release();
        assert!((env.next(0.5, 0.25, 0.5, 0.1) - 0.16).abs() < 1e-6);
        for _ in 0..4 {
            env.next(0.5, 0.25, 0.5, 0.1);
        }
        assert_eq!((env.next(0.5, 0.25, 0.5, 0.1), env.state), (0.0, 0));
        // A time knob is its fourth power times 10 s, plus 1 ms.
        assert_eq!(pace(0.0, 44_100.0), 1.0);
        let samples = 1.0 / pace(0.5, 44_100.0);
        assert!((samples - (0.0625 * 441_000.0 + 44.1)).abs() < 0.01);
        assert!((cutoff(5.0 * 12.0 / 127.0) - 440.0).abs() < 0.01);
    }

    #[test]
    fn the_lowpass_passes_its_floor_and_stops_the_top() {
        let c = coefficients(1, 1000.0, FRAC_1_SQRT_2, 44_100.0);
        let dc = (c[0] + c[1] + c[2]) / (1.0 + c[3] + c[4]);
        let top = (c[0] - c[1] + c[2]) / (1.0 - c[3] + c[4]);
        assert!((dc - 1.0).abs() < 1e-5 && top.abs() < 1e-6, "{dc} {top}");
        let h = coefficients(2, 1000.0, FRAC_1_SQRT_2, 44_100.0);
        assert!((h[0] + h[1] + h[2]).abs() < 1e-6);
        assert_eq!(
            coefficients(0, 1000.0, 1.0, 44_100.0),
            [1.0, 0.0, 0.0, 0.0, 0.0]
        );
        // Two stages: a 1 kHz sine at 10 kHz cutoff passes, at 100 Hz it is
        // 40 dB down.
        let sine = testing::sine(1000.0 / 44_100.0, 4410);
        for (hz, low, high) in [(10_000.0, 0.95, 1.05), (100.0, 0.0, 0.012)] {
            let c = coefficients(1, hz, FRAC_1_SQRT_2, 44_100.0);
            let mut stage = Biquads::default();
            let out: Vec<f32> = sine.iter().map(|x| stage.run(*x, 0.0, &c)).collect();
            let peak = out[2205..].iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!(low < peak && peak < high, "{hz}: {peak}");
        }
        // The voice's filter at a cutoff of 0.47 takes the top off a
        // square wave.
        let square: Vec<f32> = (0..4410)
            .map(|n| if n / 4 % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let mut dw = kit(
            0,
            &[Spec {
                filter: (1, 0.47, 0.0),
                slots: vec![(2, 2, 1.0)],
                ..Spec::default()
            }],
        );
        dw.set_sample("a.wav", Some(file(&square)));
        dw.note_on(note(1, 60, 1.0));
        let (left, _) = play(&mut dw, 4410);
        let peak = left[1000..].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.01 * unity(), "{peak}");
    }

    #[test]
    fn samples_a_zone_carries_play_without_a_file() {
        let values = ramp(1000);
        for width in [2, 4] {
            let held = Spec {
                path: "Demo project\\gone.wav",
                held: Some((width, values.clone())),
                ..Spec::default()
            };
            let mut dw = kit(0, &[held]);
            // A held file has a place and no path.
            assert_eq!((dw.bank.slots.len(), dw.bank.paths().len()), (1, 0));
            assert!(dw.sample_paths().is_empty());
            assert_eq!(dw.fidelity(), Fidelity::Exact);
            dw.note_on(note(1, 60, 1.0));
            let (left, _) = play(&mut dw, 512);
            for n in 0..512 {
                let want = if width == 2 {
                    (values[n] * 32_768.0) as i16 as f32 * SHORT
                } else {
                    values[n]
                };
                assert!((left[n] - want * unity()).abs() < 1e-6, "{width} {n}");
            }
        }
    }

    #[test]
    fn a_mono_program_cuts_the_note_before() {
        let mut dw = kit(1, &[Spec::default()]);
        dw.set_sample("a.wav", Some(file(&[0.5; 8000])));
        dw.note_on(note(1, 60, 1.0));
        play(&mut dw, 64);
        dw.note_on(note(2, 62, 1.0));
        play(&mut dw, 4000);
        let sounding = dw.voices.iter().filter(|v| v.amp.state > 0).count();
        assert_eq!(sounding, 1);
        // Letting the new note go goes back to the one still held.
        dw.note_off(2);
        play(&mut dw, 64);
        let notes: Vec<i32> = dw
            .voices
            .iter()
            .filter(|v| (1..=3).contains(&v.amp.state))
            .map(|v| v.note)
            .collect();
        assert_eq!(notes, [60]);
    }

    #[test]
    fn a_state_fl_does_not_write_is_turned_down_and_an_old_empty_one_is_empty() {
        let mut dw = kit(0, &[Spec::default()]);
        let kept = dw.clone();
        assert_eq!(
            dw.set_state(&0x27i32.to_le_bytes()),
            Err(StateError::Version { found: 0x27 })
        );
        assert_eq!(dw, kept);
        let good = state(0, &[Spec::default()]);
        for len in [2, 10, good.len() - 30] {
            assert!(dw.set_state(&good[..len]).is_err(), "{len}");
            assert_eq!(dw, kept);
        }
        // Version 0x20: a mode, 16 parts and 128 programs of 0x1c4 bytes.
        let mut old = 0x20i32.to_le_bytes().to_vec();
        old.extend(vec![0; 4 + 0x2700 + 128 * 0x1c4]);
        dw.set_state(&old).unwrap();
        assert!(dw.programs().iter().all(|p| p.zones.is_empty()));
        old[8 + 0x2700 + 0x130] = 1;
        assert_eq!(dw.set_state(&old), Err(StateError::Version { found: 0x20 }));
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, DirectWave's states skipped");
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        let presets = saved.len();
        saved.extend(testing::projects(&root));
        let (mut states, mut zones, mut held) = (0, 0, 0);
        let click = vec![0.3; 1024];
        for saved in saved {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let mut dw = crate::generator_by_name(&saved.plugin).unwrap();
            if let Err(error) = dw.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            let read = DirectWave::read(&saved.state).unwrap();
            zones += read.programs().iter().map(|p| p.zones.len()).sum::<usize>();
            held += read.bank.slots.len() - read.bank.paths().len();
            // Longer than a step: a file that ends in the step its voice
            // starts in lets go of the voice before it is heard.
            let named = dw.sample_paths();
            for path in &named {
                dw.set_sample(path, Some(file(&click)));
            }
            for key in 0..128 {
                dw.note_on(note(key as u32, key, 1.0));
            }
            let (mut left, mut right) = ([0.0; 256], [0.0; 256]);
            dw.process(&mut left, &mut right);
            assert!(
                left.iter().chain(&right).all(|v| v.is_finite()),
                "{}",
                saved.file
            );
            let sounding = read.programs[read.program_of(0)]
                .zones
                .iter()
                .any(|z| z.wave.is_some());
            let heard = left.iter().any(|v| *v != 0.0);
            assert_eq!(heard, sounding, "{}", saved.file);
            states += 1;
        }
        eprintln!("DirectWave: {presets} presets, {states} states, {zones} zones, {held} carried");
        assert_eq!(states, 197);
    }
}
