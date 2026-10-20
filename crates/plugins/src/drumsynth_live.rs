//! Fruity DrumSynth Live: 120 drum patches, one for each key, each made of
//! two oscillators, a sampled wave or noise and a tone filter, as
//! `docs/plugins/fruity-drumsynth-live.md` and the listings of its
//! `drumsynth_eng_x64.dll` have it.
//!
//! # The state
//!
//! An i32 version from 2 to 4, the i32 key of the patch the editor shows,
//! the 120 names as Delphi short strings, 120 records of 38 i32 fields and,
//! from version 3 on, an i32 of flags. The records are not in the order of
//! the keys: the first 24 are keys 48 to 71, the next 48 keys 0 to 47 and
//! the rest keys 72 to 119. A field is the whole number of its control:
//!
//! | field | control | range |
//! |---|---|---|
//! | 0 Level | gain `(11^(v/128) - 1) / 10` | 0..128 |
//! | 1 Pan | added to the note's, over 64 | -64..64 |
//! | 2 Tone | `v/100 + 1/2` | -50..50 |
//! | 3 Osc1 harmonics | `v/100 + 1/2` | -50..50 |
//! | 4 Osc1 frequency | `v/1000` | 0..1000 |
//! | 5 Osc1 sweep start, or band width | `v/127` | 0..127 |
//! | 6 to 9 Osc1 sweep time, decay, level; Osc2 shape | `v/127` | 0..127 |
//! | 10 Osc2 frequency | `v/1000` | 0..1000 |
//! | 11 to 14 Osc2 width, attack, decay, level | `v/127` | 0..127 |
//! | 15 Wave | one of [`WAVES`], -1 for none | -1..17 |
//! | 16 Wave tune | `v/100 + 1/2` | -50..50 |
//! | 17 Wave mix: noise to wave | `v/127` | 0..127 |
//! | 18 Re-trigger count | | 0..100 |
//! | 19, 20, 21 Re-trigger rate, decay; release | `v/127` | 0..127 |
//! | 22 Osc1 phase invert | | 0, 1 |
//! | 23 Click | `v/100` | 0..100 |
//! | 24 Voice mode: mono, poly, groups 2 to 5 | | 0..5 |
//! | 25 to 31 Velocity to Osc1 frequency, Osc1 level, Osc2 frequency, Osc2 level, wave tune, tone, harmonics | `v/100 + 1/2` | -50..50 |
//! | 32 Osc1 mode: swept tone, noise band | | 0, 1 |
//! | 33 Ring modulation of Osc2 by Osc1 | `v/127` | 0..127 |
//! | 34 Morph to: a key, or its own for none | | -1..119 |
//! | 35, 36 Send level, send track | | 0..128 |
//! | 37 Morph amount | `v/128` | 0..128 |
//!
//! # A note
//!
//! The key picks the patch, and a key outside 0 to 119 plays nothing. The
//! patch's fields, morphed toward the patch of field 34 by field 37, go to
//! the engine, which works the voice out once at the note's start and
//! renders it in mono. The engine's own level is fixed at 45000 of 65536.
//! A voice of mode 0 cuts the voices of its key that are of mode 0 too,
//! and one of a group cuts that group's, unless the new note is of mode 1.
//!
//! Every sample: the noise of the engine's one generator and the wave,
//! mixed by field 17, drive Osc1's two resonators, the second of which
//! rings on its own when Osc1 is a swept tone. Its output is shaped by the
//! harmonics and decays to silence. Osc2 is two resonators on the same
//! mix, with an attack, a decay toward a level below 0, and ring
//! modulation by Osc1. Clipped to -1..1, the sum goes through a one-pole
//! lowpass or highpass set by the tone. A voice is over once both
//! envelopes are under 0.0001 and no re-trigger is to come.
//!
//! The voice's output is `ComputeLRVol` of the note's pan in FL's integer
//! steps plus field 1, at the note's gain times the level of field 0.
//!
//! What is not FL's:
//!
//! - FL's blocks are not ours. The noise generator is one for all the
//!   voices, each of which draws a block at a time, so the noise of two
//!   voices that sound together falls to them in another order.
//! - A cut voice fades out over the block after the cut, as FL's does, in
//!   a straight line from where it was. FL's ramp is not read.
//! - FL reads the note's gain once in the engine and once more for the
//!   level, so a channel's volume, which FL puts in that gain, counts
//!   twice in FL and once here.
//! - Sends to another mixer track (field 36 over 0) are not built. No
//!   shipped state has one: they hold 0 and -1, and FL turns a -1 of a
//!   state before version 4 into 0.
//! - 256 voices sound at once at most. A note past that cuts the oldest.

use crate::sampler::circular;
use crate::{Fidelity, Generator, NoteOn, StateError};
use std::path::Path;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity DrumSynth Live";

/// The engine's DLL, whose resource holds the waves, from the directory
/// [`Generator::set_resource_dir`] is handed.
pub const DLL: &str = "Fruity DrumSynth Live/drumsynth_eng_x64.dll";

/// How many keys have a patch, and how many fields a patch has.
pub const KEYS: usize = 120;
pub const FIELDS: usize = 38;

/// The engine's parameters.
const ENGINE: usize = 35;

/// The most voices that sound at once.
const VOICES: usize = 256;

/// Where each of the 18 waves is in the resource, in samples times 4096:
/// where it starts, the point past which it loops, and how far back the
/// loop goes. From the engine's constructor (`FUN_180001060`).
pub const WAVES: [(i32, i32, i32); 18] = [
    (0x1a80_d000, 0x1fc6_2000, 0x0545_5000),
    (0x1920_3000, 0x1a83_1000, 0x00c5_7000),
    (0x1fc9_3000, 0x24ef_d000, 0x0526_b000),
    (0x0000_0000, 0x0178_e000, 0x0001_0000),
    (0x0178_f000, 0x0707_d000, 0x0001_0000),
    (0x0707_e000, 0x0ee8_a000, 0x072c_b000),
    (0x0ee9_0000, 0x191e_5000, 0x06b9_0000),
    (0x24f0_9000, 0x262d_7000, 0x0001_0000),
    (0x262f_6000, 0x280c_4000, 0x0001_0000),
    (0x280c_4000, 0x2932_b000, 0x0001_0000),
    (0x2932_c000, 0x3191_2000, 0x0001_0000),
    (0x3191_2000, 0x3345_f000, 0x0001_0000),
    (0x3345_f000, 0x3589_0000, 0x0001_0000),
    (0x358a_b000, 0x3906_5000, 0x0001_0000),
    (0x3906_5000, 0x3cc0_d000, 0x0001_0000),
    (0x3cc0_d000, 0x4095_8000, 0x0001_0000),
    (0x4095_8000, 0x444e_c000, 0x0001_0000),
    (0x444e_c000, 0x4793_6000, 0x0001_0000),
];

/// The resource the engine reads the waves from: type "Waves", id 4003.
const WAVES_ID: u32 = 4003;

/// The noise generator's seed before the checksum of the waves is added.
const SEED: u32 = 0x56ce;

/// The bytes of the waves the checksum reads, one 16-bit sample every
/// 254 bytes.
const CHECKED: usize = 0x8f5c0;

/// The lowest and highest value of each field's control.
const RANGES: [(i32, i32); FIELDS] = [
    (0, 128),
    (-64, 64),
    (-50, 50),
    (-50, 50),
    (0, 1000),
    (0, 127),
    (0, 127),
    (0, 127),
    (0, 127),
    (0, 127),
    (0, 1000),
    (0, 127),
    (0, 127),
    (0, 127),
    (0, 127),
    (-1, 17),
    (-50, 50),
    (0, 127),
    (0, 100),
    (0, 127),
    (0, 127),
    (0, 127),
    (0, 1),
    (0, 100),
    (0, 5),
    (-50, 50),
    (-50, 50),
    (-50, 50),
    (-50, 50),
    (-50, 50),
    (-50, 50),
    (-50, 50),
    (0, 1),
    (0, 127),
    (-1, KEYS as i32 - 1),
    (0, 128),
    (0, 128),
    (0, 128),
];

/// The key of the record at this place in the state (`FUN_00719b00`).
pub fn key_of_record(record: usize) -> usize {
    match record {
        0..24 => record + 48,
        24..72 => record - 24,
        _ => record,
    }
}

/// The record of a key (`FUN_00719b40`).
pub fn record_of_key(key: usize) -> usize {
    match key {
        48..72 => key - 48,
        0..48 => key + 24,
        _ => key,
    }
}

/// FL's fader curve, which a Level and a send level are read through
/// (`FUN_00717ed0`).
pub fn level_gain(value: i32) -> f32 {
    let x = value as f32 * 0.007_812_5;
    if x == 1.0 {
        1.0
    } else {
        // The shell keeps ln 11 as a single.
        let ln11 = 11f64.ln() as f32;
        ((f64::from(x * ln11).exp() - 1.0) * f64::from(0.1f32)) as f32
    }
}

/// How far a patch moves toward the one it morphs to, for field 37
/// (`FUN_00719d70`).
pub fn morph_share(amount: i32) -> f32 {
    let x = (amount as f32 * 0.007_812_5).min(1.0);
    let mut share = (f64::from(x) * 1.6 - 0.3) as f32;
    if f64::from(share) > 0.9 {
        share = (f64::from(x) * 0.4 + 0.6) as f32;
    }
    if f64::from(share) < 0.1 {
        share = (f64::from(x) * 0.4) as f32;
    }
    share
}

/// What the engine makes of its parameters at a note's start
/// (`FUN_180001880`): each whole number as a float of its own scale.
fn scaled(engine: &[i32; ENGINE]) -> [f32; ENGINE] {
    std::array::from_fn(|index| {
        let value = engine[index] as f32;
        match index {
            0 | 1 => value * 1.525_878_9e-5,
            4 | 10 => value * 0.001,
            5..=9 | 11..=14 | 17 | 19..=21 | 33 => value * 0.007_874,
            23 => value * 0.01,
            15 | 18 | 22 | 24 | 32 => 0.0,
            _ => value * 0.01 + 0.5,
        }
    })
}

/// The rate and one sample of it, as the engine keeps them.
#[derive(Debug, Clone, Copy)]
struct Rate {
    hz: f32,
    step: f32,
}

impl Rate {
    fn new(hz: i32) -> Self {
        let hz = hz.max(1) as f32;
        Rate { hz, step: 1.0 / hz }
    }

    /// A one-pole coefficient of a time from 0 to 1 (`FUN_180002a80`).
    fn coefficient(self, time: f32) -> f32 {
        let speed = (f64::from(8.16f32) - f64::from(time) * 9.21).exp();
        1.0 - (speed * -f64::from(self.step)).exp() as f32
    }

    /// A resonator's frequency coefficient, of a frequency from 0 to 1 at
    /// the note's pitch, moved by velocity, and pulled in for a damping
    /// (`FUN_180002b50`).
    fn frequency(self, at: f32, ratio: f32, by: f32, velocity: f32, damping: f32) -> f32 {
        let t = (by - 0.5) * 2.23;
        let x = t * velocity * t * t + at * 6.907_77;
        let hz = ((f64::from(x).exp() * 16.0).floor() as f32 * ratio).min(16_000.0);
        let w = hz * (self.step * std::f32::consts::TAU);
        let pull = (0.04 - damping * 0.07) * w + damping * 0.3;
        w - pull * (w * w)
    }
}

/// A level from a knob, moved by velocity, and nothing under 0.01
/// (`FUN_180002ae0`).
fn level(knob: f32, by: f32, velocity: f32) -> f32 {
    let x = (by * 3.0 - 1.5) * velocity + knob * 6.9 - 4.7;
    let value = f64::from(x).exp() as f32;
    if 0.01 > knob { 0.0 } else { value }
}

/// The note as the engine sees it in FL's `TVoiceParams`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Played {
    /// `InitLevels.Vol` and `FinalLevels.Vol`.
    pub velocity: f32,
    /// Cents from the key, as a whole number.
    pub cents: i32,
    /// `FinalLevels.FCut` and `FinalLevels.FRes`.
    pub cut: f32,
    pub res: f32,
}

/// One voice of the engine: the fields of FL's 0xe0 bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct Engine {
    // The re-trigger: samples to the next, how many are left, the
    // period, and the levels and decays each one starts over with.
    wait: i32,
    left: i32,
    period: i32,
    osc2_again: f32,
    osc2_decay_again: f32,
    osc1_again: f32,
    osc1_decay_again: f32,
    noise: f32,
    wave: f32,
    at: i32,
    step: i32,
    end: i32,
    back: i32,
    // Osc1.
    sweep: f32,
    target: f32,
    freq: f32,
    offset: f32,
    damping: f32,
    a: [f32; 3],
    couple: f32,
    b: [f32; 3],
    shape: [f32; 4],
    osc1_env: f32,
    osc1_decay: f32,
    // Osc2.
    f9: f32,
    f10: f32,
    damping2: f32,
    c: [f32; 3],
    couple2: f32,
    d: [f32; 3],
    osc2_env: f32,
    osc2_decay: f32,
    floor: f32,
    rise: f32,
    attack: f32,
    ring: f32,
    // The tone filter and the gain.
    low: f32,
    tone: f32,
    dry: f32,
    gain: f32,
}

impl Engine {
    /// A voice of the engine's parameters (`FUN_180001880`). `noise` is
    /// the generator, of which the click may read a byte.
    pub fn start(engine: &[i32; ENGINE], rate: i32, note: Played, noise: u32) -> Self {
        let rate = Rate::new(rate);
        let p = scaled(engine);
        let mut v = Engine::default();
        let gain2 = note.velocity + note.velocity;
        let pitch = note.cents as f32 * 0.01 + 60.0;
        let w = rate.step * std::f32::consts::TAU;
        let vel = (note.velocity + note.velocity) - 1.0 + note.res;
        let ratio = (f64::from(pitch - 60.0) * f64::from_bits(0x3fad_9303_feaa_4efa)).exp() as f32;

        // The wave, read at its tune.
        let t = (p[29] + p[29]) - 1.0;
        let tune = (f64::from(vel * 0.5 * t * t * t + p[16] * 1.3863)).exp() as f32;
        let step = 8f32.min(tune * (rate.step * 22_050.0 * ratio)) * 4096.0;
        v.step = step as i32;
        let mut share = 1.0;
        match usize::try_from(engine[15]).ok().and_then(|w| WAVES.get(w)) {
            Some(&(at, end, back)) => {
                (v.at, v.end, v.back) = (at, end, back);
                v.wave = p[17] * 3.0;
                share = 1.0001 - p[17];
            }
            None => v.step = 0,
        }
        v.noise = (f64::from(rate.hz).sqrt() as f32) * 0.004_761_9 * share;

        // Osc1: a swept tone, or a band of the noise.
        let mut couple = 0.0;
        if engine[32] != 0 {
            let width = if p[5] > 0.5 { 1.0 } else { p[5] + p[5] };
            v.damping = width * width;
            couple = v.damping * width;
            v.couple = couple;
            let most = 1.98 - v.damping;
            let f = most.min(rate.frequency(p[4], ratio, p[25], vel, v.damping));
            v.freq = f;
            v.offset = 0.0;
            if p[5] > 0.5 {
                let k = 0.999 - (1.0 - p[5]) * 4.0 * (1.0 - p[5]);
                v.offset = 0.0 - (k + k) * f;
                v.freq = f * k + f;
                v.target = v.freq;
            } else {
                v.target = f;
            }
        } else {
            v.target = rate.frequency(p[4], ratio, 0.5, 0.0, 0.0);
            let from = if 0.5 > p[5] {
                (p[5] + p[5]) * p[4]
            } else {
                ((p[5] + p[5]) - 1.0) * (1.0 - p[4]) + p[4]
            };
            v.freq = rate.frequency(from, ratio, p[25], vel, 0.0);
        }

        // Osc2: a band of the noise and the wave.
        let width = if p[11] > 0.5 { 1.0 } else { p[11] + p[11] };
        v.damping2 = width * width;
        v.couple2 = v.damping2 * width;
        let most = 1.98 - v.damping2;
        let at = note.cut * 0.5 + p[10];
        let f = most.min(rate.frequency(at, ratio, p[27], vel, v.damping2));
        (v.f9, v.f10) = (f, f);
        if p[11] > 0.5 {
            let k = 0.999 - (1.0 - p[11]) * 4.0 * (1.0 - p[11]);
            v.f10 = f * k + f;
            v.f9 = f - f * k;
        }
        let osc2 = level(p[14], p[28], vel);
        v.osc2_env = osc2;
        let below = -(f64::from(p[9] * 9.0 - 7.0).exp() as f32);
        v.floor = osc2 * below;
        let span = f64::from((0.03 - below) / (1.0 - below)).ln() * f64::from(rate.step);
        let speed = f64::from(6.91 - p[13] * 9.21).exp();
        let osc2_decay = 1.0 - (speed * span).exp() as f32;
        v.osc2_decay = osc2_decay;
        v.attack = 0.0;
        v.rise = osc2;
        if p[12] > 0.005 {
            let time = f64::from(p[12] * 8.516_9 - 9.21).exp() as f32;
            v.rise = osc2 / (time * rate.hz);
        }

        // The click starts the second resonator of Osc1, and both of
        // Osc2, off rest.
        let mut click = p[23] * p[23];
        if p[23] > 0.9 {
            click = ((noise >> 8) & 0xff) as f32 * 0.0036;
        }
        let cube = click * click * click - 1.0;
        v.d = [click, cube, 0.0];
        v.b = [click, cube, 0.0];
        if engine[22] != 0 {
            v.couple = -couple;
            v.b = [-click, -cube, 0.0];
        }

        // The harmonics.
        let d = ((p[31] + p[31]) - 1.0) * vel + ((p[3] + p[3]) - 1.0);
        if 0.0 > d {
            let c = (-1f32).max(d);
            let k = c * -6.666_7 * c;
            v.shape = [0.0, 1.0 + k, -k, k];
        } else {
            let c = 1f32.min(d);
            let k = c * 4.708 * c;
            v.shape = [k * -0.25, c * c + 1.0, 0.0, k];
        }

        let osc1 = level(p[8], p[26], vel);
        v.osc1_env = osc1;
        v.osc1_decay = rate.coefficient(p[7]);
        v.sweep = rate.coefficient(p[6]);

        // The re-trigger.
        let count = engine[18];
        v.left = count.clamp(0, 100);
        if count > 0 {
            v.osc2_again = osc2;
            v.osc2_decay_again = osc2_decay;
            v.osc1_again = osc1;
            v.osc1_decay_again = v.osc1_decay;
            let period = f64::from(p[19] * 8.516_9 - 9.21).exp() as f32 * rate.hz;
            v.period = period as i32;
            v.wait = v.period;
            if 0.995 > p[20] {
                v.osc2_decay = rate.coefficient(p[20]);
                v.osc1_decay = v.osc2_decay;
            } else {
                v.osc2_decay = 0.0;
                v.osc1_decay = 0.0;
            }
        }

        // The tone: a lowpass under the middle and a highpass over it.
        let x = 1f32.min(0f32.max((p[30] - 0.5) * vel + p[2]));
        let (x, dry) = if x > 0.5 { (x - 0.5, -1.0) } else { (x, 1e-4) };
        let speed = (f64::from(x) * 14.0).exp() * (f64::from(-w) * 20.0);
        v.tone = 1.0 - speed.exp() as f32;
        v.dry = dry;
        v.ring = p[33];
        v.gain = level(p[0], 0.5, 0.0) * gain2;
        v
    }

    /// The note's release, with the parameters of its key
    /// (`FUN_180002430`). A release of 0.95 or more does nothing.
    pub fn release(&mut self, engine: &[i32; ENGINE], rate: i32) {
        let rate = Rate::new(rate);
        let unit = |index: usize| (engine[index] as f32 * 0.007_874).clamp(0.0, 1.0);
        let (osc1, osc2, release) = (unit(7), unit(13), unit(21));
        if 0 < self.period {
            self.left = 0;
        }
        if release < 0.95 {
            if self.attack < self.osc2_env {
                self.osc2_env = self.attack;
            }
            let r = release * 0.5 + 0.5;
            self.osc1_decay = rate.coefficient(r * osc1);
            self.osc1_decay_again = self.osc1_decay;
            self.rise = rate.coefficient(r * osc2);
            self.osc2_decay = self.rise;
            self.osc2_decay_again = self.rise;
        }
    }

    /// One sample (`FUN_180002560`), with the generator's noise and the
    /// waves.
    pub fn next(&mut self, noise: &mut u32, waves: &[i16]) -> f32 {
        self.wait = self.wait.wrapping_sub(1);
        if self.wait == 0 {
            self.left -= 1;
            self.osc2_env = self.osc2_again;
            self.osc1_env = self.osc1_again;
            self.attack = 0.0;
            if self.left > 0 {
                self.wait = self.period;
            } else {
                self.osc2_decay = self.osc2_decay_again;
                self.osc1_decay = self.osc1_decay_again;
            }
        }
        *noise = noise.wrapping_mul(0xbb3_8435).wrapping_add(0x3619_636b);
        let mut input = (f32::from_bits((*noise & 0x7f_ffff) + 0x4000_0000) - 3.0) * self.noise;
        if self.step != 0 {
            self.at = self.at.wrapping_add(self.step);
            if self.at > self.end {
                self.at = self.at.wrapping_sub(self.back);
            }
            let point = (self.at >> 12) as usize;
            let read = |k: usize| i32::from(waves.get(point + k).copied().unwrap_or(0));
            let (s0, s1) = (read(0), read(1));
            let bits = (s1 - s0) * ((self.at >> 5) & 0x7f) + ((s0 + 0x80_8000) << 7);
            input += (f32::from_bits(bits as u32) - 3.0) * self.wave;
        }

        // Osc1.
        let f = (self.target - self.freq) * self.sweep + self.freq;
        self.freq = f;
        let g = f + self.offset;
        let [y, s, last] = self.a;
        let y = y - (self.damping * y + s + input + last) * f;
        self.a = [y, y * g + s, input];
        let [z, t, last] = self.b;
        let z = z - (z * self.damping + t + (y + last) * self.couple) * f;
        self.b = [z, z * g + t, y];
        let bend = if z > 0.0 {
            self.shape[2]
        } else {
            self.shape[3]
        };
        let env = self.osc1_env;
        self.osc1_env = (1e-5 - env) * self.osc1_decay + env;
        let mut out = ((z * bend + self.shape[1]) * z + self.shape[0]) * env;

        // Osc2.
        let level = self.osc2_env;
        if 0.0 < level {
            let ring = (z - 1.0) * self.ring + 1.0;
            let [y, s, last] = self.c;
            let y = y - (self.damping2 * y + s + last + input) * self.f10;
            self.c = [y, self.f9 * y + s, input];
            let [q, u, last] = self.d;
            let q = q - ((y + last) * self.couple2 + (q * self.damping2 + u)) * self.f10;
            self.d = [q, q * self.f9 + u, y];
            let env = if self.attack < level {
                self.attack += self.rise;
                self.attack
            } else {
                self.osc2_env = (self.floor - level) * self.osc2_decay + level;
                self.osc2_env
            };
            out += env * (ring * q);
        }

        let x = 1f32.min((-1f32).max(out));
        self.low = (x - self.low) * self.tone + self.low;
        (x * self.dry + self.low) * self.gain
    }

    /// Whether the voice is over, as the engine says at the end of a
    /// block.
    pub fn done(&self) -> bool {
        self.left < 1 && self.osc1_env < 1e-4 && self.osc2_env < 1e-4
    }
}

/// One note: the engine's voice and what the plugin plays it at.
#[derive(Debug, Clone, Copy)]
struct Voice {
    id: u32,
    key: usize,
    /// Field 24 of the key's patch when the note started.
    mode: i32,
    /// Cut by another note: it fades out over the next block.
    cut: bool,
    /// FL's pan of the note in steps of 1/64, and its gain.
    pan: i32,
    velocity: f32,
    /// The gains the last block ended at, none before the first.
    ramp: Option<(f32, f32)>,
    engine: Engine,
}

/// The waves out of the engine's DLL, and the seed their checksum gives.
#[derive(Debug, Clone, PartialEq)]
pub struct Waves {
    pub pcm: Vec<i16>,
    pub seed: u32,
}

impl Waves {
    /// From the bytes of `drumsynth_eng_x64.dll`. `None` for a file with
    /// no such resource, or one too short for the 18 waves.
    pub fn from_dll(bytes: &[u8]) -> Option<Self> {
        let data = resource(bytes, "WAVES", WAVES_ID)?;
        let last = WAVES.iter().map(|w| (w.1 >> 12) as usize + 2).max()?;
        if data.len() < CHECKED || data.len() < 2 * last {
            return None;
        }
        let pcm: Vec<i16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect();
        let sum = (0..CHECKED).step_by(0xfe).fold(SEED as i32, |sum, at| {
            sum.wrapping_add(i32::from(i16::from_le_bytes([data[at], data[at + 1]])))
        });
        Some(Waves {
            pcm,
            seed: sum as u32,
        })
    }
}

/// The data of a resource of a 64-bit DLL, by a type's name and an id, in
/// its first language.
fn resource<'a>(bytes: &'a [u8], kind: &str, id: u32) -> Option<&'a [u8]> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?));
    let header = u32_at(0x3c)? as usize;
    let optional = header.checked_add(24)?;
    if bytes.get(header..header + 4)? != b"PE\0\0" || u16_at(optional)? != 0x20b {
        return None;
    }
    let sections = (
        optional + usize::from(u16_at(header + 20)?),
        u16_at(header + 6)?,
    );
    let file = |rva: u32| -> Option<usize> {
        (0..usize::from(sections.1)).find_map(|k| {
            let at = sections.0 + 40 * k;
            let (size, start, raw) = (u32_at(at + 8)?, u32_at(at + 12)?, u32_at(at + 20)?);
            let into = rva.checked_sub(start).filter(|into| *into < size)?;
            Some(raw as usize + into as usize)
        })
    };
    let root = file(u32_at(optional + 112 + 16)?)?;
    // The entries of a directory: name or id, and where what it names is.
    let entries = |dir: usize| -> Option<Vec<(u32, u32)>> {
        let count = usize::from(u16_at(dir + 12)?) + usize::from(u16_at(dir + 14)?);
        (0..count)
            .map(|k| Some((u32_at(dir + 16 + 8 * k)?, u32_at(dir + 20 + 8 * k)?)))
            .collect()
    };
    let named = |name: u32| -> Option<String> {
        let at = root + (name & 0x7fff_ffff) as usize;
        let len = usize::from(u16_at(at)?);
        let units: Option<Vec<u16>> = (0..len).map(|k| u16_at(at + 2 + 2 * k)).collect();
        Some(String::from_utf16_lossy(&units?))
    };
    let sub = |to: u32| (to & 0x8000_0000 != 0).then(|| root + (to & 0x7fff_ffff) as usize);
    let (_, types) = entries(root)?.into_iter().find(|(name, _)| {
        name & 0x8000_0000 != 0 && named(*name).is_some_and(|n| n.eq_ignore_ascii_case(kind))
    })?;
    let (_, ids) = entries(sub(types)?)?
        .into_iter()
        .find(|(name, _)| *name == id)?;
    let (_, leaf) = *entries(sub(ids)?)?.first()?;
    let leaf = root + leaf as usize;
    let (rva, size) = (u32_at(leaf)?, u32_at(leaf + 4)? as usize);
    let at = file(rva)?;
    bytes.get(at..at.checked_add(size)?)
}

/// The plugin: 120 patches and the voices that sound.
pub struct DrumSynthLive {
    version: i32,
    /// The key the editor shows, and the flags of version 3 on.
    selected: i32,
    flags: u32,
    names: Vec<String>,
    /// The patches, by key.
    patches: Box<[[i32; FIELDS]; KEYS]>,
    levels: [f32; KEYS],
    rate: i32,
    waves: Option<Waves>,
    noise: u32,
    voices: Vec<Voice>,
}

impl Default for DrumSynthLive {
    fn default() -> Self {
        // The fields of a new patch are the controls' defaults.
        let mut patch = [0; FIELDS];
        for (field, value) in [
            (0, 75),
            (5, 64),
            (6, 64),
            (7, 64),
            (8, 64),
            (9, 64),
            (11, 64),
            (12, 64),
            (13, 64),
            (14, 64),
            (15, -1),
            (17, 64),
            (19, 64),
            (20, 64),
            (21, 64),
            (33, 64),
            (35, 50),
            (37, 64),
        ] {
            patch[field] = value;
        }
        let mut patches = Box::new([patch; KEYS]);
        for (key, patch) in patches.iter_mut().enumerate() {
            patch[34] = key as i32;
        }
        DrumSynthLive {
            version: 4,
            selected: 0,
            flags: 0,
            names: vec![String::new(); KEYS],
            patches,
            levels: [level_gain(75); KEYS],
            rate: 44_100,
            waves: None,
            noise: SEED,
            voices: Vec::with_capacity(VOICES),
        }
    }
}

impl DrumSynthLive {
    /// The name of each key's patch.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The key the editor shows.
    pub fn selected(&self) -> i32 {
        self.selected
    }

    /// The fields of a key's patch.
    pub fn patch(&self, key: usize) -> Option<&[i32; FIELDS]> {
        self.patches.get(key)
    }

    /// The engine's parameters for a note on `key`: its patch, morphed
    /// toward the one field 34 names (`FUN_00719d70`).
    fn engine(&self, key: usize) -> [i32; ENGINE] {
        let patch = &self.patches[key];
        let mut engine = [0; ENGINE];
        engine[0] = 45_000;
        let to = usize::try_from(patch[34])
            .ok()
            .filter(|to| *to != key && *to < KEYS);
        let share = morph_share(patch[37]);
        for field in 2..34 {
            if field == 24 {
                continue;
            }
            let mut value = patch[field];
            if let Some(to) = to
                && field != 15
            {
                let other = self.patches[to][field];
                let moved = value as f32 + share * (other - value) as f32;
                value = f64::from(moved).round_ties_even() as i32;
            }
            if field == 15 && value == -1 {
                value = 0x80;
            }
            engine[field] = value;
        }
        engine
    }

    /// Where a parameter is: its key and field.
    fn place(index: usize) -> Option<(usize, usize)> {
        (index < KEYS * FIELDS).then(|| (key_of_record(index / FIELDS), index % FIELDS))
    }

    fn stop(&mut self) {
        self.voices.clear();
    }
}

impl Generator for DrumSynthLive {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let short = StateError::Length {
            expected: 19_467,
            found: bytes.len(),
        };
        let i32_at = |at: usize| {
            bytes
                .get(at..)
                .and_then(|rest| rest.first_chunk())
                .map(|word| i32::from_le_bytes(*word))
        };
        let version = i32_at(0).ok_or(short)?;
        if !(2..=4).contains(&version) {
            return Err(StateError::Version { found: version });
        }
        let selected = i32_at(4).ok_or(short)?;
        let mut at = 8;
        let mut names = Vec::with_capacity(KEYS);
        for _ in 0..KEYS {
            let len = usize::from(*bytes.get(at).ok_or(short)?);
            let text = bytes.get(at + 1..at + 1 + len).ok_or(short)?;
            names.push(text.iter().map(|byte| char::from(*byte)).collect());
            at += 1 + len;
        }
        let mut patches = Box::new([[0; FIELDS]; KEYS]);
        for record in 0..KEYS {
            let patch = &mut patches[key_of_record(record)];
            for (field, value) in patch.iter_mut().enumerate() {
                *value = i32_at(at + 4 * (record * FIELDS + field)).ok_or(short)?;
            }
            // Before version 4 a send track of -1 is none.
            if version < 4 && patch[36] == -1 {
                patch[36] = 0;
            }
        }
        at += 4 * KEYS * FIELDS;
        let flags = match version {
            2 => 0,
            _ => i32_at(at).ok_or(short)? as u32,
        };
        let end = at + if version == 2 { 0 } else { 4 };
        if bytes.len() != end {
            return Err(StateError::Length {
                expected: end,
                found: bytes.len(),
            });
        }
        self.version = version;
        self.selected = selected;
        self.flags = flags;
        self.names = names;
        self.levels = std::array::from_fn(|key| level_gain(patches[key][0]));
        self.patches = patches;
        Ok(())
    }

    /// Cuts every note.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz.round() as i32;
        self.stop();
    }

    /// Reads the waves out of the engine's DLL. Waves that were read stay
    /// where the directory has none.
    fn set_resource_dir(&mut self, dir: &Path) {
        let read = std::fs::read(dir.join(DLL)).ok();
        if let Some(waves) = read.as_deref().and_then(Waves::from_dll) {
            if self.waves.is_none() {
                self.noise = waves.seed;
            }
            self.waves = Some(waves);
            self.stop();
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let Some(key) = usize::try_from(note.key).ok().filter(|key| *key < KEYS) else {
            return;
        };
        let mode = self.patches[key][24];
        if mode != 1 {
            for voice in &mut self.voices {
                if (voice.mode == 0 && voice.key == key) || (voice.mode > 1 && voice.mode == mode) {
                    voice.cut = true;
                }
            }
        }
        let played = Played {
            velocity: note.velocity,
            cents: note.fine_pitch.round() as i32,
            cut: note.mod_x,
            res: note.mod_y,
        };
        let engine = Engine::start(&self.engine(key), self.rate, played, self.noise);
        if self.voices.len() == VOICES {
            self.voices.remove(0);
        }
        self.voices.push(Voice {
            id: note.voice,
            key,
            mode,
            cut: false,
            pan: (note.pan * 64.0).round() as i32,
            velocity: note.velocity,
            ramp: None,
            engine,
        });
    }

    fn note_off(&mut self, voice: u32) {
        for slot in 0..self.voices.len() {
            if self.voices[slot].id == voice {
                let engine = self.engine(self.voices[slot].key);
                self.voices[slot].engine.release(&engine, self.rate);
            }
        }
    }

    fn all_notes_off(&mut self) {
        for slot in 0..self.voices.len() {
            let engine = self.engine(self.voices[slot].key);
            self.voices[slot].engine.release(&engine, self.rate);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let waves = self.waves.as_ref().map_or(&[][..], |waves| &waves.pcm[..]);
        let mut slot = 0;
        while slot < self.voices.len() {
            let voice = &mut self.voices[slot];
            let (to_left, to_right) = if voice.cut {
                (0.0, 0.0)
            } else {
                let pan = (voice.pan + self.patches[voice.key][1]) as f32 / 64.0;
                circular(pan, voice.velocity * self.levels[voice.key])
            };
            let (from_left, from_right) = voice.ramp.unwrap_or((to_left, to_right));
            let steps = (
                (to_left - from_left) / len as f32,
                (to_right - from_right) / len as f32,
            );
            for (n, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                let value = voice.engine.next(&mut self.noise, waves);
                *l += value * (from_left + steps.0 * n as f32);
                *r += value * (from_right + steps.1 * n as f32);
            }
            voice.ramp = Some((to_left, to_right));
            let silent = voice.cut && to_left == 0.0 && to_right == 0.0;
            if silent || voice.engine.done() {
                self.voices.remove(slot);
            } else {
                slot += 1;
            }
        }
    }

    fn param_count(&self) -> usize {
        KEYS * FIELDS
    }

    /// A field of a patch, numbered by its record in the state.
    fn param(&self, index: usize) -> f32 {
        Self::place(index).map_or(0.0, |(key, field)| self.patches[key][field] as f32)
    }

    /// The next note hears it, and a level or a pan at once.
    fn set_param(&mut self, index: usize, value: f32) {
        let Some((key, field)) = Self::place(index) else {
            return;
        };
        let (low, high) = RANGES[field];
        let value = if value.is_nan() { 0.0 } else { value.round() };
        self.patches[key][field] = value.clamp(low as f32, high as f32) as i32;
        if field == 0 {
            self.levels[key] = level_gain(self.patches[key][0]);
        }
    }

    fn reset(&mut self) {
        self.stop();
    }

    /// As long as the slowest patch takes to fall to silence after its
    /// re-triggers.
    fn tail(&self) -> usize {
        let rate = Rate::new(self.rate);
        let frames = |coefficient: f32| {
            // From the most an envelope can start at down to 0.0001.
            let fall = f64::from(1.0 - coefficient.clamp(1e-9, 1.0)).ln();
            if fall < 0.0 {
                (1e-4f64 / 3000.0).ln() / fall
            } else {
                0.0
            }
        };
        let mut most = 0.0f64;
        for key in 0..KEYS {
            let p = scaled(&self.engine(key));
            let slowest = [p[7], p[13], p[20]]
                .into_iter()
                .map(|time| frames(rate.coefficient(time)))
                .fold(0.0, f64::max);
            let again = f64::from(p[19] * 8.516_9 - 9.21).exp() * f64::from(rate.hz);
            let count = f64::from(self.patches[key][18].clamp(0, 100));
            most = most.max(slowest + count * again);
        }
        // Osc2's attack takes half a second at most.
        (most + f64::from(rate.hz) * 0.5).min(f64::from(rate.hz) * 600.0) as usize + 1
    }

    fn fidelity(&self) -> Fidelity {
        let waved = self
            .patches
            .iter()
            .any(|patch| (0..18).contains(&patch[15]));
        if waved && self.waves.is_none() {
            Fidelity::Approximate("DrumSynth Live waves not available")
        } else if self.patches.iter().any(|patch| patch[36] > 0) {
            Fidelity::Approximate("DrumSynth Live sends not built")
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

    /// A state of version 4 with these patches by key and the defaults
    /// elsewhere.
    fn state(set: &[(usize, usize, i32)]) -> Vec<u8> {
        let plugin = DrumSynthLive::default();
        let mut patches = *plugin.patches;
        for &(key, field, value) in set {
            patches[key][field] = value;
        }
        let mut bytes = Vec::new();
        bytes.extend(4i32.to_le_bytes());
        bytes.extend(60i32.to_le_bytes());
        for key in 0..KEYS {
            let name = format!("Patch {key}");
            bytes.push(name.len() as u8);
            bytes.extend(name.as_bytes());
        }
        for record in 0..KEYS {
            for value in patches[key_of_record(record)] {
                bytes.extend(value.to_le_bytes());
            }
        }
        bytes.extend(1u32.to_le_bytes());
        bytes
    }

    fn with(set: &[(usize, usize, i32)]) -> DrumSynthLive {
        let mut plugin = DrumSynthLive::default();
        plugin.set_state(&state(set)).unwrap();
        plugin
    }

    fn play(plugin: &mut DrumSynthLive, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        for (l, r) in left.chunks_mut(100).zip(right.chunks_mut(100)) {
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

    #[test]
    fn the_records_are_keys_48_to_71_then_0_to_47_then_the_rest() {
        assert_eq!(key_of_record(0), 48);
        assert_eq!(key_of_record(23), 71);
        assert_eq!(key_of_record(24), 0);
        assert_eq!(key_of_record(71), 47);
        assert_eq!(key_of_record(72), 72);
        for record in 0..KEYS {
            assert_eq!(record_of_key(key_of_record(record)), record);
        }
        // A parameter is numbered by its record.
        let mut plugin = with(&[(48, 4, 500), (0, 4, 600)]);
        assert_eq!(plugin.param(4), 500.0);
        assert_eq!(plugin.param(24 * FIELDS + 4), 600.0);
        plugin.set_param(24 * FIELDS + 4, 2000.0);
        assert_eq!(plugin.patch(0).unwrap()[4], 1000);
        assert_eq!(plugin.param(KEYS * FIELDS), 0.0);
    }

    #[test]
    fn the_state_is_names_and_records_and_a_short_or_new_one_is_turned_down() {
        let bytes = state(&[(60, 0, 100)]);
        let mut plugin = DrumSynthLive::default();
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.names()[3], "Patch 3");
        assert_eq!(plugin.selected(), 60);
        assert_eq!(plugin.patch(60).unwrap()[0], 100);
        assert!(plugin.set_state(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(plugin.set_state(&longer).is_err());
        let mut newer = bytes.clone();
        newer[0] = 5;
        assert_eq!(
            plugin.set_state(&newer),
            Err(StateError::Version { found: 5 })
        );
        // Version 2 has no flags, and before 4 a send to -1 is none.
        let mut older = bytes.clone();
        older[0] = 2;
        older.truncate(bytes.len() - 4);
        let at = older.len() - 4 * (KEYS - record_of_key(5)) * FIELDS + 4 * 36;
        older[at..at + 4].copy_from_slice(&(-1i32).to_le_bytes());
        plugin.set_state(&older).unwrap();
        assert_eq!(plugin.patch(5).unwrap()[36], 0);
        assert_eq!(plugin.patch(60).unwrap()[0], 100);
    }

    #[test]
    fn the_level_is_fl_s_fader_curve_and_the_morph_a_bent_line() {
        assert_eq!(level_gain(128), 1.0);
        assert_eq!(level_gain(0), 0.0);
        let half = level_gain(64);
        assert!(
            (half - ((11f32.sqrt() - 1.0) / 10.0)).abs() < 1e-6,
            "{half}"
        );
        assert_eq!(morph_share(0), 0.0);
        assert!((morph_share(64) - 0.5).abs() < 1e-6);
        assert!((morph_share(128) - 1.0).abs() < 1e-6);
        // Under a quarter it is 0.4 x, over the middle it bends again.
        assert!((morph_share(16) - 0.05).abs() < 1e-6);
        assert!((morph_share(112) - 0.95).abs() < 1e-6);
        // A morph moves every field but the wave and the mode, rounded.
        let plugin = with(&[(60, 34, 61), (60, 37, 64), (61, 4, 900), (61, 15, 3)]);
        let engine = plugin.engine(60);
        assert_eq!(engine[4], 450);
        assert_eq!(engine[15], 0x80);
        assert_eq!(engine[0], 45_000);
    }

    #[test]
    fn the_resonator_frequency_is_16_times_a_power_of_1000_at_the_pitch() {
        let rate = Rate::new(44_100);
        let w = rate.frequency(0.5, 1.0, 0.5, 1.0, 0.0);
        let hz = (16.0 * (0.5f64 * 6.90777).exp()).floor();
        let expected = hz * std::f64::consts::TAU / 44_100.0;
        let expected = expected - 0.04 * expected.powi(3);
        assert!((f64::from(w) - expected).abs() < 1e-5, "{w} {expected}");
        // Never over 16 kHz.
        assert_eq!(
            rate.frequency(1.0, 4.0, 0.5, 0.0, 0.0),
            rate.frequency(1.0, 2.0, 0.5, 0.0, 0.0)
        );
        // A time of 0 is a coefficient near 1/13 at 44.1 kHz, 1 near none.
        assert!((rate.coefficient(0.0) - 0.0761).abs() < 1e-3);
        assert!(rate.coefficient(1.0) < 1e-4);
    }

    /// A swept tone of no sweep rings at the frequency of field 4.
    #[test]
    fn a_swept_tone_rings_at_its_frequency_and_falls_away() {
        let hz = (16.0 * (0.4f64 * 6.90777).exp()).floor();
        let mut plugin = with(&[
            (60, 4, 400),
            (60, 5, 51),
            (60, 6, 0),
            (60, 7, 100),
            (60, 8, 64),
            (60, 14, 0),
            (60, 15, -1),
        ]);
        plugin.set_sample_rate(44_100.0);
        // Osc2 is at 0, and a swept tone hears none of the noise.
        plugin.note_on(note(1, 60));
        let (left, right) = play(&mut plugin, 44_100);
        assert_eq!(left, right);
        let turns = crossings(&left[..44_100 / 4]) as f64 * 4.0;
        assert!((turns - hz).abs() < hz * 0.05, "{turns} {hz}");
        let early = left[..4410].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let late = left[40_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(early > 0.1 && late < early * 0.5, "{early} {late}");
    }

    #[test]
    fn a_key_picks_its_patch_and_none_past_119() {
        let mut plugin = with(&[(60, 8, 127), (61, 8, 0), (61, 14, 0)]);
        plugin.note_on(note(1, 120));
        plugin.note_on(note(2, -1));
        assert!(plugin.voices.is_empty());
        plugin.note_on(note(3, 61));
        let (left, _) = play(&mut plugin, 4410);
        // Key 61 has Osc1 and Osc2 at 0: only its noise rests, which
        // neither oscillator passes.
        assert!(left.iter().all(|v| v.abs() < 1e-3));
        plugin.note_on(note(4, 60));
        let (left, _) = play(&mut plugin, 4410);
        assert!(left.iter().any(|v| v.abs() > 0.05));
    }

    #[test]
    fn mono_cuts_its_own_key_and_a_group_its_group_over_one_block() {
        let mut plugin = with(&[(60, 24, 0), (62, 24, 3), (63, 24, 3), (64, 24, 1)]);
        plugin.note_on(note(1, 60));
        plugin.note_on(note(2, 62));
        play(&mut plugin, 100);
        plugin.note_on(note(3, 60));
        plugin.note_on(note(4, 63));
        plugin.note_on(note(5, 64));
        let cut: Vec<(u32, bool)> = plugin.voices.iter().map(|v| (v.id, v.cut)).collect();
        assert_eq!(
            cut,
            [(1, true), (2, true), (3, false), (4, false), (5, false)]
        );
        play(&mut plugin, 100);
        let left: Vec<u32> = plugin.voices.iter().map(|v| v.id).collect();
        assert_eq!(left, [3, 4, 5]);
    }

    #[test]
    fn a_voice_ends_once_its_envelopes_are_under_a_ten_thousandth() {
        let mut plugin = with(&[(60, 7, 0), (60, 13, 0), (60, 12, 0)]);
        plugin.note_on(note(1, 60));
        let tail = plugin.tail();
        play(&mut plugin, tail);
        assert!(plugin.voices.is_empty(), "{tail}");
        // A re-trigger plays its count of hits at its rate.
        let mut plugin = with(&[(60, 18, 3), (60, 19, 80), (60, 20, 20), (60, 7, 20)]);
        plugin.note_on(note(1, 60));
        let period = plugin.voices[0].engine.period as usize;
        assert!(period > 400, "{period}");
        let (left, _) = play(&mut plugin, period * 5);
        let peak = |from: usize| {
            left[from..from + period / 4]
                .iter()
                .fold(0.0f32, |m, v| m.max(v.abs()))
        };
        let hits: Vec<f32> = (0..5).map(|k| peak(k * period)).collect();
        assert!(
            hits[1] > hits[0] * 0.3 && hits[3] > hits[0] * 0.3,
            "{hits:?}"
        );
        assert!(hits[4] < hits[3] * 0.5, "{hits:?}");
    }

    #[test]
    fn the_pan_and_level_go_through_compute_lr_vol() {
        let mut plugin = with(&[(60, 1, 64), (60, 0, 128)]);
        plugin.note_on(note(1, 60));
        let (left, right) = play(&mut plugin, 1000);
        assert!(left.iter().all(|v| v.abs() < 1e-6));
        assert!(right.iter().any(|v| v.abs() > 0.05));
        let mut plugin = with(&[(60, 0, 0)]);
        plugin.note_on(note(1, 60));
        let (left, _) = play(&mut plugin, 1000);
        assert!(left.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_waves_are_read_out_of_a_dll_s_resource() {
        assert!(Waves::from_dll(b"MZ").is_none());
        assert!(Waves::from_dll(&[]).is_none());
    }

    fn fl_s_directory() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, DrumSynth Live's files and states skipped");
            return None;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        Some((root, dir))
    }

    #[test]
    fn fl_s_dll_has_18_waves_and_a_seed() {
        let Some((_, dir)) = fl_s_directory() else {
            return;
        };
        let waves = Waves::from_dll(&std::fs::read(dir.join(DLL)).unwrap()).unwrap();
        assert_eq!(waves.pcm.len(), 587_366 / 2);
        assert_ne!(waves.seed, SEED);
        // Every wave is heard: none is all zeros.
        for (at, end, _) in WAVES {
            let wave = &waves.pcm[(at >> 12) as usize..(end >> 12) as usize];
            assert!(wave.iter().any(|v| v.unsigned_abs() > 1000));
        }
    }

    #[test]
    fn every_state_in_fl_s_projects_loads_and_plays_out() {
        let Some((root, dir)) = fl_s_directory() else {
            return;
        };
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let saved: Vec<_> = testing::projects(&root).into_iter().filter(ours).collect();
        assert_eq!(saved.len(), 9);
        for saved in saved {
            let mut plugin = DrumSynthLive::default();
            plugin.set_state(&saved.state).unwrap();
            assert_ne!(plugin.fidelity(), Fidelity::Exact, "{}", saved.file);
            plugin.set_resource_dir(&dir);
            assert_eq!(plugin.fidelity(), Fidelity::Exact, "{}", saved.file);
            let key = plugin.selected() as i16;
            plugin.note_on(note(1, key));
            plugin.note_off(1);
            let tail = plugin.tail();
            let (left, right) = play(&mut plugin, tail);
            let peak = left
                .iter()
                .chain(&right)
                .fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(left.iter().chain(&right).all(|v| v.is_finite()));
            assert!((0.01..4.0).contains(&peak), "{peak} in {}", saved.file);
            assert!(plugin.voices.is_empty(), "{} past {tail}", saved.file);
        }
    }
}
