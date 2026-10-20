//! Fruity Phase Inverter: turns the left side or the right side upside
//! down, as `docs/plugins/fruity-phase-inverter.md` has it.
//!
//! The state is a version word and the one parameter, the State wheel, 0
//! to 1024. The wheel's place, a third of its turn at a time, is Off, Left
//! or Right: `Round(v / 1024 * 3 - 0.5)` in singles, rounding half to even,
//! so the default 1024 is Right.

use crate::{Effect, StateError, knob_value};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Phase Inverter";

/// The State wheel's range and default (the DFM's `StateWheel`).
const RANGE: (i32, i32) = (0, 1024);
const DEFAULT: i32 = 1024;

/// What the wheel turns upside down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Off,
    Left,
    Right,
}

/// The side for a State value, as `ProcessParam` (`0x006ba520`) and
/// `0x006ba720` work it out: the wheel's place as a single from 0 to 1,
/// times 3 less a half, rounded.
pub fn side(value: i32) -> Side {
    let place = (f64::from(value - RANGE.0) / f64::from(RANGE.1 - RANGE.0)).clamp(0.0, 1.0);
    match f64::from(place as f32 * 3.0 - 0.5).round_ties_even() as i32 {
        ..=0 => Side::Off,
        1 => Side::Left,
        _ => Side::Right,
    }
}

/// Read a state of a version word under 2 and then one i32 a parameter, as
/// these small plugins' `SaveRestoreState` does. Each value is kept in its
/// range.
pub(crate) fn read_versioned<const N: usize>(
    bytes: &[u8],
    ranges: &[(i32, i32); N],
) -> Result<[i32; N], StateError> {
    let expected = 4 + 4 * N;
    if bytes.len() < expected {
        return Err(StateError::Length {
            expected,
            found: bytes.len(),
        });
    }
    let word = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default());
    let version = word(0);
    if version >= 2 {
        return Err(StateError::Version { found: version });
    }
    Ok(std::array::from_fn(|i| {
        word(4 + 4 * i).clamp(ranges[i].0, ranges[i].1)
    }))
}

/// Fruity Phase Inverter. A new one has FL's default, which turns the
/// right side over.
#[derive(Debug, Clone, PartialEq)]
pub struct PhaseInverter {
    value: i32,
    side: Side,
}

impl Default for PhaseInverter {
    fn default() -> Self {
        PhaseInverter {
            value: DEFAULT,
            side: side(DEFAULT),
        }
    }
}

impl Effect for PhaseInverter {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let [value] = read_versioned(bytes, &[RANGE])?;
        self.set_param(0, value as f32);
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// `Eff_Render` (`0x006ba930`) copies the block and flips the sign bit
    /// of every sample of the side.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let flipped = match self.side {
            Side::Off => return,
            Side::Left => left,
            Side::Right => right,
        };
        for sample in flipped {
            *sample = -*sample;
        }
    }

    fn param_count(&self) -> usize {
        1
    }

    fn param(&self, index: usize) -> f32 {
        if index == 0 { self.value as f32 } else { 0.0 }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.value = knob_value(value, RANGE);
            self.side = side(self.value);
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wheel_is_off_left_and_right_by_thirds() {
        assert_eq!(side(0), Side::Off);
        // 341 / 1024 * 3 - 0.5 = 0.499 rounds to 0, and 342 makes 0.502.
        assert_eq!(side(341), Side::Off);
        assert_eq!(side(342), Side::Left);
        assert_eq!(side(682), Side::Left);
        assert_eq!(side(683), Side::Right);
        // 2.5 rounds to the even 2.
        assert_eq!(side(1024), Side::Right);
    }

    #[test]
    fn the_side_comes_out_upside_down_to_the_bit() {
        let mut plugin = PhaseInverter::default();
        let (mut left, mut right) = ([0.5, -0.25, 0.0], [0.5, -0.25, 0.0]);
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [0.5, -0.25, 0.0]);
        assert_eq!(
            right.map(f32::to_bits),
            [-0.5f32, 0.25, -0.0].map(f32::to_bits)
        );
        plugin.set_state(&[1, 0, 0, 0, 0, 2, 0, 0]).unwrap();
        plugin.process(&mut left, &mut right);
        assert_eq!(left, [-0.5, 0.25, -0.0]);
    }

    #[test]
    fn a_state_is_a_version_and_the_value() {
        let mut plugin = PhaseInverter::default();
        assert!(plugin.set_state(&[1, 0, 0, 0]).is_err());
        assert!(plugin.set_state(&[2, 0, 0, 0, 0, 0, 0, 0]).is_err());
        plugin.set_state(&[1, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(plugin.side, Side::Off);
    }
}
