//! What the model's robustness tests share: a small fixed-seed generator,
//! the event stream they start from, and the changes they make to it.

#![allow(dead_code)]

use flp::event::layout::{AutomationData, Point};
use flp::event::{Id, Kind};
use flp::{Header, RawEvent};
use model::{ItemSource, Note, PlaylistItem};

/// SplitMix64: small, fast and the same on every machine.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n`, which must not be 0.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn byte(&mut self) -> u8 {
        self.next() as u8
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

pub fn ev(id: Id, data: &[u8]) -> RawEvent {
    RawEvent {
        id: id.0,
        data: data.to_vec(),
    }
}

pub fn word(id: Id, value: u16) -> RawEvent {
    ev(id, &value.to_le_bytes())
}

pub fn dword(id: Id, value: u32) -> RawEvent {
    ev(id, &value.to_le_bytes())
}

pub fn text(id: Id, s: &str) -> RawEvent {
    let data: Vec<u8> = s
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    ev(id, &data)
}

/// A full-size 212: insert @0, slot @4, flags @16.
pub fn wrapper(slot: i32, flags: u32) -> RawEvent {
    let mut data = vec![0; 52];
    data[4..8].copy_from_slice(&slot.to_le_bytes());
    data[16..20].copy_from_slice(&flags.to_le_bytes());
    ev(Id::PluginWrapper, &data)
}

/// A 20-byte controller link from `source` to `target`.
pub fn link(id: Id, source: u32, target: u32) -> RawEvent {
    let mut data = vec![0; 20];
    data[0..4].copy_from_slice(&source.to_le_bytes());
    data[8..12].copy_from_slice(&target.to_le_bytes());
    ev(id, &data)
}

/// A 12-byte 225 record.
pub fn initial(rec_id: u32, value: i32) -> [u8; 12] {
    let mut record = [0; 12];
    record[4..8].copy_from_slice(&rec_id.to_le_bytes());
    record[8..].copy_from_slice(&value.to_le_bytes());
    record
}

/// The stream every case starts from.
pub fn fixture() -> Vec<RawEvent> {
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

pub fn header(ppq: u16) -> Header {
    Header {
        format: 0,
        channel_count: 3,
        ppq,
    }
}

/// A value that tends to break things: an end of its range, or random.
pub fn extreme(rng: &mut Rng, width: usize) -> Vec<u8> {
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
pub fn mutate(rng: &mut Rng, events: &mut Vec<RawEvent>, ids: &[u32]) {
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
