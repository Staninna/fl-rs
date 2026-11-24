//! Fruity Voltage Controller: a channel that puts out control voltages,
//! DC on its audio outputs for a DC-coupled interface to drive modular
//! gear with, as `docs/plugins/fruity-voltage-controller.md` reads it
//! from `Fruity Voltage Controller_x64.dll`.
//!
//! It has eight outputs, a left and a right on its main output and on
//! three more. Each takes a source from the notes of one colour (FL's
//! "MIDI channel"): the pitch (1 V an octave, or 1 V a semitone, at a
//! tenth of full scale a volt), the gate, a trigger pulse, the velocity,
//! the volume, the pan, mod X or mod Y, or a clock at 24 a beat; then an
//! offset (bipolar or unipolar) and a scale, and an output range. While
//! "ON" is off it puts out nothing.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 to 7, each output's offset | 0 to 65536 | 32768 |
//! | 8 to 15, each output's scale, `2 v / 65536 - 1` | 0 to 65536 | 65536 |
//!
//! The outputs past the main one go to mixer tracks FL's host hands the
//! plugin; this crate's generators have one output, so those are left
//! out, as is the clock, which counts FL's ticks.

use crate::{Fidelity, Generator, NoteOn, StateError, knob_value};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Voltage Controller";

const PARAMS: usize = 16;
const OUTPUTS: usize = 8;
const SLOTS: usize = 16;
/// The notes the plugin keeps at once. FL's list has no end; a note past
/// these is not followed.
const HELD: usize = 64;

/// The sources, as the state numbers them (`0x00704c60`).
const PITCH: u8 = 0;
const GATE: u8 = 1;
const TRIGGER: u8 = 2;
const VELOCITY: u8 = 3;
const VOLUME: u8 = 4;
const PAN: u8 = 5;
const MOD_X: u8 = 6;
const MOD_Y: u8 = 7;
const CLOCK: u8 = 8;

/// The newest version FL reads.
const NEWEST: i32 = 3;

/// What one colour's notes last set (`0x194 + 0x38 slot`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Slot {
    pitch: f64,
    pan: f64,
    velocity: f64,
    volume: f64,
    mod_x: f64,
    mod_y: f64,
    gate: bool,
    /// A trigger pulse is under way, and the frames it has left.
    trigger: bool,
    left: i32,
}

/// A note that sounds: the host's number, its colour, and what it was
/// started with.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    id: u32,
    slot: usize,
    note: NoteOn,
}

/// Fruity Voltage Controller, as a [`Generator`].
#[derive(Debug, Clone, PartialEq)]
pub struct VoltageController {
    /// The offsets and the scales.
    params: [i32; PARAMS],
    /// Each output's source, its colour, and whether its offset is
    /// unipolar (`0x160 + 3 i`).
    sources: [u8; OUTPUTS],
    colours: [u8; OUTPUTS],
    unipolar: [bool; OUTPUTS],
    enabled: bool,
    /// 0 -1..1, 1 halved, 2 halved and lifted to 0..1 (`0x179`).
    range: u8,
    /// 0 an octave a volt, 1 a semitone, other none (`0x17a`).
    pitch_mode: u8,
    /// The mixer tracks outputs 2 to 4 go to, 0 none (`0x534`).
    tracks: [i32; 3],
    /// The clock's division, 24 (`0x52c`), and the pulses' length in
    /// frames (`0x514`).
    division: i32,
    pulse: i32,
    /// The pitch scale (`0x18c`).
    pitch_scale: f64,
    slots: [Slot; SLOTS],
    voices: [Option<Voice>; HELD],
}

impl Default for VoltageController {
    /// As the constructor (`0x00703890`) leaves it.
    fn default() -> Self {
        VoltageController {
            params: std::array::from_fn(|i| if i < OUTPUTS { 0x8000 } else { 0x10000 }),
            sources: [PITCH, GATE, VELOCITY, PAN, MOD_X, MOD_Y, 9, 9],
            colours: [0; OUTPUTS],
            unipolar: [false; OUTPUTS],
            enabled: false,
            range: 0,
            pitch_mode: 0,
            tracks: [0; 3],
            division: 24,
            pulse: 15,
            pitch_scale: 1.0,
            slots: [Slot::default(); SLOTS],
            voices: [None; HELD],
        }
    }
}

/// The reference the pitch counts octaves from: C5, `440 exp(3 ln 2 /
/// 12)` (`0x184`, with FL's math unit's `ln 2 / 12` at `0x770458`).
fn c5() -> f64 {
    (2f64.ln() / 12.0 * 3.0).exp() * 440.0
}

impl VoltageController {
    /// What a note sets its colour's values to (`0x00705ba0`), with the
    /// pitch in cents from key 60.
    fn update(&mut self, slot: usize, note: &NoteOn, cents: f32) {
        let clamp = |v: f64, lo: f64| v.clamp(lo, 1.0);
        let key = (f64::from(cents) + 6000.0) * f64::from(0.01f32);
        let s = &mut self.slots[slot];
        s.pan = clamp(f64::from(note.pan), -1.0);
        let ln11 = 1.0f32 / 11f64.ln() as f32;
        let loud = (f64::from(note.velocity * 10.0 + 1.0)).ln();
        s.velocity = clamp(
            f64::from((loud * f64::from(ln11)) as f32 / (0.01f32 * 128.0)),
            0.0,
        );
        s.volume = clamp(f64::from(note.velocity), 0.0);
        s.mod_x = clamp(f64::from(note.mod_x), -1.0);
        s.mod_y = clamp(f64::from(note.mod_y), -1.0);
        s.pitch = match self.pitch_mode {
            0 => {
                let hz = ((key - 57.0) * (2f64.ln() / 12.0)).exp();
                self.pitch_scale * (hz * 440.0 / c5()).log2() / 10.0
            }
            1 => self.pitch_scale * (key - 60.0) / 10.0,
            _ => 0.0,
        };
    }

    /// An output's offset (`0x007054f0`).
    fn offset(&self, out: usize) -> f64 {
        let v = f64::from(self.params[out]) / 65536.0;
        if self.unipolar[out] { v } else { v * 2.0 - 1.0 }
    }

    /// An output's scale (`0x00703190`).
    fn scale(&self, out: usize) -> f64 {
        f64::from(self.params[out + OUTPUTS] as f32 * 1.525_878_9e-5 * 2.0) - 1.0
    }

    /// An output's voltage (`0x00704c60`), the source and offset scaled
    /// and taken into the output's range (`0x00703760`).
    fn voltage(&self, out: usize) -> f64 {
        let s = &self.slots[usize::from(self.colours[out]) % SLOTS];
        let source = match self.sources[out] {
            PITCH => s.pitch,
            GATE => f64::from(u8::from(s.gate)),
            VELOCITY => s.velocity,
            VOLUME => s.volume,
            PAN => s.pan,
            MOD_X => s.mod_x,
            MOD_Y => s.mod_y,
            _ => 0.0,
        };
        let v = (source + self.offset(out)) * self.scale(out);
        match self.range {
            1 => (v * 0.5).clamp(-1.0, 1.0),
            2 => (v * 0.5 + 0.5).clamp(0.0, 1.0),
            _ => v.clamp(-1.0, 1.0),
        }
    }
}

/// Little-endian i32 at `at`.
fn word(bytes: &[u8], at: usize) -> Result<i32, StateError> {
    bytes
        .get(at..at + 4)
        .map(|w| i32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        .ok_or(StateError::Length {
            expected: at + 4,
            found: bytes.len(),
        })
}

impl Generator for VoltageController {
    /// `SaveRestoreState` (`0x00702bd0`): a version, the 16 parameters,
    /// the eight sources, ON, the range and the pitch mode, the eight
    /// polarities, the three tracks and the clock's division; from
    /// version 2 the pulse length and the pitch scale; from 3 the eight
    /// colours. FL leaves a state of version 4 or more alone.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = word(bytes, 0)?;
        if !(0..=NEWEST).contains(&version) {
            return Err(StateError::Version { found: version });
        }
        let mut len = 4 + 4 * PARAMS + OUTPUTS + 3 + OUTPUTS + 12 + 4;
        if version > 1 {
            len += 12;
        }
        if version >= 3 {
            len += OUTPUTS;
        }
        if bytes.len() != len {
            return Err(StateError::Length {
                expected: len,
                found: bytes.len(),
            });
        }
        let mut plugin = VoltageController::default();
        for (i, param) in plugin.params.iter_mut().enumerate() {
            *param = word(bytes, 4 + 4 * i)?;
        }
        let mut at = 4 + 4 * PARAMS;
        plugin.sources.copy_from_slice(&bytes[at..at + OUTPUTS]);
        at += OUTPUTS;
        plugin.enabled = bytes[at] != 0;
        plugin.range = bytes[at + 1];
        plugin.pitch_mode = bytes[at + 2];
        at += 3;
        for (unipolar, byte) in plugin.unipolar.iter_mut().zip(&bytes[at..at + OUTPUTS]) {
            *unipolar = *byte != 0;
        }
        at += OUTPUTS;
        for (i, track) in plugin.tracks.iter_mut().enumerate() {
            *track = word(bytes, at + 4 * i)?;
        }
        at += 12;
        plugin.division = word(bytes, at)?;
        at += 4;
        if version > 1 {
            plugin.pulse = word(bytes, at)?;
            let raw: [u8; 8] = bytes[at + 4..at + 12].try_into().unwrap_or([0; 8]);
            plugin.pitch_scale = f64::from_le_bytes(raw);
            at += 12;
        }
        if version >= 3 {
            plugin.colours.copy_from_slice(&bytes[at..at + OUTPUTS]);
        }
        *self = plugin;
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// `TriggerVoice` (`0x007058e0`): the note's colour (`FPV_GetColor`)
    /// takes its values, its gate opens and a trigger pulse starts.
    fn note_on(&mut self, note: NoteOn) {
        if note.slide {
            return;
        }
        let slot = usize::from(note.color) % SLOTS;
        let cents = f32::from(note.key - 60) * 100.0 + note.fine_pitch;
        self.update(slot, &note, cents);
        let s = &mut self.slots[slot];
        s.gate = true;
        s.trigger = true;
        s.left = self.pulse;
        if let Some(free) = self.voices.iter_mut().find(|v| v.is_none()) {
            *free = Some(Voice {
                id: note.voice,
                slot,
                note,
            });
        }
    }

    /// `Voice_Release` (`0x00705eb0`): the colour's gate closes; FL then
    /// kills the voice, which drops it from the list (`0x00705ef0`).
    fn note_off(&mut self, voice: u32) {
        for held in &mut self.voices {
            if let Some(v) = held
                && v.id == voice
            {
                self.slots[v.slot].gate = false;
                *held = None;
            }
        }
    }

    fn all_notes_off(&mut self) {
        for held in &mut self.voices {
            if let Some(v) = held.take() {
                self.slots[v.slot].gate = false;
            }
        }
    }

    /// `NewTick` (`0x007057f0`) reads every voice's levels again; the
    /// pitch is the one that moves.
    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        let found = self
            .voices
            .iter()
            .flatten()
            .find(|v| v.id == voice)
            .copied();
        if let Some(v) = found {
            self.update(v.slot, &v.note, pitch);
        }
    }

    /// `Gen_Render` (`0x00706180`): the main output's two voltages over
    /// the whole block, a trigger pulse on the frames it has left, and the
    /// pulses' count down by the block.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.enabled {
            for slot in &mut self.slots {
                slot.trigger = false;
                slot.left = 0;
            }
            return;
        }
        let len = left.len().min(right.len());
        for (side, out) in [(&mut *left, 0usize), (&mut *right, 1)] {
            let v = self.voltage(out);
            for sample in side[..len].iter_mut() {
                *sample = (f64::from(*sample) + v) as f32;
            }
            if self.sources[out] == TRIGGER {
                let s = &self.slots[usize::from(self.colours[out]) % SLOTS];
                if s.trigger {
                    let pulse = self.scale(out) + self.offset(out);
                    let frames = usize::try_from(s.left).unwrap_or(0).min(len);
                    for sample in side[..frames].iter_mut() {
                        *sample = (f64::from(*sample) + pulse) as f32;
                    }
                }
            }
        }
        for slot in &mut self.slots {
            if slot.trigger {
                slot.left = slot.left.saturating_sub(len as i32);
                if slot.left < 1 {
                    slot.trigger = false;
                }
            }
        }
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |v| *v as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(param) = self.params.get_mut(index) {
            *param = knob_value(value, (0, 65536));
        }
    }

    /// The song stops: every note goes, and the gates with them.
    fn reset(&mut self) {
        self.all_notes_off();
        for slot in &mut self.slots {
            slot.trigger = false;
            slot.left = 0;
        }
    }

    fn tail(&self) -> usize {
        0
    }

    fn fidelity(&self) -> Fidelity {
        if !self.enabled {
            return Fidelity::Exact;
        }
        let routed = (0..3).any(|k| {
            self.tracks[k] != 0 && self.sources[2 + 2 * k..4 + 2 * k].iter().any(|s| *s < 9)
        });
        if routed {
            Fidelity::Approximate(
                "Voltage Controller's outputs 2 to 4 go to mixer tracks, which are left out",
            )
        } else if self.sources[..2].contains(&CLOCK) {
            Fidelity::Approximate(
                "Voltage Controller's clock counts FL's ticks, which it is not given",
            )
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, projects};

    fn state(version: i32) -> Vec<u8> {
        let mut bytes = version.to_le_bytes().to_vec();
        for i in 0..PARAMS {
            let v: i32 = if i < 8 {
                0x8000
            } else if i == 9 {
                0xc000
            } else {
                0x10000
            };
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend([PITCH, GATE, VELOCITY, PAN, MOD_X, MOD_Y, 9, 9]);
        bytes.extend([1, 0, 0]);
        bytes.extend([0; 8]);
        bytes.extend([0; 12]);
        bytes.extend(24i32.to_le_bytes());
        if version > 1 {
            bytes.extend(15i32.to_le_bytes());
            bytes.extend(1f64.to_le_bytes());
        }
        if version >= 3 {
            bytes.extend([0; 8]);
        }
        bytes
    }

    #[test]
    fn a_note_puts_its_pitch_on_the_left_and_its_gate_on_the_right() {
        let mut plugin = VoltageController::default();
        plugin.set_state(&state(2)).unwrap();
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        plugin.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|v| *v == 0.0));
        // An octave up is a volt, a tenth of full scale; the gate, scaled
        // by a half, is half of it.
        plugin.note_on(NoteOn {
            voice: 1,
            key: 72,
            ..NoteOn::default()
        });
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        plugin.process(&mut l, &mut r);
        assert!(l.iter().all(|v| (v - 0.1).abs() < 1e-7), "{}", l[0]);
        assert!(r.iter().all(|v| *v == 0.5));
        plugin.set_note_pitch(1, -1200.0);
        let (mut l, mut r) = (vec![0.0; 8], vec![0.0; 8]);
        plugin.process(&mut l, &mut r);
        assert!((l[0] + 0.1).abs() < 1e-7);
        plugin.note_off(1);
        let (mut l, mut r) = (vec![0.0; 8], vec![0.0; 8]);
        plugin.process(&mut l, &mut r);
        assert!((l[0] + 0.1).abs() < 1e-7);
        assert_eq!(r[0], 0.0);
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
    }

    #[test]
    fn a_trigger_is_a_pulse_of_fifteen_frames_on_its_colour() {
        let mut plugin = VoltageController::default();
        let mut bytes = state(3);
        bytes[4 + 4 * PARAMS] = TRIGGER;
        let colours = bytes.len() - 8;
        bytes[colours] = 2;
        plugin.set_state(&bytes).unwrap();
        plugin.note_on(NoteOn {
            voice: 1,
            color: 1,
            ..NoteOn::default()
        });
        let (mut l, mut r) = (vec![0.0; 32], vec![0.0; 32]);
        plugin.process(&mut l, &mut r);
        assert!(l.iter().all(|v| *v == 0.0));
        plugin.note_on(NoteOn {
            voice: 2,
            color: 2,
            ..NoteOn::default()
        });
        let (mut l, mut r) = (vec![0.0; 32], vec![0.0; 32]);
        plugin.process(&mut l, &mut r);
        assert!(l[..15].iter().all(|v| *v == 1.0) && l[15..].iter().all(|v| *v == 0.0));
        let (mut l, mut r) = (vec![0.0; 32], vec![0.0; 32]);
        plugin.process(&mut l, &mut r);
        assert!(l.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn off_it_is_silent_and_a_bad_state_is_turned_down() {
        let mut plugin = VoltageController::default();
        plugin.note_on(NoteOn::default());
        let (mut l, mut r) = (vec![0.0; 8], vec![0.0; 8]);
        plugin.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|v| *v == 0.0));
        assert_eq!(
            plugin.set_state(&state(4)),
            Err(StateError::Version { found: 4 })
        );
        let mut short = state(2);
        short.pop();
        assert!(matches!(
            plugin.set_state(&short),
            Err(StateError::Length { .. })
        ));
    }

    #[test]
    fn every_state_fl_ships_loads() {
        let Some(root) = corpus() else {
            return;
        };
        let saved: Vec<_> = projects(&root)
            .into_iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        assert_eq!(saved.len(), 1);
        let mut plugin = VoltageController::default();
        plugin.set_state(&saved[0].state).unwrap();
        assert_eq!(plugin, {
            let mut p = VoltageController::default();
            p.set_state(&state(2)).unwrap();
            p
        });
        let presets = every_preset(&root, NAME);
        assert!(!presets.is_empty());
        for preset in &presets {
            let mut plugin = VoltageController::default();
            plugin.set_state(&preset.state).unwrap();
            assert_eq!(
                plugin.params,
                VoltageController::default().params,
                "{}",
                preset.file
            );
        }
    }
}
