//! Every edit the model offers, applied in random runs with random and
//! extreme arguments, never panics, never asks for memory out of
//! proportion to the project, and leaves a project that writes, reads back
//! and writes the same bytes again.
//!
//! The projects come from the event stream of `common` as it is, with
//! notes and playlist items at the ends of their fields added, cut and
//! mangled as `robustness.rs` mangles it, at PPQs down to 0, and built in
//! code with no events at all. Each run calls the piano roll's note tools
//! and note edits, the step sequencer's, the playlist's, the channel
//! rack's, the mixer's, the pattern selector's and the tempo's on them, as
//! the app does and with arguments the app never sends.
//!
//! The cases come from a small fixed-seed generator, so a failure repeats.

mod common;

use common::{Rng, fixture, header, mutate};
use flp::RawEvent;
use flp::event::{Id, Kind};
use model::rec::RecTarget;
use model::{ItemSource, Note, NoteField, Pattern, PlaylistItem, Project, from_flp, to_flp};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// The system allocator, keeping the largest single request this thread
/// made while [`largest_allocation`] watched. A request past
/// [`REFUSED`] fails, so that a runaway edit stops the test at once
/// rather than filling the machine.
struct Watching;

/// What no edit of these small projects may ask for in one go.
const REFUSED: usize = 1 << 30;

thread_local! {
    static WATCH: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn note_size(size: usize) {
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
        note_size(layout.size());
        if layout.size() > REFUSED {
            return std::ptr::null_mut();
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_size(layout.size());
        if layout.size() > REFUSED {
            return std::ptr::null_mut();
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_size(new_size);
        if new_size > REFUSED {
            return std::ptr::null_mut();
        }
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

/// The most one run of edits may ask for at once: the notes the note
/// tools may make in a call, each as big as it is in memory and in a
/// file, for every edit of a run, and room to spare.
const ALLOWANCE: usize = 64 << 20;

/// The arguments the edits get.
impl Rng {
    /// One of `values`, which must not be empty.
    fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[self.below(values.len())]
    }

    fn flip(&mut self) -> bool {
        self.below(2) == 0
    }

    fn u64(&mut self) -> u64 {
        match self.below(4) {
            0 => self.pick(&[
                0,
                1,
                2,
                3,
                24,
                96,
                i32::MAX as u64,
                i32::MAX as u64 + 1,
                u32::MAX.into(),
                u64::MAX,
            ]),
            1 => self.next(),
            _ => self.below(4000) as u64,
        }
    }

    fn i64(&mut self) -> i64 {
        match self.below(4) {
            0 => self.pick(&[
                0,
                1,
                -1,
                i32::MAX.into(),
                i32::MIN.into(),
                i64::MAX,
                i64::MIN,
            ]),
            1 => self.next() as i64,
            _ => self.below(4000) as i64 - 2000,
        }
    }

    fn i32(&mut self) -> i32 {
        match self.below(4) {
            0 => self.pick(&[0, 1, -1, i32::MAX, i32::MIN, 12_800, 16_000, -6400]),
            1 => self.next() as i32,
            _ => self.below(20_000) as i32 - 10_000,
        }
    }

    fn u32(&mut self) -> u32 {
        match self.below(4) {
            0 => self.pick(&[0, 1, 2, 4, 8, 16, 64, 65_536, i32::MAX as u32, u32::MAX]),
            1 => self.next() as u32,
            _ => self.below(70) as u32,
        }
    }

    fn u16(&mut self) -> u16 {
        match self.below(4) {
            0 => self.pick(&[0, 1, 2, 3, 125, 126, 127, 500, 501, 999, 1000, u16::MAX]),
            1 => self.next() as u16,
            _ => self.below(5) as u16,
        }
    }

    fn u8(&mut self) -> u8 {
        match self.below(3) {
            0 => self.pick(&[0, 1, 9, 10, 60, 127, 128, 131, 132, u8::MAX]),
            _ => self.next() as u8,
        }
    }

    fn i16(&mut self) -> i16 {
        match self.below(3) {
            0 => self.pick(&[0, 60, 131, -1, i16::MAX, i16::MIN]),
            _ => self.next() as i16,
        }
    }

    /// Places in a list of `len`: none, all, or a few, some past its end
    /// and some twice.
    fn ids(&mut self, len: usize) -> Vec<usize> {
        match self.below(5) {
            0 => Vec::new(),
            1 => (0..len).collect(),
            _ => (0..1 + self.below(6))
                .map(|_| match self.below(6) {
                    0 => usize::MAX,
                    1 => len,
                    _ => self.below(len + 2),
                })
                .collect(),
        }
    }

    fn name(&mut self) -> String {
        match self.below(8) {
            0 => String::new(),
            1 => "Pattern 1 #2".into(),
            2 => "Ä".into(),
            3 => "\u{1F600}".into(),
            4 => "a\0b".into(),
            5 => "x".repeat(300),
            _ => format!("n{}", self.below(4)),
        }
    }

    /// A note with any value in any field.
    fn note(&mut self) -> Note {
        let any = self.next() as u16;
        Note {
            position: self.i32(),
            tag: self.pick(&[0x4000, 0x4008, u16::MAX, any]),
            channel: self.u16(),
            length: self.i32(),
            key: self.i16(),
            group: self.next() as u16,
            fine_pitch: self.u8(),
            repeats: self.u8(),
            release: self.u8(),
            flags: self.u8(),
            pan: self.u8(),
            velocity: self.u8(),
            mod_x: self.u8(),
            mod_y: self.u8(),
        }
    }

    fn field(&mut self) -> NoteField {
        self.pick(&NoteField::ALL)
    }
}

/// The index of a pattern the project has, or another one.
fn pattern_index(rng: &mut Rng, project: &Project) -> u16 {
    match project.patterns.len() {
        0 => rng.u16(),
        n if rng.below(3) > 0 => project.patterns[rng.below(n)].index,
        _ => rng.u16(),
    }
}

fn channel_index(rng: &mut Rng, project: &Project) -> u16 {
    match project.channels.len() {
        n if n > 0 && rng.below(3) > 0 => project.channels[rng.below(n)].index,
        _ => rng.u16(),
    }
}

/// One edit of a pattern: the piano roll's and the step sequencer's.
fn edit_pattern(rng: &mut Rng, pattern: &mut Pattern, ppq: u16, channel: u16) {
    let len = pattern.notes.len();
    let ppq = if rng.below(4) == 0 {
        rng.pick(&[0, 1, 3, 4, 96, u16::MAX])
    } else {
        ppq
    };
    match rng.below(27) {
        0 => {
            let notes: Vec<(u64, u8)> = (0..rng.below(5)).map(|_| (rng.u64(), rng.u8())).collect();
            pattern.add_notes(channel, rng.u64(), &notes);
        }
        1 => pattern.slice_notes(&rng.ids(len), rng.u64()),
        2 => {
            let ids = rng.ids(len);
            let from = (rng.u64(), rng.i32());
            let to = (rng.u64(), rng.i32());
            pattern.slice_notes_along(&ids, from, to, rng.u64());
        }
        3 => pattern.glue_notes(&rng.ids(len)),
        4 => pattern.quantize_notes(&rng.ids(len), rng.u64(), rng.flip()),
        5 => pattern.chop_notes(&rng.ids(len), rng.u64()),
        6 => pattern.arpeggiate_notes(&rng.ids(len), rng.u64()),
        7 => pattern.strum_notes(&rng.ids(len), rng.u64()),
        8 => pattern.snap_notes_to_scale(&rng.ids(len), rng.next() as u16),
        9 => pattern.reverse_notes(&rng.ids(len)),
        10 => pattern.flip_notes(&rng.ids(len)),
        11 => pattern.legato_notes(&rng.ids(len)),
        12 => {
            // A paste of the whole pattern doubles it, which a run of
            // them would do past what the allowance is for.
            let notes: Vec<Note> = match rng.below(3) {
                0 if len < 1000 => pattern.notes.clone(),
                _ => (0..rng.below(5)).map(|_| rng.note()).collect(),
            };
            pattern.paste_notes(&notes);
        }
        13 => pattern.set_step(channel, rng.u32(), rng.flip(), ppq),
        14 => pattern.shift_steps(channel, rng.i32(), rng.u32(), ppq),
        15 => pattern.fill_steps(channel, rng.u32(), rng.u32(), ppq),
        16 => pattern.clear_steps(channel),
        17 => pattern.toggle_step(channel, rng.u32(), ppq),
        18 => pattern.set_note_velocity(rng.below(len + 2), rng.u8()),
        19 => pattern.set_note_pan(rng.below(len + 2), rng.u8()),
        20 => pattern.add_note(rng.note()),
        21 => pattern.delete_notes(&rng.ids(len)),
        22 => pattern.move_notes(&rng.ids(len), rng.i64(), rng.i32()),
        23 => pattern.resize_notes(&rng.ids(len), rng.i64()),
        24 => pattern.set_notes_muted(&rng.ids(len), rng.flip()),
        25 => pattern.set_note_field(rng.below(len + 2), rng.field(), rng.u8()),
        _ => pattern.select_notes(&rng.ids(len), rng.flip()),
    }
    for step in [0, 1, u32::MAX] {
        let _ = pattern.step_note(channel, step, ppq);
    }
    let _ = pattern.steps(channel, ppq);
}

/// One edit of the project, anywhere in it.
fn edit(rng: &mut Rng, project: &mut Project) {
    let ppq = project.header.ppq;
    match rng.below(40) {
        0..10 => {
            let index = pattern_index(rng, project);
            let channel = channel_index(rng, project);
            // The app edits the selected pattern through pattern_to_edit,
            // which makes it when the file has none.
            let pattern = if rng.below(4) == 0 {
                match project.patterns.len() {
                    0 => return,
                    n => &mut project.patterns[rng.below(n)],
                }
            } else {
                project.pattern_to_edit(index)
            };
            edit_pattern(rng, pattern, ppq, channel);
        }
        10..14 => {
            let version = project.version();
            let Some(arrangement) = (match rng.below(4) {
                0 => {
                    let n = project.arrangements.len();
                    (n > 0).then(|| &mut project.arrangements[rng.below(n)])
                }
                _ => project.current_arrangement_mut(),
            }) else {
                return;
            };
            let ids = rng.ids(arrangement.items.len());
            match rng.below(8) {
                0 => arrangement.move_items(&ids, rng.i64(), rng.i32(), version),
                1 => arrangement.resize_items(&ids, rng.i32()),
                2 => arrangement.mute_items(&ids, rng.flip()),
                3 => arrangement.delete_items(&ids),
                4 => arrangement.duplicate_items(&ids),
                5 => arrangement.select_items(&ids, rng.flip()),
                6 => {
                    let rate = rng.pick(&[None, Some(0.0), Some(1.0), Some(f32::MAX)]);
                    arrangement.split_items(&ids, rng.i32(), |_| rate);
                }
                _ => {
                    let at = rng.i32();
                    project.split_items(&ids, at);
                }
            }
        }
        14 => {
            let channel = channel_index(rng, project);
            project.set_channel_enabled(channel, rng.flip());
        }
        15 => {
            let channel = channel_index(rng, project);
            project.set_channel_volume(channel, rng.u32());
        }
        16 => {
            let channel = channel_index(rng, project);
            project.set_channel_pan(channel, rng.u32());
        }
        17 => {
            let channel = channel_index(rng, project);
            project.set_channel_insert(channel, rng.u16());
        }
        18 => {
            let channel = channel_index(rng, project);
            let path = rng.name();
            project.set_channel_sample(channel, &path);
        }
        19 => {
            let path = rng.name();
            let _ = project.add_sampler_channel(&path);
        }
        20 => {
            let channel = channel_index(rng, project);
            let name = rng.name();
            project.set_channel_name(channel, &name);
        }
        21 => {
            let channel = channel_index(rng, project);
            let _ = project.clone_channel(channel);
        }
        22 => {
            let channel = channel_index(rng, project);
            project.solo_channel(channel);
        }
        23 => project.set_insert_volume_raw(rng.u16(), rng.i32()),
        24 => project.set_insert_pan_raw(rng.u16(), rng.i32()),
        25 => project.set_insert_separation_raw(rng.u16(), rng.i32()),
        26 => project.set_send_level_raw(rng.u16(), rng.u16(), rng.i32()),
        27 => project.set_slot_mix_raw(rng.u16(), rng.u8(), rng.i32()),
        28 => project.set_slot_enabled(rng.u16(), rng.u8(), rng.flip()),
        29 => project.set_insert_enabled(rng.u16(), rng.flip()),
        30 => project.set_insert_polarity_reversed(rng.u16(), rng.flip()),
        31 => project.set_insert_swapped(rng.u16(), rng.flip()),
        32 => project.solo_insert(rng.u16()),
        33 => project.set_route(rng.u16(), rng.u16(), rng.flip()),
        34 => {
            let name = rng.name();
            project.set_insert_name(rng.u16(), &name);
        }
        35 => project.set_tempo_milli(rng.u32()),
        36 => {
            let index = match rng.below(3) {
                0 => project.free_pattern_index().unwrap_or(0),
                _ => rng.u16(),
            };
            project.add_pattern(index);
        }
        37 => {
            let from = pattern_index(rng, project);
            let to = match rng.below(3) {
                0 => project.free_pattern_index().unwrap_or(0),
                _ => rng.u16(),
            };
            project.clone_pattern(from, to);
        }
        38 => {
            let index = pattern_index(rng, project);
            let name = rng.name();
            project.set_pattern_name(index, &name);
        }
        _ => {
            let target = RecTarget::Insert {
                index: rng.u16(),
                slot: rng.pick(&[None, Some(0), Some(9), Some(u8::MAX)]),
                control: rng.u16(),
            };
            let _ = project.rec_id(target);
            let _ = project.inserts_fed_by(rng.u16());
            let _ = project.inserts_feeding(rng.u16());
            let from = (rng.u64(), rng.i32());
            let to = (rng.u64(), rng.i32());
            let _ = model::slice_line_at(from, to, rng.i32(), rng.u64());
        }
    }
}

/// Write the project, read the bytes back, and see that what was read
/// writes the same bytes. A project the file's strings cannot hold, which
/// the edits turn down for every string they set, is left alone.
fn writes_the_same_again(project: &Project) {
    let Ok(file) = to_flp(project) else {
        return;
    };
    let bytes = file.write();
    let read = flp::Project::parse(&bytes);
    let read = read.unwrap_or_else(|e| panic!("what the model writes does not parse: {e}"));
    let again = to_flp(&from_flp(&read)).expect("what was read writes");
    assert!(
        again.write() == bytes,
        "the project read back writes other bytes"
    );
}

/// A run of edits on the project, then the round trip. Whether the edits
/// changed what the project writes.
fn run(rng: &mut Rng, mut project: Project, edits: usize) -> bool {
    let before = to_flp(&project);
    let (_, largest) = largest_allocation(|| {
        for _ in 0..edits {
            edit(rng, &mut project);
        }
        let _ = to_flp(&project).map(|file| file.write());
    });
    assert!(largest <= ALLOWANCE, "an allocation of {largest} bytes");
    writes_the_same_again(&project);
    to_flp(&project) != before
}

/// The common stream with notes and playlist items at the ends of their
/// fields, the kind of values only a file gives.
fn extreme_fixture() -> Vec<RawEvent> {
    let note = |position, length, key| Note {
        position,
        length,
        key,
        ..Note::default()
    };
    let notes = [
        note(i32::MIN, i32::MAX, i16::MIN),
        note(-1, 1, -1),
        note(0, i32::MAX, i16::MAX),
        note(0, i32::MAX, i16::MAX - 1),
        note(0, 0, 60),
        note(5, -1, 131),
        note(i32::MAX, i32::MAX, 60),
        note(i32::MAX, 0, i16::MIN),
    ];
    let mut items = vec![
        PlaylistItem::new(ItemSource::Pattern(1), i32::MAX, i32::MAX),
        PlaylistItem::new(ItemSource::Pattern(1), i32::MIN, i32::MAX),
        PlaylistItem::new(ItemSource::Channel(2), 0, i32::MIN),
        PlaylistItem::new(ItemSource::Pattern(u16::MAX), -1, 1),
    ];
    items[0].track_word = i16::MIN;
    items[1].track_word = i16::MAX;
    items[2].trim_start = f32::NAN.to_bits();
    items[3].trim_start = i32::MIN as u32;
    items[3].trim_end = i32::MAX as u32;
    let mut events = fixture();
    for e in &mut events {
        if e.id == Id::PatternNotes.0 {
            e.data.extend(Note::list_to_bytes(&notes));
        }
        if e.id == Id::ArrangementPlaylist.0 {
            e.data.extend(PlaylistItem::list_to_bytes(&items));
        }
    }
    events
}

/// Make the stream one a file can hold: each event of a fixed size gets a
/// payload of that size, and an event with the ID of the escape byte goes.
/// The mangling cuts and grows payloads whatever their ID and gives any
/// ID, and such events have no bytes to be written as.
fn as_a_file_holds(events: &mut Vec<RawEvent>) {
    events.retain(|e| e.id != u32::from(flp::ESCAPE));
    for e in events {
        let len = match flp::event::kind(e.id) {
            Kind::U8 => 1,
            Kind::U16 => 2,
            Kind::U32 => 4,
            Kind::Var => continue,
        };
        e.data.resize(len, 0);
    }
}

fn read(events: Vec<RawEvent>, ppq: u16) -> Project {
    from_flp(&flp::Project {
        header: header(ppq),
        events,
    })
}

/// A project built in code as the ui tests build theirs: fields and no
/// events, so most edits find no place to write and the writer writes
/// little.
fn built() -> Project {
    let mut project = Project::new(header(96));
    project.tempo_milli = Some(140_000);
    project.channels.push(model::Channel {
        kind: Some(model::ChannelKind::Sampler),
        ..model::Channel::default()
    });
    let notes = vec![
        Note {
            position: i32::MAX,
            length: i32::MAX,
            ..Note::default()
        },
        Note {
            key: i16::MAX,
            length: 96,
            ..Note::default()
        },
    ];
    project.patterns.push(Pattern {
        index: 1,
        notes,
        ..Pattern::default()
    });
    let items = vec![PlaylistItem::new(ItemSource::Pattern(1), 0, i32::MAX)];
    project.arrangements.push(model::Arrangement {
        items,
        ..model::Arrangement::default()
    });
    project
}

#[test]
fn random_edits_of_the_fixture_write_and_read_back() {
    let mut rng = Rng(21);
    let mut changed = 0;
    for _ in 0..3000 {
        let ppq = rng.pick(&[96, 96, 0, 1, 3, 4, u16::MAX]);
        let project = read(fixture(), ppq);
        let edits = 1 + rng.below(16);
        changed += usize::from(run(&mut rng, project, edits));
    }
    // The edits do something, so the round trip has something to keep.
    assert!(changed > 2000, "{changed} runs changed the project");
}

#[test]
fn random_edits_of_notes_and_items_at_their_ends_write_and_read_back() {
    let mut rng = Rng(22);
    for _ in 0..3000 {
        let ppq = rng.pick(&[96, 96, 0, 1, 3, 4, u16::MAX]);
        let project = read(extreme_fixture(), ppq);
        let edits = 1 + rng.below(16);
        let _ = run(&mut rng, project, edits);
    }
}

#[test]
fn random_edits_of_mangled_streams_write_and_read_back() {
    let base = extreme_fixture();
    let ids: Vec<u32> = base.iter().map(|e| e.id).collect();
    let mut rng = Rng(23);
    for _ in 0..3000 {
        let mut events = base.clone();
        for _ in 0..1 + rng.below(6) {
            mutate(&mut rng, &mut events, &ids);
        }
        as_a_file_holds(&mut events);
        let ppq = rng.pick(&[96, 0, 1, 3, 4, u16::MAX]);
        let edits = 1 + rng.below(16);
        let _ = run(&mut rng, read(events, ppq), edits);
    }
}

#[test]
fn random_edits_of_a_project_built_in_code_never_panic() {
    let mut rng = Rng(24);
    for _ in 0..500 {
        let edits = 1 + rng.below(16);
        let _ = run(&mut rng, built(), edits);
    }
}
