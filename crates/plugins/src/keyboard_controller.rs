//! Fruity Keyboard Controller: a channel whose notes are a controller.
//! Every key has a value of its own, and a note sends the value of its key
//! as output 0 and its velocity as output 1, glided towards once a tick.
//! It has no sound. `docs/plugins/fruity-keyboard-controller.md` reads it
//! from `Fruity Keyboard Controller_x64.dll`.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Attack, the glide up | 0 to 256 | 32 |
//! | 1 Release, the glide down | 0 to 256 | 32 |
//!
//! # Notes
//!
//! `TriggerVoice` (`0x6fdbd0`) takes a note whose key is inside the key
//! zone: the key's value becomes output 0's target and the note's
//! velocity, `ln(10 v + 1) / ln(21.55)` with `v` FL's `InitLevels.Vol`,
//! output 1's. A note outside the zone starts no voice. When a note ends
//! (`Voice_Kill`, `0x6fddf0`) the targets go back to the note held last,
//! or with none held and "Note off value" on, output 0's goes to that
//! value.
//!
//! # Ticks
//!
//! `NewTick` (`0x6fe180`) moves each output from where it is towards its
//! target, `target + (at - target) * k`, with `k` the attack's going up
//! and the release's going down: `0.01` to the power of the tick's
//! length over the wheel's time, `(11^(x/256) - 1) / 10` seconds, so that
//! the output is within 1% of its target once that time has passed. A
//! time of 0 jumps. Where an output has moved, it is sent in 2^30ths.

use crate::{Generator, NoteOn, StateError, knob_value};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Keyboard Controller";

/// The range of each parameter's wheel, from the editor's form.
const RANGES: [(i32, i32); 2] = [(0, 256), (0, 256)];
const DEFAULTS: [i32; 2] = [32, 32];

/// The keys that have a value, 0 to 131: the table at `0x144`.
const KEYS: usize = 132;
/// The state FL 26 saves: version 2 or 3. Version 0 has 120 keys.
const LEN_V2: usize = 4 + 8 + 4 * KEYS + 8 + 4 + 1;
/// An output that has not been set, as FL's constructor and flush leave it.
const UNSET: f32 = -1.0;
/// The most notes held at once that the plugin keeps. FL's list has no
/// end; a note past these takes the place of the one held longest.
const HELD: usize = 64;
/// An output of 1, in the units FL sends.
const FULL: f64 = 1_073_741_824.0;

/// A note that is held: what it set the targets to.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Held {
    voice: u32,
    value: f32,
    velocity: f32,
}

/// Fruity Keyboard Controller, as a [`Generator`] that makes no sound.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyboardController {
    params: [i32; 2],
    /// The value of each key, `0x144`.
    values: [f32; KEYS],
    /// The key zone, `0x13c..=0x140`.
    zone: (i32, i32),
    /// Output 0's value with no note held, `0x354`, and whether it is
    /// used, `0x370`.
    note_off: f32,
    auto_release: bool,
    /// Where each output is and where it goes, `0x358`.
    at: [f32; 2],
    target: [f32; 2],
    /// How much of the distance stays a tick, going up and going down:
    /// `0x368` and `0x36c`.
    keep: [f32; 2],
    tempo: f32,
    ppq: u16,
    held: [Held; HELD],
    holding: usize,
    outputs: [f32; 2],
    changed: bool,
}

impl Default for KeyboardController {
    /// As FL's constructor (`0x6fd340`) leaves it: the zone C4 to B5, every
    /// key's value 0, and the wheels at their defaults.
    fn default() -> Self {
        let mut plugin = KeyboardController {
            params: DEFAULTS,
            values: [0.0; KEYS],
            zone: (48, 71),
            note_off: 0.0,
            auto_release: false,
            at: [UNSET; 2],
            target: [UNSET; 2],
            keep: [0.0; 2],
            tempo: 120.0,
            ppq: 96,
            held: [Held {
                voice: 0,
                value: 0.0,
                velocity: 0.0,
            }; HELD],
            holding: 0,
            outputs: [0.0; 2],
            changed: false,
        };
        plugin.glides();
        plugin
    }
}

/// A wheel's time in seconds: `0x6fd6d0`.
fn seconds(wheel: i32) -> f64 {
    let x = (f64::from(wheel) * 0.003_906_25) as f32;
    let grown = (f64::from(x) * 11f64.ln()).exp();
    f64::from((grown - 1.0) as f32) * 0.1
}

impl KeyboardController {
    /// The glide of each wheel at the song's tempo and PPQ: `0x6fdea0`.
    fn glides(&mut self) {
        let tick = 0.01f64.ln() * 60.0 / f64::from(self.tempo * f32::from(self.ppq));
        for (keep, wheel) in self.keep.iter_mut().zip(self.params) {
            let time = seconds(wheel);
            *keep = if time > 0.0 {
                (tick / time).exp() as f32
            } else {
                0.0
            };
        }
    }

    /// FL's flush (`FPD_Flush`, in the dispatcher at `0x6fcd60`): no
    /// output is set, and output 0 heads for the note off value when it
    /// is used.
    fn flush(&mut self) {
        self.at = [UNSET; 2];
        self.target = [UNSET; 2];
        if self.auto_release {
            self.target[0] = self.note_off;
        }
        self.holding = 0;
    }

    /// A note has ended, or every note has: `Voice_Kill`.
    fn retarget(&mut self) {
        match self.holding.checked_sub(1) {
            Some(last) => {
                self.target = [self.held[last].value, self.held[last].velocity];
            }
            None if self.auto_release => self.target[0] = self.note_off,
            None => {}
        }
    }
}

impl Generator for KeyboardController {
    /// The state FL's `SaveRestoreState` (`0x6fcff0`) writes: a version,
    /// the two wheels (one before version 2), the 132 key values (120 in
    /// version 0), the key zone's two ends, the note off value, its switch
    /// and from version 3 the link button. FL 26 saves version 3; the
    /// corpus has version 2, 553 bytes.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let Some((version, _)) = bytes.split_first_chunk::<4>() else {
            return Err(StateError::Length {
                expected: LEN_V2,
                found: bytes.len(),
            });
        };
        let version = i32::from_le_bytes(*version);
        if version > 3 {
            return Err(StateError::Version { found: version });
        }
        let wheels = if version < 2 { 4 } else { 8 };
        let keys = if version > 0 { KEYS } else { 120 };
        let expected = 4 + wheels + 4 * keys + 8 + 4 + 1 + usize::from(version > 2);
        if bytes.len() != expected {
            return Err(StateError::Length {
                expected,
                found: bytes.len(),
            });
        }
        let word = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let float = |at: usize| f32::from_bits(word(at) as u32);
        self.params[0] = word(4);
        self.params[1] = if version < 2 { word(4) } else { word(8) };
        let table = 4 + wheels;
        for (key, value) in self.values.iter_mut().enumerate().take(keys) {
            *value = float(table + 4 * key);
        }
        let rest = table + 4 * keys;
        // The keyboard keeps the zone inside its keys (`0x6fed30`).
        let (low, high) = (word(rest), word(rest + 4));
        self.zone = (low.max(0), high.min(KEYS as i32 - 1));
        self.note_off = float(rest + 8);
        self.auto_release = bytes[rest + 12] != 0;
        self.glides();
        self.flush();
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// FL hands the plugin the tempo as `FPE_Tempo` (`ProcessEvent`,
    /// `0x6fd680`) and the glides follow it.
    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm as f32;
        self.glides();
    }

    fn set_ppq(&mut self, ppq: u16) {
        self.ppq = ppq.max(1);
        self.glides();
    }

    fn note_on(&mut self, note: NoteOn) {
        // `InitLevels.Pitch / 100`, rounded: the note's key. FL's is the
        // key the note has in the piano roll; the host's has the channel's
        // root note and pitch on it, which is the same for a channel at
        // C5 and with no pitch.
        let key = i32::from(note.key).max(0);
        if key < self.zone.0 || key > self.zone.1 {
            return;
        }
        let ceiling = 20.551_182_f32;
        let scaled = (note.velocity * 10.0).min(ceiling);
        let velocity = (f64::from(scaled + 1.0).ln() / f64::from(ceiling + 1.0).ln()) as f32;
        let value = self.values[key as usize];
        if self.holding == HELD {
            self.held.copy_within(1.., 0);
            self.holding -= 1;
        }
        self.held[self.holding] = Held {
            voice: note.voice,
            value,
            velocity,
        };
        self.holding += 1;
        self.target = [value, velocity];
    }

    /// FL's `Voice_Release` asks the host to kill the voice at once.
    fn note_off(&mut self, voice: u32) {
        let held = &self.held[..self.holding];
        let Some(at) = held.iter().position(|held| held.voice == voice) else {
            return;
        };
        self.held.copy_within(at + 1..self.holding, at);
        self.holding -= 1;
        self.retarget();
    }

    fn all_notes_off(&mut self) {
        self.holding = 0;
        self.retarget();
    }

    fn tick(&mut self, _ppq_tick: u64, _tempo: f64) {
        for output in 0..2 {
            let (at, target) = (self.at[output], self.target[output]);
            if at == target || target == UNSET {
                continue;
            }
            // FL compares the two as integers, which for the values from 0
            // up is comparing them as numbers.
            let next = if at == UNSET {
                target
            } else if (target.to_bits() as i32) < (at.to_bits() as i32) {
                target + (at - target) * self.keep[1]
            } else {
                target + (at - target) * self.keep[0]
            };
            self.at[output] = next;
            let sent = (f64::from(next) * FULL).round_ties_even();
            self.outputs[output] = (sent / FULL) as f32;
            self.changed = true;
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
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// `ProcessParam` (`0x6fd750`) works the glides out again.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
            self.glides();
        }
    }

    fn reset(&mut self) {
        self.flush();
    }

    fn tail(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version 2 state with a value for every key.
    fn state(
        wheels: [i32; 2],
        values: &[f32; KEYS],
        zone: (i32, i32),
        off: Option<f32>,
    ) -> Vec<u8> {
        let mut bytes = 2i32.to_le_bytes().to_vec();
        bytes.extend(wheels.iter().flat_map(|w| w.to_le_bytes()));
        bytes.extend(values.iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend(zone.0.to_le_bytes());
        bytes.extend(zone.1.to_le_bytes());
        bytes.extend(off.unwrap_or(0.0).to_le_bytes());
        bytes.push(u8::from(off.is_some()));
        bytes
    }

    fn ramp() -> [f32; KEYS] {
        std::array::from_fn(|key| key as f32 / 131.0)
    }

    fn note(voice: u32, key: i16) -> NoteOn {
        NoteOn {
            voice,
            key,
            ..NoteOn::default()
        }
    }

    fn ticked(plugin: &mut KeyboardController) -> [f32; 2] {
        plugin.tick(0, 120.0);
        plugin.outputs
    }

    #[test]
    fn a_note_sends_its_key_s_value_and_its_velocity() {
        let mut plugin = KeyboardController::default();
        plugin
            .set_state(&state([0, 0], &ramp(), (48, 83), None))
            .unwrap();
        // Nothing is sent before a note without the note off value.
        ticked(&mut plugin);
        assert!(!plugin.controller_changed());
        plugin.note_on(note(1, 60));
        let [value, velocity] = ticked(&mut plugin);
        assert_eq!(
            value,
            ((60.0f32 / 131.0) as f64 * FULL).round() as f32 / FULL as f32
        );
        // Velocity 100 is FL's gain of 1: ln 11 / ln 21.55, about 100/128.
        assert!((velocity - 100.0 / 128.0).abs() < 1e-3, "{velocity}");
        assert!(plugin.controller_changed());
        // A key outside the zone does nothing.
        plugin.note_on(note(2, 90));
        ticked(&mut plugin);
        assert!(!plugin.controller_changed());
        // A second note takes over, and when it ends the first is back.
        plugin.note_on(note(3, 70));
        assert_eq!(ticked(&mut plugin)[0], 70.0 / 131.0);
        plugin.note_off(3);
        assert_eq!(ticked(&mut plugin)[0], value);
        // With nothing held and no note off value, it stays.
        plugin.controller_changed();
        plugin.note_off(1);
        ticked(&mut plugin);
        assert!(!plugin.controller_changed());
    }

    #[test]
    fn the_note_off_value_is_sent_when_no_note_is_held() {
        let mut plugin = KeyboardController::default();
        let mut values = [0.0; KEYS];
        values[60] = 1.0;
        // Drumloop Masher's "Delay send 1".
        plugin
            .set_state(&state([0, 0], &values, (60, 60), Some(0.0)))
            .unwrap();
        assert_eq!(ticked(&mut plugin), [0.0, 0.0]);
        assert!(plugin.controller_changed());
        plugin.note_on(note(1, 60));
        assert_eq!(ticked(&mut plugin)[0], 1.0);
        plugin.note_off(1);
        assert_eq!(ticked(&mut plugin)[0], 0.0);
        plugin.controller_changed();
        plugin.note_on(note(1, 61));
        ticked(&mut plugin);
        assert!(!plugin.controller_changed());
    }

    #[test]
    fn the_wheels_glide_by_a_hundredth_over_their_time() {
        let mut plugin = KeyboardController::default();
        let mut values = [0.0; KEYS];
        values[60] = 1.0;
        plugin
            .set_state(&state([128, 64], &values, (0, 131), Some(0.0)))
            .unwrap();
        plugin.set_tempo(120.0);
        plugin.set_ppq(96);
        ticked(&mut plugin);
        // Attack 128 is (11^0.5 - 1) / 10 seconds, 44.6 ticks at 120 BPM
        // and 96 PPQ, after which 1% is left to go.
        let time = (11f64.sqrt() - 1.0) * 0.1;
        let ticks = time / (60.0 / 120.0 / 96.0);
        plugin.note_on(note(1, 60));
        let first = ticked(&mut plugin)[0];
        let keep = 0.01f64.powf(1.0 / ticks);
        assert!((f64::from(first) - (1.0 - keep)).abs() < 1e-6, "{first}");
        for _ in 1..ticks.round() as usize {
            ticked(&mut plugin);
        }
        assert!(
            (plugin.outputs[0] - 0.99).abs() < 0.002,
            "{}",
            plugin.outputs[0]
        );
        // The release is quicker on the way down.
        plugin.note_off(1);
        let down = 1.0 - ticked(&mut plugin)[0] / plugin.outputs[0].max(1e-9);
        assert!(down >= 0.0);
        assert!(plugin.keep[1] < plugin.keep[0]);
    }

    #[test]
    fn parameters_stay_inside_their_wheels() {
        let mut plugin = KeyboardController::default();
        assert_eq!(plugin.param_count(), 2);
        for (index, (low, high)) in RANGES.iter().enumerate() {
            assert_eq!(plugin.param(index), DEFAULTS[index] as f32);
            plugin.set_param(index, f32::MIN);
            assert_eq!(plugin.param(index), *low as f32);
            plugin.set_param(index, f32::MAX);
            assert_eq!(plugin.param(index), *high as f32);
        }
        assert_eq!(plugin.param(2), 0.0);
    }

    #[test]
    fn a_state_of_another_length_is_refused() {
        let mut plugin = KeyboardController::default();
        let mut bytes = state(DEFAULTS, &ramp(), (0, 10), None);
        assert!(plugin.set_state(&bytes).is_ok());
        bytes.push(0);
        assert!(plugin.set_state(&bytes).is_err());
        // Version 3 has the link button.
        bytes[0] = 3;
        assert!(plugin.set_state(&bytes).is_ok());
        bytes[0] = 4;
        assert!(plugin.set_state(&bytes).is_err());
        assert!(plugin.set_state(&[]).is_err());
    }

    #[test]
    fn every_state_in_fl_s_projects_loads() {
        let Some(root) = crate::testing::corpus() else {
            return;
        };
        let saved = crate::testing::projects(&root);
        let mine = saved.iter().filter(|s| s.plugin.eq_ignore_ascii_case(NAME));
        let mut count = 0;
        for saved in mine {
            let mut plugin = KeyboardController::default();
            plugin.set_state(&saved.state).unwrap();
            assert_eq!(plugin.controller_outputs().len(), 2);
            count += 1;
        }
        assert_eq!(count, 10);
    }
}
