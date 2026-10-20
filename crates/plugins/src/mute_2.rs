//! Fruity Mute 2: silences the left side, the right side or both, as
//! `docs/plugins/fruity-mute-2.md` has it.
//!
//! The state is a version word and the two wheels, each 0 to 1024 with 512
//! its default. The Mute wheel mutes below half way. The Channel wheel's
//! place over a third, cut to a whole number, is L, L + R or R.

use crate::phase_inverter::read_versioned;
use crate::{Effect, StateError, knob_value};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Mute 2";

/// The Mute and Channel wheels' range and default (the DFM's `MuteWheel`
/// and `ChannelWheel`).
const RANGES: [(i32, i32); 2] = [(0, 1024), (0, 1024)];
const DEFAULTS: [i32; 2] = [512, 512];

/// A wheel's place from 0 to 1, as a single.
fn place(value: i32) -> f32 {
    (f64::from(value) / 1024.0).clamp(0.0, 1.0) as f32
}

/// Whether the Mute wheel mutes: its place under a half (`0x006ba850`).
pub fn muted(value: i32) -> bool {
    place(value) < 0.5
}

/// The side the Channel wheel mutes: 0 the left, 1 both, 2 the right. Its
/// place over the single nearest a third, cut toward 0, at most 2
/// (`0x006ba850`).
pub fn channel(value: i32) -> usize {
    ((place(value) / (1.0f32 / 3.0)) as usize).min(2)
}

/// Fruity Mute 2. A new one has FL's defaults, which mute nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Mute2 {
    params: [i32; 2],
}

impl Default for Mute2 {
    fn default() -> Self {
        Mute2 { params: DEFAULTS }
    }
}

impl Effect for Mute2 {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.params = read_versioned(bytes, &RANGES)?;
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// `Eff_Render` (`0x006baaf0`): unmuted, a copy. Muted, both sides set
    /// to 0, or one side times 0 and the other times 1 (`FUN_005df490`).
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !muted(self.params[0]) {
            return;
        }
        let gains = match channel(self.params[1]) {
            1 => {
                left.fill(0.0);
                right.fill(0.0);
                return;
            }
            0 => (0.0, 1.0),
            _ => (1.0, 0.0),
        };
        for sample in left.iter_mut() {
            *sample *= gains.0;
        }
        for sample in right.iter_mut() {
            *sample *= gains.1;
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(mute: i32, channel: i32) -> Vec<u8> {
        [1, mute, channel]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect()
    }

    fn run(mute: i32, channel: i32) -> ([f32; 2], [f32; 2]) {
        let mut plugin = Mute2::default();
        plugin.set_state(&state(mute, channel)).unwrap();
        let (mut left, mut right) = ([0.5, -0.25], [0.75, -1.0]);
        plugin.process(&mut left, &mut right);
        (left, right)
    }

    #[test]
    fn the_wheels_mute_below_half_and_pick_a_side_by_thirds() {
        assert!(!muted(512) && muted(511) && muted(0));
        assert_eq!(
            [0, 341, 342, 512, 682, 683, 1024].map(channel),
            [0, 0, 1, 1, 1, 2, 2]
        );
    }

    #[test]
    fn the_defaults_pass_and_a_mute_takes_its_side_out() {
        assert_eq!(run(512, 512), ([0.5, -0.25], [0.75, -1.0]));
        assert_eq!(run(0, 512), ([0.0, 0.0], [0.0, 0.0]));
        let (left, right) = run(0, 0);
        assert_eq!(
            (left.map(f32::to_bits), right),
            ([0.0f32, -0.0].map(f32::to_bits), [0.75, -1.0])
        );
        assert_eq!(run(0, 1024), ([0.5, -0.25], [0.0, -0.0]));
    }
}
