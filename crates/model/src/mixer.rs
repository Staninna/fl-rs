//! Mixer inserts and their effect slots, and the mixer state that lives
//! outside an insert's own events.

use crate::codec::object;
use crate::rec::{self, RecTarget, control};
use crate::{Item, Placed, Project};
use flp::event::{Id, layout};
use std::collections::BTreeSet;

/// One mixer insert. Nothing opens an insert: the insert-scope events
/// address the current one, and 147 `InsertID.Output` closes it and moves on
/// to the next.
///
/// [`Project::inserts`](crate::Project::inserts) is in file order. FL's
/// loader numbers them otherwise: it counts up from 0 and puts the last one
/// at 501. [`Channel::insert`](crate::Channel::insert) and a slot wrapper's
/// `insert` use the loader's numbers.
///
/// There is no volume or pan here. The dictionary has no insert-scope event
/// for either. They are among the 225 records in
/// [`Project::initial_values`](crate::Project::initial_values), keyed by
/// the loader's insert number. [`Project::insert_mix`] reads them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Insert {
    /// 204 `InsertID.Name`: string.
    pub name: Option<String>,
    /// 236 `InsertID.Flags`: 12 bytes, two u32 around a u32 of flag bits,
    /// or 5 or 8 bytes in old files.
    pub flags: Option<layout::InsertFlags>,
    /// 149 `InsertID.Color`: RGBA u32.
    pub color: Option<u32>,
    /// 42 `InsertColorIsCustom`: bool.
    pub color_is_custom: Option<bool>,
    /// 95 `InsertID.Icon`: i16.
    pub icon: Option<i16>,
    /// 154 `InsertID.Input`: i32.
    pub input: Option<i32>,
    /// 147 `InsertID.Output`: i32. This is the event that closes the insert.
    pub output: Option<i32>,
    /// 235 `InsertID.Routing`: u8 array, one byte per destination insert,
    /// nonzero for a send. [`InsertMix::destinations`] reads the bytes as
    /// the loader's insert numbers.
    pub routing: Option<Vec<u8>>,
    /// Effect slots in the order their first event arrived.
    pub slots: Vec<Slot>,
    /// Every event of the insert in the order it arrived.
    pub order: Vec<Placed>,
}

object!(Insert {
    one name = InsertName;
    one flags = InsertFlags;
    one color = InsertColor;
    one color_is_custom = InsertColorIsCustom;
    one icon = InsertIcon;
    one input = InsertInput;
    one output = InsertOutput;
    one routing = InsertRouting;
});

/// One effect slot of an insert, with the plugin loaded into it.
///
/// A 212 `PluginID.Wrapper` without flag bit 4 opens the plugin, and the
/// plugin-scope events that follow go to it until the next 64 or 212. A 98
/// `SlotID.Index` sets a flag on the slot it names and selects nothing.
///
/// The loader finds the insert through the wrapper's own `insert` field.
/// Here the slot hangs off the insert that is current when the 212 arrives.
/// The two agree for all 3662 effect plugins in the corpus.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Slot {
    /// The slot number: 212 @4, or bits 0 to 6 of a 98.
    pub index: i32,
    /// 98 `SlotID.Index`: u16, bits 0 to 6 the slot and bit 8 a flag. See
    /// [`Slot::flag`].
    pub index_word: Option<u16>,
    /// 201 `PluginID.InternalName`: string, the name FL instantiates the
    /// plugin by. Parked until the 212 that follows it.
    pub plugin_name: Option<String>,
    /// 212 `PluginID.Wrapper`: 52-byte struct.
    pub wrapper: Option<layout::PluginWrapper>,
    /// 203 `PluginID.Name` or 192 `ChannelID._Name`: string. The later one
    /// when a slot has both.
    pub name: Option<String>,
    /// 128 `PluginID.Color`: RGBA u32.
    pub color: Option<u32>,
    /// 41 `PluginColorIsCustom`: bool.
    pub color_is_custom: Option<bool>,
    /// 155 `PluginID.Icon`: u32.
    pub icon: Option<u32>,
    /// 213 `PluginID.Data`: plugin-specific bytes.
    pub plugin_state: Option<Vec<u8>>,
    /// Every event of the slot in the order it arrived.
    pub order: Vec<Placed>,
}

object!(Slot {
    one index_word = SlotIndex;
    one plugin_name = PluginInternalName;
    one wrapper = PluginWrapper;
    one name = PluginName | ChannelName;
    one color = PluginColor;
    one color_is_custom = PluginColorIsCustom;
    one icon = PluginIcon;
    one plugin_state = PluginData;
});

impl Slot {
    /// Bit 8 of the slot's 98.
    pub fn flag(&self) -> Option<bool> {
        Some(self.index_word? & 0x100 != 0)
    }
}

/// Bit 3 of 236's flags word and of 27: the insert is switched on. The
/// dictionary gives the bit no name. The loader hands it to `FUN_012c1810`,
/// which stores it at insert `+0x18` and starts a fade, and the insert reset
/// calls the same function with 1. PyFLP calls the bit `Enabled`.
pub(crate) const ENABLED: u32 = 1 << 3;

/// Bit 0 of 236's flags word and of 27: the insert's polarity is reversed.
/// The loader stores it at insert `+0x390`, and `FUN_0127b050` turns the
/// insert's gain negative for it (`docs/mixer-insert.md`).
pub(crate) const POLARITY: u32 = 1 << 0;

/// Bit 1 of 236's flags word and of 27: the insert swaps its left and
/// right outputs. The loader stores it at insert `+0x391`, and
/// `FUN_0127b050` swaps the outputs of the insert's gain matrix for it.
pub(crate) const SWAP: u32 = 1 << 1;

/// The three raw values of one band of an insert's EQ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EqBand {
    /// Hundredths of a dB.
    pub gain: i32,
    /// 0..=65536, which `FUN_0127b390` maps to
    /// `(101^(raw / 65536) - 1) * 159.9 + 10` Hz: 10 Hz to 16 kHz.
    pub frequency: i32,
    /// 65536ths: a shelf's slope or a peak's Q, `docs/mixer-insert.md`.
    pub width: i32,
}

/// What an insert's mixer strip is set to: the 225 records that address the
/// insert, and its routes and its switch from the insert's own events.
///
/// Each value is the last record for its control. A control without a
/// record reads as what FL's insert reset leaves in it, so a strip of an
/// insert the project does not have reads as an untouched one. `index` is
/// the loader's insert number: 0 the master, 501 the current track.
#[derive(Debug, Clone, Copy)]
pub struct InsertMix<'a> {
    project: &'a Project,
    index: u16,
}

impl Project {
    /// The 199 version as FL's loader compares it, 0 without one. This is
    /// the `writer_version` that the methods of a
    /// [`PlaylistItem`](crate::PlaylistItem) take.
    pub fn version(&self) -> u32 {
        let version = self.writer_version.as_deref();
        version.and_then(flp::event::version_number).unwrap_or(0)
    }

    /// Whether the 225 records use the old ID encoding, which is also the
    /// test for the old insert numbering: the project is from before FL
    /// 24.2.99 or has no 103.
    pub fn legacy_rec_ids(&self) -> bool {
        self.version() < rec::NEW_MIXER_VERSION || self.insert_count.is_none()
    }

    /// What a 225 record's ID addresses in this project.
    pub fn rec_target(&self, id: u32) -> RecTarget {
        RecTarget::decode(id, self.legacy_rec_ids())
    }

    /// The place in [`Project::inserts`] of the insert the loader puts at
    /// 501. Inserts before it keep their place as their number: 104 before
    /// 12.9.0, 126 in a legacy project, else one less than the 103 count.
    fn current_track_position(&self) -> usize {
        let version = self.version();
        let mut last = usize::from(rec::LAST_INSERT);
        if version < 12_009_000 {
            last = 104;
        }
        if self.legacy_rec_ids() {
            last = last.min(126);
        }
        if version >= rec::NEW_MIXER_VERSION || self.insert_count.is_some() {
            // Without a 103 the loader's mixer has 127 inserts.
            let count = self.insert_count.map_or(127, |n| n.clamp(3, 502));
            last = last.min(usize::from(count) - 1);
        }
        last
    }

    /// The loader's number for `inserts[position]`. `None` past the insert
    /// that lands on 501.
    ///
    /// The index jumps of files older than 8.5.0 are not applied.
    pub fn insert_index(&self, position: usize) -> Option<u16> {
        let last = self.current_track_position();
        match position {
            p if p < last => Some(p as u16),
            p if p == last => Some(rec::LAST_INSERT),
            _ => None,
        }
    }

    /// The place in [`Project::inserts`] of the insert with this loader
    /// number, which is [`Project::insert_index`] the other way round.
    /// `None` for a number no place has. The place may be past the end of
    /// the list, in a project too short to have the insert.
    pub fn insert_position(&self, index: u16) -> Option<usize> {
        let last = self.current_track_position();
        match usize::from(index) {
            i if i < last => Some(i),
            i if i == usize::from(rec::LAST_INSERT) => Some(last),
            _ => None,
        }
    }

    /// The mixer strip of the insert with this loader number.
    pub fn insert_mix(&self, index: u16) -> InsertMix<'_> {
        InsertMix {
            project: self,
            index,
        }
    }

    /// The main volume, record 0x40000000: 0..=16000 with 12800 at 0 dB,
    /// which is also what a project without the record has.
    pub fn main_volume_raw(&self) -> i32 {
        let want = RecTarget::Global {
            control: control::MAIN_VOLUME,
        };
        self.record(want).unwrap_or(12_800)
    }

    /// The value of the last 225 record that addresses `want`.
    fn record(&self, want: RecTarget) -> Option<i32> {
        let legacy = self.legacy_rec_ids();
        let mut records = self.initial_values.iter().rev();
        let found = records.find(|r| RecTarget::decode(r.rec_id, legacy) == want);
        found.map(|r| r.value)
    }
}

impl<'a> InsertMix<'a> {
    /// The loader's insert number.
    pub fn index(&self) -> u16 {
        self.index
    }

    /// The insert's own events, when the project has the insert.
    pub fn insert(&self) -> Option<&'a Insert> {
        let position = self.project.insert_position(self.index)?;
        self.project.inserts.get(position)
    }

    fn record(&self, slot: Option<u8>, control: u16) -> Option<i32> {
        self.project.record(RecTarget::Insert {
            index: self.index,
            slot,
            control,
        })
    }

    /// 0x1FC0: 0..=16000 with 12800 at 0 dB. 12800 without a record.
    pub fn volume_raw(&self) -> i32 {
        self.record(None, control::VOLUME).unwrap_or(12_800)
    }

    /// 0x1FC1: -6400 (left) to 6400 (right). 0 without a record.
    pub fn pan_raw(&self) -> i32 {
        self.record(None, control::PAN).unwrap_or(0)
    }

    /// 0x1FC2: -64..=64. 0 without a record.
    pub fn separation_raw(&self) -> i32 {
        self.record(None, control::SEPARATION).unwrap_or(0)
    }

    /// 0x2000 + `dest`: the level of the send to insert `dest`, 0..=16000
    /// with 12800 at unity. `None` when `dest` is past 501.
    ///
    /// This is the level only. Whether the route exists is in
    /// [`InsertMix::destinations`], and FL writes a level for a route only
    /// when it has been moved. Without a record the level is the insert
    /// reset's: 12800, or 0 towards the current track. A file from before
    /// 11.5.10 also gets 0 from inserts 0..=99 towards 100..=103, the four
    /// send tracks of that mixer, which the loader zeroes when it reads the
    /// 199.
    pub fn send_level_raw(&self, dest: u16) -> Option<i32> {
        if dest > rec::LAST_INSERT {
            return None;
        }
        let old_send =
            self.project.version() < 11_005_010 && self.index < 100 && (100..=103).contains(&dest);
        let default = if dest == rec::LAST_INSERT || old_send {
            0
        } else {
            12_800
        };
        Some(self.record(None, control::SEND + dest).unwrap_or(default))
    }

    /// 0x1F01 of a slot: 0..=12800, where 12800 is all wet. 12800 without a
    /// record, `None` for a slot past 9.
    pub fn slot_mix_raw(&self, slot: u8) -> Option<i32> {
        let found = self.record(Some(slot), control::SLOT_MIX);
        (slot < 10).then(|| found.unwrap_or(12_800))
    }

    /// 0x1F00 of a slot: on for a value above 0. On without a record, `None`
    /// for a slot past 9.
    pub fn slot_enabled(&self, slot: u8) -> Option<bool> {
        let found = self.record(Some(slot), control::SLOT_ENABLED);
        (slot < 10).then(|| found.is_none_or(|value| value > 0))
    }

    /// 0x1FD0, 0x1FD8 and 0x1FE0 plus `band`. Without records a band has no
    /// gain, a width of 17500 and FL's 90 Hz, 1.5 kHz or 8 kHz as its
    /// frequency. `None` for a band past 2.
    pub fn eq_band(&self, band: u8) -> Option<EqBand> {
        let frequency = *[5777, 33_145, 55_825].get(usize::from(band))?;
        let record = |first: u16| self.record(None, first + u16::from(band));
        Some(EqBand {
            gain: record(control::EQ_GAIN).unwrap_or(0),
            frequency: record(control::EQ_FREQUENCY).unwrap_or(frequency),
            width: record(control::EQ_WIDTH).unwrap_or(17_500),
        })
    }

    /// The insert's flag bits: the flags word of 236, or the byte of the
    /// last 27 in a file whose 236 is the 5-byte kind and has no flags.
    /// What FL's insert reset leaves, bit 3 alone, when the insert has
    /// neither.
    pub fn flag_bits(&self) -> u32 {
        let Some(insert) = self.insert() else {
            return ENABLED;
        };
        if let Some(flags) = insert.flags.filter(|flags| flags.len != 5) {
            return flags.flags;
        }
        let old = insert
            .order
            .iter()
            .rev()
            .find_map(|placed| match &placed.item {
                Item::Raw(e) if e.id == Id::FXProperties.0 => e.data.first().copied(),
                _ => None,
            });
        old.map_or(ENABLED, u32::from)
    }

    /// Whether the insert is switched on: bit 3 of
    /// [`InsertMix::flag_bits`].
    pub fn enabled(&self) -> bool {
        self.flag_bits() & ENABLED != 0
    }

    /// Whether the insert's polarity is reversed: bit 0 of
    /// [`InsertMix::flag_bits`].
    pub fn polarity_reversed(&self) -> bool {
        self.flag_bits() & POLARITY != 0
    }

    /// Whether the insert swaps left and right: bit 1 of
    /// [`InsertMix::flag_bits`].
    pub fn swapped(&self) -> bool {
        self.flag_bits() & SWAP != 0
    }

    /// The inserts this one sends to, in rising order.
    ///
    /// Routes are the bytes of 235 `InsertID.Routing`: byte `i` switches
    /// the send to insert `i` on or off. Before 12.9.0 byte 104 is the
    /// current track, in other legacy projects byte 126 is. A destination
    /// 235 has no byte for keeps what the insert reset gives it, which is a
    /// send to the master (0) from every insert but the master and the
    /// current track, and nothing else.
    ///
    /// 147 `InsertID.Output` and 154 `InsertID.Input` name audio device
    /// ports and route nothing inside the mixer. The byte shifts of files
    /// older than 8.5.0 are not applied.
    pub fn destinations(&self) -> Vec<u16> {
        let mut on = BTreeSet::new();
        if self.index != 0 && self.index != rec::LAST_INSERT {
            on.insert(0);
        }
        let version = self.project.version();
        let legacy = self.project.legacy_rec_ids();
        let routing = self.insert().and_then(|insert| insert.routing.as_deref());
        for (i, byte) in routing.unwrap_or_default().iter().enumerate().take(502) {
            let dest = match i {
                i if version < 12_009_000 && i > 103 => i + 397,
                i if version >= 12_009_000 && legacy && i > 125 => i + 375,
                i => i,
            };
            if dest > usize::from(rec::LAST_INSERT) {
                continue;
            }
            if *byte != 0 {
                on.insert(dest as u16);
            } else {
                on.remove(&(dest as u16));
            }
        }
        on.into_iter().collect()
    }

    /// The inserts that send to this one, in rising order: the sidechain
    /// inputs of a plugin on this insert, the first being input 1.
    ///
    /// The file has no mark for a sidechain route. FL's "Sidechain to this
    /// track" makes a route like any other, a 1 in the sender's 235, and
    /// turns the route's send level down to 0 in a 225 record, so the
    /// sender is not heard here. A plugin counts every route into its
    /// insert all the same, the ones that are heard too: of the 175 Fruity
    /// Limiters with a sidechain input in the corpus, 162 name a route
    /// whose level is 0 when the inserts are counted this way.
    ///
    /// An insert the project does not have sends to the master, as
    /// [`InsertMix::destinations`] has it, so the master's inputs are the
    /// whole mixer's. That the order is the rising one is inferred: a 235
    /// has a byte for each destination and nothing else to order by.
    pub fn sidechain_sources(&self) -> Vec<u16> {
        let sends_here = |from: &u16| {
            let destinations = self.project.insert_mix(*from).destinations();
            destinations.binary_search(&self.index).is_ok()
        };
        (0..=rec::LAST_INSERT).filter(sends_here).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flp::RawEvent;

    /// A project with `inserts` empty inserts, saved by `version`, with a
    /// 103 when `count` is set.
    fn project(version: &str, count: Option<u16>, inserts: usize) -> Project {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 0,
            ppq: 96,
        });
        project.writer_version = Some(version.to_owned());
        project.insert_count = count;
        project.inserts = vec![Insert::default(); inserts];
        project
    }

    fn set(project: &mut Project, rec_id: u32, value: i32) {
        project.initial_values.push(layout::InitialValue {
            unknown_0: [0; 4],
            rec_id,
            value,
        });
    }

    #[test]
    fn the_last_insert_of_the_file_is_the_current_track() {
        // 24.2.99 on with a 103: the count decides.
        let new = project("25.1.0.4858", Some(18), 18);
        assert!(!new.legacy_rec_ids());
        assert_eq!(new.insert_index(0), Some(0));
        assert_eq!(new.insert_index(16), Some(16));
        assert_eq!(new.insert_index(17), Some(501));
        assert_eq!(new.insert_index(18), None);
        assert!(std::ptr::eq(
            new.insert_mix(501).insert().unwrap(),
            &new.inserts[17]
        ));
        assert!(new.insert_mix(17).insert().is_none());
        // 12.9.0 up to 24.2.99, or no 103: 127 inserts.
        for legacy in [
            project("20.8.4.2576", None, 127),
            project("25.1.0.4858", None, 127),
        ] {
            assert!(legacy.legacy_rec_ids());
            assert_eq!(legacy.insert_index(125), Some(125));
            assert_eq!(legacy.insert_index(126), Some(501));
        }
        // Before 12.9.0: 105 inserts.
        let old = project("11.0.3", None, 105);
        assert_eq!(old.insert_index(103), Some(103));
        assert_eq!(old.insert_index(104), Some(501));
        assert!(std::ptr::eq(
            old.insert_mix(501).insert().unwrap(),
            &old.inserts[104]
        ));
        // A project too short to have its current track.
        assert!(
            project("20.8.4.2576", None, 4)
                .insert_mix(501)
                .insert()
                .is_none()
        );
    }

    #[test]
    fn a_strip_without_records_is_an_untouched_one() {
        let p = project("25.1.0.4858", Some(18), 18);
        let strip = p.insert_mix(3);
        assert_eq!(strip.index(), 3);
        assert_eq!(
            (strip.volume_raw(), strip.pan_raw(), strip.separation_raw()),
            (12_800, 0, 0)
        );
        assert_eq!(strip.send_level_raw(0), Some(12_800));
        assert_eq!(strip.send_level_raw(501), Some(0));
        assert_eq!(strip.send_level_raw(502), None);
        assert_eq!(strip.slot_mix_raw(9), Some(12_800));
        assert_eq!(strip.slot_mix_raw(10), None);
        assert_eq!(strip.slot_enabled(0), Some(true));
        assert_eq!(strip.slot_enabled(10), None);
        let band = |gain, frequency| EqBand {
            gain,
            frequency,
            width: 17_500,
        };
        assert_eq!(strip.eq_band(0), Some(band(0, 5777)));
        assert_eq!(strip.eq_band(2), Some(band(0, 55_825)));
        assert_eq!(strip.eq_band(3), None);
        assert!(strip.enabled());
        assert_eq!(p.main_volume_raw(), 12_800);
    }

    #[test]
    fn a_strip_reads_the_last_record_for_each_control() {
        let mut p = project("25.1.0.4858", Some(18), 18);
        let insert_3 = 0x7000_0000 + (3 << 22);
        set(&mut p, insert_3 + 0x1FC0, 100);
        set(&mut p, insert_3 + 0x1FC0, 9000);
        set(&mut p, insert_3 + 0x1FC1, -6400);
        set(&mut p, insert_3 + 0x1FC2, 64);
        set(&mut p, insert_3 + 0x2000 + 7, 4000);
        set(&mut p, insert_3 + (2 << 16) + 0x1F00, 0);
        set(&mut p, insert_3 + (2 << 16) + 0x1F01, 6400);
        set(&mut p, insert_3 + 0x1FD1, -1800);
        set(&mut p, insert_3 + 0x1FD9, 30_000);
        set(&mut p, insert_3 + 0x1FE1, 9);
        set(&mut p, 0x4000_0000, 16_000);
        // Insert 4 and the old encoding are somebody else's.
        set(&mut p, 0x7000_0000 + (4 << 22) + 0x1FC0, 1);
        set(&mut p, 0x2000_0000 + (3 << 22) + 0x1FC0, 2);
        let strip = p.insert_mix(3);
        assert_eq!(
            (strip.volume_raw(), strip.pan_raw(), strip.separation_raw()),
            (9000, -6400, 64)
        );
        assert_eq!(strip.send_level_raw(7), Some(4000));
        assert_eq!(strip.send_level_raw(8), Some(12_800));
        assert_eq!(strip.slot_enabled(2), Some(false));
        assert_eq!(strip.slot_enabled(1), Some(true));
        assert_eq!(strip.slot_mix_raw(2), Some(6400));
        let band = strip.eq_band(1).unwrap();
        assert_eq!((band.gain, band.frequency, band.width), (-1800, 30_000, 9));
        assert_eq!(p.insert_mix(4).volume_raw(), 1);
        assert_eq!(p.main_volume_raw(), 16_000);
    }

    #[test]
    fn a_legacy_project_s_records_are_remapped_first() {
        let mut p = project("20.8.4.2576", None, 127);
        set(&mut p, 0x2000_0000 + (3 << 22) + 0x1FC0, 9000);
        set(&mut p, 0x2000_0000 + (3 << 22) + 0x1F40 + 7, 4000);
        set(&mut p, 0x2000_0000 + (126 << 22) + 0x1FC1, 50);
        assert_eq!(
            p.rec_target(0x2000_0000 + (126 << 22) + 0x1FC1),
            RecTarget::Insert {
                index: 501,
                slot: None,
                control: control::PAN
            }
        );
        assert_eq!(p.insert_mix(3).volume_raw(), 9000);
        assert_eq!(p.insert_mix(3).send_level_raw(7), Some(4000));
        assert_eq!(p.insert_mix(501).pan_raw(), 50);
        // With a 103 and a new version the same IDs address nothing.
        p.writer_version = Some("24.2.99.4720".to_owned());
        p.insert_count = Some(127);
        assert_eq!(p.insert_mix(3).volume_raw(), 12_800);
    }

    #[test]
    fn a_file_from_before_11_5_10_sends_nothing_to_its_send_tracks() {
        let old = project("11.0.3", None, 105);
        assert_eq!(old.insert_mix(5).send_level_raw(100), Some(0));
        assert_eq!(old.insert_mix(5).send_level_raw(103), Some(0));
        assert_eq!(old.insert_mix(5).send_level_raw(0), Some(12_800));
        assert_eq!(old.insert_mix(5).send_level_raw(99), Some(12_800));
        assert_eq!(old.insert_mix(100).send_level_raw(101), Some(12_800));
        let later = project("11.5.10", None, 105);
        assert_eq!(later.insert_mix(5).send_level_raw(100), Some(12_800));
    }

    #[test]
    fn routes_are_the_bytes_of_235_over_a_send_to_the_master() {
        let mut p = project("25.1.0.4858", Some(18), 18);
        // No 235: the master and the current track send nowhere, the rest
        // to the master.
        assert_eq!(p.insert_mix(0).destinations(), []);
        assert_eq!(p.insert_mix(501).destinations(), []);
        assert_eq!(p.insert_mix(4).destinations(), [0]);
        assert_eq!(p.insert_mix(300).destinations(), [0]);
        // Bytes past the end of a short 235 keep that.
        p.inserts[4].routing = Some(vec![]);
        assert_eq!(p.insert_mix(4).destinations(), [0]);
        p.inserts[4].routing = Some(vec![0, 0, 1, 0, 0, 1]);
        assert_eq!(p.insert_mix(4).destinations(), [2, 5]);
        p.inserts[4].routing = Some(vec![1, 0, 7]);
        assert_eq!(p.insert_mix(4).destinations(), [0, 2]);
    }

    #[test]
    fn the_inserts_that_send_to_one_are_its_sidechain_inputs() {
        let mut p = project("25.1.0.4858", Some(18), 18);
        // Insert 7 and insert 2 send to insert 4, and the level of a route
        // does not count.
        p.inserts[7].routing = Some(vec![0, 0, 0, 0, 1]);
        p.inserts[2].routing = Some(vec![1, 0, 0, 0, 1]);
        set(&mut p, 0x7000_0000 + (7 << 22) + 0x2000 + 4, 0);
        assert_eq!(p.insert_mix(4).sidechain_sources(), [2, 7]);
        assert_eq!(p.insert_mix(7).sidechain_sources(), []);
        // Every insert sends to the master until its 235 says otherwise,
        // the ones the project does not have too. The current track does
        // not.
        let mut all: Vec<u16> = (1..=500).collect();
        all.retain(|index| *index != 7);
        assert_eq!(p.insert_mix(0).sidechain_sources(), all);
    }

    #[test]
    fn a_legacy_235_has_the_current_track_at_byte_126_or_104() {
        let mut bytes = vec![0; 127];
        (bytes[0], bytes[104], bytes[125], bytes[126]) = (1, 1, 1, 1);
        let mut p = project("20.8.4.2576", None, 127);
        p.inserts[1].routing = Some(bytes.clone());
        assert_eq!(p.insert_mix(1).destinations(), [0, 104, 125, 501]);
        // Before 12.9.0 byte 104 is the current track and 105 is nothing.
        p.writer_version = Some("12.5.0.59".to_owned());
        assert_eq!(p.insert_mix(1).destinations(), [0, 501]);
        // A new project's bytes are insert numbers as they are.
        p.writer_version = Some("25.1.0.4858".to_owned());
        p.insert_count = Some(127);
        assert_eq!(p.insert_mix(1).destinations(), [0, 104, 125, 126]);
    }

    #[test]
    fn an_insert_is_switched_off_by_bit_3_of_236_or_of_27() {
        let flags = |flags: u32, len: usize| layout::InsertFlags {
            unknown_0: [0; 4],
            flags,
            unknown_8: [0; 4],
            len,
        };
        let mut p = project("20.8.4.2576", None, 127);
        p.inserts[2].flags = Some(flags(0b1100, 12));
        assert!(p.insert_mix(2).enabled());
        p.inserts[2].flags = Some(flags(0b0100, 12));
        assert!(!p.insert_mix(2).enabled());
        p.inserts[2].flags = Some(flags(0b0100, 8));
        assert!(!p.insert_mix(2).enabled());
        // A 5-byte 236 has no flags. The last 27 has the bit then.
        p.inserts[2].flags = Some(flags(0, 5));
        assert!(p.insert_mix(2).enabled());
        for (pos, bits) in [0b1100u8, 0b0100].into_iter().enumerate() {
            p.inserts[2].order.push(Placed {
                pos,
                item: Item::Raw(RawEvent {
                    id: Id::FXProperties.0,
                    data: vec![bits],
                }),
            });
        }
        assert!(!p.insert_mix(2).enabled());
        p.inserts[2].order.pop();
        assert!(p.insert_mix(2).enabled());
    }

    #[test]
    fn bits_0_and_1_reverse_the_polarity_and_swap_the_sides() {
        let flags = |flags: u32, len: usize| layout::InsertFlags {
            unknown_0: [0; 4],
            flags,
            unknown_8: [0; 4],
            len,
        };
        let mut p = project("20.8.4.2576", None, 127);
        let strip = p.insert_mix(2);
        assert!(!strip.polarity_reversed() && !strip.swapped());
        assert_eq!(p.insert_mix(400).flag_bits(), ENABLED);
        p.inserts[2].flags = Some(flags(0b1101, 12));
        let strip = p.insert_mix(2);
        assert!(strip.polarity_reversed() && !strip.swapped());
        p.inserts[2].flags = Some(flags(0b0110, 12));
        let strip = p.insert_mix(2);
        assert!(!strip.polarity_reversed() && strip.swapped() && !strip.enabled());
        // With a 5-byte 236 they come from the last 27, as the switch does.
        p.inserts[2].flags = Some(flags(0, 5));
        p.inserts[2].order.push(Placed {
            pos: 0,
            item: Item::Raw(RawEvent {
                id: Id::FXProperties.0,
                data: vec![0b1011],
            }),
        });
        assert_eq!(p.insert_mix(2).flag_bits(), 0b1011);
        assert!(p.insert_mix(2).swapped() && p.insert_mix(2).polarity_reversed());
    }
}
