//! Control Surface, a panel of knobs, sliders and pads whose values drive
//! other controls through links: FL never asks it for sound, as
//! `docs/plugins/control-surface.md` reads it. [`crate::by_name`] hosts it
//! as a [`crate::pass_through::PassThrough`] that says its outputs are not
//! driven.

/// The plugin's event 201 name.
pub const NAME: &str = "Control Surface";

/// What the pass-through leaves out.
pub const LEFT_OUT: &str = "Control Surface's outputs are not driven";
