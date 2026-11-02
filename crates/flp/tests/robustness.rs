//! Bytes that are not a project never panic the reader and never make it
//! ask for memory the input cannot account for.
//!
//! A project is built in code, one event of each shape the typed layer
//! reads, and then cut short at every length, flipped a bit at a time and
//! overwritten at random. Fully random bytes and a header followed by a
//! random body go in too, and so do length fields that claim 4 GB. Every
//! stream that parses goes through every decoder in [`flp::event`] and must
//! write back to a stream that parses to the same events.
//!
//! The cases come from a small fixed-seed generator, so a failure repeats.

use flp::event::layout::{
    ArrangementViewState, AutomationData, ChannelSubState, ControllerEvent, ControllerLink, Delay,
    EnvelopeLfo, Filter, InitialValue, InsertFlags, ItemSource, LevelAdjusts, Levels, Note,
    Parameters, PlSelection, PlaylistItem, PluginWrapper, Point, Polyphony, ProjectPairU32,
    Timestamp, Track, Tracking,
};
use flp::event::{self, Id, Value};
use flp::{Header, Project, RawEvent};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// The system allocator, keeping the largest single request this thread
/// made while [`largest_allocation`] watched.
struct Watching;

thread_local! {
    static WATCH: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn note(size: usize) {
    // `try_with`: the thread's locals may already be gone while it exits.
    let _ = WATCH.try_with(|watch| {
        if watch.get() {
            let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
        }
    });
}

// A global allocator is unsafe to write; this one only hands on to the
// system's.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Watching = Watching;

/// Run `f` and give the largest single allocation it made on this thread.
fn largest_allocation<T>(f: impl FnOnce() -> T) -> (T, usize) {
    LARGEST.with(|largest| largest.set(0));
    WATCH.with(|watch| watch.set(true));
    let out = f();
    WATCH.with(|watch| watch.set(false));
    (out, LARGEST.with(Cell::get))
}

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

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A small project with one event of each kind the typed layer has a
/// decoder for.
fn fixture() -> Vec<u8> {
    let note = Note {
        position: 96,
        length: 48,
        ..Note::default()
    };
    let item = PlaylistItem::new(ItemSource::Pattern(1), 0, 384);
    let automation = AutomationData::new(vec![Point::new(0.0, 0.5), Point::new(1.0, 1.0)]);
    let mut wrapper = vec![0; 52];
    wrapper[16] = 0x10;
    let events = vec![
        ev(Id::FLVersion, b"24.2.0.4000\0"),
        ev(Id::Title, &utf16("Song")),
        ev(Id::Tempo, &140_000u32.to_le_bytes()),
        ev(Id::ChannelNew, &0u16.to_le_bytes()),
        ev(Id::ChannelType, &[0]),
        ev(Id::IsEnabled, &[1]),
        ev(Id::ChannelName, &utf16("Kick")),
        ev(Id::Levels, &[0; 24]),
        ev(Id::PluginWrapper, &wrapper),
        ev(Id::ChannelNew, &1u16.to_le_bytes()),
        ev(Id::ChannelType, &[5]),
        ev(Id::Automation, &automation.to_bytes()),
        ev(Id::PatternNew, &1u16.to_le_bytes()),
        ev(Id::PatternNotes, &Note::list_to_bytes(&[note, note])),
        ev(Id::ArrangementNew, &0u16.to_le_bytes()),
        ev(Id::ArrangementPlaylist, &item.to_bytes()),
        ev(Id::TrackData, &[0; 70]),
        ev(Id::InsertName, &utf16("Master")),
        ev(Id::InsertOutput, &u32::MAX.to_le_bytes()),
        // An escaped ID with a length-prefixed payload.
        RawEvent {
            id: 0xC000_0101,
            data: utf16("me"),
        },
    ];
    let header = Header {
        format: 0,
        channel_count: 2,
        ppq: 96,
    };
    Project { header, events }.write()
}

/// Every decoder the event layer has, on one event. None may panic.
fn decode_everything(e: &RawEvent, writer_version: u32) {
    let d = e.data.as_slice();
    let _ = Value::decode(e);
    for wide in [false, true] {
        let _ = event::text_units(e, wide);
        let _ = event::text(e, wide);
    }
    let _ = Delay::from_bytes(d);
    let _ = PluginWrapper::from_bytes(d);
    let _ = Parameters::from_bytes(d);
    let _ = ControllerEvent::list_from_bytes(d);
    let _ = PlSelection::from_bytes(d);
    let _ = EnvelopeLfo::from_bytes(d);
    let _ = Levels::from_bytes(d);
    let _ = Filter::from_bytes(d);
    let _ = Polyphony::from_bytes(d);
    let _ = InitialValue::list_from_bytes(d);
    let _ = ControllerLink::from_bytes(d);
    let _ = Tracking::from_bytes(d);
    let _ = LevelAdjusts::from_bytes(d);
    let _ = InsertFlags::from_bytes(d);
    let _ = Timestamp::from_bytes(d);
    let _ = Track::from_bytes(d);
    let _ = ArrangementViewState::from_bytes(d);
    let _ = ProjectPairU32::from_bytes(d);
    let _ = ChannelSubState::from_bytes(d);
    let _ = AutomationData::from_bytes(d).map(|a| a.to_bytes());
    if let Some(notes) = Note::list_from_bytes(d) {
        for note in notes {
            let _ = (note.is_deleted(), note.slide(), note.fine_pitch_cents());
        }
    }
    // Every item size the writer version and 169 can pick.
    for (version, flags) in [(writer_version, 0), (0, 0), (8_001_000, 0)]
        .into_iter()
        .chain([0, 2, 10].map(|flags| (20_099_001, flags)))
    {
        for item in PlaylistItem::list_from_bytes(d, version, flags).unwrap_or_default() {
            let _ = (item.source(), item.track(version), item.trim(), item.clip());
            let _ = item.shortened(32);
        }
    }
}

/// Parse `bytes`. When they parse, decode every event, and check that the
/// project writes back to bytes that parse to the same thing. Returns the
/// largest single allocation the parse made.
fn check(bytes: &[u8]) -> usize {
    let (parsed, largest) = largest_allocation(|| Project::parse(bytes));
    if let Ok(project) = parsed {
        let version = project.writer_version().unwrap_or(0);
        for e in &project.events {
            decode_everything(e, version);
        }
        let again = Project::parse(&project.write());
        assert_eq!(again.as_ref(), Ok(&project), "{bytes:02x?}");
    }
    largest
}

/// What the parse may ask for at once: the event list grows by doubling and
/// holds at most one event per two input bytes, and a payload is never
/// longer than the input.
fn allowance(len: usize) -> usize {
    4096 + 2 * len * size_of::<RawEvent>()
}

fn check_bounded(bytes: &[u8]) {
    let largest = check(bytes);
    assert!(
        largest <= allowance(bytes.len()),
        "{} bytes of input asked for {largest} bytes at once: {bytes:02x?}",
        bytes.len()
    );
}

#[test]
fn the_fixture_parses() {
    let bytes = fixture();
    let (project, largest) = largest_allocation(|| Project::parse(&bytes).unwrap());
    assert_eq!(project.events.len(), 20);
    // The watch sees the parse's allocations, so the bounds below mean
    // something.
    assert!(largest >= 20 * size_of::<RawEvent>(), "{largest}");
    assert_eq!(project.write(), bytes);
}

#[test]
fn a_project_cut_short_anywhere_is_an_error_and_no_panic() {
    let bytes = fixture();
    for len in 0..bytes.len() {
        check_bounded(&bytes[..len]);
        assert!(Project::parse(&bytes[..len]).is_err(), "cut at {len}");
    }
}

#[test]
fn a_cut_project_with_its_lengths_patched_up_never_panics() {
    // Cutting inside FLdt with the chunk length fixed up reaches the event
    // reader, which then meets a payload or an ID that stops early.
    let bytes = fixture();
    for len in 22..bytes.len() {
        let mut cut = bytes[..len].to_vec();
        cut[18..22].copy_from_slice(&((len - 22) as u32).to_le_bytes());
        check_bounded(&cut);
    }
}

#[test]
fn flipped_bits_never_panic() {
    let bytes = fixture();
    for bit in 0..bytes.len() * 8 {
        let mut flipped = bytes.clone();
        flipped[bit / 8] ^= 1 << (bit % 8);
        check_bounded(&flipped);
    }
    // And several at once.
    let mut rng = Rng(1);
    for _ in 0..3000 {
        let mut flipped = bytes.clone();
        for _ in 0..1 + rng.below(8) {
            let i = rng.below(flipped.len());
            flipped[i] ^= 1 << rng.below(8);
        }
        check_bounded(&flipped);
    }
}

#[test]
fn overwritten_bytes_never_panic() {
    let bytes = fixture();
    let mut rng = Rng(2);
    for _ in 0..3000 {
        let mut mangled = bytes.clone();
        // Skip the chunk headers most of the time, so the event reader sees
        // most of the cases.
        for _ in 0..1 + rng.below(6) {
            let i = 22 + rng.below(mangled.len() - 22);
            mangled[i] = rng.byte();
        }
        check_bounded(&mangled);
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(3);
    for _ in 0..3000 {
        let len = rng.below(200);
        check_bounded(&rng.bytes(len));
    }
}

#[test]
fn a_valid_header_with_a_random_body_never_panics() {
    let mut rng = Rng(4);
    let head = &fixture()[..18];
    for _ in 0..5000 {
        let len = rng.below(300);
        let body = rng.bytes(len);
        let mut bytes = head.to_vec();
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);
        check_bounded(&bytes);
    }
}

#[test]
fn a_random_stream_of_well_formed_events_never_panics() {
    // Each event's frame is right, so every payload reaches the decoders:
    // the right ID with the wrong bytes, and random IDs.
    let mut rng = Rng(5);
    let ids: Vec<u32> = Project::parse(&fixture())
        .unwrap()
        .events
        .iter()
        .map(|e| e.id)
        .collect();
    for _ in 0..2000 {
        let mut events = Vec::new();
        for _ in 0..rng.below(12) {
            let id = if rng.below(2) == 0 {
                ids[rng.below(ids.len())]
            } else {
                rng.byte().into()
            };
            let len = match event::kind(id) {
                event::Kind::U8 => 1,
                event::Kind::U16 => 2,
                event::Kind::U32 => 4,
                event::Kind::Var => rng.below(120),
            };
            events.push(RawEvent {
                id,
                data: rng.bytes(len),
            });
        }
        let header = Header {
            format: 0,
            channel_count: 1,
            ppq: 96,
        };
        check_bounded(&Project { header, events }.write());
    }
}

#[test]
fn a_length_of_4_gb_asks_for_no_4_gb() {
    let head = &fixture()[..18];
    // FLdt claims 4 GB and holds nothing.
    let mut bytes = head.to_vec();
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    check_bounded(&bytes);
    assert!(Project::parse(&bytes).is_err());

    // An event whose LEB128 length says 4 GB - 1, in a chunk that holds
    // only the event's head, and in one that claims to hold all of it.
    for chunk in [7, u32::MAX] {
        let mut bytes = head.to_vec();
        bytes.extend_from_slice(&chunk.to_le_bytes());
        bytes.extend_from_slice(&[0xC7, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0]);
        check_bounded(&bytes);
        assert!(Project::parse(&bytes).is_err());
    }

    // An automation payload whose point count says 4 G points.
    let mut data = AutomationData::new(vec![Point::new(0.0, 1.0)]).to_bytes();
    data[17..21].copy_from_slice(&u32::MAX.to_le_bytes());
    let e = ev(Id::Automation, &data);
    let (decoded, largest) = largest_allocation(|| AutomationData::from_bytes(&e.data));
    assert_eq!(decoded, None);
    assert!(largest <= allowance(data.len()), "{largest}");
}
