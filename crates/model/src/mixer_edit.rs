//! What the mixer window changes: an insert's volume, pan and send levels
//! and a slot's switch and mix level, which are 225 records, and an
//! insert's routes, its switch and its name, which are its own events.
//!
//! [`InsertMix`](crate::InsertMix) reads every value set here, and each
//! setter leaves the project alone when the value is the one that reads
//! already. An insert the project does not have is skipped, and so is a
//! slot past 9.

use crate::codec::{Codec, Cx};
use crate::mixer::ENABLED;
use crate::rec::{LAST_INSERT, RecTarget, control};
use crate::{Insert, Item, Placed, Project};
use flp::event::{Id, layout};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;

/// The base of a mixer ID from 24.2.99 on, and before it.
const MIXER: u64 = 0x7000_0000;
const LEGACY_MIXER: u64 = 0x2000_0000;
/// Where the old encoding has the level of the send to insert 0.
const LEGACY_SEND: u16 = 0x1F40;
/// The insert the old encoding has the current track at.
const LEGACY_CURRENT: u16 = 126;
/// The ends of a fader, which an insert's volume and a send level have, of
/// a pan and of a slot's mix level.
const FADER: RangeInclusive<i32> = 0..=16_000;
const PAN: RangeInclusive<i32> = -6400..=6400;
const MIX: RangeInclusive<i32> = 0..=12_800;

/// An insert's own events in the order FL writes them, by ID: every one of
/// the 16,651 inserts in the corpus has the ones it has in this order.
const INSERT_EVENTS: [u32; 12] = [149, 42, 95, 204, 27, 236, 235, 165, 166, 49, 154, 147];

/// Give an insert the event `id` to write from its field.
///
/// An event of that ID which stayed raw becomes the field's. Otherwise the
/// event goes in front of the first of the insert's events that FL writes
/// after it, and at that event's position, which
/// [`to_flp`](crate::to_flp) keeps in this order. The 147 that closes the
/// insert is the last of them, so the new event is the same insert's when
/// the file is read back. An insert without any later event was not read
/// from a file, and nothing is added.
fn write_field(order: &mut Vec<Placed>, id: Id) {
    if order.iter().any(|placed| placed.item == Item::Field(id)) {
        return;
    }
    let is_raw = |placed: &&mut Placed| matches!(&placed.item, Item::Raw(e) if e.id == id.0);
    if let Some(placed) = order.iter_mut().rev().find(is_raw) {
        placed.item = Item::Field(id);
        return;
    }
    let rank = |event: u32| INSERT_EVENTS.iter().position(|known| *known == event);
    let own = rank(id.0);
    let later = |placed: &Placed| {
        let event = match &placed.item {
            Item::Field(id) => id.0,
            Item::Raw(e) => e.id,
        };
        rank(event) > own
    };
    if let Some(at) = order.iter().position(later) {
        let (pos, item) = (order[at].pos, Item::Field(id));
        order.insert(at, Placed { pos, item });
    }
}

impl Project {
    /// The ID a 225 record of this project has for `target`:
    /// [`Project::rec_target`] the other way round.
    ///
    /// A project that FL's loader runs the legacy rewrite on gets the old
    /// encoding, which is what every such project in the corpus has: base
    /// 0x20000000, the send levels from 0x1F40 and the current track as
    /// insert 126.
    ///
    /// `None` when no ID reads back as the target. The old encoding has
    /// none for insert 126, which reads as the current track, for an
    /// insert from 128 up, or for the level of a send to one from 128 up,
    /// the current track among them.
    pub fn rec_id(&self, target: RecTarget) -> Option<u32> {
        let id = match target {
            RecTarget::Global { control } => 0x4000_0000 + u64::from(control),
            RecTarget::Channel { index, control } => (u64::from(index) << 16) + u64::from(control),
            RecTarget::Other(id) => u64::from(id),
            RecTarget::Insert {
                index,
                slot,
                control,
            } => {
                let slot = u64::from(slot.unwrap_or(0)) << 16;
                let (mut base, mut index, mut control) = (MIXER, index, control);
                if self.legacy_rec_ids() {
                    base = LEGACY_MIXER;
                    if index == LAST_INSERT {
                        index = LEGACY_CURRENT;
                    }
                    if (control::SEND..=control::SEND + LAST_INSERT).contains(&control) {
                        control = control - control::SEND + LEGACY_SEND;
                    }
                }
                base + (u64::from(index) << 22) + slot + u64::from(control)
            }
        };
        let id = u32::try_from(id).ok()?;
        (self.rec_target(id) == target).then_some(id)
    }

    /// Whether [`to_flp`](crate::to_flp) writes the 225 records, after
    /// giving a project that has no 225 one behind the last of its own
    /// events. FL's loader replays the records once the whole file is
    /// read, so the place does not matter to it.
    ///
    /// A 225 that stayed a raw event says no: its records are not in
    /// [`Project::initial_values`] to keep, and a second 225 beside it
    /// would leave the loader with two lists.
    fn writes_records(&mut self) -> bool {
        let records = Id::MixerParams;
        if self.order.iter().any(|p| p.item == Item::Field(records)) {
            return true;
        }
        let is_raw = |placed: &Placed| matches!(&placed.item, Item::Raw(e) if e.id == records.0);
        if self.order.iter().any(is_raw) {
            return false;
        }
        let (pos, item) = (self.order.last().map_or(0, |p| p.pos), Item::Field(records));
        self.order.push(Placed { pos, item });
        true
    }

    /// Set the value of the last 225 record that addresses `target`, which
    /// is the one the loader's replay ends on. Without such a record a new
    /// one goes to the end of the list, under the ID [`Project::rec_id`]
    /// gives, and nothing happens when that gives none.
    fn set_record(&mut self, target: RecTarget, value: i32) {
        if !self.writes_records() {
            return;
        }
        let legacy = self.legacy_rec_ids();
        let mut records = self.initial_values.iter_mut().rev();
        if let Some(record) = records.find(|r| RecTarget::decode(r.rec_id, legacy) == target) {
            record.value = value;
        } else if let Some(rec_id) = self.rec_id(target) {
            self.initial_values.push(layout::InitialValue {
                unknown_0: [0; 4],
                rec_id,
                value,
            });
        }
    }

    /// Set a control of an insert the project has to `value`, held to
    /// `range`. `now` is what the control reads as, and `None` when the
    /// insert has no such control.
    fn set_control(
        &mut self,
        (index, slot, control): (u16, Option<u8>, u16),
        value: i32,
        range: RangeInclusive<i32>,
        now: Option<i32>,
    ) {
        let target = RecTarget::Insert {
            index,
            slot,
            control,
        };
        let value = value.clamp(*range.start(), *range.end());
        let there = self.insert_mix(index).insert().is_some();
        if there && now.is_some_and(|now| now != value) {
            self.set_record(target, value);
        }
    }

    /// The insert's volume, record 0x1FC0: 0 to 16000 with 12800 at 0 dB.
    pub fn set_insert_volume_raw(&mut self, insert: u16, volume: i32) {
        let now = self.insert_mix(insert).volume_raw();
        self.set_control((insert, None, control::VOLUME), volume, FADER, Some(now));
    }

    /// The insert's pan, record 0x1FC1: -6400 on the left to 6400 on the
    /// right.
    pub fn set_insert_pan_raw(&mut self, insert: u16, pan: i32) {
        let now = self.insert_mix(insert).pan_raw();
        self.set_control((insert, None, control::PAN), pan, PAN, Some(now));
    }

    /// The level of the send from insert `from` to insert `to`, record
    /// 0x2000 + `to`: 0 to 16000 with 12800 at unity. The level is kept
    /// whether or not the route is there, see [`Project::set_route`].
    pub fn set_send_level_raw(&mut self, from: u16, to: u16, level: i32) {
        let strip = self.insert_mix(from);
        let now = strip.send_level_raw(to);
        let there = from != to && self.insert_mix(to).insert().is_some();
        let send = (from, None, control::SEND.saturating_add(to));
        self.set_control(send, level, FADER, now.filter(|_| there));
    }

    /// The mix level of a slot, record 0x1F01: 0 to 12800, where 12800 is
    /// all the plugin's.
    pub fn set_slot_mix_raw(&mut self, insert: u16, slot: u8, mix: i32) {
        let now = self.insert_mix(insert).slot_mix_raw(slot);
        self.set_control((insert, Some(slot), control::SLOT_MIX), mix, MIX, now);
    }

    /// Switch a slot on or off, record 0x1F00.
    pub fn set_slot_enabled(&mut self, insert: u16, slot: u8, enabled: bool) {
        let now = self.insert_mix(insert).slot_enabled(slot).map(i32::from);
        let switch = (insert, Some(slot), control::SLOT_ENABLED);
        self.set_control(switch, i32::from(enabled), 0..=1, now);
    }

    fn insert_to_edit(&mut self, index: u16) -> Option<&mut Insert> {
        let position = self.insert_position(index)?;
        self.inserts.get_mut(position)
    }

    /// Switch the insert on or off: bit 3 of its 236, or of its last 27
    /// when the 236 is the 5-byte kind, which is where
    /// [`InsertMix::enabled`](crate::InsertMix::enabled) reads it. An
    /// insert with neither stays on. No insert of the corpus is one.
    pub fn set_insert_enabled(&mut self, insert: u16, enabled: bool) {
        let Some(insert) = self.insert_to_edit(insert) else {
            return;
        };
        let switch = |bits: u32| (bits & !ENABLED) | (u32::from(enabled) * ENABLED);
        if let Some(flags) = insert.flags.as_mut().filter(|flags| flags.len != 5) {
            flags.flags = switch(flags.flags);
            return;
        }
        let old = insert
            .order
            .iter_mut()
            .rev()
            .find_map(|placed| match &mut placed.item {
                Item::Raw(e) if e.id == Id::FXProperties.0 => e.data.first_mut(),
                _ => None,
            });
        if let Some(bits) = old {
            *bits = switch(u32::from(*bits)) as u8;
        }
    }

    /// Every insert whose sound gets to `insert`, through however many
    /// routes. A route from `insert` to one of them would close a loop.
    pub fn inserts_feeding(&self, insert: u16) -> BTreeSet<u16> {
        let numbers = (0..self.inserts.len()).filter_map(|place| self.insert_index(place));
        let routes: Vec<(u16, Vec<u16>)> = numbers
            .map(|from| (from, self.insert_mix(from).destinations()))
            .collect();
        let mut feeding = BTreeSet::new();
        let mut reached = vec![insert];
        while let Some(to) = reached.pop() {
            for (from, destinations) in &routes {
                if destinations.contains(&to) && feeding.insert(*from) {
                    reached.push(*from);
                }
            }
        }
        feeding
    }

    /// The byte of a 235 that switches the send to `dest`: the insert's
    /// number, as [`InsertMix::destinations`](crate::InsertMix::destinations)
    /// reads the bytes. `None` for the current track and for an insert the
    /// 235 of this project has no byte for.
    fn route_byte(&self, dest: u16) -> Option<usize> {
        let end = match self.version() {
            version if version < 12_009_000 => 104,
            _ if self.legacy_rec_ids() => usize::from(LEGACY_CURRENT),
            _ => usize::from(LAST_INSERT),
        };
        Some(usize::from(dest)).filter(|byte| *byte < end)
    }

    /// Make or remove the route from insert `from` to insert `to`, a byte
    /// of `from`'s 235. An insert without a 235 gets one behind its 236.
    ///
    /// As in FL, the master and the current track send nowhere, nothing is
    /// sent to the current track, and a route that would lead an insert
    /// back into itself is not made.
    pub fn set_route(&mut self, from: u16, to: u16, on: bool) {
        let ends = [0, LAST_INSERT].contains(&from) || to == LAST_INSERT;
        let there = self.insert_mix(to).insert().is_some();
        let Some(byte) = self.route_byte(to).filter(|_| there && !ends) else {
            return;
        };
        if self.insert_mix(from).destinations().contains(&to) == on {
            return;
        }
        if on && (from == to || self.inserts_feeding(from).contains(&to)) {
            return;
        }
        let Some(insert) = self.insert_to_edit(from) else {
            return;
        };
        // A byte the 235 does not have reads as what the insert reset
        // leaves: a send to the master, which is byte 0, and nothing else.
        let routing = insert.routing.get_or_insert_default();
        if routing.is_empty() {
            routing.push(1);
        }
        if routing.len() <= byte {
            routing.resize(byte + 1, 0);
        }
        routing[byte] = u8::from(on);
        write_field(&mut insert.order, Id::InsertRouting);
    }

    /// Name the insert, in its 204, which an insert without one gets in
    /// front of its 236. An empty name takes the 204 away, and the insert
    /// goes by its number again.
    ///
    /// A name the file's strings cannot hold is not set: a character above
    /// U+00FF in a file from before 11.5.5, or any name in a project
    /// without a writer version.
    pub fn set_insert_name(&mut self, insert: u16, name: &str) {
        let mut cx = Cx::default();
        if let Some(version) = &self.writer_version {
            cx.set_version(version);
        }
        let name = Some(name.to_owned()).filter(|name| !name.is_empty());
        let fits =
            |name: &String| !name.contains('\0') && name.encode(Id::InsertName, &cx).is_some();
        let Some(insert) = self.insert_to_edit(insert) else {
            return;
        };
        if insert.name == name || !name.as_ref().is_none_or(fits) {
            return;
        }
        insert.name = name;
        if insert.name.is_some() {
            write_field(&mut insert.order, Id::InsertName);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_flp, to_flp};
    use flp::RawEvent;

    const NEW: &str = "25.1.0.4858";
    const OLD: &str = "20.8.4.2576";

    fn ev(id: Id, data: &[u8]) -> RawEvent {
        RawEvent {
            id: id.0,
            data: data.to_vec(),
        }
    }

    /// A string as files from 11.5.5 on store it.
    fn text(id: Id, text: &str) -> RawEvent {
        let units = text.encode_utf16().chain([0]);
        ev(id, &units.flat_map(u16::to_le_bytes).collect::<Vec<u8>>())
    }

    fn record(rec_id: u32, value: i32) -> [u8; 12] {
        let mut bytes = [0; 12];
        bytes[4..8].copy_from_slice(&rec_id.to_le_bytes());
        bytes[8..].copy_from_slice(&value.to_le_bytes());
        bytes
    }

    /// The ID of a control of an insert in the new encoding.
    fn id(insert: u32, slot: u32, control: u16) -> u32 {
        0x7000_0000 + (insert << 22) + (slot << 16) + u32::from(control)
    }

    /// A mixer as FL writes one, cut down to the master, inserts 1 to 3
    /// and the current track: each insert switched on, with a 235 of one
    /// byte that sends 1 to 3 to the master. A 103 makes it a project in
    /// the new encoding. Insert 2 has a name, and the 225 has `records`.
    fn file(version: &str, records: &[(u32, i32)]) -> flp::Project {
        let mut events = vec![ev(Id::FLVersion, format!("{version}\0").as_bytes())];
        if version == NEW {
            events.push(ev(Id::FLP_FXTrackCount, &5u16.to_le_bytes()));
        }
        for insert in 0..5 {
            if insert == 2 {
                events.push(text(Id::InsertName, "Bass"));
            }
            let flags = [[0; 4], 0b0100_1100u32.to_le_bytes(), [0; 4]].concat();
            events.push(ev(Id::InsertFlags, &flags));
            events.push(ev(Id::InsertRouting, &[u8::from((1..4).contains(&insert))]));
            events.push(ev(Id::InsertOutput, &(-1i32).to_le_bytes()));
        }
        let records: Vec<u8> = records.iter().flat_map(|r| record(r.0, r.1)).collect();
        events.push(ev(Id::MixerParams, &records));
        events.push(ev(Id::from(133), &[0; 4]));
        let header = flp::Header {
            format: 0,
            channel_count: 0,
            ppq: 96,
        };
        flp::Project { header, events }
    }

    /// The file's project. A legacy one has the current track at place
    /// 126, so its five inserts are 0 to 4.
    fn project(file: &flp::Project) -> Project {
        let project = from_flp(file);
        assert_eq!(to_flp(&project).as_ref(), Ok(file));
        assert_eq!(to_flp(&project).unwrap().write(), file.write());
        project
    }

    /// What the project writes, which has to read back as the same events.
    fn written(project: &Project) -> Vec<RawEvent> {
        let file = to_flp(project).unwrap();
        assert_eq!(to_flp(&from_flp(&file)), Ok(file.clone()));
        file.events
    }

    /// The place of the file's only event with this ID.
    fn place(events: &[RawEvent], id: Id) -> usize {
        let mut places = events.iter().enumerate().filter(|(_, e)| e.id == id.0);
        let (place, _) = places.next().unwrap();
        assert!(places.next().is_none());
        place
    }

    /// The events with the payload of the 225 replaced.
    fn with_records(file: &flp::Project, records: &[(u32, i32)]) -> Vec<RawEvent> {
        let mut events = file.events.clone();
        let payload = records.iter().flat_map(|r| record(r.0, r.1)).collect();
        events[place(&file.events, Id::MixerParams)].data = payload;
        events
    }

    /// The events with the `nth` event of an ID replaced.
    fn with_event(file: &flp::Project, id: Id, nth: usize, data: &[u8]) -> Vec<RawEvent> {
        let mut events = file.events.clone();
        let mut own = events.iter_mut().filter(|e| e.id == id.0);
        own.nth(nth).unwrap().data = data.to_vec();
        events
    }

    #[test]
    fn a_fader_sets_the_last_record_for_its_control_and_no_other() {
        let records = [
            (0x4000_0000, 12_800),
            (id(1, 0, control::VOLUME), 100),
            (id(1, 0, control::PAN), 0),
            (id(2, 0, control::VOLUME), 12_800),
            (id(1, 0, control::VOLUME), 9000),
        ];
        let file = file(NEW, &records);
        let mut p = project(&file);
        p.set_insert_volume_raw(1, 4000);
        assert_eq!(p.insert_mix(1).volume_raw(), 4000);
        let mut want = records;
        want[4].1 = 4000;
        assert_eq!(written(&p), with_records(&file, &want));

        p.set_insert_pan_raw(1, -3200);
        want[2].1 = -3200;
        assert_eq!(written(&p), with_records(&file, &want));
        assert_eq!(
            from_flp(&to_flp(&p).unwrap()).insert_mix(1).pan_raw(),
            -3200
        );

        // A value past the end of the fader stops there.
        p.set_insert_volume_raw(2, 99_999);
        p.set_insert_pan_raw(1, -99_999);
        (want[3].1, want[2].1) = (16_000, -6400);
        assert_eq!(written(&p), with_records(&file, &want));
    }

    #[test]
    fn a_control_without_a_record_gets_one_at_the_end() {
        let records = [(0x4000_0000, 12_800), (id(1, 0, control::VOLUME), 9000)];
        let file = file(NEW, &records);
        let mut p = project(&file);
        p.set_insert_pan_raw(3, 640);
        p.set_send_level_raw(1, 2, 6400);
        p.set_slot_mix_raw(1, 9, 3200);
        p.set_slot_enabled(1, 0, false);
        p.set_insert_volume_raw(501, 0);
        let mut want = records.to_vec();
        want.extend([
            (id(3, 0, control::PAN), 640),
            (id(1, 0, control::SEND + 2), 6400),
            (id(1, 9, control::SLOT_MIX), 3200),
            (id(1, 0, control::SLOT_ENABLED), 0),
            (0xED40_1FC0, 0),
        ]);
        assert_eq!(written(&p), with_records(&file, &want));

        let read = from_flp(&to_flp(&p).unwrap());
        assert_eq!(read.insert_mix(3).pan_raw(), 640);
        assert_eq!(read.insert_mix(1).send_level_raw(2), Some(6400));
        assert_eq!(read.insert_mix(1).slot_mix_raw(9), Some(3200));
        assert_eq!(read.insert_mix(1).slot_enabled(0), Some(false));
        assert_eq!(read.insert_mix(501).volume_raw(), 0);
        // The new records are the last ones now, and are set where they are.
        p.set_slot_enabled(1, 0, true);
        p.set_slot_mix_raw(1, 9, 99_999);
        (want[4].1, want[5].1) = (12_800, 1);
        assert_eq!(written(&p), with_records(&file, &want));
    }

    #[test]
    fn a_legacy_project_gets_its_records_in_the_old_encoding() {
        let old = |insert: u32, control: u32| 0x2000_0000 + (insert << 22) + control;
        let records = [(0x4000_0000, 12_800), (old(1, 0x1FC0), 9000)];
        let file = file(OLD, &records);
        let mut p = project(&file);
        assert!(p.legacy_rec_ids());
        p.set_insert_volume_raw(1, 4000);
        p.set_insert_pan_raw(2, 640);
        p.set_send_level_raw(1, 2, 6400);
        p.set_slot_mix_raw(3, 4, 3200);
        let want = [
            (0x4000_0000, 12_800),
            (old(1, 0x1FC0), 4000),
            (old(2, 0x1FC1), 640),
            (old(1, 0x1F40 + 2), 6400),
            (old(3, (4 << 16) + 0x1F01), 3200),
        ];
        assert_eq!(written(&p), with_records(&file, &want));
        let read = from_flp(&to_flp(&p).unwrap());
        assert_eq!(read.insert_mix(1).volume_raw(), 4000);
        assert_eq!(read.insert_mix(2).pan_raw(), 640);
        assert_eq!(read.insert_mix(1).send_level_raw(2), Some(6400));
        assert_eq!(read.insert_mix(3).slot_mix_raw(4), Some(3200));
    }

    #[test]
    fn an_id_is_what_reads_back_as_its_target() {
        let insert = |index, slot, control| RecTarget::Insert {
            index,
            slot,
            control,
        };
        let new = project(&file(NEW, &[]));
        let old = project(&file(OLD, &[]));
        assert_eq!(new.rec_id(insert(3, None, control::PAN)), Some(0x70C0_1FC1));
        assert_eq!(old.rec_id(insert(3, None, control::PAN)), Some(0x20C0_1FC1));
        assert_eq!(
            new.rec_id(insert(501, Some(9), control::SLOT_MIX)),
            Some(0xED49_1F01)
        );
        // The old encoding has the current track at 126, and the sends
        // from 0x1F40.
        assert_eq!(
            old.rec_id(insert(501, None, control::VOLUME)),
            Some(0x3F80_1FC0)
        );
        assert_eq!(
            old.rec_id(insert(1, None, control::SEND + 127)),
            Some(0x2040_1FBF)
        );
        assert_eq!(
            new.rec_id(insert(1, None, control::SEND + 501)),
            Some(0x7040_21F5)
        );
        for none in [
            insert(1, None, control::SEND + 128),
            insert(126, None, control::VOLUME),
            insert(128, None, control::VOLUME),
            insert(300, None, control::PAN),
        ] {
            assert_eq!(old.rec_id(none), None, "{none:?}");
        }
        // The slot bits of a control that is not a slot's read as no slot.
        assert_eq!(new.rec_id(insert(1, Some(0), control::VOLUME)), None);
        assert_eq!(new.rec_id(insert(502, None, control::VOLUME)), None);
        let global = RecTarget::Global { control: 5 };
        assert_eq!(old.rec_id(global), Some(0x4000_0005));
        let channel = RecTarget::Channel {
            index: 3,
            control: 1,
        };
        assert_eq!(new.rec_id(channel), Some(0x0003_0001));
        assert_eq!(new.rec_id(RecTarget::Other(0x5000_0000)), Some(0x5000_0000));
    }

    #[test]
    fn a_value_that_reads_already_changes_nothing() {
        let records = [(0x4000_0000, 12_800), (id(1, 0, control::VOLUME), 9000)];
        let file = file(NEW, &records);
        let mut p = project(&file);
        let before = p.clone();
        p.set_insert_volume_raw(1, 9000);
        // What an untouched strip reads as needs no record.
        p.set_insert_volume_raw(2, 12_800);
        p.set_insert_pan_raw(1, 0);
        p.set_send_level_raw(1, 0, 12_800);
        p.set_slot_mix_raw(1, 0, 12_800);
        p.set_slot_enabled(1, 0, true);
        p.set_insert_enabled(1, true);
        p.set_route(1, 0, true);
        p.set_route(1, 2, false);
        p.set_insert_name(2, "Bass");
        p.set_insert_name(1, "");
        // An insert the project lacks, a slot past the tenth and a send
        // to the insert itself.
        p.set_insert_volume_raw(4, 1);
        p.set_insert_pan_raw(300, 1);
        p.set_send_level_raw(1, 4, 1);
        p.set_send_level_raw(1, 1, 1);
        p.set_send_level_raw(1, 502, 1);
        p.set_slot_mix_raw(1, 10, 1);
        p.set_slot_enabled(1, 10, false);
        p.set_insert_enabled(4, false);
        p.set_route(1, 4, true);
        p.set_route(4, 1, true);
        p.set_insert_name(4, "Lost");
        assert!(p == before);
        assert_eq!(written(&p), file.events);
    }

    #[test]
    fn a_project_without_a_225_gets_one_and_a_raw_225_stays() {
        let mut file = file(NEW, &[]);
        let at = place(&file.events, Id::MixerParams);
        let records = file.events.remove(at);
        let mut p = project(&file);
        p.set_insert_volume_raw(1, 4000);
        assert_eq!(p.insert_mix(1).volume_raw(), 4000);
        let events = written(&p);
        assert_eq!(events.len(), file.events.len() + 1);
        let value = record(id(1, 0, control::VOLUME), 4000);
        assert_eq!(events[place(&events, Id::MixerParams)].data, value);
        assert_eq!(
            from_flp(&to_flp(&p).unwrap()).insert_mix(1).volume_raw(),
            4000
        );

        // Thirteen bytes are no list of records.
        file.events.insert(at, records);
        file.events[at].data = vec![7; 13];
        let mut p = project(&file);
        p.set_insert_volume_raw(1, 4000);
        assert_eq!(p.insert_mix(1).volume_raw(), 12_800);
        assert_eq!(written(&p), file.events);
    }

    #[test]
    fn the_switch_is_bit_3_of_236_or_of_the_last_27() {
        let file = file(NEW, &[]);
        let mut p = project(&file);
        p.set_insert_enabled(2, false);
        assert!(!p.insert_mix(2).enabled());
        assert!(p.insert_mix(1).enabled());
        let off = [[0; 4], 0b0100_0100u32.to_le_bytes(), [0; 4]].concat();
        assert_eq!(written(&p), with_event(&file, Id::InsertFlags, 2, &off));
        p.set_insert_enabled(2, true);
        assert_eq!(written(&p), file.events);

        // A 5-byte 236 has no flags, and the bit is in the 27s in front.
        let mut old = file.clone();
        let flags = old.events.iter().position(|e| e.id == 236).unwrap();
        old.events[flags].data = vec![0, 0, 0, 0, 1];
        for bits in [0b0100u8, 0b1101] {
            old.events.insert(flags, ev(Id::FXProperties, &[bits]));
        }
        let mut p = project(&old);
        assert!(!p.insert_mix(0).enabled());
        p.set_insert_enabled(0, true);
        assert!(p.insert_mix(0).enabled());
        assert_eq!(
            written(&p),
            with_event(&old, Id::FXProperties, 1, &[0b1100])
        );
        p.set_insert_enabled(0, false);
        assert_eq!(written(&p), old.events);
    }

    #[test]
    fn a_route_is_a_byte_of_235() {
        let file = file(NEW, &[]);
        let mut p = project(&file);
        // The 235 grows to the byte, and keeps the send to the master.
        p.set_route(1, 3, true);
        assert_eq!(p.insert_mix(1).destinations(), [0, 3]);
        assert_eq!(
            written(&p),
            with_event(&file, Id::InsertRouting, 1, &[1, 0, 0, 1])
        );
        p.set_route(1, 0, false);
        p.set_route(1, 2, true);
        assert_eq!(p.insert_mix(1).destinations(), [2, 3]);
        assert_eq!(
            written(&p),
            with_event(&file, Id::InsertRouting, 1, &[0, 0, 1, 1])
        );
        let read = from_flp(&to_flp(&p).unwrap());
        assert_eq!(read.insert_mix(1).destinations(), [2, 3]);

        // The master sends nowhere. 2 and 3 get what 1 sends, so neither
        // sends to 1, and no insert sends to itself.
        let before = p.clone();
        assert_eq!(p.inserts_feeding(3), BTreeSet::from([1]));
        assert_eq!(p.inserts_feeding(0), BTreeSet::from([1, 2, 3]));
        assert!(p.inserts_feeding(1).is_empty());
        p.set_route(0, 3, true);
        p.set_route(3, 1, true);
        p.set_route(2, 1, true);
        p.set_route(2, 2, true);
        // The current track takes no part.
        p.set_route(1, 501, true);
        p.set_route(501, 1, true);
        assert!(p == before);
        // 2 can send to 3, and then 3 cannot send to 2.
        p.set_route(2, 3, true);
        assert_eq!(p.insert_mix(2).destinations(), [0, 3]);
        let before = p.clone();
        p.set_route(3, 2, true);
        assert!(p == before);
    }

    #[test]
    fn an_insert_without_a_235_gets_one_behind_its_236() {
        let mut file = file(NEW, &[]);
        // Insert 2 is the one with a name: 204, 236, 235, 147.
        let at = place(&file.events, Id::InsertName) + 2;
        assert_eq!(file.events.remove(at).id, Id::InsertRouting.0);
        let mut p = project(&file);
        assert_eq!(p.insert_mix(2).destinations(), [0]);
        p.set_route(2, 3, true);
        assert_eq!(p.insert_mix(2).destinations(), [0, 3]);
        let mut want = file.events.clone();
        want.insert(at, ev(Id::InsertRouting, &[1, 0, 0, 1]));
        assert_eq!(written(&p), want);

        // The master's 235 says so when it is the first byte to be set.
        let mut p = project(&file);
        p.set_route(2, 0, false);
        assert_eq!(p.insert_mix(2).destinations(), []);
        want[at].data = vec![0];
        assert_eq!(written(&p), want);
    }

    #[test]
    fn a_legacy_235_has_no_byte_from_the_current_track_on() {
        let mut p = project(&file(OLD, &[]));
        p.inserts.resize(127, Insert::default());
        p.set_route(1, 125, true);
        assert_eq!(p.insert_mix(1).destinations(), [0, 125]);
        let before = p.clone();
        p.set_route(1, 501, true);
        assert!(p == before);
        // Before 12.9.0 the current track is at byte 104.
        p.writer_version = Some("12.5.0.59".to_owned());
        let before = p.clone();
        p.set_route(1, 104, true);
        assert!(p == before);
        p.set_route(1, 103, true);
        assert_eq!(p.insert_mix(1).destinations(), [0, 103]);
    }

    #[test]
    fn a_name_is_the_204_in_front_of_the_236() {
        let file = file(NEW, &[]);
        let mut p = project(&file);
        p.set_insert_name(2, "Sub");
        let name = place(&file.events, Id::InsertName);
        let mut want = file.events.clone();
        want[name] = text(Id::InsertName, "Sub");
        assert_eq!(written(&p), want);

        // Insert 1 has none: its 236 is the first one after the master's.
        p.set_insert_name(1, "Drums");
        let flags = file.events.iter().position(|e| e.id == 147).unwrap() + 1;
        want.insert(flags, text(Id::InsertName, "Drums"));
        assert_eq!(written(&p), want);
        let read = from_flp(&to_flp(&p).unwrap());
        assert_eq!(
            read.insert_mix(1).insert().unwrap().name.as_deref(),
            Some("Drums")
        );
        assert_eq!(read.inserts[2].name.as_deref(), Some("Sub"));

        // No name is no 204.
        p.set_insert_name(1, "");
        p.set_insert_name(2, "");
        want.remove(flags);
        want.remove(name);
        assert_eq!(written(&p), want);
        assert_eq!(p.inserts[2].name, None);
    }

    #[test]
    fn a_name_the_file_cannot_hold_is_not_set() {
        // Strings are 8-bit up to 11.5.4.
        let mut narrow = project(&file("11.0.3", &[]));
        let before = narrow.clone();
        narrow.set_insert_name(1, "Snare €");
        narrow.set_insert_name(1, "Sn\0are");
        assert!(narrow == before);
        narrow.set_insert_name(1, "Café");
        assert_eq!(narrow.inserts[1].name.as_deref(), Some("Café"));
        let read = from_flp(&to_flp(&narrow).unwrap());
        assert_eq!(read.inserts[1].name.as_deref(), Some("Café"));

        let mut wide = project(&file(NEW, &[]));
        wide.set_insert_name(1, "Snare €");
        assert_eq!(written(&wide).len(), file(NEW, &[]).events.len() + 1);
        // Without a 199 nothing says how a string is stored.
        wide.writer_version = None;
        let before = wide.clone();
        wide.set_insert_name(3, "Hat");
        assert!(wide == before);
    }
}
