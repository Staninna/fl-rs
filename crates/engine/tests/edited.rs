//! A project after random edits renders a short stretch without a panic
//! and without a sample that is not finite.
//!
//! The project is `dangling.rs`'s: a sampler playing a click into insert
//! 1, which holds Fruity Fast Dist; TestSine on a plugin channel; an audio
//! clip channel playing a ramp; and an automation clip on insert 1's
//! volume. Each case makes a run of the edits the app makes, through the
//! model's own methods and with random and extreme arguments: the piano
//! roll's note tools and note edits, the steps, the playlist's moves,
//! resizes, splits and copies, the channels' and the mixer's settings, new
//! and cloned patterns and the tempo. Then it is rendered offline, and
//! played by an [`Engine`] in song mode and in pattern mode on each of its
//! patterns, from the start and from a seek. The engine may turn a project
//! down with [`Error::Tempo`], [`Error::Ppq`] or [`Error::RouteCycle`],
//! and nothing else.
//!
//! The cases come from a small fixed-seed generator, so a failure repeats.

use engine::{Engine, Error, RenderOptions, prepare};
use flp::event::Id;
use flp::event::layout::{self, AutomationData, InsertFlags, Levels, PluginWrapper, Point, Trim};
use model::{
    Arrangement, Channel, ChannelKind, ControllerLink, Insert, Item, ItemSource, LinkKind, Note,
    NoteField, Pattern, Placed, PlaylistItem, Project, Slot,
};
use std::path::{Path, PathBuf};

const RATE: u32 = 48_000;
const BAR: i32 = 384;

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

    fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[self.below(values.len())]
    }

    fn flip(&mut self) -> bool {
        self.below(2) == 0
    }

    fn u64(&mut self) -> u64 {
        match self.below(4) {
            0 => self.pick(&[0, 1, 2, 24, 96, i32::MAX as u64, u64::MAX]),
            1 => self.next(),
            _ => self.below(800) as u64,
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
            _ => self.below(800) as i32 - 400,
        }
    }

    fn u32(&mut self) -> u32 {
        match self.below(4) {
            0 => self.pick(&[0, 1, 2, 4, 16, 12_800, 65_536, i32::MAX as u32, u32::MAX]),
            1 => self.next() as u32,
            _ => self.below(70) as u32,
        }
    }

    /// An index of a channel, an insert or a pattern: mostly one the
    /// project has.
    fn index(&mut self) -> u16 {
        match self.below(4) {
            0 => self.pick(&[0, 4, 127, 501, 999, u16::MAX]),
            _ => self.below(4) as u16,
        }
    }

    fn u8(&mut self) -> u8 {
        self.pick(&[0, 1, 9, 10, 60, 100, 128, 131, 200, u8::MAX])
    }

    /// Places in a list of `len`: none, all, or a few, some past its end.
    fn ids(&mut self, len: usize) -> Vec<usize> {
        match self.below(4) {
            0 => (0..len).collect(),
            _ => (0..self.below(5))
                .map(|_| match self.below(5) {
                    0 => usize::MAX,
                    _ => self.below(len + 2),
                })
                .collect(),
        }
    }

    fn note(&mut self) -> Note {
        Note {
            position: self.i32(),
            channel: self.index(),
            length: self.i32(),
            key: self.next() as i16,
            fine_pitch: self.u8(),
            velocity: self.u8(),
            pan: self.u8(),
            release: self.u8(),
            flags: self.u8(),
            ..Note::default()
        }
    }
}

/// A directory under the system's temp directory, removed on drop, with
/// the two samples the project plays.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("fl-rs-engine-{}-edited-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut click = hound::WavWriter::create(dir.join("click.wav"), spec).unwrap();
        for frame in 0..2048 {
            click
                .write_sample(if frame == 0 { 1f32 } else { 0.0 })
                .unwrap();
        }
        click.finalize().unwrap();
        let mut ramp = hound::WavWriter::create(dir.join("ramp.wav"), spec).unwrap();
        for frame in 0..1000 {
            ramp.write_sample(frame as f32 / 1000.0).unwrap();
        }
        ramp.finalize().unwrap();
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn levels(pan: u32, volume: u32) -> Levels {
    Levels {
        pan,
        volume,
        unknown_8: [0; 4],
        mod_x: 0,
        mod_y: 0,
        unknown_20: [0; 4],
        len: 24,
    }
}

fn wrapper(slot: i32, flags: u32) -> PluginWrapper {
    PluginWrapper {
        insert: 0,
        slot,
        plugin_kind: -1,
        unknown_12: [0; 4],
        flags,
        page: 0,
        unknown_21: [0; 15],
        editor_position: 0,
        editor_size: 0,
        len: 52,
    }
}

fn params(values: &[i32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn note(position: i32, length: i32, channel: u16, key: i16) -> Note {
    Note {
        position,
        length,
        channel,
        key,
        ..Note::default()
    }
}

/// The project every case starts from, as FL 26.1 would hold it:
/// `dangling.rs`'s, with a trimmed clip item, and a 64 for each channel.
fn base() -> Project {
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 4,
        ppq: 96,
    });
    project.writer_version = Some("26.1.0.4975".to_owned());
    project.tempo_milli = Some(120_000);
    let sampler = Channel {
        index: 0,
        kind: Some(ChannelKind::Sampler),
        sample_path: Some("click.wav".to_owned()),
        levels: Some(levels(6400, 10_000)),
        target_insert: Some(1),
        ..Channel::default()
    };
    let sine = Channel {
        index: 1,
        kind: Some(ChannelKind::Plugin),
        plugin_name: Some("TestSine".to_owned()),
        wrapper: Some(wrapper(0, 0x50)),
        plugin_state: Some(params(&[100])),
        levels: Some(levels(6400, 10_000)),
        target_insert: Some(0),
        ..Channel::default()
    };
    let clip = Channel {
        index: 2,
        kind: Some(ChannelKind::AudioClip),
        sample_path: Some("ramp.wav".to_owned()),
        levels: Some(levels(6400, 10_000)),
        target_insert: Some(2),
        ..Channel::default()
    };
    let points = vec![Point::new(0.0, 0.2), Point::new(4.0, 0.9)];
    let automation = Channel {
        index: 3,
        kind: Some(ChannelKind::Automation),
        levels: Some(levels(0, 12_800)),
        automation: Some(AutomationData::new(points)),
        ..Channel::default()
    };
    project.channels = vec![sampler, sine, clip, automation];
    // Each channel has its 64, so that a clone and a new sampler have a
    // place to go.
    for (pos, channel) in project.channels.iter_mut().enumerate() {
        channel.order = vec![Placed {
            pos,
            item: Item::Field(Id::ChannelNew),
        }];
    }
    project.controller_links.push(ControllerLink {
        kind: LinkKind::Remote,
        link: Some(layout::ControllerLink {
            source: 3 << 16,
            unknown_4: [0; 4],
            target: 0x2000_0000 + (1 << 22) + 0x1FC0,
            flags: 8,
            unknown_16: 469i32.to_le_bytes(),
        }),
        ..ControllerLink::default()
    });
    let notes = (0..4)
        .flat_map(|beat| [note(beat * 96, 0, 0, 60), note(beat * 96, 48, 1, 69)])
        .collect();
    project.patterns.push(Pattern {
        index: 1,
        notes,
        ..Pattern::default()
    });
    let mut trimmed = PlaylistItem::new(ItemSource::Channel(2), BAR, BAR);
    trimmed.set_trim(Some(Trim::Clip {
        start: 10.0,
        end: 400.0,
    }));
    project.arrangements.push(Arrangement {
        index: 0,
        items: vec![
            PlaylistItem::new(ItemSource::Pattern(1), 0, BAR),
            PlaylistItem::new(ItemSource::Channel(2), 0, BAR),
            PlaylistItem::new(ItemSource::Channel(3), 0, BAR),
            trimmed,
        ],
        ..Arrangement::default()
    });
    for index in 0..4u8 {
        let flags = InsertFlags {
            unknown_0: [0; 4],
            flags: 0b1100,
            unknown_8: [0; 4],
            len: 12,
        };
        project.inserts.push(Insert {
            flags: Some(flags),
            routing: Some(vec![u8::from(index != 0), 0, 0, 0]),
            output: Some(-1),
            ..Insert::default()
        });
    }
    project.inserts[1].slots.push(Slot {
        index: 0,
        plugin_name: Some("Fruity Fast Dist".to_owned()),
        wrapper: Some(wrapper(0, 0x140)),
        plugin_state: Some(params(&[128, 10, 0, 128, 128])),
        ..Slot::default()
    });
    project
}

/// One edit of the pattern the app has selected, as the piano roll and
/// the channel rack make them.
fn edit_pattern(rng: &mut Rng, pattern: &mut Pattern, ppq: u16) {
    let ids = rng.ids(pattern.notes.len());
    let channel = rng.index();
    match rng.below(24) {
        0 => {
            let notes: Vec<(u64, u8)> = (0..rng.below(4)).map(|_| (rng.u64(), rng.u8())).collect();
            pattern.add_notes(channel, rng.u64(), &notes);
        }
        1 => pattern.slice_notes(&ids, rng.u64()),
        2 => {
            let (from, to) = ((rng.u64(), rng.i32()), (rng.u64(), rng.i32()));
            pattern.slice_notes_along(&ids, from, to, rng.u64());
        }
        3 => pattern.glue_notes(&ids),
        4 => pattern.quantize_notes(&ids, rng.u64(), rng.flip()),
        5 => pattern.chop_notes(&ids, rng.u64()),
        6 => pattern.arpeggiate_notes(&ids, rng.u64()),
        7 => pattern.strum_notes(&ids, rng.u64()),
        8 => pattern.snap_notes_to_scale(&ids, rng.next() as u16),
        9 => pattern.reverse_notes(&ids),
        10 => pattern.flip_notes(&ids),
        11 => pattern.legato_notes(&ids),
        12 => {
            let notes: Vec<Note> = (0..rng.below(4)).map(|_| rng.note()).collect();
            pattern.paste_notes(&notes);
        }
        13 => pattern.set_step(channel, rng.u32(), rng.flip(), ppq),
        14 => pattern.shift_steps(channel, rng.i32(), rng.u32(), ppq),
        15 => pattern.fill_steps(channel, rng.u32(), rng.u32(), ppq),
        16 => pattern.clear_steps(channel),
        17 => pattern.add_note(rng.note()),
        18 => pattern.delete_notes(&ids),
        19 => pattern.move_notes(&ids, rng.i64(), rng.i32()),
        20 => pattern.resize_notes(&ids, rng.i64()),
        21 => pattern.set_notes_muted(&ids, rng.flip()),
        22 => {
            let id = rng.below(pattern.notes.len() + 1);
            pattern.set_note_field(id, rng.pick(&NoteField::ALL), rng.u8());
        }
        _ => pattern.set_note_velocity(rng.below(pattern.notes.len() + 1), rng.u8()),
    }
}

/// One edit of the project, anywhere in it.
fn edit(rng: &mut Rng, project: &mut Project) {
    let (version, ppq) = (project.version(), project.header.ppq);
    match rng.below(30) {
        0..10 => {
            let index = rng.pick(&[1, 1, 1, 2, 0, 999, u16::MAX]);
            edit_pattern(rng, project.pattern_to_edit(index), ppq);
        }
        10..15 => {
            let Some(playlist) = project.current_arrangement_mut() else {
                return;
            };
            let ids = rng.ids(playlist.items.len());
            match rng.below(6) {
                0 => playlist.move_items(&ids, rng.i64(), rng.i32(), version),
                1 => playlist.resize_items(&ids, rng.i32()),
                2 => playlist.mute_items(&ids, rng.flip()),
                3 => playlist.delete_items(&ids),
                4 => playlist.duplicate_items(&ids),
                _ => {
                    let at = rng.i32();
                    project.split_items(&ids, at);
                }
            }
        }
        15 => project.set_channel_enabled(rng.index(), rng.flip()),
        16 => project.set_channel_volume(rng.index(), rng.u32()),
        17 => project.set_channel_pan(rng.index(), rng.u32()),
        18 => project.set_channel_insert(rng.index(), rng.index()),
        19 => {
            let path = rng.pick(&["click.wav", "ramp.wav", "gone.wav", ""]);
            project.set_channel_sample(rng.index(), path);
        }
        20 => match rng.below(3) {
            0 => project.solo_channel(rng.index()),
            1 => {
                let _ = project.clone_channel(rng.index());
            }
            _ => {
                let _ = project.add_sampler_channel(rng.pick(&["click.wav", "gone.wav"]));
            }
        },
        21 => project.set_insert_volume_raw(rng.index(), rng.i32()),
        22 => project.set_insert_pan_raw(rng.index(), rng.i32()),
        23 => project.set_insert_separation_raw(rng.index(), rng.i32()),
        24 => match rng.below(4) {
            0 => project.set_insert_enabled(rng.index(), rng.flip()),
            1 => project.set_insert_polarity_reversed(rng.index(), rng.flip()),
            2 => project.set_insert_swapped(rng.index(), rng.flip()),
            _ => project.solo_insert(rng.index()),
        },
        25 => project.set_route(rng.index(), rng.index(), rng.flip()),
        26 => project.set_send_level_raw(rng.index(), rng.index(), rng.i32()),
        27 => match rng.flip() {
            true => project.set_slot_enabled(rng.index(), rng.u8(), rng.flip()),
            false => project.set_slot_mix_raw(rng.index(), rng.u8(), rng.i32()),
        },
        28 => project.set_tempo_milli(rng.u32()),
        _ => match rng.below(3) {
            0 => project.add_pattern(rng.index()),
            1 => project.clone_pattern(1, rng.pick(&[2, 3, 999])),
            _ => {
                // What a file from before PPQ 96 has: the edits take it as
                // they find it.
                project.header.ppq = rng.pick(&[1, 4, 24, 96, 960]);
            }
        },
    }
}

/// Turned down, as the engine may turn a project down.
fn refused(error: &Error) -> bool {
    matches!(error, Error::Tempo | Error::Ppq | Error::RouteCycle { .. })
}

fn assert_finite(left: &[f32], right: &[f32], what: &str) {
    let bad = left.iter().chain(right).position(|x| !x.is_finite());
    assert!(
        bad.is_none(),
        "{what}: a sample that is not finite at {bad:?}"
    );
}

/// Render a short stretch of `project` offline, and play it on an engine
/// in song mode and on each of its patterns.
fn check(project: &Project, root: &Path, what: &str) {
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: root.to_path_buf(),
        max_seconds: 0.05,
        plugin_dir: None,
    };
    match engine::render(project, &opts) {
        Ok(out) => assert_finite(&out.left, &out.right, what),
        Err(e) => assert!(refused(&e), "{what}: {e}"),
    }
    let _ = engine::song_length(project, RATE);
    let samples = prepare(project, root).unwrap();
    let mut engine = match Engine::new(project, &samples, RATE) {
        Ok(engine) => engine,
        Err(e) => {
            assert!(refused(&e), "{what}: {e}");
            return;
        }
    };
    let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
    let mut play = |engine: &mut Engine| {
        for _ in 0..2 {
            engine.process(&mut left, &mut right);
            assert_finite(&left, &right, what);
        }
    };
    engine.play();
    play(&mut engine);
    for pattern in project.patterns.iter().map(|p| p.index).take(4) {
        if let Err(e) = engine.set_pattern(pattern) {
            assert!(refused(&e), "{what}: {e}");
        }
        engine.seek(0);
        play(&mut engine);
        engine.seek(u64::from(u32::MAX));
        play(&mut engine);
    }
}

#[test]
fn the_base_project_plays_after_an_edit_that_changes_nothing() {
    let dir = Dir::new("base");
    let mut project = base();
    project.set_channel_volume(500, 0);
    project.pattern_to_edit(1).delete_notes(&[]);
    check(&project, &dir.0, "base");
}

#[test]
fn random_edits_play_with_every_sample_finite() {
    let dir = Dir::new("random");
    let mut rng = Rng(51);
    let mut failed = Vec::new();
    for case in 0..1000 {
        let mut project = base();
        for _ in 0..1 + rng.below(10) {
            edit(&mut rng, &mut project);
        }
        let what = format!("case {case}");
        let run = std::panic::catch_unwind(|| check(&project, &dir.0, &what));
        if let Err(panic) = run {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            failed.push(format!("{what}: {message}"));
        }
    }
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
