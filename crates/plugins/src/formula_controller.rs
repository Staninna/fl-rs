//! Fruity Formula Controller: a formula of three knobs, `a`, `b` and `c`,
//! and of the song, worked out once a tick and sent as an internal
//! controller output. It has no sound path.
//! `docs/plugins/fruity-formula-controller.md` reads it from
//! `Fruity Formula Controller_x64.dll`, and [`crate::fl_formula`] is its
//! parser.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 to 2: `a`, `b`, `c` | 0 to 65536, which is 1.0 | 32768 |
//!
//! # Ticks
//!
//! `NewTick` (`0x6d82b0`), once the formula has compiled: its value as a
//! single, kept inside 0 to 1 by comparing its bits as an integer (so a
//! NaN with its sign bit set is 0 and one without is 1), sent as output 0
//! in 2^30ths when it is not what was sent last. A formula that does not
//! compile sends nothing; an empty one is FL's NaN constant and sends 0.

use crate::fl_formula::{Formula, Inputs};
use crate::{Effect, Fidelity, StateError, knob_value};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Formula Controller";

const RANGE: (i32, i32) = (0, 65_536);
const DEFAULTS: [i32; 3] = [32_768; 3];
/// An output of 1 in the units FL sends.
const FULL: f64 = 1_073_741_824.0;

/// Fruity Formula Controller, as an [`Effect`] that leaves its block alone.
#[derive(Debug, Clone, PartialEq)]
pub struct FormulaController {
    params: [i32; 3],
    /// The compiled formula, `None` where it did not compile.
    formula: Option<Formula>,
    /// FL's formula text as the state has it.
    text: String,
    inputs: Inputs,
    /// `Random`'s seed.
    seed: u32,
    /// One over the song's PPQ, a single (`0x741570`).
    per_tick: f32,
    /// What was sent last, in 2^30ths; -1 sends the next.
    sent: i64,
    output: [f32; 1],
    changed: bool,
}

impl Default for FormulaController {
    /// As FL's constructor (`0x6d79d0`) leaves it: the knobs at a half, no
    /// formula, 96 PPQ.
    fn default() -> Self {
        let mut plugin = FormulaController {
            params: DEFAULTS,
            formula: Some(Formula::empty()),
            text: String::new(),
            inputs: Inputs {
                tempo: 120.0,
                ..Inputs::default()
            },
            seed: 0,
            per_tick: 1.0 / 96.0,
            sent: 0,
            output: [0.0],
            changed: false,
        };
        for index in 0..3 {
            plugin.update(index);
        }
        plugin
    }
}

/// The bytes of a state, read from the front.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], StateError> {
        let end = self.at.saturating_add(len);
        let taken = self.bytes.get(self.at..end).ok_or(StateError::Length {
            expected: end,
            found: self.bytes.len(),
        })?;
        self.at = end;
        Ok(taken)
    }

    fn i32(&mut self) -> Result<i32, StateError> {
        let bytes = self.take(4)?;
        Ok(i32::from_le_bytes(bytes.try_into().unwrap_or([0; 4])))
    }

    /// A length: a byte, or 255 and an i32 (`0x6c56b0`, `0x6c57f0`).
    fn len(&mut self) -> Result<usize, StateError> {
        let short = self.take(1)?[0];
        if short == 0xff {
            Ok(self.i32()? as u32 as usize)
        } else {
            Ok(usize::from(short))
        }
    }
}

impl FormulaController {
    /// `ProcessParam` (`0x6d7da0`) with `REC_UpdateValue`: `a`, `b` or `c`
    /// is the knob over 65536.
    fn update(&mut self, index: usize) {
        self.inputs.vars[index] = f64::from(self.params[index]) * (1.0 / 65_536.0);
    }

    /// The formula's text, as the editor has it.
    pub fn formula(&self) -> &str {
        &self.text
    }

    /// Whether the formula compiled.
    pub fn compiled(&self) -> bool {
        self.formula.is_some()
    }
}

impl Effect for FormulaController {
    /// The state FL's `SaveRestoreState` (`0x6d7680`) writes: a version,
    /// the three knobs, the formula (a length and 8-bit text, its trailing
    /// control characters dropped), the description (from version 4 a
    /// length and UTF-16, before an i32 length and 8-bit text) and from
    /// version 2 a byte for the editor.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let mut reader = Reader { bytes, at: 0 };
        let version = reader.i32()?;
        if version > 4 {
            return Err(StateError::Version { found: version });
        }
        let mut params = [0; 3];
        for param in &mut params {
            *param = reader.i32()?;
        }
        let len = reader.len()?;
        let text = reader.take(len)?;
        let text: String = text.iter().map(|b| char::from(*b)).collect();
        let text = text.trim_end_matches(|c: char| u32::from(c) < 0x20);
        if version < 4 {
            let len = reader.i32()?.max(0) as usize;
            reader.take(len)?;
        } else {
            let len = reader.len()?;
            reader.take(len.saturating_mul(2))?;
        }
        if version > 1 {
            reader.take(1)?;
        }
        if reader.at != bytes.len() {
            return Err(StateError::Length {
                expected: reader.at,
                found: bytes.len(),
            });
        }
        self.params = params.map(|p| p.clamp(RANGE.0, RANGE.1));
        for index in 0..3 {
            self.update(index);
        }
        self.text = text.to_owned();
        self.formula = Formula::compile(text).ok();
        self.reset();
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// `GetTempo` is the tempo FL hands the plugin with `FPE_Tempo`
    /// (`ProcessEvent`, `0x6d7d70`).
    fn set_tempo(&mut self, bpm: f64) {
        self.inputs.tempo = f64::from(bpm as f32);
    }

    /// `FPD_SetTimeSig` sets `SongTime`'s step, one over the PPQ.
    fn set_ppq(&mut self, ppq: u16) {
        self.per_tick = (1.0 / f64::from(ppq.max(1))) as f32;
    }

    fn tick(&mut self, ppq_tick: u64, tempo: f64) {
        let Some(formula) = &self.formula else {
            return;
        };
        self.inputs.tempo = f64::from(tempo as f32);
        // The host's song position in ticks, times one over the PPQ.
        self.inputs.song_time = f64::from(ppq_tick as i32 as f32 * self.per_tick);
        let Some(value) = formula.eval(&self.inputs, &mut self.seed) else {
            return;
        };
        let single = value as f32;
        let bits = if value.is_nan() {
            // A NaN keeps its sign on its way to a single.
            if value.is_sign_negative() {
                -1
            } else {
                i32::MAX
            }
        } else {
            single.to_bits() as i32
        };
        let clamped = if bits < 0 {
            0.0
        } else if bits > 0x3f80_0000 {
            1.0
        } else {
            single
        };
        let sent = (f64::from(clamped) * FULL).round_ties_even() as i64;
        if sent != self.sent {
            self.sent = sent;
            self.output[0] = (sent as f64 / FULL) as f32;
            self.changed = true;
        }
    }

    fn controller_outputs(&self) -> &[f32] {
        &self.output
    }

    fn controller_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// `FPF_NoProcess`: FL never asks the plugin for sound.
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}

    fn param_count(&self) -> usize {
        3
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index < 3 {
            self.params[index] = knob_value(value, RANGE);
            self.update(index);
        }
    }

    /// FL's flush (`FPD_Flush` in the dispatcher at `0x6d7310`): the next
    /// tick sends its value whatever it is.
    fn reset(&mut self) {
        self.sent = -1;
    }

    fn fidelity(&self) -> Fidelity {
        let uses = self.formula.as_ref().map(Formula::uses).unwrap_or_default();
        if uses.outside {
            Fidelity::Approximate("the formula reads the clock or the mouse, which are 0 here")
        } else if uses.rand {
            Fidelity::Approximate("Rand starts from a seed of 0, where FL's is the process's")
        } else {
            Fidelity::Exact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version 4 state with this formula and no description.
    fn state(params: [i32; 3], formula: &str) -> Vec<u8> {
        let mut bytes = 4i32.to_le_bytes().to_vec();
        bytes.extend(params.iter().flat_map(|p| p.to_le_bytes()));
        bytes.push(formula.len() as u8);
        bytes.extend(formula.bytes());
        bytes.push(0);
        bytes.push(0);
        bytes
    }

    fn ticked(plugin: &mut FormulaController, tick: u64) -> Option<f32> {
        plugin.tick(tick, 120.0);
        plugin.controller_changed().then_some(plugin.output[0])
    }

    #[test]
    fn the_formula_of_the_knobs_is_sent_when_it_moves() {
        let mut plugin = FormulaController::default();
        plugin
            .set_state(&state([65_536, 16_384, 0], "a*b+c"))
            .unwrap();
        assert_eq!(ticked(&mut plugin, 0), Some(0.25));
        assert_eq!(ticked(&mut plugin, 1), None);
        plugin.set_param(2, 32_768.0);
        assert_eq!(ticked(&mut plugin, 2), Some(0.75));
        // Past 1 it stays at 1, under 0 at 0.
        plugin.set_param(2, 65_536.0);
        assert_eq!(ticked(&mut plugin, 3), Some(1.0));
        plugin.set_state(&state([0, 0, 0], "a-1")).unwrap();
        assert_eq!(ticked(&mut plugin, 4), Some(0.0));
    }

    #[test]
    fn song_time_counts_beats_from_the_tick() {
        let mut plugin = FormulaController::default();
        plugin.set_ppq(96);
        plugin
            .set_state(&state(DEFAULTS, "frac(SongTime)"))
            .unwrap();
        assert_eq!(ticked(&mut plugin, 48), Some(0.5));
        assert_eq!(ticked(&mut plugin, 96 + 24), Some(0.25));
        plugin.set_state(&state(DEFAULTS, "GetTempo/240")).unwrap();
        assert_eq!(ticked(&mut plugin, 0), Some(0.5));
    }

    #[test]
    fn an_empty_formula_sends_0_after_a_flush_and_a_bad_one_nothing() {
        let mut plugin = FormulaController::default();
        // Astes - Raubana - LIFE's "Randomness".
        let mut life = 4i32.to_le_bytes().to_vec();
        life.extend(
            [46_592i32, 25_941, 32_768]
                .iter()
                .flat_map(|p| p.to_le_bytes()),
        );
        life.push(0);
        let about = "Randomness\ra=base level, b=range";
        life.push(about.len() as u8);
        life.extend(about.encode_utf16().flat_map(u16::to_le_bytes));
        life.push(0);
        plugin.set_state(&life).unwrap();
        assert!(plugin.compiled());
        assert_eq!(ticked(&mut plugin, 0), Some(0.0));
        assert_eq!(ticked(&mut plugin, 1), None);
        plugin.set_state(&state(DEFAULTS, "a +* b")).unwrap();
        assert!(!plugin.compiled());
        assert_eq!(ticked(&mut plugin, 0), None);
    }

    #[test]
    fn a_state_of_another_length_is_refused() {
        let mut plugin = FormulaController::default();
        let mut bytes = state(DEFAULTS, "a");
        bytes.push(0);
        assert!(plugin.set_state(&bytes).is_err());
        assert!(plugin.set_state(&bytes[..10]).is_err());
        bytes.pop();
        bytes[0] = 5;
        assert!(plugin.set_state(&bytes).is_err());
    }

    #[test]
    fn rand_and_the_mouse_are_said_to_be_approximate() {
        let mut plugin = FormulaController::default();
        plugin.set_state(&state(DEFAULTS, "a")).unwrap();
        assert_eq!(plugin.fidelity(), Fidelity::Exact);
        plugin.set_state(&state(DEFAULTS, "rand()*a")).unwrap();
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(_)));
        plugin.set_state(&state(DEFAULTS, "MouseX")).unwrap();
        assert!(matches!(plugin.fidelity(), Fidelity::Approximate(_)));
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
            let mut plugin = FormulaController::default();
            plugin.set_state(&saved.state).unwrap();
            assert!(plugin.compiled(), "{}", saved.file);
            plugin.tick(0, 120.0);
            assert!(plugin.controller_changed());
            count += 1;
        }
        assert_eq!(count, 2);
    }
}
