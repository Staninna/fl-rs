//! Kepler Exo: Kepler's engine with all of it showing, as
//! `docs/plugins/kepler-exo.md` reads it out of `Kepler Exo_x64.dll` and
//! its `engine_x64.dll`.
//!
//! Kepler and Kepler Exo ship the same engine: the two `engine_x64.dll`
//! have the same data section byte for byte and code that differs by 0x90
//! bytes, with the same 258 parameters and the same functions at the same
//! addresses through the voice code. Kepler's shell saves 36 of the
//! parameters ("Kepler-1"); Exo's saves the first 155 in their own order
//! and an 8-slot modulation matrix ("Kepler-2"), and the engine loads
//! either.
//!
//! # The state
//!
//! FL's wrapper (a version of 1 and chunks; chunk 1 is the plugin's) or,
//! from older FL, the bare chunk: Kepler's 12-byte shell header, an i32
//! length and "Kepler-2", an i32 version (1 to 4), an i32 count and that
//! many f32, engine parameters 0 on (`FUN_18001efd0`); an i32 1, an i32
//! count and that many matrix slots (source, destination); from version 3
//! two bytes (a flag, and oversampling 4 for 2), from version 4 one more.
//! Before version 2 a synced LFO's rate is read anew, as for Kepler-1.
//!
//! # What plays
//!
//! The parameters go into [`Kepler`]'s port of the engine, which plays
//! what Kepler can set: DCO 1 with its saw, pulse and sub, the noise,
//! LFO 1, envelope 1, the four-pole ladder, the highpass, the chorus, the
//! arpeggiator, the main volume and pan and the velocity's curve.
//!
//! # What is not FL's
//!
//! Everything else the engine has is left out, and
//! [`Generator::fidelity`] says so when a state turns any of it on: DCO 2
//! with hard sync, the cross modulation and AM, the second LFO and
//! envelope, the filter's other slopes and types and its FM, the
//! modulation matrix, mono, legato and portamento, the EQ, saturation,
//! delay, reverb and hyper chorus. Of FL's 188 presets two use none of it.

use crate::kepler::{self, Kepler, PARAMS};
use crate::menvelope::Reader;
use crate::sampler::Chunks;
use crate::{Fidelity, Generator, NoteOn, StateError};
use std::path::Path;

/// The plugin's event 201 name.
pub const NAME: &str = "Kepler Exo";

/// The engine's DLL, from the directory [`Generator::set_resource_dir`] is
/// handed.
pub const DLL: &str = "Kepler Exo/engine_x64.dll";

/// The parameters a Kepler-2 state can set (`FUN_18001ed60` writes 0x9b).
const SAVED: usize = 0x9b;

/// The slots of the modulation matrix.
const SLOTS: usize = 8;

/// Kepler Exo.
pub struct KeplerExo {
    engine: Kepler,
    params: [f32; PARAMS],
    /// The state's own version, and "Kepler-1" for a state of Kepler's.
    pub version: i32,
    pub kepler_1: bool,
    /// Source and destination of each matrix slot, -1 for none.
    pub matrix: [(i32, i32); SLOTS],
}

impl Default for KeplerExo {
    fn default() -> Self {
        KeplerExo {
            engine: Kepler::default(),
            params: std::array::from_fn(kepler::default),
            version: 4,
            kepler_1: false,
            matrix: [(-1, -1); SLOTS],
        }
    }
}

/// Kepler's reread of a synced LFO's rate in states before its fix (the
/// end of `FUN_18001efd0`).
fn reread(rate: f32) -> f32 {
    let x = (2f64.powf(f64::from(rate * 7.0)) as f32 - 1.0) * 48.370_08 + 1.1;
    let ticks = (9216.0 / (x as i32).max(1) as f32) as i32 - 1;
    let y = (f64::from(ticks as f32 * 0.020_673_936 + 1.0)).log2() as f32;
    1.0 - y * 0.206_099_3
}

/// The engine's chunk out of FL's wrapper, or the bytes as they are.
fn chunk(bytes: &[u8]) -> Result<&[u8], StateError> {
    let short = StateError::Length {
        expected: 32,
        found: bytes.len(),
    };
    let first = bytes.first_chunk::<4>().ok_or(short)?;
    if u32::from_le_bytes(*first) != 1 {
        return Ok(bytes);
    }
    for part in Chunks::new(&bytes[4..]) {
        let (id, body) = part?;
        if id == 1 {
            return Ok(body);
        }
    }
    Err(short)
}

impl KeplerExo {
    /// An engine parameter, from 0 to 1.
    pub fn param_of(&self, index: usize) -> f32 {
        self.params.get(index).copied().unwrap_or(0.0)
    }

    /// The first part of the engine that the state turns on and the port
    /// leaves out.
    fn left_out(&self) -> Option<&'static str> {
        let p = &self.params;
        let on = |i: usize| p[i] >= 0.5;
        let moved = |i: usize| p[i] != kepler::default(i);
        let dco2 = on(0xc);
        let matrix = self
            .matrix
            .iter()
            .enumerate()
            .any(|(k, (s, d))| *s >= 0 && *d >= 0 && p[0x5c + k] != 0.5);
        let parts: [(bool, &'static str); 15] = [
            (dco2, "Kepler Exo: DCO 2 is left out"),
            (
                p[2] != 0.0 || p[0x2e] != 0.0 || (dco2 && p[0x16] != 0.0),
                "Kepler Exo: LFO 2 is left out",
            ),
            (
                p[4] != 0.0 || (dco2 && p[0x18] != 0.0),
                "Kepler Exo: envelope 2 is left out",
            ),
            (
                p[0x1d] != 0.0 || p[0x23] != 0.0 || (dco2 && p[0x1c] != 0.0),
                "Kepler Exo: the cross modulation and AM are left out",
            ),
            (p[0x29] != 0.0, "Kepler Exo: the filter's FM is left out"),
            (
                moved(0x28) || moved(0x9c),
                "Kepler Exo: the filter's other slopes and types are left out",
            ),
            (matrix, "Kepler Exo: the modulation matrix is left out"),
            (
                on(0x4b) || p[0x50] != 0.0,
                "Kepler Exo: mono and portamento are left out",
            ),
            (on(0x64), "Kepler Exo: the EQ is left out"),
            (on(0x86), "Kepler Exo: saturation is left out"),
            (on(0x6e), "Kepler Exo: the delay is left out"),
            (on(0x79), "Kepler Exo: the reverb is left out"),
            (on(0x89), "Kepler Exo: the hyper chorus is left out"),
            (on(0x52), "Kepler Exo: hold is left out"),
            (p[0x46] != 0.0, "Kepler Exo: the chorus noise is left out"),
        ];
        let tune = moved(0x99) || moved(0x95) || moved(0x96) || moved(0x100) || moved(0x101);
        parts
            .iter()
            .find(|(used, _)| *used)
            .map(|(_, why)| *why)
            .or(tune.then_some("Kepler Exo: master tune and the oscillator offsets are left out"))
    }
}

impl Generator for KeplerExo {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let body = chunk(bytes)?;
        let mut reader = Reader::new(body.get(12..).unwrap_or_default());
        let len = usize::try_from(reader.i32()?).map_err(|_| StateError::Length {
            expected: body.len() + 1,
            found: body.len(),
        })?;
        let name = reader.bytes(len)?;
        if name == b"Kepler-1\0" {
            // Kepler's own state: its loader, into the same engine.
            self.engine.set_state(body)?;
            self.params = std::array::from_fn(|i| self.engine.param_of(i));
            self.kepler_1 = true;
            self.matrix = [(-1, -1); SLOTS];
            return Ok(());
        }
        if name != b"Kepler-2\0" {
            return Err(StateError::Version {
                found: i32::from_le_bytes(name.first_chunk().copied().unwrap_or([0; 4])),
            });
        }
        let version = reader.i32()?;
        if version > 4 {
            return Err(StateError::Version { found: version });
        }
        let mut params: [f32; PARAMS] = std::array::from_fn(kepler::default);
        let mut matrix = [(-1, -1); SLOTS];
        let mut flags = [0u8; 2];
        if version > 0 {
            let count = usize::try_from(reader.i32()?).unwrap_or(0);
            for i in 0..count {
                let value = reader.f32()?;
                if let Some(param) = params.get_mut(i).filter(|_| i < SAVED) {
                    *param = value;
                }
            }
            if reader.i32()? == 1 {
                let slots = usize::try_from(reader.i32()?).unwrap_or(0);
                for k in 0..slots {
                    let pair = (reader.i32()?, reader.i32()?);
                    if let Some(slot) = matrix.get_mut(k) {
                        *slot = pair;
                    }
                }
            }
            if version > 2 {
                flags = [reader.u8()?, reader.u8()?];
            }
            if version > 3 {
                reader.u8()?;
            }
        }
        reader.end()?;
        if version < 2 {
            for (rate, sync) in [(0x30, 0x31), (0x36, 0x37)] {
                if params[sync] >= 0.5 {
                    params[rate] = reread(params[rate]);
                }
            }
        }
        self.engine
            .load(params, version, SAVED.min(PARAMS), false, flags[1] != 0);
        self.params = params;
        self.version = version;
        self.kepler_1 = false;
        self.matrix = matrix;
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.engine.set_sample_rate(hz);
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.engine.set_tempo(bpm);
    }

    /// Reads the noise and LFO 1's wave out of Exo's engine DLL.
    fn set_resource_dir(&mut self, dir: &Path) {
        self.engine.read_engine(&dir.join(DLL));
    }

    fn note_on(&mut self, note: NoteOn) {
        self.engine.note_on(note);
    }

    fn note_off(&mut self, voice: u32) {
        self.engine.note_off(voice);
    }

    fn all_notes_off(&mut self) {
        self.engine.all_notes_off();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.engine.process(left, right);
    }

    fn param(&self, _index: usize) -> f32 {
        0.0
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn tail(&self) -> usize {
        self.engine.tail()
    }

    fn fidelity(&self) -> Fidelity {
        if !self.engine.has_data() {
            return Fidelity::Approximate("Kepler Exo noise and LFO wave not available");
        }
        match self.left_out() {
            Some(why) => Fidelity::Approximate(why),
            None => Fidelity::Exact,
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
            ..NoteOn::default()
        }
    }

    /// The engine's stream inside Kepler's 12-byte shell.
    fn shell(stream: &[u8]) -> Vec<u8> {
        let mut bytes = 0u32.to_le_bytes().to_vec();
        bytes.extend((stream.len() as u32).to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(stream);
        bytes
    }

    /// A Kepler-2 state as FL 2026 writes one, in FL's wrapper: the 151
    /// parameters of a preset with `set` on the defaults, and a matrix.
    fn state(version: i32, set: &[(usize, f32)], matrix: &[(i32, i32)]) -> Vec<u8> {
        let mut s = 9i32.to_le_bytes().to_vec();
        s.extend(b"Kepler-2\0");
        s.extend(version.to_le_bytes());
        s.extend(151i32.to_le_bytes());
        for i in 0..151 {
            let value = set
                .iter()
                .find(|(k, _)| *k == i)
                .map_or(kepler::default(i), |(_, v)| *v);
            s.extend(value.to_le_bytes());
        }
        s.extend(1i32.to_le_bytes());
        s.extend(8i32.to_le_bytes());
        for k in 0..8 {
            let (a, b) = matrix.get(k).copied().unwrap_or((-1, -1));
            s.extend(a.to_le_bytes());
            s.extend(b.to_le_bytes());
        }
        if version > 2 {
            s.extend([0, 1]);
        }
        if version > 3 {
            s.push(0);
        }
        let chunk = shell(&s);
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend(1i32.to_le_bytes());
        bytes.extend((chunk.len() as i64).to_le_bytes());
        bytes.extend(chunk);
        bytes.extend(2i32.to_le_bytes());
        bytes.extend(0i64.to_le_bytes());
        bytes
    }

    fn play(plugin: &mut dyn Generator, len: usize) -> Vec<f32> {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        for (l, r) in left.chunks_mut(128).zip(right.chunks_mut(128)) {
            plugin.process(l, r);
        }
        left
    }

    #[test]
    fn a_kepler_2_state_sets_the_engine_s_parameters_in_order() {
        let mut plugin = KeplerExo::default();
        let bytes = state(2, &[(0x25, 0.25), (0x0c, 1.0), (0x5c, 0.8)], &[(4, 0x25)]);
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.version, 2);
        assert_eq!(plugin.param_of(0x25), 0.25);
        assert_eq!(plugin.param_of(0x0c), 1.0);
        assert_eq!(plugin.matrix[0], (4, 0x25));
        assert_eq!(plugin.matrix[1], (-1, -1));
        // What the state does not reach keeps the engine's default.
        assert_eq!(plugin.param_of(0x9a), 1.0);
        assert_eq!(plugin.engine.param_of(0x25), 0.25);
        // Cut short, it is turned down; a version 3 state has two bytes more.
        assert!(plugin.set_state(&bytes[..bytes.len() - 20]).is_err());
        plugin.set_state(&state(4, &[], &[])).unwrap();
        assert_eq!(plugin.version, 4);
    }

    #[test]
    fn before_version_2_a_synced_lfo_s_rate_is_read_anew() {
        let mut plugin = KeplerExo::default();
        let set = [(0x31, 1.0), (0x30, 0.5), (0x37, 1.0), (0x36, 0.25)];
        plugin.set_state(&state(1, &set, &[])).unwrap();
        assert_eq!(plugin.param_of(0x30), reread(0.5));
        assert_eq!(plugin.param_of(0x36), reread(0.25));
        plugin.set_state(&state(2, &set, &[])).unwrap();
        assert_eq!(plugin.param_of(0x30), 0.5);
    }

    #[test]
    fn a_kepler_1_state_plays_as_kepler_does() {
        let mut s = 9i32.to_le_bytes().to_vec();
        s.extend(b"Kepler-1\0");
        s.extend(4i32.to_le_bytes());
        s.extend(36i32.to_le_bytes());
        for i in 0..36 {
            let value = kepler::default(kepler::KEPLER1[i]);
            let value = if i == 14 { 1.0 } else { value };
            s.extend(value.to_le_bytes());
        }
        s.push(0);
        s.extend(1f32.to_le_bytes());
        s.extend(0.5f32.to_le_bytes());
        s.extend([0, 0, 0]);
        let bytes = shell(&s);
        let mut exo = KeplerExo::default();
        exo.set_state(&bytes).unwrap();
        assert!(exo.kepler_1);
        let mut kepler = Kepler::default();
        kepler.set_state(&bytes).unwrap();
        exo.note_on(note(1, 57));
        kepler.note_on(note(1, 57));
        let a = play(&mut exo, 4410);
        let b = play(&mut kepler, 4410);
        assert_eq!(a, b);
        assert!(a.iter().any(|x| x.abs() > 1e-3));
    }

    #[test]
    fn fidelity_names_what_the_state_turns_on_and_the_port_leaves_out() {
        let mut plugin = KeplerExo::default();
        plugin.set_state(&state(2, &[], &[])).unwrap();
        assert_eq!(plugin.left_out(), None);
        plugin.set_state(&state(2, &[(0x0c, 1.0)], &[])).unwrap();
        assert_eq!(plugin.left_out(), Some("Kepler Exo: DCO 2 is left out"));
        plugin.set_state(&state(2, &[(0x64, 1.0)], &[])).unwrap();
        assert_eq!(plugin.left_out(), Some("Kepler Exo: the EQ is left out"));
        plugin
            .set_state(&state(2, &[(0x5d, 0.9)], &[(-1, -1), (2, 0x25)]))
            .unwrap();
        assert_eq!(
            plugin.left_out(),
            Some("Kepler Exo: the modulation matrix is left out")
        );
    }

    #[test]
    fn every_state_fl_ships_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, Kepler Exo's states skipped");
            return;
        };
        let dir = root.join("../Plugins/Fruity/Generators");
        let ours = |saved: &testing::Saved| saved.plugin.eq_ignore_ascii_case(NAME);
        let projects: Vec<_> = testing::projects(&root).into_iter().filter(ours).collect();
        let presets = testing::every_preset(&root, NAME);
        assert!(presets.len() >= 188, "{}", presets.len());
        let mut exact = 0;
        for (n, saved) in projects.iter().chain(&presets).enumerate() {
            let mut plugin = KeplerExo::default();
            plugin
                .set_state(&saved.state)
                .unwrap_or_else(|e| panic!("{}: {e:?}", saved.file));
            plugin.set_resource_dir(&dir);
            exact += usize::from(plugin.fidelity() == Fidelity::Exact);
            if n % 6 != 0 {
                continue;
            }
            plugin.note_on(note(1, 48));
            plugin.note_on(note(2, 55));
            let left = play(&mut plugin, 8820);
            let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak.is_finite() && peak < 8.0, "{}: {peak}", saved.file);
        }
        assert!(exact >= 2, "{exact}");
    }
}
