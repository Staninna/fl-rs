//! Random runs of commands on the app never panic it, and its undo and
//! redo take it back and forth over the whole run.
//!
//! Each run opens a fixture, then sends edits of every kind with random
//! and extreme arguments, some of them in drags, and undo, redo, pattern
//! selections, the song mode, seeks and saves to a file of its own in
//! between. Then it undoes all the way, which must give the bytes the app
//! opened, and redoes all the way, which must give the bytes it had before
//! the undos. A selection is no undo step, and its flags are in the file,
//! so the bytes are compared with every note and item let go of.
//!
//! The cases come from a small fixed-seed generator, so a failure repeats.

use fl_rs::App;
use flp::event::Id;
use flp::event::layout::{InsertFlags, Levels};
use model::{
    Arrangement, Channel, ChannelKind, Insert, Item, ItemSource, Note, NoteField, Pattern, Placed,
    PlaylistItem, Project,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui::{Command, Edit};

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

    fn pick<T: Clone>(&mut self, values: &[T]) -> T {
        values[self.below(values.len())].clone()
    }

    fn flip(&mut self) -> bool {
        self.below(2) == 0
    }

    fn u64(&mut self) -> u64 {
        match self.below(4) {
            0 => self.pick(&[0, 1, 24, 96, i32::MAX as u64, i32::MAX as u64 + 1, u64::MAX]),
            1 => self.next(),
            _ => self.below(2000) as u64,
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
            _ => self.below(800) as i64 - 400,
        }
    }

    fn i32(&mut self) -> i32 {
        match self.below(4) {
            0 => self.pick(&[0, 1, -1, i32::MAX, i32::MIN, 12_800, 16_000, -6400, 64]),
            1 => self.next() as i32,
            _ => self.below(200) as i32 - 100,
        }
    }

    fn u32(&mut self) -> u32 {
        match self.below(4) {
            0 => self.pick(&[0, 1, 2, 4, 16, 65_536, i32::MAX as u32, u32::MAX]),
            1 => self.next() as u32,
            _ => self.below(70) as u32,
        }
    }

    /// An index of a channel, an insert or a pattern: mostly one the
    /// fixture has.
    fn index(&mut self) -> u16 {
        match self.below(4) {
            0 => self.pick(&[0, 2, 127, 501, 999, 1000, u16::MAX]),
            _ => self.below(4) as u16,
        }
    }

    fn u8(&mut self) -> u8 {
        self.pick(&[0, 1, 9, 10, 60, 100, 128, 131, 200, u8::MAX])
    }

    fn ids(&mut self) -> Vec<usize> {
        (0..self.below(5))
            .map(|_| self.pick(&[0, 1, 2, 3, 4, 5, 9, usize::MAX]))
            .collect()
    }

    fn name(&mut self) -> String {
        self.pick(&["", "Lead", "Ä", "\u{1F600}", "a\0b", "Pattern 1 #2"])
            .to_owned()
    }

    fn note(&mut self) -> Note {
        Note {
            position: self.i32(),
            channel: self.index(),
            length: self.i32(),
            key: self.next() as i16,
            velocity: self.u8(),
            flags: self.u8(),
            ..Note::default()
        }
    }

    /// Any edit, with arguments the windows send and ones they never do.
    fn edit(&mut self) -> Edit {
        let ids = self.ids();
        match self.below(52) {
            0 => Edit::MoveItems {
                ids,
                delta_ticks: self.i64(),
                delta_tracks: self.i32(),
            },
            1 => Edit::ResizeItems {
                ids,
                new_length: self.u64(),
            },
            2 => Edit::SetMuted {
                ids,
                muted: self.flip(),
            },
            3 => Edit::DeleteItems { ids },
            4 => Edit::DuplicateItems { ids },
            5 => Edit::SelectItems {
                ids,
                add: self.flip(),
            },
            6 => Edit::SplitItems {
                ids,
                at: self.u64(),
            },
            7 => Edit::SetStep {
                channel: self.index(),
                step: self.u32(),
                on: self.flip(),
            },
            8 => Edit::SetChannelEnabled {
                channel: self.index(),
                enabled: self.flip(),
            },
            9 => Edit::SetChannelVolume {
                channel: self.index(),
                volume: self.u32(),
            },
            10 => Edit::SetChannelPan {
                channel: self.index(),
                pan: self.u32(),
            },
            11 => Edit::SetChannelInsert {
                channel: self.index(),
                insert: self.index(),
            },
            12 => Edit::SetChannelSample {
                channel: self.index(),
                path: PathBuf::from(self.pick(&["click.wav", "", "gone/x.wav"])),
            },
            13 => Edit::AddSamplerChannel {
                path: PathBuf::from(self.pick(&["click.wav", "gone.wav"])),
            },
            14 => Edit::SetStepVelocity {
                channel: self.index(),
                step: self.u32(),
                velocity: self.u8(),
            },
            15 => Edit::SetChannelName {
                channel: self.index(),
                name: self.name(),
            },
            16 => Edit::SoloChannel {
                channel: self.index(),
            },
            17 => Edit::ShiftSteps {
                channel: self.index(),
                by: self.i32(),
                count: self.u32(),
            },
            18 => Edit::FillSteps {
                channel: self.index(),
                every: self.u32(),
                count: self.u32(),
            },
            19 => Edit::ClearSteps {
                channel: self.index(),
            },
            20 => Edit::CloneChannel {
                channel: self.index(),
            },
            21 => Edit::AddNote {
                channel: self.index(),
                position: self.u64(),
                length: self.u64(),
                key: self.u8(),
            },
            22 => Edit::DeleteNotes { ids },
            23 => Edit::MoveNotes {
                ids,
                delta_ticks: self.i64(),
                delta_keys: self.i32(),
            },
            24 => Edit::ResizeNotes {
                ids,
                delta_ticks: self.i64(),
            },
            25 => Edit::SetNotesMuted {
                ids,
                muted: self.flip(),
            },
            26 => Edit::SetNoteField {
                id: self.pick(&[0, 1, 5, usize::MAX]),
                field: self.pick(&NoteField::ALL),
                value: self.u8(),
            },
            27 => Edit::AddNotes {
                channel: self.index(),
                length: self.u64(),
                notes: (0..self.below(4))
                    .map(|_| (self.u64(), self.u8()))
                    .collect(),
            },
            28 => Edit::SliceNotes {
                ids,
                at: self.u64(),
            },
            29 => Edit::SliceNotesAlong {
                ids,
                from: (self.u64(), self.i32()),
                to: (self.u64(), self.i32()),
                step: self.u64(),
            },
            30 => Edit::GlueNotes { ids },
            31 => Edit::QuantizeNotes {
                ids,
                step: self.u64(),
                lengths: self.flip(),
            },
            32 => Edit::ChopNotes {
                ids,
                step: self.u64(),
            },
            33 => Edit::ArpeggiateNotes {
                ids,
                step: self.u64(),
            },
            34 => Edit::StrumNotes {
                ids,
                delay: self.u64(),
            },
            35 => Edit::SnapNotesToScale {
                ids,
                keys: self.next() as u16,
            },
            36 => Edit::ReverseNotes { ids },
            37 => Edit::FlipNotes { ids },
            38 => Edit::LegatoNotes { ids },
            39 => Edit::PasteNotes {
                notes: (0..self.below(4)).map(|_| self.note()).collect(),
            },
            40 => Edit::SelectNotes {
                ids,
                add: self.flip(),
            },
            41 => Edit::SetInsertVolume {
                insert: self.index(),
                volume: self.i32(),
            },
            42 => Edit::SetInsertPan {
                insert: self.index(),
                pan: self.i32(),
            },
            43 => Edit::SetInsertEnabled {
                insert: self.index(),
                enabled: self.flip(),
            },
            44 => Edit::SoloInsert {
                insert: self.index(),
            },
            45 => Edit::SetInsertSeparation {
                insert: self.index(),
                separation: self.i32(),
            },
            46 => match self.below(3) {
                0 => Edit::SetInsertPolarity {
                    insert: self.index(),
                    reversed: self.flip(),
                },
                1 => Edit::SetInsertSwapped {
                    insert: self.index(),
                    swapped: self.flip(),
                },
                _ => Edit::SetInsertName {
                    insert: self.index(),
                    name: self.name(),
                },
            },
            47 => Edit::SetRoute {
                from: self.index(),
                to: self.index(),
                on: self.flip(),
            },
            48 => Edit::SetSendLevel {
                from: self.index(),
                to: self.index(),
                level: self.i32(),
            },
            49 => match self.flip() {
                true => Edit::SetSlotEnabled {
                    insert: self.index(),
                    slot: self.u8(),
                    enabled: self.flip(),
                },
                false => Edit::SetSlotMix {
                    insert: self.index(),
                    slot: self.u8(),
                    mix: self.i32(),
                },
            },
            50 => Edit::SetTempo { milli: self.u32() },
            _ => match self.below(3) {
                0 => Edit::AddPattern {
                    index: self.index(),
                },
                1 => Edit::ClonePattern {
                    from: self.index(),
                    to: self.index(),
                },
                _ => Edit::SetPatternName {
                    pattern: self.index(),
                    name: self.name(),
                },
            },
        }
    }
}

/// A directory under the system's temp directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("fl-rs-random-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Poll until the load that is running has landed.
fn settle(app: &mut App) {
    let started = Instant::now();
    loop {
        app.poll();
        if !app.is_busy() {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "still busy");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The app tests' bar at 120 BPM as FL 20.8 would write it, with a second
/// channel, a second pattern and the mixer's four inserts: every edit has
/// something to change, and an event to write it in.
fn fixture() -> Project {
    let mut pos = 0;
    let mut field = |id: Id| {
        pos += 1;
        Placed {
            pos,
            item: Item::Field(id),
        }
    };
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 2,
        ppq: 96,
    });
    project.writer_version = Some("20.8.4.2576".to_owned());
    project.tempo_milli = Some(120_000);
    project.title = Some("One bar".to_owned());
    project.order = vec![field(Id::FLVersion), field(Id::Tempo), field(Id::Title)];
    let note = |beat: i32, key: i16| Note {
        position: beat * 96,
        length: 48,
        key,
        ..Note::default()
    };
    project.patterns.push(Pattern {
        index: 1,
        notes: vec![
            note(0, 60),
            note(0, 64),
            note(1, 60),
            note(2, 67),
            note(3, 60),
        ],
        order: vec![field(Id::PatternNew), field(Id::PatternNotes)],
        ..Pattern::default()
    });
    project.patterns.push(Pattern {
        index: 2,
        name: Some("Fill".to_owned()),
        notes: vec![note(0, 72)],
        order: vec![
            field(Id::PatternNew),
            field(Id::PatternName),
            field(Id::PatternNotes),
        ],
        ..Pattern::default()
    });
    for index in 0..2 {
        project.channels.push(Channel {
            index,
            kind: Some(ChannelKind::Sampler),
            sample_path: Some("click.wav".to_owned()),
            levels: Some(Levels {
                pan: 6400,
                volume: 10_000,
                unknown_8: [0; 4],
                mod_x: 0,
                mod_y: 0,
                unknown_20: [0; 4],
                len: 24,
            }),
            routed_to: Some(1),
            order: vec![
                field(Id::ChannelNew),
                field(Id::ChannelType),
                field(Id::SamplePath),
                field(Id::Levels),
                field(Id::RoutedTo),
            ],
            ..Channel::default()
        });
    }
    let items = [
        PlaylistItem::new(ItemSource::Pattern(1), 0, 384),
        PlaylistItem::new(ItemSource::Pattern(2), 384, 384),
        PlaylistItem::new(ItemSource::Channel(1), 0, 768),
    ];
    let items = items.iter().map(|item| item.shortened(32).unwrap());
    project.arrangements.push(Arrangement {
        index: 0,
        items: items.collect(),
        order: vec![field(Id::ArrangementNew), field(Id::ArrangementPlaylist)],
        ..Arrangement::default()
    });
    for index in 0..4 {
        project.inserts.push(Insert {
            flags: Some(InsertFlags {
                unknown_0: [0; 4],
                flags: 0b1100,
                unknown_8: [0; 4],
                len: 12,
            }),
            routing: Some(vec![u8::from(index != 0), 0, 0, 0]),
            output: Some(-1),
            order: vec![
                field(Id::InsertFlags),
                field(Id::InsertRouting),
                field(Id::InsertOutput),
            ],
            ..Insert::default()
        });
    }
    project.order.push(field(Id::MixerParams));
    project
}

/// The fixture and its sample in `dir`.
fn write_fixture(dir: &Path) -> PathBuf {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(dir.join("click.wav"), spec).unwrap();
    for frame in 0..64 {
        writer
            .write_sample(if frame == 0 { i16::MAX } else { 0 })
            .unwrap();
    }
    writer.finalize().unwrap();
    let path = dir.join("song.flp");
    fl_rs::save(&fixture(), &path).unwrap();
    path
}

/// What the project writes, with every note and playlist item let go of.
fn bytes(project: &Project) -> Vec<u8> {
    let mut project = project.clone();
    for pattern in &mut project.patterns {
        pattern.select_notes(&[], false);
    }
    for arrangement in &mut project.arrangements {
        arrangement.select_items(&[], false);
    }
    model::to_flp(&project)
        .expect("an edited project writes")
        .write()
}

/// Fewer than the app's undo keeps, so undo reaches the start.
const COMMANDS: usize = 60;

/// A run of random commands on the fixture, then undo all the way and
/// redo all the way. How many steps the undo went back.
fn run(rng: &mut Rng, dir: &Dir, path: &Path) -> usize {
    let mut app = App::new(None);
    app.apply(Command::Open(path.to_path_buf()));
    settle(&mut app);
    let opened = bytes(app.project());
    let mut drag = None;
    for n in 0..COMMANDS {
        let command = match rng.below(10) {
            0 => rng.pick(&[Command::Undo, Command::Undo, Command::Redo]),
            1 => match rng.below(4) {
                0 => Command::SelectPattern(rng.index()),
                1 => Command::ToggleSongMode,
                2 => Command::Seek(rng.u64()),
                _ => Command::SaveAs(dir.0.join(format!("saved-{n}.flp"))),
            },
            _ => {
                // A drag goes on for a few edits, which make one step.
                if rng.below(3) == 0 {
                    drag = Some(rng.below(3) as u64);
                } else if rng.below(3) == 0 {
                    drag = None;
                }
                Command::Edit {
                    edit: rng.edit(),
                    drag_id: drag,
                }
            }
        };
        app.apply(command);
        // A save writes the bytes the project has.
        if let Some(saved) = app.path().filter(|saved| *saved != path)
            && app.error().is_none()
            && !app.is_modified()
        {
            let read = fl_rs::read(saved).expect("a saved project reads");
            assert!(
                bytes(&read) == bytes(app.project()),
                "a save wrote other bytes"
            );
        }
    }
    let edited = bytes(app.project());
    // The redo list can hold steps undone before the last edits, so redo
    // goes as far as undo went: the steps that gave another project.
    let mut undone = 0;
    for _ in 0..=COMMANDS {
        let before = Arc::clone(app.project());
        app.apply(Command::Undo);
        undone += usize::from(!Arc::ptr_eq(&before, app.project()));
    }
    assert!(
        bytes(app.project()) == opened,
        "undo all the way is not the file opened"
    );
    for _ in 0..undone {
        app.apply(Command::Redo);
    }
    assert!(
        bytes(app.project()) == edited,
        "redo all the way is not where the undo began"
    );
    app.poll();
    undone
}

#[test]
fn random_commands_never_panic_and_undo_and_redo_all_of_them() {
    let dir = Dir::new("commands");
    let path = write_fixture(&dir.0);
    let mut rng = Rng(41);
    let mut undone = 0;
    for _ in 0..150 {
        undone += run(&mut rng, &dir, &path);
    }
    // The edits made steps for the undo to go back over.
    assert!(undone > 1000, "{undone} steps undone");
}
