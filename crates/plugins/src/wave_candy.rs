//! Wave Candy, FL's scope, spectrum and meter: it reads each block into a
//! ring for its display and passes the block on untouched, as
//! `docs/plugins/wave-candy.md` reads it. [`crate::by_name`] hosts it as a
//! [`crate::pass_through::PassThrough`].

/// The plugin's event 201 name.
pub const NAME: &str = "Wave Candy";
