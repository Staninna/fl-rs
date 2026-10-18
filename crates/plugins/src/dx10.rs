//! Fruity DX10: Paul Kellett's mda DX10, a two-operator FM synth, as
//! Image-Line rebuilt it with a second modulator, as `docs/plugins/fruity-dx10.md`
//! reads it out of `Fruity DX10_x64.dll`.
//!
//! # The state
//!
//! An i32 version, which FL writes as 2 and reads below 3, then the 22
//! parameters as i32, 88 bytes. Version 1 ends there, at 92 bytes. Version 2
//! adds four bytes FL does not read and the instrument tab, 364 bytes: the
//! five envelope and LFO blocks and the filter that 3x Osc has too
//! ([`crate::fruit_kick::Articulation`]). FL takes the values as they come,
//! without a look at the range of their wheel.
//!
//! | index | parameter | range |
//! |---|---|---|
//! | 0 | attack | 0 to 65536 |
//! | 1 | decay, at its top no decay | 0 to 65536 |
//! | 2 | release | 0 to 65536 |
//! | 3 | modulator 1 coarse ratio | 0 to 65536 |
//! | 4 | modulator 1 fine ratio | 0 to 65536 |
//! | 5 | modulator 1 initial depth | 0 to 65536 |
//! | 6 | modulator 1 decay | 0 to 65536 |
//! | 7 | modulator 1 sustain depth | 0 to 65536 |
//! | 8 | modulator 1 release | 0 to 65536 |
//! | 9 | modulator 1 velocity sensitivity | 0 to 65536 |
//! | 10 | vibrato | 0 to 65536 |
//! | 11 | waveform | 0 to 65536 |
//! | 12 | modulator thru | 0 to 65536 |
//! | 13 | LFO rate | 0 to 65536 |
//! | 14 to 20 | modulator 2: coarse, fine, initial depth, decay, sustain, release, velocity | 0 to 65536 |
//! | 21 | octave | -3 to 3 |
//!
//! Each goes to `p = (value - low) / (high - low)` and the maths reads `p`.
//!
//! # A note
//!
//! The carrier is mda's: a phase from -1 to 1 that turns at the key's
//! frequency, read through a fifth-order polynomial whose shape the
//! waveform sets. Its phase is moved every sample by two sine modulators,
//! each read from FL's 16384-point sine table with a 32-bit phase that
//! turns at its ratio of half the carrier's frequency, each with its own
//! depth envelope that falls from the initial depth to the sustain depth.
//! The velocity scales the depths. The carrier's envelope rises at the
//! attack rate and falls at the decay rate; a note off makes it, and the
//! modulators' depths, fall at their release rates. Modulator 1 can also
//! be heard as it is, by the modulator thru. One LFO adds vibrato, and it
//! is stepped every 100 samples of every voice, so it runs faster for every
//! voice that sounds. A voice ends when its envelope is under 0.0002.
//!
//! FL scales the voice so that its envelope starts at 1.2 whatever the
//! velocity, and the velocity reaches the level only through FL's pan law,
//! with the note's pan, as the instrument tab mixes every voice.
//!
//! # What is not FL's
//!
//! - The instrument tab is read but not played. A state whose tab would
//!   change the sound (an envelope or LFO that moves its target, the volume
//!   at zero, a filter that is not wide open) is approximate.
//! - A note with mod X under 0 or mod Y over 0 goes through the tab's Fast
//!   LP in FL. It plays unfiltered here.
//! - FL plays every voice the channel lets it; here a note past [`VOICES`]
//!   takes the place of the quietest, by FL's own rule for a channel
//!   whose polyphony is set to that.
//! - A slide note sounds nothing, and a voice's pitch does not move once
//!   it starts. FL moves the pitch whenever the host's does.
//! - The voices are added into the block one after the other, where FL
//!   sums them in a buffer of its own first.

use crate::fruit_kick::Articulation;
use crate::{Fidelity, Generator, NoteOn, StateError, knob_value};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity DX10";

/// How many parameters there are.
pub const PARAMS: usize = 22;

/// The most voices that sound at once here. FL sets no limit of its own.
pub const VOICES: usize = 64;

/// The voices kept, the stolen ones that play one more block included.
const SLOTS: usize = 2 * VOICES;

/// The range of each parameter's wheel, from the DLL's form.
pub const RANGES: [(i32, i32); PARAMS] = {
    let mut ranges = [(0, 65_536); PARAMS];
    ranges[OCTAVE] = (-3, 3);
    ranges
};

const OCTAVE: usize = 21;

/// What a new Fruity DX10 plays: the constructor's "Bright E.Piano", mda
/// DX10's first program.
const DEFAULTS: [i32; PARAMS] = [
    0, 42_598, 28_901, 55_181, 21_561, 15_073, 52_429, 3277, 52_429, 58_982, 0, 29_295, 0, 27_132,
    0, 0, 0, 0, 0, 0, 0, 0,
];

/// A version 1 state's length, and where version 2's tab starts.
const SHORT: usize = 4 + 4 * PARAMS;
const TAB: usize = SHORT + 4;
const LONG: usize = TAB + 364;

/// A voice ends at the end of the block its envelope is under this.
const QUIET: f32 = 0.0002;

/// A depth envelope under this, as bits, goes to 0 at the end of a block.
const TINY_BITS: i32 = 0x3380_0000;

/// The samples between two steps of the LFO, for each voice.
const LFO_STEP: i32 = 100;

/// The table FL reads the modulators from, its index the phase's top 14
/// bits.
const SINE_SHIFT: u32 = 18;

/// The phase a modulator turns by for a ratio of 1 and a carrier step of
/// 1: a quarter turn.
const QUARTER: f32 = 1_073_741_824.0;

/// What `exp(k * 0.05776226505)` is a key's step of: `ln 2 / 12`.
const SEMITONE: f64 = 0.057_762_265_05;

/// FL's `Round`: to the nearest, and to even from a half.
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// The fine ratio, `FUN_00735690`: mda's, save that FL rounds where mda
/// truncates.
fn fine_ratio(p: f32) -> f32 {
    let p = f64::from(p);
    if p >= 0.5 {
        match round(p * 8.9) {
            4 => 0.25,
            5 => 1.0 / 3.0,
            6 => 0.5,
            7 => 2.0 / 3.0,
            _ => 0.75,
        }
    } else {
        (p * 0.2 * p) as f32
    }
}

/// The coarse ratio, `FUN_00735660`: `floor(40.1 p^2)`.
fn coarse_ratio(p: f32) -> f32 {
    let p = f64::from(p);
    (p * 40.1 * p).floor() as f32
}

/// What a modulator's parameters come to.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ModParams {
    /// The phase step for a carrier step of 1.
    ratio: f32,
    depth: f32,
    sustain: f32,
    velocity: f32,
    decay: f32,
    release: f32,
}

/// What the parameters come to at a rate, `FUN_00735760`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Derived {
    /// The carrier's step at key 0.
    tune: f32,
    mods: [ModParams; 2],
    vibrato: f32,
    attack: f32,
    decay: f32,
    release: f32,
    rich: f32,
    modmix: f32,
    lfo: f32,
}

impl Derived {
    fn of(p: &[f32; PARAMS], rate: i32) -> Self {
        let inverse = (1.0 / f64::from(rate)) as f32;
        let per = (44_100.0 / f64::from(rate)) as f32;
        let sq = |p: f32, k: f64| f64::from(p) * k * f64::from(p);
        let fall = |k: f64| (f64::from(-inverse) * k.exp()).exp();
        let octave = (f64::from(p[OCTAVE]) * 6.9).floor() as i32;
        let tune = (f64::from(inverse) * 8.175_798_915_644 * 2f64.powi(octave - 2)) as f32;
        let modulator = |at: usize, velocity: usize| {
            let level = |p: f32| (f64::from(per) * 0.0002 * f64::from(p) * f64::from(p)) as f32;
            ModParams {
                ratio: (coarse_ratio(p[at]) + fine_ratio(p[at + 1])) * QUARTER,
                depth: level(p[at + 2]),
                sustain: level(p[at + 4]),
                velocity: p[velocity],
                decay: (1.0 - fall(6.0 - f64::from(p[at + 3]) * 7.0)) as f32,
                release: (1.0 - fall(5.0 - f64::from(p[at + 5]) * 8.0)) as f32,
            }
        };
        Derived {
            tune,
            mods: [modulator(3, 9), modulator(14, 20)],
            vibrato: sq(p[10], 0.001) as f32,
            attack: (1.0 - fall(8.0 - f64::from(p[0]) * 8.0)) as f32,
            decay: match f64::from(p[1]) > 0.98 {
                true => 1.0,
                false => fall(5.0 - f64::from(p[1]) * 8.0) as f32,
            },
            release: fall(5.0 - f64::from(p[2]) * 5.0) as f32,
            rich: (0.5 - sq(p[11], 3.0)) as f32,
            modmix: sq(p[12], 0.25) as f32,
            lfo: (f64::from(inverse) * 15_707.499_999_999_998 * sq(p[13], 1.0)) as f32,
        }
    }
}

/// One modulator of a voice.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Modulator {
    step: u32,
    phase: u32,
    gain: f32,
    /// The depth now and where it is going.
    depth: f32,
    target: f32,
    rate: f32,
}

/// One voice, FL's 0x78-byte record.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    id: u32,
    dying: bool,
    /// The host's pitch it was started at, cents from key 60.
    pitch: f32,
    /// The envelope, and the level that follows it at the attack rate.
    env: f32,
    level: f32,
    attack: f32,
    decay: f32,
    /// 1.2 over the envelope's start.
    scale: f32,
    mods: [Modulator; 2],
    carrier: f32,
    step: f32,
    /// Samples to the next LFO step, and the vibrato it last gave.
    count: i32,
    vibrato: f32,
    gains: [f32; 2],
}

/// The plugin.
#[derive(Debug, Clone)]
pub struct Dx10 {
    values: [i32; PARAMS],
    p: [f32; PARAMS],
    rate: i32,
    derived: Derived,
    tab: Articulation,
    /// The LFO, a sine and a cosine, which every voice steps.
    lfo: [f32; 2],
    voices: Vec<Voice>,
    /// The mod wheel, `+0x188`, which nothing in the DLL moves from 0.
    wheel: f32,
    /// A note came with mod X or mod Y that FL's filter would hear.
    filtered: bool,
    sine: &'static [f32],
}

impl Default for Dx10 {
    fn default() -> Self {
        let mut dx10 = Dx10 {
            values: DEFAULTS,
            p: [0.0; PARAMS],
            rate: 44_100,
            derived: Derived::default(),
            tab: default_tab(),
            lfo: [0.0, 1.0],
            wheel: 0.0,
            voices: Vec::with_capacity(SLOTS),
            filtered: false,
            sine: crate::three_osc::host_table(0),
        };
        dx10.update();
        dx10
    }
}

/// The instrument tab of a new plugin, `FUN_00722bc0`.
fn default_tab() -> Articulation {
    let block = [
        0, 0, 100, 20_000, 20_000, 30_000, 50, 20_000, 0, 100, 20_000, 0, 32_950, 0, 0, 0, 0,
    ];
    let mut blocks = [block; 5];
    blocks[1][0] = 4;
    blocks[1][16] = -101;
    Articulation {
        blocks,
        cutoff: 256,
        resonance: 0,
        filter: 0,
    }
}

/// Whether FL would play a voice differently for the tab: what
/// [`Articulation::is_heard`] says, and also a pitch envelope that moves,
/// or a volume envelope that is off and does not start at full level.
fn tab_is_heard(tab: &Articulation) -> bool {
    let volume = &tab.blocks[1];
    let silent = volume[1] == 0 && volume[0] & 4 == 0;
    tab.is_heard() || tab.blocks[4][8] != 0 || silent
}

impl Dx10 {
    /// `ProcessParam` with `REC_UpdateValue`: the value as `p`, then every
    /// derived value again.
    fn update(&mut self) {
        for (n, (low, high)) in RANGES.iter().enumerate() {
            self.p[n] = (self.values[n] - low) as f32 / (high - low) as f32;
        }
        self.derived = Derived::of(&self.p, self.rate);
    }

    /// `FUN_00735d90`: with the voices at the limit, the quietest that is
    /// not dying is cut, and it plays out one more block.
    fn steal(&mut self) {
        if self.voices.len() < VOICES {
            return;
        }
        let mut live = 0;
        let mut quietest: Option<usize> = None;
        let mut lowest = 2.0f32;
        for (n, voice) in self.voices.iter().enumerate() {
            if !voice.dying {
                live += 1;
                if voice.env < lowest {
                    lowest = voice.env;
                    quietest = Some(n);
                }
            }
        }
        if live >= VOICES
            && let Some(n) = quietest
        {
            self.voices[n].dying = true;
            self.voices[n].env = 0.0;
        }
    }

    /// `TriggerVoice`, `FUN_00735e30`.
    fn start(&self, note: &NoteOn) -> Voice {
        let d = &self.derived;
        let pitch = (f32::from(note.key) - 60.0) * 100.0 + note.fine_pitch;
        let cents = round(f64::from(pitch));
        let fraction = (cents % 100) as f32 * 0.01;
        let key = (pitch * 0.01 - fraction + 60.0) as i64;
        let key = key.clamp(0, 127) as f32 + fraction;
        let velocity = Self::velocity(note.velocity);
        let l = (f64::from(key) * SEMITONE).exp() as f32;
        let step = d.tune * l;
        let tracked = if f64::from(l) > 50.0 { 50.0 } else { l };
        let modulator = |m: &ModParams| {
            let k = f64::from(m.velocity * (velocity - 64.0)) + 64.0;
            let k = (f64::from(tracked) * k) as f32;
            Modulator {
                step: round(f64::from(m.ratio * step)) as u32,
                phase: 0,
                gain: 1.0,
                depth: m.depth * k,
                target: m.sustain * k,
                rate: m.decay,
            }
        };
        let env = ((1.5 - f64::from(self.p[11]))
            * f64::from(0.0035f32)
            * f64::from(velocity + 10.0)) as f32;
        Voice {
            id: note.voice,
            dying: false,
            pitch,
            env,
            level: 0.0,
            attack: d.attack,
            decay: d.decay,
            scale: (1.2 / f64::from(env)) as f32,
            mods: [modulator(&d.mods[0]), modulator(&d.mods[1])],
            carrier: 0.0,
            step,
            count: 0,
            vibrato: 0.0,
            gains: crate::three_osc::pan_law(note.pan, note.velocity),
        }
    }

    /// The 0 to 127 the voice reads its velocity as, from the host's
    /// velocity gain: `127 ln(10 v + 1) / ln(21.551182)`, rounded.
    fn velocity(gain: f32) -> f32 {
        const TOP: f32 = 20.551_182;
        let v = (gain * 10.0).min(TOP);
        let x = (f64::from(v + 1.0)).ln() / f64::from(TOP + 1.0).ln();
        round(f64::from(x as f32 * 127.0)).clamp(0, 1270) as f32
    }

    /// `Voice_Release`, `FUN_00736320`.
    fn release(&self, voice: &mut Voice) {
        let d = &self.derived;
        voice.decay = d.release;
        voice.env = voice.level;
        voice.attack = 1.0;
        for (m, params) in voice.mods.iter_mut().zip(&d.mods) {
            m.target = 0.0;
            m.rate = params.release;
        }
    }

    /// `Voice_Render`, `FUN_007364a0`, for one voice, added into the block
    /// through the voice's gains. True when the voice has ended.
    fn render(&mut self, n: usize, left: &mut [f32], right: &mut [f32]) -> bool {
        let d = self.derived;
        let sine = self.sine;
        let mut v = self.voices[n];
        let mut lfo = self.lfo;
        let mut step = v.step + v.vibrato;
        let mut carrier = v.carrier;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if v.count == 0 {
                lfo[0] += d.lfo * lfo[1];
                lfo[1] -= d.lfo * lfo[0];
                v.vibrato = lfo[1] * (self.wheel + d.vibrato);
                step = v.step + v.vibrato;
                v.count = LFO_STEP;
            } else {
                v.count -= 1;
            }
            v.level += v.attack * (v.env - v.level);
            v.env *= v.decay;
            let [m1, m2] = &mut v.mods;
            let s1 = m1.gain * sine[(m1.phase >> SINE_SHIFT) as usize];
            let s2 = sine[(m2.phase >> SINE_SHIFT) as usize];
            carrier = carrier + step + m1.depth * s1 + m2.depth * m2.gain * s2;
            m1.depth += m1.rate * (m1.target - m1.depth);
            m2.depth += m2.rate * (m2.target - m2.depth);
            m1.phase = m1.phase.wrapping_add(m1.step);
            m2.phase = m2.phase.wrapping_add(m2.step);
            while carrier.abs() > 1.0 {
                carrier -= 2.0f32.copysign(carrier);
            }
            let c = carrier;
            let shape = c + c * c * c * (d.rich * (c * c - 1.0) - 1.0);
            let out = v.scale * v.level * (d.modmix * s1 + shape);
            *l += out * v.gains[0];
            *r += out * v.gains[1];
        }
        v.carrier = carrier;
        if (v.env.to_bits() as i32) < QUIET.to_bits() as i32 {
            v.decay = 1.0;
            v.dying = true;
        }
        for m in &mut v.mods {
            if (m.depth.to_bits() as i32) < TINY_BITS {
                m.depth = 0.0;
                m.target = 0.0;
            }
        }
        self.voices[n] = v;
        self.lfo = lfo;
        v.dying
    }
}

impl Generator for Dx10 {
    /// FL's `SaveRestoreState`: a version under 3, then the 22 values and,
    /// from 460 bytes on, the instrument tab. A shorter state has the tab
    /// of a new plugin.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = crate::state_version(bytes, SHORT)?;
        if !(..3).contains(&version) {
            return Err(StateError::Version { found: version });
        }
        let Some(values) = bytes.get(4..SHORT) else {
            return Err(StateError::Length {
                expected: SHORT,
                found: bytes.len(),
            });
        };
        for (value, word) in self.values.iter_mut().zip(values.as_chunks::<4>().0) {
            *value = i32::from_le_bytes(*word);
        }
        let tab = bytes
            .get(TAB..)
            .and_then(|b| b.first_chunk::<{ LONG - TAB }>());
        self.tab = tab.map_or_else(default_tab, Articulation::read);
        self.update();
        self.voices.clear();
        self.filtered = false;
        Ok(())
    }

    /// The voices that sound keep what they were started with.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = (hz.round() as i32).max(1);
        self.update();
    }

    /// `TriggerVoice`. Nothing sounds for a slide note.
    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        self.filtered |= note.mod_x < 0.0 || note.mod_y > 0.0;
        self.steal();
        if self.voices.len() == SLOTS {
            let gone = self.voices.iter().position(|v| v.dying).unwrap_or(0);
            self.voices.remove(gone);
        }
        let voice = self.start(&note);
        self.voices.push(voice);
    }

    fn note_off(&mut self, voice: u32) {
        for n in 0..self.voices.len() {
            if self.voices[n].id == voice {
                let mut v = self.voices[n];
                self.release(&mut v);
                self.voices[n] = v;
            }
        }
    }

    fn all_notes_off(&mut self) {
        for n in 0..self.voices.len() {
            let mut v = self.voices[n];
            self.release(&mut v);
            self.voices[n] = v;
        }
    }

    /// `Gen_Render`, `FUN_00736a10`: each voice in turn over the whole
    /// block, then the ones that ended go.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.voices.is_empty() {
            return;
        }
        let len = left.len().min(right.len());
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let mut ended = [false; SLOTS];
        for (n, ended) in ended.iter_mut().enumerate().take(self.voices.len()) {
            *ended = self.render(n, left, right);
        }
        let mut n = 0;
        self.voices.retain(|_| {
            n += 1;
            !ended[n - 1]
        });
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.values.get(index).map_or(0.0, |v| *v as f32)
    }

    /// The voices that sound keep what they started with; the LFO's rate
    /// and the vibrato, the waveform and the modulator thru change at once.
    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS {
            self.values[index] = knob_value(value, RANGES[index]);
            self.update();
        }
    }

    fn reset(&mut self) {
        self.voices.clear();
    }

    /// The longest a released voice takes to fall from the highest
    /// envelope a note can start at to 0.0002 at the release rate.
    fn tail(&self) -> usize {
        let start = (1.5 - f64::from(self.p[11])) * 0.0035 * 137.0;
        let per_sample = -f64::from(self.derived.release).ln();
        let frames = match per_sample > 0.0 {
            true => (start / f64::from(QUIET)).ln().max(0.0) / per_sample,
            false => f64::from(self.rate) * 60.0,
        };
        frames.ceil() as usize + 1
    }

    fn fidelity(&self) -> Fidelity {
        if tab_is_heard(&self.tab) {
            return Fidelity::Approximate(
                "Fruity DX10: the instrument tab's envelopes, LFOs or filter are not built",
            );
        }
        if self.filtered {
            return Fidelity::Approximate("Fruity DX10: mod X or mod Y would filter a note");
        }
        Fidelity::Exact
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

    fn with(values: [i32; PARAMS]) -> Dx10 {
        let mut words = vec![1];
        words.extend(values);
        let mut dx10 = Dx10::default();
        dx10.set_sample_rate(48_000.0);
        dx10.set_state(&testing::state(&words)).unwrap();
        dx10
    }

    fn play(dx10: &mut Dx10, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            dx10.process(l, r);
        }
        (left, right)
    }

    /// A plain carrier: no modulation, a waveform that is mda's sine.
    fn plain() -> [i32; PARAMS] {
        let mut values = DEFAULTS;
        values[5] = 0;
        values[7] = 0;
        values[12] = 0;
        values[1] = 65_536;
        values
    }

    #[test]
    fn the_default_state_is_mda_s_first_program() {
        let dx10 = Dx10::default();
        assert_eq!(dx10.values, DEFAULTS);
        assert_eq!(dx10.tab, default_tab());
        assert_eq!(dx10.fidelity(), Fidelity::Exact);
        // mda's 0.65 decay and 0.842 coarse, in FL's 65536ths.
        assert_eq!(dx10.p[1], 42_598.0 / 65_536.0);
    }

    #[test]
    fn a_state_is_read_by_its_version_and_length() {
        let mut dx10 = Dx10::default();
        let mut words = vec![2];
        words.extend(plain());
        assert!(dx10.set_state(&testing::state(&words)).is_ok());
        assert_eq!(dx10.values, plain());
        words[0] = 3;
        assert_eq!(
            dx10.set_state(&testing::state(&words)),
            Err(StateError::Version { found: 3 })
        );
        assert_eq!(
            dx10.set_state(&testing::state(&words[..10])),
            Err(StateError::Version { found: 3 })
        );
        words[0] = 1;
        assert_eq!(
            dx10.set_state(&testing::state(&words[..10])),
            Err(StateError::Length {
                expected: 92,
                found: 40
            })
        );
        // Version 2 with its tab: the volume envelope on is heard.
        let mut long = words.clone();
        long[0] = 2;
        long.push(0);
        let mut tab: Vec<i32> = default_tab().blocks.concat();
        tab.extend([0, 0, 256, 0, 0, 0]);
        assert_eq!(tab.len(), 91);
        long.extend(&tab);
        assert!(dx10.set_state(&testing::state(&long)).is_ok());
        assert_eq!(dx10.fidelity(), Fidelity::Exact);
        long[SHORT / 4 + 1 + 17 + 1] = 1;
        dx10.set_state(&testing::state(&long)).unwrap();
        assert!(matches!(dx10.fidelity(), Fidelity::Approximate(_)));
    }

    #[test]
    fn the_ratios_are_mda_s_with_the_fine_one_rounded() {
        assert_eq!(coarse_ratio(0.0), 0.0);
        // floor(40.1 * 0.842^2) = floor(28.43)
        assert_eq!(coarse_ratio(55_181.0 / 65_536.0), 28.0);
        assert_eq!(coarse_ratio(1.0), 40.0);
        assert_eq!(fine_ratio(0.25), (0.25f64 * 0.2 * 0.25) as f32);
        // 8.9 * 0.55 = 4.895: mda truncates to 4 (1/4), FL rounds to 5.
        assert_eq!(fine_ratio(0.55), 1.0 / 3.0);
        assert_eq!(fine_ratio(0.5), 0.25);
        assert_eq!(fine_ratio(1.0), 0.75);
    }

    #[test]
    fn the_octave_wheel_is_the_octave() {
        for octave in -3..=3 {
            let mut values = DEFAULTS;
            values[OCTAVE] = octave;
            let dx10 = with(values);
            let tune = 8.175_798_915_644 / 48_000.0 * 2f64.powi(octave + 1);
            assert!((f64::from(dx10.derived.tune) / tune - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn velocity_100_is_99_and_the_top_is_127() {
        assert_eq!(Dx10::velocity(1.0), 99.0);
        assert_eq!(Dx10::velocity(2.05), 127.0);
        assert_eq!(Dx10::velocity(0.0), 0.0);
    }

    #[test]
    fn an_unmodulated_a_plays_at_440_hz_at_1_2_through_the_pan_law() {
        let mut dx10 = with(plain());
        dx10.note_on(note(1, 69));
        let (left, right) = play(&mut dx10, 48_000);
        assert_eq!(left, right);
        // Rising zero crossings over the second half second.
        let half = &left[24_000..];
        let rises = half
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!((219..=221).contains(&rises), "{rises}");
        // mda's polynomial at waveform 0.447 peaks near 0.6 of its input;
        // the voice is 1.2 times it, and 0.7071 of that on each side.
        let peak = half.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let shape = |c: f32| {
            let rich = dx10.derived.rich;
            c + c * c * c * (rich * (c * c - 1.0) - 1.0)
        };
        let top = (0..=1000)
            .map(|k| shape(k as f32 / 1000.0).abs())
            .fold(0.0, f32::max);
        let expected = 1.2 * top * std::f32::consts::FRAC_1_SQRT_2;
        assert!((peak / expected - 1.0).abs() < 0.01, "{peak} {expected}");
    }

    #[test]
    fn a_released_note_ends_within_the_tail() {
        let mut dx10 = with(DEFAULTS);
        dx10.note_on(note(7, 60));
        play(&mut dx10, 4800);
        dx10.note_off(7);
        let tail = dx10.tail();
        let (left, _) = play(&mut dx10, tail + 512);
        assert!(dx10.voices.is_empty());
        assert!(left.iter().all(|v| v.is_finite()));
        assert!(left[..100].iter().any(|v| *v != 0.0));
    }

    #[test]
    fn every_voice_steps_the_one_lfo() {
        let mut one = with(DEFAULTS);
        one.note_on(note(1, 60));
        play(&mut one, 1000);
        let mut two = with(DEFAULTS);
        two.note_on(note(1, 60));
        two.note_on(note(2, 64));
        play(&mut two, 500);
        // Ten steps a voice in 1000 samples, or in 500 with two voices.
        assert_eq!(one.lfo, two.lfo);
    }

    #[test]
    fn a_note_past_the_limit_cuts_the_quietest_for_one_block() {
        let mut dx10 = with(DEFAULTS);
        for k in 0..VOICES as u32 {
            dx10.note_on(note(k, 40 + k as i16 % 40));
        }
        play(&mut dx10, 64);
        dx10.note_on(note(99, 60));
        assert_eq!(dx10.voices.iter().filter(|v| v.dying).count(), 1);
        assert_eq!(dx10.voices.len(), VOICES + 1);
        play(&mut dx10, 64);
        assert_eq!(dx10.voices.len(), VOICES);
    }

    #[test]
    fn every_state_fl_ships_is_read() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Fruity DX10 states skipped");
            return;
        };
        let mut saved = testing::projects(&root);
        saved.extend(testing::every_preset(&root, NAME));
        let ours: Vec<_> = saved
            .iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        // 11 in projects, the render test, 59 presets and three in the
        // plugin databases.
        assert!(ours.len() >= 74, "{}", ours.len());
        for saved in ours {
            let mut dx10 = Dx10::default();
            dx10.set_sample_rate(48_000.0);
            dx10.set_state(&saved.state)
                .unwrap_or_else(|e| panic!("{}: {e}", saved.file));
            assert_eq!(dx10.fidelity(), Fidelity::Exact, "{}", saved.file);
            dx10.note_on(note(1, 48));
            dx10.note_on(note(2, 72));
            let (left, _) = play(&mut dx10, 9600);
            let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak.is_finite() && peak < 4.0, "{}: {peak}", saved.file);
        }
    }
}
