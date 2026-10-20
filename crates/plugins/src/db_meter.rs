//! Fruity dB Meter: it keeps the peak of each side for its display and
//! passes the block on untouched, as `docs/plugins/fruity-db-meter.md`
//! reads it. [`crate::by_name`] hosts it as a
//! [`crate::pass_through::PassThrough`].

/// The plugin's event 201 name. FL's own info record spells it "Fruity dB
/// meter"; a name is found in any case.
pub const NAME: &str = "Fruity dB Meter";
