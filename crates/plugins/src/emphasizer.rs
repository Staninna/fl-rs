//! Emphasizer: FL's light mastering limiter, as `docs/plugins/emphasizer.md`
//! has it. It is not Emphasis, though the two share their library.
//!
//! `Emphasizer_x64.dll` is a shell: it keeps six parameters, turns them
//! into the values of the limiter `LimiterLite` that
//! `LimiterLite_CreateInstance` of `Shared/ildsp_x64.dll` makes, and hands
//! it the sound. The install this was read from has no `Shared/`
//! directory, so the limiter is not built: the state, the parameters and
//! the values the shell gives the limiter are, and the sound passes
//! through as it is. [`Effect::fidelity`] says so for every state.
//!
//! The state is a version (2 now), the parameters as i32 (five before
//! version 2, six from it) and a byte: a switch of the limiter's that the
//! shell reads and sets as a float, 1 in every state FL ships. FL leaves a
//! state of version 3 or more alone.
//!
//! | parameter | range | to the limiter |
//! |---|---|---|
//! | 0 input gain | 0 to 10000 | `24 n` dB |
//! | 1 emphasis | 0 to 10000 | `n` |
//! | 2 clipping, soft or hard | 0 to 1 | as is |
//! | 3 level matching | 0 to 1 | as is |
//! | 4 bypass | 0 to 1 | as is |
//! | 5 output gain | 0 to 10000 | `20 log10((11^n - 1) / 10)` dB, 0 at the top |
//!
//! `n` is the wheel's value over 10000.

use crate::{Effect, Fidelity, StateError, knob_value, state_version};

/// The plugin's event 201 name.
pub const NAME: &str = "Emphasizer";

const PARAMS: usize = 6;

/// The range of each parameter's control.
const RANGES: [(i32, i32); PARAMS] = [
    (0, 10_000),
    (0, 10_000),
    (0, 1),
    (0, 1),
    (0, 1),
    (0, 10_000),
];

/// FL's own "Default" preset, with the output gain a state of version 1
/// does not have at the top, as the one project state has it.
const DEFAULTS: [i32; PARAMS] = [0, 5013, 0, 0, 0, 10_000];

/// The newest version FL reads, and the lengths of the two it writes.
const NEWEST: i32 = 2;
const LEN_1: usize = 4 + 4 * 5 + 1;
const LEN_2: usize = 4 + 4 * PARAMS + 1;

/// What the sound misses.
const MISSING: &str =
    "Emphasizer's limiter, LimiterLite in Shared/ildsp_x64.dll, is not built: the sound passes";

/// A wheel's share of its way, as `ProcessParam` (`0x006c2980`) works it
/// out and hands it on as a float.
fn share(param: usize, value: i32) -> f32 {
    let (min, max) = RANGES[param];
    (f64::from(value - min) * (1.0 / f64::from(max - min))).clamp(0.0, 1.0) as f32
}

/// The input gain the limiter is given, in dB (`0x006c1520` with 0, 24
/// and 1: `0 + 24 exp(ln n / 1)`).
fn input_db(share: f32) -> f32 {
    if share <= 0.0 {
        return 0.0;
    }
    ((f64::from(share).ln() / 1.0).exp() * 24.0) as f32
}

/// The output gain the limiter is given, in dB (`0x006c1dc0`): FL's knob
/// curve, 1 at the top, and minus infinity at the bottom.
fn output_db(share: f32) -> f32 {
    let gain = if share == 1.0 {
        1.0
    } else {
        ((f64::from(share * 11f32.ln()).exp() - 1.0) * f64::from(0.1f32)) as f32
    };
    if gain > 0.0 {
        (f64::from(gain).log10() * 20.0) as f32
    } else {
        f32::NEG_INFINITY
    }
}

/// Emphasizer. A new one has FL's "Default" preset.
#[derive(Debug, Clone, PartialEq)]
pub struct Emphasizer {
    params: [i32; PARAMS],
    /// The byte after the parameters.
    switch: u8,
}

impl Default for Emphasizer {
    fn default() -> Self {
        Emphasizer {
            params: DEFAULTS,
            switch: 1,
        }
    }
}

impl Emphasizer {
    /// The values the shell hands the limiter's six parameters
    /// (`0x006c2bd0`), in the limiter's units: input gain in dB, the
    /// emphasis share, clipping, level matching, bypass, and output gain
    /// in dB. What a port of the limiter would be set from.
    pub fn limiter_values(&self) -> [f32; PARAMS] {
        let n = |param: usize| share(param, self.params[param]);
        [
            input_db(n(0)),
            n(1),
            self.params[2] as f32,
            self.params[3] as f32,
            self.params[4] as f32,
            output_db(n(5)),
        ]
    }
}

impl Effect for Emphasizer {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let version = state_version(bytes, LEN_2)?;
        if version > NEWEST {
            return Err(StateError::Version { found: version });
        }
        let (len, count) = if version < 2 {
            (LEN_1, 5)
        } else {
            (LEN_2, PARAMS)
        };
        if bytes.len() != len {
            return Err(StateError::Length {
                expected: len,
                found: bytes.len(),
            });
        }
        let mut params = DEFAULTS;
        for (param, word) in bytes[4..4 + 4 * count]
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            let value = i32::from_le_bytes(*word);
            let (min, max) = RANGES[param];
            if value < min || value > max {
                return Err(StateError::Range { param, value });
            }
            params[param] = value;
        }
        self.params = params;
        self.switch = bytes[len - 1];
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// The limiter is not built: the sound passes as it is.
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            self.params[index] = knob_value(value, *range);
        }
    }

    fn reset(&mut self) {}

    fn fidelity(&self) -> Fidelity {
        Fidelity::Approximate(MISSING)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, projects, sine, state};

    fn bytes(version: i32, params: &[i32], switch: u8) -> Vec<u8> {
        let mut bytes = state(&[version]);
        bytes.extend(state(params));
        bytes.push(switch);
        bytes
    }

    #[test]
    fn the_wheels_come_to_the_limiter_s_values() {
        assert_eq!(input_db(0.0), 0.0);
        assert_eq!(input_db(1.0), 24.0);
        assert!((input_db(0.5) - 12.0).abs() < 1e-5);
        assert_eq!(output_db(1.0), 0.0);
        assert_eq!(output_db(0.0), f32::NEG_INFINITY);
        // Half way is the knob curve's (sqrt 11 - 1) / 10, -12.7 dB.
        let half = 20.0 * ((11f64.sqrt() - 1.0) / 10.0).log10();
        assert!((f64::from(output_db(0.5)) - half).abs() < 1e-4);
        assert_eq!(share(1, 5013), 0.5013);
        let default = Emphasizer::default().limiter_values();
        assert_eq!(default, [0.0, 0.5013, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn both_versions_load_and_the_sound_passes() {
        let mut plugin = Emphasizer::default();
        plugin
            .set_state(&bytes(1, &[2500, 10_000, 1, 1, 0], 0))
            .unwrap();
        assert_eq!(plugin.params, [2500, 10_000, 1, 1, 0, 10_000]);
        assert_eq!(plugin.switch, 0);
        plugin
            .set_state(&bytes(2, &[0, 5013, 0, 0, 1, 7000], 1))
            .unwrap();
        assert_eq!(plugin.params, [0, 5013, 0, 0, 1, 7000]);
        let tone = sine(0.01, 500);
        let (mut l, mut r) = (tone.clone(), tone.clone());
        plugin.process(&mut l, &mut r);
        assert_eq!((l, r), (tone.clone(), tone));
        assert_eq!(plugin.fidelity(), Fidelity::Approximate(MISSING));
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = Emphasizer::default();
        let before = plugin.clone();
        assert_eq!(
            plugin.set_state(&bytes(2, &[0; 5], 1)),
            Err(StateError::Length {
                expected: 29,
                found: 25
            })
        );
        assert_eq!(
            plugin.set_state(&bytes(1, &[0; 6], 1)),
            Err(StateError::Length {
                expected: 25,
                found: 29
            })
        );
        assert_eq!(
            plugin.set_state(&bytes(3, &[0; 6], 1)),
            Err(StateError::Version { found: 3 })
        );
        assert_eq!(
            plugin.set_state(&bytes(2, &[0, 0, 2, 0, 0, 0], 1)),
            Err(StateError::Range { param: 2, value: 2 })
        );
        assert_eq!(
            plugin.set_state(&[2, 0]),
            Err(StateError::Length {
                expected: 29,
                found: 2
            })
        );
        assert_eq!(plugin, before);
    }

    #[test]
    fn a_moved_wheel_is_the_same_plugin_as_its_state() {
        let mut plugin = Emphasizer::default();
        for (index, value) in [12_000.0, 3.4, 0.6, -1.0, f32::NAN, 9999.5]
            .into_iter()
            .enumerate()
        {
            plugin.set_param(index, value);
        }
        plugin.set_param(6, 1.0);
        assert_eq!((plugin.param(6), plugin.param_count()), (0.0, 6));
        let mut same = Emphasizer::default();
        same.set_state(&bytes(2, &[10_000, 3, 1, 0, 0, 10_000], 1))
            .unwrap();
        assert_eq!(plugin, same);
    }

    #[test]
    fn every_state_fl_ships_loads() {
        let Some(root) = corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states of {NAME} skipped");
            return;
        };
        // The template's one, of version 2, and the five presets of
        // version 1: its own two and the plugin databases' four, all the
        // "Default" preset.
        let saved: Vec<_> = projects(&root)
            .into_iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        assert_eq!(saved.len(), 1);
        let mut plugin = Emphasizer::default();
        plugin.set_state(&saved[0].state).unwrap();
        assert_eq!(plugin, Emphasizer::default());
        let presets = every_preset(&root, NAME);
        assert_eq!(presets.len(), 5);
        for preset in &presets {
            let mut plugin = Emphasizer::default();
            plugin.set_state(&preset.state).unwrap();
            assert_eq!(plugin, Emphasizer::default(), "{}", preset.file);
        }
    }
}
