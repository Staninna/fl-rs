//! Razer Chroma, which lights a Razer keyboard from the sound: it reads the
//! block for its peaks and spectrum and passes it on untouched, as
//! `docs/plugins/razer-chroma.md` reads it. [`crate::by_name`] hosts it as
//! a [`crate::pass_through::PassThrough`].

/// The plugin's event 201 name.
pub const NAME: &str = "Razer Chroma";
