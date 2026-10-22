//! Fruit Kick: a sine that starts high and falls to a floor while it dies
//! away, as `docs/plugins/fruit-kick.md` has it.
//!
//! The state is a version and little-endian i32 after it. Version 0 is 32
//! bytes: the six parameters and the floor the saving FL worked out, which
//! FL works out again and so is not read here. Version 1 is 400 bytes: the
//! six parameters, 4 bytes of nothing, the 364 bytes of the instrument tab
//! and 4 more of nothing.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Max freq, cents | -2400 to 3600 | 0 |
//! | 1 Min freq, cents | -1200 to 1200 | -600 |
//! | 2 Freq decay | 0 to 256 | 64 |
//! | 3 Amp decay | 0 to 256 | 128 |
//! | 4 Click | 0 to 64 | 32 |
//! | 5 Distortion | 0 to 128 | 0 |
//!
//! A note is a phase of 32 bits and what it moves by a sample, the
//! increment. 0 cents is 523.25 Hz, and the note starts at
//! `max + pitch / 2 + 1200` cents, kept between `min - 3600` and 4800,
//! where `pitch` is 100 cents a key from key 60. The doc has not read
//! which key FL gives a pitch of 0. FL's SDK has its middle C, key 60, at
//! the 523.25 Hz the DLL's constant is made of, so 60 it is.
//!
//! Every sample is `sine(phase) * amplitude`, with the sine FL keeps in a
//! table of 16384 points. Then the amplitude, which starts at 1.5, and the
//! increment are each multiplied by their factor of [`decays`], the
//! increment down to the floor at `min - 3600` cents. The click is where
//! the phase starts: 64 is a quarter of a turn in, at the sine's peak. The
//! note is over when its amplitude is under 0.0002, and a note off does
//! nothing to it. FL plays such a note on to the end of its block.
//!
//! What is not FL's:
//!
//! - Distortion runs the sound through a table of FL's host that is made
//!   when FL starts and is not read. `tanh` stands in for it here, and
//!   [`Generator::fidelity`] says so while distortion is on.
//! - The instrument tab is read and kept in [`FruitKick::articulation`],
//!   and none of it is played: its filter and its pan law are not read.
//!   Fidelity says so for a state whose tab would be heard. Its pitch
//!   envelope would not be: the kick reads its pitch once, when it starts.
//! - A note's velocity is its gain and its pan is [`dsp::pan_linear`],
//!   the pan the engine's curves have, until FL's own law is read.
//! - A note's mods, which move the tab's filter, are not read. A slide
//!   note sounds like any other.
//! - FL lets go of the oldest note at the channel's polyphony. Here a
//!   note past [`VOICES`] takes the place of the oldest.
//!
//! The doc rounds the increments. FL's own presets hold the floor cut off
//! to a whole number, from a base kept as an f32, and that is what
//! [`increment`] does.

use crate::{Fidelity, Generator, NoteOn, StateError, knob_value, state_version};
use std::f64::consts::TAU;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruit Kick";

/// The notes it sounds at once. One more takes the place of the oldest.
pub const VOICES: usize = 32;

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 6] = [
    (-2400, 3600),
    (-1200, 1200),
    (0, 256),
    (0, 256),
    (0, 64),
    (0, 128),
];
const DEFAULTS: [i32; 6] = [0, -600, 64, 128, 32, 0];

/// How long each version of the state is.
const LENGTHS: [usize; 2] = [32, 400];

/// `2^32 * 523.2511`: the increment at 0 cents, times the rate.
const BASE: f64 = 2_247_346_493_527.166;

/// The amplitude a note starts at, and the one it is over under.
const START: f32 = 1.5;
const END: f32 = 2e-4;

/// What the phase moves by a sample at this many cents and this rate.
pub fn increment(cents: f32, rate: f32) -> i32 {
    let base = (BASE / f64::from(rate)) as f32;
    (f64::from(base) * (f64::from(cents) / 1200.0).exp2()) as i32
}

/// What one sample makes of the amplitude and of the increment of the one
/// before, for the two decay parameters at this rate:
///
/// ```text
/// amplitude: (0.99998 - (1001^(1 - p3 / 256) - 1) * 4.98e-6) ^ (44100 / rate)
/// increment: (0.9997  - (1001^(1 - p2 / 256) - 1) * 9.7e-6)  ^ (44100 / rate)
/// ```
pub fn decays(freq_decay: i32, amp_decay: i32, rate: f32) -> (f32, f32) {
    let curve = |p: i32| {
        let left = f64::from(1.0 - p as f32 / 256.0);
        ((left * 1001f64.ln()).exp() - 1.0) as f32
    };
    let amplitude = (0.99998 - f64::from(curve(amp_decay)) * 4.98e-6) as f32;
    let increment = 0.9997 - curve(freq_decay) * 9.7e-6;
    let per = 44_100.0 / rate;
    (amplitude.powf(per), increment.powf(per))
}

/// The instrument tab a version 1 state holds, as it is saved. None of it
/// is played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Articulation {
    /// The envelope and LFO of the pan, the volume, mod X, mod Y and the
    /// pitch: 17 i32 each, laid out as section 1 of
    /// `docs/plugins/3x-osc.md` has them.
    pub blocks: [[i32; 17]; 5],
    /// 0 to 256, and 256 for a new plugin.
    pub cutoff: i32,
    /// 0 to 256, and 0 for a new plugin.
    pub resonance: i32,
    /// 0 to 7, and 0, Fast LP, for a new plugin.
    pub filter: i32,
}

impl Articulation {
    /// Where a block has whether its envelope is on, how far the envelope
    /// moves its target and how far its LFO does.
    const ENABLED: usize = 1;
    const AMOUNT: usize = 8;
    const LFO_AMOUNT: usize = 11;

    /// The 364 bytes a state has from byte 32 on. 3x Osc's state has the
    /// same from byte 96 on.
    pub(crate) fn read(bytes: &[u8; 364]) -> Self {
        let words = bytes.as_chunks::<4>().0;
        let word = |at: usize| i32::from_le_bytes(words[at]);
        Articulation {
            blocks: std::array::from_fn(|block| std::array::from_fn(|k| word(block * 17 + k))),
            cutoff: word(87),
            resonance: word(88),
            filter: word(90),
        }
    }

    /// Whether FL would play a kick differently for it: the volume
    /// envelope is on, an LFO or a mod envelope moves its target, or the
    /// filter is not the wide open one of a new plugin. The LFO of the
    /// pitch counts too, though the doc has a running kick not hear it.
    pub fn is_heard(&self) -> bool {
        let [_, volume, mod_x, mod_y, _] = &self.blocks;
        volume[Self::ENABLED] != 0
            || self.blocks.iter().any(|block| block[Self::LFO_AMOUNT] != 0)
            || [mod_x, mod_y].iter().any(|block| block[Self::AMOUNT] != 0)
            || (self.cutoff, self.resonance, self.filter) != (256, 0, 0)
    }
}

/// What the parameters come to at a rate.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Derived {
    /// The increment a note does not go under.
    floor: i32,
    amp_decay: f32,
    freq_decay: f32,
    /// How much of the sound stays as it is, and how much of what the
    /// distortion makes of it is added.
    dry: f32,
    wet: f32,
}

impl Derived {
    fn of(params: &[i32; 6], rate: f32) -> Self {
        let (amp_decay, freq_decay) = decays(params[2], params[3], rate);
        let distortion = params[5] as f32 / 128.0;
        Derived {
            floor: increment((params[1] - 3600) as f32, rate),
            amp_decay,
            freq_decay,
            dry: 1.0 - distortion,
            wet: (f64::from(distortion) * 3.3) as f32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    phase: u32,
    /// What the phase moves by a sample. FL keeps it as a float and adds
    /// the whole number in it.
    increment: f32,
    amplitude: f32,
    /// Left and right gain: the note's velocity and its pan.
    gains: (f32, f32),
}

/// Fruit Kick. A new one is at 44.1 kHz with FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct FruitKick {
    params: [i32; 6],
    articulation: Option<Articulation>,
    rate: f32,
    derived: Derived,
    /// In the order they started, and never more than [`VOICES`].
    voices: Vec<Voice>,
}

impl Default for FruitKick {
    fn default() -> Self {
        let rate = 44_100.0;
        FruitKick {
            params: DEFAULTS,
            articulation: None,
            rate,
            derived: Derived::of(&DEFAULTS, rate),
            voices: Vec::with_capacity(VOICES),
        }
    }
}

impl FruitKick {
    /// The instrument tab of the state, when the state had one.
    pub fn articulation(&self) -> Option<&Articulation> {
        self.articulation.as_ref()
    }

    /// The cents a note of this pitch starts at: half the pitch on top of
    /// the Max freq knob and an octave, no lower than the floor and no
    /// higher than 4800.
    fn start_cents(&self, pitch: f32) -> f32 {
        let cents = self.params[0] as f32 + pitch * 0.5 + 1200.0;
        let floor = (self.params[1] - 3600) as f32;
        // FL's order: under the floor is the floor, whatever the top is.
        if cents < floor {
            floor
        } else {
            cents.min(4800.0)
        }
    }
}

/// The sine of FL's host at a phase: a table of 16384 points, read with no
/// interpolation.
fn sine(phase: u32) -> f32 {
    (TAU * f64::from(phase >> 18) / 16384.0).sin() as f32
}

impl Generator for FruitKick {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LENGTHS[1])?;
        let Some(expected) = usize::try_from(version)
            .ok()
            .and_then(|v| LENGTHS.get(v).copied())
        else {
            return Err(StateError::Version { found: version });
        };
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let mut params = [0; 6];
        for (param, word) in bytes[4..28].as_chunks::<4>().0.iter().enumerate() {
            let value = i32::from_le_bytes(*word);
            let (min, max) = RANGES[param];
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        self.params = params;
        self.articulation = bytes[32..].first_chunk::<364>().map(Articulation::read);
        self.derived = Derived::of(&self.params, self.rate);
        Ok(())
    }

    /// The notes that are sounding keep the increment they have.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.derived = Derived::of(&self.params, hz);
    }

    fn note_on(&mut self, note: NoteOn) {
        if self.voices.len() == VOICES {
            self.voices.remove(0);
        }
        let pitch = (f32::from(note.key) - 60.0) * 100.0 + note.fine_pitch;
        let (left, right) = dsp::pan_linear(note.pan);
        self.voices.push(Voice {
            phase: (self.params[4] as u32) << 24,
            increment: increment(self.start_cents(pitch), self.rate) as f32,
            amplitude: START,
            gains: (note.velocity * left, note.velocity * right),
        });
    }

    /// A kick plays itself out.
    fn note_off(&mut self, _voice: u32) {}

    fn all_notes_off(&mut self) {}

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let derived = self.derived;
        let floor = derived.floor as f32;
        let distorted = self.params[5] > 0;
        for voice in &mut self.voices {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let mut value = sine(voice.phase) * voice.amplitude;
                voice.phase = voice.phase.wrapping_add(voice.increment as i32 as u32);
                voice.amplitude *= derived.amp_decay;
                voice.increment = if floor < voice.increment {
                    voice.increment * derived.freq_decay
                } else {
                    floor
                };
                if distorted {
                    value = derived.dry * value + derived.wet * value.tanh();
                }
                *l += value * voice.gains.0;
                *r += value * voice.gains.1;
                if voice.amplitude < END {
                    break;
                }
            }
        }
        self.voices.retain(|voice| voice.amplitude >= END);
    }

    /// The six of the plugin's own. FL numbers the 93 of the instrument
    /// tab after them.
    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.derived = Derived::of(&self.params, self.rate);
        }
    }

    fn reset(&mut self) {
        self.voices.clear();
    }

    /// As long as a whole note: the samples that take the amplitude from
    /// 1.5 to under 0.0002.
    fn tail(&self) -> usize {
        let samples = f64::from(END / START).ln() / f64::from(self.derived.amp_decay).ln();
        samples.ceil() as usize + 1
    }

    fn fidelity(&self) -> Fidelity {
        if self.params[5] > 0 {
            Fidelity::Approximate("waveshaper table 10")
        } else if self.articulation.is_some_and(|tab| tab.is_heard()) {
            Fidelity::Approximate("Fruit Kick articulation")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, state};

    const RATE: f64 = 44_100.0;

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

    /// A version 0 state of these parameters.
    fn v0(params: [i32; 6]) -> Vec<u8> {
        state(&[&[0][..], &params, &[0]].concat())
    }

    fn with(params: [i32; 6]) -> FruitKick {
        let mut kick = FruitKick::default();
        kick.set_state(&v0(params)).unwrap();
        kick
    }

    fn play(kick: &mut FruitKick, len: usize) -> Vec<f32> {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        kick.process(&mut left, &mut right);
        assert_eq!(left, right);
        left
    }

    /// The doc's maths for a note of these parameters at 44.1 kHz: the
    /// phase in turns and the amplitude of every sample.
    fn model(params: [i32; 6], pitch: f64, len: usize) -> Vec<(f64, f64)> {
        let [max, min, freq, amp, click, _] = params.map(f64::from);
        let factor = |p: f64, top: f64, by: f64| top - (1001f64.powf(1.0 - p / 256.0) - 1.0) * by;
        let (amp_decay, freq_decay) = (factor(amp, 0.99998, 4.98e-6), factor(freq, 0.9997, 9.7e-6));
        let turns = |cents: f64| 523.251_130_6 * 2f64.powf(cents / 1200.0) / RATE;
        let floor = turns(min - 3600.0);
        let start = (max + pitch * 0.5 + 1200.0).clamp(min - 3600.0, 4800.0);
        let (mut phase, mut step, mut level) = (click / 256.0, turns(start), 1.5);
        let mut samples = Vec::with_capacity(len);
        for _ in 0..len {
            samples.push((phase, level));
            phase += step;
            level *= amp_decay;
            step = if step > floor {
                step * freq_decay
            } else {
                floor
            };
        }
        samples
    }

    /// Where the sound goes through 0, in samples, by a straight line
    /// between the two samples either side.
    fn crossings(sound: &[f32]) -> Vec<f64> {
        let mut found = Vec::new();
        for (n, pair) in sound.windows(2).enumerate() {
            let (a, b) = (f64::from(pair[0]), f64::from(pair[1]));
            if a != 0.0 && a.signum() != b.signum() {
                found.push(n as f64 + a / (a - b));
            }
        }
        found
    }

    /// The model's phase at a time between two samples.
    fn phase_at(model: &[(f64, f64)], time: f64) -> f64 {
        let n = time.floor() as usize;
        let (a, b) = (model[n].0, model[n + 1].0);
        a + (b - a) * (time - n as f64)
    }

    #[test]
    fn a_note_starts_at_the_start_pitch_and_falls_to_the_floor_at_the_formula_s_rate() {
        // No click, so that the first sample is the sine's start.
        let params = [0, -600, 64, 128, 0, 0];
        let mut kick = with(params);
        kick.note_on(note(1, 60));
        let sound = play(&mut kick, 40_000);
        let model = model(params, 0.0, 40_001);

        // 0 + 0 / 2 + 1200 cents is 1046.5 Hz, and the second sample is
        // one step of it up the sine.
        let step = f64::from(sound[1]) / model[1].1;
        let start = step.asin() / TAU * RATE;
        assert!((start - 1046.502).abs() < 1046.5 * 0.005, "{start} Hz");
        // 0.9997 - (1001^0.75 - 1) * 9.7e-6 a sample.
        let rate = (model[2].0 - model[1].0) / model[1].0;
        assert!((rate - 0.997_983_4).abs() < 1e-6, "{rate}");

        // Every turn between two crossings two apart: the frequency over
        // it against the formula's mean over the same samples.
        let found = crossings(&sound);
        let sweep: Vec<f64> = found.iter().copied().filter(|t| *t < 3000.0).collect();
        assert!(sweep.len() > 20, "{} crossings", sweep.len());
        let mut fell = 0;
        for pair in sweep.windows(3) {
            let (from, to) = (pair[0], pair[2]);
            let measured = 1.0 / (to - from);
            let formula = (phase_at(&model, to) - phase_at(&model, from)) / (to - from);
            assert!(
                (measured / formula - 1.0).abs() < 0.005,
                "{measured} for {formula} at {from}"
            );
            fell += usize::from(measured < 1046.5 / RATE);
        }
        assert_eq!(fell, sweep.len() - 2);
        // The first turn is under 1046.5 Hz already and the turns get
        // longer: 11 of them in the first 1600 samples, where 1046.5 Hz
        // would have 38.
        let turns = phase_at(&model, 1600.0);
        assert!((11.0..11.6).contains(&turns), "{turns}");

        // The floor is -600 - 3600 cents: 46.25 Hz, and it stays there.
        let late: Vec<f64> = found.iter().copied().filter(|t| *t > 6000.0).collect();
        let hz = (late.len() - 1) as f64 / 2.0 / (late[late.len() - 1] - late[0]) * RATE;
        let floor = 523.251_130_6 * 2f64.powf(-3.5);
        assert!((floor - 46.249).abs() < 1e-3);
        assert!((hz / floor - 1.0).abs() < 0.001, "{hz} Hz");
    }

    #[test]
    fn the_amplitude_falls_by_its_factor_every_sample() {
        for params in [
            DEFAULTS,
            [0, -600, 64, 200, 32, 0],
            [512, -1200, 43, 91, 64, 0],
        ] {
            let mut kick = with(params);
            kick.note_on(note(1, 60));
            let len = kick.tail().min(30_000) - 2;
            let sound = play(&mut kick, len);
            let model = model(params, 0.0, len);
            let mut checked = 0;
            for (n, (value, (phase, level))) in sound.iter().zip(&model).enumerate() {
                let sine = (TAU * phase).sin();
                // Where the sine is small, the 16384 points of the table
                // count for more than the amplitude.
                if sine.abs() < 0.5 {
                    continue;
                }
                let amplitude = f64::from(*value) / sine;
                assert!(
                    (amplitude / level - 1.0).abs() < 0.01,
                    "{amplitude} for {level} at {n} of {params:?}"
                );
                checked += 1;
            }
            assert!(checked > len / 2);
        }
        // The factor itself: 0.99998 - (1001^0.5 - 1) * 4.98e-6 at the
        // default, and the doc's ends.
        let (amplitude, increment) = decays(64, 128, 44_100.0);
        assert!((f64::from(amplitude) - 0.999_827_4).abs() < 1e-6);
        assert!((f64::from(increment) - 0.997_983_4).abs() < 1e-6);
        let (fast, slow) = (decays(0, 0, 44_100.0), decays(256, 256, 44_100.0));
        assert!((fast.0 - 0.995).abs() < 1e-6 && (fast.1 - 0.99).abs() < 1e-6);
        assert!((slow.0 - 0.99998).abs() < 1e-7 && (slow.1 - 0.9997).abs() < 1e-7);
        // At twice the rate a sample is half the way.
        let half = decays(64, 128, 88_200.0);
        assert!((half.0 * half.0 - amplitude).abs() < 1e-6);
        assert!((half.1 * half.1 - increment).abs() < 1e-6);
    }

    #[test]
    fn the_click_is_where_the_sine_starts() {
        let first = |click: i32| {
            let mut kick = with([0, -600, 64, 128, click, 0]);
            kick.note_on(note(1, 60));
            play(&mut kick, 1)[0]
        };
        assert_eq!(first(0), 0.0);
        // A quarter of a turn in, at the peak.
        assert_eq!(first(64), 1.5);
        assert!((first(32) - 1.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((f64::from(first(16)) - 1.5 * (TAU / 16.0).sin()).abs() < 1e-6);
    }

    #[test]
    fn the_key_moves_the_start_at_half_rate_and_the_floor_not_at_all() {
        let start = |params: [i32; 6], key: i16, fine: f32| {
            let mut kick = with(params);
            kick.note_on(NoteOn {
                fine_pitch: fine,
                ..note(1, key)
            });
            (kick.voices[0].increment, kick.derived.floor)
        };
        let (middle, floor) = start(DEFAULTS, 60, 0.0);
        assert_eq!(middle, increment(1200.0, 44_100.0) as f32);
        // An octave up starts 600 cents up and lands on the same note.
        assert_eq!(
            start(DEFAULTS, 72, 0.0),
            (increment(1800.0, 44_100.0) as f32, floor)
        );
        assert_eq!(
            start(DEFAULTS, 60, 50.0).0,
            increment(1225.0, 44_100.0) as f32
        );
        // No higher than 4800 cents and no lower than the floor.
        let top = increment(4800.0, 44_100.0) as f32;
        assert_eq!(start([3600, -1200, 158, 256, 0, 0], 60, 0.0).0, top);
        assert_eq!(start(DEFAULTS, 127, 600.0).0, top);
        let low = start([-2400, 1200, 64, 128, 0, 0], 0, 0.0);
        assert_eq!(low.0, low.1 as f32);
        assert_eq!(low.1, increment(-2400.0, 44_100.0));
        // 0 cents is 523.25 Hz of a 32-bit phase.
        let turn = f64::from(increment(0.0, 44_100.0)) / 2f64.powi(32);
        assert!((turn * RATE - 523.251).abs() < 1e-3);
    }

    /// The floor a version 0 state says its FL worked out.
    fn saved_floor(state: &[u8]) -> (i32, i32) {
        let word = |at: usize| i32::from_le_bytes(state[at..at + 4].try_into().unwrap());
        (word(8), word(28))
    }

    #[test]
    fn the_floor_default_fst_holds_is_the_formula_s() {
        // The doc's check: round(2247346493527.166 / 44100 * 2^-3.5).
        let floor = 2_247_346_493_527.166_f64 / 44_100.0 * 2f64.powf(-3.5);
        assert_eq!(floor.round() as i32, 0x44bae3);
        assert_eq!(increment(-4200.0, 44_100.0), 0x44bae3);
        assert_eq!(FruitKick::default().derived.floor, 4_504_291);

        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Fruit Kick's presets skipped");
            return;
        };
        let presets = testing::every_preset(&root, NAME);
        let default = presets.iter().find(|saved| saved.file == "Default");
        assert_eq!(saved_floor(&default.unwrap().state), (-600, 0x44bae3));
        // Every preset holds the floor of its Min freq, at the 44.1 kHz or
        // the 48 kHz its FL ran at. Five of them hold what cutting off
        // gives and rounding does not.
        let (mut at_48, mut cut) = (0, 0);
        for saved in &presets {
            let (min, floor) = saved_floor(&saved.state);
            let cents = (min - 3600) as f32;
            let ours = [44_100.0, 48_000.0].map(|rate| increment(cents, rate));
            assert!(ours.contains(&floor), "{}: {floor}", saved.file);
            at_48 += usize::from(floor == ours[1]);
            let exact = BASE / 44_100.0 * 2f64.powf(f64::from(cents) / 1200.0);
            cut += usize::from(floor == ours[0] && exact.round() as i32 != floor);
        }
        assert_eq!((presets.len(), at_48, cut), (11, 2, 5));
    }

    #[test]
    fn every_preset_and_every_state_in_fl_s_projects_loads() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Fruit Kick's states skipped");
            return;
        };
        let mut lengths = std::collections::BTreeMap::new();
        let mut saved = testing::every_preset(&root, NAME);
        saved.extend(testing::projects(&root));
        for saved in saved {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            let mut kick = crate::generator_by_name(&saved.plugin).unwrap();
            if let Err(error) = kick.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            // The state's parameters are the plugin's.
            for index in 0..6 {
                let at = 4 + 4 * index;
                let word = i32::from_le_bytes(saved.state[at..at + 4].try_into().unwrap());
                assert_eq!(kick.param(index), word as f32, "{}", saved.file);
            }
            kick.note_on(note(1, 60));
            let (mut left, mut right) = ([0.0; 512], [0.0; 512]);
            kick.process(&mut left, &mut right);
            assert!(left.iter().all(|v| v.is_finite() && v.abs() < 8.0));
            *lengths.entry(saved.state.len()).or_insert(0) += 1;
        }
        // 11 presets and 2 projects of version 0, and 1 project of FL
        // 2026's version 1.
        assert_eq!(
            lengths.into_iter().collect::<Vec<_>>(),
            [(32, 13), (400, 1)]
        );
    }

    /// A version 1 state with the instrument tab of a new plugin.
    fn v1(params: [i32; 6]) -> Vec<i32> {
        let block = [
            0, 0, 100, 20_000, 20_000, 30_000, 50, 20_000, 0, 100, 20_000, 0, 32_950, 0, 0, 0, 0,
        ];
        let mut words = [&[1][..], &params, &[0]].concat();
        for _ in 0..5 {
            words.extend(block);
        }
        words.extend([0, 0, 256, 0, 0, 0, 0]);
        assert_eq!(words.len(), 100);
        words
    }

    #[test]
    fn a_version_1_state_keeps_its_instrument_tab() {
        let params = [-2400, -840, 13, 89, 0, 0];
        let mut kick = FruitKick::default();
        assert!(kick.articulation().is_none());
        kick.set_state(&state(&v1(params))).unwrap();
        assert_eq!(kick.params, params);
        let tab = *kick.articulation().unwrap();
        assert_eq!(tab.blocks[1][3], 20_000);
        assert_eq!((tab.cutoff, tab.resonance, tab.filter), (256, 0, 0));
        assert!(!tab.is_heard());
        assert_eq!(kick.fidelity(), Fidelity::Exact);

        // What would be heard: the volume envelope, as the one project
        // with a version 1 state has it, an LFO, a mod envelope and the
        // filter. An envelope on the pitch is not.
        let heard = |word: usize, value: i32| {
            let mut words = v1(params);
            words[word] = value;
            let mut kick = FruitKick::default();
            kick.set_state(&state(&words)).unwrap();
            kick.fidelity() == Fidelity::Approximate("Fruit Kick articulation")
        };
        let block = |block: usize, field: usize| 8 + 17 * block + field;
        assert!(heard(block(1, 1), -1));
        assert!(heard(block(0, 11), 64));
        assert!(heard(block(2, 8), -128));
        assert!(heard(block(3, 8), 1));
        assert!(heard(95, 255) && heard(96, 1) && heard(98, 2));
        assert!(!heard(block(4, 1), -1) && !heard(block(4, 8), 100));
        assert!(!heard(block(0, 1), -1) && !heard(99, 7) && !heard(7, 7));

        // A version 0 state after it has no tab.
        kick.set_state(&v0(DEFAULTS)).unwrap();
        assert!(kick.articulation().is_none());
    }

    #[test]
    fn a_state_fl_does_not_write_is_turned_down() {
        let mut kick = with([100, -700, 3, 4, 5, 6]);
        let before = kick.clone();
        let length = |expected, found| Err(StateError::Length { expected, found });
        assert_eq!(kick.set_state(&[0; 3]), length(400, 3));
        assert_eq!(kick.set_state(&[0; 28]), length(32, 28));
        assert_eq!(kick.set_state(&[0; 400]), length(32, 400));
        assert_eq!(kick.set_state(&state(&v1(DEFAULTS))[..32]), length(400, 32));
        for version in [2, -1, i32::MAX] {
            let mut words = v1(DEFAULTS);
            words[0] = version;
            assert_eq!(
                kick.set_state(&state(&words)),
                Err(StateError::Version { found: version })
            );
        }
        for (param, value) in [(0, 3601), (1, -1201), (2, 257), (3, -1), (4, 65), (5, 129)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                kick.set_state(&v0(params)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(kick, before);
        // The floor a version 0 state holds is not read.
        let mut words = [&[0][..], &DEFAULTS, &[123]].concat();
        kick.set_state(&state(&words)).unwrap();
        assert_eq!(kick.derived.floor, 0x44bae3);
        words[2] = 1200;
        kick.set_state(&state(&words)).unwrap();
        assert_eq!(kick.derived.floor, increment(-2400.0, 44_100.0));
    }

    #[test]
    fn distortion_mixes_the_shaped_sound_in_and_is_not_fl_s() {
        let clean = {
            let mut kick = with(DEFAULTS);
            assert_eq!(kick.fidelity(), Fidelity::Exact);
            kick.note_on(note(1, 60));
            play(&mut kick, 2000)
        };
        for distortion in [20, 128] {
            let mut kick = with([0, -600, 64, 128, 32, distortion]);
            assert_eq!(
                kick.fidelity(),
                Fidelity::Approximate("waveshaper table 10")
            );
            kick.note_on(note(1, 60));
            let d = distortion as f32 / 128.0;
            for (n, (value, clean)) in play(&mut kick, 2000).iter().zip(&clean).enumerate() {
                // (1 - d) of the sound and 3.3 d of the shaped one.
                let want = (1.0 - d) * clean + 3.3 * d * clean.tanh();
                assert!((value - want).abs() < 1e-6, "sample {n}");
            }
        }
        // The knob is a parameter like the others.
        let mut kick = FruitKick::default();
        kick.set_param(5, 1.0);
        assert_ne!(kick.fidelity(), Fidelity::Exact);
        kick.set_param(5, 0.0);
        assert_eq!(kick.fidelity(), Fidelity::Exact);
    }

    #[test]
    fn a_note_is_over_under_0_0002_and_a_note_off_does_not_end_it() {
        // Pure click: 0.995 a sample.
        let mut kick = with([-1216, -1200, 0, 0, 64, 0]);
        let tail = kick.tail();
        // 1.5 * 0.995^n is under 0.0002 from n = 1781 on.
        assert_eq!(tail, 1782);
        kick.note_on(note(1, 60));
        kick.note_off(1);
        kick.all_notes_off();
        let sound = play(&mut kick, 2000);
        let last = sound.iter().rposition(|value| *value != 0.0).unwrap();
        assert!((1770..tail).contains(&last), "{last}");
        assert!(kick.voices.is_empty());
        assert!(sound[1000].abs() > 0.001);

        // The default lasts 1.17 seconds, and the slowest 10.
        assert_eq!(FruitKick::default().tail(), 51_706);
        let slow = with([0, -600, 64, 256, 32, 0]);
        assert!((440_000..450_000).contains(&slow.tail()));
        // A reset cuts what sounds.
        let mut kick = FruitKick::default();
        kick.note_on(note(1, 60));
        kick.reset();
        assert_eq!(play(&mut kick, 4), [0.0; 4]);
    }

    #[test]
    fn velocity_and_pan_set_the_two_gains_and_notes_add_up() {
        let one = {
            let mut kick = FruitKick::default();
            kick.note_on(note(1, 60));
            play(&mut kick, 300)
        };
        let mut kick = FruitKick::default();
        kick.note_on(NoteOn {
            velocity: 0.5,
            pan: -1.0,
            ..note(1, 60)
        });
        let (mut left, mut right) = (vec![1.0; 100], vec![1.0; 100]);
        kick.process(&mut left, &mut right);
        for n in 0..100 {
            assert_eq!(left[n], 1.0 + one[n] * 0.5);
            assert_eq!(right[n], 1.0);
        }
        // A second note 100 samples behind the first.
        kick.note_on(note(2, 60));
        let (mut left, mut right) = (vec![0.0; 100], vec![0.0; 100]);
        kick.process(&mut left, &mut right);
        for n in 0..100 {
            assert_eq!(left[n], one[100 + n] * 0.5 + one[n]);
            assert_eq!(right[n], one[n]);
        }
    }

    #[test]
    fn the_blocks_can_fall_anywhere_and_the_rate_moves_the_pitch_with_it() {
        let whole = {
            let mut kick = with([0, -600, 64, 60, 32, 40]);
            kick.note_on(note(1, 64));
            play(&mut kick, 6000)
        };
        for block in [1, 64, 1000] {
            let mut kick = with([0, -600, 64, 60, 32, 40]);
            kick.note_on(note(1, 64));
            let mut sound = Vec::new();
            while sound.len() < 6000 {
                sound.extend(play(&mut kick, block.min(6000 - sound.len())));
            }
            assert_eq!(testing::bits(&sound), testing::bits(&whole), "{block}");
        }

        // At 88.2 kHz every second sample is a sample of 44.1 kHz, as near
        // as the whole numbers of the two increments allow.
        let mut kick = FruitKick::default();
        kick.set_sample_rate(88_200.0);
        assert_eq!(kick.derived.floor, 0x44bae3 / 2);
        kick.note_on(note(1, 60));
        let fast = play(&mut kick, 4000);
        let mut kick = FruitKick::default();
        kick.note_on(note(1, 60));
        for (n, value) in play(&mut kick, 2000).iter().enumerate() {
            assert!((value - fast[2 * n]).abs() < 0.1, "sample {n}");
        }
    }

    #[test]
    fn the_parameters_are_the_six_knobs_inside_their_ranges() {
        let mut kick = FruitKick::default();
        assert_eq!(kick.param_count(), 6);
        let params: Vec<f32> = (0..7).map(|index| kick.param(index)).collect();
        assert_eq!(params, [0.0, -600.0, 64.0, 128.0, 32.0, 0.0, 0.0]);
        kick.set_param(0, 1e9);
        kick.set_param(1, -1e9);
        kick.set_param(3, 90.6);
        kick.set_param(6, 5.0);
        assert_eq!(kick.params, [3600, -1200, 64, 91, 32, 0]);
        assert_eq!(kick.derived, Derived::of(&kick.params, 44_100.0));
        assert_eq!(kick.derived.floor, increment(-4800.0, 44_100.0));
    }

    #[test]
    fn a_33rd_note_takes_the_place_of_the_first() {
        let mut kick = FruitKick::default();
        let capacity = kick.voices.capacity();
        for voice in 0..=VOICES as u32 {
            kick.note_on(NoteOn {
                velocity: voice as f32,
                ..note(voice, 60)
            });
        }
        assert_eq!(kick.voices.len(), VOICES);
        assert_eq!(kick.voices[0].gains.0, 1.0);
        assert_eq!(kick.voices.capacity(), capacity);
    }
}
