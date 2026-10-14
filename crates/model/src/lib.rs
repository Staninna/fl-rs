//! An FL Studio project as Rust types.
//!
//! [`from_flp`] reads a [`Project`] out of the event stream that the `flp`
//! crate parses, and [`to_flp`] turns it back into one. For a project nobody
//! changed in between, the events that come out are the events that went
//! in, one for one. `tests/corpus.rs` holds that against all 165 projects FL
//! Studio 26.1.7 ships.
//!
//! # How the round trip stays exact
//!
//! Every object ([`Project`], [`Channel`], [`Pattern`], [`Arrangement`],
//! [`Track`], [`TimeMarker`], [`Insert`], [`Slot`], [`ControllerLink`]) has
//! plain typed fields and one more field, `order: Vec<Placed>`. `order`
//! lists every event the object owns. Each entry is a stream position and
//! an [`Item`]:
//!
//! - [`Item::Field`] names an event ID whose payload a typed field holds.
//! - [`Item::Raw`] is an event no field took, kept whole.
//!
//! [`to_flp`] replays every object's `order`, encodes the `Field` entries
//! from the fields as they are now, and sorts the lot by position.
//!
//! I chose this over the two obvious alternatives. A list of raw extras
//! beside the fields would need a fixed order to write the fields in, and
//! FL has changed that order between versions. One ordered list of typed
//! items would be exact too, but then `channel.name` becomes a search. With
//! tags in the list and values in the fields, reading a field is a field
//! access, changing one changes what gets written, and removing a channel
//! from [`Project::channels`] removes its events.
//!
//! The position is global because objects interleave. A pattern is selected
//! twice in most files, an insert's slots and an arrangement's tracks sit
//! inside its run of events, and a 201 arrives before the 212 that says
//! which object it belongs to.
//!
//! A field takes an event only when encoding the decoded value gives the
//! same bytes back. A bool stored as 2, a string with half an emoji in it
//! (one corpus project has that in its comments) or a struct of a size the
//! dictionary does not allow stays raw, and the field stays `None`.
//!
//! When a file repeats an event for an `Option` field, the last one wins,
//! as it does in FL's loader, and the earlier entry turns raw. Thirteen
//! channels in FL 11 projects name themselves twice this way, with a 203
//! and then a 192. Two things are read once only: a record list such as a
//! pattern's notes, and the 199 version, because the first 199 decides how
//! every string after it is encoded.
//!
//! # Changing a project
//!
//! This first version is built for reading. A changed field is written back
//! at the place its event had. A field that the file had no event for has
//! no entry in `order`, so setting it writes nothing until you push a
//! `Placed { pos, item: Item::Field(id) }` for it. Setting a field to `None`
//! drops its event.
//!
//! # Units
//!
//! Values are what FL stores: tempo in thousandths of a BPM, volume as the
//! u32 from the levels struct, colours as RGBA u32. Each field's doc gives
//! the event ID and the payload as `docs/event-dictionary.md` describes it.
//! Structs with a byte layout are the ones from [`flp::event::layout`].
//! A pattern's notes and an arrangement's playlist items are such structs
//! too, [`Note`] and [`PlaylistItem`], with their fields named and methods
//! for what takes more than one field to say: an item's [`ItemSource`], its
//! [`Trim`], its track and its [`ClipSettings`]. An automation clip's points
//! are on its channel, and [`Project::automation_bindings`] says which
//! controls each clip drives.
//!
//! # Which object an event goes to
//!
//! [`from_flp`] walks the stream the way FL's loader does, following the
//! "Object delimiters" section of the dictionary: 64 opens a channel, 65
//! selects a pattern, 99 an arrangement and 238 a track, 147 closes an
//! insert, 205 closes a time marker, 212 opens a plugin, 226 and 227 make a
//! controller link. Any other event goes to the current object for its
//! [`flp::event::Scope`], or to the project when there is none.

mod arrangement;
mod automation;
mod channel;
mod codec;
mod mixer;
mod mixer_edit;
mod pattern;
mod project;
mod read;
pub mod rec;

pub use arrangement::{Arrangement, TimeMarker, Track};
pub use automation::Binding;
pub use channel::{Channel, ChannelKind};
pub use codec::{Item, Placed};
pub use flp::event::layout::{ClipSettings, ItemSource, Note, PlaylistItem, Trim};
pub use mixer::{EqBand, Insert, InsertMix, Slot};
pub use pattern::{NoteField, Pattern};
pub use project::{ControllerLink, LinkKind, Project, to_flp};
pub use read::from_flp;
pub use rec::RecTarget;

use thiserror::Error;

/// Why [`to_flp`] could not write a project.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// A string has a character the file's 8-bit strings cannot hold, or
    /// the project has no writer version to pick a string encoding from.
    #[error("the value for event {id} does not fit the file's string encoding")]
    Unencodable { id: u32 },
}
