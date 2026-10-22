//! The effects that leave the sound as it is: FL's meters, visualisers
//! and notebooks, as `docs/plugins/` reads each one's `Eff_Render` and
//! plugin info. Each has a module of its own for its name,
//! [`crate::wave_candy`] and the rest, and all of them are a
//! [`PassThrough`]. The controllers that leave the sound alone too,
//! [`crate::control_surface`] and [`crate::xy_controller`], have types of
//! their own for their outputs.
//!
//! Two ways to leave the sound alone are in FL's DLLs. A notebook or a
//! controller sets `FPF_NoProcess` (0x40) in its `TFruityPlugInfo` flags,
//! which tells FL never to call its `Eff_Render`, and that slot of its
//! method table is Delphi's abstract-method error. A meter or a visualiser
//! reads the block for its display and copies it from the source buffer to
//! the destination with IPP's `ippsCopy_64s`, one stereo frame a 64-bit
//! element, when the two are not the same buffer.

use crate::{Effect, StateError};

/// An effect whose output is its input. Its state is the plugin's own and
/// says nothing about the sound, so any state is taken. It has no
/// parameters a host can move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassThrough;

impl PassThrough {
    /// A plugin that does nothing beside the sound that a host plays.
    pub const fn new() -> Self {
        PassThrough
    }
}

impl Default for PassThrough {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for PassThrough {
    fn set_state(&mut self, _bytes: &[u8]) -> Result<(), StateError> {
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: f32) {}

    /// The block stays as it came.
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}

    fn param(&self, _index: usize) -> f32 {
        0.0
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use crate::testing::bits;

    #[test]
    fn the_block_comes_out_as_it_went_in_to_the_bit() {
        let input = [0.0, -0.0, 1.0, -1.0, 0.3, -1e-30, 7.5, f32::NAN];
        for name in [
            crate::wave_candy::NAME,
            crate::notebook::NAME,
            crate::notebook_2::NAME,
            crate::html_notebook::NAME,
            crate::db_meter::NAME,
            crate::razer_chroma::NAME,
        ] {
            let mut plugin = crate::by_name(name).unwrap();
            plugin.set_state(&[1, 2, 3]).unwrap();
            let (mut left, mut right) = (input, input);
            right.reverse();
            plugin.process(&mut left, &mut right);
            assert_eq!(bits(&left), bits(&input), "{name}");
            right.reverse();
            assert_eq!(bits(&right), bits(&input), "{name}");
            assert_eq!(plugin.param_count(), 0);
            assert_eq!(plugin.latency(), 0);
        }
    }
}
