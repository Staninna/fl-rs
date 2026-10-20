//! The effects that leave the sound as it is: FL's meters, visualisers,
//! notebooks and controllers, as `docs/plugins/` reads each one's
//! `Eff_Render` and plugin info. Each has a module of its own for its name,
//! [`crate::wave_candy`] and the rest, and all of them are a
//! [`PassThrough`].
//!
//! Two ways to leave the sound alone are in FL's DLLs. A notebook or a
//! controller sets `FPF_NoProcess` (0x40) in its `TFruityPlugInfo` flags,
//! which tells FL never to call its `Eff_Render`, and that slot of its
//! method table is Delphi's abstract-method error. A meter or a visualiser
//! reads the block for its display and copies it from the source buffer to
//! the destination with IPP's `ippsCopy_64s`, one stereo frame a 64-bit
//! element, when the two are not the same buffer.

use crate::{Effect, Fidelity, StateError};

/// An effect whose output is its input. Its state is the plugin's own and
/// says nothing about the sound, so any state is taken. It has no
/// parameters a host can move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassThrough {
    /// What the plugin does beside the sound that this leaves out, said in
    /// [`Effect::fidelity`], for a controller whose outputs drive links.
    left_out: Option<&'static str>,
}

impl PassThrough {
    /// A plugin that does nothing beside the sound that a host plays.
    pub const fn new() -> Self {
        PassThrough { left_out: None }
    }

    /// A controller: the sound passes, but `what` it puts out is not
    /// driven, which [`Effect::fidelity`] says.
    pub const fn controller(what: &'static str) -> Self {
        PassThrough {
            left_out: Some(what),
        }
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

    fn fidelity(&self) -> Fidelity {
        self.left_out.map_or(Fidelity::Exact, Fidelity::Approximate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            crate::control_surface::NAME,
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

    #[test]
    fn only_the_controllers_say_what_they_leave_out() {
        assert_eq!(PassThrough::new().fidelity(), Fidelity::Exact);
        let exact = |name| crate::by_name(name).unwrap().fidelity() == Fidelity::Exact;
        assert!(exact(crate::wave_candy::NAME));
        assert!(exact(crate::db_meter::NAME));
        let controller = PassThrough::controller("its outputs");
        assert_eq!(controller.fidelity(), Fidelity::Approximate("its outputs"));
        // Control Surface's links are left out, and it says so.
        let said = crate::control_surface::LEFT_OUT;
        let fidelity = crate::by_name(crate::control_surface::NAME)
            .unwrap()
            .fidelity();
        assert_eq!(fidelity, Fidelity::Approximate(said));
    }
}
