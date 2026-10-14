//! The event stream as FL's loader walks it: a handful of "current object"
//! pointers that a few events move, and every other event addressed to
//! whichever object is current for its scope. The rules are the "Object
//! delimiters" section of `docs/event-dictionary.md`.

use crate::codec::{self, Cx};
use crate::{
    Arrangement, Channel, ControllerLink, Insert, Item, LinkKind, Pattern, Placed, Project, Slot,
    TimeMarker, Track,
};
use flp::RawEvent;
use flp::event::{Id, Scope, Value, layout};

/// Markers go to the arrangement in files from before this version, even
/// with a pattern selected.
const MARKERS_IN_PATTERNS: u32 = 11_005_001;

/// Read a project out of its event stream. Nothing is rejected: an event
/// the model has no field for, or cannot decode, stays in its owner's
/// `order` as a raw event.
pub fn from_flp(file: &flp::Project) -> Project {
    let mut reader = Reader {
        project: Project::new(file.header),
        cx: Cx::default(),
        channel: None,
        pattern: None,
        arrangement: None,
        track: None,
        host: None,
        closed_inserts: 0,
        pending_name: None,
        pending_marker: Vec::new(),
    };
    for (pos, e) in file.events.iter().enumerate() {
        reader.event(pos, e);
    }
    reader.finish()
}

/// An object in the project, by its place in the model's vectors.
#[derive(Clone, Copy)]
enum Target {
    Project,
    Channel(usize),
    Pattern(usize),
    Arrangement(usize),
    Track(usize, usize),
    Insert(usize),
    Slot(usize, usize),
    Link(usize),
}

struct Reader {
    project: Project,
    cx: Cx,
    /// The loader's `chan`.
    channel: Option<usize>,
    /// The loader's `c4`. `None` before the first 65 and after a 233.
    pattern: Option<usize>,
    /// The loader's `arr`. `None` until something addresses an arrangement.
    arrangement: Option<usize>,
    /// The loader's `c8`, a track index. A 99 does not reset it.
    track: Option<u32>,
    /// The loader's `plug`: the channel after a 64, the plugin after a 212.
    host: Option<Target>,
    /// The loader's `e0` in file order: how many 147s have gone by.
    closed_inserts: usize,
    /// A 201 waiting for its 212.
    pending_name: Option<(usize, RawEvent)>,
    /// Marker events waiting for their 205.
    pending_marker: Vec<(usize, RawEvent)>,
}

/// The value of a fixed-size event.
fn number(e: &RawEvent) -> Option<u32> {
    match Value::decode(e)? {
        Value::U8(v) => Some(u32::from(v)),
        Value::U16(v) => Some(u32::from(v)),
        Value::U32(v) => Some(v),
        Value::Bytes(_) => None,
    }
}

impl Reader {
    fn event(&mut self, pos: usize, e: &RawEvent) {
        let id = Id::from(e.id);
        let Some(target) = self.target(pos, id, e) else {
            return;
        };
        self.give(target, pos, e);
        if id == Id::InsertOutput {
            self.closed_inserts += 1;
        } else if id == Id::DataFormatFlags {
            self.cx.data_format_flags = number(e).unwrap_or(0);
        } else if id == Id::FLVersion
            && self.cx.utf16.is_none()
            && let Some(version) = &self.project.writer_version
        {
            self.cx.set_version(version);
        }
    }

    /// Who an event belongs to, after moving the current-object pointers it
    /// moves. `None` when the event was parked or already placed.
    fn target(&mut self, pos: usize, id: Id, e: &RawEvent) -> Option<Target> {
        Some(match id {
            Id::ChannelNew => match number(e) {
                Some(index) => {
                    let channel = self.channel_at(index as u16);
                    self.channel = Some(channel);
                    self.host = Some(Target::Channel(channel));
                    Target::Channel(channel)
                }
                None => Target::Project,
            },
            Id::PatternNew => match number(e) {
                Some(index) => {
                    let pattern = self.pattern_at(index.min(999) as u16);
                    self.pattern = Some(pattern);
                    Target::Pattern(pattern)
                }
                None => Target::Project,
            },
            Id::ArrangementNew => match number(e) {
                Some(index) => {
                    let arrangement = self.arrangement_at(index as u16);
                    self.arrangement = Some(arrangement);
                    Target::Arrangement(arrangement)
                }
                None => Target::Project,
            },
            Id::ArrangementPlaylist => {
                self.pattern = None;
                Target::Arrangement(self.arrangement())
            }
            Id::TrackData => {
                let arrangement = self.arrangement();
                match layout::Track::from_bytes(&e.data) {
                    Some(data) => {
                        self.track = Some(data.index);
                        Target::Track(arrangement, self.track_at(arrangement, data.index))
                    }
                    None => Target::Arrangement(arrangement),
                }
            }
            // The first 199 decides how every string after it is read and
            // written. A second one must not change that halfway.
            Id::FLVersion if self.cx.utf16.is_some() => {
                self.raw(pos, e.clone());
                return None;
            }
            Id::PluginInternalName => {
                if let Some((pos, lost)) = self.pending_name.replace((pos, e.clone())) {
                    self.raw(pos, lost);
                }
                return None;
            }
            Id::PluginWrapper => {
                let wrapper = layout::PluginWrapper::from_bytes(&e.data);
                match wrapper.and_then(|wrapper| self.plugin(wrapper)) {
                    Some(target) => {
                        self.host = Some(target);
                        if let Some((pos, name)) = self.pending_name.take() {
                            self.give(target, pos, &name);
                        }
                        target
                    }
                    None => Target::Project,
                }
            }
            Id::SlotIndex => {
                let insert = self.insert();
                match number(e).map(|word| word & 0x7f) {
                    Some(slot) if slot < 10 => {
                        Target::Slot(insert, self.slot_at(insert, slot as i32))
                    }
                    _ => Target::Insert(insert),
                }
            }
            Id::TimeMarkerName => {
                self.close_marker(pos, e);
                return None;
            }
            Id::ControllerMIDI | Id::ControllerRemote => {
                let kind = if id == Id::ControllerMIDI {
                    LinkKind::Midi
                } else {
                    LinkKind::Remote
                };
                self.project.controller_links.push(ControllerLink {
                    kind,
                    ..Default::default()
                });
                Target::Link(self.project.controller_links.len() - 1)
            }
            Id::ControllerFormula => match self.project.controller_links.len() {
                0 => Target::Project,
                n => Target::Link(n - 1),
            },
            _ => match id.scope() {
                Scope::Channel => self.channel.map_or(Target::Project, Target::Channel),
                Scope::Plugin => self.host.unwrap_or(Target::Project),
                Scope::Pattern | Scope::ChannelPattern | Scope::ChannelPatternStep => {
                    self.pattern.map_or(Target::Project, Target::Pattern)
                }
                Scope::Insert | Scope::InsertSlot => Target::Insert(self.insert()),
                Scope::Arrangement => Target::Arrangement(self.arrangement()),
                Scope::Track => match self.track {
                    Some(index) => {
                        let arrangement = self.arrangement();
                        Target::Track(arrangement, self.track_at(arrangement, index))
                    }
                    None => Target::Project,
                },
                Scope::TimeMarker => {
                    self.pending_marker.push((pos, e.clone()));
                    return None;
                }
                Scope::ControllerLink
                | Scope::DisplayGroup
                | Scope::Project
                | Scope::Stream
                | Scope::Unknown => Target::Project,
            },
        })
    }

    fn give(&mut self, target: Target, pos: usize, e: &RawEvent) {
        let (p, cx) = (&mut self.project, &self.cx);
        match target {
            Target::Project => codec::give(p, pos, e, cx),
            Target::Channel(i) => codec::give(&mut p.channels[i], pos, e, cx),
            Target::Pattern(i) => codec::give(&mut p.patterns[i], pos, e, cx),
            Target::Arrangement(i) => codec::give(&mut p.arrangements[i], pos, e, cx),
            Target::Track(i, j) => codec::give(&mut p.arrangements[i].tracks[j], pos, e, cx),
            Target::Insert(i) => codec::give(&mut p.inserts[i], pos, e, cx),
            Target::Slot(i, j) => codec::give(&mut p.inserts[i].slots[j], pos, e, cx),
            Target::Link(i) => codec::give(&mut p.controller_links[i], pos, e, cx),
        }
    }

    /// Keep an event that lost its owner with the project.
    fn raw(&mut self, pos: usize, e: RawEvent) {
        let item = Item::Raw(e);
        self.project.order.push(Placed { pos, item });
    }

    /// Where a 212 puts its plugin. Flag bit 4 is a generator on the current
    /// channel, anything else an effect in the slot the wrapper names. The
    /// loader takes the flags as 1 when the payload stops before them.
    /// `None` when a generator has no channel to go to.
    fn plugin(&mut self, wrapper: layout::PluginWrapper) -> Option<Target> {
        let flags = if wrapper.len < 21 { 1 } else { wrapper.flags };
        if flags & 0x10 != 0 {
            self.channel.map(Target::Channel)
        } else {
            let insert = self.insert();
            Some(Target::Slot(insert, self.slot_at(insert, wrapper.slot)))
        }
    }

    /// A 205 closes a marker: the parked events and the name become one
    /// object, in the pattern when one is selected and the writer is new
    /// enough, otherwise in the arrangement.
    fn close_marker(&mut self, pos: usize, e: &RawEvent) {
        let mut marker = TimeMarker::default();
        for (pos, parked) in self.pending_marker.drain(..) {
            codec::give(&mut marker, pos, &parked, &self.cx);
        }
        codec::give(&mut marker, pos, e, &self.cx);
        match self.pattern {
            Some(pattern) if self.cx.version >= MARKERS_IN_PATTERNS => {
                self.project.patterns[pattern].time_markers.push(marker);
            }
            _ => {
                let arrangement = self.arrangement();
                self.project.arrangements[arrangement]
                    .time_markers
                    .push(marker);
            }
        }
    }

    fn channel_at(&mut self, index: u16) -> usize {
        let channels = &mut self.project.channels;
        channels
            .iter()
            .position(|c| c.index == index)
            .unwrap_or_else(|| {
                channels.push(Channel {
                    index,
                    ..Default::default()
                });
                channels.len() - 1
            })
    }

    fn pattern_at(&mut self, index: u16) -> usize {
        let patterns = &mut self.project.patterns;
        patterns
            .iter()
            .position(|p| p.index == index)
            .unwrap_or_else(|| {
                patterns.push(Pattern {
                    index,
                    ..Default::default()
                });
                patterns.len() - 1
            })
    }

    fn arrangement_at(&mut self, index: u16) -> usize {
        let arrangements = &mut self.project.arrangements;
        arrangements
            .iter()
            .position(|a| a.index == index)
            .unwrap_or_else(|| {
                arrangements.push(Arrangement {
                    index,
                    ..Default::default()
                });
                arrangements.len() - 1
            })
    }

    /// The current arrangement. A file without a 99 gets arrangement 0.
    fn arrangement(&mut self) -> usize {
        match self.arrangement {
            Some(arrangement) => arrangement,
            None => {
                let arrangement = self.arrangement_at(0);
                self.arrangement = Some(arrangement);
                arrangement
            }
        }
    }

    fn track_at(&mut self, arrangement: usize, index: u32) -> usize {
        let tracks = &mut self.project.arrangements[arrangement].tracks;
        // Tracks arrive in order, so the one wanted is nearly always last.
        tracks
            .iter()
            .rposition(|t| t.index == index)
            .unwrap_or_else(|| {
                tracks.push(Track {
                    index,
                    ..Default::default()
                });
                tracks.len() - 1
            })
    }

    /// The insert the next 147 will close.
    fn insert(&mut self) -> usize {
        if self.project.inserts.len() == self.closed_inserts {
            self.project.inserts.push(Insert::default());
        }
        self.closed_inserts
    }

    fn slot_at(&mut self, insert: usize, index: i32) -> usize {
        let slots = &mut self.project.inserts[insert].slots;
        slots
            .iter()
            .position(|s| s.index == index)
            .unwrap_or_else(|| {
                slots.push(Slot {
                    index,
                    ..Default::default()
                });
                slots.len() - 1
            })
    }

    /// A 201 with no 212 after it and marker events with no 205 after them
    /// have no object to belong to.
    fn finish(mut self) -> Project {
        let name = self.pending_name.take();
        let marker = std::mem::take(&mut self.pending_marker);
        for (pos, e) in name.into_iter().chain(marker) {
            self.raw(pos, e);
        }
        self.project
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Object;
    use crate::{ChannelKind, Error, to_flp};
    use flp::event::layout::{AutomationData, Point};

    fn ev(id: Id, data: &[u8]) -> RawEvent {
        RawEvent {
            id: id.0,
            data: data.to_vec(),
        }
    }

    fn word(id: Id, value: u16) -> RawEvent {
        ev(id, &value.to_le_bytes())
    }

    fn dword(id: Id, value: u32) -> RawEvent {
        ev(id, &value.to_le_bytes())
    }

    /// A UTF-16 string event, as files from 11.5.5 on store them.
    fn text(id: Id, s: &str) -> RawEvent {
        let data: Vec<u8> = s
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        ev(id, &data)
    }

    fn version(v: &str) -> RawEvent {
        ev(Id::FLVersion, format!("{v}\0").as_bytes())
    }

    /// A full-size 212: insert @0, slot @4, flags @16.
    fn wrapper(insert: i32, slot: i32, flags: u32) -> RawEvent {
        let mut data = vec![0; 52];
        data[0..4].copy_from_slice(&insert.to_le_bytes());
        data[4..8].copy_from_slice(&slot.to_le_bytes());
        data[16..20].copy_from_slice(&flags.to_le_bytes());
        ev(Id::PluginWrapper, &data)
    }

    /// A 238 for track `index`, in the shortest form the loader takes.
    fn track(index: u32) -> RawEvent {
        let mut data = vec![0; 13];
        data[0..4].copy_from_slice(&index.to_le_bytes());
        ev(Id::TrackData, &data)
    }

    /// Read a hand-built stream that starts with a 24.0.0 version event,
    /// and check that it writes back unchanged.
    fn read(events: Vec<RawEvent>) -> Project {
        let mut all = vec![version("24.0.0")];
        all.extend(events);
        read_all(all)
    }

    fn read_all(events: Vec<RawEvent>) -> Project {
        let header = flp::Header {
            format: 0,
            channel_count: 0,
            ppq: 96,
        };
        let file = flp::Project { header, events };
        let project = from_flp(&file);
        assert_eq!(to_flp(&project).as_ref(), Ok(&file));
        project
    }

    /// The IDs of an object's raw events.
    fn raw(order: &[Placed]) -> Vec<Id> {
        order
            .iter()
            .filter_map(|placed| match &placed.item {
                Item::Raw(e) => Some(Id::from(e.id)),
                Item::Field(_) => None,
            })
            .collect()
    }

    /// The IDs of an object's typed events.
    fn typed(order: &[Placed]) -> Vec<Id> {
        order
            .iter()
            .filter_map(|placed| match &placed.item {
                Item::Field(id) => Some(*id),
                Item::Raw(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_64_opens_a_channel_or_selects_it_again() {
        let p = read(vec![
            ev(Id::IsEnabled, &[1]),
            word(Id::ChannelNew, 0),
            ev(Id::ChannelType, &[0]),
            ev(Id::IsEnabled, &[1]),
            word(Id::ChannelNew, 1),
            ev(Id::ChannelType, &[2]),
            word(Id::ChannelNew, 0),
            ev(Id::IsLocked, &[1]),
            word(Id::Cutoff, 512),
        ]);
        assert_eq!(p.channels.len(), 2);
        let first = p.channel(0).unwrap();
        assert_eq!(first.kind, Some(ChannelKind::Sampler));
        assert_eq!(first.enabled, Some(true));
        assert_eq!(first.locked, Some(true));
        assert_eq!(
            typed(&first.order),
            [
                Id::ChannelNew,
                Id::ChannelType,
                Id::IsEnabled,
                Id::ChannelNew,
                Id::IsLocked
            ]
        );
        // A channel event without a field stays with its channel.
        assert_eq!(raw(&first.order), [Id::Cutoff]);
        // The second channel got no 0 and does not inherit the first one's.
        let second = p.channel(1).unwrap();
        assert_eq!(second.kind, Some(ChannelKind::Plugin));
        assert_eq!(second.enabled, None);
        // A channel event before any 64 has no channel.
        assert_eq!(raw(&p.order), [Id::IsEnabled]);
    }

    #[test]
    fn a_65_selects_a_pattern_by_index() {
        let note = [7; 24];
        let p = read(vec![
            word(Id::PatternNew, 1),
            text(Id::PatternName, "Intro"),
            word(Id::PatternNew, 2),
            word(Id::PatternNew, 1),
            ev(Id::PatternNotes, &note),
            word(Id::PatternNew, 1500),
            dword(Id::PatternLength, 384),
        ]);
        assert_eq!(p.patterns.len(), 3);
        // Both blocks for index 1 land in one pattern.
        let intro = p.pattern(1).unwrap();
        assert_eq!(intro.name.as_deref(), Some("Intro"));
        assert_eq!(intro.notes.len(), 1);
        assert_eq!(intro.notes[0].to_bytes(), note);
        assert!(p.pattern(2).unwrap().notes.is_empty());
        // The loader caps the index at 999. The 65 itself says 1500, so it
        // stays raw in the pattern it selected.
        let last = p.pattern(999).unwrap();
        assert_eq!(last.length, Some(384));
        assert_eq!(raw(&last.order), [Id::PatternNew]);
    }

    #[test]
    fn a_233_deselects_the_pattern() {
        let p = read(vec![
            word(Id::PatternNew, 1),
            ev(Id::ArrangementPlaylist, &[]),
            dword(Id::PatternColor, 0x112233),
        ]);
        assert_eq!(p.pattern(1).unwrap().color, None);
        assert_eq!(raw(&p.order), [Id::PatternColor]);
    }

    #[test]
    fn a_147_closes_an_insert() {
        let p = read(vec![
            text(Id::InsertName, "Master"),
            dword(Id::InsertOutput, u32::MAX),
            text(Id::InsertName, "Drums"),
            dword(Id::InsertInput, 3),
            ev(Id::FXProperties, &[4]),
            dword(Id::InsertOutput, 0),
            text(Id::InsertName, "Open"),
        ]);
        assert_eq!(p.inserts.len(), 3);
        assert_eq!(p.inserts[0].name.as_deref(), Some("Master"));
        assert_eq!(p.inserts[0].output, Some(-1));
        assert_eq!(p.inserts[0].input, None);
        assert_eq!(p.inserts[1].name.as_deref(), Some("Drums"));
        assert_eq!(p.inserts[1].input, Some(3));
        assert_eq!(p.inserts[1].output, Some(0));
        assert_eq!(raw(&p.inserts[1].order), [Id::FXProperties]);
        // Nothing opens an insert, so events after the last 147 start one.
        assert_eq!(p.inserts[2].name.as_deref(), Some("Open"));
        assert_eq!(p.inserts[2].output, None);
    }

    #[test]
    fn a_212_opens_a_plugin_on_the_channel_or_in_a_slot() {
        let p = read(vec![
            word(Id::ChannelNew, 0),
            text(Id::PluginInternalName, "Sytrus"),
            wrapper(0, 0, 0x10),
            text(Id::PluginName, "Lead"),
            ev(Id::PluginData, &[1, 2]),
            // The mixer: one insert with an effect in slot 3.
            text(Id::InsertName, "Master"),
            text(Id::PluginInternalName, "Fruity Reeverb 2"),
            wrapper(0, 3, 0),
            text(Id::PluginName, "Verb"),
            ev(Id::PluginData, &[9]),
            word(Id::SlotIndex, 3 | 0x100),
            word(Id::SlotIndex, 4),
            word(Id::SlotIndex, 10),
            dword(Id::InsertOutput, 0),
            // A 64 hands the plugin events back to the channel.
            word(Id::ChannelNew, 1),
            text(Id::PluginName, "Kick"),
        ]);
        let lead = p.channel(0).unwrap();
        assert_eq!(lead.plugin_name.as_deref(), Some("Sytrus"));
        assert_eq!(lead.wrapper.unwrap().flags, 0x10);
        assert_eq!(lead.name.as_deref(), Some("Lead"));
        assert_eq!(lead.plugin_state.as_deref(), Some(&[1, 2][..]));

        let master = &p.inserts[0];
        assert_eq!(master.slots.len(), 2);
        let verb = &master.slots[0];
        assert_eq!(verb.index, 3);
        assert_eq!(verb.plugin_name.as_deref(), Some("Fruity Reeverb 2"));
        assert_eq!(verb.wrapper.unwrap().slot, 3);
        assert_eq!(verb.name.as_deref(), Some("Verb"));
        assert_eq!(verb.plugin_state.as_deref(), Some(&[9][..]));
        assert_eq!(verb.flag(), Some(true));
        // A 98 flags its slot and opens nothing.
        let empty = &master.slots[1];
        assert_eq!((empty.index, empty.flag()), (4, Some(false)));
        assert_eq!(empty.wrapper, None);
        // The loader ignores a 98 for slot 10 and up.
        assert_eq!(raw(&master.order), [Id::SlotIndex]);

        assert_eq!(p.channel(1).unwrap().name.as_deref(), Some("Kick"));
    }

    #[test]
    fn a_201_waits_for_its_212() {
        // The 201 comes before the 64, and still belongs to the 212's plugin.
        let p = read(vec![
            text(Id::PluginInternalName, "lost"),
            text(Id::PluginInternalName, "3x Osc"),
            word(Id::ChannelNew, 0),
            wrapper(0, 0, 0x10),
            text(Id::PluginInternalName, "dangling"),
        ]);
        assert_eq!(p.channel(0).unwrap().plugin_name.as_deref(), Some("3x Osc"));
        // A 201 that another replaced, and one no 212 followed.
        assert_eq!(
            raw(&p.order),
            [Id::PluginInternalName, Id::PluginInternalName]
        );
    }

    #[test]
    fn a_plugin_event_needs_a_host() {
        let p = read(vec![
            dword(Id::PluginColor, 1),
            // A generator with no channel to sit on.
            wrapper(0, 0, 0x10),
            // A payload the loader's length check rejects.
            ev(Id::PluginWrapper, &[0; 53]),
            // Under 21 bytes the flags count as 1, an effect.
            ev(Id::PluginWrapper, &[0; 20]),
            dword(Id::PluginColor, 2),
        ]);
        assert_eq!(
            raw(&p.order),
            [Id::PluginColor, Id::PluginWrapper, Id::PluginWrapper]
        );
        let slot = &p.inserts[0].slots[0];
        assert_eq!(slot.wrapper.unwrap().len, 20);
        assert_eq!(slot.color, Some(2));
    }

    #[test]
    fn a_99_selects_an_arrangement_and_keeps_the_track() {
        let p = read(vec![
            word(Id::ArrangementNew, 0),
            text(Id::ArrangementName, "Verse"),
            track(5),
            text(Id::TrackName, "Bass"),
            word(Id::ArrangementNew, 1),
            // Still track 5, now of arrangement 1.
            text(Id::TrackName, "Also bass"),
            // 100 does not end the arrangement.
            word(Id::ArrangementsCurrent, 1),
            text(Id::ArrangementName, "Chorus"),
            word(Id::ArrangementNew, 0),
            ev(Id::ArrangementByte10, &[1]),
        ]);
        assert_eq!(p.arrangements.len(), 2);
        let verse = &p.arrangements[0];
        assert_eq!((verse.index, verse.name.as_deref()), (0, Some("Verse")));
        assert_eq!(verse.tracks[0].name.as_deref(), Some("Bass"));
        assert_eq!(raw(&verse.order), [Id::ArrangementByte10]);
        let chorus = &p.arrangements[1];
        assert_eq!((chorus.index, chorus.name.as_deref()), (1, Some("Chorus")));
        assert_eq!(chorus.tracks.len(), 1);
        assert_eq!(chorus.tracks[0].index, 5);
        assert_eq!(chorus.tracks[0].data, None);
        assert_eq!(chorus.tracks[0].name.as_deref(), Some("Also bass"));
        assert_eq!(raw(&p.order), [Id::ArrangementsCurrent]);
    }

    #[test]
    fn a_238_selects_a_track() {
        // No 99: the tracks go to an arrangement 0 that has no 99 to write.
        let p = read(vec![
            text(Id::TrackName, "no track yet"),
            track(0),
            ev(Id::TrackColorIsCustom, &[1]),
            track(1),
            text(Id::TrackName, "Two"),
            track(0),
            text(Id::TrackName, "One"),
            // Too short for the loader, so it selects nothing.
            ev(Id::TrackData, &[9; 12]),
            ev(Id::TrackColorIsCustom, &[0]),
        ]);
        let arrangement = &p.arrangements[0];
        assert_eq!(arrangement.index, 0);
        assert_eq!(typed(&arrangement.order), []);
        assert_eq!(raw(&arrangement.order), [Id::TrackData]);
        assert_eq!(arrangement.tracks.len(), 2);
        let one = &arrangement.tracks[0];
        assert_eq!(one.data.unwrap().index, 0);
        assert_eq!(one.name.as_deref(), Some("One"));
        // The second 238 and 43 for track 0 replaced the first.
        assert_eq!(one.color_is_custom, Some(false));
        assert_eq!(raw(&one.order), [Id::TrackData, Id::TrackColorIsCustom]);
        assert_eq!(arrangement.tracks[1].name.as_deref(), Some("Two"));
        assert_eq!(raw(&p.order), [Id::TrackName]);
    }

    #[test]
    fn an_automation_clip_is_its_channel() {
        let points = vec![Point::new(0.0, 0.25), Point::new(4.0, 1.0)];
        let data = AutomationData::new(points.clone());
        let p = read(vec![
            word(Id::ChannelNew, 0),
            ev(Id::ChannelType, &[5]),
            ev(Id::Automation, &data.to_bytes()),
            // A payload that is no automation data stays as it is.
            word(Id::ChannelNew, 1),
            ev(Id::ChannelType, &[5]),
            ev(Id::Automation, &[1, 2, 3]),
        ]);
        let clip = p.channel(0).unwrap();
        assert_eq!(clip.kind, Some(ChannelKind::Automation));
        assert_eq!(clip.automation.as_ref().unwrap().envelope, points);
        assert_eq!(clip.automation, Some(data));
        let broken = p.channel(1).unwrap();
        assert_eq!(broken.automation, None);
        assert_eq!(raw(&broken.order), [Id::Automation]);
    }

    #[test]
    fn a_226_or_227_makes_a_controller_link() {
        let p = read(vec![
            text(Id::ControllerFormula, "orphan"),
            ev(Id::ControllerMIDI, &[1; 20]),
            ev(Id::ControllerRemote, &[2; 20]),
            text(Id::ControllerFormula, "Input*2"),
        ]);
        assert_eq!(raw(&p.order), [Id::ControllerFormula]);
        assert_eq!(p.controller_links.len(), 2);
        let midi = &p.controller_links[0];
        assert_eq!((midi.kind, midi.formula.as_deref()), (LinkKind::Midi, None));
        assert_eq!(midi.link.unwrap().source, 0x0101_0101);
        // The formula attaches to the most recent link.
        let remote = &p.controller_links[1];
        assert_eq!(remote.kind, LinkKind::Remote);
        assert_eq!(remote.formula.as_deref(), Some("Input*2"));
        assert_eq!(
            typed(&remote.order),
            [Id::ControllerRemote, Id::ControllerFormula]
        );
    }

    #[test]
    fn a_205_closes_a_time_marker() {
        let p = read(vec![
            word(Id::PatternNew, 1),
            dword(Id::TimeMarkerPosition, 96),
            ev(Id::TimeMarkerNumerator, &[3]),
            ev(Id::TimeMarkerDenominator, &[4]),
            ev(Id::TimeMarkerFlag, &[1]),
            text(Id::TimeMarkerName, "in the pattern"),
            ev(Id::ArrangementPlaylist, &[]),
            dword(Id::TimeMarkerPosition, 192),
            text(Id::TimeMarkerName, "in the arrangement"),
            text(Id::TimeMarkerName, "bare"),
            dword(Id::TimeMarkerPosition, 288),
        ]);
        let pattern = p.pattern(1).unwrap();
        assert_eq!(pattern.time_markers.len(), 1);
        let marker = &pattern.time_markers[0];
        assert_eq!(marker.name.as_deref(), Some("in the pattern"));
        assert_eq!(marker.position, Some(96));
        assert_eq!((marker.numerator, marker.denominator), (Some(3), Some(4)));
        assert_eq!(raw(&marker.order), [Id::TimeMarkerFlag]);
        // After the 233 no pattern is selected.
        let markers = &p.arrangements[0].time_markers;
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].name.as_deref(), Some("in the arrangement"));
        assert_eq!(markers[0].position, Some(192));
        // The file gave this one no position. FL reuses 192 for it.
        assert_eq!(markers[1].name.as_deref(), Some("bare"));
        assert_eq!(markers[1].position, None);
        // No 205 followed the last position.
        assert_eq!(raw(&p.order), [Id::TimeMarkerPosition]);
    }

    #[test]
    fn markers_stay_out_of_patterns_before_11_5_1() {
        let events = |v: &str| {
            vec![
                version(v),
                word(Id::PatternNew, 1),
                ev(Id::TimeMarkerName, b"old\0"),
            ]
        };
        let old = read_all(events("11.5.0"));
        assert!(old.pattern(1).unwrap().time_markers.is_empty());
        assert_eq!(
            old.arrangements[0].time_markers[0].name.as_deref(),
            Some("old")
        );
        let new = read_all(events("11.5.1"));
        assert_eq!(new.pattern(1).unwrap().time_markers.len(), 1);
        assert!(new.arrangements.is_empty());
    }

    #[test]
    fn a_160_161_pair_rides_along_in_its_pattern() {
        let p = read(vec![
            word(Id::PatternNew, 3),
            dword(Id::PatternChannelIID, 7),
            dword(Id::Pattern161, 1),
        ]);
        assert_eq!(
            raw(&p.pattern(3).unwrap().order),
            [Id::PatternChannelIID, Id::Pattern161]
        );
    }

    #[test]
    fn display_groups_are_positional() {
        let p = read(vec![
            text(Id::DisplayGroupName, "Audio"),
            word(Id::ChannelNew, 0),
            text(Id::DisplayGroupName, "Unsorted"),
        ]);
        assert_eq!(p.display_groups, ["Audio", "Unsorted"]);
    }

    #[test]
    fn the_last_event_for_a_field_wins() {
        let p = read(vec![
            dword(Id::Tempo, 140_000),
            dword(Id::Tempo, 90_000),
            word(Id::ChannelNew, 0),
            text(Id::PluginName, "One Lead"),
            text(Id::ChannelName, "LEAD"),
        ]);
        assert_eq!(p.tempo_milli, Some(90_000));
        assert_eq!(raw(&p.order), [Id::Tempo]);
        // 203 and 192 set the same name. FL 11 files carry both.
        let channel = p.channel(0).unwrap();
        assert_eq!(channel.name.as_deref(), Some("LEAD"));
        assert_eq!(typed(&channel.order), [Id::ChannelNew, Id::ChannelName]);
        assert_eq!(raw(&channel.order), [Id::PluginName]);
    }

    #[test]
    fn a_second_record_list_stays_raw() {
        let p = read(vec![
            ev(Id::MixerParams, &[1; 12]),
            ev(Id::MixerParams, &[2; 24]),
        ]);
        assert_eq!(p.initial_values.len(), 1);
        assert_eq!(p.initial_values[0].unknown_0, [1; 4]);
        assert_eq!(raw(&p.order), [Id::MixerParams]);
    }

    #[test]
    fn only_the_first_199_counts() {
        // The second 199 would turn strings 8-bit. It stays raw, and the
        // title after it is still read and written as UTF-16.
        let p = read(vec![version("11.0.0"), text(Id::Title, "Song")]);
        assert_eq!(p.writer_version.as_deref(), Some("24.0.0"));
        assert_eq!(p.title.as_deref(), Some("Song"));
        assert_eq!(raw(&p.order), [Id::FLVersion]);
    }

    #[test]
    fn a_value_that_would_not_write_back_the_same_stays_raw() {
        // A lone high surrogate, as in the comments of one corpus project.
        let broken = ev(Id::Comments, &[0x3d, 0xd8, 0, 0]);
        let p = read(vec![
            broken,
            word(Id::ChannelNew, 0),
            ev(Id::IsEnabled, &[2]),
            ev(Id::Levels, &[0; 23]),
            ev(Id::PatternNotes, &[0; 25]),
        ]);
        assert_eq!(p.comments, None);
        let channel = p.channel(0).unwrap();
        assert_eq!((channel.enabled, channel.levels), (None, None));
        assert_eq!(raw(&channel.order), [Id::IsEnabled, Id::Levels]);
        assert_eq!(raw(&p.order), [Id::Comments, Id::PatternNotes]);
    }

    #[test]
    fn strings_follow_the_199() {
        // Before the 199 nothing says how a string is encoded.
        let p = read_all(vec![
            ev(Id::Title, b"early\0"),
            version("11.5.4"),
            ev(Id::Title, b"Caf\xe9\0"),
        ]);
        assert_eq!(p.writer_version.as_deref(), Some("11.5.4"));
        assert_eq!(p.title.as_deref(), Some("Caf\u{e9}"));
        assert_eq!(raw(&p.order), [Id::Title]);

        let p = read_all(vec![
            version("11.5.5"),
            text(Id::Title, "Caf\u{e9} \u{1f448}"),
        ]);
        assert_eq!(p.title.as_deref(), Some("Caf\u{e9} \u{1f448}"));

        // No 199 at all: every string stays raw.
        let p = read_all(vec![ev(Id::Title, b"x\0")]);
        assert_eq!((p.writer_version, p.title), (None, None));
    }

    #[test]
    fn tempo_and_time_signature() {
        let p = read(vec![
            dword(Id::Tempo, 128_500),
            word(Id::TempoCoarse, 99),
            ev(Id::ArrangementsTimeSigNum, &[3]),
        ]);
        assert_eq!(p.tempo(), Some(128.5));
        assert_eq!(p.time_signature(), None);

        let p = read(vec![
            word(Id::TempoCoarse, 140),
            word(Id::TempoFine, 250),
            ev(Id::ArrangementsTimeSigNum, &[3]),
            ev(Id::ArrangementsTimeSigBeat, &[4]),
        ]);
        assert_eq!(p.tempo(), Some(140.25));
        assert_eq!(p.time_signature(), Some((3, 4)));

        assert_eq!(read(vec![word(Id::TempoFine, 250)]).tempo(), None);
    }

    #[test]
    fn a_changed_field_changes_its_event() {
        let events = vec![
            version("24.0.0"),
            dword(Id::Tempo, 140_000),
            word(Id::ChannelNew, 0),
            text(Id::PluginName, "Kick"),
            ev(Id::IsEnabled, &[1]),
            word(Id::ChannelNew, 1),
        ];
        let mut p = read_all(events.clone());
        p.tempo_milli = Some(90_000);
        p.channels[0].name = Some("Snare".into());
        p.channels[0].enabled = None;
        // A field the file had no event for has no place to go.
        p.channels[0].locked = Some(true);
        let mut want = events.clone();
        want[1] = dword(Id::Tempo, 90_000);
        want[3] = text(Id::PluginName, "Snare");
        want.remove(4);
        assert_eq!(to_flp(&p).unwrap().events, want);

        // Until it gets one.
        p.channels[0].order.push(Placed {
            pos: 4,
            item: Item::Field(Id::IsLocked),
        });
        want.insert(4, ev(Id::IsLocked, &[1]));
        assert_eq!(to_flp(&p).unwrap().events, want);

        // Removing an object removes its events.
        p.channels.remove(0);
        want.drain(2..5);
        assert_eq!(to_flp(&p).unwrap().events, want);
    }

    #[test]
    fn a_string_the_file_cannot_hold_is_an_error() {
        let mut p = read_all(vec![version("11.0.0"), ev(Id::Title, b"old\0")]);
        p.title = Some("\u{1f448}".into());
        assert_eq!(to_flp(&p), Err(Error::Unencodable { id: Id::Title.0 }));
        p.title = Some("Caf\u{e9}".into());
        assert_eq!(to_flp(&p).unwrap().events[1], ev(Id::Title, b"Caf\xe9\0"));
        // Without a version nothing picks the encoding.
        p.writer_version = None;
        assert_eq!(to_flp(&p), Err(Error::Unencodable { id: Id::Title.0 }));
    }

    /// A field whose type has the wrong width for its event would never
    /// decode, and nothing else would notice.
    #[test]
    fn fixed_size_fields_have_their_event_s_width() {
        fn check<T: Object>(mut object: T, ids: &[Id]) {
            let cx = Cx::default();
            for &id in ids {
                let size = match id.kind() {
                    flp::event::Kind::U8 => 1,
                    flp::event::Kind::U16 => 2,
                    flp::event::Kind::U32 => 4,
                    flp::event::Kind::Var => continue,
                };
                let name = id.name().unwrap();
                assert!(object.take(&ev(id, &vec![0; size]), &cx), "{name}");
                assert_eq!(
                    object.payload(id, 0, &cx),
                    Ok(Some(vec![0; size])),
                    "{name}"
                );
            }
        }
        let header = flp::Header {
            format: 0,
            channel_count: 0,
            ppq: 96,
        };
        check(Project::new(header), Project::IDS);
        check(Channel::default(), Channel::IDS);
        check(Pattern::default(), Pattern::IDS);
        check(Arrangement::default(), Arrangement::IDS);
        check(Track::default(), Track::IDS);
        check(TimeMarker::default(), TimeMarker::IDS);
        check(Insert::default(), Insert::IDS);
        check(Slot::default(), Slot::IDS);
        check(ControllerLink::default(), ControllerLink::IDS);
    }
}
