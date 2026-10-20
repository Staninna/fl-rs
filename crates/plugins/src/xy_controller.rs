//! Fruity X-Y Controller, a pad whose two values drive other controls
//! through links: FL never asks it for sound, as
//! `docs/plugins/fruity-x-y-controller.md` reads it. [`crate::by_name`]
//! hosts it as a [`crate::pass_through::PassThrough`] that says its outputs
//! are not driven.

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity X-Y Controller";

/// What the pass-through leaves out.
pub const LEFT_OUT: &str = "Fruity X-Y Controller's outputs are not driven";
