//! Drumpad: a struck membrane, as `docs/plugins/drumpad.md` reads it out
//! of the `engine_x64.dll` beside the plugin's `Drumpad_x64.dll`. The DSP is
//! in [`crate::drumpad_voice`]; this is the plugin around it: the state,
//! the parameters and the voices, as `DrumpadSynth` and the wrapper FL
//! talks to have them.
//!
//! # The state
//!
//! `SSLF`, the length of what follows as a u32, then the engine's chunk:
//! the magic `DMP3` (`3PMD` on disk), 128 bytes of preset name when the
//! chunk is longer than 132, and the parameters as f32 in their order, 38
//! of them. Every shipped state is `DMP3`: the projects' 284-byte chunks
//! hold the 38, the presets' 288-byte ones a 39th that FL does not read,
//! and four bytes past the chunk. Before it reads the parameters the
//! loader sets Membrane Shape to 1 and SOF Level to 0; a chunk that ends
//! early leaves the rest as they were.
//!
//! | parameter | default | what a note makes of `v` |
//! |---|---|---|
//! | 0 Noise repeat | 0 | not 0: every note starts the noise at its seed |
//! | 1 Retrigger | 0 | `int(3 v)` more starts of the MID envelope |
//! | 2 Mallet Amplitude | 0.5 | `10^(-4 (1 - v))` |
//! | 3 Mallet Decay | 0.5 | `(500 (1 - v)^4 + 1) 1e-4` a sample |
//! | 4 Mallet Noise | 0 | noise against tone |
//! | 5 Mallet Noise RP | 0 | the seed, `v (2^32 - 1)` |
//! | 6 Membrane Decay | 0.9 | feedback `-v^0.3` |
//! | 7 Membrane Cutoff | 0.35 | lowpass `v^2` |
//! | 8 Membrane Tension | 0.3 | gain phase step `v^4 / 2` |
//! | 9 Membrane Phase | 0.5 | gain phase start |
//! | 10 Membrane Material | 0.5 | gain power `20 m^4 + 1`, `m = 2 v - 1` |
//! | 11 Membrane Size | 1 | first lane `512 (1 - v)` |
//! | 12 Membrane Shape | 1 | lanes `128 v` |
//! | 13, 14 LOW Freq, Res | 0, 0 | highpass `2^(11 v + 2)` Hz, `q = 8 v + 1`; off at 0 |
//! | 15, 16, 17 SOF Freq, Decay, Level | 0.5, 0.5, 0 | a self-oscillating ladder; off at 0 |
//! | 18, 19, 20 MID Freq, Level, Res | 0.5, 0.5, 0 | peak `2^(9 v + 5)` Hz, `32 (v - 0.5)` dB |
//! | 21, 22, 23 MID Freq Mod, Level Mod, Attack | 0.5, 0.5, 0.5 | the MID envelope |
//! | 24, 25, 26 HIGH Freq, Level, Res | 0.5, 0.5, 0 | shelf `2^(4.1 v + 8)` Hz, `36 (v - 0.5)` dB |
//! | 27 Lo-Fi | 0 | `int(65536 (1 - v)^5 + 4)` steps, from 0.01 |
//! | 28 Hold | 1 | `3 v^3 + 0.001` seconds |
//! | 29 Pitch | 0.5 | `2^(2 v - 1)` |
//! | 30 to 33 velocity slots | 0 | each picks parameter `n + 1`, 0 none |
//! | 34 to 37 their targets | 0 | where velocity moves it |
//!
//! FL's automation reaches a parameter as `int / 256`, 0 to 1, and a
//! slot through it only as 0 or 1.
//!
//! # A note
//!
//! The key starts a voice and nothing else: Drumpad has no pitch of its
//! own key. A note off does nothing; a hit plays until it has been quiet
//! for 25 ms. The velocity is the MIDI number FL makes of it,
//! `int(127 log21(10 velocity + 1))`, and a note of 0 sounds none. Its
//! square root, of the number over 127, moves every parameter a velocity
//! slot picks towards the slot's target. The voice reads the parameters
//! once, as it starts.
//!
//! The voices take their gains and their pitch not from their own note.
//! FL keeps, for each key with a voice sounding or a note held, the pan
//! law of the last note's pan and velocity and the note's fine pitch; it
//! fills the keys between those from the one below, the ones under the
//! lowest from the lowest, and voice `i` of the pool reads key `i`. With
//! the keys a song plays over the voices' numbers, every voice follows the
//! last note on the lowest key that sounds. The pitch is
//! `2^(fine / 1200)` of that, at most an octave either way.
//!
//! What is not FL's:
//!
//! - FL has 99 voices. Here there are [`VOICES`], and a note past them
//!   takes the oldest, whose lines it starts from silence where FL runs
//!   on with what they hold. [`Generator::fidelity`] says so once it
//!   happens.
//! - The lines a note runs are laid out for it in room asked for by
//!   [`Generator::set_state`]. An automated Size or Shape that wants more
//!   gets fewer lines, and fidelity says so.
//! - FL's noise generators run on from one plugin to the next in the
//!   order FL makes their voices. Each Drumpad here starts them as the
//!   first one FL loads does: only its noise without Noise repeat is FL's.
//! - A `DMP1` or `DMP2` state, which FL maps onto the 38, is turned down.

use crate::bassdrum::velocity_level;
use crate::drumpad_voice::{MAX_LINES, Patch, RATE, Voice, line_floats};
use crate::sampler::circular;
use crate::{Fidelity, Generator, NoteOn, StateError};

pub use crate::drumpad_voice::PARAMS;

/// The plugin's event 201 name.
pub const NAME: &str = "Drumpad";

/// The voices it sounds at once. One more takes the oldest.
pub const VOICES: usize = 32;

/// The keys FL keeps the gains of.
const KEYS: usize = 128;

/// What a new plugin is set to (`FUN_180023290`).
pub(crate) const DEFAULTS: [f32; PARAMS] = [
    0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.9, 0.35, 0.3, 0.5, 0.5, 1.0, 1.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.5,
    0.5, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
];

/// The chunk's magic for the layout read here, as FL saves it and as its
/// demo saves it.
const MAGIC: [u32; 2] = [0x444d_5033, 0x4450_4433];

/// How long the preset name is.
const NAME_LEN: usize = 128;

/// What a key holds for the voices: the gains and the pitch of the last
/// note on it, and the host's number for that note.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    note: u32,
    left: f32,
    right: f32,
    semitones: f32,
}

/// The noise seed FL's voice number `n` starts with: a generator of its
/// own run on 1000 steps a voice from 0 (`FUN_18001f1a0`).
fn seed(voice: usize) -> u32 {
    let mut state = 0u32;
    for _ in 0..1000 * (voice + 1) {
        state = state.wrapping_mul(0x1c3c_718d).wrapping_sub(0x0af2_c25b);
    }
    state
}

/// Drumpad. A new one is at 44.1 kHz with FL's defaults.
#[derive(Debug, Clone)]
pub struct Drumpad {
    params: [f32; PARAMS],
    preset: String,
    rate: f32,
    voices: Vec<Voice>,
    /// The key each voice sounds, and when it started.
    keys: [Option<u8>; VOICES],
    started: [u32; VOICES],
    count: u32,
    /// For each key: what its last note left, whether a note is held on
    /// it and how many voices sound it.
    notes: [Option<Key>; KEYS],
    held: [bool; KEYS],
    sounding: [u32; KEYS],
    stole: bool,
    cut: bool,
}

impl Default for Drumpad {
    fn default() -> Self {
        let rate = 44_100.0;
        let floats = room(&DEFAULTS);
        Drumpad {
            params: DEFAULTS,
            preset: String::new(),
            rate,
            voices: (0..VOICES)
                .map(|v| Voice::new(floats, rate, seed(v)))
                .collect(),
            keys: [None; VOICES],
            started: [0; VOICES],
            count: 0,
            notes: [None; KEYS],
            held: [false; KEYS],
            sounding: [0; KEYS],
            stole: false,
            cut: false,
        }
    }
}

/// The floats a voice's lines need for every velocity: the lines from the
/// first a note can start at to the last one can end at.
fn room(params: &[f32; PARAMS]) -> usize {
    let spans = [0.0, 1.0].map(|x| Patch::of(params, x).lines());
    let start = spans.iter().map(|(start, _)| *start).min().unwrap_or(0);
    let end = spans.iter().map(|(s, n)| s + n).max().unwrap_or(0);
    line_floats(start, (end - start).min(MAX_LINES))
}

impl Drumpad {
    /// The preset name the state holds.
    pub fn preset(&self) -> &str {
        &self.preset
    }

    /// The parameters, as the state holds them: 0 to 1, and the slots'
    /// numbers.
    pub fn params(&self) -> &[f32; PARAMS] {
        &self.params
    }

    /// The voice a note takes: the first free one, or else the one that
    /// started first.
    fn pick(&mut self) -> usize {
        if let Some(free) = self.keys.iter().position(Option::is_none) {
            return free;
        }
        self.stole = true;
        (0..VOICES).min_by_key(|v| self.started[*v]).unwrap_or(0)
    }

    /// Cut every voice, as FL does when the rate changes (`FUN_1800201c0`
    /// for each).
    fn silence(&mut self) {
        for voice in &mut self.voices {
            voice.clear();
        }
        self.keys = [None; VOICES];
        self.sounding = [0; KEYS];
    }
}

impl Generator for Drumpad {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let Some(head) = bytes.first_chunk::<8>() else {
            return Err(StateError::Length {
                expected: 8,
                found: bytes.len(),
            });
        };
        let word = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
        if &head[..4] != b"SSLF" {
            return Err(StateError::Version {
                found: word(0) as i32,
            });
        }
        let len = word(4) as usize;
        let Some(chunk) = bytes.get(8..8 + len).filter(|chunk| chunk.len() >= 4) else {
            return Err(StateError::Length {
                expected: 8 + len.max(4),
                found: bytes.len(),
            });
        };
        let magic = u32::from_le_bytes(chunk[..4].try_into().unwrap());
        if !MAGIC.contains(&magic) {
            return Err(StateError::Version {
                found: magic as i32,
            });
        }
        let mut params = self.params;
        params[17] = 0.0;
        let mut rest = &chunk[4..];
        if rest.len() > NAME_LEN {
            let name = &rest[..NAME_LEN];
            let end = name.iter().position(|b| *b == 0).unwrap_or(NAME_LEN);
            self.preset = name[..end].iter().map(|b| char::from(*b)).collect();
            rest = &rest[NAME_LEN..];
        }
        params[12] = 1.0;
        params[17] = 0.0;
        for (param, word) in params.iter_mut().zip(rest.as_chunks::<4>().0) {
            *param = f32::from_le_bytes(*word);
        }
        self.params = params;
        let floats = room(&self.params);
        for voice in &mut self.voices {
            voice.reserve(floats);
        }
        Ok(())
    }

    /// The voices are cut, as FL cuts them.
    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        for voice in &mut self.voices {
            voice.set_rate(hz);
        }
        self.silence();
    }

    fn note_on(&mut self, note: NoteOn) {
        let key = note.key.clamp(0, KEYS as i16 - 1) as usize;
        // A key that has a note already lets go of it.
        let (left, right) = circular(note.pan, note.velocity);
        self.notes[key] = Some(Key {
            note: note.voice,
            left,
            right,
            semitones: (f64::from(note.fine_pitch) * 0.01) as f32,
        });
        let level = velocity_level(note.velocity);
        self.held[key] = level != 0.0;
        if level == 0.0 {
            return;
        }
        let patch = Patch::of(&self.params, level.powf(0.5));
        let voice = self.pick();
        if let Some(old) = self.keys[voice] {
            self.sounding[usize::from(old)] -= 1;
            self.voices[voice].clear();
        }
        self.keys[voice] = Some(key as u8);
        self.sounding[key] += 1;
        self.started[voice] = self.count;
        self.count = self.count.wrapping_add(1);
        self.cut |= self.voices[voice].trigger(patch);
    }

    /// A hit plays itself out. The key's gains stay until it is quiet.
    fn note_off(&mut self, voice: u32) {
        for (key, held) in self.notes.iter().zip(&mut self.held) {
            if key.is_some_and(|key| key.note == voice) {
                *held = false;
            }
        }
    }

    fn all_notes_off(&mut self) {
        self.held = [false; KEYS];
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // FUN_180003dd0: every key gets the gains and pitch of the key
        // with a note at or below it, or of the lowest such key.
        let mut keys = [(0.0f32, 0.0f32, 0.0f32); KEYS];
        let mut last = None;
        for key in 0..KEYS {
            if let Some(note) = self.notes[key] {
                let now = (note.left, note.right, note.semitones);
                if last.is_none() {
                    keys[..key].fill(now);
                }
                last = Some(now);
            }
            keys[key] = last.unwrap_or_default();
        }
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if !voice.active {
                continue;
            }
            let (l, r, semitones) = keys[index];
            voice.bend = 2f32.powf(semitones.max(-12.0).min(12.0) * (1.0 / 12.0));
            voice.target = (l, r);
            if voice.play(left, right, self.rate)
                && let Some(key) = self.keys[index].take()
            {
                self.sounding[usize::from(key)] -= 1;
            }
        }
        // FUN_180004840: a key lets go once nothing sounds or holds it.
        for key in 0..KEYS {
            if self.sounding[key] == 0 && !self.held[key] {
                self.notes[key] = None;
            }
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    /// The parameter as FL's automation has it: `int(256 v)`.
    fn param(&self, index: usize) -> f32 {
        self.params
            .get(index)
            .map_or(0.0, |value| (value * 256.0) as i32 as f32)
    }

    /// `int / 256`, 0 to 1. A note that is sounding keeps what it read.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(param) = self.params.get_mut(index) {
            let value = f64::from(value.round().clamp(-1e9, 1e9) as i32) * (1.0 / 256.0);
            *param = (value as f32).max(0.0).min(1.0);
        }
    }

    fn reset(&mut self) {
        self.silence();
        self.notes = [None; KEYS];
        self.held = [false; KEYS];
    }

    /// The longest a voice can sound: its hold, the hold envelope's fall
    /// from 1 to under 0.0001, the 25 ms of quiet and a pass, at the
    /// lowest pitch and bend; or the half second a quiet voice lasts.
    fn tail(&self) -> usize {
        let ends = [0.0, 1.0].map(|x| Patch::of(&self.params, x));
        let hold = ends.iter().map(|p| f64::from(p.hold)).fold(0.0, f64::max);
        let pitch = ends.iter().map(|p| p.pitch).fold(2.0, f64::min);
        let samples = (hold * f64::from(RATE) + 9_300.0).max(19_000.0) + 950.0 + 1_100.0;
        let step = f64::from(RATE / self.rate) * pitch * 0.5;
        (samples / step).ceil() as usize
    }

    fn fidelity(&self) -> Fidelity {
        if self.cut {
            Fidelity::Approximate("Drumpad membrane lines past their room")
        } else if self.stole {
            Fidelity::Approximate("Drumpad voices past 32")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

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

    /// A state as FL's projects save it.
    fn state(name: &str, params: &[f32; PARAMS]) -> Vec<u8> {
        let mut chunk = MAGIC[0].to_le_bytes().to_vec();
        let mut title = [0u8; NAME_LEN];
        title[..name.len()].copy_from_slice(name.as_bytes());
        chunk.extend(title);
        for value in params {
            chunk.extend(value.to_le_bytes());
        }
        let mut bytes = b"SSLF".to_vec();
        bytes.extend((chunk.len() as u32).to_le_bytes());
        bytes.extend(chunk);
        bytes
    }

    fn play(pad: &mut Drumpad, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
            pad.process(l, r);
        }
        (left, right)
    }

    #[test]
    fn a_state_reads_back_its_name_and_parameters() {
        let mut params = DEFAULTS;
        params[3] = 0.25;
        params[30] = 2.0;
        let mut pad = Drumpad::default();
        pad.set_state(&state("Test Kick", &params)).unwrap();
        assert_eq!(pad.preset(), "Test Kick");
        assert_eq!(pad.params(), &params);
        assert_eq!(pad.param(3), 64.0);
        pad.set_param(3, 128.0);
        assert_eq!(pad.params()[3], 0.5);
        // Another magic and a short chunk are turned down and change
        // nothing.
        let mut old = state("Old", &DEFAULTS);
        old[8] = b'1';
        assert!(pad.set_state(&old).is_err());
        assert!(pad.set_state(&state("x", &DEFAULTS)[..10]).is_err());
        assert_eq!(pad.preset(), "Test Kick");
    }

    #[test]
    fn a_hit_rings_and_stops_by_itself_and_a_note_off_does_nothing() {
        let mut pad = Drumpad::default();
        let mut params = DEFAULTS;
        params[28] = 0.2;
        pad.set_state(&state("", &params)).unwrap();
        pad.note_on(note(1, 60, 1.0));
        pad.note_off(1);
        let (left, right) = play(&mut pad, 44_100);
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
        let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01 && peak < 1.0, "{peak}");
        // Centred: both sides alike.
        assert_eq!(left, right);
        // The hold of 0.024 s and the fall after it are over well inside a
        // second, and the voice with them.
        assert!(left[30_000..].iter().all(|v| *v == 0.0));
        assert!(pad.voices.iter().all(|v| !v.active));
        assert!(pad.tail() < 44_100 * 2);
    }

    #[test]
    fn velocity_zero_sounds_nothing_and_the_key_does_not_change_the_pitch() {
        let mut pad = Drumpad::default();
        pad.note_on(note(1, 60, 0.0));
        let (left, _) = play(&mut pad, 2048);
        assert!(left.iter().all(|v| *v == 0.0));
        // Two keys, one after the other on fresh plugins: the same sound.
        let hit = |key| {
            let mut pad = Drumpad::default();
            pad.note_on(note(1, key, 1.0));
            play(&mut pad, 4096).0
        };
        assert_eq!(hit(40), hit(80));
    }

    #[test]
    fn notes_past_the_voices_take_the_oldest_and_say_so() {
        let mut pad = Drumpad::default();
        for voice in 0..VOICES as u32 + 1 {
            pad.note_on(note(voice, 60, 1.0));
        }
        assert_eq!(pad.sounding[60], VOICES as u32);
        assert_eq!(pad.started[0], VOICES as u32);
        assert!(matches!(pad.fidelity(), Fidelity::Approximate(_)));
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Drumpad's states skipped");
            return;
        };
        let mut saved = testing::every_preset(&root, NAME);
        let presets = saved.len();
        saved.extend(
            testing::projects(&root)
                .into_iter()
                .filter(|saved| saved.plugin.eq_ignore_ascii_case(NAME)),
        );
        let mut most = 0;
        for saved in &saved {
            let mut pad = crate::generator_by_name(&saved.plugin).unwrap();
            if let Err(error) = pad.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            let mut read = Drumpad::default();
            read.set_state(&saved.state).unwrap();
            most = most.max(read.voices[0].capacity());
            pad.note_on(note(1, 60, 1.0));
            let (mut left, mut right) = (vec![0.0; 8192], vec![0.0; 8192]);
            for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
                pad.process(l, r);
            }
            assert!(
                left.iter()
                    .chain(&right)
                    .all(|v| v.is_finite() && v.abs() < 4.0),
                "{}",
                saved.file
            );
            assert!(left.iter().any(|v| *v != 0.0), "{}", saved.file);
            assert_eq!(pad.fidelity(), Fidelity::Exact, "{}", saved.file);
        }
        // 961 presets and 11 states in projects, the render test with them.
        assert!(presets >= 961, "{presets}");
        assert_eq!(saved.len() - presets, 11);
        eprintln!("largest room for a voice's lines: {most} floats");
    }
}
