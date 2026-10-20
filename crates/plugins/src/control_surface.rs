//! Control Surface: a panel of knobs, sliders, buttons and X-Y pads whose
//! values are internal controller outputs. It has no sound path.
//! `docs/plugins/control-surface.md` reads it from `Control Surface_x64.dll`.
//!
//! # State
//!
//! FL's `SaveRestoreState` (`0x854f70`) writes a version, 1, and then
//! chunks (`0x853b90`): an id, a 64-bit length and the bytes. Chunk
//! `0x834` starts a control, its first word the control's type, and the
//! chunks up to `0x835` are that control's: its name, place, look, and
//! one `0x836` for each value it has, a single from 0 to 1, its default
//! and the parameter it is. A knob, a slider, a button, a check box and a
//! digit wheel have one value, an X-Y pad two (X then Y), and a bevel, a
//! label and a keyboard none. A state of version -1, from before the
//! chunks, is a count and a record of 0x148 bytes for each control
//! (`0x853850`), whose values take the first parameters that are free.
//!
//! # Outputs
//!
//! Parameter `n` is output `n`. When a value is set, from the editor or
//! from the host, its control's handler (`0x8556e0`) rounds it times 2^30
//! and queues it for the first parameter that names that value, and the
//! next `NewTick` (`0x854880`) sends what is queued to FL. A value only
//! moves when it is set, so the outputs are the values the state has,
//! which the first [`Effect::tick`] sends.
//!
//! A host sets a parameter in 2^30ths, as `ProcessParam` (`0x8549a0`)
//! takes it with `REC_FromMIDI`, and so do [`Effect::param`] and
//! [`Effect::set_param`] here.

use crate::{Effect, StateError};

/// The plugin's event 201 name.
pub const NAME: &str = "Control Surface";

/// The parameters, and so the outputs, FL gives the plugin.
pub const PARAMS: usize = 2048;

/// A value of 1 in FL's units.
const FULL: f32 = 1_073_741_824.0;

/// How many values a control of each type has: knob, X-Y pad, slider,
/// button, check box, bevel, label, digit wheel and keyboard, as their
/// constructors (`0x848cf0` to `0x84db80`) give them.
const VALUES: [usize; 9] = [1, 2, 1, 1, 1, 0, 0, 1, 0];

/// The chunks the state is made of.
const CONTROL: u32 = 0x834;
const VALUE: u32 = 0x836;

/// Control Surface, as an [`Effect`] that leaves its block alone.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlSurface {
    /// Every control's values, one after another.
    values: Vec<f32>,
    /// For each parameter, the value it names, and for each value the
    /// output its handler queues: the first parameter that names it.
    params: Vec<Option<usize>>,
    queue: Vec<usize>,
    outputs: Vec<f32>,
    changed: bool,
}

impl Default for ControlSurface {
    fn default() -> Self {
        ControlSurface {
            values: Vec::new(),
            params: vec![None; PARAMS],
            queue: Vec::new(),
            outputs: vec![0.0; PARAMS],
            changed: false,
        }
    }
}

/// A control as a state has it: its type, and each value with the
/// parameter it is.
struct Saved {
    kind: u32,
    values: Vec<(f32, i32)>,
}

/// The controls of a state of version 0 or 1.
fn chunks(mut bytes: &[u8]) -> Result<Vec<Saved>, StateError> {
    let mut controls: Vec<Saved> = Vec::new();
    let mut read = 4;
    while !bytes.is_empty() {
        let short = |expected| StateError::Length {
            expected,
            found: read + bytes.len(),
        };
        let Some((head, rest)) = bytes.split_first_chunk::<12>() else {
            return Err(short(read + 12));
        };
        let id = u32::from_le_bytes(head[..4].try_into().unwrap());
        let size = u64::from_le_bytes(head[4..].try_into().unwrap());
        let Some(data) = usize::try_from(size).ok().and_then(|size| rest.get(..size)) else {
            return Err(short(read + 12 + size as usize));
        };
        let word = |at: usize| data.get(at..at + 4).map(|w| w.try_into().unwrap());
        match id {
            CONTROL => {
                let kind = word(0).map_or(u32::MAX, u32::from_le_bytes);
                controls.push(Saved {
                    kind,
                    values: Vec::new(),
                });
            }
            VALUE => {
                if let (Some(control), Some(value), Some(param)) =
                    (controls.last_mut(), word(0), word(8))
                {
                    let value = (f32::from_le_bytes(value), i32::from_le_bytes(param));
                    control.values.push(value);
                }
            }
            _ => {}
        }
        read += 12 + data.len();
        bytes = &rest[data.len()..];
    }
    Ok(controls)
}

/// A value of a state of version -1 that takes the first free parameter.
const FREE: i32 = i32::MIN;

/// The controls of a state of version -1: a count, and a record of 0x148
/// bytes a control, its type at 0x108 and two values at 0x110 of 20 bytes
/// each, the value at 8 and whether it is a parameter at 16.
fn records(bytes: &[u8]) -> Vec<Saved> {
    const RECORD: usize = 0x148;
    let Some((count, bytes)) = bytes.split_first_chunk::<4>() else {
        return Vec::new();
    };
    // FL reads a record while a whole one is left and the count, counted
    // down, is not 0: one of 0 or less reads them all.
    let count = usize::try_from(i32::from_le_bytes(*count))
        .ok()
        .filter(|n| *n > 0);
    let records = bytes.as_chunks::<RECORD>().0.iter();
    let records = records.take(count.unwrap_or(usize::MAX));
    let word = |record: &[u8], at: usize| -> [u8; 4] { record[at..at + 4].try_into().unwrap() };
    records
        .map(|record| Saved {
            kind: u32::from_le_bytes(word(record, 0x108)),
            values: (0..2)
                .map(|value| {
                    let at = 0x110 + 20 * value;
                    let param = i32::from_le_bytes(word(record, at + 16));
                    let param = if param != 0 { FREE } else { -1 };
                    (f32::from_le_bytes(word(record, at + 8)), param)
                })
                .collect(),
        })
        .collect()
}

impl ControlSurface {
    /// The output a value's handler queues.
    fn send(&mut self, value: usize) {
        let output = self.queue[value];
        let raw = (self.values[value] * FULL) as f64;
        self.outputs[output] = (raw.round_ties_even() / f64::from(FULL)) as f32;
        self.changed = true;
    }
}

impl Effect for ControlSurface {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let Some(version) = bytes.first_chunk::<4>().map(|v| i32::from_le_bytes(*v)) else {
            return Err(StateError::Length {
                expected: 4,
                found: bytes.len(),
            });
        };
        let controls = match version {
            -1 => records(&bytes[4..]),
            0 | 1 => chunks(&bytes[4..])?,
            found => return Err(StateError::Version { found }),
        };
        self.values.clear();
        self.params.fill(None);
        self.queue.clear();
        self.outputs.fill(0.0);
        for control in controls {
            let count = VALUES.get(control.kind as usize).copied().unwrap_or(0);
            for (value, param) in control.values.into_iter().take(count) {
                let index = self.values.len();
                self.values.push(value);
                let param = if param == FREE {
                    self.params.iter().position(Option::is_none)
                } else {
                    usize::try_from(param).ok()
                };
                if let Some(slot) = param.and_then(|param| self.params.get_mut(param)) {
                    *slot = Some(index);
                }
            }
        }
        // A value's handler finds the first parameter that names it.
        self.queue = vec![usize::MAX; self.values.len()];
        for (param, value) in self.params.iter().enumerate().rev() {
            if let Some(value) = value {
                self.queue[*value] = param;
            }
        }
        for value in 0..self.values.len() {
            if self.queue[value] != usize::MAX {
                self.send(value);
            }
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    fn controller_outputs(&self) -> &[f32] {
        &self.outputs
    }

    fn controller_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// FL never asks the plugin for sound, so the block stays as it came.
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}

    fn param_count(&self) -> usize {
        PARAMS
    }

    /// The value a parameter names, in 2^30ths.
    fn param(&self, index: usize) -> f32 {
        let value = self.params.get(index).copied().flatten();
        value.map_or(0.0, |value| self.values[value] * FULL)
    }

    /// The value moves to `value` 2^30ths, a single from 0 to 1, and its
    /// output goes out on the next tick.
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(Some(at)) = self.params.get(index).copied() {
            let raw = value.round().clamp(0.0, FULL) as i32;
            self.values[at] = (f64::from(raw) / f64::from(FULL)) as f32;
            self.send(at);
        }
    }

    /// FL's flush does nothing to the plugin (`0x853510`).
    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chunk that ends a control's.
    const END: u32 = 0x835;

    fn chunk(id: u32, data: &[u8]) -> Vec<u8> {
        let mut bytes = id.to_le_bytes().to_vec();
        bytes.extend((data.len() as u64).to_le_bytes());
        bytes.extend(data);
        bytes
    }

    fn control(kind: u32, values: &[(f32, i32)]) -> Vec<u8> {
        let mut head = kind.to_le_bytes().to_vec();
        head.resize(32, 0);
        let mut bytes = chunk(CONTROL, &head);
        bytes.extend(chunk(
            0x837,
            "Knob"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        ));
        for (value, param) in values {
            let mut data = value.to_le_bytes().to_vec();
            data.extend(value.to_le_bytes());
            data.extend(param.to_le_bytes());
            bytes.extend(chunk(VALUE, &data));
        }
        bytes.extend(chunk(END, &[]));
        bytes
    }

    fn state(controls: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend(chunk(2000, &[0; 64]));
        bytes.extend(controls.concat());
        bytes
    }

    #[test]
    fn each_value_is_the_output_its_parameter_names() {
        let mut plugin = ControlSurface::default();
        let bytes = state(&[
            control(5, &[]),
            control(0, &[(0.5, 20)]),
            control(3, &[(1.0, 2)]),
            control(1, &[(0.25, 7), (0.75, 8)]),
            // A label has no value, and a knob only one.
            control(6, &[(0.9, 30)]),
            control(0, &[(0.3, 31), (0.6, 32)]),
        ]);
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.controller_outputs().len(), PARAMS);
        let out = plugin.controller_outputs();
        assert_eq!((out[20], out[2], out[7], out[8]), (0.5, 1.0, 0.25, 0.75));
        assert_eq!((out[30], out[31], out[32]), (0.0, 0.3, 0.0));
        assert_eq!(plugin.param(31), 0.3f32 * FULL);
        // The first tick sends them, once.
        plugin.tick(0, 120.0);
        assert!(plugin.controller_changed());
        plugin.tick(1, 120.0);
        assert!(!plugin.controller_changed());
    }

    #[test]
    fn a_host_sets_a_value_in_2_30ths() {
        let mut plugin = ControlSurface::default();
        plugin
            .set_state(&state(&[control(0, &[(0.0, 0)])]))
            .unwrap();
        plugin.controller_changed();
        plugin.set_param(0, FULL / 4.0);
        assert!(plugin.controller_changed());
        assert_eq!(plugin.controller_outputs()[0], 0.25);
        assert_eq!(plugin.param(0), FULL / 4.0);
        plugin.set_param(0, f32::MIN);
        assert_eq!(plugin.param(0), 0.0);
        plugin.set_param(0, f32::MAX);
        assert_eq!(plugin.param(0), FULL);
        // A parameter no control has does nothing.
        plugin.set_param(5, FULL);
        assert_eq!(plugin.param(5), 0.0);
        assert_eq!(plugin.controller_outputs()[5], 0.0);
    }

    #[test]
    fn a_later_control_takes_a_parameter_over() {
        let mut plugin = ControlSurface::default();
        let bytes = state(&[control(0, &[(0.5, 9)]), control(0, &[(0.25, 9)])]);
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.controller_outputs()[9], 0.25);
        plugin.set_param(9, FULL);
        assert_eq!(plugin.values, [0.5, 1.0]);
    }

    #[test]
    fn other_versions_and_cut_states_are_refused() {
        let mut plugin = ControlSurface::default();
        let mut bytes = state(&[control(0, &[(0.5, 0)])]);
        bytes[0] = 2;
        assert!(matches!(
            plugin.set_state(&bytes),
            Err(StateError::Version { found: 2 })
        ));
        bytes[0] = 1;
        bytes.pop();
        assert!(plugin.set_state(&bytes).is_err());
        assert!(plugin.set_state(&[1, 0]).is_err());
    }

    #[test]
    fn a_state_from_before_the_chunks_gives_free_parameters() {
        let mut bytes = (-1i32).to_le_bytes().to_vec();
        bytes.extend(0i32.to_le_bytes());
        for value in [0.5f32, 0.75] {
            let mut record = vec![0; 0x148];
            record[0x110 + 8..0x110 + 12].copy_from_slice(&value.to_le_bytes());
            record[0x110 + 16] = 1;
            bytes.extend(record);
        }
        let mut plugin = ControlSurface::default();
        plugin.set_state(&bytes).unwrap();
        assert_eq!(plugin.controller_outputs()[..3], [0.5, 0.75, 0.0]);
    }

    #[test]
    fn every_state_in_fl_s_projects_loads() {
        let Some(root) = crate::testing::corpus() else {
            return;
        };
        let saved = crate::testing::projects(&root);
        let mine = saved.iter().filter(|s| s.plugin.eq_ignore_ascii_case(NAME));
        let mut params = 0;
        for saved in mine {
            let mut plugin = ControlSurface::default();
            plugin.set_state(&saved.state).unwrap();
            params += plugin.params.iter().flatten().count();
        }
        assert_eq!(params, 133);
    }
}
