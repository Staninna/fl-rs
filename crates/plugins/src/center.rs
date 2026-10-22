//! Fruity Center: takes a standing offset out of each side, as
//! `docs/plugins/fruity-center.md` has it.
//!
//! The state is a version word and the one parameter, the switch, on (1)
//! by default. On, each side runs through a follower of its slow movement
//! (`FUN_005e0ca0`, in doubles):
//!
//! ```text
//! v += (x - y) * a;  y += v;  v *= 0.96;  out = x - y
//! ```
//!
//! with `a = 4.567e-6 * 44100 / rate` as a single, a fifth of the pace of
//! WaveShaper's Center. After each block every part of the state under
//! 2^-24 becomes 0.

use crate::parts::Follower;
use crate::phase_inverter::read_versioned;
use crate::{Effect, StateError, knob_value};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Center";

/// The switch's range and default.
const RANGE: (i32, i32) = (0, 1);
const DEFAULT: i32 = 1;

/// What `0x006a2bb0` puts the host's `44100 / rate` through for the pace,
/// and the speed kept (`0x3f75c28f`).
const PACE: f64 = 4.567e-6;
const KEEP: f32 = 0.96;

/// Under this a part of the state becomes 0 after a block (`DAT_006e2d5c`).
const FLOOR: f32 = 5.960_464_5e-8;

/// Fruity Center. A new one has FL's defaults: switched on, at 44.1 kHz.
#[derive(Debug, Clone, PartialEq)]
pub struct Center {
    on: i32,
    pace: f64,
    sides: [Follower; 2],
}

impl Default for Center {
    fn default() -> Self {
        let mut center = Center {
            on: DEFAULT,
            pace: 0.0,
            sides: [Follower::default(); 2],
        };
        center.set_sample_rate(44_100.0);
        center
    }
}

impl Effect for Center {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let [on] = read_versioned(bytes, &[RANGE])?;
        self.on = on;
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.pace = f64::from((44_100.0 / f64::from(hz) * PACE) as f32);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.on == 0 {
            return;
        }
        let keep = f64::from(KEEP);
        for (side, samples) in self.sides.iter_mut().zip([left, right]) {
            for sample in samples.iter_mut() {
                let x = f64::from(*sample);
                *sample = (x - side.step(x, self.pace, keep)) as f32;
            }
            side.clear_below(f64::from(FLOOR));
        }
    }

    fn param_count(&self) -> usize {
        1
    }

    fn param(&self, index: usize) -> f32 {
        if index == 0 { self.on as f32 } else { 0.0 }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.on = knob_value(value, RANGE);
        }
    }

    /// The dispatcher's reset (`0x006a3140`) empties the followers.
    fn reset(&mut self) {
        self.sides = [Follower::default(); 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standing_offset_dies_away_and_a_tone_passes() {
        let mut center = Center::default();
        let mut left = vec![0.5f32; 44_100 * 4];
        let mut right: Vec<f32> = (0..left.len())
            .map(|n| (n as f32 * 0.05).sin() * 0.5)
            .collect();
        let tone = right.clone();
        center.process(&mut left, &mut right);
        // The follower closes in on the offset over seconds.
        assert!(
            left[0] > 0.49 && left.last().unwrap().abs() < 0.01,
            "{}",
            left.last().unwrap()
        );
        let off = right[44_100..]
            .iter()
            .zip(&tone[44_100..])
            .map(|(a, b)| (a - b).abs());
        assert!(off.fold(0.0, f32::max) < 0.01);
    }

    #[test]
    fn switched_off_it_passes_to_the_bit() {
        let mut center = Center::default();
        center.set_state(&[1, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        let (mut left, mut right) = ([0.3f32, -0.7], [1e-30f32, 2.0]);
        center.process(&mut left, &mut right);
        assert_eq!((left, right), ([0.3, -0.7], [1e-30, 2.0]));
    }

    #[test]
    fn the_state_s_crumbs_are_cleared_after_a_block() {
        let mut center = Center::default();
        let (mut left, mut right) = ([1e-9f32; 4], [0.0f32; 4]);
        center.process(&mut left, &mut right);
        assert_eq!(center.sides[0], Follower::default());
    }
}
