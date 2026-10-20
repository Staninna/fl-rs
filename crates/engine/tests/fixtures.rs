//! Projects built in code, rendered, and checked to the sample.
//!
//! Every fixture is 120 BPM at PPQ 96 and renders at 48 kHz, so a beat is
//! 24,000 samples and a bar 96,000. Its one sampler channel plays a click: a
//! WAV whose first frame is 1.0 and whose other frames are 0. Each render
//! also goes through a `.flp` on disk and back, and has to come out the
//! same.
//!
//! The audio clip fixtures add a second channel that plays a ramp: 1,000
//! frames that climb from 0.001 to 1.0, so a sample of the output says
//! which frame it came from.
//!
//! The automation fixtures add automation channels, each with its points,
//! its items on the playlist and the 227 links that bind it. Where a level
//! has to be seen moving, the sampler plays `tone.wav`, which is 1.0 all
//! the way through.
//!
//! The effect fixtures put a plugin into a slot of insert 1 of the mixer
//! fixture, as the 201, 212 and 213 that FL writes for it.
//!
//! The delay compensation fixtures play into inserts 1 and 2, one panned
//! left and the other right, so each side of the output says when one of
//! them came out.
//!
//! The generator fixtures add a plugin channel that plays TestSine, the
//! generator the `plugins` crate keeps for tests: a sine for every note,
//! whose every frame the fixtures work out for themselves.
//!
//! The controller fixtures put Fruity Peak Controller into a slot and add
//! the 227 links that carry its outputs, with a 230 where a link has a
//! formula. A tick is 250 samples, so every number in them is a count of
//! ticks times that.

use engine::{Approximate, Curves, Engine, Error, RenderOptions, Rendered, Transport};
use flp::RawEvent;
use flp::event::Id;
use flp::event::layout::{
    self, AutomationData, InitialValue, InsertFlags, Levels, PluginWrapper, Point,
};
use model::{
    Arrangement, Channel, ChannelKind, ControllerLink, Insert, Item, ItemSource, LinkKind, Note,
    Pattern, Placed, PlaylistItem, Project, Slot, Trim,
};
use plugins::Generator;
use plugins::test_sine::TestSine;
use std::f64::consts::{FRAC_PI_4, SQRT_2, TAU};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const RATE: u32 = 48_000;
const BEAT: i32 = 96;
const BAR: i32 = 4 * BEAT;
/// Frames in `ramp.wav`.
const RAMP: usize = 1000;

/// A directory under the system's temp directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("fl-rs-engine-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = Dir(dir);
        dir.wav("click.wav", &[0]);
        dir
    }

    /// A mono f32 WAV of 2,048 frames with 1.0 at each of `impulses`.
    fn wav(&self, name: &str, impulses: &[usize]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(self.0.join(name), spec).unwrap();
        for frame in 0..2048 {
            let value = if impulses.contains(&frame) { 1.0 } else { 0.0 };
            writer.write_sample::<f32>(value).unwrap();
        }
        writer.finalize().unwrap();
    }

    /// `tone.wav`: mono f32, `frames` frames of 1.0.
    fn tone(&self, frames: usize) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(self.0.join("tone.wav"), spec).unwrap();
        for _ in 0..frames {
            writer.write_sample(1f32).unwrap();
        }
        writer.finalize().unwrap();
    }

    /// `tone.wav` as white noise: mono f32, `frames` frames between -0.5
    /// and 0.5 from a generator that gives the same ones every time.
    fn noise(&self, frames: usize) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(self.0.join("tone.wav"), spec).unwrap();
        let mut state = 0x2545_f491u32;
        for _ in 0..frames {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            writer
                .write_sample((state >> 8) as f32 / 16_777_216.0 - 0.5)
                .unwrap();
        }
        writer.finalize().unwrap();
    }

    /// `tone.wav` as a sine: mono f32, `frames` frames of 100 Hz at half
    /// scale.
    fn sine(&self, frames: usize) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(self.0.join("tone.wav"), spec).unwrap();
        for frame in 0..frames {
            let turns = frame as f64 * 100.0 / f64::from(RATE);
            let level = 0.5 * (turns * std::f64::consts::TAU).sin();
            writer.write_sample(level as f32).unwrap();
        }
        writer.finalize().unwrap();
    }

    /// `ramp.wav`: mono f32, [`RAMP`] frames, frame `i` at `(i + 1) / 1000`.
    fn ramp(&self) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(self.0.join("ramp.wav"), spec).unwrap();
        for frame in 0..RAMP {
            writer.write_sample(ramp(frame)).unwrap();
        }
        writer.finalize().unwrap();
    }
}

/// The level of a frame of `ramp.wav`.
fn ramp(frame: usize) -> f32 {
    (frame + 1) as f32 / RAMP as f32
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn note(position: i32) -> Note {
    Note {
        position,
        key: 60,
        ..Note::default()
    }
}

/// A bar of a pattern.
fn item(position: i32, pattern: u16) -> PlaylistItem {
    PlaylistItem::new(ItemSource::Pattern(pattern), position, BAR)
}

/// `ticks` of the audio clip channel that [`with_clips`] adds.
fn clip(position: i32, ticks: i32) -> PlaylistItem {
    PlaylistItem::new(ItemSource::Channel(1), position, ticks)
}

/// A project as FL 20.8 would write it: the version, the tempo, the
/// patterns, one sampler channel playing `click.wav` into the master, and
/// one arrangement of 32-byte items. It has no mixer events. [`with_mixer`]
/// adds them.
fn project(patterns: &[(u16, &[Note])], items: &[PlaylistItem]) -> Project {
    project_from("20.8.4.2576", None, patterns, items)
}

/// The same project as FL 26.1 would write it: a 169 with both size bits
/// set, and items of 88 bytes, which have a gain, a pitch, a stretch, a
/// direction and a pan.
fn project_26(patterns: &[(u16, &[Note])], items: &[PlaylistItem]) -> Project {
    project_from("26.1.0.4975", Some(0xf), patterns, items)
}

fn project_from(
    version: &str,
    flags169: Option<u32>,
    patterns: &[(u16, &[Note])],
    items: &[PlaylistItem],
) -> Project {
    let mut pos = 0;
    let mut field = |id: Id| {
        pos += 1;
        Placed {
            pos: pos - 1,
            item: Item::Field(id),
        }
    };
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 1,
        ppq: BEAT as u16,
    });
    project.writer_version = Some(version.to_owned());
    project.tempo_milli = Some(120_000);
    project.order = vec![field(Id::FLVersion), field(Id::Tempo)];
    if let Some(flags) = flags169 {
        // The model has no field for 169, so it is a raw event.
        let mut placed = field(Id::DataFormatFlags);
        placed.item = Item::Raw(RawEvent {
            id: Id::DataFormatFlags.0,
            data: flags.to_le_bytes().to_vec(),
        });
        project.order.push(placed);
    }
    let version = flp::event::version_number(version).unwrap();
    let size = PlaylistItem::size_for(version, flags169.unwrap_or(0));
    for (index, notes) in patterns {
        project.patterns.push(Pattern {
            index: *index,
            notes: notes.to_vec(),
            order: vec![field(Id::PatternNew), field(Id::PatternNotes)],
            ..Pattern::default()
        });
    }
    project.channels.push(Channel {
        index: 0,
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
        routed_to: Some(0),
        order: vec![
            field(Id::ChannelNew),
            field(Id::ChannelType),
            field(Id::SamplePath),
            field(Id::Levels),
            field(Id::RoutedTo),
        ],
        ..Channel::default()
    });
    project.arrangements.push(Arrangement {
        index: 0,
        items: items
            .iter()
            .map(|item| item.shortened(size).unwrap())
            .collect(),
        order: vec![field(Id::ArrangementNew), field(Id::ArrangementPlaylist)],
        ..Arrangement::default()
    });
    project
}

/// Insert volume, insert pan, and the level of the send to insert 0: the
/// control numbers FL 20.8 writes into a 225 record's ID.
const VOLUME: u32 = 0x1FC0;
const PAN: u32 = 0x1FC1;
const SEND: u32 = 0x1F40;
/// The ID of the main volume's record.
const MAIN_VOLUME: u32 = 0x4000_0000;

/// Give a project the mixer FL 20.8 would write, cut down to the master
/// and inserts 1 to 3: every insert switched on, the three sending to the
/// master, and an empty 225. Channel 0 moves to insert 1.
///
/// The events are 10 places apart, which leaves room for [`add_effect`] to
/// put a slot's events ahead of the 147 that closes their insert.
fn with_mixer(mut project: Project) -> Project {
    let mut pos = 1000;
    let mut field = |id: Id| {
        pos += 10;
        Placed {
            pos,
            item: Item::Field(id),
        }
    };
    project.order.push(field(Id::MixerParams));
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
    project.channels[0].routed_to = Some(1);
    project
}

/// Add a 225 record for a control of an insert, under the ID FL 20.8 gives
/// it.
fn set(project: &mut Project, insert: u32, control: u32, value: i32) {
    record(project, 0x2000_0000 + (insert << 22) + control, value);
}

fn record(project: &mut Project, rec_id: u32, value: i32) {
    project.initial_values.push(InitialValue {
        unknown_0: [0; 4],
        rec_id,
        value,
    });
}

/// Whether a slot is switched on, and its mix level: the control numbers
/// of the two 225 records every slot has.
const SLOT_ENABLED: u32 = 0x1F00;
const SLOT_MIX: u32 = 0x1F01;

/// Add a 225 record for a control of a slot of an insert.
fn set_slot(project: &mut Project, insert: u32, slot: u32, control: u32, value: i32) {
    set(project, insert, (slot << 16) + control, value);
}

/// The 213 of a plugin whose state is its parameters.
fn params(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Fruity Fast Dist as FL sets a new one: type A at threshold 10, all wet.
const FAST_DIST: [i32; 5] = [128, 10, 0, 128, 128];

/// Put a plugin into a slot of an insert of the [`with_mixer`] fixture: its
/// name, the wrapper that opens the slot, and its state. An insert takes up
/// to three.
fn add_effect(project: &mut Project, insert: usize, slot: i32, name: &str, state: &[u8]) {
    let target = &mut project.inserts[insert];
    let close = target.order.last().unwrap().pos;
    let first = close - 9 + 3 * target.slots.len();
    assert!(first + 2 < close, "no room for another slot");
    let ids = [Id::PluginInternalName, Id::PluginWrapper, Id::PluginData];
    let placed = |(n, id)| Placed {
        pos: first + n,
        item: Item::Field(id),
    };
    target.slots.push(Slot {
        index: slot,
        plugin_name: Some(name.to_owned()),
        wrapper: Some(PluginWrapper {
            insert: insert as i32,
            slot,
            plugin_kind: 2,
            unknown_12: [0; 4],
            flags: 0x140,
            page: 0,
            unknown_21: [0; 15],
            editor_position: 0,
            editor_size: 0,
            len: 52,
        }),
        plugin_state: Some(state.to_vec()),
        order: ids.into_iter().enumerate().map(placed).collect(),
        ..Slot::default()
    });
}

/// Replace an insert's routes with sends to `destinations`.
fn route(project: &mut Project, insert: usize, destinations: &[usize]) {
    let bytes = (0..4).map(|dest| u8::from(destinations.contains(&dest)));
    project.inserts[insert].routing = Some(bytes.collect());
}

/// A second sampler channel that plays the click into `insert`.
fn add_channel(project: &mut Project, insert: i8) {
    let mut channel = project.channels[0].clone();
    channel.index = 1;
    channel.routed_to = Some(insert);
    for placed in &mut channel.order {
        placed.pos += 100;
    }
    project.channels.push(channel);
}

/// A second channel, an audio clip that plays `ramp.wav` into the master.
fn with_clips(mut project: Project) -> Project {
    add_channel(&mut project, 0);
    let channel = &mut project.channels[1];
    channel.kind = Some(ChannelKind::AudioClip);
    channel.sample_path = Some("ramp.wav".to_owned());
    project
}

/// A project of clips alone, as FL 26.1 would write it.
fn clips(items: &[PlaylistItem]) -> Project {
    with_clips(project_26(&[], items))
}

/// Four notes, one on each beat of a bar.
fn four_beats() -> [Note; 4] {
    [0, 1, 2, 3].map(|beat| note(beat * BEAT))
}

/// Save the project as a `.flp` in `dir` and read that file back.
fn reload(project: &Project, dir: &Path) -> Project {
    let file = dir.join("fixture.flp");
    std::fs::write(&file, model::to_flp(project).unwrap().write()).unwrap();
    model::from_flp(&flp::Project::parse(&std::fs::read(&file).unwrap()).unwrap())
}

fn try_render(project: &Project, dir: &Dir) -> Result<Rendered, Error> {
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: dir.0.clone(),
        max_seconds: 60.0,
        plugin_dir: None,
    };
    engine::render(project, &opts)
}

/// Render the project, and render it again from its `.flp`. The two have to
/// agree.
fn render(project: &Project, dir: &Dir) -> Rendered {
    let out = try_render(project, dir).unwrap();
    let again = try_render(&reload(project, &dir.0), dir).unwrap();
    assert!(out == again, "the render changed after a save as .flp");
    out
}

/// Where the buffer is not silent.
fn impulses(buffer: &[f32]) -> Vec<usize> {
    let loud = buffer.iter().enumerate().filter(|(_, v)| v.abs() > 1e-4);
    loud.map(|(i, _)| i).collect()
}

fn assert_impulses(buffer: &[f32], want: &[usize]) {
    let got = impulses(buffer);
    let close = got.len() == want.len() && got.iter().zip(want).all(|(g, w)| g.abs_diff(*w) <= 1);
    assert!(close, "impulses at {got:?}, wanted {want:?}");
}

#[test]
fn notes_on_the_beats_land_on_the_beats() {
    let dir = Dir::new("beats");
    let out = render(&project(&[(1, &four_beats())], &[item(0, 1)]), &dir);
    assert_impulses(&out.left, &[0, 24_000, 48_000, 72_000]);
    assert_eq!(out.left, out.right);
    assert_eq!(out.left.len(), 96_000);
    assert_eq!(out.sample_rate, RATE);
    assert_eq!((out.notes, out.voices, out.skipped), (4, 4, 0));
    assert!(out.unresolved.is_empty() && out.unsupported.is_empty());
}

#[test]
fn the_fixture_is_the_same_project_after_a_save_as_flp() {
    let dir = Dir::new("flp");
    let mut trimmed = item(2 * BAR, 2);
    trimmed.set_trim(Some(Trim::Ticks {
        start: BEAT,
        end: BEAT + BAR,
    }));
    let mut muted = item(4 * BAR, 1);
    muted.set_muted(true);
    let saved = project(
        &[(1, &four_beats()), (2, &[note(BEAT)])],
        &[item(0, 1), trimmed, muted],
    );
    let loaded = reload(&saved, &dir.0);
    assert_eq!(loaded, saved);
    let out = render(&saved, &dir);
    assert_impulses(&out.left, &[0, 24_000, 48_000, 72_000, 192_000]);
    assert_eq!(out.left.len(), 5 * 96_000);

    // The 88-byte items of FL 26.1 come back whole too.
    let mut reversed = clip(BAR, BEAT);
    reversed.set_trim(Some(Trim::Clip {
        start: 2.5,
        end: 10.0,
    }));
    (reversed.gain, reversed.pitch, reversed.pan) = (0.5, -3.25, 0.75);
    (reversed.stretch, reversed.clip_flags) = (1.5, PlaylistItem::REVERSED);
    let saved = project_26(
        &[(1, &four_beats())],
        &[item(0, 1), trimmed, muted, reversed],
    );
    let loaded = reload(&saved, &dir.0);
    assert_eq!(loaded, saved);
    assert_eq!(loaded.arrangements[0].items[3].len, 88);
    assert_eq!(loaded.arrangements[0].items[3].clip(), reversed.clip());
}

#[test]
fn half_the_velocity_is_half_the_amplitude() {
    let dir = Dir::new("velocity");
    let mut quiet = note(BEAT);
    quiet.velocity = 50;
    let out = render(&project(&[(1, &[note(0), quiet])], &[item(0, 1)]), &dir);
    assert_impulses(&out.left, &[0, 24_000]);
    assert_eq!((out.left[0], out.left[24_000]), (1.0, 0.5));
}

#[test]
fn full_left_pan_silences_the_right_side() {
    let dir = Dir::new("pan");
    let mut panned = project(&[(1, &four_beats())], &[item(0, 1)]);
    panned.channels[0].levels.as_mut().unwrap().pan = 0;
    let out = render(&panned, &dir);
    assert_impulses(&out.left, &[0, 24_000, 48_000, 72_000]);
    assert_impulses(&out.right, &[]);

    // The note's own pan does the same on a centred channel.
    let mut left = note(0);
    left.pan = 0;
    let out = render(&project(&[(1, &[left])], &[item(0, 1)]), &dir);
    assert_impulses(&out.left, &[0]);
    assert_impulses(&out.right, &[]);
}

#[test]
fn the_gap_between_two_items_is_silent() {
    let dir = Dir::new("gap");
    let out = render(
        &project(
            &[(1, &four_beats()), (2, &[note(0), note(BEAT)])],
            &[item(0, 1), item(2 * BAR, 2)],
        ),
        &dir,
    );
    assert_impulses(&out.left, &[0, 24_000, 48_000, 72_000, 192_000, 216_000]);
    // The click's 2,048 frames are over long before bar 2 starts.
    assert!(out.left[96_000..192_000].iter().all(|v| *v == 0.0));
    assert!(out.right[96_000..192_000].iter().all(|v| *v == 0.0));
    assert_eq!(out.left.len(), 3 * 96_000);
}

#[test]
fn a_note_on_a_muted_item_is_silent() {
    let dir = Dir::new("muted");
    let mut muted = item(BAR, 1);
    muted.set_muted(true);
    let out = render(&project(&[(1, &four_beats())], &[item(0, 1), muted]), &dir);
    assert_impulses(&out.left, &[0, 24_000, 48_000, 72_000]);
    assert_eq!(out.left.len(), 2 * 96_000);
    assert_eq!((out.notes, out.voices), (4, 4));
}

#[test]
fn a_trimmed_item_plays_its_window_of_the_pattern() {
    let dir = Dir::new("trim");
    let mut trimmed = item(BAR, 1);
    trimmed.length = 2 * BEAT;
    trimmed.set_trim(Some(Trim::Ticks {
        start: BEAT,
        end: 3 * BEAT,
    }));
    let out = render(&project(&[(1, &four_beats())], &[trimmed]), &dir);
    assert_impulses(&out.left, &[96_000, 120_000]);
}

#[test]
fn a_muted_note_is_silent() {
    let dir = Dir::new("muted-note");
    let mut notes = four_beats();
    notes[1].flags = Note::MUTED;
    let out = render(&project(&[(1, &notes)], &[item(0, 1)]), &dir);
    assert_impulses(&out.left, &[0, 48_000, 72_000]);
    assert_eq!((out.notes, out.voices), (3, 3));
}

#[test]
fn a_note_s_fine_pitch_moves_it_10_cents_a_step() {
    let dir = Dir::new("fine-pitch");
    dir.wav("click.wav", &[0, 1000]);
    // 120 steps above the centre are an octave.
    let mut high = note(BEAT);
    high.fine_pitch = 240;
    let out = render(&project(&[(1, &[note(0), high])], &[item(0, 1)]), &dir);
    assert_impulses(&out.left, &[0, 1000, 24_000, 24_500]);
}

/// Whether the output from `start` on is these frames of the ramp, each
/// times `gain`.
fn assert_ramp(buffer: &[f32], start: usize, frames: impl Iterator<Item = usize>, gain: f32) {
    for (i, frame) in frames.enumerate() {
        assert_eq!(buffer[start + i], gain * ramp(frame), "sample {i}");
    }
}

#[test]
fn a_clip_plays_its_channel_s_sample_from_where_the_item_is() {
    let dir = Dir::new("clip");
    dir.ramp();
    // Half-way through the bar, for a beat.
    let out = render(&clips(&[clip(2 * BEAT, BEAT)]), &dir);
    assert!(out.left[..48_000].iter().all(|v| *v == 0.0));
    assert_ramp(&out.left, 48_000, 0..RAMP, 1.0);
    assert!(out.left[48_000 + RAMP..].iter().all(|v| *v == 0.0));
    assert_eq!(out.left, out.right);
    assert_eq!(out.left.len(), 72_000);
    assert_eq!((out.notes, out.voices, out.skipped), (0, 0, 0));
    assert_eq!((out.clips, out.unsupported_stretch), (1, 0));

    // The clip ends with its item: 2 ticks are 500 samples.
    let out = render(&clips(&[clip(2 * BEAT, 2)]), &dir);
    assert_ramp(&out.left, 48_000, 0..500, 1.0);
    assert_eq!(out.left.len(), 48_500);

    // A 32-byte item has no gain of its own and plays at full level.
    let old = with_clips(project(&[(1, &[note(0)])], &[item(0, 1), clip(BEAT, BEAT)]));
    assert_eq!(old.arrangements[0].items[1].len, 32);
    let out = render(&old, &dir);
    assert_eq!(out.left[0], 1.0);
    assert_ramp(&out.left, 24_000, 0..RAMP, 1.0);
    assert_eq!((out.notes, out.voices, out.clips), (1, 1, 1));
}

#[test]
fn a_clip_s_gain_scales_it_and_its_reverse_flag_turns_it_round() {
    let dir = Dir::new("clip-reverse");
    dir.ramp();
    let mut quiet = clip(0, BEAT);
    quiet.gain = 0.5;
    let out = render(&clips(&[quiet]), &dir);
    assert_ramp(&out.left, 0, 0..RAMP, 0.5);
    assert_eq!(out.left, out.right);

    // Backwards, the ramp falls from its last frame to its first.
    let mut reversed = quiet;
    reversed.clip_flags = PlaylistItem::REVERSED;
    let out = render(&clips(&[reversed]), &dir);
    assert_eq!((out.left[0], out.left[RAMP - 1]), (0.5, 0.0005));
    assert_ramp(&out.left, 0, (0..RAMP).rev(), 0.5);
    assert_eq!(out.left[RAMP], 0.0);
}

#[test]
fn a_trimmed_clip_starts_inside_its_sample() {
    let dir = Dir::new("clip-trim");
    dir.ramp();
    // 5 ms of a 48 kHz sample are 240 frames.
    let mut trimmed = clip(BEAT, BEAT);
    trimmed.set_trim(Some(Trim::Clip {
        start: 5.0,
        end: 15.0,
    }));
    let out = render(&clips(&[trimmed]), &dir);
    assert_ramp(&out.left, 24_000, 240..RAMP, 1.0);
    assert_eq!(out.left[24_000 + RAMP - 240], 0.0);

    // Backwards, the 240 frames are counted from the sample's end.
    trimmed.clip_flags = PlaylistItem::REVERSED;
    let out = render(&clips(&[trimmed]), &dir);
    assert_ramp(&out.left, 24_000, (0..RAMP - 240).rev(), 1.0);
    assert_eq!(out.left[24_000 + RAMP - 240], 0.0);

    // A trim past the sample's end plays nothing.
    trimmed.set_trim(Some(Trim::Clip {
        start: 1000.0,
        end: 2000.0,
    }));
    let out = render(&clips(&[trimmed]), &dir);
    assert_impulses(&out.left, &[]);
    assert_eq!(out.clips, 0);
}

#[test]
fn a_clip_s_pan_and_pitch_go_on_top_of_its_channel_s() {
    let dir = Dir::new("clip-pan-pitch");
    dir.ramp();
    let mut left = clip(0, BEAT);
    left.pan = -1.0;
    let out = render(&clips(&[left]), &dir);
    assert_ramp(&out.left, 0, 0..RAMP, 1.0);
    assert!(out.right.iter().all(|v| *v == 0.0));

    // An octave up reads every other frame.
    let mut high = clip(0, BEAT);
    high.pitch = 12.0;
    let out = render(&clips(&[high]), &dir);
    for i in [0, 1, 250, 499] {
        assert!((out.left[i] - ramp(2 * i)).abs() < 1e-5, "sample {i}");
    }
    assert_eq!(out.left[500], 0.0);

    // The channel's volume, pan and pitch knob count as they do for a note.
    let mut song = clips(&[clip(0, BEAT)]);
    let levels = song.channels[1].levels.as_mut().unwrap();
    (levels.volume, levels.pan) = (5000, 12_800);
    let out = render(&song, &dir);
    assert!(out.left.iter().all(|v| *v == 0.0));
    assert_ramp(&out.right, 0, 0..RAMP, 0.5);
    song.channels[1].levels.as_mut().unwrap().unknown_8 = 1200i32.to_le_bytes();
    assert!((render(&song, &dir).right[499] - 0.5 * ramp(998)).abs() < 1e-5);
}

#[test]
fn a_stretched_clip_plays_at_its_own_speed_and_is_counted() {
    let dir = Dir::new("clip-stretch");
    dir.ramp();
    let mut stretched = clip(0, BEAT);
    stretched.stretch = 2.0;
    let out = render(&clips(&[stretched, clip(BEAT, BEAT)]), &dir);
    assert_ramp(&out.left, 0, 0..RAMP, 1.0);
    assert_ramp(&out.left, 24_000, 0..RAMP, 1.0);
    assert_eq!((out.clips, out.unsupported_stretch), (2, 1));
}

#[test]
fn clips_that_do_not_play_are_not_counted() {
    let dir = Dir::new("clip-silent");
    dir.ramp();
    let mut muted = clip(0, BEAT);
    muted.set_muted(true);
    let out = render(&clips(&[muted]), &dir);
    assert_impulses(&out.left, &[]);
    assert_eq!((out.clips, out.left.len()), (0, 24_000));

    let mut off = clips(&[clip(0, BEAT)]);
    off.channels[1].enabled = Some(false);
    assert_eq!(try_render(&off, &dir).unwrap().clips, 0);

    // An item on a sampler channel is no clip.
    let mut sampler = clips(&[clip(0, BEAT)]);
    sampler.channels[1].kind = Some(ChannelKind::Sampler);
    let out = render(&sampler, &dir);
    assert_impulses(&out.left, &[]);
    assert_eq!(out.clips, 0);

    let mut lost = clips(&[clip(0, BEAT)]);
    lost.channels[1].sample_path = Some("lost.wav".to_owned());
    let out = render(&lost, &dir);
    assert_eq!(
        (out.clips, out.unresolved),
        (0, vec!["lost.wav".to_owned()])
    );

    // A clip that starts at the limit or after it is cut with the rest.
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: dir.0.clone(),
        max_seconds: 0.5,
        plugin_dir: None,
    };
    let song = clips(&[clip(0, BEAT), clip(BEAT, BEAT)]);
    let out = engine::render(&song, &opts).unwrap();
    assert_eq!((out.clips, out.left.len()), (1, 24_000));
}

#[test]
fn an_empty_playlist_plays_pattern_1_once() {
    let dir = Dir::new("pattern-mode");
    let out = render(
        &project(&[(2, &[note(BEAT / 2)]), (1, &four_beats())], &[]),
        &dir,
    );
    assert_impulses(&out.left, &[0, 24_000, 48_000, 72_000]);
    assert_eq!(out.left.len(), 96_000);
}

#[test]
fn an_octave_up_plays_the_sample_twice_as_fast() {
    let dir = Dir::new("pitch");
    dir.wav("click.wav", &[0, 1000]);
    let mut high = note(BEAT);
    high.key = 72;
    let out = render(&project(&[(1, &[note(0), high])], &[item(0, 1)]), &dir);
    assert_impulses(&out.left, &[0, 1000, 24_000, 24_500]);

    // An octave down reads between the frames, which smears each impulse
    // over its neighbours. The peaks are 2,000 samples apart.
    let mut low = note(0);
    low.key = 48;
    let out = render(&project(&[(1, &[low])], &[item(0, 1)]), &dir);
    assert_eq!((out.left[0], out.left[2000]), (1.0, 1.0));
    assert!(
        impulses(&out.left)
            .iter()
            .all(|i| *i <= 3 || i.abs_diff(2000) <= 3)
    );

    // The channel's pitch knob, in cents, moves every note.
    let mut shifted = project(&[(1, &[note(0)])], &[item(0, 1)]);
    shifted.channels[0].levels.as_mut().unwrap().unknown_8 = 1200i32.to_le_bytes();
    assert_impulses(&render(&shifted, &dir).left, &[0, 500]);
}

#[test]
fn a_stereo_sample_keeps_its_sides_and_follows_its_own_rate() {
    let dir = Dir::new("stereo");
    // 16-bit stereo at half the output rate, with a click on the right at
    // frames 0 and 100.
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE / 2,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(dir.0.join("click.wav"), spec).unwrap();
    for frame in 0..512 {
        writer.write_sample(0i16).unwrap();
        let right = if frame % 100 == 0 { i16::MIN } else { 0 };
        writer.write_sample(right).unwrap();
    }
    writer.finalize().unwrap();
    let out = render(&project(&[(1, &[note(0)])], &[item(0, 1)]), &dir);
    assert_impulses(&out.left, &[]);
    for peak in [0, 200, 400, 600, 800, 1000] {
        assert_eq!(out.right[peak], -1.0, "{peak}");
    }
    assert_eq!(out.right[100], 0.0);
}

#[test]
fn a_held_note_stops_at_its_length() {
    let dir = Dir::new("length");
    dir.wav("click.wav", &[0, 1000]);
    let mut held = note(0);
    // A tick is 250 samples, so 3 ticks end before the second impulse.
    held.length = 3;
    let out = render(&project(&[(1, &[held])], &[item(0, 1)]), &dir);
    assert_impulses(&out.left, &[0]);
}

#[test]
fn the_render_stops_at_max_seconds() {
    let dir = Dir::new("limit");
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: dir.0.clone(),
        max_seconds: 1.25,
        plugin_dir: None,
    };
    let song = project(&[(1, &four_beats())], &[item(0, 1), item(BAR, 1)]);
    let out = engine::render(&song, &opts).unwrap();
    assert_eq!(out.left.len(), 60_000);
    assert_impulses(&out.left, &[0, 24_000, 48_000]);
    assert_eq!((out.notes, out.voices), (3, 3));
}

#[test]
fn a_missing_sample_is_counted_and_not_an_error() {
    let dir = Dir::new("missing");
    let mut lost = project(&[(1, &four_beats())], &[item(0, 1)]);
    lost.channels[0].sample_path = Some(r"%FLStudioFactoryData%\Data\Patches\Lost.wav".to_owned());
    let out = render(&lost, &dir);
    assert_impulses(&out.left, &[]);
    assert_eq!((out.notes, out.voices, out.left.len()), (4, 0, 96_000));
    assert_eq!(
        out.unresolved,
        [r"%FLStudioFactoryData%\Data\Patches\Lost.wav"]
    );
}

#[test]
fn a_sample_that_is_there_and_does_not_decode_is_an_error() {
    let dir = Dir::new("broken");
    // A WAV header that promises 16-bit PCM and stops inside its `fmt `.
    let mut bytes = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
    bytes.extend_from_slice(&[0x80, 0xbb]);
    std::fs::write(dir.0.join("click.wav"), bytes).unwrap();
    let broken = project(&[(1, &four_beats())], &[item(0, 1)]);
    match try_render(&broken, &dir) {
        Err(Error::Decode { path, .. }) => assert_eq!(path, dir.0.join("click.wav")),
        other => panic!("wanted a decode error, got {other:?}"),
    }
}

/// The header of a WavPack block whose flags say hybrid, which is the lossy
/// mode and the one kind of WavPack FL ships that is not decoded.
fn hybrid_wavpack() -> Vec<u8> {
    let mut block = b"wvpk".to_vec();
    block.extend_from_slice(&24u32.to_le_bytes());
    block.extend_from_slice(&0x407u16.to_le_bytes());
    block.extend_from_slice(&[0; 14]);
    block.extend_from_slice(&8u32.to_le_bytes());
    block.extend_from_slice(&[0; 4]);
    block
}

#[test]
fn a_format_without_a_decoder_is_counted_and_not_an_error() {
    let dir = Dir::new("wavpack");
    std::fs::write(dir.0.join("click.wv"), hybrid_wavpack()).unwrap();
    let mut packed = project(&[(1, &four_beats())], &[item(0, 1)]);
    packed.channels[0].sample_path = Some("click.wv".to_owned());
    let out = render(&packed, &dir);
    assert_eq!((out.notes, out.voices), (4, 0));
    assert_eq!(out.unsupported, ["click.wv"]);
    assert!(out.unresolved.is_empty());
}

#[test]
fn channels_that_are_not_samplers_are_skipped_and_counted() {
    let dir = Dir::new("skipped");
    let mut on_plugin = note(BEAT);
    on_plugin.channel = 1;
    let mut mixed = project(&[(1, &[note(0), on_plugin])], &[item(0, 1)]);
    let mut plugin = mixed.channels[0].clone();
    plugin.index = 1;
    plugin.kind = Some(ChannelKind::Plugin);
    for placed in &mut plugin.order {
        placed.pos += 100;
    }
    mixed.channels.push(plugin);
    for kind in [ChannelKind::Layer, ChannelKind::Automation] {
        mixed.channels[1].kind = Some(kind);
        let out = render(&mixed, &dir);
        assert_impulses(&out.left, &[0]);
        assert_eq!((out.notes, out.voices, out.skipped), (2, 1, 1));
    }
    // An audio clip channel is one the engine plays, so it is not counted.
    // A note on it plays nothing all the same: only playlist items do.
    mixed.channels[1].kind = Some(ChannelKind::AudioClip);
    let out = render(&mixed, &dir);
    assert_impulses(&out.left, &[0]);
    assert_eq!((out.notes, out.voices, out.skipped), (2, 1, 0));

    // A sampler channel that is switched off plays nothing either.
    let mut off = project(&[(1, &four_beats())], &[item(0, 1)]);
    off.channels[0].enabled = Some(false);
    assert_impulses(&try_render(&off, &dir).unwrap().left, &[]);
}

#[test]
fn a_project_without_a_tempo_does_not_render() {
    let dir = Dir::new("tempo");
    let mut timeless = project(&[(1, &four_beats())], &[item(0, 1)]);
    timeless.tempo_milli = None;
    assert!(matches!(try_render(&timeless, &dir), Err(Error::Tempo)));
}

/// The mixer fixture with one click at the start.
fn one_click() -> Project {
    with_mixer(project(&[(1, &[note(0)])], &[item(0, 1)]))
}

#[test]
fn an_insert_s_volume_scales_what_plays_into_it() {
    let dir = Dir::new("insert-volume");
    // No record at all is an untouched fader.
    let out = render(&one_click(), &dir);
    assert_eq!((out.left[0], out.right[0]), (1.0, 1.0));

    let mut unity = one_click();
    set(&mut unity, 1, VOLUME, 12_800);
    let out = render(&unity, &dir);
    assert_impulses(&out.left, &[0]);
    assert_eq!((out.left[0], out.right[0]), (1.0, 1.0));

    let mut silent = one_click();
    set(&mut silent, 1, VOLUME, 0);
    let out = render(&silent, &dir);
    assert!(out.left.iter().chain(&out.right).all(|v| *v == 0.0));
    assert_eq!((out.notes, out.voices), (1, 1));

    // Half the way down in dB terms, and the last record is the one that
    // counts.
    let mut quiet = one_click();
    set(&mut quiet, 1, VOLUME, 0);
    set(&mut quiet, 1, VOLUME, 9600);
    let out = render(&quiet, &dir);
    assert!((20.0 * out.left[0].log10() + 5.6).abs() < 1e-3);
    assert_eq!(out.left[0], out.right[0]);

    // Another insert's fader is none of this one's business.
    let mut other = one_click();
    set(&mut other, 2, VOLUME, 0);
    assert_eq!(render(&other, &dir).left[0], 1.0);
}

#[test]
fn an_insert_panned_left_silences_the_right_side() {
    let dir = Dir::new("insert-pan");
    let mut panned = one_click();
    set(&mut panned, 1, PAN, -6400);
    let out = render(&panned, &dir);
    assert_impulses(&out.left, &[0]);
    assert_eq!(out.left[0], 1.0);
    assert!(out.right.iter().all(|v| *v == 0.0));

    let mut panned = one_click();
    set(&mut panned, 1, PAN, 6400);
    let out = render(&panned, &dir);
    assert!(out.left.iter().all(|v| *v == 0.0));
    assert_eq!(out.right[0], 1.0);
}

#[test]
fn a_route_carries_an_insert_through_another_to_the_master() {
    let dir = Dir::new("route");
    let routed = || {
        let mut routed = one_click();
        route(&mut routed, 1, &[2]);
        routed
    };
    assert_eq!(render(&routed(), &dir).left[0], 1.0);

    // Insert 2 pulled down takes insert 1 with it.
    let mut closed = routed();
    set(&mut closed, 2, VOLUME, 0);
    let out = render(&closed, &dir);
    assert!(out.left.iter().chain(&out.right).all(|v| *v == 0.0));

    // So does a send at 0, and a send level scales like a fader.
    let mut unsent = routed();
    set(&mut unsent, 1, SEND + 2, 0);
    assert_impulses(&render(&unsent, &dir).left, &[]);
    let mut sent = routed();
    set(&mut sent, 1, SEND + 2, 9600);
    assert!((20.0 * render(&sent, &dir).left[0].log10() + 5.6).abs() < 1e-3);

    // An insert that sends nowhere is not heard.
    let mut nowhere = one_click();
    route(&mut nowhere, 1, &[]);
    assert_impulses(&render(&nowhere, &dir).left, &[]);
}

#[test]
fn two_inserts_that_send_to_the_master_sum() {
    let dir = Dir::new("sum");
    let mut second = note(0);
    second.channel = 1;
    let mut both = with_mixer(project(&[(1, &[note(0), second])], &[item(0, 1)]));
    add_channel(&mut both, 2);
    let out = render(&both, &dir);
    assert_impulses(&out.left, &[0]);
    assert_eq!((out.left[0], out.right[0]), (2.0, 2.0));
    assert_eq!((out.notes, out.voices), (2, 2));

    // One insert sending both to the master and to another one arrives
    // twice as well.
    let mut split = one_click();
    route(&mut split, 1, &[0, 2]);
    assert_eq!(render(&split, &dir).left[0], 2.0);

    // Each insert keeps its own fader on the way.
    set(&mut both, 2, VOLUME, 0);
    assert_eq!(render(&both, &dir).left[0], 1.0);
}

#[test]
fn a_route_cycle_is_an_error() {
    let dir = Dir::new("cycle");
    let mut looped = one_click();
    route(&mut looped, 1, &[2]);
    route(&mut looped, 2, &[3]);
    route(&mut looped, 3, &[0, 1]);
    assert!(matches!(
        try_render(&looped, &dir),
        Err(Error::RouteCycle { insert: 1..=3 })
    ));
    // A send at 0 is still a route.
    set(&mut looped, 3, SEND + 1, 0);
    assert!(matches!(
        try_render(&looped, &dir),
        Err(Error::RouteCycle { .. })
    ));
    // So is a loop nothing plays into, and an insert that sends to itself.
    let mut aside = one_click();
    route(&mut aside, 2, &[3]);
    route(&mut aside, 3, &[2]);
    assert!(matches!(
        try_render(&aside, &dir),
        Err(Error::RouteCycle { .. })
    ));
    let mut own = one_click();
    route(&mut own, 1, &[0, 1]);
    assert!(matches!(
        try_render(&own, &dir),
        Err(Error::RouteCycle { insert: 1 })
    ));
}

#[test]
fn an_insert_that_is_switched_off_passes_nothing_on() {
    let dir = Dir::new("insert-off");
    let mut second = note(BEAT);
    second.channel = 1;
    let mut off = with_mixer(project(&[(1, &[note(0), second])], &[item(0, 1)]));
    add_channel(&mut off, 2);
    off.inserts[1].flags.as_mut().unwrap().flags = 0b0100;
    let out = render(&off, &dir);
    assert_impulses(&out.left, &[24_000]);
    assert_eq!((out.notes, out.voices), (2, 2));
}

#[test]
fn the_main_volume_goes_on_the_master_last() {
    let dir = Dir::new("main-volume");
    let mut quiet = one_click();
    set(&mut quiet, 0, VOLUME, 9600);
    record(&mut quiet, MAIN_VOLUME, 9600);
    let out = render(&quiet, &dir);
    assert!((20.0 * out.left[0].log10() + 11.2).abs() < 1e-3);
    record(&mut quiet, MAIN_VOLUME, 0);
    assert_impulses(&render(&quiet, &dir).left, &[]);
}

#[test]
fn fl_s_fader_curve_can_stand_in_for_the_approximate_one() {
    let dir = Dir::new("fl-exact");
    let mut quiet = one_click();
    set(&mut quiet, 1, VOLUME, 6400);
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: dir.0.clone(),
        max_seconds: 60.0,
        plugin_dir: None,
    };
    let out = engine::render_with(&quiet, &opts, &engine::FlExact).unwrap();
    // (11^0.5 - 1) / 10 of the level FL's curves play the note at.
    assert!((out.left[0] - 0.231_662_5 * circular(0.0, new_channel()).0).abs() < 1e-6);
    assert!(out.left[0] < render(&quiet, &dir).left[0]);
}

/// The 0.551 that FL's curve makes of the 10000 a new channel's volume is
/// at: (11^(10000 / 12800) - 1) / 10.
fn new_channel() -> f64 {
    (11f64.powf(10_000.0 / 12_800.0) - 1.0) / 10.0
}

/// Left and right gain of FL's `ComputeLRVol` at a level: the level over
/// the square root of 2 in the centre.
fn circular(pan: f64, vol: f64) -> (f32, f32) {
    let side = |pan: f64| (vol * ((1.0 + pan) * FRAC_PI_4).sin()) as f32;
    (side(-pan), side(pan))
}

fn assert_gains(out: &Rendered, at: usize, want: (f32, f32)) {
    let got = (out.left[at], out.right[at]);
    let close = (got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6;
    assert!(close, "{got:?} at {at}, wanted {want:?}");
}

#[test]
fn fl_s_curves_play_a_new_note_8_db_under_its_sample_and_a_clip_at_its_level() {
    let dir = Dir::new("fl-exact-level");
    // The click is 1.0, and a new note has velocity 100 and is centred.
    // FL's level for it is 0.551 / sqrt(2): the DLL has 0.551 for a new
    // channel's volume (FUN_00fe9d20) and its pan law halves the power in
    // the centre (FUN_006a3a00), with nothing to make up for either.
    let level = (new_channel() / SQRT_2) as f32;
    assert!((level - 0.3896).abs() < 1e-4);
    let song = project(&[(1, &four_beats())], &[item(0, 1)]);
    let out = render_exact(&song, &dir);
    assert_impulses(&out.left, &[0, 24_000, 48_000, 72_000]);
    assert_gains(&out, 0, (level, level));
    assert_eq!(out.left, out.right);
    // The other curves play it at the sample's own level.
    assert_eq!(render(&song, &dir).left[0], 1.0);
    // The mixer's faders and pans leave it alone.
    assert_gains(&render_exact(&one_click(), &dir), 0, (level, level));

    // Every frame of a sample comes out at that level.
    dir.ramp();
    let mut song = project(&[(1, &[note(0)])], &[item(0, 1)]);
    song.channels[0].sample_path = Some("ramp.wav".to_owned());
    let out = render_exact(&song, &dir);
    for frame in 0..RAMP {
        assert_gains(&out, frame, (level * ramp(frame), level * ramp(frame)));
    }

    // An audio clip of the same file is at the file's level: FL gives a
    // clip sqrt(2) / 0.551 for its volume (FUN_00ffa0a0).
    let out = render_exact(&clips(&[clip(0, BEAT)]), &dir);
    assert_eq!(out.clips, 1);
    for frame in 0..RAMP {
        assert_gains(&out, frame, (ramp(frame), ramp(frame)));
    }
}

#[test]
fn fl_s_curves_take_a_note_s_velocity_and_pan() {
    let dir = Dir::new("fl-exact-note");
    let mut notes = four_beats();
    notes[0].velocity = 64;
    notes[1].pan = 0;
    notes[2].pan = 96;
    (notes[3].velocity, notes[3].pan) = (128, 128);
    let song = project(&[(1, &notes)], &[item(0, 1)]);
    let out = render_exact(&song, &dir);
    let new = new_channel();
    // (11^0.64 - 1) / 10 of a new note, on both sides.
    let soft = (11f64.powf(0.64) - 1.0) / 10.0 * new;
    assert_gains(&out, 0, circular(0.0, soft));
    // All the way left is the level itself, 3 dB over the centre, and
    // silent on the right.
    assert_gains(&out, 24_000, (new as f32, 0.0));
    assert_eq!(out.right[24_000], 0.0);
    // Half way right: cos and sin of 3 pi/8.
    assert_gains(&out, 48_000, circular(0.5, new));
    // (11^1.28 - 1) / 10 all the way right.
    let loud = (11f64.powf(1.28) - 1.0) / 10.0 * new;
    assert_gains(&out, 72_000, (0.0, loud as f32));
    // The other curves have a line for the velocity and no law for the pan.
    let plain = render(&song, &dir);
    assert_eq!((plain.left[0], plain.left[24_000]), (0.64, 1.0));

    // The channel's pan and the note's add up before the law: a channel
    // half way left brings the third note back to the centre.
    let mut song = song;
    song.channels[0].levels.as_mut().unwrap().pan = 3200;
    let out = render_exact(&song, &dir);
    assert_gains(&out, 48_000, circular(0.0, new));
    assert_gains(&out, 0, circular(-0.5, soft));
}

#[test]
fn fl_s_curves_pan_an_insert_on_the_circle() {
    let dir = Dir::new("fl-exact-insert-pan");
    let panned = |pan: i32| {
        let mut panned = one_click();
        set(&mut panned, 1, PAN, pan);
        render_exact(&panned, &dir)
    };
    // The click is the same on both sides of the insert, so its level
    // follows the law with the centre at unity: 3 dB up at a hard pan.
    let new = new_channel();
    let out = panned(-6400);
    assert_gains(&out, 0, (new as f32, 0.0));
    assert!(out.right.iter().all(|v| *v == 0.0));
    let out = panned(6400);
    assert!(out.left.iter().all(|v| *v == 0.0));
    assert_gains(&out, 0, (0.0, new as f32));
    assert_gains(&panned(3200), 0, circular(0.5, new));
    assert_gains(&panned(-1600), 0, circular(-0.25, new));
    assert_gains(&panned(0), 0, circular(0.0, new));

    // The far input comes out on the near side. A note all the way left is
    // on the left of its insert alone. With the insert all the way right,
    // it is on the right, 3 dB down.
    let mut crossed = one_click();
    crossed.patterns[0].notes[0].pan = 0;
    set(&mut crossed, 1, PAN, 6400);
    let out = render_exact(&crossed, &dir);
    assert_gains(&out, 0, (0.0, (new / SQRT_2) as f32));
    assert!(out.left.iter().all(|v| *v == 0.0));
    // Half way right, the left output is the law's far gain of it, and the
    // right output has the share of the near gain that goes across.
    set(&mut crossed, 1, PAN, 3200);
    let (far, near) = circular(0.5, SQRT_2);
    let across = near * (1.0 - far) / 2.0;
    let want = (new as f32 * far, new as f32 * across);
    assert_gains(&render_exact(&crossed, &dir), 0, want);
    // The other curves feed nothing across: the note is gone.
    set(&mut crossed, 1, PAN, 6400);
    assert_impulses(&render(&crossed, &dir).right, &[]);
}

#[test]
fn fl_s_curves_put_a_channel_s_volume_on_the_fader_curve() {
    let dir = Dir::new("fl-exact-volume");
    let curve = |raw: f64| (11f64.powf(raw / 12_800.0) - 1.0) / 10.0;
    let mut song = project(&[(1, &[note(0)])], &[item(0, 1)]);
    song.channels[0].levels.as_mut().unwrap().volume = 5000;
    let out = render_exact(&song, &dir);
    assert_gains(&out, 0, circular(0.0, curve(5000.0)));
    assert_eq!(render(&song, &dir).left[0], 0.5);
    // All the way up is the curve's unity, 5.2 dB over a new channel, and
    // the centre of the pan takes 3 dB off it.
    song.channels[0].levels.as_mut().unwrap().volume = 12_800;
    let out = render_exact(&song, &dir);
    assert!((db(out.left[0]) + 3.01).abs() < 0.01);
    assert!((db(out.left[0] / circular(0.0, new_channel()).0) - 5.18).abs() < 0.01);

    // The adjustment in the channel's 229 is on the same curve, with 12800
    // for none, and the other curves leave it out.
    song.channels[0].level_adjusts = Some(layout::LevelAdjusts {
        unknown_0: [0; 4],
        unknown_4: 6400i32.to_le_bytes(),
        unknown_8: [0; 12],
    });
    assert_gains(&render_exact(&song, &dir), 0, circular(0.0, curve(6400.0)));
    assert_eq!(try_render(&song, &dir).unwrap().left[0], 1.28);
}

#[test]
fn fl_s_curves_track_a_note_s_key_and_velocity_into_its_pan() {
    let dir = Dir::new("fl-exact-tracking");
    let record = |fields: [i32; 4]| {
        let mut bytes = [0; 16];
        for (at, field) in fields.into_iter().enumerate() {
            bytes[at * 4..at * 4 + 4].copy_from_slice(&field.to_le_bytes());
        }
        layout::Tracking { unknown_0: bytes }
    };
    // An octave over the middle key, and a velocity under the middle one.
    let mut notes = [note(0), note(BEAT), note(2 * BEAT)];
    notes[1].key = 72;
    notes[2].velocity = 64;
    let mut song = project(&[(1, &notes)], &[item(0, 1)]);
    // The velocity's pan knob is half way up and the key's all the way.
    song.channels[0].tracking = vec![record([100, 128, 0, 0]), record([60, 256, 0, 0])];
    let out = render_exact(&song, &dir);
    // The middle key at the middle velocity stays in the centre.
    let new = new_channel();
    assert_gains(&out, 0, circular(0.0, new));
    // 1200 cents of the 6000 that a whole knob pans all the way for.
    assert_gains(&out, 24_000, circular(0.2, new));
    // Half the volume that the note is under the middle one, to the left.
    let soft = (11f64.powf(0.64) - 1.0) / 10.0 * new;
    assert_gains(&out, 48_000, circular((soft - new) / 2.0, soft));
    assert!(out.left[48_000] > out.right[48_000]);

    // The other curves leave tracking out.
    let plain = try_render(&song, &dir).unwrap();
    assert_eq!(plain.left, plain.right);
    assert_eq!((plain.left[24_000], plain.left[48_000]), (1.0, 0.64));
}

/// The level of a sample in dB.
fn db(sample: f32) -> f32 {
    20.0 * sample.abs().log10()
}

#[test]
fn fast_dist_on_an_insert_squashes_what_plays_into_it() {
    let dir = Dir::new("fast-dist");
    let squashed = || {
        let mut project = one_click();
        add_effect(&mut project, 1, 0, "Fruity Fast Dist", &params(&FAST_DIST));
        project
    };
    // Type A at threshold 10 holds everything above -25 dB to a tenth of
    // its range and lifts it 14.4 dB, so the full-scale click leaves at
    // 2.5 - 25 + 14.4 = -8.1 dB.
    let out = render(&squashed(), &dir);
    assert_impulses(&out.left, &[0]);
    assert!((db(out.left[0]) + 8.1).abs() < 0.01, "{}", out.left[0]);
    assert_eq!(out.left[0], out.right[0]);
    assert_eq!(out.hosted_slots, 1);
    assert!(out.unhosted_slots.is_empty());
    // Without the plugin the click is the 1.0 it is in the file.
    let plain = render(&one_click(), &dir);
    assert_eq!(plain.left[0], 1.0);
    assert_eq!((plain.hosted_slots, plain.unhosted_slots.len()), (0, 0));

    // A click below -25 dB gets the lift and nothing else, so it comes
    // out louder than it went in: velocity 5 is 0.05, which is -26 dB.
    let mut quiet = squashed();
    quiet.patterns[0].notes[0].velocity = 5;
    let out = render(&quiet, &dir);
    assert!((db(out.left[0]) - db(0.05) - 14.4).abs() < 0.05);

    // A plugin on another insert is none of this one's business, and it
    // is not counted, because no sound gets to it.
    let mut other = one_click();
    add_effect(&mut other, 2, 0, "Fruity Fast Dist", &params(&FAST_DIST));
    let out = render(&other, &dir);
    assert_eq!((out.left[0], out.hosted_slots), (1.0, 0));

    // The presets FL ships have the name in lower case.
    let mut lower = one_click();
    add_effect(&mut lower, 1, 0, "Fruity fast dist", &params(&FAST_DIST));
    let out = render(&lower, &dir);
    assert!((db(out.left[0]) + 8.1).abs() < 0.01);
    assert_eq!(out.hosted_slots, 1);
}

#[test]
fn a_slot_s_switch_and_mix_level_gate_and_blend_its_plugin() {
    let dir = Dir::new("slot-mix");
    let squashed = || {
        let mut project = one_click();
        add_effect(&mut project, 1, 3, "Fruity Fast Dist", &params(&FAST_DIST));
        project
    };
    let wet = render(&squashed(), &dir).left[0];
    assert!((db(wet) + 8.1).abs() < 0.01);

    // Switched off, the slot passes the click on. It is hosted all the
    // same.
    let mut off = squashed();
    set_slot(&mut off, 1, 3, SLOT_ENABLED, 0);
    let out = render(&off, &dir);
    assert_eq!((out.left[0], out.right[0]), (1.0, 1.0));
    assert_eq!(out.hosted_slots, 1);
    // Another slot's switch does nothing here.
    let mut other = squashed();
    set_slot(&mut other, 1, 2, SLOT_ENABLED, 0);
    assert_eq!(render(&other, &dir).left[0], wet);

    // The mix level all the way up is the plugin alone, all the way down
    // the click alone, and in between both, by the fader's curve.
    let mixed = |level: i32| {
        let mut project = squashed();
        set_slot(&mut project, 1, 3, SLOT_MIX, level);
        project
    };
    assert_eq!(render(&mixed(12_800), &dir).left[0], wet);
    assert_eq!(render(&mixed(0), &dir).left[0], 1.0);
    let share = Approximate.slot_mix(6400);
    assert!((db(share) + 11.2).abs() < 1e-3);
    let out = render(&mixed(6400), &dir);
    assert!((out.left[0] - (wet * share + 1.0 - share)).abs() < 1e-6);
    assert!(out.left[0] > wet && out.left[0] < 1.0);
    // FL's own curve puts half the knob at (11^0.5 - 1) / 10.
    let share = engine::FlExact.slot_mix(6400);
    assert!((share - 0.231_662_5).abs() < 1e-6);
    // Those curves play the click at another level, so the plugin makes
    // something else of it.
    let wet = render_exact(&mixed(12_800), &dir).left[0];
    let dry = render_exact(&mixed(0), &dir).left[0];
    let out = render_exact(&mixed(6400), &dir);
    assert!((out.left[0] - (wet * share + dry * (1.0 - share))).abs() < 1e-6);
}

#[test]
fn an_insert_runs_its_slots_in_slot_order() {
    let dir = Dir::new("slot-order");
    // Fruity Balance with its volume all the way up is +5.6 dB.
    let loud = params(&[0, 320]);
    let boost = (11f32.powf(1.25) - 1.0) / 10.0;
    let clipped = 10f32.powf(-8.1 / 20.0);

    // Balance after Fast Dist lifts the squashed click.
    let mut after = one_click();
    add_effect(&mut after, 1, 5, "Fruity Balance", &loud);
    add_effect(&mut after, 1, 2, "Fruity Fast Dist", &params(&FAST_DIST));
    let out = render(&after, &dir);
    assert!((out.left[0] - clipped * boost).abs() < 1e-4);
    assert_eq!(out.hosted_slots, 2);

    // Balance ahead of it pushes the click past full scale, where Fast
    // Dist holds it to what full scale comes out as.
    let mut ahead = one_click();
    add_effect(&mut ahead, 1, 5, "Fruity Fast Dist", &params(&FAST_DIST));
    add_effect(&mut ahead, 1, 2, "Fruity Balance", &loud);
    let out = render(&ahead, &dir);
    assert!((out.left[0] - clipped).abs() < 1e-4);

    // Balance as FL sets a new one changes no bit.
    let mut unity = one_click();
    add_effect(&mut unity, 1, 0, "Fruity Balance", &params(&[0, 256]));
    let out = render(&unity, &dir);
    assert!(
        out == Rendered {
            hosted_slots: 1,
            ..render(&one_click(), &dir)
        }
    );

    // Hard left, it folds both sides into the left at -3 dB each.
    let mut left = one_click();
    add_effect(&mut left, 1, 0, "Fruity Balance", &params(&[-128, 256]));
    let out = render(&left, &dir);
    assert!((out.left[0] - 2f32.sqrt()).abs() < 1e-6);
    assert!(out.right.iter().all(|v| *v == 0.0));
}

#[test]
fn a_plugin_the_engine_does_not_host_passes_through_and_is_counted() {
    let dir = Dir::new("unhosted");
    let mut project = one_click();
    add_effect(&mut project, 1, 0, "Fruity Reeverb 2", &[1, 2, 3]);
    add_effect(&mut project, 1, 1, "Fruity Fast Dist", &params(&FAST_DIST));
    add_effect(&mut project, 1, 2, "Fruity Reeverb 2", &[]);
    // The master is on the way out, so its slots count too.
    add_effect(&mut project, 0, 0, "Maximus", &[9; 40]);
    // Insert 3 gets no sound, and its slot is not counted.
    add_effect(&mut project, 3, 0, "Fruity Limiter", &[0; 8]);
    let out = render(&project, &dir);
    assert!((db(out.left[0]) + 8.1).abs() < 0.01);
    assert_eq!(out.hosted_slots, 1);
    let unhosted: Vec<_> = out.unhosted_slots.iter().collect();
    assert_eq!(
        unhosted,
        [
            (&"Fruity Reeverb 2".to_owned(), &2),
            (&"Maximus".to_owned(), &1)
        ]
    );

    // A hosted plugin with a state it does not write is left out of the
    // sound and counted with the others: Fast Dist has 20 bytes, and its
    // threshold stops at 10.
    for state in [params(&[128, 10, 0, 128]), params(&[128, 11, 0, 128, 128])] {
        let mut broken = one_click();
        add_effect(&mut broken, 1, 0, "Fruity Fast Dist", &state);
        let out = render(&broken, &dir);
        assert_eq!((out.left[0], out.hosted_slots), (1.0, 0));
        assert_eq!(out.unhosted_slots.get("Fruity Fast Dist"), Some(&1));
    }

    // Without a 213 the plugin has its defaults.
    let mut bare = one_click();
    add_effect(&mut bare, 1, 0, "Fruity Fast Dist", &[]);
    bare.inserts[1].slots[0].plugin_state = None;
    bare.inserts[1].slots[0].order.pop();
    let out = render(&bare, &dir);
    assert!((db(out.left[0]) + 8.1).abs() < 0.01);
    assert_eq!(out.hosted_slots, 1);
}

/// A WAV of 3,000 frames that is loud all the way through, with both signs
/// in it, so two voices that overlap add up sample by sample.
fn noisy_wav(dir: &Dir, channels: u16) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(dir.0.join("click.wav"), spec).unwrap();
    for value in 0..3000 * u32::from(channels) {
        let level = (value * 37 % 101) as f32 / 50.0 - 1.0;
        writer.write_sample(level / 3.0).unwrap();
    }
    writer.finalize().unwrap();
}

/// FNV-1a over the bits of every sample, left side first.
fn fingerprint(left: &[f32], right: &[f32]) -> u64 {
    let bits = left.iter().chain(right).map(|v| v.to_bits());
    bits.flat_map(u32::to_le_bytes)
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// Projects whose voices overlap and whose gains are not all 1. Every gain
/// in them comes out of plain arithmetic, so the bits of a render do not
/// depend on the platform's `pow`.
fn busy_projects() -> Vec<(&'static str, Project, f32)> {
    // Six notes 100 samples apart at two octaves, then a held one.
    let mut notes: Vec<Note> = (0..6).map(note).collect();
    for (i, note) in notes.iter_mut().enumerate() {
        note.key = [60, 72, 48][i % 3];
        note.velocity = 40 + 20 * i as u8;
        note.pan = 16 * i as u8;
    }
    let mut held = note(BEAT);
    held.length = 5;
    notes.push(held);
    let mut plain = project(&[(1, &notes)], &[item(0, 1), item(BAR, 1)]);
    plain.tempo_milli = Some(300_000);
    plain.channels[0].levels.as_mut().unwrap().volume = 7000;
    plain.channels[0].levels.as_mut().unwrap().pan = 1600;

    // The same notes on two channels, into inserts 1 and 2. Insert 1 sends
    // to 2 and to the master, insert 3 gets a send and is switched off.
    let mut both: Vec<Note> = notes.clone();
    both.extend(notes.iter().map(|note| Note {
        channel: 1,
        position: note.position + 2,
        ..*note
    }));
    let mut mixed = with_mixer(project(&[(1, &both)], &[item(0, 1)]));
    mixed.tempo_milli = Some(300_000);
    add_channel(&mut mixed, 2);
    route(&mut mixed, 1, &[0, 2, 3]);
    set(&mut mixed, 1, PAN, 3200);
    set(&mut mixed, 2, PAN, -1600);
    mixed.inserts[3].flags.as_mut().unwrap().flags = 0b0100;

    // The master's fader at 0 turns every negative sample into -0.0.
    let mut silenced = mixed.clone();
    set(&mut silenced, 0, VOLUME, 0);

    vec![
        ("plain", plain.clone(), 60.0),
        ("cut short", plain, 0.05),
        ("mixed", mixed, 60.0),
        ("silenced", silenced, 60.0),
    ]
}

/// The length, the voice count and the fingerprint of every
/// [`busy_projects`] render, first with a mono sample and then with a
/// stereo one. The renderer that mixed a whole song in one pass, before
/// there was an [`engine::Engine`], gave these.
const ONE_PASS: [(&str, usize, usize, u64); 8] = [
    ("plain", 76_800, 14, 0xb441_76a0_9ba6_1555),
    ("cut short", 2400, 6, 0xf776_fdc6_5043_e171),
    ("mixed", 38_400, 14, 0xceb4_a84b_2697_8055),
    ("silenced", 38_400, 14, 0x3ae0_b2be_06da_4025),
    ("plain", 76_800, 14, 0x1da7_9b2c_fc22_1401),
    ("cut short", 2400, 6, 0x1719_8c88_51a4_0c7b),
    ("mixed", 38_400, 14, 0x0f9e_e001_49a5_a060),
    ("silenced", 38_400, 14, 0x0ec6_e3bc_074a_11a5),
];

#[test]
fn a_render_has_the_bits_the_one_pass_renderer_gave_it() {
    let dir = Dir::new("bits");
    let mut got = Vec::new();
    for channels in [1, 2] {
        noisy_wav(&dir, channels);
        for (name, project, max_seconds) in busy_projects() {
            let opts = RenderOptions {
                sample_rate: RATE,
                sample_root: dir.0.clone(),
                max_seconds,
                plugin_dir: None,
            };
            let out = engine::render(&project, &opts).unwrap();
            assert_eq!(out.left.len(), out.right.len());
            let bits = fingerprint(&out.left, &out.right);
            got.push((name, out.left.len(), out.voices, bits));
        }
    }
    assert_eq!(got, ONE_PASS);
}

/// An output with no device behind it. It asks an engine for one block
/// after another, all of one size, and keeps what it gets.
struct NullSink {
    block: usize,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl NullSink {
    fn new(block: usize) -> Self {
        NullSink {
            block,
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    /// Pull until the sink holds `frames` samples a side. The last block is
    /// as short as it has to be.
    fn pull(&mut self, engine: &mut Engine, frames: usize) {
        let (mut left, mut right) = (vec![0.0; self.block], vec![0.0; self.block]);
        while self.left.len() < frames {
            let count = self.block.min(frames - self.left.len());
            engine.process(&mut left[..count], &mut right[..count]);
            self.left.extend_from_slice(&left[..count]);
            self.right.extend_from_slice(&right[..count]);
        }
    }
}

#[test]
fn the_clicks_stay_on_the_beats_whatever_the_block_size() {
    let dir = Dir::new("null-sink");
    let song = project(
        &[(1, &four_beats()), (2, &[note(0), note(BEAT)])],
        &[item(0, 1), item(2 * BAR, 2)],
    );
    let samples = engine::prepare(&song, &dir.0).unwrap();
    let offline = render(&song, &dir);
    // A beat is 24,000 samples: 375 blocks of 64, 24 blocks of 1,000, and
    // 93 blocks of 256 with 192 samples over.
    for block in [64, 256, 1000] {
        let mut engine = Engine::new(&song, &samples, RATE).unwrap();
        engine.play();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, 3 * 96_000);
        assert_impulses(&sink.left, &[0, 24_000, 48_000, 72_000, 192_000, 216_000]);
        assert_eq!(sink.left, sink.right);
        assert!(sink.left == offline.left, "blocks of {block}");
        assert_eq!(engine.position(), 3 * BAR as u64);
    }
}

#[test]
fn blocks_of_any_size_add_up_to_the_same_bits() {
    let dir = Dir::new("null-sink-bits");
    noisy_wav(&dir, 2);
    for (name, project, max_seconds) in busy_projects() {
        let opts = RenderOptions {
            sample_rate: RATE,
            sample_root: dir.0.clone(),
            max_seconds,
            plugin_dir: None,
        };
        let offline = engine::render(&project, &opts).unwrap();
        let want = fingerprint(&offline.left, &offline.right);
        let samples = engine::prepare(&project, &dir.0).unwrap();
        for block in [1, 64, 256, 1000] {
            let mut engine = Engine::new(&project, &samples, RATE).unwrap();
            engine.play();
            let mut sink = NullSink::new(block);
            sink.pull(&mut engine, offline.left.len());
            let got = fingerprint(&sink.left, &sink.right);
            assert_eq!(got, want, "{name} in blocks of {block}");
        }
    }
}

#[test]
fn notes_and_clips_come_out_the_same_in_blocks_of_any_size() {
    let dir = Dir::new("null-sink-clips");
    dir.ramp();
    noisy_wav(&dir, 2);
    let mut reversed = clip(1, 3);
    (reversed.gain, reversed.pan) = (0.75, -0.5);
    reversed.clip_flags = PlaylistItem::REVERSED;
    let mut trimmed = clip(2, BEAT);
    trimmed.pitch = -7.0;
    trimmed.set_trim(Some(Trim::Clip {
        start: 1.25,
        end: 20.0,
    }));
    let notes: Vec<Note> = (0..6).map(note).collect();
    let song = with_clips(project_26(
        &[(1, &notes)],
        &[item(0, 1), clip(0, BEAT), reversed, trimmed],
    ));
    let offline = render(&song, &dir);
    assert_eq!((offline.voices, offline.clips), (6, 3));
    // `prepare` decodes the audio clip channel's sample too.
    let samples = engine::prepare(&song, &dir.0).unwrap();
    for block in [1, 64, 256, 1000] {
        let mut engine = Engine::new(&song, &samples, RATE).unwrap();
        engine.play();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, offline.left.len());
        assert!(sink.left == offline.left, "left in blocks of {block}");
        assert!(sink.right == offline.right, "right in blocks of {block}");
    }
}

/// Play the first 1,000 samples of the project in blocks of 256 and read
/// the meters: the peak of every insert that has a bus, by its number.
fn meters(project: &Project, dir: &Dir) -> Vec<(u16, (f32, f32))> {
    let samples = engine::prepare(project, &dir.0).unwrap();
    let mut engine = Engine::new(project, &samples, RATE).unwrap();
    engine.play();
    NullSink::new(256).pull(&mut engine, 1000);
    let mut peaks = Vec::new();
    engine.insert_peaks(|insert, peak| peaks.push((insert, peak)));
    peaks.sort_by_key(|(insert, _)| *insert);
    // A meter starts again from silence once it is read.
    engine.insert_peaks(|_, peak| assert_eq!(peak, (0.0, 0.0)));
    peaks
}

#[test]
fn the_meters_report_the_inserts_the_click_goes_through() {
    let dir = Dir::new("meters");
    // The click plays into insert 1, which sends to the master. Nothing
    // reaches inserts 2 and 3, so they have no meter to read.
    let full = (1.0, 1.0);
    assert_eq!(meters(&one_click(), &dir), [(0, full), (1, full)]);

    // An insert's meter is behind its volume and its pan.
    let mut panned = one_click();
    set(&mut panned, 1, PAN, -6400);
    set(&mut panned, 0, VOLUME, 0);
    let left = (1.0, 0.0);
    assert_eq!(meters(&panned, &dir), [(0, (0.0, 0.0)), (1, left)]);
    // The main volume comes after the master's meter.
    let mut quiet = one_click();
    record(&mut quiet, MAIN_VOLUME, 0);
    assert_eq!(meters(&quiet, &dir), [(0, full), (1, full)]);

    // Through insert 2 the click is on three meters, and on one alone
    // when insert 1 sends nowhere.
    let mut routed = one_click();
    route(&mut routed, 1, &[2]);
    assert_eq!(meters(&routed, &dir), [(0, full), (1, full), (2, full)]);
    let mut nowhere = one_click();
    route(&mut nowhere, 1, &[]);
    assert_eq!(meters(&nowhere, &dir), [(1, full)]);
    // An insert that is switched off puts out nothing.
    let mut off = one_click();
    off.inserts[1].flags.as_mut().unwrap().flags = 0b0100;
    assert_eq!(meters(&off, &dir), [(1, (0.0, 0.0))]);
}

#[test]
fn prepare_decodes_every_sampler_channel_and_counts_what_it_cannot() {
    let dir = Dir::new("prepare");
    std::fs::write(dir.0.join("click.wv"), hybrid_wavpack()).unwrap();
    let mut song = project(&[(1, &four_beats())], &[item(0, 1)]);
    for (index, path) in [(1, "click.wv"), (2, "lost.wav"), (3, "")] {
        add_channel(&mut song, 0);
        let channel = song.channels.last_mut().unwrap();
        channel.index = index;
        channel.sample_path = Some(path.to_owned());
    }
    let samples = engine::prepare(&song, &dir.0).unwrap();
    assert_eq!(samples.unresolved(), ["lost.wav"]);
    assert_eq!(samples.unsupported(), ["click.wv"]);

    // An engine without the channel's sample plays the channel as silence.
    let mut engine = Engine::new(&song, &engine::PreparedSamples::default(), RATE).unwrap();
    engine.play();
    let mut sink = NullSink::new(256);
    sink.pull(&mut engine, 96_000);
    assert_impulses(&sink.left, &[]);
}

#[test]
fn prepare_leaves_a_channel_silent_when_its_sample_does_not_decode() {
    let dir = Dir::new("prepare-corrupt");
    std::fs::write(dir.0.join("bad.wav"), b"RIFF\x04\0\0\0WAVE and no chunks").unwrap();
    // Channel 0 plays the click, channel 5 the corrupt file, and channel 6
    // the click again.
    let on = |channel: u16, position: i32| Note {
        channel,
        ..note(position)
    };
    let notes = [on(0, 0), on(5, BEAT), on(6, 2 * BEAT)];
    let mut song = project(&[(1, &notes)], &[item(0, 1)]);
    for (index, path) in [(5, "bad.wav"), (6, "click.wav")] {
        add_channel(&mut song, 0);
        let channel = song.channels.last_mut().unwrap();
        channel.index = index;
        channel.sample_path = Some(path.to_owned());
    }
    let samples = engine::prepare(&song, &dir.0).unwrap();
    match &samples.failed[..] {
        [(5, Error::Decode { path, .. })] => assert_eq!(*path, dir.0.join("bad.wav")),
        other => panic!("{other:?}"),
    }
    assert!(samples.unresolved().is_empty() && samples.unsupported().is_empty());

    let mut engine = Engine::new(&song, &samples, RATE).unwrap();
    engine.play();
    let mut sink = NullSink::new(256);
    sink.pull(&mut engine, 96_000);
    assert_impulses(&sink.left, &[0, 48_000]);
    // A render still turns the file down.
    assert!(matches!(try_render(&song, &dir), Err(Error::Decode { .. })));
}

#[test]
fn a_pattern_on_its_own_plays_round_and_round() {
    let dir = Dir::new("pattern-loop");
    let two_bars = [note(0), note(BAR + BEAT)];
    let song = project(
        &[
            (1, &four_beats()),
            (2, &[note(0), note(BEAT)]),
            (3, &two_bars),
        ],
        &[item(2 * BAR, 2)],
    );
    let samples = engine::prepare(&song, &dir.0).unwrap();
    let beats = |beats: &[usize]| -> Vec<usize> { beats.iter().map(|b| b * 24_000).collect() };
    for block in [64, 1000] {
        // Pattern 1 is a bar, and the playlist's two silent bars are not
        // played.
        let mut engine = Engine::new(&song, &samples, RATE).unwrap();
        engine.set_song_mode(false).unwrap();
        engine.play();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, 3 * 96_000);
        assert_impulses(&sink.left, &beats(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]));
        assert_eq!(sink.left[..96_000], sink.left[96_000..2 * 96_000]);
        assert!(engine.is_playing());

        // The clock keeps its tick, a bar in, so pattern 3 goes on from
        // its second bar and then comes round.
        engine.set_pattern(3).unwrap();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, 4 * 96_000);
        assert_impulses(&sink.left, &beats(&[1, 4, 9, 12]));
        assert_eq!(engine.position(), BAR as u64);

        // A tick past the end of the new pattern starts it over.
        engine.seek((BAR + 2 * BEAT) as u64);
        engine.set_pattern(1).unwrap();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, 96_000);
        assert_impulses(&sink.left, &beats(&[0, 1, 2, 3]));

        // Back in song mode the clock keeps its tick, and the song stops
        // where its item ends.
        engine.seek(2 * BAR as u64);
        engine.set_song_mode(true).unwrap();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, 2 * 96_000);
        assert_impulses(&sink.left, &beats(&[0, 1]));
        assert!(!engine.is_playing());
    }
    // A pattern the project does not have is a bar of silence.
    let mut engine = Engine::new(&song, &samples, RATE).unwrap();
    engine.set_pattern(9).unwrap();
    engine.set_song_mode(false).unwrap();
    engine.play();
    let mut sink = NullSink::new(256);
    sink.pull(&mut engine, 96_000 + 256);
    assert_impulses(&sink.left, &[]);
    assert_eq!((engine.is_playing(), engine.position()), (true, 1));
}

#[test]
fn the_song_stops_at_its_end_and_starts_over_when_it_loops() {
    let dir = Dir::new("song-end");
    let song = project(&[(1, &four_beats())], &[item(0, 1), item(BAR, 1)]);
    assert_eq!(song.song_length_ticks(), 2 * BAR as u64);
    assert_eq!(engine::song_length(&song, RATE), 2 * 96_000);
    let samples = engine::prepare(&song, &dir.0).unwrap();
    let mut engine = Engine::new(&song, &samples, RATE).unwrap();
    engine.play();
    let mut sink = NullSink::new(256);
    sink.pull(&mut engine, 2 * 96_000 + 256);
    let beats: Vec<usize> = (0..8).map(|beat| beat * 24_000).collect();
    assert_impulses(&sink.left, &beats);
    assert_eq!((engine.is_playing(), engine.position()), (false, 0));
    // Play starts the song again.
    engine.play();
    let mut sink = NullSink::new(256);
    sink.pull(&mut engine, 96_000);
    assert_impulses(&sink.left, &beats[..4]);

    let mut engine = Engine::new(&song, &samples, RATE).unwrap();
    engine.set_loop(true);
    engine.play();
    let mut sink = NullSink::new(1000);
    sink.pull(&mut engine, 5 * 96_000);
    let beats: Vec<usize> = (0..20).map(|beat| beat * 24_000).collect();
    assert_impulses(&sink.left, &beats);
    assert_eq!((engine.is_playing(), engine.position()), (true, BAR as u64));
}

#[test]
fn the_song_s_length_follows_the_tempo_a_clip_sets() {
    // 60 BPM all through the bar: twice the samples.
    let slow = [Point::new(0.0, 0.0), Point::new(4.0, 0.0)];
    let mut halved = project(&[(1, &four_beats())], &[item(0, 1)]);
    assert_eq!(engine::song_length(&halved, RATE), 96_000);
    assert_eq!(engine::song_length(&halved, 44_100), 88_200);
    let clip = add_automation(&mut halved, &slow, TEMPO_RANGE);
    bind(&mut halved, clip, TEMPO);
    place(&mut halved, clip, 0, BAR);
    assert_eq!(halved.song_length_ticks(), BAR as u64);
    assert_eq!(engine::song_length(&halved, RATE), 2 * 96_000);

    // An empty playlist plays pattern 1 once, a bar here.
    let bare = project(&[(1, &[note(0)])], &[]);
    assert_eq!(bare.song_length_ticks(), 0);
    assert_eq!(engine::song_length(&bare, RATE), 96_000);
    let mut timeless = bare.clone();
    timeless.tempo_milli = None;
    assert_eq!(engine::song_length(&timeless, RATE), 0);
}

#[test]
fn a_render_reports_its_progress_every_half_second_or_sooner() {
    let dir = Dir::new("progress");
    let song = project(&[(1, &four_beats())], &[item(0, 1)]);
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: dir.0.clone(),
        max_seconds: 60.0,
        plugin_dir: None,
    };
    let mut seen = Vec::new();
    let mut progress = |done: f32| seen.push(done);
    let out =
        engine::render_with_progress(&song, &opts, &engine::Approximate, &mut progress).unwrap();
    assert!(out == try_render(&song, &dir).unwrap());
    // Two seconds, in steps of 46 blocks of 512 samples.
    let want: Vec<f32> = [23_552, 47_104, 70_656, 94_208, 96_000]
        .iter()
        .map(|done| *done as f32 / 96_000.0)
        .collect();
    assert_eq!(seen, want);
    assert_eq!(seen.last(), Some(&1.0));

    // A render with nothing in it says it is done.
    let mut seen = Vec::new();
    let mut progress = |done: f32| seen.push(done);
    let empty = project(&[], &[]);
    let out =
        engine::render_with_progress(&empty, &opts, &engine::Approximate, &mut progress).unwrap();
    assert_eq!((out.left.len(), seen), (0, vec![1.0]));
}

#[test]
fn a_transport_plays_the_clicks_on_the_default_device() {
    let dir = Dir::new("device");
    let song = Arc::new(project(&[(1, &four_beats())], &[item(0, 1)]));
    let samples = engine::prepare(&song, &dir.0).unwrap();
    let mut transport = match Transport::open(Arc::clone(&song), samples, Some(RATE)) {
        Ok(transport) => transport,
        Err(e @ (Error::NoDevice | Error::Stream { .. })) => {
            eprintln!("no audio stream, device test skipped: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    transport.play().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let position = transport.position();
    let (left, right) = transport.peak();
    transport.stop().unwrap();
    eprintln!(
        "device test ran at {} Hz: tick {position} after a second, peaks {left} and {right}, \
         {} stream errors",
        transport.sample_rate(),
        transport.faults(),
    );
    assert!(position > 0, "the clock stood still");
    assert!(left > 0.0 && right > 0.0, "peaks {left} and {right}");
}

/// A point at the end of a hold: the curve stays at the point before for
/// `x` beats and then jumps to `y`.
fn hold(x: f64, y: f64) -> Point {
    Point {
        mode: 2,
        ..Point::new(x, y)
    }
}

/// Add an automation channel that draws `points` and give its index. Its
/// 219 says the clip covers `range` of its target, in 12800ths.
fn add_automation(project: &mut Project, points: &[Point], range: (i32, i32)) -> u16 {
    let index = project.channels.len() as u16;
    let pos = 200 + 10 * usize::from(index);
    let ids = [Id::ChannelNew, Id::ChannelType, Id::Levels, Id::Automation];
    let fields = ids.into_iter().enumerate().map(|(n, id)| Placed {
        pos: pos + n,
        item: Item::Field(id),
    });
    project.channels.push(Channel {
        index,
        kind: Some(ChannelKind::Automation),
        levels: Some(Levels {
            pan: range.0 as u32,
            volume: range.1 as u32,
            unknown_8: [0; 4],
            mod_x: 0,
            mod_y: 0,
            unknown_20: [0; 4],
            len: 24,
        }),
        automation: Some(AutomationData::new(points.to_vec())),
        order: fields.collect(),
        ..Channel::default()
    });
    project.header.channel_count += 1;
    index
}

/// Add the 227 that binds an automation channel to the control with this
/// REC ID.
fn bind(project: &mut Project, channel: u16, target: u32) {
    let pos = 2000 + project.controller_links.len();
    project.controller_links.push(ControllerLink {
        kind: LinkKind::Remote,
        link: Some(layout::ControllerLink {
            source: u32::from(channel) << 16,
            unknown_4: [0; 4],
            target,
            flags: 8,
            unknown_16: 469i32.to_le_bytes(),
        }),
        formula: None,
        order: vec![Placed {
            pos,
            item: Item::Field(Id::ControllerRemote),
        }],
    });
}

/// Put `ticks` of an automation channel on the playlist.
fn place(project: &mut Project, channel: u16, position: i32, ticks: i32) {
    let size = project.arrangements[0].items.first().map_or(32, |i| i.len);
    let item = PlaylistItem::new(ItemSource::Channel(channel), position, ticks);
    let items = &mut project.arrangements[0].items;
    items.push(item.shortened(size).unwrap());
}

/// The ID FL 20.8 gives a control of an insert.
fn insert_control(insert: u32, control: u32) -> u32 {
    0x2000_0000 + (insert << 22) + control
}

/// The tempo's REC ID.
const TEMPO: u32 = 0x4000_0005;
/// The part of the tempo's range FL gives a new tempo clip: 60 to 180 BPM.
const TEMPO_RANGE: (i32, i32) = (1250, 4250);
/// The whole of a target's range.
const FULL: (i32, i32) = (0, 12_800);

/// The mixer fixture with a note that holds `tone.wav` for `ticks`.
fn tone(ticks: i32) -> Project {
    let mut held = note(0);
    held.length = ticks;
    let mut item = item(0, 1);
    item.length = ticks;
    let mut project = with_mixer(project(&[(1, &[held])], &[item]));
    project.channels[0].sample_path = Some("tone.wav".to_owned());
    project
}

fn render_exact(project: &Project, dir: &Dir) -> Rendered {
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: dir.0.clone(),
        max_seconds: 60.0,
        plugin_dir: None,
    };
    engine::render_with(project, &opts, &engine::FlExact).unwrap()
}

#[test]
fn a_clip_on_an_insert_s_volume_fades_a_tone_out_over_its_bar() {
    let dir = Dir::new("automation-fade");
    dir.tone(3 * 96_000);
    // The tone lasts a bar and a half. The clip takes insert 1 from 12800,
    // which is 0.8 of a fader that ends at 16000, to 0 over the first bar.
    let mut faded = tone(BAR + 2 * BEAT);
    let fade = add_automation(
        &mut faded,
        &[Point::new(0.0, 0.8), Point::new(4.0, 0.0)],
        FULL,
    );
    bind(&mut faded, fade, insert_control(1, VOLUME));
    place(&mut faded, fade, 0, BAR);

    let out = render(&faded, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (1, 0));
    assert_eq!(out.left.len(), 144_000);
    assert_eq!(out.left, out.right);
    assert_eq!(out.left[0], 1.0);
    assert!(out.left[..96_000].windows(2).all(|pair| pair[1] <= pair[0]));
    // The fader moves once per 512 samples, so the level is a staircase.
    assert_eq!(out.left[511], 1.0);
    assert!(out.left[512] < 1.0 && out.left[512] == out.left[1023]);
    assert!(
        (out.left[48_000] - 0.5).abs() > 0.1,
        "a fader is no straight line"
    );
    // After its bar the clip has left the fader at 0, and the tone goes
    // on unheard.
    assert!(out.left[96_256..].iter().all(|v| *v == 0.0));

    // On FL's own fader curve the tone is gone by the end of the bar. The
    // approximate curve is a straight line in dB that stops at -22 dB.
    let exact = render_exact(&faded, &dir);
    assert_gains(&exact, 0, circular(0.0, new_channel()));
    assert!(exact.left[95_999] < 0.01 * exact.left[0]);
    assert!(exact.left[95_999] > 0.0);
    assert!(
        exact.left[..96_000]
            .windows(2)
            .all(|pair| pair[1] <= pair[0])
    );

    // Without the clip the tone holds its level.
    let plain = render(&tone(BAR + 2 * BEAT), &dir);
    assert!(plain.left.iter().all(|v| *v == 1.0));
    assert_eq!((plain.automation_clips, plain.unsupported_targets), (0, 0));
}

#[test]
fn a_control_is_the_project_s_until_a_clip_starts_and_stays_where_the_clip_ends() {
    let dir = Dir::new("automation-hold");
    dir.tone(3 * 96_000);
    // Insert 1 is at 9600 in the project. The clip plays over beats 1 and
    // 2 and takes it from 0 to 12800.
    let mut moved = tone(BAR);
    set(&mut moved, 1, VOLUME, 9600);
    let rise = add_automation(
        &mut moved,
        &[Point::new(0.0, 0.0), Point::new(2.0, 0.8)],
        FULL,
    );
    bind(&mut moved, rise, insert_control(1, VOLUME));
    place(&mut moved, rise, BEAT, 2 * BEAT);
    let out = render(&moved, &dir);
    let before = out.left[0];
    assert!((20.0 * before.log10() + 5.6).abs() < 1e-3);
    assert!(out.left[..24_000].iter().all(|v| *v == before));
    // The first read after the clip starts is on the next 512th sample,
    // where the clip has left 0 behind. Just above 0 the approximate fader
    // is at its bottom, -22.4 dB.
    assert_eq!(out.left[24_063], before);
    assert!(out.left[24_064] < 0.08 && out.left[24_064] > 0.07);
    assert!(
        out.left[24_064..72_000]
            .windows(2)
            .all(|pair| pair[1] >= pair[0])
    );
    assert!(out.left[72_192..].iter().all(|v| *v == 1.0));

    // The second half of the same clip: an item that starts a beat in.
    let mut half = tone(BAR);
    let rise = add_automation(
        &mut half,
        &[Point::new(0.0, 0.0), Point::new(2.0, 0.8)],
        FULL,
    );
    bind(&mut half, rise, insert_control(1, VOLUME));
    place(&mut half, rise, 0, BEAT);
    half.arrangements[0].items[1].set_trim(Some(Trim::Clip {
        start: 1.0,
        end: 2.0,
    }));
    let out = render(&half, &dir);
    // 6400 on the approximate fader is -11.2 dB.
    assert!((20.0 * out.left[0].log10() + 11.2).abs() < 1e-3);
    assert!(out.left[24_064..].iter().all(|v| *v == 1.0));

    // A muted item plays nothing, and neither does a clip whose channel is
    // switched off.
    let mut muted = moved.clone();
    muted.arrangements[0].items[1].set_muted(true);
    let out = render(&muted, &dir);
    assert!(out.left.iter().all(|v| *v == before));
    assert_eq!((out.automation_clips, out.unsupported_targets), (0, 0));
    let mut off = moved.clone();
    off.channels[usize::from(rise)].enabled = Some(false);
    let out = try_render(&off, &dir).unwrap();
    assert!(out.left.iter().all(|v| *v == before));
    assert_eq!((out.automation_clips, out.unsupported_targets), (0, 0));
}

#[test]
fn a_clip_on_the_tempo_doubles_the_beat_spacing() {
    let dir = Dir::new("automation-tempo");
    // The bottom of the clip's range is 60 BPM, half the project's 120.
    let slow = [Point::new(0.0, 0.0), Point::new(4.0, 0.0)];
    let mut halved = project(&[(1, &four_beats())], &[item(0, 1)]);
    let clip = add_automation(&mut halved, &slow, TEMPO_RANGE);
    bind(&mut halved, clip, TEMPO);
    place(&mut halved, clip, 0, BAR);
    let out = render(&halved, &dir);
    assert_impulses(&out.left, &[0, 48_000, 96_000, 144_000]);
    assert_eq!(out.left.len(), 2 * 96_000);
    assert_eq!((out.automation_clips, out.unsupported_targets), (1, 0));

    // The top of the range is 180 BPM: 16,000 samples a beat.
    let fast = [Point::new(0.0, 1.0), Point::new(4.0, 1.0)];
    let mut faster = project(&[(1, &four_beats())], &[item(0, 1)]);
    let clip = add_automation(&mut faster, &fast, TEMPO_RANGE);
    bind(&mut faster, clip, TEMPO);
    place(&mut faster, clip, 0, BAR);
    assert_impulses(&render(&faster, &dir).left, &[0, 16_000, 32_000, 48_000]);

    // A clip that starts on beat 2 leaves the first two beats alone. The
    // tempo is read every 512 samples, so it turns at sample 48,128, 128
    // samples after the beat, and beat 3 comes that much early.
    let mut late = project(&[(1, &four_beats())], &[item(0, 1)]);
    let clip = add_automation(&mut late, &slow, TEMPO_RANGE);
    bind(&mut late, clip, TEMPO);
    place(&mut late, clip, 2 * BEAT, 2 * BEAT);
    let out = render(&late, &dir);
    assert_impulses(&out.left, &[0, 24_000, 48_000, 96_000 - 128]);
    assert_eq!(out.left.len(), 144_000 - 128);

    // The tempo stays where the clip left it: a second bar after a clip
    // of two beats is at 60 BPM all through.
    let mut kept = project(&[(1, &four_beats())], &[item(0, 1), item(BAR, 1)]);
    let clip = add_automation(&mut kept, &slow, TEMPO_RANGE);
    bind(&mut kept, clip, TEMPO);
    place(&mut kept, clip, 0, 2 * BEAT);
    let out = render(&kept, &dir);
    let beats: Vec<usize> = (0..8).map(|beat| beat * 48_000).collect();
    assert_impulses(&out.left, &beats);
}

#[test]
fn a_tempo_ramp_brings_the_beats_closer_together() {
    let dir = Dir::new("automation-tempo-ramp");
    // 60 to 180 BPM over the bar. With the tempo a straight line in
    // beats, beat n comes after 60 / 30 * ln(1 + n / 2) seconds.
    let ramp = [Point::new(0.0, 0.0), Point::new(4.0, 1.0)];
    let mut song = project(&[(1, &four_beats())], &[item(0, 1)]);
    let clip = add_automation(&mut song, &ramp, TEMPO_RANGE);
    bind(&mut song, clip, TEMPO);
    place(&mut song, clip, 0, BAR);
    let out = render(&song, &dir);
    let got = impulses(&out.left);
    assert_eq!(got.len(), 4);
    for (beat, sample) in got.iter().enumerate() {
        let seconds = 2.0 * (1.0 + beat as f64 / 2.0).ln();
        let want = seconds * f64::from(RATE);
        // The tempo is read at the start of every 512 samples, so while
        // it climbs it is a few milliseconds behind, and the beats come
        // late by a little more each.
        let late = *sample as f64 - want;
        assert!(
            (0.0..300.0).contains(&late),
            "beat {beat} at {sample}, {late} late"
        );
    }
    assert!(got[1] - got[0] > got[2] - got[1] && got[2] - got[1] > got[3] - got[2]);
}

#[test]
fn a_clip_bound_to_nothing_the_engine_plays_is_counted() {
    let dir = Dir::new("automation-unbound");
    let line = [Point::new(0.0, 0.0), Point::new(4.0, 1.0)];
    let base = || with_mixer(project(&[(1, &four_beats())], &[item(0, 1)]));
    let plain = render(&base(), &dir);

    // No link at all.
    let mut unbound = base();
    let clip = add_automation(&mut unbound, &line, FULL);
    place(&mut unbound, clip, 0, BAR);
    let out = render(&unbound, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (0, 1));
    assert!(out.left == plain.left && out.right == plain.right);
    // Not on the playlist, it is not played and not counted.
    unbound.arrangements[0].items.pop();
    let out = render(&unbound, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (0, 0));

    // Links to a plugin parameter of channel 0, to insert 1's stereo
    // separation and to the main volume: three controls the engine does
    // not play. The mix level of a slot of insert 1 that holds no plugin
    // changes nothing, and is not counted.
    let mut bound = base();
    let clip = add_automation(&mut bound, &line, FULL);
    for target in [
        0x8003,
        insert_control(1, 0x1F01) + (2 << 16),
        insert_control(1, 0x1FC2),
        MAIN_VOLUME,
    ] {
        bind(&mut bound, clip, target);
    }
    place(&mut bound, clip, 0, BAR);
    place(&mut bound, clip, BAR, BAR);
    let out = render(&bound, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (0, 3));
    assert!(out.left[..96_000] == plain.left[..]);
    // One link the engine plays makes it a clip that plays, and the other
    // three stay counted.
    bind(&mut bound, clip, insert_control(1, VOLUME));
    let out = render(&bound, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (1, 3));
    assert_eq!(out.left[0], 0.0);

    // The volume of a channel the engine does not play is one more.
    let mut plugin = base();
    add_channel(&mut plugin, 1);
    plugin.channels[1].kind = Some(ChannelKind::Plugin);
    let clip = add_automation(&mut plugin, &line, FULL);
    bind(&mut plugin, clip, 1 << 16);
    place(&mut plugin, clip, 0, BAR);
    let out = render(&plugin, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (0, 1));

    // A clip that starts past the end of the render is not counted.
    let mut late = base();
    let clip = add_automation(&mut late, &line, FULL);
    place(&mut late, clip, 4 * BAR, BAR);
    let opts = RenderOptions {
        sample_rate: RATE,
        sample_root: dir.0.clone(),
        max_seconds: 4.0,
        plugin_dir: None,
    };
    let out = engine::render(&late, &opts).unwrap();
    assert_eq!((out.automation_clips, out.unsupported_targets), (0, 0));
    assert_eq!(try_render(&late, &dir).unwrap().unsupported_targets, 1);
}

#[test]
fn a_clip_on_a_channel_s_volume_and_pan_moves_its_notes() {
    let dir = Dir::new("automation-channel");
    // A new channel is at 10000 of 12800. The clip steps on the beats:
    // 10000, 5000, 0 and 10000 again.
    let steps = [
        Point::new(0.0, 0.781_25),
        hold(1.0, 0.390_625),
        hold(1.0, 0.0),
        hold(1.0, 0.781_25),
        Point::new(1.0, 0.781_25),
    ];
    let mut stepped = project(&[(1, &four_beats())], &[item(0, 1)]);
    let clip = add_automation(&mut stepped, &steps, FULL);
    bind(&mut stepped, clip, 0);
    place(&mut stepped, clip, 0, BAR);
    let out = render(&stepped, &dir);
    // A step on a beat is there for the note on that beat.
    assert_impulses(&out.left, &[0, 24_000, 72_000]);
    assert_eq!(
        (out.left[0], out.left[24_000], out.left[72_000]),
        (1.0, 0.5, 1.0)
    );
    assert_eq!(out.left, out.right);
    assert_eq!((out.automation_clips, out.unsupported_targets), (1, 0));

    // Pan: hard left, centre, hard right, and centre.
    let sides = [
        Point::new(0.0, 0.0),
        hold(1.0, 0.5),
        hold(1.0, 1.0),
        hold(1.0, 0.5),
        Point::new(1.0, 0.5),
    ];
    let mut panned = project(&[(1, &four_beats())], &[item(0, 1)]);
    let clip = add_automation(&mut panned, &sides, FULL);
    bind(&mut panned, clip, 1);
    place(&mut panned, clip, 0, BAR);
    let out = render(&panned, &dir);
    assert_impulses(&out.left, &[0, 24_000, 72_000]);
    assert_impulses(&out.right, &[24_000, 48_000, 72_000]);

    // A voice that is sounding follows its channel: the tone drops to half
    // on beat 1 and is gone from beat 2 to beat 3.
    dir.tone(96_000);
    let mut held = note(0);
    held.length = BAR;
    let mut sung = project(&[(1, &[held])], &[item(0, 1)]);
    sung.channels[0].sample_path = Some("tone.wav".to_owned());
    let clip = add_automation(&mut sung, &steps, FULL);
    bind(&mut sung, clip, 0);
    place(&mut sung, clip, 0, BAR);
    let out = render(&sung, &dir);
    assert!(out.left[..24_000].iter().all(|v| *v == 1.0));
    // Beat 1 is 64 samples before a 512th sample.
    assert!(out.left[24_064..48_000].iter().all(|v| *v == 0.5));
    assert!(out.left[48_128..72_000].iter().all(|v| *v == 0.0));
    assert!(out.left[72_192..].iter().all(|v| *v == 1.0));

    // An audio clip channel's volume moves the same way.
    dir.ramp();
    let mut clips = clips(&[clip_item(0), clip_item(BEAT)]);
    let clip = add_automation(&mut clips, &steps, FULL);
    bind(&mut clips, clip, 1 << 16);
    place(&mut clips, clip, 0, BAR);
    let out = render(&clips, &dir);
    assert_ramp(&out.left, 0, 0..RAMP, 1.0);
    assert_ramp(&out.left, 24_000, 0..RAMP, 0.5);
}

/// A beat of the audio clip channel.
fn clip_item(position: i32) -> PlaylistItem {
    clip(position, BEAT)
}

#[test]
fn a_clip_on_an_insert_s_pan_and_on_a_send_moves_them() {
    let dir = Dir::new("automation-mixer");
    // Insert 1's pan: hard left for two beats, then hard right.
    let sides = [Point::new(0.0, 0.0), hold(2.0, 1.0), Point::new(2.0, 1.0)];
    let mut panned = with_mixer(project(&[(1, &four_beats())], &[item(0, 1)]));
    let clip = add_automation(&mut panned, &sides, FULL);
    bind(&mut panned, clip, insert_control(1, PAN));
    place(&mut panned, clip, 0, BAR);
    let out = render(&panned, &dir);
    assert_impulses(&out.left, &[0, 24_000]);
    assert_impulses(&out.right, &[48_000, 72_000]);
    assert_eq!((out.left[0], out.right[48_000]), (1.0, 1.0));

    // Insert 1 sends to insert 2 alone, at a level of 0 in the project.
    // The clip opens the send to unity on beat 2.
    let opens = [Point::new(0.0, 0.0), hold(2.0, 0.8), Point::new(2.0, 0.8)];
    let mut sent = with_mixer(project(&[(1, &four_beats())], &[item(0, 1)]));
    route(&mut sent, 1, &[2]);
    set(&mut sent, 1, SEND + 2, 0);
    assert_impulses(&render(&sent, &dir).left, &[]);
    let clip = add_automation(&mut sent, &opens, FULL);
    bind(&mut sent, clip, insert_control(1, SEND + 2));
    place(&mut sent, clip, 0, BAR);
    let out = render(&sent, &dir);
    assert_impulses(&out.left, &[48_000, 72_000]);
    assert_eq!((out.left[48_000], out.right[72_000]), (1.0, 1.0));
    assert_eq!((out.automation_clips, out.unsupported_targets), (1, 0));

    // One clip can drive two controls, each over its own range: here the
    // volumes of inserts 1 and 2, which the click passes through in turn.
    let mut both = with_mixer(project(&[(1, &four_beats())], &[item(0, 1)]));
    route(&mut both, 1, &[2]);
    let clip = add_automation(&mut both, &opens, FULL);
    bind(&mut both, clip, insert_control(1, VOLUME));
    bind(&mut both, clip, insert_control(2, VOLUME));
    place(&mut both, clip, 0, BAR);
    let out = render(&both, &dir);
    assert_impulses(&out.left, &[48_000, 72_000]);
    assert_eq!((out.automation_clips, out.unsupported_targets), (1, 0));
}

#[test]
fn automation_comes_out_the_same_in_blocks_of_any_size() {
    let dir = Dir::new("automation-blocks");
    dir.tone(3 * 96_000);
    noisy_wav(&dir, 2);
    // A tone on insert 1 and clicks on insert 2, under a volume fade, a
    // channel volume that steps, a pan sweep and a tempo that climbs.
    let mut held = note(0);
    held.length = BAR;
    let mut clicks = four_beats();
    for click in &mut clicks {
        click.channel = 1;
    }
    let notes = [&[held][..], &clicks].concat();
    let mut song = with_mixer(project(&[(1, &notes)], &[item(0, 1), item(BAR, 1)]));
    add_channel(&mut song, 2);
    song.channels[0].sample_path = Some("tone.wav".to_owned());
    let line = [Point::new(0.0, 0.8), Point::new(8.0, 0.1)];
    let fade = add_automation(&mut song, &line, FULL);
    bind(&mut song, fade, insert_control(1, VOLUME));
    bind(&mut song, fade, insert_control(2, PAN));
    place(&mut song, fade, 0, 2 * BAR);
    let steps = [Point::new(0.0, 0.5), hold(1.0, 0.25), Point::new(3.0, 0.75)];
    let level = add_automation(&mut song, &steps, FULL);
    bind(&mut song, level, 0);
    bind(&mut song, level, (1 << 16) + 1);
    place(&mut song, level, BEAT, BAR);
    let climb = [Point::new(0.0, 0.2), Point::new(8.0, 0.9)];
    let tempo = add_automation(&mut song, &climb, TEMPO_RANGE);
    bind(&mut song, tempo, TEMPO);
    place(&mut song, tempo, 0, 2 * BAR);

    let offline = render(&song, &dir);
    assert_eq!(
        (offline.automation_clips, offline.unsupported_targets),
        (3, 0)
    );
    assert!(offline.left != offline.right);
    let samples = engine::prepare(&song, &dir.0).unwrap();
    for block in [1, 64, 256, 1000] {
        let mut engine = Engine::new(&song, &samples, RATE).unwrap();
        engine.play();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, offline.left.len());
        assert!(sink.left == offline.left, "left in blocks of {block}");
        assert!(sink.right == offline.right, "right in blocks of {block}");
    }

    // A seek lands on the controls as they are at that tick: from the
    // second bar on, where no voice of the first is left, the engine puts
    // out what the render has there.
    let mut engine = Engine::new(&song, &samples, RATE).unwrap();
    engine.play();
    engine.seek(BAR as u64);
    assert_eq!(engine.position(), BAR as u64);
    let mut sink = NullSink::new(256);
    sink.pull(&mut engine, 4096);
    let found = offline.left.windows(4096).position(|w| w == &sink.left[..]);
    let at = found.expect("the samples after the seek are in the render");
    assert!(at > 48_000 && offline.right[at..at + 4096] == sink.right[..]);
}

#[test]
fn a_project_with_automation_is_the_same_project_after_a_save_as_flp() {
    let dir = Dir::new("automation-flp");
    let mut saved = with_mixer(project(&[(1, &four_beats())], &[item(0, 1)]));
    let points = [
        Point::new(0.0, 0.8),
        Point {
            tension: -0.5,
            mode: 10,
            ..Point::new(2.5, 0.25)
        },
        hold(1.5, 1.0),
    ];
    let clip = add_automation(&mut saved, &points, (1250, 4250));
    bind(&mut saved, clip, insert_control(1, VOLUME));
    bind(&mut saved, clip, TEMPO);
    place(&mut saved, clip, BEAT, BAR);
    // The file numbers its events from 0, so the two differ in where
    // their events sit and in nothing else.
    let loaded = reload(&saved, &dir.0);
    assert_eq!(reload(&loaded, &dir.0), loaded);
    assert_eq!(loaded.channels[1].automation, saved.channels[1].automation);
    assert_eq!(
        loaded.channels[1].automation.as_ref().unwrap().envelope,
        points
    );
    assert_eq!(loaded.channels[1].automation_range(), (1250, 4250));
    assert_eq!(loaded.arrangements[0].items, saved.arrangements[0].items);
    let bindings = loaded.automation_bindings();
    assert_eq!(bindings, saved.automation_bindings());
    assert_eq!((bindings.len(), bindings[1].min_raw), (2, Some(60_000.0)));
}

/// Fruity Delay 2 with one echo: no feedback, and a time of 144, which is
/// a dotted eighth.
const DELAY_2: [i32; 8] = [0, 128, 128, 0, 144, 0, 0, 128];

/// Where the buffer stops being silent: every loud sample that has a quiet
/// one before it.
fn onsets(buffer: &[f32]) -> Vec<usize> {
    let loud = impulses(buffer);
    let first = |at: &usize| *at == 0 || !loud.contains(&(at - 1));
    loud.iter().copied().filter(first).collect()
}

#[test]
fn delay_2_echoes_in_time_with_the_song_s_tempo() {
    let dir = Dir::new("delay-tempo");
    let delayed = |notes: &[Note]| {
        let mut project = with_mixer(project(&[(1, notes)], &[item(0, 1)]));
        add_effect(&mut project, 1, 0, "Fruity Delay 2", &params(&DELAY_2));
        project
    };
    // D = round(time * rate * 0.3125 / tempo): 144 * 48000 * 0.3125 / 120
    // is 18,000 frames. At the plugin's own 140 BPM it would be 15,429.
    let out = render(&delayed(&[note(0)]), &dir);
    assert_eq!(out.left[0], 1.0);
    assert_eq!(onsets(&out.left), [0, 18_000]);
    assert!(out.left[18_000] > 0.9);

    // At half the tempo the echo is twice as late.
    let mut slow = delayed(&[note(0)]);
    slow.tempo_milli = Some(60_000);
    assert_eq!(onsets(&render(&slow, &dir).left), [0, 36_000]);

    // A clip takes the tempo to 60 BPM from beat 1 on. The clock turns on
    // the 512th sample after the beat, 24,064, so beat 2 is at 71,936. The
    // click on beat 0 has its echo at the old tempo's 18,000 frames and
    // the one on beat 2 at the new tempo's 36,000.
    let mut turned = delayed(&[note(0), note(2 * BEAT)]);
    let clip = add_automation(
        &mut turned,
        &[Point::new(0.0, 0.0), Point::new(4.0, 0.0)],
        TEMPO_RANGE,
    );
    bind(&mut turned, clip, TEMPO);
    place(&mut turned, clip, BEAT, 3 * BEAT);
    let out = render(&turned, &dir);
    assert_eq!(onsets(&out.left), [0, 18_000, 71_936, 71_936 + 36_000]);
}

/// The ID FL 20.8 gives a control of a slot of an insert.
fn slot_control(insert: u32, slot: u32, control: u32) -> u32 {
    insert_control(insert, control) + (slot << 16)
}

/// The control number of a plugin's parameter 0. The others follow.
const PARAM: u32 = 0x8000;

/// The energy above the low end of each quarter of the first bar: the sum
/// of the squares of the steps from one sample to the next.
fn high_band(buffer: &[f32]) -> [f32; 4] {
    let quarter = |n: usize| {
        let steps = buffer[n * 24_000..(n + 1) * 24_000].windows(2);
        steps.map(|pair| (pair[1] - pair[0]).powi(2)).sum()
    };
    [0, 1, 2, 3].map(quarter)
}

#[test]
fn a_clip_on_a_filter_s_cutoff_closes_it_over_its_bar() {
    let dir = Dir::new("automation-param");
    dir.noise(96_000);
    // Fruity Filter, version 1: wide open, no resonance, the low pass
    // alone.
    let filtered = || {
        let mut project = tone(BAR);
        let state = params(&[1, 1024, 0, 1024, 0, 0, 0]);
        add_effect(&mut project, 1, 0, "Fruity Filter", &state);
        project
    };
    let open = render(&filtered(), &dir);
    let level = high_band(&open.left);
    assert!(level[3] > 0.9 * level[0] && level[3] < 1.1 * level[0]);
    assert_eq!((open.automation_clips, open.slot_targets), (0, 0));

    // The clip takes the cutoff, parameter 0, from 1024 down to 0.
    let mut swept = filtered();
    let line = [Point::new(0.0, 1.0), Point::new(4.0, 0.0)];
    let clip = add_automation(&mut swept, &line, FULL);
    bind(&mut swept, clip, slot_control(1, 0, PARAM));
    place(&mut swept, clip, 0, BAR);
    let out = render(&swept, &dir);
    assert_eq!(
        (
            out.automation_clips,
            out.slot_targets,
            out.unsupported_targets
        ),
        (1, 1, 0)
    );
    let falling = high_band(&out.left);
    assert!(falling[0] < level[0], "{falling:?} against {level:?}");
    assert!(falling.windows(2).all(|pair| pair[1] < 0.5 * pair[0]));
    assert!(falling[3] < 0.001 * level[3], "{falling:?}");

    // Half the clip's range stops half-way down the wheel, at 512, where
    // the filter still lets a part of the top through.
    let mut half = filtered();
    let clip = add_automation(&mut half, &line, (6400, 12_800));
    bind(&mut half, clip, slot_control(1, 0, PARAM));
    place(&mut half, clip, 0, BAR);
    let stopped = high_band(&render(&half, &dir).left);
    assert!(stopped[3] > 100.0 * falling[3] && stopped[3] < 0.5 * level[3]);

    // A parameter the filter does not have, a parameter of a plugin the
    // engine does not host and that plugin's mix level are not played.
    let mut more = swept.clone();
    add_effect(&mut more, 1, 1, "Fruity Reeverb 2", &[1, 2, 3]);
    for target in [
        slot_control(1, 0, PARAM + 6),
        slot_control(1, 1, PARAM),
        slot_control(1, 1, SLOT_MIX),
    ] {
        bind(&mut more, clip, target);
    }
    let counted = render(&more, &dir);
    assert_eq!(
        (
            counted.automation_clips,
            counted.slot_targets,
            counted.unsupported_targets
        ),
        (1, 1, 3)
    );
    assert!(counted.left == out.left);
}

#[test]
fn a_clip_on_a_slot_s_mix_level_and_on_its_switch_takes_the_plugin_out() {
    let dir = Dir::new("automation-slot");
    dir.tone(3 * 96_000);
    // Fruity Balance with its volume all the way up, +5.6 dB, in slot 2.
    let boost = (11f32.powf(1.25) - 1.0) / 10.0;
    let boosted = || {
        let mut project = tone(BAR + 2 * BEAT);
        add_effect(&mut project, 1, 2, "Fruity Balance", &params(&[0, 320]));
        project
    };
    let plain = render(&boosted(), &dir);
    assert!(plain.left.iter().all(|v| (v - boost).abs() < 1e-6));

    // The clip takes the mix level from all the plugin's to none of it
    // over the first bar. What is left then is the slot's input.
    let mut mixed = boosted();
    let line = [Point::new(0.0, 1.0), Point::new(4.0, 0.0)];
    let clip = add_automation(&mut mixed, &line, FULL);
    bind(&mut mixed, clip, slot_control(1, 2, SLOT_MIX));
    place(&mut mixed, clip, 0, BAR);
    let out = render(&mixed, &dir);
    assert_eq!(
        (
            out.automation_clips,
            out.slot_targets,
            out.unsupported_targets
        ),
        (1, 1, 0)
    );
    assert_eq!(out.left[0], plain.left[0]);
    assert!(out.left[..96_256].windows(2).all(|pair| pair[1] <= pair[0]));
    assert!(out.left[48_000] < plain.left[0] && out.left[48_000] > 1.0);
    assert!(out.left[96_256..].iter().all(|v| *v == 1.0));

    // A clip that drops to 0 on beat 2 switches the slot off there, on
    // the 512th sample after the beat.
    let mut switched = boosted();
    let step = [Point::new(0.0, 1.0), hold(2.0, 0.0), Point::new(2.0, 0.0)];
    let clip = add_automation(&mut switched, &step, FULL);
    bind(&mut switched, clip, slot_control(1, 2, SLOT_ENABLED));
    place(&mut switched, clip, 0, BAR);
    let out = render(&switched, &dir);
    assert_eq!((out.automation_clips, out.slot_targets), (1, 1));
    assert!(out.left[..48_128] == plain.left[..48_128]);
    assert!(out.left[48_128..].iter().all(|v| *v == 1.0));
}

/// Fruity Compressor as FL sets a new one, in a version 3 state. Its ratio
/// is 1, so all it does to a click is hold it for the 64 frames its
/// limiter looks ahead and take it through a 3 Hz high pass.
const COMPRESSOR: [i32; 9] = [3, 0, 10, 0, 150, 200, 0, 1, 0];
/// How many frames late Fruity Compressor is.
const LATE: usize = 64;

/// Where the buffer is loudest.
fn peak_at(buffer: &[f32]) -> usize {
    let frames = buffer.iter().enumerate();
    let loudest = frames.max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()));
    loudest.unwrap().0
}

/// The mixer fixture with a second channel into insert 2, and the note at
/// tick 0 on both. Insert 1 is panned left and insert 2 right.
fn two_inserts(first: Note) -> Project {
    let second = Note {
        channel: 1,
        ..first
    };
    let mut item = item(0, 1);
    item.length = first.length.max(BAR);
    let mut project = with_mixer(project(&[(1, &[first, second])], &[item]));
    add_channel(&mut project, 2);
    set(&mut project, 1, PAN, -6400);
    set(&mut project, 2, PAN, 6400);
    project
}

#[test]
fn a_late_plugin_holds_the_other_insert_back_to_come_out_with_it() {
    let dir = Dir::new("pdc");
    let mut late = two_inserts(note(0));
    add_effect(&mut late, 1, 0, "Fruity Compressor", &params(&COMPRESSOR));
    let out = render(&late, &dir);
    // The click through the compressor is on the left, and the dry one on
    // the right is as late.
    assert_eq!(peak_at(&out.left), LATE);
    assert!(out.left[LATE] > 0.99 && out.left[..LATE].iter().all(|v| *v == 0.0));
    assert_eq!(impulses(&out.right), [LATE]);
    assert_eq!(out.right[LATE], 1.0);
    assert_eq!((out.latency, out.delayed_inserts), (LATE, 1));

    // Without the plugin nothing waits.
    let out = render(&two_inserts(note(0)), &dir);
    assert_eq!(
        (impulses(&out.left), impulses(&out.right)),
        (vec![0], vec![0])
    );
    assert_eq!((out.latency, out.delayed_inserts), (0, 0));
    // Nor with its slot switched off.
    let mut off = late.clone();
    set_slot(&mut off, 1, 0, SLOT_ENABLED, 0);
    let out = render(&off, &dir);
    assert_eq!(
        (impulses(&out.left), impulses(&out.right)),
        (vec![0], vec![0])
    );
    assert_eq!(
        (out.hosted_slots, out.latency, out.delayed_inserts),
        (1, 0, 0)
    );

    // The master's own voices wait as well.
    let mut master = late.clone();
    master.channels[1].routed_to = Some(0);
    let out = render(&master, &dir);
    assert_eq!(impulses(&out.right), [LATE]);
    assert_eq!(peak_at(&out.left), LATE);
    assert!(out.left[LATE] > 1.99);
    assert_eq!((out.latency, out.delayed_inserts), (LATE, 1));
}

#[test]
fn a_send_between_two_inserts_keeps_them_in_step() {
    let dir = Dir::new("pdc-send");
    // Insert 1 has the compressor and sends to insert 2 alone. Insert 2's
    // own click waits for it, so both leave insert 2 together.
    let mut through = two_inserts(note(0));
    add_effect(
        &mut through,
        1,
        0,
        "Fruity Compressor",
        &params(&COMPRESSOR),
    );
    route(&mut through, 1, &[2]);
    set(&mut through, 2, PAN, 0);
    let out = render(&through, &dir);
    assert_eq!(impulses(&out.right), [LATE]);
    assert_eq!(out.right[LATE], 1.0);
    assert_eq!(peak_at(&out.left), LATE);
    assert!(out.left[LATE] > 1.99 && out.left[..LATE].iter().all(|v| *v == 0.0));
    assert_eq!((out.latency, out.delayed_inserts), (LATE, 1));

    // Insert 2 has the compressor, and insert 1 sends to it and to the
    // master. The send to insert 2 goes in at once and the one to the
    // master waits, so the click arrives by both ways on one sample.
    let mut split = one_click();
    add_effect(&mut split, 2, 0, "Fruity Compressor", &params(&COMPRESSOR));
    route(&mut split, 1, &[0, 2]);
    set(&mut split, 2, PAN, -6400);
    let out = render(&split, &dir);
    assert_eq!(impulses(&out.right), [LATE]);
    assert_eq!(out.right[LATE], 1.0);
    assert_eq!(peak_at(&out.left), LATE);
    assert!(out.left[LATE] > 1.99 && out.left[..LATE].iter().all(|v| *v == 0.0));
    assert_eq!((out.latency, out.delayed_inserts), (LATE, 1));
}

#[test]
fn switching_a_late_plugin_off_takes_the_delay_out_without_a_click() {
    let dir = Dir::new("pdc-switch");
    dir.sine(96_000);
    // A bar of a 100 Hz sine into both inserts.
    let toned = || {
        let mut held = note(0);
        held.length = BAR;
        let mut project = two_inserts(held);
        for channel in &mut project.channels {
            channel.sample_path = Some("tone.wav".to_owned());
        }
        project
    };
    let plain = render(&toned(), &dir);
    // A clip switches the compressor's slot off on beat 2, which the
    // engine reads on the 512th sample after it.
    let mut switched = toned();
    add_effect(
        &mut switched,
        1,
        0,
        "Fruity Compressor",
        &params(&COMPRESSOR),
    );
    let step = [Point::new(0.0, 1.0), hold(2.0, 0.0), Point::new(2.0, 0.0)];
    let clip = add_automation(&mut switched, &step, FULL);
    bind(&mut switched, clip, slot_control(1, 0, SLOT_ENABLED));
    place(&mut switched, clip, 0, BAR);
    let out = render(&switched, &dir);
    assert_eq!((out.slot_targets, out.latency), (1, LATE));
    let (switch, faded) = (48_128, 48_128 + 256);

    // Insert 2, on the right, is 64 frames late up to the switch and on
    // time once the fade is over. So is insert 1, where the compressor's
    // high pass turns the sine by a degree or two.
    assert!(out.right[LATE..switch] == plain.right[..switch - LATE]);
    assert!(out.right[faded..] == plain.right[faded..]);
    let held = out.left[LATE..switch].iter().zip(&plain.left);
    assert!(held.skip(4800).all(|(got, want)| (got - want).abs() < 0.02));
    assert!(out.left[faded..] == plain.left[faded..]);

    // Through the fade neither side steps much further than the sine
    // does from one sample to the next by itself. Switched in one frame,
    // the widest step is 18 times the sine's.
    let step = |buffer: &[f32]| {
        let steps = buffer.windows(2).map(|pair| (pair[1] - pair[0]).abs());
        steps.fold(0f32, f32::max)
    };
    let sine = step(&plain.right[1000..90_000]);
    assert!((sine - 0.5 * std::f32::consts::TAU * 100.0 / RATE as f32).abs() < 1e-4);
    for side in [&out.left, &out.right] {
        let widest = step(&side[switch - 1000..faded + 1000]);
        assert!(widest < 1.5 * sine, "{widest} against {sine}");
    }

    // The fade counts frames, so it comes out the same however the output
    // is cut into blocks.
    let samples = engine::prepare(&switched, &dir.0).unwrap();
    for block in [1, 64, 1000] {
        let mut engine = Engine::new(&switched, &samples, RATE).unwrap();
        engine.play();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, out.left.len());
        assert!(sink.left == out.left, "left in blocks of {block}");
        assert!(sink.right == out.right, "right in blocks of {block}");
    }
}

/// Fruity Compressor in a version 3 state that listens to sidechain input
/// 1: a threshold of -30 dB, 4 : 1, 1 ms of attack and 50 ms of release.
const DUCKER: [i32; 9] = [3, -300, 40, 0, 10, 50, 0, 1, 1];
/// The frame the first kick of [`kick_and_tone`] starts on: 11 ticks in,
/// where the tone is at a peak.
const KICK: usize = 2750;

/// [`kick_and_bare_tone`] with [`DUCKER`] in the first slot of insert 2,
/// and a send from insert 1 to insert 2 at level 0 beside the one to the
/// master, which is what FL's "Sidechain to this track" leaves.
fn kick_and_tone(dir: &Dir) -> Project {
    let mut project = kick_and_bare_tone(dir);
    route(&mut project, 1, &[0, 2]);
    set(&mut project, 1, SEND + 2, 0);
    add_effect(&mut project, 2, 0, "Fruity Compressor", &params(&DUCKER));
    project
}

/// The mixer fixture with a kick on every beat into insert 1, which is
/// panned left, and a 100 Hz tone at half scale for the bar into insert 2,
/// which is panned right. A kick is 2,048 frames at full scale. Both
/// inserts send to the master alone and have no plugin.
fn kick_and_bare_tone(dir: &Dir) -> Project {
    dir.wav("kick.wav", &(0..2048).collect::<Vec<_>>());
    dir.sine(2 * 96_000);
    let mut notes: Vec<Note> = [0, 1, 2, 3].map(|beat| note(11 + beat * BEAT)).into();
    let mut held = note(0);
    (held.channel, held.length) = (1, BAR);
    notes.push(held);
    let mut project = with_mixer(project(&[(1, &notes)], &[item(0, 1)]));
    project.channels[0].sample_path = Some("kick.wav".to_owned());
    add_channel(&mut project, 2);
    project.channels[1].sample_path = Some("tone.wav".to_owned());
    set(&mut project, 1, PAN, -6400);
    set(&mut project, 2, PAN, 6400);
    project
}

#[test]
fn a_kick_ducks_the_tone_on_the_insert_it_sidechains() {
    let dir = Dir::new("sidechain");
    let ducked = kick_and_tone(&dir);
    let out = render(&ducked, &dir);
    assert_eq!((out.sidechain_routes, out.sidechain_slots), (1, 1));
    assert!(out.approximated_slots.is_empty());
    assert_eq!((out.hosted_slots, out.latency), (1, LATE));
    // The loudest the tone gets over one of its periods.
    let level = |out: &Rendered, from: usize| {
        let period = &out.right[from..from + 480];
        period.iter().fold(0f32, |peak, v| peak.max(v.abs()))
    };
    for beat in 0..4 {
        // The kick is on the left alone, as late as the compressor makes
        // the tone.
        let kick = KICK + beat * 24_000 + LATE;
        assert_eq!((out.left[kick - 1], out.left[kick]), (0.0, 1.0));
        // Up to the kick the tone is at its own level. Under the kick it
        // is down by what 30 dB over the threshold come to at 4 : 1, and
        // nothing of the kick is in it.
        assert!((level(&out, kick - 480) - 0.5).abs() < 0.01);
        let low = db(level(&out, kick + 1024) / 0.5);
        assert!((low + 22.5).abs() < 0.5, "beat {beat}: {low} dB");
    }

    // With no kick the input is silent and the tone stays where it is.
    let mut alone = ducked.clone();
    alone.patterns[0].notes.retain(|note| note.channel == 1);
    let plain = render(&alone, &dir);
    assert_eq!((plain.sidechain_routes, plain.sidechain_slots), (1, 1));
    assert!(plain.left.iter().all(|v| *v == 0.0));
    // The compressor's high pass takes a tenth of a second to settle.
    for from in (4_800..90_000).step_by(480) {
        assert!((level(&plain, from) - 0.5).abs() < 0.01, "{from}");
    }
    // The tone starts to dip on the frame the kick starts, give or take
    // the attack: the first frame it is under 0.9 of the tone alone.
    let dip = |out: &Rendered| {
        let (tone, whole) = (&out.right[out.latency..], &plain.right[LATE..]);
        let mut frames = tone.iter().zip(whole);
        frames.position(|(v, w)| w.abs() > 0.1 && v.abs() < 0.9 * w.abs())
    };
    let onset = dip(&out).unwrap();
    assert!((KICK..KICK + 12).contains(&onset), "{onset}");

    // A plugin that makes the kick late holds the tone back for it, and
    // the dip is still where the kick is.
    let mut late = ducked.clone();
    add_effect(&mut late, 1, 0, "Fruity Compressor", &params(&COMPRESSOR));
    let out = render(&late, &dir);
    assert_eq!((out.latency, out.sidechain_slots), (2 * LATE, 1));
    let heard = out.left.iter().position(|v| v.abs() > 0.5);
    assert_eq!(heard, Some(KICK + 2 * LATE));
    let onset = dip(&out).unwrap();
    assert!((KICK..KICK + 12).contains(&onset), "{onset}");

    // Without the route insert 2 has no input 1. The compressor hears the
    // tone, which is 24 dB over its threshold all along, and the slot is
    // counted.
    let mut unrouted = ducked.clone();
    route(&mut unrouted, 1, &[0]);
    let out = render(&unrouted, &dir);
    assert_eq!((out.sidechain_routes, out.sidechain_slots), (0, 0));
    assert_eq!(out.approximated_slots.values().sum::<usize>(), 1);
    let low = db(level(&out, 48_000) / 0.5);
    assert!((low + 18.0).abs() < 0.5, "{low} dB");

    // A route that is heard is an input all the same: the kick is in the
    // insert's bus and the compressor's level is the kick's.
    let mut heard = ducked.clone();
    set(&mut heard, 1, SEND + 2, 12_800);
    let out = render(&heard, &dir);
    assert_eq!((out.sidechain_routes, out.sidechain_slots), (1, 1));
    let kick = KICK + 24_000 + LATE;
    assert!((level(&out, kick - 480) - 0.5).abs() < 0.01);
    assert!(level(&out, kick + 1024) < 0.1);
}

#[test]
fn a_sidechained_tone_comes_out_the_same_in_blocks_of_any_size() {
    let dir = Dir::new("sidechain-blocks");
    let mut project = kick_and_tone(&dir);
    add_effect(
        &mut project,
        1,
        0,
        "Fruity Compressor",
        &params(&COMPRESSOR),
    );
    let out = render(&project, &dir);
    let samples = engine::prepare(&project, &dir.0).unwrap();
    for block in [64, 333, 512, 4096] {
        let mut engine = Engine::new(&project, &samples, RATE).unwrap();
        engine.play();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, out.left.len());
        assert!(sink.left == out.left, "left in blocks of {block}");
        assert!(sink.right == out.right, "right in blocks of {block}");
    }
}

/// Fruity Stereo Shaper as its "Left - Right splitter" preset has it: the
/// left kept, the right muted, and the difference to the second insert
/// its own sends to.
const SPLITTER: [i32; 9] = [0, 0, 12_800, 0, 0, 0, 0, 2, 1];

#[test]
fn the_shaper_s_difference_goes_into_the_insert_its_number_counts_to() {
    let dir = Dir::new("splitter");
    // Insert 1 has the splitter and sends to the master and, at level 0,
    // to insert 2. The left of the click stays on insert 1 and the right
    // is on insert 2, though the send carries nothing.
    let mut split = one_click();
    add_effect(&mut split, 1, 0, "Fruity Stereo Shaper", &params(&SPLITTER));
    route(&mut split, 1, &[0, 2]);
    set(&mut split, 1, SEND + 2, 0);
    let (left, right, full) = ((1.0, 0.0), (0.0, 1.0), (1.0, 1.0));
    assert_eq!(meters(&split, &dir), [(0, full), (1, left), (2, right)]);
    let out = render(&split, &dir);
    assert_eq!((out.left[0], out.right[0], out.hosted_slots), (1.0, 1.0, 1));
    assert_eq!(
        (impulses(&out.left), impulses(&out.right)),
        ([0].into(), [0].into())
    );

    // Insert 2's volume and pan are on it, and insert 1's are not.
    let mut panned = split.clone();
    set(&mut panned, 1, VOLUME, 0);
    set(&mut panned, 2, PAN, -6400);
    assert_eq!(
        meters(&panned, &dir)[1..],
        [(1, (0.0, 0.0)), (2, (0.0, 0.0))]
    );
    set(&mut panned, 2, PAN, 6400);
    assert_eq!(
        meters(&panned, &dir),
        [(0, right), (1, (0.0, 0.0)), (2, right)]
    );

    // A number past the inserts it sends to is the last of them, and
    // number 1 is the first, here the master.
    let mut only = split.clone();
    route(&mut only, 1, &[2]);
    assert_eq!(meters(&only, &dir), [(0, right), (1, left), (2, right)]);
    let mut first = split.clone();
    first.inserts[1].slots[0].plugin_state = Some(params(&[0, 0, 12_800, 0, 0, 0, 0, 1, 1]));
    assert_eq!(meters(&first, &dir), [(0, full), (1, left)]);
    // With nowhere to send, or switched off, the difference goes nowhere.
    let mut nowhere = split.clone();
    route(&mut nowhere, 1, &[]);
    assert_eq!(meters(&nowhere, &dir), [(1, left)]);
    let mut off = split.clone();
    set_slot(&mut off, 1, 0, SLOT_ENABLED, 0);
    assert_eq!(meters(&off, &dir), [(0, full), (1, full), (2, (0.0, 0.0))]);

    // Behind a late plugin the difference waits with the insert's sends:
    // both sides of the click come out together.
    let mut late = split.clone();
    add_effect(&mut late, 1, 1, "Fruity Compressor", &params(&COMPRESSOR));
    let out = render(&late, &dir);
    assert_eq!(out.latency, LATE);
    assert_eq!(
        (peak_at(&out.left), impulses(&out.right)),
        (LATE, vec![LATE])
    );
    // And ahead of one it is as late as the plugin made it.
    let mut after = split.clone();
    add_effect(&mut after, 1, 1, "Fruity Compressor", &params(&COMPRESSOR));
    after.inserts[1].slots[0].index = 2;
    after.inserts[1].slots[0].wrapper.as_mut().unwrap().slot = 2;
    let out = render(&after, &dir);
    assert_eq!(out.latency, LATE);
    assert_eq!(peak_at(&out.left), LATE);
    assert_eq!(peak_at(&out.right), LATE);
}

/// A second channel, a plugin channel that plays `plugin` into the master:
/// its name, the wrapper that says it is a generator, and its state.
fn add_generator(project: &mut Project, plugin: &str, state: &[u8]) {
    add_channel(project, 0);
    let channel = &mut project.channels[1];
    channel.kind = Some(ChannelKind::Plugin);
    channel.sample_path = None;
    channel
        .order
        .retain(|placed| placed.item != Item::Field(Id::SamplePath));
    channel.plugin_name = Some(plugin.to_owned());
    channel.wrapper = Some(PluginWrapper {
        insert: 0,
        slot: 0,
        plugin_kind: -1,
        unknown_12: [0; 4],
        flags: 0x50,
        page: 0,
        unknown_21: [0; 15],
        editor_position: 0,
        editor_size: 0,
        len: 52,
    });
    channel.plugin_state = Some(state.to_vec());
    let last = channel.order.last().unwrap().pos;
    let ids = [Id::PluginInternalName, Id::PluginWrapper, Id::PluginData];
    let placed = ids.into_iter().enumerate().map(|(n, id)| Placed {
        pos: last + 1 + n,
        item: Item::Field(id),
    });
    channel.order.extend(placed);
}

/// A bar of pattern 1 with these notes, and a channel 1 that plays them on
/// TestSine with its level all the way up.
fn sine_song(notes: &[Note]) -> Project {
    let mut song = project(&[(1, notes)], &[item(0, 1)]);
    add_generator(&mut song, "TestSine", &params(&[100]));
    song
}

/// A note for channel 1.
fn sine_note(position: i32, length: i32, key: i16) -> Note {
    Note {
        position,
        length,
        key,
        channel: 1,
        ..Note::default()
    }
}

/// Frame `n` of a TestSine note of this key at full level: key 69 is 440
/// Hz.
fn sine_frame(key: i16, n: usize) -> f32 {
    let step = 440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0) / f64::from(RATE);
    (TAU * step * n as f64).sin() as f32
}

/// The frames TestSine sounds on for after a note off.
const TAIL: usize = 2400;

/// Check that `buffer` from `at` on is the tail of a TestSine note that
/// was let go `held` frames in, and silent after it.
fn assert_tail(buffer: &[f32], at: usize, key: i16, held: usize) {
    let decay = TestSine::default().decay();
    let mut level = 1.0;
    for n in 0..TAIL {
        level *= decay;
        let want = sine_frame(key, held + n) * level;
        assert_eq!(buffer[at + n], want, "frame {n} of the tail");
    }
    assert!(buffer[at + TAIL..].iter().all(|v| *v == 0.0));
}

#[test]
fn a_generator_s_note_is_a_sine_from_its_onset_to_the_end_of_its_tail() {
    let dir = Dir::new("generator-note");
    assert_eq!(TestSine::default().tail(), TAIL);
    // A beat long, on the second beat of the bar.
    let out = render(&sine_song(&[sine_note(BEAT, BEAT, 69)]), &dir);
    assert_eq!((out.notes, out.voices, out.skipped), (1, 0, 0));
    assert_eq!(out.hosted_generators, 1);
    assert!(out.unhosted_generators.is_empty());
    assert_eq!(out.left, out.right);
    let (on, off) = (24_000, 48_000);
    assert!(out.left[..=on].iter().all(|v| *v == 0.0));
    assert!(out.left[on + 1] > 0.05);
    for n in 0..off - on {
        assert_eq!(out.left[on + n], sine_frame(69, n), "frame {n}");
    }
    assert_tail(&out.left, off, 69, off - on);
    assert_eq!(out.left.len(), 96_000);

    // A note that ends with the song has its tail past the song's end.
    let out = render(&sine_song(&[sine_note(3 * BEAT, BEAT, 57)]), &dir);
    assert_eq!(out.left.len(), 96_000 + TAIL);
    assert_eq!(out.left[95_999], sine_frame(57, 23_999));
    assert_tail(&out.left, 96_000, 57, 24_000);

    // A step sequencer note has no length: it is let go on its first
    // frame.
    let out = render(&sine_song(&[sine_note(BEAT, 0, 69)]), &dir);
    assert!(out.left[..=on].iter().all(|v| *v == 0.0));
    assert_tail(&out.left, on, 69, 0);
}

#[test]
fn a_generator_s_note_takes_its_velocity_its_pan_and_its_pitch() {
    let dir = Dir::new("generator-levels");
    let mut soft = sine_note(2 * BEAT, BEAT, 69);
    soft.velocity = 50;
    let out = render(&sine_song(&[sine_note(0, BEAT, 69), soft]), &dir);
    for n in (0..24_000).step_by(7) {
        assert_eq!(out.left[n], sine_frame(69, n));
        assert_eq!(out.left[48_000 + n], 0.5 * sine_frame(69, n));
    }
    assert_eq!(out.left, out.right);

    // The note's own pan, hard left and hard right.
    let (mut left, mut right) = (sine_note(0, BEAT, 69), sine_note(2 * BEAT, BEAT, 69));
    (left.pan, right.pan) = (0, 128);
    let out = render(&sine_song(&[left, right]), &dir);
    assert!(out.right[..48_000].iter().all(|v| *v == 0.0));
    assert!(out.left[48_000..].iter().all(|v| *v == 0.0));
    assert_eq!(out.left[100], sine_frame(69, 100));
    assert_eq!(out.right[48_100], sine_frame(69, 100));

    // An octave up and 50 cents on top: 12.5 semitones over 440 Hz.
    let mut sharp = sine_note(0, BEAT, 81);
    sharp.fine_pitch = 125;
    let out = render(&sine_song(&[sharp]), &dir);
    let step = 440.0 * 2f64.powf(12.5 / 12.0) / f64::from(RATE);
    for n in (0..24_000).step_by(7) {
        assert_eq!(out.left[n], (TAU * step * n as f64).sin() as f32);
    }

    // The channel's root note moves the key, as it does for a sampler: a
    // root 3 keys under 60 plays every note 3 keys up.
    let mut rooted = sine_song(&[sine_note(0, BEAT, 69)]);
    rooted.channels[1].root_note = Some(57);
    rooted.channels[1].order.push(Placed {
        pos: 150,
        item: Item::Field(Id::RootNote),
    });
    let out = render(&rooted, &dir);
    assert_eq!(out.left[100], sine_frame(72, 100));
}

#[test]
fn a_generator_without_its_files_is_counted_and_told_fl_s_plugin_directory() {
    let dir = Dir::new("generator-files");
    // BooBass plays samples that FL ships. A channel without a 213 has the
    // plugin's defaults.
    let mut song = sine_song(&[sine_note(0, BEAT, 48)]);
    song.channels[1].plugin_name = Some("BooBass".to_owned());
    song.channels[1].plugin_state = None;
    let render = |sample_root: &Path, plugin_dir: Option<PathBuf>| {
        let opts = RenderOptions {
            sample_rate: RATE,
            sample_root: sample_root.to_path_buf(),
            max_seconds: 60.0,
            plugin_dir,
        };
        engine::render(&song, &opts).unwrap()
    };
    // No directory, and one that has nothing in it: hosted, silent and
    // counted with what it lacks.
    for plugin_dir in [None, Some(dir.0.clone())] {
        let out = render(&dir.0, plugin_dir);
        assert_eq!(out.hosted_generators, 1);
        assert!(out.unhosted_generators.is_empty());
        assert_eq!(out.approximated_generators.values().sum::<usize>(), 1);
        assert!(out.left.iter().chain(&out.right).all(|v| *v == 0.0));
    }
    // A generator that needs no file is not counted.
    let sine = sine_song(&[sine_note(0, BEAT, 48)]);
    assert!(self::render(&sine, &dir).approximated_generators.is_empty());

    // With FL's install at hand it plays, when it is told the directory and
    // when the sample root is FL's `Data`, which has it next door.
    let Some(data) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        return;
    };
    let generators = data.parent().unwrap().join("Plugins/Fruity/Generators");
    for (sample_root, plugin_dir) in [(&dir.0, Some(generators)), (&data, None)] {
        let out = render(sample_root, plugin_dir);
        assert!(out.approximated_generators.is_empty());
        assert!(out.left.iter().any(|v| v.abs() > 0.01));
    }
}

#[test]
fn fl_s_curves_hand_a_generator_the_note_s_volume_and_pan_its_channel() {
    let dir = Dir::new("generator-fl-exact");
    let mut soft = sine_note(2 * BEAT, BEAT, 69);
    soft.velocity = 64;
    let song = sine_song(&[sine_note(0, BEAT, 69), soft]);
    let out = render_exact(&song, &dir);
    // The generator gets 1 for velocity 100 and (11^0.64 - 1) / 10 for 64.
    // Its channel's volume and pan go on what it plays: 0.551 over the
    // square root of 2 for a new channel.
    let vol = engine::FlExact.velocity_gain(64);
    assert!((vol - 0.364).abs() < 1e-3);
    let channel = circular(0.0, new_channel()).0;
    for n in (0..24_000).step_by(7) {
        let sine = channel * sine_frame(69, n);
        assert!((out.left[n] - sine).abs() < 1e-6, "frame {n}");
        assert!(
            (out.left[48_000 + n] - vol * sine).abs() < 1e-6,
            "frame {n}"
        );
    }
    assert_eq!(out.left, out.right);

    // The channel's pan is on the circle: all the way left is 3 dB up.
    let mut left = song;
    left.channels[1].levels.as_mut().unwrap().pan = 0;
    let out = render_exact(&left, &dir);
    assert!(out.right.iter().all(|v| *v == 0.0));
    assert!((out.left[100] - new_channel() as f32 * sine_frame(69, 100)).abs() < 1e-6);
}

#[test]
fn a_generator_plays_through_its_channel_s_volume_and_pan() {
    let dir = Dir::new("generator-channel");
    let levels = |pan: u32, volume: u32| {
        let mut song = sine_song(&[sine_note(0, BEAT, 69)]);
        let levels = song.channels[1].levels.as_mut().unwrap();
        (levels.pan, levels.volume) = (pan, volume);
        render(&song, &dir)
    };
    // Full left mutes the right, tail and all.
    let out = levels(0, 10_000);
    assert!(out.right.iter().all(|v| *v == 0.0));
    for n in (0..24_000).step_by(7) {
        assert_eq!(out.left[n], sine_frame(69, n));
    }
    assert_tail(&out.left, 24_000, 69, 24_000);
    let out = levels(12_800, 10_000);
    assert!(out.left.iter().all(|v| *v == 0.0));
    assert_eq!(out.right[100], sine_frame(69, 100));
    // Half the volume is half the amplitude on both sides.
    let out = levels(6400, 5000);
    assert_eq!(out.left[100], 0.5 * sine_frame(69, 100));
    assert_eq!(out.left, out.right);

    // The state is the plugin's: a level of 25 in the 213.
    let mut quiet = sine_song(&[sine_note(0, BEAT, 69)]);
    quiet.channels[1].plugin_state = Some(params(&[25]));
    assert_eq!(render(&quiet, &dir).left[100], 0.25 * sine_frame(69, 100));

    // The channel plays into its insert, whose volume goes on top.
    let mut mixed = with_mixer(sine_song(&[sine_note(0, BEAT, 69)]));
    mixed.channels[1].routed_to = Some(2);
    set(&mut mixed, 2, PAN, -6400);
    let out = render(&mixed, &dir);
    assert_eq!(out.left[100], sine_frame(69, 100));
    assert!(out.right.iter().all(|v| *v == 0.0));
}

#[test]
fn two_notes_of_a_generator_that_overlap_add_up() {
    let dir = Dir::new("generator-sum");
    // Two beats of one key, and a beat of another from the second beat on.
    // The sampler channel clicks on top.
    let notes = [
        sine_note(0, 2 * BEAT, 69),
        sine_note(BEAT, BEAT, 76),
        note(BEAT),
    ];
    let out = render(&sine_song(&notes), &dir);
    assert_eq!((out.notes, out.voices, out.hosted_generators), (3, 1, 1));
    for n in 1..24_000 {
        assert_eq!(out.left[n], sine_frame(69, n), "frame {n}");
        let both = sine_frame(69, 24_000 + n) + sine_frame(76, n);
        assert_eq!(out.left[24_000 + n], both, "frame {n} of the two");
    }
    assert_eq!(out.left[24_000], 1.0 + sine_frame(69, 24_000));
    // Both are let go on the third beat, and their tails add up too.
    let decay = TestSine::default().decay();
    let first = sine_frame(69, 48_000) * decay;
    assert_eq!(out.left[48_000], first + sine_frame(76, 24_000) * decay);
    assert!(out.left[48_000 + TAIL..].iter().all(|v| *v == 0.0));

    // Two notes of one key on one tick are two notes.
    let twice = [sine_note(0, BEAT, 69), sine_note(0, 2 * BEAT, 69)];
    let out = render(&sine_song(&twice), &dir);
    assert_eq!(out.left[100], sine_frame(69, 100) + sine_frame(69, 100));
    assert_eq!(out.left[30_000], sine_frame(69, 30_000));
}

#[test]
fn a_generator_s_notes_come_out_the_same_in_blocks_of_any_size() {
    let dir = Dir::new("generator-blocks");
    // Notes that start and end off every block's edge: a tick is 250
    // samples. The last one ends with the song, so its tail plays out
    // after the clock has stopped.
    let mut soft = sine_note(5, 130, 64);
    (soft.velocity, soft.pan) = (70, 20);
    let notes = [
        sine_note(1, 37, 69),
        soft,
        sine_note(9, 0, 72),
        sine_note(300, 84, 57),
        note(3),
    ];
    let song = sine_song(&notes);
    let offline = render(&song, &dir);
    assert_eq!(offline.left.len(), 96_000 + TAIL);
    let samples = engine::prepare(&song, &dir.0).unwrap();
    for block in [1, 64, 256, 1000] {
        let mut engine = Engine::new(&song, &samples, RATE).unwrap();
        engine.play();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, offline.left.len() + 1000);
        let len = offline.left.len();
        assert!(
            sink.left[..len] == offline.left,
            "left in blocks of {block}"
        );
        assert!(
            sink.right[..len] == offline.right,
            "right in blocks of {block}"
        );
        assert!(sink.left[len..].iter().all(|v| *v == 0.0));
        assert_eq!((engine.is_playing(), engine.position()), (false, 0));
    }
}

#[test]
fn stop_cuts_a_generator_s_notes_and_a_loop_lets_them_go() {
    let dir = Dir::new("generator-transport");
    // One note, held for the whole bar.
    let song = sine_song(&[sine_note(0, BAR, 69)]);
    let samples = engine::prepare(&song, &dir.0).unwrap();
    let mut engine = Engine::new(&song, &samples, RATE).unwrap();
    engine.play();
    let mut sink = NullSink::new(256);
    sink.pull(&mut engine, 1000);
    assert_eq!(sink.left[999], sine_frame(69, 999));
    engine.stop();
    sink.pull(&mut engine, 1100);
    // The note is not picked up part-way, and nor is it after a seek into
    // it.
    engine.play();
    sink.pull(&mut engine, 2000);
    engine.seek(48);
    sink.pull(&mut engine, 3000);
    assert!(sink.left[1000..].iter().all(|v| *v == 0.0));

    // With the loop on the note starts over on the sample the song ends
    // on, over the tail of the round before.
    engine.seek(0);
    engine.set_loop(true);
    let mut sink = NullSink::new(1000);
    sink.pull(&mut engine, 96_000 + TAIL + 100);
    let decay = TestSine::default().decay();
    let mut level = 1.0;
    for n in 0..TAIL {
        level *= decay;
        let want = sine_frame(69, 96_000 + n) * level + sine_frame(69, n);
        assert_eq!(sink.left[96_000 + n], want, "frame {n} of the second round");
    }
    assert_eq!(sink.left[96_000 + TAIL], sine_frame(69, TAIL));
    assert!(engine.is_playing());
}

#[test]
fn a_clip_on_a_generator_s_parameter_and_on_its_channel_s_volume_moves_them() {
    let dir = Dir::new("generator-automation");
    // All the way up for a beat, then half.
    let steps = [Point::new(0.0, 1.0), hold(1.0, 0.5), Point::new(3.0, 0.5)];
    // TestSine's level is parameter 0 of channel 1, and the clip covers
    // the whole of its 0 to 100.
    let mut levelled = sine_song(&[sine_note(0, BAR, 69)]);
    let clip = add_automation(&mut levelled, &steps, FULL);
    bind(&mut levelled, clip, (1 << 16) + PARAM);
    place(&mut levelled, clip, 0, BAR);
    let out = render(&levelled, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (1, 0));
    // The second beat is 64 samples before a 512th sample, where the
    // controls are read.
    for n in (1..24_064).step_by(7) {
        assert_eq!(out.left[n], sine_frame(69, n), "frame {n}");
    }
    for n in (24_064..96_000).step_by(7) {
        assert_eq!(out.left[n], sine_frame(69, n) * 0.5, "frame {n}");
    }

    // The channel's volume, from the 10000 of a new channel down to 5000,
    // and its pan from the centre to hard left.
    let steps = [
        Point::new(0.0, 0.781_25),
        hold(1.0, 0.390_625),
        Point::new(3.0, 0.390_625),
    ];
    let mut faded = sine_song(&[sine_note(0, BAR, 69)]);
    let clip = add_automation(&mut faded, &steps, FULL);
    bind(&mut faded, clip, 1 << 16);
    place(&mut faded, clip, 0, BAR);
    let sides = [Point::new(0.0, 0.5), hold(2.0, 0.0), Point::new(2.0, 0.0)];
    let clip = add_automation(&mut faded, &sides, FULL);
    bind(&mut faded, clip, (1 << 16) + 1);
    place(&mut faded, clip, 0, BAR);
    let out = render(&faded, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (2, 0));
    assert_eq!(out.left[20_000], sine_frame(69, 20_000));
    assert_eq!(out.left[30_000], sine_frame(69, 30_000) * 0.5);
    assert_eq!(out.left[..48_000], out.right[..48_000]);
    assert_eq!(out.left[60_000], sine_frame(69, 60_000) * 0.5);
    assert!(out.right[48_128..].iter().all(|v| *v == 0.0));

    // A parameter the generator does not have is not played, and nor is
    // any control of a plugin channel the engine does not host.
    let mut none = sine_song(&[sine_note(0, BAR, 69)]);
    let clip = add_automation(&mut none, &steps, FULL);
    bind(&mut none, clip, (1 << 16) + PARAM + 1);
    place(&mut none, clip, 0, BAR);
    let out = render(&none, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (0, 1));
    let mut unhosted = sine_song(&[sine_note(0, BAR, 69)]);
    unhosted.channels[1].plugin_name = Some("FLEX".to_owned());
    for target in [1 << 16, (1 << 16) + PARAM] {
        let clip = add_automation(&mut unhosted, &steps, FULL);
        bind(&mut unhosted, clip, target);
        place(&mut unhosted, clip, 0, BAR);
    }
    let out = render(&unhosted, &dir);
    assert_eq!((out.automation_clips, out.unsupported_targets), (0, 2));
}

#[test]
fn a_generator_counts_its_ticks_at_the_project_s_ppq() {
    // 3x Osc with a volume envelope whose attack is 0.83 seconds: 6 for
    // the version, the 21 knobs, the 5 flags, and the tab's 5 blocks and
    // its filter.
    let knobs = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, -12, 0, 0, 0, 0, 0, 0, -24, 0, 0, 0, 0,
    ];
    let block = [
        0, 0, 100, 20_000, 20_000, 30_000, 50, 20_000, 0, 100, 20_000, 0, 32_950, 0, 0, 0, 0,
    ];
    let mut state = params(&[&[14][..], &knobs].concat());
    state.extend([0; 8]);
    for at in 0..5 {
        let mut block = block;
        if at == 1 {
            (block[0], block[1], block[3], block[4]) = (4, -1, 25_000, 60_000);
        }
        state.extend(params(&block));
    }
    state.extend(params(&[0, 0, 256, 0, 0, 0]));

    // The same bar of one note, in ticks of a 96th and of a 48th of a
    // beat.
    let dir = Dir::new("generator-ppq");
    let song = |ppq: i32| {
        let bar = 4 * ppq;
        let items = [PlaylistItem::new(ItemSource::Pattern(1), 0, bar)];
        let mut song = project(&[(1, &[sine_note(0, bar, 60)])], &items);
        add_generator(&mut song, "3x Osc", &state);
        song.header.ppq = ppq as u16;
        render(&song, &dir)
    };
    let (fine, coarse) = (song(96), song(48));
    assert_eq!((fine.hosted_generators, coarse.hosted_generators), (1, 1));
    // The attack is as many seconds in either, 160 ticks of 250 frames
    // or 80 of 500: it is at half its level 20,000 frames in.
    let half = |out: &Rendered| {
        let peak = |chunk: &[f32]| chunk.iter().fold(0.0f32, |peak, v| peak.max(v.abs()));
        let peaks: Vec<f32> = out.left.chunks(500).map(peak).collect();
        let top = peaks.iter().fold(0.0f32, |top, peak| top.max(*peak));
        peaks.iter().position(|peak| *peak >= 0.5 * top).unwrap()
    };
    assert_eq!((half(&fine), half(&coarse)), (40, 40));
    // The ticks are not: a level holds for 500 frames and not for 250.
    assert!(fine.left != coarse.left);
}

#[test]
fn a_generator_the_engine_does_not_host_is_silent_and_counted() {
    let dir = Dir::new("generator-unhosted");
    let notes = [sine_note(0, BEAT, 69), sine_note(BEAT, BEAT, 69), note(0)];
    let mut song = sine_song(&notes);
    // No plugin of this name is hosted, so the channel is passed over for
    // its name and not for a state its plugin turns down.
    assert!(plugins::generator_by_name("FLEX").is_none());
    song.channels[1].plugin_name = Some("FLEX".to_owned());
    let out = render(&song, &dir);
    assert_impulses(&out.left, &[0]);
    assert_eq!((out.notes, out.voices, out.skipped), (3, 1, 1));
    assert_eq!(out.hosted_generators, 0);
    // The channel is counted once, however many notes it has.
    let unhosted: Vec<_> = out.unhosted_generators.iter().collect();
    assert_eq!(unhosted, [(&"FLEX".to_owned(), &1)]);

    // A hosted generator that turns the channel's state down is counted
    // with the others: TestSine's level stops at 100, and its state is 4
    // bytes.
    for state in [params(&[101]), params(&[100, 0])] {
        let mut broken = sine_song(&notes);
        broken.channels[1].plugin_state = Some(state);
        let out = render(&broken, &dir);
        assert_impulses(&out.left, &[0]);
        assert_eq!((out.hosted_generators, out.skipped), (0, 1));
        assert_eq!(out.unhosted_generators.get("TestSine"), Some(&1));
    }

    // A channel that is switched off is silent in FL too, and one without
    // a note has nothing to play. Neither is counted.
    let mut off = sine_song(&notes);
    off.channels[1].enabled = Some(false);
    off.channels[1].order.push(Placed {
        pos: 150,
        item: Item::Field(Id::IsEnabled),
    });
    let mut idle = sine_song(&[note(0)]);
    idle.channels[1].plugin_name = Some("FLEX".to_owned());
    for song in [off, idle] {
        let out = render(&song, &dir);
        assert_impulses(&out.left, &[0]);
        assert_eq!((out.hosted_generators, out.skipped), (0, 1));
        assert!(out.unhosted_generators.is_empty());
    }
}

/// The frames in a tick of the fixtures.
const TICK: usize = 250;

/// The 213 of Fruity Peak Controller: its ten parameters, Mute and Ramp.
fn controller(values: [i32; 10], mute: bool, ramp: bool) -> Vec<u8> {
    let mut state = params(&values);
    state.extend([u8::from(mute), u8::from(ramp)]);
    state
}

/// The Peak Controller's outputs, as FL numbers them.
const PEAK: u32 = 1;
const LFO: u32 = 2;

/// Add the 227 that links an output of the controller in a slot of an
/// insert to the control with this REC ID, and the 230 after it when the
/// link has a formula.
fn link(project: &mut Project, source: (u32, u32, u32), target: u32, formula: Option<&str>) {
    let (insert, slot, output) = source;
    let pos = 3000 + 2 * project.controller_links.len();
    let mut order = vec![Placed {
        pos,
        item: Item::Field(Id::ControllerRemote),
    }];
    if formula.is_some() {
        order.push(Placed {
            pos: pos + 1,
            item: Item::Field(Id::ControllerFormula),
        });
    }
    project.controller_links.push(ControllerLink {
        kind: LinkKind::Remote,
        link: Some(layout::ControllerLink {
            source: slot_control(insert, slot, PARAM + output),
            unknown_4: [0; 4],
            target,
            flags: 8,
            unknown_16: 469i32.to_le_bytes(),
        }),
        formula: formula.map(str::to_owned),
        order,
    });
}

/// What a render counts of controller links: the links from a hosted
/// controller, the ones that are played and the ones that are not.
fn linked(out: &Rendered) -> (usize, usize, usize) {
    (
        out.controller_links,
        out.controller_targets,
        out.unsupported_controller_targets,
    )
}

/// Fruity Peak Controller as its "Default" preset has it, but for a decay
/// of 100: the peak at unity, down from full scale in under a quarter of a
/// beat, and the LFO off.
const FOLLOWER: [i32; 10] = [0, 179, 0, 100, 0, 0, 0, 32_950, 0, 0];
/// A formula that leaves a fader at 0 dB, 0.8 of its range, in silence
/// and takes it all the way down at full scale.
const DUCK: &str = "0.8-Input*0.8";

/// [`kick_and_bare_tone`] with [`FOLLOWER`] on the kick's insert and its
/// Peak output linked to the volume of the tone's insert by [`DUCK`]: a
/// sidechain compressor made of a controller, which is what FL's users
/// build with it.
fn kick_ducks_tone(dir: &Dir) -> Project {
    let mut project = kick_and_bare_tone(dir);
    let follower = controller(FOLLOWER, false, false);
    add_effect(&mut project, 1, 0, "Fruity Peak Controller", &follower);
    link(
        &mut project,
        (1, 0, PEAK),
        insert_control(2, VOLUME),
        Some(DUCK),
    );
    project
}

#[test]
fn a_peak_controller_on_the_kick_ducks_the_tone_its_link_drives() {
    let dir = Dir::new("controller-duck");
    let plain = render(&kick_and_bare_tone(&dir), &dir);
    assert_eq!(linked(&plain), (0, 0, 0));
    let ducked = kick_ducks_tone(&dir);
    let out = render(&ducked, &dir);
    assert_eq!(linked(&out), (1, 1, 0));
    assert_eq!((out.hosted_slots, out.latency), (1, 0));
    assert!(out.approximated_slots.is_empty() && out.unhosted_slots.is_empty());
    assert_eq!(out.left.len(), plain.left.len());
    // The kick goes through the controller as it came.
    assert!(out.left == plain.left);

    for beat in 0..4 {
        let kick = KICK + beat * 24_000;
        // The controller hears the kick in the tick it starts in and says
        // so as the next one starts. Up to there the fader is at 0 dB, and
        // the tone is the tone.
        let before = kick.saturating_sub(16_000)..kick + TICK;
        assert!(out.right[before.clone()] == plain.right[before]);
        // The kick lasts into its ninth tick, and the hold is still full
        // as the tenth starts. So the fader is all the way down from the
        // second tick to the end of the tenth.
        let down = &out.right[kick + TICK..kick + 10 * TICK];
        assert!(down.iter().all(|v| *v == 0.0), "beat {beat}");
        assert!(
            plain.right[kick + TICK..kick + 10 * TICK]
                .iter()
                .any(|v| v.abs() > 0.4)
        );
        // Then the hold decays and the fader comes back up, a step a tick.
        // The decay is 4.22 a beat, so 23 ticks on the tone is back.
        let step = |tick: usize| {
            let from = kick + (10 + tick) * TICK;
            let peak = |side: &[f32]| {
                side[from..from + TICK]
                    .iter()
                    .fold(0f32, |m, v| m.max(v.abs()))
            };
            peak(&out.right) / peak(&plain.right)
        };
        let steps: Vec<f32> = (0..24).map(step).collect();
        assert!(steps[0] > 0.0 && steps[0] < 0.1, "beat {beat}: {steps:?}");
        assert!(steps.windows(2).all(|pair| pair[0] <= pair[1]), "{steps:?}");
        assert!(
            steps[21] < 1.0 && steps[22] == 1.0,
            "beat {beat}: {steps:?}"
        );
        let back = kick + 33 * TICK..(kick + 24_000).min(out.right.len());
        assert!(out.right[back.clone()] == plain.right[back]);
    }

    // The same peak on the pan of the tone's insert, which the approximate
    // curves take down in a straight line on the left as it goes right:
    // the left side of the tone is 1 less the peak of its right side. With
    // Mute on, the kick is out of the way. The hold drops by
    // (1001^(100 / 128) - 1) * 0.0192 / 96 a tick, and the peak is
    // 0.4 * (6^(179 / 256) - 1) of it and no more than 1.
    let mut panned = kick_and_bare_tone(&dir);
    let follower = controller(FOLLOWER, true, false);
    add_effect(&mut panned, 1, 0, "Fruity Peak Controller", &follower);
    let pan = insert_control(2, PAN);
    link(&mut panned, (1, 0, PEAK), pan, Some("0.5+Input*0.5"));
    let out = render(&panned, &dir);
    assert_eq!(linked(&out), (1, 1, 0));
    assert!(out.right == plain.right);
    let decay = (1001f64.powf(100.0 / 128.0) - 1.0) * 0.0192 / 96.0;
    let unity = 0.4 * (6f64.powf(179.0 / 256.0) - 1.0);
    // From the tick before the kick to where the hold is gone. The hold
    // is full up to the kick's tenth tick and decays from there.
    for tick in -1..36i32 {
        let from = (KICK as i32 + 24_000 + tick * TICK as i32) as usize;
        let at = (from..from + TICK)
            .max_by(|a, b| out.right[*a].abs().total_cmp(&out.right[*b].abs()))
            .unwrap();
        let got = f64::from(out.left[at] / out.right[at]);
        let hold = match tick {
            ..1 => 0.0,
            1..10 => 1.0,
            _ => (1.0 - f64::from(tick - 9) * decay).max(0.0),
        };
        let peak = (hold * unity).min(1.0);
        let want = 1.0 - (6400.0 * peak).round() / 6400.0;
        assert!(
            (got - want).abs() < 1e-4,
            "tick {tick}: {got}, wanted {want}"
        );
    }

    // With Mute on, the kick is gone from the mix and ducks all the same.
    let ducked_right = render(&ducked, &dir).right;
    let mut muted = ducked.clone();
    muted.inserts[1].slots[0].plugin_state = Some(controller(FOLLOWER, true, false));
    let silent = render(&muted, &dir);
    assert!(silent.left.iter().all(|v| *v == 0.0));
    assert!(silent.right == ducked_right);

    // Without the formula the fader follows the peak: shut in silence,
    // and all the way up, at +5.6 dB, under the kick.
    let mut gated = ducked.clone();
    gated.controller_links[0].formula = None;
    gated.controller_links[0].order.truncate(1);
    let out = render(&gated, &dir);
    assert_eq!(linked(&out), (1, 1, 0));
    assert!(out.right[..KICK + TICK].iter().all(|v| *v == 0.0));
    let loud = out.right[KICK + TICK..KICK + 10 * TICK].iter();
    let peak = loud.fold(0f32, |m, v| m.max(v.abs()));
    assert!((db(peak / 0.5) - 5.6).abs() < 0.1, "{peak}");
    assert!(
        out.right[KICK + 40 * TICK..KICK + 24_000]
            .iter()
            .all(|v| *v == 0.0)
    );

    // A slot that is switched off hears of no tick, and the fader stays
    // where the project has it.
    let mut off = ducked.clone();
    set_slot(&mut off, 1, 0, SLOT_ENABLED, 0);
    let out = render(&off, &dir);
    assert_eq!(linked(&out), (1, 1, 0));
    assert!(out.right == plain.right && out.left == plain.left);
}

#[test]
fn a_ducked_tone_comes_out_the_same_in_blocks_of_any_size() {
    let dir = Dir::new("controller-blocks");
    let project = kick_ducks_tone(&dir);
    let out = render(&project, &dir);
    let samples = engine::prepare(&project, &dir.0).unwrap();
    for block in [1, 64, 333, 512, 4096] {
        let mut engine = Engine::new(&project, &samples, RATE).unwrap();
        engine.play();
        let mut sink = NullSink::new(block);
        sink.pull(&mut engine, out.left.len());
        assert!(sink.left == out.left, "left in blocks of {block}");
        assert!(sink.right == out.right, "right in blocks of {block}");
    }
}

/// Fruity Peak Controller with the peak off and a square LFO of a beat at
/// full amount: 20643 on the speed wheel is 96 ticks.
const SQUARE: [i32; 10] = [0, 0, 0, 66, 0, 256, 0, 20_643, 2, 0];

/// [`tone`] for a bar, played twice, with [`SQUARE`] on insert 3, which
/// nothing plays into, and its LFO output linked to the volume of insert
/// 1, where the tone is: 0 dB for the first half of every beat and shut
/// for the second.
fn gated_tone() -> Project {
    let mut project = tone(BAR);
    let size = project.arrangements[0].items[0].len;
    let again = item(BAR, 1).shortened(size).unwrap();
    project.arrangements[0].items.push(again);
    let square = controller(SQUARE, false, false);
    add_effect(&mut project, 3, 0, "Fruity Peak Controller", &square);
    link(
        &mut project,
        (3, 0, LFO),
        insert_control(1, VOLUME),
        Some("Input*0.8"),
    );
    project
}

#[test]
fn a_controller_s_lfo_keeps_time_with_the_song_on_an_insert_no_sound_reaches() {
    let dir = Dir::new("controller-lfo");
    dir.tone(3 * 96_000);
    let gated = gated_tone();
    let out = render(&gated, &dir);
    assert_eq!(linked(&out), (1, 1, 0));
    // The controller's insert is mixed for the link's sake.
    assert_eq!(out.hosted_slots, 1);
    assert_eq!(out.left.len(), 192_000);
    assert_eq!(out.left, out.right);
    // The square is high for half of its 96 ticks. FL rounds the step of
    // the phase down, so the phase is a point of the table short of the
    // half at tick 48 and of the whole at tick 96: the first beat is high
    // for 49 ticks, and every later one a tick late for 48.
    for beat in 0..8 {
        let ticks = out.left[beat * 24_000..(beat + 1) * 24_000].chunks(TICK);
        let open: Vec<bool> = ticks
            .map(|tick| {
                assert!(tick.iter().all(|v| *v == tick[0]), "beat {beat}");
                tick[0] == 1.0
            })
            .collect();
        let late = usize::from(beat > 0);
        assert_eq!(
            open.iter().position(|open| *open),
            Some(late),
            "beat {beat}"
        );
        assert_eq!(open.iter().filter(|open| **open).count(), 49 - late);
        assert!(open[49..].iter().all(|open| !open), "beat {beat}");
    }
    assert!(out.left.iter().all(|v| *v == 0.0 || *v == 1.0));

    // Without the link the insert is left out, as any insert no sound
    // reaches, and the tone holds its level.
    let mut unlinked = gated.clone();
    unlinked.controller_links.clear();
    let plain = render(&unlinked, &dir);
    assert_eq!((linked(&plain), plain.hosted_slots), ((0, 0, 0), 0));
    assert!(plain.left.iter().all(|v| *v == 1.0));

    // The phase wheel moves the square by a part of its beat: a quarter
    // on, it is high for 25 ticks, low for 48 and high again.
    let mut ahead = gated.clone();
    let mut state = SQUARE;
    state[9] = 16_384;
    ahead.inserts[3].slots[0].plugin_state = Some(controller(state, false, false));
    let out = render(&ahead, &dir);
    assert_eq!((out.left[25 * TICK - 1], out.left[25 * TICK]), (1.0, 0.0));
    assert_eq!((out.left[73 * TICK - 1], out.left[73 * TICK]), (0.0, 1.0));

    // A song that starts over has the LFO start over with it: the second
    // pass is the first again.
    let samples = engine::prepare(&gated, &dir.0).unwrap();
    let mut engine = Engine::new(&gated, &samples, RATE).unwrap();
    engine.set_loop(true);
    engine.play();
    let mut sink = NullSink::new(777);
    sink.pull(&mut engine, 2 * 192_000);
    let out = render(&gated, &dir);
    assert!(sink.left[..192_000] == out.left[..]);
    assert!(sink.left[192_000..] == out.left[..]);
    // And one that is moved has it where the song is. The second bar
    // starts a tick short of a turn, which a square that started over
    // there would not: the bar comes out as the whole song has it.
    engine.set_loop(false);
    engine.seek(BAR as u64);
    let mut sink = NullSink::new(100);
    sink.pull(&mut engine, 96_000);
    assert!(sink.left[..] == out.left[96_000..]);
    assert_eq!((sink.left[0], sink.left[TICK]), (0.0, 1.0));
}

#[test]
fn a_link_the_engine_cannot_play_is_counted() {
    let dir = Dir::new("controller-counts");
    let mut project = kick_ducks_tone(&dir);
    // To the tempo, which the clock is built from before the song plays,
    // and with a formula the engine does not read.
    link(&mut project, (1, 0, PEAK), TEMPO, None);
    let volume = insert_control(3, VOLUME);
    link(
        &mut project,
        (1, 0, LFO),
        volume,
        Some("Tension(Input, 0.5)"),
    );
    // To a control of a plugin the engine does not host.
    add_effect(&mut project, 2, 1, "ZGameEditor Visualizer", &[]);
    link(
        &mut project,
        (1, 0, PEAK),
        slot_control(2, 1, PARAM + 0x21),
        None,
    );
    let out = render(&project, &dir);
    assert_eq!(linked(&out), (4, 1, 3));
    // None of them moves what the first link does.
    let ducked = render(&kick_ducks_tone(&dir), &dir);
    assert!(out.left == ducked.left && out.right == ducked.right);

    // A link is a controller's only when its source is an output of a
    // plugin the engine hosts: the Peak Controller has three, Fast Dist
    // has none, and an empty slot has no plugin.
    let mut none = kick_and_bare_tone(&dir);
    let follower = controller(FOLLOWER, false, false);
    add_effect(&mut none, 1, 0, "Fruity Peak Controller", &follower);
    add_effect(&mut none, 1, 1, "Fruity Fast Dist", &params(&FAST_DIST));
    let volume = insert_control(2, VOLUME);
    for source in [(1, 0, 3), (1, 1, PEAK), (1, 2, PEAK), (3, 0, PEAK)] {
        link(&mut none, source, volume, None);
    }
    assert_eq!(linked(&render(&none, &dir)), (0, 0, 0));
    // And a 226, a MIDI link, is no remote link whatever its source.
    let mut midi = kick_ducks_tone(&dir);
    midi.controller_links[0].kind = LinkKind::Midi;
    midi.controller_links[0].order[0].item = Item::Field(Id::ControllerMIDI);
    assert_eq!(linked(&render(&midi, &dir)), (0, 0, 0));
}

#[test]
fn a_controller_drives_a_parameter_of_a_hosted_plugin() {
    let dir = Dir::new("controller-param");
    // The peak of the kick on PanOMatic's volume on the tone's insert,
    // upside down: 256 of its 320 in silence, which is unity, and 0 at
    // full scale.
    let mut project = kick_and_bare_tone(&dir);
    let follower = controller(FOLLOWER, false, false);
    add_effect(&mut project, 1, 0, "Fruity Peak Controller", &follower);
    let still = params(&[128, 256, 0, 0, 64, 27_500]);
    add_effect(&mut project, 2, 0, "Fruity PanOMatic", &still);
    let volume = slot_control(2, 0, PARAM + 1);
    link(&mut project, (1, 0, PEAK), volume, Some(DUCK));
    let out = render(&project, &dir);
    assert_eq!((linked(&out), out.hosted_slots), ((1, 1, 0), 2));
    let plain = render(&kick_and_bare_tone(&dir), &dir);
    assert!(out.left == plain.left);
    assert!(out.right[..KICK + TICK] == plain.right[..KICK + TICK]);
    // PanOMatic ramps to where its volume is put over the block, which is
    // the tick, so the tone is gone a tick after the fader went down.
    let down = &out.right[KICK + 2 * TICK..KICK + 10 * TICK];
    assert!(down.iter().all(|v| *v == 0.0));
    assert!(out.right[KICK + 34 * TICK..24_000] == plain.right[KICK + 34 * TICK..24_000]);
}

#[test]
fn panomatic_pans_and_scales_what_plays_into_its_insert() {
    let dir = Dir::new("panomatic");
    // As FL sets a new one it changes no bit.
    let mut unity = one_click();
    let still = |pan: i32, volume: i32| params(&[pan, volume, 0, 0, 64, 27_500]);
    add_effect(&mut unity, 1, 0, "Fruity PanOMatic", &still(128, 256));
    let out = render(&unity, &dir);
    assert!(
        out == Rendered {
            hosted_slots: 1,
            ..render(&one_click(), &dir)
        }
    );
    // Hard left folds both sides into the left at half gain each, so the
    // click, which is on both, keeps its level there.
    let mut left = one_click();
    add_effect(&mut left, 1, 0, "Fruity PanOMatic", &still(0, 256));
    let out = render(&left, &dir);
    assert_eq!(out.left[0], 1.0);
    assert!(out.right.iter().all(|v| *v == 0.0));
    // The volume wheel at 128 is 12.7 dB down.
    let mut quiet = one_click();
    add_effect(&mut quiet, 1, 0, "Fruity PanOMatic", &still(128, 128));
    let out = render(&quiet, &dir);
    assert!((db(out.left[0]) + 12.7).abs() < 0.05 && out.left[0] == out.right[0]);
    assert!(out.approximated_slots.is_empty());

    // With its LFO on, the plugin says its rate is FL's formula as read,
    // which the render's 48 kHz puts off the wheel's.
    let mut moving = one_click();
    let auto_pan = params(&[128, 256, 1, 1, 87, 45_614]);
    add_effect(&mut moving, 1, 0, "Fruity PanOMatic", &auto_pan);
    let out = render(&moving, &dir);
    let said: Vec<_> = out.approximated_slots.iter().collect();
    assert_eq!(said, [(&"PanOMatic LFO rate at non-44.1k", &1)]);
}
