//! An event stream that no FL wrote never panics the reader, the writer or
//! the views the model offers of what it read.
//!
//! The streams start from one built in code that has an event for most of
//! the model's fields: channels of three kinds, a pattern with notes, an
//! arrangement with playlist items and tracks, inserts with slots, a time
//! marker, controller links and initial values. Each case then drops,
//! duplicates, moves or swaps events, gives an event another ID, and cuts,
//! grows or overwrites payloads, with indices set to their extremes. What
//! [`model::from_flp`] reads must write back through [`model::to_flp`] to
//! the stream it was read from, as the crate promises for any stream.
//!
//! The cases come from a small fixed-seed generator, so a failure repeats.

use flp::event::layout::{AutomationData, Point};
use flp::event::{Id, Kind};
use flp::{Header, RawEvent};
use model::{ItemSource, Note, PlaylistItem, Project, from_flp, to_flp};

/// SplitMix64: small, fast and the same on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n`, which must not be 0.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

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

fn text(id: Id, s: &str) -> RawEvent {
    let data: Vec<u8> = s
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    ev(id, &data)
}

/// A full-size 212: insert @0, slot @4, flags @16.
fn wrapper(slot: i32, flags: u32) -> RawEvent {
    let mut data = vec![0; 52];
    data[4..8].copy_from_slice(&slot.to_le_bytes());
    data[16..20].copy_from_slice(&flags.to_le_bytes());
    ev(Id::PluginWrapper, &data)
}

/// A 20-byte controller link from `source` to `target`.
fn link(id: Id, source: u32, target: u32) -> RawEvent {
    let mut data = vec![0; 20];
    data[0..4].copy_from_slice(&source.to_le_bytes());
    data[8..12].copy_from_slice(&target.to_le_bytes());
    ev(id, &data)
}

/// A 12-byte 225 record.
fn initial(rec_id: u32, value: i32) -> [u8; 12] {
    let mut record = [0; 12];
    record[4..8].copy_from_slice(&rec_id.to_le_bytes());
    record[8..].copy_from_slice(&value.to_le_bytes());
    record
}

/// The stream every case starts from.
fn fixture() -> Vec<RawEvent> {
    let note = |position, channel, key| Note {
        position,
        channel,
        length: 48,
        key,
        ..Note::default()
    };
    let notes = [note(0, 0, 60), note(96, 1, 64), note(192, 0, 67)];
    let items = [
        PlaylistItem::new(ItemSource::Pattern(1), 0, 384),
        PlaylistItem::new(ItemSource::Channel(2), 384, 384),
    ];
    let automation = AutomationData::new(vec![Point::new(0.0, 0.25), Point::new(4.0, 1.0)]);
    let mut levels = vec![0; 24];
    levels[4..8].copy_from_slice(&12_800u32.to_le_bytes());
    let mut track = vec![0; 70];
    track[0..4].copy_from_slice(&1u32.to_le_bytes());
    let records: Vec<u8> = [initial(0x4000_0000, 10_000), initial(0x0001_0000, 6400)]
        .concat()
        .to_vec();
    vec![
        ev(Id::FLVersion, b"24.2.99.4720\0"),
        dword(Id::DataFormatFlags, 10),
        text(Id::Title, "Song"),
        dword(Id::Tempo, 140_000),
        ev(Id::ArrangementsTimeSigNum, &[4]),
        ev(Id::ArrangementsTimeSigBeat, &[4]),
        word(Id::FLP_FXTrackCount, 127),
        // A sampler on insert 1.
        word(Id::ChannelNew, 0),
        ev(Id::ChannelType, &[0]),
        ev(Id::IsEnabled, &[1]),
        text(Id::SamplePath, "%FLStudioFactoryData%/kick.wav"),
        ev(Id::Levels, &levels),
        word(Id::FLP_TargetFXTrack16, 1),
        // A generator plugin.
        word(Id::ChannelNew, 1),
        ev(Id::ChannelType, &[2]),
        text(Id::PluginInternalName, "3x Osc"),
        wrapper(0, 0x10),
        text(Id::PluginName, "Lead"),
        ev(Id::PluginData, &[1, 2, 3, 4]),
        // An automation clip bound to the master volume.
        word(Id::ChannelNew, 2),
        ev(Id::ChannelType, &[5]),
        ev(Id::Automation, &automation.to_bytes()),
        word(Id::PatternNew, 1),
        text(Id::PatternName, "Intro"),
        dword(Id::PatternLength, 384),
        ev(Id::PatternNotes, &Note::list_to_bytes(&notes)),
        dword(Id::TimeMarkerPosition, 96),
        text(Id::TimeMarkerName, "Drop"),
        word(Id::ArrangementNew, 0),
        text(Id::ArrangementName, "Main"),
        ev(
            Id::ArrangementPlaylist,
            &PlaylistItem::list_to_bytes(&items),
        ),
        ev(Id::TrackData, &track),
        text(Id::TrackName, "Drums"),
        word(Id::ArrangementsCurrent, 0),
        ev(Id::MixerParams, &records),
        text(Id::InsertName, "Master"),
        dword(Id::InsertOutput, u32::MAX),
        text(Id::InsertName, "Drums"),
        text(Id::PluginInternalName, "Fruity Fast Dist"),
        wrapper(3, 0),
        ev(Id::PluginData, &[9; 8]),
        word(Id::SlotIndex, 3),
        ev(Id::InsertFlags, &[0; 12]),
        ev(Id::InsertRouting, &[1, 0, 1]),
        dword(Id::InsertOutput, 0),
        link(Id::ControllerRemote, 2 << 16, 0x4000_0000),
        text(Id::ControllerFormula, "Input"),
        link(Id::ControllerMIDI, 0x0001_8000, 0x0000_0001),
        text(Id::DisplayGroupName, "Unsorted"),
    ]
}

fn header(ppq: u16) -> Header {
    Header {
        format: 0,
        channel_count: 3,
        ppq,
    }
}

/// A value that tends to break things: an end of its range, or random.
fn extreme(rng: &mut Rng, width: usize) -> Vec<u8> {
    let mut v = match rng.below(6) {
        0 => vec![0; width],
        1 => vec![0xff; width],
        2 => vec![0x7f; width],
        3 => vec![0x80; width],
        _ => rng.bytes(width),
    };
    if rng.below(3) == 0 && width > 1 {
        // The top bit only: the most negative value of a signed field.
        v.fill(0);
        v[width - 1] = 0x80;
    }
    v
}

/// One random change to the stream.
fn mutate(rng: &mut Rng, events: &mut Vec<RawEvent>, ids: &[u32]) {
    if events.is_empty() {
        let id = ids[rng.below(ids.len())];
        let len = rng.below(8);
        events.push(RawEvent {
            id,
            data: rng.bytes(len),
        });
        return;
    }
    let i = rng.below(events.len());
    match rng.below(10) {
        // Drop.
        0 => {
            events.remove(i);
        }
        // Duplicate, here or anywhere.
        1 => {
            let at = rng.below(events.len() + 1);
            events.insert(at, events[i].clone());
        }
        // Move.
        2 => {
            let e = events.remove(i);
            let at = rng.below(events.len() + 1);
            events.insert(at, e);
        }
        // Swap.
        3 => {
            let j = rng.below(events.len());
            events.swap(i, j);
        }
        // Another ID on the same payload, which may then have the wrong
        // length for a fixed-size ID.
        4 => {
            events[i].id = if rng.below(2) == 0 {
                ids[rng.below(ids.len())]
            } else {
                rng.byte().into()
            };
        }
        // Cut the payload.
        5 => {
            let len = rng.below(events[i].data.len() + 1);
            events[i].data.truncate(len);
        }
        // Grow it.
        6 => {
            let len = 1 + rng.below(30);
            let more = rng.bytes(len);
            events[i].data.extend(more);
        }
        // An index or count at an extreme: overwrite 1, 2 or 4 bytes
        // anywhere in the payload.
        7 | 8 => {
            let width = [1, 2, 4][rng.below(3)];
            let data = &mut events[i].data;
            if data.len() >= width {
                let at = rng.below(data.len() - width + 1);
                let value = extreme(rng, width);
                data[at..at + width].copy_from_slice(&value);
            }
        }
        // Random bytes of the right size for the ID.
        _ => {
            let len = match flp::event::kind(events[i].id) {
                Kind::U8 => 1,
                Kind::U16 => 2,
                Kind::U32 => 4,
                Kind::Var => rng.below(100),
            };
            events[i].data = rng.bytes(len);
        }
    }
}

/// Everything the model works out from what it read. None may panic.
fn look_at_everything(project: &Project) {
    let ppq = project.header.ppq;
    let version = project.version();
    let _ = (project.tempo(), project.time_signature());
    let _ = (project.song_length_ticks(), project.legacy_rec_ids());
    let _ = project.main_volume_raw();
    for binding in project.automation_bindings() {
        let _ = binding.target.raw_range();
    }
    for channel in &project.channels {
        let _ = (channel.volume(), channel.pan(), channel.insert());
        let _ = (channel.envelope_on(), channel.time_stretch_on());
        let _ = (channel.tracks_mods(), channel.echo_delay_on());
        let _ = channel.automation_range();
    }
    for pattern in &project.patterns {
        for ppq in [ppq, 1] {
            let steps = pattern.steps(0, ppq);
            assert!(steps.len() <= 1 << 16, "{} steps", steps.len());
        }
        for channel in [0, 1, 2, 500, u16::MAX] {
            for step in [0, 1, 4, 15, u32::MAX] {
                let _ = pattern.step_note(channel, step, ppq);
            }
        }
        for note in &pattern.notes {
            let _ = (note.fine_pitch_cents(), note.slide(), note.color());
        }
    }
    for arrangement in &project.arrangements {
        for item in &arrangement.items {
            let _ = (item.source(), item.track(version), item.trim(), item.clip());
        }
    }
    for value in &project.initial_values {
        let target = project.rec_target(value.rec_id);
        let _ = (target.raw_range(), target.controller_output());
        let _ = target.channel_output();
    }
    for link in project.controller_links.iter().filter_map(|l| l.link) {
        let _ = project.link_target(link.source).controller_output();
        let _ = project.link_target(link.target).raw_range();
    }
    // Each call asks every insert for its destinations, so two will do.
    for index in [0, 1] {
        let _ = project.insert_mix(index).sidechain_sources();
    }
    for position in 0..project.inserts.len() + 2 {
        let _ = project.insert_index(position);
    }
    for index in [0, 1, 2, 125, 126, 127, 500, 501, 502, 9999, u16::MAX] {
        let _ = project.insert_position(index);
        let mix = project.insert_mix(index);
        let _ = (mix.insert(), mix.volume_raw(), mix.pan_raw());
        let _ = (mix.separation_raw(), mix.flag_bits(), mix.enabled());
        let _ = (mix.polarity_reversed(), mix.swapped());
        let _ = mix.destinations();
        for dest in [0, 1, 501, u16::MAX] {
            let _ = mix.send_level_raw(dest);
        }
        for slot in [0, 9, 10, u8::MAX] {
            let _ = (mix.slot_mix_raw(slot), mix.slot_enabled(slot));
        }
        for band in [0, 2, 3, u8::MAX] {
            let _ = mix.eq_band(band);
        }
    }
}

/// Read `events`, look at what came out, and write it back.
fn check(events: Vec<RawEvent>, ppq: u16) {
    let file = flp::Project {
        header: header(ppq),
        events,
    };
    let project = from_flp(&file);
    look_at_everything(&project);
    let written = to_flp(&project);
    // A deleted note record is the one thing the writer leaves out.
    let deleted = |p: &model::Pattern| p.notes.iter().any(Note::is_deleted);
    if !project.patterns.iter().any(deleted) {
        assert_eq!(written.as_ref(), Ok(&file));
    }
}

#[test]
fn the_fixture_reads_and_writes_back() {
    let file = flp::Project {
        header: header(96),
        events: fixture(),
    };
    let project = from_flp(&file);
    assert_eq!(project.channels.len(), 3);
    assert_eq!(project.patterns[0].notes.len(), 3);
    assert_eq!(project.arrangements[0].items.len(), 2);
    assert_eq!(project.inserts[1].slots.len(), 1);
    assert_eq!(project.automation_bindings().len(), 1);
    assert_eq!(to_flp(&project).as_ref(), Ok(&file));
}

#[test]
fn a_stream_with_events_dropped_moved_or_mangled_never_panics() {
    let base = fixture();
    let ids: Vec<u32> = base.iter().map(|e| e.id).collect();
    let mut rng = Rng(11);
    for _ in 0..4000 {
        let mut events = base.clone();
        for _ in 0..1 + rng.below(6) {
            mutate(&mut rng, &mut events, &ids);
        }
        let ppq = [96, 0, 1, 3, 4, u16::MAX][rng.below(6)];
        check(events, ppq);
    }
}

#[test]
fn a_note_at_the_end_of_time_and_a_deleted_one_never_panic() {
    let mut deleted = Note::default();
    (deleted.tag, deleted.channel) = (u16::MAX, u16::MAX);
    let last = Note {
        position: i32::MAX,
        length: i32::MAX,
        ..Note::default()
    };
    let mut events = fixture();
    let notes = events
        .iter_mut()
        .find(|e| e.id == Id::PatternNotes.0)
        .unwrap();
    notes.data.extend(Note::list_to_bytes(&[deleted, last]));
    for ppq in [96, 1, 0] {
        check(events.clone(), ppq);
    }
}

#[test]
fn every_event_alone_and_twice_never_panics() {
    // Each event without the ones that give it an owner, and right after
    // itself.
    for e in fixture() {
        check(vec![e.clone()], 96);
        check(vec![e.clone(), e], 96);
    }
}

#[test]
fn every_prefix_and_suffix_of_the_stream_never_panics() {
    let base = fixture();
    for at in 0..=base.len() {
        check(base[..at].to_vec(), 96);
        check(base[at..].to_vec(), 96);
    }
}

#[test]
fn the_stream_reversed_never_panics() {
    let mut events = fixture();
    events.reverse();
    check(events, 96);
}

#[test]
fn random_events_never_panic() {
    let ids: Vec<u32> = fixture().iter().map(|e| e.id).collect();
    let mut rng = Rng(12);
    for _ in 0..3000 {
        let events = (0..rng.below(30))
            .map(|_| {
                let id = if rng.below(3) == 0 {
                    rng.byte().into()
                } else {
                    ids[rng.below(ids.len())]
                };
                let len = match flp::event::kind(id) {
                    Kind::U8 => 1,
                    Kind::U16 => 2,
                    Kind::U32 => 4,
                    Kind::Var => rng.below(100),
                };
                RawEvent {
                    id,
                    data: rng.bytes(len),
                }
            })
            .collect();
        check(events, 96);
    }
}
