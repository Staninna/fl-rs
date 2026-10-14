//! How a typed field and an event payload turn into each other, and the
//! per-object list that remembers where each event sat.

use crate::Error;
use flp::RawEvent;
use flp::event::{self, Id, layout};
use std::collections::HashMap;

/// One event of an object and where it sat in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// Index of the event in the stream. [`to_flp`](crate::to_flp) sorts
    /// every object's events by it, so the numbers only have to be ordered,
    /// not dense.
    pub pos: usize,
    pub item: Item,
}

/// What [`to_flp`](crate::to_flp) writes at one position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// The event with this ID, written from the typed field that read it.
    /// For a `Vec` field with one event per element, the nth entry writes
    /// the nth element. An entry whose field is `None`, or whose element is
    /// gone, writes nothing.
    Field(Id),
    /// An event no field took, written back as it was.
    Raw(RawEvent),
}

/// What a payload's meaning depends on besides its own bytes.
#[derive(Debug, Default)]
pub(crate) struct Cx {
    /// Whether strings are UTF-16. `None` until the stream has passed its
    /// 199, and for a project that has no writer version.
    pub utf16: Option<bool>,
    /// The writer version as FL's loader compares it, 0 before the 199.
    pub version: u32,
    /// The value of the last 169 `DataFormatFlags`, 0 before it.
    pub data_format_flags: u32,
}

impl Cx {
    /// Take the string encoding and the version gates from a 199 string.
    /// Text that is not a version counts as version 0.
    pub fn set_version(&mut self, text: &str) {
        self.version = event::version_number(text).unwrap_or(0);
        self.utf16 = Some(event::strings_are_utf16(self.version));
    }
}

/// A field type that one event payload decodes into.
pub(crate) trait Codec: Sized {
    fn decode(e: &RawEvent, cx: &Cx) -> Option<Self>;
    /// `None` when the file cannot hold the value.
    fn encode(&self, id: Id, cx: &Cx) -> Option<Vec<u8>>;
}

/// Decode a payload, but only when encoding the result gives the same bytes
/// back. Anything else stays a raw event, which is what keeps the round trip
/// exact: a bool stored as 2 or a string with a broken surrogate never
/// reaches a typed field.
fn exact<T: Codec>(e: &RawEvent, cx: &Cx) -> Option<T> {
    let value = T::decode(e, cx)?;
    (value.encode(Id::from(e.id), cx)? == e.data).then_some(value)
}

macro_rules! numbers {
    ($($ty:ty),*) => {$(
        impl Codec for $ty {
            fn decode(e: &RawEvent, _: &Cx) -> Option<Self> {
                Some(<$ty>::from_le_bytes(e.data.as_slice().try_into().ok()?))
            }
            fn encode(&self, _: Id, _: &Cx) -> Option<Vec<u8>> {
                Some(self.to_le_bytes().to_vec())
            }
        }
    )*};
}

numbers!(u8, i8, u16, i16, u32, i32);

impl Codec for bool {
    fn decode(e: &RawEvent, _: &Cx) -> Option<Self> {
        match e.data[..] {
            [0] => Some(false),
            [1] => Some(true),
            _ => None,
        }
    }
    fn encode(&self, _: Id, _: &Cx) -> Option<Vec<u8>> {
        Some(vec![u8::from(*self)])
    }
}

/// 199 is 8-bit in every file. Every other string waits for the 199 to say.
fn utf16(id: Id, cx: &Cx) -> Option<bool> {
    if id == Id::FLVersion {
        Some(false)
    } else {
        cx.utf16
    }
}

impl Codec for String {
    fn decode(e: &RawEvent, cx: &Cx) -> Option<Self> {
        event::text(e, utf16(Id::from(e.id), cx)?)
    }
    fn encode(&self, id: Id, cx: &Cx) -> Option<Vec<u8>> {
        let units: Vec<u16> = self.encode_utf16().collect();
        event::text_bytes(id, &units, utf16(id, cx)?)
    }
}

/// A payload the dictionary gives no layout for.
impl Codec for Vec<u8> {
    fn decode(e: &RawEvent, _: &Cx) -> Option<Self> {
        Some(e.data.clone())
    }
    fn encode(&self, _: Id, _: &Cx) -> Option<Vec<u8>> {
        Some(self.clone())
    }
}

macro_rules! structs {
    ($($ty:ident),*) => {$(
        impl Codec for layout::$ty {
            fn decode(e: &RawEvent, _: &Cx) -> Option<Self> {
                Self::from_bytes(&e.data)
            }
            fn encode(&self, _: Id, _: &Cx) -> Option<Vec<u8>> {
                Some(self.to_bytes())
            }
        }
    )*};
}

structs!(
    AutomationData,
    ControllerLink,
    Delay,
    EnvelopeLfo,
    Filter,
    InsertFlags,
    LevelAdjusts,
    Levels,
    Parameters,
    PluginWrapper,
    Polyphony,
    Track,
    Tracking
);

macro_rules! records {
    ($($ty:ident),*) => {$(
        impl Codec for Vec<layout::$ty> {
            fn decode(e: &RawEvent, _: &Cx) -> Option<Self> {
                layout::$ty::list_from_bytes(&e.data)
            }
            fn encode(&self, _: Id, _: &Cx) -> Option<Vec<u8>> {
                Some(layout::$ty::list_to_bytes(self))
            }
        }
    )*};
}

records!(ControllerEvent, InitialValue, Note);

impl Codec for Vec<layout::PlaylistItem> {
    /// The item size comes from the writer version and 169.
    fn decode(e: &RawEvent, cx: &Cx) -> Option<Self> {
        layout::PlaylistItem::list_from_bytes(&e.data, cx.version, cx.data_format_flags)
    }
    fn encode(&self, _: Id, _: &Cx) -> Option<Vec<u8>> {
        Some(layout::PlaylistItem::list_to_bytes(self))
    }
}

/// `one`: an `Option` field. FL's loader lets the last event for a field
/// win, so when the field is already set, the entry that filled it turns
/// into a raw event holding the old value and the new value takes over.
pub(crate) fn take_one<T: Codec>(
    field: &mut Option<T>,
    order: &mut [Placed],
    ids: &[Id],
    e: &RawEvent,
    cx: &Cx,
) -> bool {
    let Some(value) = exact(e, cx) else {
        return false;
    };
    if let Some(old) = field {
        let earlier = order.iter_mut().rev().find_map(|placed| match placed.item {
            Item::Field(id) if ids.contains(&id) => Some((placed, id)),
            _ => None,
        });
        let Some((placed, id)) = earlier else {
            return false;
        };
        let Some(data) = old.encode(id, cx) else {
            return false;
        };
        placed.item = Item::Raw(RawEvent { id: id.0, data });
    }
    *field = Some(value);
    true
}

/// `each`: a `Vec` field with one event per element.
pub(crate) fn take_each<T: Codec>(field: &mut Vec<T>, e: &RawEvent, cx: &Cx) -> bool {
    let value = exact(e, cx);
    let taken = value.is_some();
    field.extend(value);
    taken
}

/// `list`: a field that is always there, read from the first event for it.
/// A second one stays raw: the dictionary does not say whether the loader
/// would append its records or replace the first lot.
pub(crate) fn take_list<T: Codec>(field: &mut T, e: &RawEvent, cx: &Cx) -> bool {
    match exact(e, cx) {
        Some(value) => {
            *field = value;
            true
        }
        None => false,
    }
}

/// `key`: the index an object is addressed by. The event that selects the
/// object carries it, so there is nothing to store.
pub(crate) fn take_key<T: Codec + PartialEq>(field: &T, e: &RawEvent, cx: &Cx) -> bool {
    exact::<T>(e, cx).as_ref() == Some(field)
}

pub(crate) fn put<T: Codec>(value: Option<&T>, id: Id, cx: &Cx) -> Result<Option<Vec<u8>>, Error> {
    match value {
        None => Ok(None),
        Some(value) => match value.encode(id, cx) {
            Some(data) => Ok(Some(data)),
            None => Err(Error::Unencodable { id: id.0 }),
        },
    }
}

/// A model type that owns events. `object!` writes the impl from the type's
/// field table.
pub(crate) trait Object {
    /// Offer an event to the typed fields. True when one of them took it.
    fn take(&mut self, e: &RawEvent, cx: &Cx) -> bool;
    /// The payload for the `n`th `Item::Field(id)` of this object, or `None`
    /// when the field holds nothing to write.
    fn payload(&self, id: Id, n: usize, cx: &Cx) -> Result<Option<Vec<u8>>, Error>;
    fn order(&self) -> &[Placed];
    fn order_mut(&mut self) -> &mut Vec<Placed>;
}

/// Hand an event to an object: into a typed field when one takes it, raw
/// otherwise.
pub(crate) fn give<T: Object>(object: &mut T, pos: usize, e: &RawEvent, cx: &Cx) {
    let item = if object.take(e, cx) {
        Item::Field(Id::from(e.id))
    } else {
        Item::Raw(e.clone())
    };
    object.order_mut().push(Placed { pos, item });
}

/// Turn an object back into its events, each with its position.
pub(crate) fn emit<T: Object>(
    object: &T,
    cx: &Cx,
    out: &mut Vec<(usize, RawEvent)>,
) -> Result<(), Error> {
    let mut seen: HashMap<Id, usize> = HashMap::new();
    for placed in object.order() {
        match &placed.item {
            Item::Raw(e) => out.push((placed.pos, e.clone())),
            Item::Field(id) => {
                let n = seen.entry(*id).or_default();
                if let Some(data) = object.payload(*id, *n, cx)? {
                    out.push((placed.pos, RawEvent { id: id.0, data }));
                }
                *n += 1;
            }
        }
    }
    Ok(())
}

/// The field table of a model type: which events its fields are read from.
///
/// Each row is `how field = Id;` or `how field = Id | Id;`. `how` is `one`
/// for an `Option`, `each` for a `Vec` with one event per element, `list`
/// for a field read whole from one event, and `key` for the index the
/// opening event carries. With two IDs, the `Item::Field` entry remembers
/// which one the file used.
macro_rules! object {
    ($ty:ident { $($how:ident $field:ident = $($id:ident)|+;)* }) => {
        impl $ty {
            /// The event IDs the typed fields are read from. Every other
            /// event of the object stays an [`Item::Raw`](crate::Item::Raw).
            pub const IDS: &'static [flp::event::Id] = &[$($(flp::event::Id::$id,)+)*];
        }

        impl $crate::codec::Object for $ty {
            fn take(&mut self, e: &flp::RawEvent, cx: &$crate::codec::Cx) -> bool {
                let id = flp::event::Id::from(e.id);
                $(if $(id == flp::event::Id::$id)||+ {
                    return object!(@take $how self, $field, id, [$(flp::event::Id::$id),+], e, cx);
                })*
                false
            }

            fn payload(
                &self,
                id: flp::event::Id,
                n: usize,
                cx: &$crate::codec::Cx,
            ) -> Result<Option<Vec<u8>>, $crate::Error> {
                $(if $(id == flp::event::Id::$id)||+ {
                    return $crate::codec::put(object!(@get $how self, $field, n), id, cx);
                })*
                let _ = n;
                Ok(None)
            }

            fn order(&self) -> &[$crate::Placed] {
                &self.order
            }

            fn order_mut(&mut self) -> &mut Vec<$crate::Placed> {
                &mut self.order
            }
        }
    };
    (@take one $s:ident, $f:ident, $id:ident, $ids:expr, $e:ident, $cx:ident) => {
        $crate::codec::take_one(&mut $s.$f, &mut $s.order, &$ids, $e, $cx)
    };
    (@take each $s:ident, $f:ident, $id:ident, $ids:expr, $e:ident, $cx:ident) => {
        $crate::codec::take_each(&mut $s.$f, $e, $cx)
    };
    (@take list $s:ident, $f:ident, $id:ident, $ids:expr, $e:ident, $cx:ident) => {{
        let item = $crate::Item::Field($id);
        !$s.order.iter().any(|placed| placed.item == item)
            && $crate::codec::take_list(&mut $s.$f, $e, $cx)
    }};
    (@take key $s:ident, $f:ident, $id:ident, $ids:expr, $e:ident, $cx:ident) => {
        $crate::codec::take_key(&$s.$f, $e, $cx)
    };
    (@get one $s:ident, $f:ident, $n:ident) => {
        $s.$f.as_ref()
    };
    (@get each $s:ident, $f:ident, $n:ident) => {
        $s.$f.get($n)
    };
    (@get list $s:ident, $f:ident, $n:ident) => {
        Some(&$s.$f)
    };
    (@get key $s:ident, $f:ident, $n:ident) => {
        Some(&$s.$f)
    };
}

pub(crate) use object;
