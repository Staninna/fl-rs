//! FL Slayer, a guitar and bass synth: reFX's Slayer 2 engine of 2003,
//! which FL's 64-bit plugin bridges to in `Slayer_Engine.dll`. Six
//! plucked strings (two damped delay lines each, a pickup comb and a
//! lowpass), an amp with its cabinet, and one of 15 effects. The engine is
//! in [`crate::fl_slayer_engine`]; `docs/plugins/fl-slayer.md` maps it.
//!
//! The state is 29 i32s: a word FL's presets all leave 0, then the 28
//! parameters by the tag of their control. The wheels go 0 to 65535 and
//! the selectors count from 0; the engine is handed `value / 65535` and
//! `index / (count - 1)`, which its `trunc(x * (count - 0.01))` turns back
//! into the index.
//!
//! Six of the seven string types pluck with a short 16-bit sample that
//! sits in the engine DLL; [`Excitations`] reads them from there, and
//! without the DLL those strings stay silent, as [`Generator::fidelity`]
//! says.

use crate::fl_slayer_engine::{Engine, Event, Excitations, VOICES};
use crate::{Fidelity, Generator, NoteOn, StateError, knob_value};
use std::path::Path;

/// The plugin's event 201 name.
pub const NAME: &str = "FL Slayer";

/// The engine, from the directory [`Generator::set_resource_dir`] is
/// handed.
pub const DLL: &str = "FL Slayer/Slayer_Engine.dll";

/// The 28 parameters, by the tag FL gives their controls.
pub const PARAMS: usize = 28;
const STATE_LEN: usize = 4 * (PARAMS + 1);

/// How many values each selector has; 0 for a wheel of 0 to 65535.
const COUNTS: [i32; PARAMS] = [
    7, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 4, 16, 0, 0, 0, 6, 0, 2, 2,
];

/// The defaults of FL's form (`TSynthEditorForm`): 6String3, a single
/// coil, the tube amp and the combo cabinet.
const DEFAULTS: [i32; PARAMS] = [
    3, 1, 17_695, 30_147, 0, 65_535, 9175, 0, 655, 0, 0, 0, 1, 32_768, 32_768, 0, 36_700, 30_147,
    26_870, 2, 0, 7864, 27_525, 0, 0, 0, 0, 0,
];

/// The most frames a host block is walked in.
const CHUNK: usize = 1024;

fn range(index: usize) -> (i32, i32) {
    match COUNTS[index] {
        0 => (0, 65_535),
        n => (0, n - 1),
    }
}

/// What the engine is handed for a parameter's value.
fn engine_value(index: usize, value: i32) -> f32 {
    match COUNTS[index] {
        0 => value as f32 / 65_535.0,
        n => value as f32 / (n - 1) as f32,
    }
}

/// Where the 15 excitation samples are in the engine DLL: the virtual
/// address of each, a 16-bit count and then the PCM, by the index the
/// engine's voices keep (FUN_10002ad0 sets the table at 0x821678).
const TABLES: [(u8, u32); 15] = [
    (1, 0x1001_79b8),
    (7, 0x1001_8b48),
    (0xe, 0x1001_99c8),
    (0xf, 0x1001_a854),
    (0x10, 0x1001_b718),
    (0x11, 0x1001_c5e0),
    (0x12, 0x1001_d1b8),
    (0x13, 0x1001_da80),
    (0x14, 0x1001_e5e0),
    (0x15, 0x1001_f000),
    (0x16, 0x1001_fc10),
    (0x18, 0x1002_0324),
    (0x1a, 0x1002_1278),
    (0x1b, 0x1002_1f74),
    (0x1c, 0x1002_2c1c),
];

impl Excitations {
    /// From the bytes of `Slayer_Engine.dll`, a 32-bit DLL. `None` for a
    /// file that is no such DLL or has its samples elsewhere.
    pub fn from_dll(bytes: &[u8]) -> Option<Self> {
        let u16_at = |at: usize| -> Option<usize> {
            Some(usize::from(u16::from_le_bytes(
                *bytes.get(at..)?.first_chunk::<2>()?,
            )))
        };
        let u32_at = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk::<4>()?))
        };
        let header = u32_at(0x3c)? as usize;
        let optional = header.checked_add(24)?;
        if bytes.get(header..header + 4)? != b"PE\0\0" || u16_at(optional)? != 0x10b {
            return None;
        }
        let base = u32_at(optional + 28)?;
        let table = optional + u16_at(header + 20)?;
        let sections: Vec<(u32, u32, u32)> = (0..u16_at(header + 6)?)
            .map(|k| {
                let at = table + 40 * k;
                Some((u32_at(at + 12)?, u32_at(at + 16)?, u32_at(at + 20)?))
            })
            .collect::<Option<_>>()?;
        let file = |address: u32, len: usize| -> Option<&[u8]> {
            let rva = address.checked_sub(base)?;
            sections.iter().find_map(|&(start, size, raw)| {
                let from = rva.checked_sub(start)?;
                let to = from.checked_add(len as u32).filter(|&to| to <= size)?;
                bytes.get((raw + from) as usize..(raw + to) as usize)
            })
        };
        let mut tables = vec![Vec::new(); 29];
        for (index, address) in TABLES {
            let count = i16::from_le_bytes(*file(address, 2)?.first_chunk::<2>()?);
            let count = usize::try_from(count).ok().filter(|&n| n > 16)?;
            let pcm = file(address + 2, count * 2)?;
            tables[usize::from(index)] = pcm
                .as_chunks::<2>()
                .0
                .iter()
                .map(|w| i16::from_le_bytes(*w))
                .collect();
        }
        Some(Excitations { tables })
    }

    /// From the DLL under FL's `Plugins/Fruity/Generators` directory.
    pub fn read(dir: &Path) -> Option<Self> {
        Self::from_dll(&std::fs::read(dir.join(DLL)).ok()?)
    }
}

/// FL Slayer: its 29 saved words and the engine they set.
pub struct FlSlayer {
    head: i32,
    params: [i32; PARAMS],
    engine: Box<Engine>,
    rate: f32,
    /// The host's notes and the keys they went down on.
    keys: [(u32, i32); 64],
    mono: Vec<f32>,
}

impl Default for FlSlayer {
    fn default() -> Self {
        let mut slayer = FlSlayer {
            head: 0,
            params: DEFAULTS,
            engine: Box::new(Engine::new()),
            rate: 44_100.0,
            keys: [(u32::MAX, 0); 64],
            mono: vec![0.0; CHUNK],
        };
        slayer.send_all();
        slayer
    }
}

impl FlSlayer {
    fn send_all(&mut self) {
        for (index, &value) in self.params.iter().enumerate() {
            self.engine.params[index] = engine_value(index, value);
        }
    }

    /// The 29 words of the state FL saves for these parameters.
    pub fn state(&self) -> Vec<u8> {
        std::iter::once(self.head)
            .chain(self.params)
            .flat_map(i32::to_le_bytes)
            .collect()
    }

    pub fn set_excitations(&mut self, excitations: Option<Excitations>) {
        self.engine.set_excitations(excitations);
    }
}

impl Generator for FlSlayer {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        if bytes.len() != STATE_LEN {
            return Err(StateError::Length {
                expected: STATE_LEN,
                found: bytes.len(),
            });
        }
        let words: Vec<i32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|w| i32::from_le_bytes(*w))
            .collect();
        for (index, &value) in words[1..].iter().enumerate() {
            let (min, max) = range(index);
            if !(min..=max).contains(&value) {
                return Err(StateError::Range {
                    param: index,
                    value,
                });
            }
        }
        self.head = words[0];
        self.params.copy_from_slice(&words[1..]);
        self.send_all();
        self.engine.settle();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.rate = hz;
        self.engine.set_rate(hz);
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.engine.set_tempo(bpm as f32);
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        if let Some(excitations) = Excitations::read(dir) {
            self.engine.set_excitations(Some(excitations));
        }
    }

    /// The key and the velocity go to the engine as FL's bridge sends MIDI:
    /// the velocity over 127, and the note's cents as the engine's own
    /// detune of up to 127 cents.
    fn note_on(&mut self, note: NoteOn) {
        let key = i32::from(note.key);
        let velocity = (note.velocity * 100.0 / 127.0).clamp(0.0, 1.0);
        let detune = note.fine_pitch.round().clamp(-127.0, 127.0) as i8;
        if let Some(slot) = self.keys.iter_mut().find(|(v, _)| *v == u32::MAX) {
            *slot = (note.voice, key);
        }
        self.engine.push(Event::On {
            key,
            velocity,
            detune,
        });
    }

    fn note_off(&mut self, voice: u32) {
        if let Some(slot) = self.keys.iter_mut().find(|(v, _)| *v == voice) {
            let key = slot.1;
            *slot = (u32::MAX, 0);
            self.engine.push(Event::Off { key });
        }
    }

    fn all_notes_off(&mut self) {
        for slot in &mut self.keys {
            if slot.0 != u32::MAX {
                self.engine.push(Event::Off { key: slot.1 });
                *slot = (u32::MAX, 0);
            }
        }
        self.engine.release_all();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let mut at = 0;
        while at < n {
            let len = (n - at).min(CHUNK);
            self.engine.process(
                &mut left[at..at + len],
                &mut right[at..at + len],
                &mut self.mono[..len],
            );
            at += len;
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |&v| v as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index < PARAMS {
            let value = knob_value(value, range(index));
            self.params[index] = value;
            self.engine.params[index] = engine_value(index, value);
        }
    }

    fn reset(&mut self) {
        self.keys = [(u32::MAX, 0); 64];
        self.engine.reset();
    }

    /// A released string fades within `0.2 rate` of its tone steps, the
    /// slowest of which is 0.625 a sample; the delays add their own.
    fn tail(&self) -> usize {
        let strings = 0.2 * self.rate / 0.625 * self.rate / 44_100.0;
        let fx = match self.params[20] {
            9 | 10 | 12 => 0.05,
            14 => 0.1,
            15 => 8.0,
            _ => 0.0,
        };
        (strings + (fx + 0.05) * self.rate) as usize
    }

    fn fidelity(&self) -> Fidelity {
        if self.params[0] != 0 && !self.engine.has_excitations() {
            return Fidelity::Approximate(
                "FL Slayer's strings but Noise pluck with samples in Slayer_Engine.dll, which was not found",
            );
        }
        Fidelity::Exact
    }
}

const _: () = assert!(VOICES == 6);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use std::path::PathBuf;

    fn generators() -> Option<PathBuf> {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, FL Slayer's files skipped");
            return None;
        };
        Some(root.join("../Plugins/Fruity/Generators"))
    }

    fn note(key: i16) -> NoteOn {
        NoteOn {
            voice: key as u32,
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

    fn play(slayer: &mut FlSlayer, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        for k in (0..len).step_by(512) {
            let end = (k + 512).min(len);
            slayer.process(&mut left[k..end], &mut right[k..end]);
        }
        (left, right)
    }

    fn peak(sound: &[f32]) -> f32 {
        sound.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    #[test]
    fn the_defaults_are_fl_s_form_and_its_state_round_trips() {
        let mut slayer = FlSlayer::default();
        assert_eq!(slayer.param_count(), 28);
        assert_eq!(slayer.param(0), 3.0);
        assert_eq!(slayer.param(13), 32_768.0);
        assert_eq!(slayer.param(28), 0.0);
        let state = slayer.state();
        assert_eq!(state.len(), 116);
        slayer.set_param(20, 99.0);
        assert_eq!(slayer.param(20), 15.0);
        slayer.set_state(&state).unwrap();
        assert_eq!(slayer.param(20), 0.0);
        assert_eq!(slayer.state(), state);
    }

    #[test]
    fn a_state_out_of_range_or_of_the_wrong_length_is_turned_down() {
        let mut slayer = FlSlayer::default();
        let mut state = slayer.state();
        state[4..8].copy_from_slice(&7i32.to_le_bytes());
        assert_eq!(
            slayer.set_state(&state),
            Err(StateError::Range { param: 0, value: 7 })
        );
        assert_eq!(
            slayer.set_state(&state[..112]),
            Err(StateError::Length {
                expected: 116,
                found: 112
            })
        );
        assert_eq!(slayer.param(0), 3.0);
    }

    #[test]
    fn a_selector_reaches_the_engine_as_its_index_over_its_last() {
        // The engine's trunc(x * 6.99), trunc(x * 15.99) and so on give
        // the index back for every value.
        for (index, &count) in COUNTS.iter().enumerate() {
            for value in 0..count {
                let x = engine_value(index, value);
                assert_eq!((x * (count as f32 - 0.01)) as i32, value, "{index}");
            }
        }
    }

    #[test]
    fn the_noise_string_plays_without_the_dll_an_octave_under_its_key() {
        let mut slayer = FlSlayer::default();
        // Noise, no effect, the dry amp and cabinet.
        for (index, value) in [(0, 0.0), (12, 0.0), (19, 0.0), (20, 0.0), (7, 0.0)] {
            slayer.set_param(index, value);
        }
        assert_eq!(slayer.fidelity(), Fidelity::Exact);
        slayer.set_sample_rate(44_100.0);
        slayer.note_on(note(57));
        let (left, right) = play(&mut slayer, 44_100);
        assert_eq!(left, right);
        assert!(left.iter().all(|x| x.is_finite()));
        let level = peak(&left[4410..]);
        assert!(level > 1e-3 && level < 4.0, "{level}");
        // 220 Hz for key 57, and two lines of twice the period: 110 Hz.
        let sound = &left[8820..30_000];
        let like = |lag: usize| {
            let (mut xy, mut xx) = (0.0f64, 0.0f64);
            for k in 0..sound.len() - lag {
                xy += f64::from(sound[k]) * f64::from(sound[k + lag]);
                xx += f64::from(sound[k]) * f64::from(sound[k]);
            }
            xy / xx
        };
        let best = (300..500)
            .max_by(|&a, &b| like(a).total_cmp(&like(b)))
            .unwrap();
        assert!((best as f64 - 401.0).abs() < 3.0, "{best}");
        // Let go, and the string is gone within the tail.
        slayer.note_off(57);
        let tail = slayer.tail();
        let _ = play(&mut slayer, tail);
        let (after, _) = play(&mut slayer, 4096);
        assert!(peak(&after) < 1e-4, "{}", peak(&after));
    }

    #[test]
    fn a_sampled_string_wants_the_dll() {
        let mut slayer = FlSlayer::default();
        assert!(matches!(slayer.fidelity(), Fidelity::Approximate(_)));
        slayer.set_resource_dir(Path::new("/nowhere"));
        assert!(matches!(slayer.fidelity(), Fidelity::Approximate(_)));
        assert!(Excitations::from_dll(b"MZ, and no DLL").is_none());
        slayer.note_on(note(60));
        let (left, _) = play(&mut slayer, 8192);
        assert!(left.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn fl_s_engine_has_its_fifteen_samples() {
        let Some(dir) = generators() else { return };
        let samples = Excitations::read(&dir).expect("Slayer_Engine.dll");
        let lengths: Vec<usize> = TABLES
            .iter()
            .map(|&(i, _)| samples.tables[usize::from(i)].len())
            .collect();
        assert_eq!(
            lengths,
            [
                2245, 1853, 1860, 1887, 1889, 1513, 1122, 1453, 1293, 1542, 903, 1959, 1660, 1617,
                4010
            ]
        );
        assert_eq!(samples.tables[1][..4], [0, -3, 1, -6]);
    }

    #[test]
    fn every_preset_loads_and_plays_a_chord_sanely() {
        let Some(dir) = generators() else { return };
        let root = testing::corpus().unwrap();
        let samples = Excitations::read(&dir).expect("Slayer_Engine.dll");
        let saved = testing::every_preset(&root, NAME);
        assert_eq!(saved.len(), 64);
        for saved in saved {
            let mut slayer = FlSlayer::default();
            slayer.set_excitations(Some(samples.clone()));
            slayer.set_sample_rate(48_000.0);
            if let Err(error) = slayer.set_state(&saved.state) {
                panic!("{}: {error}", saved.file);
            }
            assert_eq!(slayer.state(), saved.state, "{}", saved.file);
            assert_eq!(slayer.fidelity(), Fidelity::Exact);
            for key in [40, 47, 52] {
                slayer.note_on(note(key));
            }
            let (left, right) = play(&mut slayer, 24_000);
            for key in [40, 47, 52] {
                slayer.note_off(key as u32);
            }
            let (l2, r2) = play(&mut slayer, 12_000);
            let all = left.iter().chain(&right).chain(&l2).chain(&r2);
            assert!(all.clone().all(|x| x.is_finite()), "{}", saved.file);
            let level = all.fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(level > 1e-3 && level < 16.0, "{}: {level}", saved.file);
        }
    }
}
