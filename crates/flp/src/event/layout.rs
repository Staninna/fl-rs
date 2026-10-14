//! Var payloads with a byte layout: one struct per dictionary row that gives
//! a fixed size.
//!
//! Every struct has `from_bytes`, which returns `None` for a size the row
//! does not allow, and `to_bytes`, which gives back the bytes it read.
//! Integers are little-endian.
//!
//! A field has a name only where `docs/event-dictionary.md` names that
//! offset, or `docs/records-224-233.md` for [`Note`] and [`PlaylistItem`].
//! The rest are `unknown_<offset>` byte arrays, split where the dictionary
//! says something about an offset without naming it, and the field's doc
//! comment repeats what it says. For most rows the dictionary gives a size
//! and little else, so most bytes here are unknown.
//!
//! Three kinds of size rule, each taken from the row's payload column:
//!
//! - One size (`20-byte struct`): [`Delay`], [`Polyphony`] and most others.
//! - A list of sizes or an upper bound (`struct <= 168`, `5 or 8 in old
//!   files`), or a loader branch on a short payload: [`PluginWrapper`],
//!   [`Parameters`], [`EnvelopeLfo`], [`Levels`], [`Filter`],
//!   [`PlaylistItem`], [`InsertFlags`], [`Track`]. These carry a `len` field with the size that was read. Fields
//!   past `len` read as zero and `to_bytes` leaves them out.
//! - Records back to back (`12-byte records`): [`ControllerEvent`], [`Note`],
//!   [`InitialValue`], [`PlaylistItem`], through `list_from_bytes` and
//!   `list_to_bytes`.
//!
//! [`AutomationData`] is the one payload with counts inside it: two blocks
//! of [`Point`] records, each with its own count, as
//! `docs/automation-234.md` lays them out.

trait Field: Sized {
    const SIZE: usize;
    /// Read one value off the front of `b`, which holds at least `SIZE` bytes.
    fn take(b: &mut &[u8]) -> Self;
    fn put(&self, out: &mut Vec<u8>);
}

macro_rules! number_fields {
    ($($ty:ty),*) => {$(
        impl Field for $ty {
            const SIZE: usize = size_of::<$ty>();
            fn take(b: &mut &[u8]) -> Self {
                let (head, rest) = b.split_at(Self::SIZE);
                *b = rest;
                <$ty>::from_le_bytes(head.try_into().unwrap())
            }
            fn put(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
        }
    )*};
}

number_fields!(u8, i8, u16, i16, u32, i32, u64, f32, f64);

impl<const N: usize> Field for [u8; N] {
    const SIZE: usize = N;
    fn take(b: &mut &[u8]) -> Self {
        let (head, rest) = b.split_at(N);
        *b = rest;
        head.try_into().unwrap()
    }
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self);
    }
}

macro_rules! layout {
    // One size.
    (
        $(#[$meta:meta])*
        $name:ident[$size:literal] {
            $($(#[$fmeta:meta])* $field:ident: $ty:ty,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct $name {
            $($(#[$fmeta])* pub $field: $ty,)*
        }

        impl $name {
            /// Payload size in bytes.
            pub const SIZE: usize = $size;

            /// Decode a payload of exactly [`Self::SIZE`] bytes.
            pub fn from_bytes(mut b: &[u8]) -> Option<Self> {
                if b.len() != Self::SIZE {
                    return None;
                }
                Some(Self {
                    $($field: Field::take(&mut b),)*
                })
            }

            /// The payload bytes, equal to what [`Self::from_bytes`] read.
            pub fn to_bytes(&self) -> Vec<u8> {
                let mut out = Vec::with_capacity(Self::SIZE);
                $(self.$field.put(&mut out);)*
                out
            }
        }

        const _: () = assert!(0 $(+ <$ty as Field>::SIZE)* == $size);
    };
    // Several sizes: the payload is a prefix of the full layout.
    (
        $(#[$meta:meta])*
        $name:ident[$size:literal, len $accept:pat] {
            $($(#[$fmeta:meta])* $field:ident: $ty:ty,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct $name {
            $($(#[$fmeta])* pub $field: $ty,)*
            /// Payload size that was read. Fields past it are zero, and
            /// [`Self::to_bytes`] writes only this many bytes.
            pub len: usize,
        }

        impl $name {
            /// Size of the full layout in bytes.
            pub const SIZE: usize = $size;

            /// Decode a payload of one of the sizes the dictionary allows.
            pub fn from_bytes(b: &[u8]) -> Option<Self> {
                if !matches!(b.len(), $accept) {
                    return None;
                }
                let mut full = [0u8; $size];
                full.get_mut(..b.len())?.copy_from_slice(b);
                let mut rest = &full[..];
                Some(Self {
                    $($field: Field::take(&mut rest),)*
                    len: b.len(),
                })
            }

            /// The first `len` payload bytes, equal to what
            /// [`Self::from_bytes`] read.
            pub fn to_bytes(&self) -> Vec<u8> {
                let mut out = Vec::with_capacity(Self::SIZE);
                $(self.$field.put(&mut out);)*
                out.truncate(self.len);
                out
            }
        }

        const _: () = assert!(0 $(+ <$ty as Field>::SIZE)* == $size);
    };
}

macro_rules! records {
    ($($name:ident),*) => {$(
        impl $name {
            /// Decode a payload of whole records back to back. `None` when
            /// the length is not a multiple of [`Self::SIZE`].
            pub fn list_from_bytes(b: &[u8]) -> Option<Vec<Self>> {
                if !b.len().is_multiple_of(Self::SIZE) {
                    return None;
                }
                b.chunks_exact(Self::SIZE).map(Self::from_bytes).collect()
            }

            /// The payload for a list of records.
            pub fn list_to_bytes(items: &[Self]) -> Vec<u8> {
                items.iter().flat_map(Self::to_bytes).collect()
            }
        }
    )*};
}

layout! {
    /// 209 `ChannelID.Delay`: 20-byte struct.
    Delay[20] {
        /// The loader multiplies it by 100 for writers below 5.3.7.
        unknown_0: [u8; 4],
        /// -6400..6400 in the file; the loader stores it centred at 6400.
        /// Multiplied by 100 for writers below 5.3.7.
        pan: i32,
        /// Rewritten as `(v - 120) * 10` for writers below 3.3.1.
        unknown_8: [u8; 4],
        unknown_12: [u8; 8],
    }
}

layout! {
    /// 212 `PluginID.Wrapper`: 52-byte struct. The loader takes anything up
    /// to 52 bytes and assumes `flags == 1` under 21, so shorter payloads
    /// decode too.
    PluginWrapper[52, len 0..=52] {
        /// Insert index, before the loader's per-version remap.
        insert: i32,
        slot: i32,
        /// How the plugin's 213 is read: 0 a parameter array, 1 or 2 a chunk.
        plugin_kind: i32,
        /// Read together with `plugin_kind` as one u64 by writers below 2.5.2.
        unknown_12: [u8; 4],
        /// Bit 4: generator on the current channel, otherwise an effect in
        /// `insert`/`slot`. The loader also reads bits 0, 2, 3, 5 to 9, 16
        /// and 17.
        flags: u32,
        /// PyFLP's "page".
        page: u8,
        unknown_21: [u8; 15],
        /// Editor window position (inferred).
        editor_position: u64,
        /// Editor window size (inferred).
        editor_size: u64,
    }
}

layout! {
    /// 215 `ChannelID.Parameters`: struct of up to 168 bytes. The loader
    /// copies `min(len, 168)`, and each FL generation writes a longer one.
    Parameters[168, len 0..=168] {
        unknown_0: [u8; 4],
        /// A legacy sample field for writers below 2.2.0. The loader zeroes
        /// it for writers below 6.4.1.
        unknown_4: [u8; 4],
        unknown_8: [u8; 44],
        /// PyFLP reads an f32 arp time. The loader treats it as an integer
        /// in `gate_time`'s unit, for writers below 3.5.2 only.
        arp_time: [u8; 4],
        unknown_56: [u8; 4],
        /// Four single bytes. For writers below 3.3.4 the loader reduces the
        /// first to 0 or 1 and zeroes the other three.
        unknown_60: [u8; 4],
        gate_time: u32,
        unknown_68: [u8; 28],
        /// An f32 when 169 bit 0 is set, otherwise an integer the loader
        /// converts to one.
        unknown_96: [u8; 4],
        unknown_100: [u8; 8],
        /// Remapped by the loader for writers below 6.1.5 and below 11.5.13.
        stretch_mode: i32,
        unknown_112: [u8; 56],
    }
}

layout! {
    /// One 12-byte record of 216 `GlobalControllerEvents` or 223
    /// `PatternID.Controllers`. The dictionary gives the size only.
    ControllerEvent[12] {
        unknown_0: [u8; 12],
    }
}

layout! {
    /// 217 `ArrangementsID.PLSelection`: two u32.
    PlSelection[8] {
        start: u32,
        end: u32,
    }
}

layout! {
    /// 218 `ChannelID.EnvelopeLFO`: 68-byte struct, one per envelope in
    /// channel order. The loader also takes a shorter payload and fills in
    /// the missing tail.
    EnvelopeLfo[68, len 0..=68] {
        /// In a payload under 68 bytes, bit 3 makes the loader set offset 56
        /// to -101 and bit 6 offset 60.
        flags: u8,
        unknown_1: [u8; 55],
        unknown_56: [u8; 4],
        unknown_60: [u8; 4],
        unknown_64: [u8; 4],
    }
}

layout! {
    /// 219 `ChannelID.Levels`: 24-byte struct. Writers 3.0.0 to 3.2.61 store
    /// only the first 12 bytes here and the other 12 in a 220 [`Filter`].
    /// Whether `pan` is signed is not established.
    Levels[24, len 12 | 24] {
        /// Same slot as the legacy 3 `_PanByte`.
        pan: u32,
        /// Same slot as the legacy 2 `_VolByte`.
        volume: u32,
        unknown_8: [u8; 4],
        mod_x: u32,
        mod_y: u32,
        unknown_20: [u8; 4],
    }
}

layout! {
    /// 220 `ChannelID._Filter`: two u32, the legacy cutoff and resonance
    /// pair, or three in the 12-byte form that writers 3.0.0 to 3.2.61
    /// store. The loader copies it over the second half of [`Levels`], so
    /// the fields land in `mod_x`, `mod_y` and `unknown_20`.
    Filter[12, len 8 | 12] {
        cutoff: u32,
        /// Rescaled as `(v << 8) / 80` for writers below 3.2.2.
        resonance: u32,
        unknown_8: [u8; 4],
    }
}

layout! {
    /// 221 `ChannelID.Polyphony`: 9-byte struct.
    Polyphony[9] {
        unknown_0: [u8; 4],
        /// Remapped by the loader for writers below 3.5.2.
        unknown_4: [u8; 4],
        unknown_8: [u8; 1],
    }
}

layout! {
    /// One 24-byte record of 224 `PatternID.Notes`. FL's loader copies the
    /// record whole, and `docs/records-224-233.md` has the function that
    /// reads each field.
    Note[24] {
        /// Ticks from the start of the pattern.
        position: i32,
        /// 0x4000 in every corpus note, with bit 3 on top for a slide note
        /// ([`Note::slide`]). No reader was found for the other bits.
        tag: u16,
        /// The channel the note plays, by the index its 64 has. 0xFFFF here
        /// and in `tag` marks a record FL deleted ([`Note::is_deleted`]).
        channel: u16,
        /// Ticks. 0 for a step sequencer note, which has no end.
        length: i32,
        /// 60 plays a sample at its own pitch.
        key: i16,
        group: u16,
        /// 120 is no offset and a step is 10 cents
        /// ([`Note::fine_pitch_cents`]). Set to 120 by the loader for
        /// writers below 3.1.6.
        fine_pitch: u8,
        repeats: u8,
        /// 0..=128, and 64 for a new note. Set to 64 by the loader for
        /// writers below 6.5.0.
        release: u8,
        /// Bits 0 to 3: colour, which FL also uses as the MIDI channel. Bit
        /// 4: porta. Bit 5: muted. Bit 7: selected. No reader was found for
        /// bit 6. The low nibble is multiplied by 4 for writers below 5.3.0.
        flags: u8,
        /// 0..=128, 64 is centre.
        pan: u8,
        /// 0..=128, and 100 for a new note.
        velocity: u8,
        /// 128 is centre.
        mod_x: u8,
        /// 128 is centre.
        mod_y: u8,
    }
}

layout! {
    /// One 12-byte record of 225 `MixerID.Params`: an initial control value
    /// that the loader replays through FL's REC-event dispatcher.
    InitialValue[12] {
        unknown_0: [u8; 4],
        /// REC control ID. PyFLP's insert and slot fields are a view of it.
        rec_id: u32,
        value: i32,
    }
}

layout! {
    /// 226 `ControllerID.MIDI` or 227 `ControllerID.Remote`: 20 bytes.
    /// `docs/automation-234.md` has the fields.
    ControllerLink[20] {
        /// REC ID of what drives the link. An automation clip is control 0
        /// of its channel, `channel << 16`.
        source: u32,
        /// 0 in most links. The rest look like a second ID.
        unknown_4: [u8; 4],
        /// REC ID of the control the link drives, in the ID space of
        /// `docs/rec-225.md`.
        target: u32,
        /// Bit 2: a MIDI link. Bit 3: a remote link. The loader sets the
        /// one that fits the event. Bits 0 and 1 are set in a few files.
        flags: u32,
        /// Multiplied by -4 for writers below 4.6.1.
        unknown_16: [u8; 4],
    }
}

layout! {
    /// 228 `ChannelID.Tracking`: 16-byte struct, two per channel in order.
    Tracking[16] {
        unknown_0: [u8; 16],
    }
}

layout! {
    /// 229 `ChannelID.LevelAdjusts`: 20-byte struct.
    LevelAdjusts[20] {
        /// Multiplied by 100 for writers below 5.3.7.
        unknown_0: [u8; 4],
        /// Multiplied by 100 for writers below 5.3.7.
        unknown_4: [u8; 4],
        unknown_8: [u8; 12],
    }
}

layout! {
    /// One item of 233 `ArrangementID.Playlist`: 32, 60, 80 or 88 bytes.
    /// The size is one value per file, see [`PlaylistItem::size_for`]. The
    /// first 24 bytes are laid out as a [`Note`], and
    /// `docs/records-224-233.md` has the function that reads each field.
    PlaylistItem[88, len 32 | 60 | 80 | 88] {
        /// Ticks from the start of the song.
        position: i32,
        /// What the item places, see [`PlaylistItem::source`]. Only the
        /// high word decides. The low word is 0x5000 in every corpus item.
        source_id: u32,
        /// Ticks.
        length: i32,
        /// The track, counted down from a top that depends on the writer.
        /// See [`PlaylistItem::track`].
        track_word: i16,
        group: u16,
        /// 120, 0 and 64 in nearly every corpus item: a note's fine pitch
        /// and release, which no item code reads.
        unknown_16: [u8; 3],
        /// Bit 5: muted. Bit 7: selected, which FL clears after loading
        /// along with bit 6. Bit 4 is set and tested by a playlist edit
        /// pass and its meaning is not known.
        flags: u8,
        /// 64, 100, 128 and 128 in nearly every corpus item: a note's pan,
        /// velocity, mod X and mod Y, which no item code reads.
        unknown_20: [u8; 4],
        /// Where the shown part of the source starts: an i32 in ticks for
        /// a pattern item and the bits of an f32 for a clip item. See
        /// [`PlaylistItem::trim`].
        trim_start: u32,
        /// Where the shown part ends, stored like `trim_start`.
        trim_end: u32,
        /// The item's ID from FL's counter. From here on a 32-byte item has
        /// nothing.
        id: i32,
        /// Zero in FL's initialiser, and no reader was found. 3,250 of the
        /// 14,117 corpus items that have these bytes hold something here.
        unknown_36: [u8; 16],
        /// The last field of a 60-byte item. See [`PlaylistItem::clip`]
        /// for the value FL plays.
        gain: f32,
        /// Zero in FL's initialiser, and no reader was found.
        unknown_56: [u8; 4],
        /// Semitones.
        pitch: f32,
        stretch: f64,
        /// Bit 0: the clip plays backwards.
        clip_flags: u32,
        /// -1 in FL's initialiser and 0 in every corpus item. No reader was
        /// found.
        unknown_76: [u8; 4],
        /// -1 (left) to 1 (right). Only an 88-byte item has it.
        pan: f32,
        /// Zero in FL's initialiser, and no reader was found.
        unknown_84: [u8; 4],
    }
}

layout! {
    /// 236 `InsertID.Flags`: 12 bytes, or 5 or 8 in old files. In the 5-byte
    /// form the byte at offset 4 says which of two fields the first word
    /// goes to, and it reads here as the low byte of `flags`.
    InsertFlags[12, len 5 | 8 | 12] {
        /// XOR-masked for writers below 12.9.0. Meaning unknown.
        unknown_0: [u8; 4],
        /// The loader reads bits 0 to 4, 6, 7 and 10 to 15.
        flags: u32,
        unknown_8: [u8; 4],
    }
}

layout! {
    /// 237 `ProjectID.Timestamp`: two f64.
    Timestamp[16] {
        /// An f64.
        unknown_0: [u8; 8],
        /// The loader keeps "now minus this", so the save path can write the
        /// time spent back.
        time_spent: f64,
    }
}

layout! {
    /// 238 `TrackID.Data`: 13 to 70 bytes, longer in newer files. Offsets
    /// are odd from 13 on because a one-byte field sits at 12.
    Track[70, len 13..=70] {
        index: u32,
        /// The low 24 bits differ from 0x565148 when the colour is custom.
        color: u32,
        icon: u32,
        unknown_12: [u8; 1],
        /// An f32.
        unknown_13: [u8; 4],
        /// An i32.
        unknown_17: [u8; 4],
        unknown_21: [u8; 1],
        /// The six performance-mode fields PyFLP lists.
        performance: [u8; 24],
        grouped: u8,
        locked: u8,
        unknown_48: [u8; 1],
        /// A u32.
        unknown_49: [u8; 4],
        /// A u32.
        unknown_53: [u8; 4],
        /// A u32.
        unknown_57: [u8; 4],
        unknown_61: [u8; 1],
        unknown_62: [u8; 4],
        /// A u32 of which the loader reads bit 0.
        unknown_66: [u8; 4],
    }
}

layout! {
    /// 242 `ArrangementViewState`: 28-byte struct, the playlist scroll and
    /// zoom of one arrangement (inferred).
    ArrangementViewState[28] {
        /// Index of the arrangement this state belongs to.
        arrangement: u32,
        /// A u64.
        unknown_4: [u8; 8],
        /// An f64 that the loader multiplies by a zoom scale.
        unknown_12: [u8; 8],
        /// A u32.
        unknown_20: [u8; 4],
        /// An i32 that the loader multiplies by the same scale.
        unknown_24: [u8; 4],
    }
}

layout! {
    /// 243 `ProjectPairU32`: two u32, meaning unknown.
    ProjectPairU32[8] {
        unknown_0: [u8; 4],
        unknown_4: [u8; 4],
    }
}

layout! {
    /// 251 `ChannelSubState`: 43 bytes in the version-2 shape FL writes.
    ChannelSubState[43] {
        /// Record version.
        version: u32,
        unknown_4: [u8; 37],
        /// Goes to the sampler flag that 19 and 143 bit 3 also set.
        unknown_41: [u8; 1],
        unknown_42: [u8; 1],
    }
}

layout! {
    /// One 24-byte point of a 234 envelope or LFO block. Its `x`, `tension`
    /// and `mode` describe the segment that ends at it, so the first
    /// point's tension and mode do nothing.
    Point[24] {
        /// Beats (quarter notes) since the previous point.
        x: f64,
        /// 0..=1 of the clip's range. FL's loader replaces a NaN by the
        /// previous point's value.
        y: f64,
        /// -1..=1.
        tension: f32,
        /// The curve of the segment, 0..=12. `docs/automation-234.md` has
        /// the formula for each.
        mode: u8,
        /// Zeroed by the loader for an automation clip.
        flag: u8,
        unknown_22: [u8; 1],
        /// FL's cache of the tension's sign, or 2 for a segment it draws
        /// as a straight line. The loader works it out again.
        cache: i8,
    }
}

impl Point {
    /// A point `x` beats after the one before it, at the end of a straight
    /// line.
    pub fn new(x: f64, y: f64) -> Self {
        Point {
            x,
            y,
            tension: 0.0,
            mode: 0,
            flag: 0,
            unknown_22: [0],
            cache: 0,
        }
    }
}

/// 234 `ChannelID.Automation`: the data of an automation clip, laid out as
/// `docs/automation-234.md` has it. A 9-byte header, the envelope block
/// with the points the clip draws, and the block of the clip's LFO:
///
/// ```text
/// header        9 bytes
/// envelope      u32 4, u32 version, u32 n, n points, 36 bytes
/// LFO           u32 5, u32 version, u32 m, m points, 40 bytes
/// ```
///
/// That is `109 + 24 (n + m)` bytes in version 3, which every project in
/// the corpus has. The automation clip presets FL ships are older. Version
/// 2 (FL 9, 2 presets) ends the LFO block 4 bytes sooner. Version 1 (FL
/// 4.6, 56 presets) does too, and its points are 16 bytes, with `x` and
/// `y` as f32. Both blocks of a payload have the same version.
#[derive(Debug, Clone, PartialEq)]
pub struct AutomationData {
    /// A u32 format tag that is 1, an i32 that PyFLP calls the LFO amount
    /// (64 in nearly every clip), and a byte that says how the LFO
    /// combines with the envelope: 1 adds, anything else multiplies.
    pub header: [u8; 9],
    /// The format version of the two blocks, 1..=3. [`Self::to_bytes`]
    /// writes the layout of this version, so in version 1 a point's `x`
    /// and `y` are written as f32.
    pub version: u32,
    /// The points the clip draws.
    pub envelope: Vec<Point>,
    /// What follows the envelope's points. 20 bytes: a u32, an i32 that
    /// switches the envelope on when it is not 0, and three point indices
    /// for the loop and sustain markers, -1 for none. Then four u32 with
    /// no known meaning.
    pub envelope_tail: [u8; 36],
    /// The points of the clip's LFO. FL writes one point at 1.0 for an LFO
    /// that is off.
    pub lfo: Vec<Point>,
    /// What follows the LFO's points. The same 20 bytes, where the i32 is 0
    /// in every corpus clip, so no LFO is on. Then four u32, the first
    /// most likely the LFO's speed, and one more u32 that only version 3
    /// has: it reads as 0 in an older payload and is not written back.
    pub lfo_tail: [u8; 40],
}

impl AutomationData {
    const ENVELOPE: u32 = 4;
    const LFO: u32 = 5;
    /// The version current writers emit.
    const VERSION: u32 = 3;
    /// Bytes of a point in version 1.
    const SHORT_POINT: usize = 16;

    /// The clip FL makes for these points: the header, the tails and the
    /// switched-off LFO that nearly every corpus clip has.
    pub fn new(envelope: Vec<Point>) -> Self {
        let mut data = AutomationData {
            header: [1, 0, 0, 0, 64, 0, 0, 0, 0],
            version: Self::VERSION,
            envelope,
            envelope_tail: [0xff; 36],
            lfo: vec![Point::new(0.0, 1.0)],
            lfo_tail: [0; 40],
        };
        data.envelope_tail[..4].copy_from_slice(&1u32.to_le_bytes());
        for (word, value) in [128u32, 128, 0, 128].into_iter().enumerate() {
            let at = 20 + 4 * word;
            data.envelope_tail[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        data.lfo_tail[..4].copy_from_slice(&1u32.to_le_bytes());
        data.lfo_tail[8..20].fill(0xff);
        data.lfo_tail[20..24].copy_from_slice(&45_819u32.to_le_bytes());
        data
    }

    /// Decode a payload. `None` unless it is exactly a header and the two
    /// blocks, each of its kind and both of one version from 1 to 3.
    pub fn from_bytes(mut b: &[u8]) -> Option<Self> {
        let header = take(&mut b)?;
        let (version, envelope) = Self::points(&mut b, Self::ENVELOPE)?;
        let envelope_tail = take(&mut b)?;
        let (lfo_version, lfo) = Self::points(&mut b, Self::LFO)?;
        let mut lfo_tail = [0; 40];
        let tail = b.split_off(..Self::lfo_tail_len(version))?;
        lfo_tail[..tail.len()].copy_from_slice(tail);
        (lfo_version == version && b.is_empty()).then_some(AutomationData {
            header,
            version,
            envelope,
            envelope_tail,
            lfo,
            lfo_tail,
        })
    }

    /// Bytes after the LFO's points. The last u32 came with version 3.
    fn lfo_tail_len(version: u32) -> usize {
        if version < Self::VERSION { 36 } else { 40 }
    }

    /// Read the three words that open a block of this kind and its points
    /// off the front of `b`. Gives the block's version too.
    fn points(b: &mut &[u8], kind: u32) -> Option<(u32, Vec<Point>)> {
        let [found, version, count]: [u32; 3] = [take(b)?, take(b)?, take(b)?];
        if found != kind || !(1..=Self::VERSION).contains(&version) {
            return None;
        }
        let size = if version < 2 {
            Self::SHORT_POINT
        } else {
            Point::SIZE
        };
        // A count the payload has no room for stops here, before any point
        // is read.
        let len = usize::try_from(count).ok()?.checked_mul(size)?;
        let points = b.split_off(..len)?.chunks_exact(size).map(|mut point| {
            if version >= 2 {
                return Point::from_bytes(point);
            }
            Some(Point {
                x: f64::from(take::<f32>(&mut point)?),
                y: f64::from(take::<f32>(&mut point)?),
                tension: take(&mut point)?,
                mode: take(&mut point)?,
                flag: take(&mut point)?,
                unknown_22: take(&mut point)?,
                cache: take(&mut point)?,
            })
        });
        Some((version, points.collect::<Option<_>>()?))
    }

    /// The payload bytes, equal to what [`Self::from_bytes`] read.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.header.to_vec();
        let lfo_tail = &self.lfo_tail[..Self::lfo_tail_len(self.version)];
        let blocks = [
            (Self::ENVELOPE, &self.envelope, &self.envelope_tail[..]),
            (Self::LFO, &self.lfo, lfo_tail),
        ];
        for (kind, points, tail) in blocks {
            for word in [kind, self.version, points.len() as u32] {
                out.extend_from_slice(&word.to_le_bytes());
            }
            for point in points {
                let bytes = point.to_bytes();
                if self.version < 2 {
                    // A short point ends with the same 8 bytes.
                    out.extend_from_slice(&(point.x as f32).to_le_bytes());
                    out.extend_from_slice(&(point.y as f32).to_le_bytes());
                    out.extend_from_slice(&bytes[16..]);
                } else {
                    out.extend_from_slice(&bytes);
                }
            }
            out.extend_from_slice(tail);
        }
        out
    }
}

/// Read one field off the front of `b`, or `None` when `b` is too short.
fn take<T: Field>(b: &mut &[u8]) -> Option<T> {
    (b.len() >= T::SIZE).then(|| T::take(b))
}

records!(ControllerEvent, Note, InitialValue);

impl Note {
    /// `flags` bit 4.
    pub const PORTA: u8 = 0x10;
    /// `flags` bit 5.
    pub const MUTED: u8 = 0x20;
    /// `flags` bit 7.
    pub const SELECTED: u8 = 0x80;

    /// Whether the record is deleted: the u32 at offset 4 is all ones. FL
    /// compacts such records away when it saves, so no file it wrote has
    /// one, and `model::to_flp` leaves them out too.
    pub fn is_deleted(&self) -> bool {
        self.tag == u16::MAX && self.channel == u16::MAX
    }

    pub fn slide(&self) -> bool {
        self.tag & 0xf == 8
    }

    /// 0..=15.
    pub fn color(&self) -> u8 {
        self.flags & 0xf
    }

    pub fn porta(&self) -> bool {
        self.flags & Self::PORTA != 0
    }

    pub fn muted(&self) -> bool {
        self.flags & Self::MUTED != 0
    }

    pub fn selected(&self) -> bool {
        self.flags & Self::SELECTED != 0
    }

    /// How far `fine_pitch` moves the note, in cents.
    pub fn fine_pitch_cents(&self) -> i32 {
        (i32::from(self.fine_pitch) - 120) * 10
    }
}

/// The record FL makes for a new note, with the 0x4000 tag every corpus
/// note has: position, channel, length and key 0, fine pitch 120, release
/// 64, pan 64, velocity 100, mod X and mod Y 128.
impl Default for Note {
    fn default() -> Self {
        Note {
            position: 0,
            tag: 0x4000,
            channel: 0,
            length: 0,
            key: 0,
            group: 0,
            fine_pitch: 120,
            repeats: 0,
            release: 64,
            flags: 0,
            pan: 64,
            velocity: 100,
            mod_x: 128,
            mod_y: 128,
        }
    }
}

/// What a playlist item places on the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemSource {
    /// A pattern, by the index its 65 has.
    Pattern(u16),
    /// An audio or automation clip, by the index its channel's 64 has.
    Channel(u16),
}

/// The part of its source that a playlist item shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trim {
    /// Of a pattern item: the first tick of the pattern that is shown and
    /// the tick after the last.
    Ticks { start: i32, end: i32 },
    /// Of a clip item, in the clip's own unit. For an audio clip that is
    /// milliseconds of the sample at its own speed, counted from the
    /// sample's end when the item plays backwards: `FUN_00695930` makes
    /// frames of it by multiplying with two doubles of the sample and
    /// 0.001, and in 4,763 of the 6,043 corpus audio clips that have a
    /// trim and no stretch, `end - start` is the item's length at the
    /// project's tempo, to the tick. For an automation clip it is beats
    /// into the clip: `end - start` is the item's length in beats for
    /// 3,102 of the 3,104 corpus items that have a trim.
    Clip { start: f32, end: f32 },
}

/// How a clip item plays, with FL's default where the record is too short
/// to have the field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipSettings {
    /// 1 in an item under 60 bytes.
    pub gain: f32,
    /// Semitones.
    pub pitch: f32,
    /// 1 in an item under 80 bytes.
    pub stretch: f64,
    pub reversed: bool,
    /// -1 (left) to 1 (right).
    pub pan: f32,
}

/// The lowest `source_id` that names a pattern.
const PATTERN_IDS: u32 = 0x5000_0000;
/// The first `source_id` past those that name a channel.
const CHANNEL_IDS_END: u32 = 0x1000_0000;

impl PlaylistItem {
    /// `flags` bit 5.
    pub const MUTED: u8 = 0x20;
    /// `flags` bit 7.
    pub const SELECTED: u8 = 0x80;
    /// `clip_flags` bit 0.
    pub const REVERSED: u32 = 1;

    /// Item size for a file, from its writer version and the value of 169
    /// `DataFormatFlags` (0 when the file has none before its 233).
    ///
    /// Writers below 20.99.1 use 32 bytes whatever 169 says. From there
    /// 169 decides: bit 1 clear is 60 bytes, bit 3 clear is 80, and both
    /// set is 88.
    ///
    /// Writers below 8.1.0 use 28 bytes, and 20 below 3.6.6. Those records
    /// have another layout, which FL converts as it loads, and
    /// [`Self::list_from_bytes`] does not read them.
    pub fn size_for(writer_version: u32, flags169: u32) -> usize {
        if writer_version < 3_006_006 {
            20
        } else if writer_version < 8_001_000 {
            28
        } else if writer_version < 20_099_001 {
            32
        } else if flags169 & 2 == 0 {
            60
        } else if flags169 & 8 == 0 {
            80
        } else {
            88
        }
    }

    /// Decode a payload of whole items, of the size [`Self::size_for`]
    /// gives. `None` when that size does not divide the payload, or for
    /// the items of a writer below 8.1.0.
    pub fn list_from_bytes(b: &[u8], writer_version: u32, flags169: u32) -> Option<Vec<Self>> {
        let size = Self::size_for(writer_version, flags169);
        if !b.len().is_multiple_of(size) {
            return None;
        }
        b.chunks_exact(size).map(Self::from_bytes).collect()
    }

    /// The payload for a list of items.
    pub fn list_to_bytes(items: &[Self]) -> Vec<u8> {
        items.iter().flat_map(Self::to_bytes).collect()
    }

    /// The item FL makes for a new clip of this source: on track 1, not
    /// trimmed, gain and stretch 1, no ID, and the note defaults in the
    /// bytes it shares with a note. The four bytes at 76 are 0 as in every
    /// corpus item, where FL's initialiser has -1. It has all 88 bytes.
    /// [`Self::shortened`] makes it fit a file whose items are shorter.
    pub fn new(source: ItemSource, position: i32, length: i32) -> Self {
        let (source_id, no_trim) = match source {
            ItemSource::Pattern(index) => {
                (PATTERN_IDS.wrapping_add(u32::from(index) << 16), u32::MAX)
            }
            ItemSource::Channel(index) => (u32::from(index) << 16, (-1f32).to_bits()),
        };
        PlaylistItem {
            position,
            // Every corpus item has 0x5000 in the low word.
            source_id: source_id | 0x5000,
            length,
            track_word: 499,
            group: 0,
            unknown_16: [120, 0, 64],
            flags: 0,
            unknown_20: [64, 100, 128, 128],
            trim_start: no_trim,
            trim_end: no_trim,
            id: -1,
            unknown_36: [0; 16],
            gain: 1.0,
            unknown_56: [0; 4],
            pitch: 0.0,
            stretch: 1.0,
            clip_flags: 0,
            unknown_76: [0; 4],
            pan: 0.0,
            unknown_84: [0; 4],
            len: Self::SIZE,
        }
    }

    /// The item as a file with items of `len` bytes holds it: the fields
    /// past `len` are gone and read as zero. `None` when `len` is not an
    /// item size, or is more than the item has.
    pub fn shortened(&self, len: usize) -> Option<Self> {
        Self::from_bytes(self.to_bytes().get(..len)?)
    }

    /// What the item places. `None` for a record FL deleted, where
    /// `source_id` is all ones, and for an ID that names neither.
    pub fn source(&self) -> Option<ItemSource> {
        let index = (self.source_id >> 16) as u16;
        match self.source_id {
            ..CHANNEL_IDS_END => Some(ItemSource::Channel(index)),
            // FL compares the ID as an i32, so the range ends at its top.
            PATTERN_IDS..0x8000_0000 => Some(ItemSource::Pattern(index - 0x5000)),
            _ => None,
        }
    }

    /// Whether FL deleted the record. It stays in the list so that the
    /// file writes back the same.
    pub fn is_deleted(&self) -> bool {
        self.source_id == u32::MAX
    }

    /// The track the item is on, where the first track is 1. Writers from
    /// 12.9.1 on store `500 - track`. Older ones had 199 tracks and store
    /// `199 - track`: that version is PyFLP's, and the corpus fits it, with
    /// 198 as the highest word up to 12.3.0 and 499 from 12.9.3.
    pub fn track(&self, writer_version: u32) -> i32 {
        let top = if writer_version < 12_009_001 {
            199
        } else {
            500
        };
        top - i32::from(self.track_word)
    }

    pub fn muted(&self) -> bool {
        self.flags & Self::MUTED != 0
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.flags = self.flags & !Self::MUTED | if muted { Self::MUTED } else { 0 };
    }

    pub fn selected(&self) -> bool {
        self.flags & Self::SELECTED != 0
    }

    /// The part of the source the item shows, or `None` for an item that
    /// shows it from the start. FL's own test: the start is not negative,
    /// read as an f32 for a channel's item and as an i32 for any other.
    /// Writers before FL 20 put -1.0 in pattern items too, and as an i32
    /// that is negative as well.
    pub fn trim(&self) -> Option<Trim> {
        if self.source_id < CHANNEL_IDS_END {
            let start = f32::from_bits(self.trim_start);
            let end = f32::from_bits(self.trim_end);
            (start >= 0.0).then_some(Trim::Clip { start, end })
        } else {
            let (start, end) = (self.trim_start as i32, self.trim_end as i32);
            (start >= 0).then_some(Trim::Ticks { start, end })
        }
    }

    /// Store a trim, or FL's "not trimmed" for `None`: -1.0 twice for a
    /// channel's item and -1 twice for any other.
    pub fn set_trim(&mut self, trim: Option<Trim>) {
        let clip = self.source_id < CHANNEL_IDS_END;
        (self.trim_start, self.trim_end) = match trim {
            Some(Trim::Ticks { start, end }) => (start as u32, end as u32),
            Some(Trim::Clip { start, end }) => (start.to_bits(), end.to_bits()),
            None if clip => ((-1f32).to_bits(), (-1f32).to_bits()),
            None => (u32::MAX, u32::MAX),
        };
    }

    /// Gain, pitch, stretch, direction and pan as FL plays them. The
    /// defaults for a short item are what FL's initialiser writes
    /// (inferred: the code that fills a short record in after loading was
    /// not read).
    pub fn clip(&self) -> ClipSettings {
        ClipSettings {
            gain: if self.len < 60 { 1.0 } else { self.gain },
            pitch: self.pitch,
            stretch: if self.len < 80 { 1.0 } else { self.stretch },
            reversed: self.clip_flags & Self::REVERSED != 0,
            pan: self.pan,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Debug;

    /// `n` bytes counting up from 0, so a field's value gives its offset.
    fn ramp(n: usize) -> Vec<u8> {
        (0..n).map(|i| i as u8).collect()
    }

    /// The little-endian u32 found at `offset` of a ramp.
    fn at(offset: u8) -> u32 {
        u32::from_le_bytes([offset, offset + 1, offset + 2, offset + 3])
    }

    /// Round-trip a ramp of `size` bytes and check the neighbours fail.
    fn exact<T: Debug>(size: usize, from: fn(&[u8]) -> Option<T>, to: fn(&T) -> Vec<u8>) -> T {
        assert!(from(&ramp(size - 1)).is_none());
        assert!(from(&ramp(size + 1)).is_none());
        prefix(size, from, to)
    }

    /// Round-trip a ramp of `len` bytes.
    fn prefix<T: Debug>(len: usize, from: fn(&[u8]) -> Option<T>, to: fn(&T) -> Vec<u8>) -> T {
        let bytes = ramp(len);
        let value = from(&bytes).unwrap();
        assert_eq!(to(&value), bytes, "{value:?}");
        value
    }

    #[test]
    fn delay() {
        let d = exact(20, Delay::from_bytes, Delay::to_bytes);
        assert_eq!(d.unknown_0, [0, 1, 2, 3]);
        assert_eq!(d.pan, at(4) as i32);
        assert_eq!(d.unknown_8, [8, 9, 10, 11]);
        assert_eq!(d.unknown_12, [12, 13, 14, 15, 16, 17, 18, 19]);
        let mut bytes = ramp(20);
        bytes[4..8].copy_from_slice(&(-6400i32).to_le_bytes());
        assert_eq!(Delay::from_bytes(&bytes).unwrap().pan, -6400);
    }

    #[test]
    fn plugin_wrapper() {
        let w = prefix(52, PluginWrapper::from_bytes, PluginWrapper::to_bytes);
        assert_eq!(w.len, 52);
        assert_eq!(w.insert, at(0) as i32);
        assert_eq!(w.slot, at(4) as i32);
        assert_eq!(w.plugin_kind, at(8) as i32);
        assert_eq!(w.flags, at(16));
        assert_eq!(w.page, 20);
        assert_eq!(w.unknown_21[0], 21);
        assert_eq!(w.editor_position.to_le_bytes(), ramp(44)[36..]);
        assert_eq!(w.editor_size.to_le_bytes(), ramp(52)[44..]);
        let short = prefix(20, PluginWrapper::from_bytes, PluginWrapper::to_bytes);
        assert_eq!((short.len, short.flags, short.page), (20, at(16), 0));
        assert!(PluginWrapper::from_bytes(&ramp(53)).is_none());
    }

    #[test]
    fn parameters() {
        let p = prefix(168, Parameters::from_bytes, Parameters::to_bytes);
        assert_eq!(p.len, 168);
        assert_eq!(p.unknown_4, [4, 5, 6, 7]);
        assert_eq!(p.arp_time, [52, 53, 54, 55]);
        assert_eq!(p.unknown_60, [60, 61, 62, 63]);
        assert_eq!(p.gate_time, at(64));
        assert_eq!(p.unknown_96, [96, 97, 98, 99]);
        assert_eq!(p.stretch_mode, at(108) as i32);
        assert_eq!(p.unknown_112[55], 167);
        // A size that ends inside the last field.
        let short = prefix(125, Parameters::from_bytes, Parameters::to_bytes);
        assert_eq!(short.len, 125);
        assert_eq!(short.unknown_112[12..14], [124, 0]);
        let old = prefix(40, Parameters::from_bytes, Parameters::to_bytes);
        assert_eq!((old.gate_time, old.stretch_mode), (0, 0));
        assert!(Parameters::from_bytes(&ramp(169)).is_none());
    }

    #[test]
    fn controller_events() {
        let e = exact(12, ControllerEvent::from_bytes, ControllerEvent::to_bytes);
        assert_eq!(e.unknown_0[..], ramp(12));
        let list = ControllerEvent::list_from_bytes(&ramp(36)).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[2].unknown_0[0], 24);
        assert_eq!(ControllerEvent::list_to_bytes(&list), ramp(36));
        assert_eq!(ControllerEvent::list_from_bytes(&[]), Some(vec![]));
        assert!(ControllerEvent::list_from_bytes(&ramp(35)).is_none());
    }

    #[test]
    fn pl_selection() {
        let s = exact(8, PlSelection::from_bytes, PlSelection::to_bytes);
        assert_eq!((s.start, s.end), (at(0), at(4)));
    }

    #[test]
    fn envelope_lfo() {
        let e = prefix(68, EnvelopeLfo::from_bytes, EnvelopeLfo::to_bytes);
        assert_eq!(e.len, 68);
        assert_eq!(e.unknown_1[0], 1);
        assert_eq!(e.unknown_56, [56, 57, 58, 59]);
        assert_eq!(e.unknown_60, [60, 61, 62, 63]);
        assert_eq!(e.unknown_64, [64, 65, 66, 67]);
        let mut bytes = ramp(56);
        bytes[0] = 0x48;
        let short = EnvelopeLfo::from_bytes(&bytes).unwrap();
        assert_eq!((short.len, short.flags), (56, 0x48));
        assert_eq!(short.unknown_56, [0; 4]);
        assert_eq!(short.to_bytes(), bytes);
        assert!(EnvelopeLfo::from_bytes(&ramp(69)).is_none());
    }

    #[test]
    fn levels() {
        let l = prefix(24, Levels::from_bytes, Levels::to_bytes);
        assert_eq!((l.len, l.pan, l.volume), (24, at(0), at(4)));
        assert_eq!(l.unknown_8, [8, 9, 10, 11]);
        assert_eq!((l.mod_x, l.mod_y), (at(12), at(16)));
        assert_eq!(l.unknown_20, [20, 21, 22, 23]);
        let old = prefix(12, Levels::from_bytes, Levels::to_bytes);
        assert_eq!((old.len, old.pan, old.volume), (12, at(0), at(4)));
        assert_eq!(old.unknown_8, [8, 9, 10, 11]);
        assert_eq!((old.mod_x, old.mod_y, old.unknown_20), (0, 0, [0; 4]));
        for len in [0, 8, 11, 13, 20, 23, 25] {
            assert!(Levels::from_bytes(&ramp(len)).is_none(), "{len}");
        }
    }

    #[test]
    fn filter() {
        let f = prefix(8, Filter::from_bytes, Filter::to_bytes);
        assert_eq!((f.len, f.cutoff, f.resonance), (8, at(0), at(4)));
        assert_eq!(f.unknown_8, [0; 4]);
        let old = prefix(12, Filter::from_bytes, Filter::to_bytes);
        assert_eq!((old.len, old.cutoff, old.resonance), (12, at(0), at(4)));
        assert_eq!(old.unknown_8, [8, 9, 10, 11]);
        for len in [0, 4, 7, 9, 11, 13] {
            assert!(Filter::from_bytes(&ramp(len)).is_none(), "{len}");
        }
    }

    #[test]
    fn polyphony() {
        let p = exact(9, Polyphony::from_bytes, Polyphony::to_bytes);
        assert_eq!(p.unknown_0, [0, 1, 2, 3]);
        assert_eq!(p.unknown_4, [4, 5, 6, 7]);
        assert_eq!(p.unknown_8, [8]);
    }

    /// The little-endian u16 found at `offset` of a ramp.
    fn word(offset: u8) -> u16 {
        u16::from_le_bytes([offset, offset + 1])
    }

    #[test]
    fn notes() {
        let n = exact(24, Note::from_bytes, Note::to_bytes);
        assert_eq!((n.position, n.length), (at(0) as i32, at(8) as i32));
        assert_eq!((n.tag, n.channel), (word(4), word(6)));
        assert_eq!((n.key, n.group), (word(12) as i16, word(14)));
        assert_eq!(
            (n.fine_pitch, n.repeats, n.release, n.flags),
            (16, 17, 18, 19)
        );
        assert_eq!((n.pan, n.velocity, n.mod_x, n.mod_y), (20, 21, 22, 23));
        let list = Note::list_from_bytes(&ramp(48)).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].fine_pitch, 40);
        assert_eq!(Note::list_to_bytes(&list), ramp(48));
        assert!(Note::list_from_bytes(&ramp(47)).is_none());
    }

    #[test]
    fn a_new_note_is_fl_s_default_record() {
        let n = Note::default();
        let mut bytes = [0u8; 24];
        bytes[5] = 0x40;
        bytes[16] = 120;
        bytes[18] = 64;
        bytes[20..].copy_from_slice(&[64, 100, 128, 128]);
        assert_eq!(n.to_bytes(), bytes);
        assert!(!n.is_deleted() && !n.slide() && !n.porta() && !n.muted() && !n.selected());
        assert_eq!((n.color(), n.fine_pitch_cents()), (0, 0));
    }

    #[test]
    fn note_flags() {
        let mut n = Note {
            tag: 0x4008,
            fine_pitch: 100,
            flags: 0x25,
            ..Note::default()
        };
        assert!(n.slide() && n.muted() && !n.porta() && !n.selected());
        assert_eq!((n.color(), n.fine_pitch_cents()), (5, -200));
        n.flags = 0x90;
        assert!(n.porta() && n.selected() && !n.muted());
        // Only all ones at offset 4 is a deleted record.
        n.tag = u16::MAX;
        assert!(!n.is_deleted());
        n.channel = u16::MAX;
        assert!(n.is_deleted());
        assert_eq!(Note::from_bytes(&n.to_bytes()), Some(n));
    }

    #[test]
    fn initial_values() {
        let v = exact(12, InitialValue::from_bytes, InitialValue::to_bytes);
        assert_eq!(v.unknown_0, [0, 1, 2, 3]);
        assert_eq!((v.rec_id, v.value), (at(4), at(8) as i32));
        let mut bytes = ramp(24);
        bytes[20..].copy_from_slice(&(-1i32).to_le_bytes());
        let list = InitialValue::list_from_bytes(&bytes).unwrap();
        assert_eq!((list[1].rec_id, list[1].value), (at(16), -1));
        assert_eq!(InitialValue::list_to_bytes(&list), bytes);
        assert!(InitialValue::list_from_bytes(&ramp(13)).is_none());
    }

    #[test]
    fn controller_link() {
        let c = exact(20, ControllerLink::from_bytes, ControllerLink::to_bytes);
        assert_eq!((c.source, c.target, c.flags), (at(0), at(8), at(12)));
        assert_eq!(c.unknown_4, [4, 5, 6, 7]);
        assert_eq!(c.unknown_16, [16, 17, 18, 19]);
    }

    #[test]
    fn point() {
        let p = exact(24, Point::from_bytes, Point::to_bytes);
        assert_eq!(p.x.to_le_bytes(), ramp(8)[..]);
        assert_eq!(p.y.to_le_bytes(), ramp(16)[8..]);
        assert_eq!(p.tension.to_bits(), at(16));
        assert_eq!((p.mode, p.flag, p.unknown_22, p.cache), (20, 21, [22], 23));
        let mut bytes = Point::new(0.5, 0.25).to_bytes();
        assert_eq!(
            bytes[..16],
            [0.5f64.to_le_bytes(), 0.25f64.to_le_bytes()].concat()
        );
        assert_eq!(bytes[16..], [0; 8]);
        bytes[23] = 0xff;
        assert_eq!(Point::from_bytes(&bytes).unwrap().cache, -1);
    }

    /// A 234 payload put together word by word: `n` envelope points and `m`
    /// LFO points, every other byte counting up.
    fn automation_bytes(n: u32, m: u32) -> Vec<u8> {
        let mut bytes = ramp(9);
        for (kind, count, tail) in [(4u32, n, 36), (5, m, 40)] {
            for word in [kind, 3, count] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            let filler = bytes.len() as u8;
            bytes.extend((0..24 * count as usize + tail).map(|i| filler.wrapping_add(i as u8)));
        }
        bytes
    }

    #[test]
    fn automation_data() {
        for (n, m) in [(0, 0), (1, 1), (3, 1), (2, 2), (120, 1)] {
            let bytes = automation_bytes(n, m);
            assert_eq!(bytes.len(), 109 + 24 * (n + m) as usize);
            let data = AutomationData::from_bytes(&bytes).unwrap();
            assert_eq!(
                (data.envelope.len(), data.lfo.len()),
                (n as usize, m as usize)
            );
            assert_eq!(data.to_bytes(), bytes, "{n} and {m} points");
        }
        let bytes = automation_bytes(3, 1);
        let data = AutomationData::from_bytes(&bytes).unwrap();
        assert_eq!(data.header[..], ramp(9));
        // The envelope's points start at 21, its tail at 21 + 72, and the
        // LFO's one point 12 bytes after that.
        assert_eq!(data.envelope[1].to_bytes(), bytes[45..69]);
        assert_eq!(data.envelope_tail[..], bytes[93..129]);
        assert_eq!(data.lfo[0].to_bytes(), bytes[141..165]);
        assert_eq!(data.lfo_tail[..], bytes[165..]);
    }

    #[test]
    fn automation_data_of_another_shape_does_not_decode() {
        let good = automation_bytes(2, 1);
        assert!(AutomationData::from_bytes(&good).is_some());
        assert!(AutomationData::from_bytes(&good[..good.len() - 1]).is_none());
        assert!(AutomationData::from_bytes(&[good.clone(), vec![0]].concat()).is_none());
        assert!(AutomationData::from_bytes(&[]).is_none());
        // The envelope's kind, a version no block has, a version the LFO
        // block does not share, the LFO's kind at 105, and a count that
        // asks for more points than the payload has.
        for (at, value) in [
            (9, 5u32),
            (13, 0),
            (13, 4),
            (13, 2),
            (105, 4),
            (17, 3),
            (17, u32::MAX),
        ] {
            let mut bad = good.clone();
            bad[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(
                AutomationData::from_bytes(&bad).is_none(),
                "{value} at {at}"
            );
        }
    }

    #[test]
    fn automation_data_from_fl_4_and_fl_9_has_its_own_sizes() {
        // Version 2 has no last u32 in its LFO block.
        let mut bytes = automation_bytes(2, 1);
        bytes.truncate(bytes.len() - 4);
        bytes[13] = 2;
        bytes[105 + 4] = 2;
        let data = AutomationData::from_bytes(&bytes).unwrap();
        assert_eq!((data.version, data.envelope.len()), (2, 2));
        assert_eq!(data.lfo_tail[36..], [0; 4]);
        assert_eq!(data.to_bytes(), bytes);

        // Version 1 has points of 16 bytes on top of that: BasicGate1.fst
        // starts with these two.
        let points = [(0f32, 1f32, 0u8), (0.375, 1.0, 2)];
        let mut bytes = vec![1, 0, 0, 0, 64, 0, 0, 0, 0];
        for (kind, count, tail) in [(4u32, 2, 36), (5, 1, 36)] {
            for word in [kind, 1, count] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            for (x, y, mode) in &points[..count as usize] {
                bytes.extend_from_slice(&x.to_le_bytes());
                bytes.extend_from_slice(&y.to_le_bytes());
                bytes.extend_from_slice(&[0, 0, 0, 0, *mode, 0, 0, 0xff]);
            }
            bytes.extend(std::iter::repeat_n(7, tail));
        }
        assert_eq!(bytes.len(), 9 + 48 + 48 + 16 * 3);
        let data = AutomationData::from_bytes(&bytes).unwrap();
        assert_eq!(data.version, 1);
        let read = |p: &Point| (p.x, p.y, p.mode, p.cache);
        assert_eq!(read(&data.envelope[1]), (0.375, 1.0, 2, -1));
        assert_eq!(read(&data.lfo[0]), (0.0, 1.0, 0, -1));
        assert_eq!(data.to_bytes(), bytes);
    }

    #[test]
    fn a_new_clip_has_the_bytes_fl_writes_around_its_points() {
        let data = AutomationData::new(vec![Point::new(0.0, 0.5), Point::new(4.0, 1.0)]);
        let bytes = data.to_bytes();
        assert_eq!(bytes.len(), 109 + 24 * 3);
        assert_eq!(AutomationData::from_bytes(&bytes), Some(data));
        let words = |from: usize, count: usize| -> Vec<i32> {
            let (words, _) = bytes[from..from + 4 * count].as_chunks::<4>();
            words.iter().map(|word| i32::from_le_bytes(*word)).collect()
        };
        assert_eq!(bytes[..9], [1, 0, 0, 0, 64, 0, 0, 0, 0]);
        assert_eq!(words(9, 3), [4, 3, 2]);
        assert_eq!(words(69, 9), [1, -1, -1, -1, -1, 128, 128, 0, 128]);
        assert_eq!(words(105, 3), [5, 3, 1]);
        assert_eq!(
            Point::from_bytes(&bytes[117..141]),
            Some(Point::new(0.0, 1.0))
        );
        assert_eq!(words(141, 10), [1, 0, -1, -1, -1, 45_819, 0, 0, 0, 0]);
    }

    #[test]
    fn tracking() {
        let t = exact(16, Tracking::from_bytes, Tracking::to_bytes);
        assert_eq!(t.unknown_0[..], ramp(16));
    }

    #[test]
    fn level_adjusts() {
        let l = exact(20, LevelAdjusts::from_bytes, LevelAdjusts::to_bytes);
        assert_eq!(l.unknown_0, [0, 1, 2, 3]);
        assert_eq!(l.unknown_4, [4, 5, 6, 7]);
        assert_eq!(l.unknown_8[..], ramp(20)[8..]);
    }

    #[test]
    fn playlist_items() {
        for len in [32, 60, 80, 88] {
            let item = prefix(len, PlaylistItem::from_bytes, PlaylistItem::to_bytes);
            assert_eq!(item.len, len);
            assert_eq!((item.position, item.length), (at(0) as i32, at(8) as i32));
            assert_eq!(item.source_id, at(4));
            assert_eq!((item.track_word, item.group), (word(12) as i16, word(14)));
            assert_eq!((item.unknown_16, item.flags), ([16, 17, 18], 19));
            assert_eq!(item.unknown_20, [20, 21, 22, 23]);
            assert_eq!((item.trim_start, item.trim_end), (at(24), at(28)));
            // A field the item is too short for reads as zero.
            let past = |offset: u8| {
                if len > usize::from(offset) {
                    at(offset)
                } else {
                    0
                }
            };
            assert_eq!(item.id, past(32) as i32);
            assert_eq!(item.unknown_36[0], past(36) as u8);
            assert_eq!(item.gain.to_bits(), past(52));
            assert_eq!(item.unknown_56, past(56).to_le_bytes());
            assert_eq!(item.pitch.to_bits(), past(60));
            let stretch = u64::from(past(64)) | u64::from(past(68)) << 32;
            assert_eq!(item.stretch.to_bits(), stretch);
            assert_eq!(item.clip_flags, past(72));
            assert_eq!(item.unknown_76, past(76).to_le_bytes());
            assert_eq!(item.pan.to_bits(), past(80));
            assert_eq!(item.unknown_84, past(84).to_le_bytes());
        }
        for len in [0, 31, 33, 59, 64, 79, 81, 87, 89] {
            assert!(PlaylistItem::from_bytes(&ramp(len)).is_none(), "{len}");
        }
    }

    /// A writer version for each item size, with the 169 that goes with it.
    const SIZES: [(u32, u32, usize); 4] = [
        (20_008_004, 0, 32),
        (24_001_002, 1, 60),
        (25_001_000, 7, 80),
        (26_001_000, 15, 88),
    ];

    #[test]
    fn a_playlist_is_cut_at_the_size_its_file_has() {
        // 5,280 bytes divide by all four sizes.
        let bytes: Vec<u8> = (0..5280).map(|i| (i % 251) as u8).collect();
        for (version, flags, size) in SIZES {
            let list = PlaylistItem::list_from_bytes(&bytes, version, flags).unwrap();
            assert_eq!(list.len(), 5280 / size);
            assert!(list.iter().all(|item| item.len == size));
            assert_eq!(PlaylistItem::list_to_bytes(&list), bytes);
            // One byte more is not whole items, and no bytes are no items.
            assert!(PlaylistItem::list_from_bytes(&bytes[1..], version, flags).is_none());
            assert_eq!(
                PlaylistItem::list_from_bytes(&[], version, flags),
                Some(vec![])
            );
        }
        // The records of a writer below 8.1.0 are not read.
        assert!(PlaylistItem::list_from_bytes(&bytes[..28], 8_000_999, 0).is_none());
        assert!(PlaylistItem::list_from_bytes(&bytes[..20], 3_006_005, 0).is_none());
        assert_eq!(PlaylistItem::list_from_bytes(&[], 0, 0), Some(vec![]));
    }

    #[test]
    fn playlist_item_size() {
        for (version, flags, size) in SIZES {
            assert_eq!(PlaylistItem::size_for(version, flags), size);
        }
        // Bit 1 of 169 clear is 60 bytes whatever bit 3 says.
        for flags in [0, 1, 5, 8, 0xd] {
            assert_eq!(PlaylistItem::size_for(24_001_002, flags), 60, "{flags:#x}");
        }
        assert_eq!(PlaylistItem::size_for(24_002_099, 2), 80);
        assert_eq!(PlaylistItem::size_for(24_002_099, 0xa), 88);
        // 169 counts from 20.99.1 on.
        for flags in [0, 7, 0xf] {
            assert_eq!(PlaylistItem::size_for(20_099_000, flags), 32, "{flags:#x}");
            assert_eq!(PlaylistItem::size_for(8_001_000, flags), 32, "{flags:#x}");
        }
        assert_eq!(PlaylistItem::size_for(20_099_001, 0), 60);
        assert_eq!(PlaylistItem::size_for(20_099_001, 0xf), 88);
        // Below 8.1.0 the records are the old ones.
        assert_eq!(PlaylistItem::size_for(8_000_999, 0xf), 28);
        assert_eq!(PlaylistItem::size_for(3_006_006, 0), 28);
        assert_eq!(PlaylistItem::size_for(3_006_005, 0), 20);
    }

    #[test]
    fn a_new_item_is_fl_s_default_record() {
        let item = PlaylistItem::new(ItemSource::Pattern(3), 384, 96);
        assert_eq!(item.len, 88);
        let bytes = item.to_bytes();
        assert_eq!(bytes[0..4], 384i32.to_le_bytes());
        assert_eq!(bytes[4..8], [0x00, 0x50, 0x03, 0x50]);
        assert_eq!(bytes[8..12], 96i32.to_le_bytes());
        assert_eq!(bytes[12..16], [0xf3, 0x01, 0, 0]);
        assert_eq!(bytes[16..24], [120, 0, 64, 0, 64, 100, 128, 128]);
        assert_eq!(bytes[24..36], [0xff; 12]);
        assert_eq!(bytes[36..52], [0; 16]);
        assert_eq!(bytes[52..56], 1f32.to_le_bytes());
        assert_eq!(bytes[56..64], [0; 8]);
        assert_eq!(bytes[64..72], 1f64.to_le_bytes());
        assert_eq!(bytes[72..88], [0; 16]);
        assert_eq!(item.source(), Some(ItemSource::Pattern(3)));
        assert_eq!(item.trim(), None);
        assert!(!item.muted() && !item.selected() && !item.is_deleted());

        let clip = PlaylistItem::new(ItemSource::Channel(7), 0, 96);
        assert_eq!(clip.to_bytes()[4..8], [0x00, 0x50, 0x07, 0x00]);
        assert_eq!(
            clip.to_bytes()[24..32],
            [0, 0, 0x80, 0xbf, 0, 0, 0x80, 0xbf]
        );
        assert_eq!(clip.source(), Some(ItemSource::Channel(7)));
        assert_eq!(clip.trim(), None);
    }

    #[test]
    fn an_item_s_source_is_a_pattern_a_channel_or_neither() {
        let mut item = PlaylistItem::new(ItemSource::Pattern(1), 0, 96);
        for (id, source) in [
            (0x0000_5000, Some(ItemSource::Channel(0))),
            (0x0fff_ffff, Some(ItemSource::Channel(0x0fff))),
            (0x1000_0000, None),
            (0x4fff_ffff, None),
            (0x5000_0000, Some(ItemSource::Pattern(0))),
            (0x53e7_5000, Some(ItemSource::Pattern(999))),
            (0x7fff_ffff, Some(ItemSource::Pattern(0x2fff))),
            (0x8000_0000, None),
            (u32::MAX, None),
        ] {
            item.source_id = id;
            assert_eq!(item.source(), source, "{id:#x}");
            assert_eq!(item.is_deleted(), id == u32::MAX, "{id:#x}");
        }
    }

    #[test]
    fn a_trim_is_ticks_for_a_pattern_and_floats_for_a_clip() {
        let mut item = PlaylistItem::new(ItemSource::Pattern(1), 0, 96);
        let ticks = Trim::Ticks {
            start: 96,
            end: 192,
        };
        item.set_trim(Some(ticks));
        assert_eq!((item.trim_start, item.trim_end), (96, 192));
        assert_eq!(item.trim(), Some(ticks));
        item.set_trim(None);
        assert_eq!((item.trim_start, item.trim_end), (u32::MAX, u32::MAX));
        // Writers before FL 20 store -1.0 in a pattern item, which is a
        // negative i32 too.
        item.trim_start = (-1f32).to_bits();
        item.trim_end = (-1f32).to_bits();
        assert_eq!(item.trim(), None);
        // A start of 0 is a trim.
        item.trim_start = 0;
        item.trim_end = 96;
        assert_eq!(item.trim(), Some(Trim::Ticks { start: 0, end: 96 }));

        let mut clip = PlaylistItem::new(ItemSource::Channel(2), 0, 96);
        let part = Trim::Clip {
            start: 250.0,
            end: 1000.5,
        };
        clip.set_trim(Some(part));
        assert_eq!(clip.to_bytes()[24..28], 250f32.to_le_bytes());
        assert_eq!(clip.trim(), Some(part));
        clip.set_trim(None);
        assert_eq!(clip.trim_start, (-1f32).to_bits());
        clip.trim_start = 0;
        clip.trim_end = 500f32.to_bits();
        assert_eq!(
            clip.trim(),
            Some(Trim::Clip {
                start: 0.0,
                end: 500.0
            })
        );
    }

    #[test]
    fn the_track_counts_down_from_500_or_from_199() {
        let item = PlaylistItem::new(ItemSource::Pattern(1), 0, 96);
        assert_eq!(item.track(26_001_000), 1);
        assert_eq!(item.track(12_009_001), 1);
        let old = PlaylistItem {
            track_word: 198,
            ..item
        };
        assert_eq!(old.track(12_009_000), 1);
        assert_eq!(old.track(12_009_001), 302);
        let low = PlaylistItem {
            track_word: 0,
            ..item
        };
        assert_eq!((low.track(26_001_000), low.track(11_000_004)), (500, 199));
    }

    #[test]
    fn mute_is_bit_5_of_the_flags() {
        let mut item = PlaylistItem::new(ItemSource::Pattern(1), 0, 96);
        item.flags = 0x90;
        item.set_muted(true);
        assert_eq!(item.flags, 0xb0);
        assert!(item.muted() && item.selected());
        assert_eq!(item.to_bytes()[19], 0xb0);
        item.set_muted(false);
        assert_eq!(item.flags, 0x90);
        assert!(!item.muted());
    }

    #[test]
    fn a_short_item_plays_with_fl_s_defaults() {
        let mut item = PlaylistItem::new(ItemSource::Channel(0), 0, 96);
        item.gain = 0.5;
        item.pitch = -12.0;
        item.stretch = 2.0;
        item.clip_flags = 1;
        item.pan = -1.0;
        let whole = ClipSettings {
            gain: 0.5,
            pitch: -12.0,
            stretch: 2.0,
            reversed: true,
            pan: -1.0,
        };
        assert_eq!(item.clip(), whole);
        let bytes = item.to_bytes();
        let cut = |len: usize| PlaylistItem::from_bytes(&bytes[..len]).unwrap().clip();
        assert_eq!(cut(80), ClipSettings { pan: 0.0, ..whole });
        assert_eq!(item.shortened(80).unwrap().clip(), cut(80));
        assert_eq!(item.shortened(32).unwrap().to_bytes(), bytes[..32]);
        assert_eq!(item.shortened(88), Some(item));
        let short = item.shortened(60).unwrap();
        assert!(item.shortened(48).is_none() && short.shortened(80).is_none());
        let no_tail = ClipSettings {
            gain: 0.5,
            pitch: 0.0,
            stretch: 1.0,
            reversed: false,
            pan: 0.0,
        };
        assert_eq!(cut(60), no_tail);
        assert_eq!(
            cut(32),
            ClipSettings {
                gain: 1.0,
                ..no_tail
            }
        );
    }

    #[test]
    fn insert_flags() {
        let f = prefix(12, InsertFlags::from_bytes, InsertFlags::to_bytes);
        assert_eq!(f.unknown_0, [0, 1, 2, 3]);
        assert_eq!((f.len, f.flags), (12, at(4)));
        assert_eq!(f.unknown_8, [8, 9, 10, 11]);
        let mid = prefix(8, InsertFlags::from_bytes, InsertFlags::to_bytes);
        assert_eq!((mid.len, mid.flags, mid.unknown_8), (8, at(4), [0; 4]));
        let old = prefix(5, InsertFlags::from_bytes, InsertFlags::to_bytes);
        assert_eq!((old.len, old.flags), (5, 4));
        for len in [0, 4, 6, 7, 9, 11, 13] {
            assert!(InsertFlags::from_bytes(&ramp(len)).is_none(), "{len}");
        }
    }

    #[test]
    fn timestamp() {
        let mut bytes = ramp(8);
        bytes.extend_from_slice(&1234.5f64.to_le_bytes());
        let t = Timestamp::from_bytes(&bytes).unwrap();
        assert_eq!(t.unknown_0[..], ramp(8));
        assert_eq!(t.time_spent, 1234.5);
        assert_eq!(t.to_bytes(), bytes);
        // A NaN payload survives too.
        exact(16, Timestamp::from_bytes, Timestamp::to_bytes);
        bytes[8..].copy_from_slice(&[1, 0, 0, 0, 0, 0, 0xf8, 0x7f]);
        assert_eq!(Timestamp::from_bytes(&bytes).unwrap().to_bytes(), bytes);
    }

    #[test]
    fn track() {
        let t = prefix(70, Track::from_bytes, Track::to_bytes);
        assert_eq!(t.len, 70);
        assert_eq!((t.index, t.color, t.icon), (at(0), at(4), at(8)));
        assert_eq!(t.unknown_12, [12]);
        assert_eq!(t.unknown_13, [13, 14, 15, 16]);
        assert_eq!(t.unknown_17, [17, 18, 19, 20]);
        assert_eq!(t.unknown_21, [21]);
        assert_eq!(t.performance[..], ramp(46)[22..]);
        assert_eq!((t.grouped, t.locked), (46, 47));
        assert_eq!(t.unknown_48, [48]);
        assert_eq!(t.unknown_49, [49, 50, 51, 52]);
        assert_eq!(t.unknown_53, [53, 54, 55, 56]);
        assert_eq!(t.unknown_57, [57, 58, 59, 60]);
        assert_eq!(t.unknown_61, [61]);
        assert_eq!(t.unknown_62, [62, 63, 64, 65]);
        assert_eq!(t.unknown_66, [66, 67, 68, 69]);
        for len in [13, 47, 49, 61, 62, 66] {
            let short = prefix(len, Track::from_bytes, Track::to_bytes);
            assert_eq!(short.len, len);
            assert_eq!(short.locked, if len > 47 { 47 } else { 0 });
        }
        assert!(Track::from_bytes(&ramp(12)).is_none());
        assert!(Track::from_bytes(&ramp(71)).is_none());
    }

    #[test]
    fn arrangement_view_state() {
        let v = exact(
            28,
            ArrangementViewState::from_bytes,
            ArrangementViewState::to_bytes,
        );
        assert_eq!(v.arrangement, at(0));
        assert_eq!(v.unknown_4[..], ramp(12)[4..]);
        assert_eq!(v.unknown_12[..], ramp(20)[12..]);
        assert_eq!(v.unknown_20, [20, 21, 22, 23]);
        assert_eq!(v.unknown_24, [24, 25, 26, 27]);
    }

    #[test]
    fn project_pair_u32() {
        let p = exact(8, ProjectPairU32::from_bytes, ProjectPairU32::to_bytes);
        assert_eq!((p.unknown_0, p.unknown_4), ([0, 1, 2, 3], [4, 5, 6, 7]));
    }

    #[test]
    fn channel_sub_state() {
        let s = exact(43, ChannelSubState::from_bytes, ChannelSubState::to_bytes);
        assert_eq!(s.version, at(0));
        assert_eq!(s.unknown_4[..], ramp(41)[4..]);
        assert_eq!((s.unknown_41, s.unknown_42), ([41], [42]));
    }
}
