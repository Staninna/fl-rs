//! A project whose parts point at things it does not have, or whose
//! numbers are at their ends, renders a short stretch without a panic and
//! without a sample that is not finite.
//!
//! The project is built in code as `fixtures.rs` builds its own: a sampler
//! playing a click into insert 1, which holds Fruity Fast Dist; TestSine on
//! a plugin channel; an audio clip channel playing a ramp; and an
//! automation clip on insert 1's volume. Each case then points something
//! at nothing (insert 9999, pattern 777, channel 500, a link to no
//! control), takes a sample away, or sets the tempo, the PPQ, a note, an
//! item, a level or an automation point to an end of its range. The
//! listed cases go in one at a time, and then in random sets of a few.
//!
//! Each project is rendered offline, and played by an [`Engine`] in song
//! mode, in pattern mode and through a seek, a range and a loop. The
//! engine may turn a project down with [`Error::Tempo`], [`Error::Ppq`] or
//! [`Error::RouteCycle`], and nothing else.
//!
//! The cases come from a small fixed-seed generator, so a failure repeats.

use engine::{Engine, Error, RenderOptions, prepare};
use flp::event::layout::{
    self, AutomationData, EnvelopeLfo, InitialValue, InsertFlags, Levels, Parameters,
    PluginWrapper, Point, Polyphony,
};
use model::{
    Arrangement, Channel, ChannelKind, ControllerLink, Insert, ItemSource, LinkKind, Note, Pattern,
    PlaylistItem, Project, Slot,
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

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// A directory under the system's temp directory, removed on drop, with
/// the two samples the project plays.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "fl-rs-engine-{}-dangling-{name}",
            std::process::id()
        ));
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

/// A 227 from channel `channel`'s control 0 to the control `target`.
fn bind(channel: u16, target: u32) -> ControllerLink {
    ControllerLink {
        kind: LinkKind::Remote,
        link: Some(layout::ControllerLink {
            source: u32::from(channel) << 16,
            unknown_4: [0; 4],
            target,
            flags: 8,
            unknown_16: 469i32.to_le_bytes(),
        }),
        ..ControllerLink::default()
    }
}

/// The ID FL 20.8 gives a control of an insert.
fn insert_control(insert: u32, control: u32) -> u32 {
    0x2000_0000 + (insert << 22) + control
}

const VOLUME: u32 = 0x1FC0;

fn note(position: i32, length: i32, channel: u16, key: i16) -> Note {
    Note {
        position,
        length,
        channel,
        key,
        ..Note::default()
    }
}

/// The project every case starts from, as FL 26.1 would hold it.
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
    project
        .controller_links
        .push(bind(3, insert_control(1, VOLUME)));
    let notes = (0..4)
        .flat_map(|beat| [note(beat * 96, 0, 0, 60), note(beat * 96, 48, 1, 69)])
        .collect();
    project.patterns.push(Pattern {
        index: 1,
        notes,
        ..Pattern::default()
    });
    project.arrangements.push(Arrangement {
        index: 0,
        items: vec![
            PlaylistItem::new(ItemSource::Pattern(1), 0, BAR),
            PlaylistItem::new(ItemSource::Channel(2), 0, BAR),
            PlaylistItem::new(ItemSource::Channel(3), 0, BAR),
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

/// A number at an end of an i32's range, near 0, or random.
fn extreme_i32(rng: &mut Rng) -> i32 {
    [i32::MIN, -1, 0, 1, i32::MAX, rng.next() as i32][rng.below(6)]
}

fn extreme_f32(rng: &mut Rng) -> f32 {
    let values = [
        f32::NAN,
        f32::INFINITY,
        -f32::INFINITY,
        0.0,
        -1.0,
        1e30,
        -1e30,
    ];
    values[rng.below(values.len())]
}

fn extreme_f64(rng: &mut Rng) -> f64 {
    let values = [
        f64::NAN,
        f64::INFINITY,
        -f64::INFINITY,
        0.0,
        -1.0,
        1e300,
        1e-300,
    ];
    values[rng.below(values.len())]
}

/// A random note of `project`, when it has one.
fn some_note<'a>(project: &'a mut Project, rng: &mut Rng) -> Option<&'a mut Note> {
    let pattern = project.patterns.first_mut()?;
    let n = pattern.notes.len();
    (n > 0).then(|| &mut pattern.notes[rng.below(n)])
}

fn some_item<'a>(project: &'a mut Project, rng: &mut Rng) -> Option<&'a mut PlaylistItem> {
    let items = &mut project.arrangements.first_mut()?.items;
    let n = items.len();
    (n > 0).then(|| &mut items[rng.below(n)])
}

/// How many kinds of change [`change`] knows.
const CHANGES: usize = 34;

/// Change `project` in the way numbered `which`, with `rng` for the
/// details.
fn change(project: &mut Project, which: usize, rng: &mut Rng) {
    // Earlier changes may have taken away what a change works on, and then
    // it does nothing.
    let pick = rng.below(3);
    let (sampler, clip) = (channel(project, 0).is_some(), channel(project, 2).is_some());
    match which {
        // Dangling references.
        0 if sampler => channel(project, 0).unwrap().target_insert = Some(9999),
        1 if sampler => {
            let sampler = channel(project, 0).unwrap();
            sampler.target_insert = None;
            sampler.routed_to = Some([-1, 127, -128][pick]);
        }
        2 | 3 => {
            let source = [ItemSource::Pattern(777), ItemSource::Channel(500)][which - 2];
            if let Some(arrangement) = project.arrangements.first_mut() {
                arrangement.items.push(PlaylistItem::new(source, 0, BAR));
            }
        }
        4 => {
            if let Some(pattern) = project.patterns.first_mut() {
                let channel = [4, 500, u16::MAX][pick];
                pattern.notes.push(note(0, 96, channel, 60));
            }
        }
        5 => project.controller_links.clear(),
        6 => {
            let target = [
                u32::MAX,
                insert_control(9999, VOLUME),
                0x4000_0005,
                0x0400_0000,
            ];
            project.controller_links.push(bind(3, target[rng.below(4)]));
        }
        7 => project
            .controller_links
            .push(bind(999, insert_control(1, VOLUME))),
        8 if sampler => channel(project, 0).unwrap().sample_path = None,
        9 if sampler => {
            let path = ["", "missing.wav", "/"][pick];
            channel(project, 0).unwrap().sample_path = Some(path.into());
        }
        10 if clip => channel(project, 2).unwrap().sample_path = None,
        0 | 1 | 8..=10 => {}
        11 => project.patterns.clear(),
        12 => {
            let n = rng.below(project.channels.len());
            project.channels.remove(n);
        }
        13 => project.inserts.truncate(rng.below(3)),
        14 => {
            let at = rng.below(project.inserts.len().max(1));
            if let Some(insert) = project.inserts.get_mut(at) {
                let len = [0, 1, 600][rng.below(3)];
                insert.routing = Some(rng.bytes(len));
            }
        }
        // The clock at its ends.
        15 => project.tempo_milli = Some([0, 1, 10_000, 999_000, u32::MAX][rng.below(5)]),
        16 => project.header.ppq = [0, 1, 2, 3, u16::MAX][rng.below(5)],
        17 => {
            project.tempo_milli = None;
            project.tempo_coarse = Some([0, 1, u16::MAX][rng.below(3)]);
        }
        // Notes, items and levels at their ends.
        18 => {
            if let Some(note) = some_note(project, rng) {
                note.position = extreme_i32(rng);
                note.length = extreme_i32(rng);
            }
        }
        19 => {
            if let Some(note) = some_note(project, rng) {
                note.key = [i16::MIN, -1, 0, 131, i16::MAX][rng.below(5)];
                note.fine_pitch = rng.next() as u8;
                note.velocity = rng.next() as u8;
                note.pan = rng.next() as u8;
                note.release = rng.next() as u8;
                note.tag = rng.next() as u16;
            }
        }
        20 => {
            if let Some(item) = some_item(project, rng) {
                item.position = extreme_i32(rng);
                item.length = extreme_i32(rng);
            }
        }
        21 => {
            if let Some(item) = some_item(project, rng) {
                item.gain = extreme_f32(rng);
                item.pitch = extreme_f32(rng);
                item.pan = extreme_f32(rng);
                item.stretch = extreme_f64(rng);
            }
        }
        22 => {
            if let Some(item) = some_item(project, rng) {
                item.trim_start = rng.next() as u32;
                item.trim_end = [u32::MAX, 0, rng.next() as u32][rng.below(3)];
                item.clip_flags = rng.next() as u32;
            }
        }
        23 => {
            let n = rng.below(project.channels.len().max(1));
            if let Some(channel) = project.channels.get_mut(n) {
                let v = [0, 12_800, u32::MAX, rng.next() as u32];
                channel.levels = Some(levels(v[rng.below(4)], v[rng.below(4)]));
            }
        }
        24 => {
            let n = rng.below(project.channels.len().max(1));
            if let Some(channel) = project.channels.get_mut(n) {
                channel.parameters = Parameters::from_bytes(&rng.bytes(Parameters::SIZE));
                channel.polyphony = Polyphony::from_bytes(&rng.bytes(Polyphony::SIZE));
                channel.envelopes = (0..rng.below(6))
                    .filter_map(|_| EnvelopeLfo::from_bytes(&rng.bytes(EnvelopeLfo::SIZE)))
                    .collect();
                channel.delay = layout::Delay::from_bytes(&rng.bytes(layout::Delay::SIZE));
                channel.root_note = Some(rng.next() as u32);
                channel.fine_tune = Some(extreme_i32(rng));
            }
        }
        // Automation that goes nowhere sensible.
        25 => {
            let n = rng.below(4);
            let points = (0..n)
                .map(|_| {
                    let mut point = Point::new(extreme_f64(rng), extreme_f64(rng));
                    point.tension = extreme_f32(rng);
                    point.mode = rng.next() as u8;
                    point
                })
                .collect();
            if let Some(channel) = project.channels.iter_mut().find(|c| c.index == 3) {
                channel.automation = Some(AutomationData::new(points));
            }
        }
        26 => {
            if let Some(channel) = project.channels.iter_mut().find(|c| c.index == 3) {
                let v = [i32::MIN, -1, 0, 12_800, i32::MAX];
                let (lo, hi) = (v[rng.below(5)], v[rng.below(5)]);
                channel.levels = Some(levels(lo as u32, hi as u32));
            }
        }
        // The mixer's records, a slot and a generator with odd states.
        27 => {
            for _ in 0..1 + rng.below(8) {
                project.initial_values.push(InitialValue {
                    unknown_0: [0; 4],
                    rec_id: rng.next() as u32,
                    value: extreme_i32(rng),
                });
            }
        }
        28 => {
            for control in [VOLUME, VOLUME + 1, VOLUME + 2, 0x1F40, 0x1FD0, 0x1FD8] {
                let insert = [0, 1, 2, 127, 9999][rng.below(5)];
                project.initial_values.push(InitialValue {
                    unknown_0: [0; 4],
                    rec_id: insert_control(insert, control),
                    value: extreme_i32(rng),
                });
            }
        }
        29 => {
            let slot = project.inserts.get_mut(1).and_then(|i| i.slots.first_mut());
            if let Some(slot) = slot {
                slot.index = [-1, 9, 10, 9999][rng.below(4)];
                let len = rng.below(30);
                slot.plugin_state = Some(rng.bytes(len));
            }
        }
        30 => {
            if let Some(channel) = project.channels.iter_mut().find(|c| c.index == 1) {
                let len = rng.below(16);
                channel.plugin_state = Some(rng.bytes(len));
            }
        }
        31 => project.insert_count = Some([0, 1, 3, 502, u16::MAX][rng.below(5)]),
        32 => {
            let versions = [None, Some("1.0.0"), Some("11.0.0"), Some("x")];
            project.writer_version = versions[rng.below(4)].map(str::to_owned);
        }
        _ => {
            if let Some(pattern) = project.patterns.first_mut() {
                pattern.length = Some([0, 1, u32::MAX][rng.below(3)]);
            }
            if let Some(arrangement) = project.arrangements.first_mut() {
                arrangement.items.clear();
            }
        }
    }
}

/// The channel whose 64 has this index.
fn channel(project: &mut Project, index: u16) -> Option<&mut Channel> {
    project.channels.iter_mut().find(|c| c.index == index)
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

/// Render a short stretch of `project` offline, and play it on an engine.
fn check(project: &Project, root: &Path, what: &str) {
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: root.to_path_buf(),
        max_seconds: 0.1,
        plugin_dir: None,
    };
    match engine::render(project, &opts) {
        Ok(out) => {
            assert_finite(&out.left, &out.right, what);
            assert!(out.left.len() <= 4800, "{what}: {} frames", out.left.len());
        }
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
    let (mut left, mut right) = (vec![0.0; 1000], vec![0.0; 1000]);
    let mut play = |engine: &mut Engine, what: &str| {
        for _ in 0..3 {
            engine.process(&mut left, &mut right);
            assert_finite(&left, &right, what);
        }
    };
    engine.play();
    play(&mut engine, what);
    for pattern in [1, 0, 777] {
        if let Err(e) = engine.set_pattern(pattern) {
            assert!(refused(&e), "{what}: {e}");
        }
        play(&mut engine, what);
    }
    if let Err(e) = engine.set_song_mode(true) {
        assert!(refused(&e), "{what}: {e}");
    }
    engine.seek(u64::MAX / 2);
    play(&mut engine, what);
    engine.set_range(Some((0, 1)));
    engine.set_loop(true);
    play(&mut engine, what);
    engine.set_range(Some((10, 5)));
    engine.seek(0);
    play(&mut engine, what);
    engine.stop();
    play(&mut engine, what);
}

/// Check every case, and fail with the list of those that failed, so that
/// one run shows them all. With `DANGLING_TRACE` set each case's name is
/// printed before it runs, for a case that takes the process down, as one
/// that runs out of memory does.
fn check_all(root: &Path, cases: impl IntoIterator<Item = (String, Project)>) {
    let mut failed = Vec::new();
    for (what, project) in cases {
        if std::env::var_os("DANGLING_TRACE").is_some() {
            eprintln!("{what}");
        }
        let run = std::panic::catch_unwind(|| check(&project, root, &what));
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

#[test]
fn the_base_project_plays() {
    let dir = Dir::new("base");
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: dir.0.clone(),
        max_seconds: 0.1,
        plugin_dir: None,
    };
    let out = engine::render(&base(), &opts).unwrap();
    assert_eq!(out.left.len(), 4800);
    assert!(out.left.iter().any(|x| x.abs() > 0.01));
    assert_eq!((out.hosted_slots, out.hosted_generators), (1, 1));
    assert_eq!((out.clips, out.automation_clips), (1, 1));
    assert!(out.unresolved.is_empty());
}

#[test]
fn each_dangling_reference_and_end_of_range_alone_plays() {
    let dir = Dir::new("each");
    let mut rng = Rng(31);
    // Each with a few draws of its details.
    let cases = (0..CHANGES).flat_map(|which| (0..4).map(move |draw| (which, draw)));
    let cases = cases.map(|(which, draw)| {
        let mut project = base();
        change(&mut project, which, &mut rng);
        (format!("change {which}, draw {draw}"), project)
    });
    check_all(&dir.0, cases.collect::<Vec<_>>());
}

#[test]
fn several_at_once_play() {
    let dir = Dir::new("several");
    let mut rng = Rng(32);
    let cases = (0..1000).map(|case| {
        let mut project = base();
        let mut applied = Vec::new();
        for _ in 0..2 + rng.below(4) {
            let which = rng.below(CHANGES);
            applied.push(which);
            change(&mut project, which, &mut rng);
        }
        (format!("case {case}, changes {applied:?}"), project)
    });
    check_all(&dir.0, cases.collect::<Vec<_>>());
}
