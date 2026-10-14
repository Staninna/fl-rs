//! The app without its window: commands in, files and sound out.
//!
//! The first tests run on a project built in code. The corpus tests want
//! `FL_RS_CORPUS` to name FL Studio's own `Data/` directory, which stays
//! out of this repo, and pass with a line on stderr when it is unset. The
//! tests that play skip themselves when there is no output device.

use fl_rs::App;
use flp::event::Id;
use flp::event::layout::{InsertFlags, Levels};
use model::{
    Arrangement, Channel, ChannelKind, Insert, Item, ItemSource, Note, Pattern, Placed,
    PlaylistItem, Project, RecTarget,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ui::{Command, Edit};

/// A directory under the system's temp directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("fl-rs-app-{}-{name}", std::process::id()));
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

/// Poll until the load or the render that is running has landed.
fn settle(app: &mut App) {
    let started = Instant::now();
    loop {
        app.poll();
        if !app.is_busy() {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(300), "still busy");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// One bar at 120 BPM and PPQ 96 as FL 20.8 would write it: a sampler
/// channel that plays `click.wav` on every beat, into the master.
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
        channel_count: 1,
        ppq: 96,
    });
    project.writer_version = Some("20.8.4.2576".to_owned());
    project.tempo_milli = Some(120_000);
    project.title = Some("One bar".to_owned());
    project.order = vec![field(Id::FLVersion), field(Id::Tempo), field(Id::Title)];
    let note = |beat: i32| Note {
        position: beat * 96,
        key: 60,
        ..Note::default()
    };
    project.patterns.push(Pattern {
        index: 1,
        notes: (0..4).map(note).collect(),
        order: vec![field(Id::PatternNew), field(Id::PatternNotes)],
        ..Pattern::default()
    });
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
    let item = PlaylistItem::new(ItemSource::Pattern(1), 0, 384);
    project.arrangements.push(Arrangement {
        index: 0,
        items: vec![item.shortened(32).unwrap()],
        order: vec![field(Id::ArrangementNew), field(Id::ArrangementPlaylist)],
        ..Arrangement::default()
    });
    project
}

/// Write the fixture and its sample into `dir`: `song.flp`, and a mono
/// `click.wav` of 64 frames whose first frame is 1.0.
fn write_fixture(dir: &Path) -> PathBuf {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(dir.join("click.wav"), spec).unwrap();
    for frame in 0..64 {
        writer
            .write_sample(if frame == 0 { 1f32 } else { 0.0 })
            .unwrap();
    }
    writer.finalize().unwrap();
    let path = dir.join("song.flp");
    fl_rs::save(&fixture(), &path).unwrap();
    path
}

#[test]
fn open_loads_on_a_worker_and_save_as_writes_the_same_bytes() {
    let dir = Dir::new("save");
    let path = write_fixture(&dir.0);
    let mut app = App::new(None);
    assert!(app.path().is_none());

    app.apply(Command::Open(path.clone()));
    // The file is read on another thread, so nothing is open yet.
    assert!(app.is_busy());
    assert!(app.path().is_none());
    assert_eq!(app.state().busy.as_deref(), Some("Opening song.flp"));
    settle(&mut app);
    assert_eq!(app.path(), Some(path.as_path()));
    assert_eq!(app.state().project.title.as_deref(), Some("One bar"));
    assert_eq!(app.project().patterns[0].notes.len(), 4);
    // With no device the project is open all the same, and says so.
    match app.error() {
        None => assert!(app.has_audio()),
        Some(error) => assert!(
            !app.has_audio() && error.starts_with("no sound: "),
            "{error}"
        ),
    }

    let copy = dir.0.join("copy.flp");
    app.apply(Command::SaveAs(copy.clone()));
    assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&path).unwrap());
    assert_eq!(app.path(), Some(copy.as_path()));
}

#[test]
fn render_writes_the_song_to_a_wav() {
    let dir = Dir::new("render");
    let path = write_fixture(&dir.0);
    let mut app = App::new(None);
    app.apply(Command::Open(path));
    settle(&mut app);
    let rate = app.state().sample_rate.unwrap_or(44_100);

    let wav = dir.0.join("song.wav");
    assert_eq!(app.state().progress, None);
    app.apply(Command::Render(wav.clone()));
    assert_eq!(app.state().busy.as_deref(), Some("Rendering song.wav"));
    // A render says how far it is, and a load does not.
    assert!(app.state().progress.is_some_and(|done| done <= 1.0));
    // A second render waits for the first.
    app.apply(Command::Render(dir.0.join("other.wav")));
    assert_eq!(app.error(), Some("a render is running already"));
    settle(&mut app);

    assert_eq!(app.state().progress, None);

    let mut reader = hound::WavReader::open(&wav).unwrap();
    let spec = reader.spec();
    assert_eq!((spec.channels, spec.sample_rate), (2, rate));
    let samples: Vec<f32> = reader.samples().map(Result::unwrap).collect();
    // A click on each side at the start of the bar, and two seconds of
    // song at most.
    assert!(samples[0] > 0.0 && samples[1] > 0.0, "{:?}", &samples[..2]);
    assert!(samples.len() <= 2 * 2 * rate as usize, "{}", samples.len());
    assert!(!dir.0.join("other.wav").exists());
}

#[test]
fn a_render_reports_its_progress_up_to_the_whole() {
    let dir = Dir::new("progress");
    let path = write_fixture(&dir.0);
    let project = fl_rs::read(&path).unwrap();
    let mut seen = Vec::new();
    let wav = dir.0.join("song.wav");
    let mut progress = |done: f32| seen.push(done);
    fl_rs::render_with_progress(&project, &dir.0, 48_000, &wav, &mut progress).unwrap();
    // Two seconds of song are four half seconds at least.
    assert!(seen.len() >= 4, "{seen:?}");
    assert!(seen.is_sorted() && seen[0] > 0.0, "{seen:?}");
    assert_eq!(seen.last(), Some(&1.0));
    assert!(wav.exists());
}

#[test]
fn a_sample_that_does_not_decode_names_its_channel() {
    let dir = Dir::new("silent");
    let path = write_fixture(&dir.0);
    std::fs::write(dir.0.join("click.wav"), b"RIFF and then nothing a WAV has").unwrap();
    let loaded = fl_rs::load(&path, &dir.0).unwrap();
    // The fixture's channel has no name.
    assert_eq!(
        loaded.notice.as_deref(),
        Some("silent, the sample does not decode: Channel 0")
    );

    let mut named = fixture();
    named.channels[0].name = Some("Click".to_owned());
    named.channels[0].order.push(Placed {
        pos: 100,
        item: Item::Field(Id::PluginName),
    });
    fl_rs::save(&named, &path).unwrap();
    let mut app = App::new(None);
    app.apply(Command::Open(path));
    settle(&mut app);
    let error = app.state().last_error.unwrap();
    assert!(
        error.starts_with("silent, the sample does not decode: Click"),
        "{error}"
    );
}

#[test]
fn what_goes_wrong_is_kept_for_the_window() {
    let dir = Dir::new("errors");
    let mut app = App::new(None);
    for command in [
        Command::Play,
        Command::Save,
        Command::SaveAs(dir.0.join("nothing.flp")),
        Command::Render(dir.0.join("nothing.wav")),
    ] {
        app.apply(command);
        assert_eq!(app.error(), Some("no project is open"));
    }
    assert!(!dir.0.join("nothing.flp").exists());

    let missing = dir.0.join("missing.flp");
    app.apply(Command::Open(missing));
    assert_eq!(app.error(), None);
    settle(&mut app);
    let error = app.state().last_error.unwrap();
    assert!(error.starts_with("cannot read "), "{error}");

    let junk = dir.0.join("junk.flp");
    std::fs::write(&junk, b"not a project").unwrap();
    app.apply(Command::Open(junk));
    settle(&mut app);
    let error = app.state().last_error.unwrap();
    assert!(error.contains("is not a project FL wrote"), "{error}");
    assert!(app.path().is_none());

    assert!(!app.quit_requested());
    app.apply(Command::Quit);
    assert!(app.quit_requested());
}

#[test]
fn play_moves_the_clock_and_stop_takes_it_back() {
    let dir = Dir::new("play");
    let path = write_fixture(&dir.0);
    let mut app = App::new(None);
    app.apply(Command::Open(path));
    // Play during the load starts the song when it lands.
    app.apply(Command::Play);
    settle(&mut app);
    if !app.has_audio() {
        eprintln!("no audio stream, play test skipped: {:?}", app.error());
        return;
    }
    assert!(app.is_playing());
    std::thread::sleep(Duration::from_millis(300));
    assert!(app.position() > 0, "the clock stood still");
    assert!(app.state().playing);

    // The bar is 384 ticks long, and the clock stops at its end.
    app.apply(Command::Seek(192));
    std::thread::sleep(Duration::from_millis(100));
    assert!(app.position() >= 192, "tick {}", app.position());

    app.apply(Command::Stop);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(app.position(), 0);
    assert!(!app.is_playing());
    assert_eq!(app.error(), None);
}

#[test]
fn the_song_stops_at_its_end_unless_it_loops_or_a_pattern_plays() {
    let dir = Dir::new("end");
    let (mut app, _) = open_fixture(&dir);
    if !app.has_audio() {
        eprintln!("no audio stream, end test skipped: {:?}", app.error());
        return;
    }
    // The bar is 384 ticks and two seconds long. From tick 360 the end is
    // an eighth of a second away.
    let play_to_the_end = |app: &mut App| {
        app.apply(Command::Play);
        app.apply(Command::Seek(360));
        assert!(app.state().playing);
        std::thread::sleep(Duration::from_millis(400));
    };
    play_to_the_end(&mut app);
    assert!(!app.is_playing() && !app.state().playing);
    assert_eq!(app.position(), 0);

    app.apply(Command::ToggleLoop);
    assert!(app.state().looping);
    play_to_the_end(&mut app);
    assert!(app.is_playing());
    assert!(app.position() < 360, "tick {}", app.position());
    app.apply(Command::Stop);
    app.apply(Command::ToggleLoop);

    // Pattern mode plays the pattern round and round.
    app.apply(Command::ToggleSongMode);
    app.apply(Command::SelectPattern(1));
    let state = app.state();
    assert!(!state.song_mode && !state.looping && state.pattern == 1);
    play_to_the_end(&mut app);
    assert!(app.is_playing());
    app.apply(Command::Stop);
    assert_eq!(app.error(), None);
}

fn moved(id: usize, delta_ticks: i64, delta_tracks: i32) -> Edit {
    Edit::MoveItems {
        ids: vec![id],
        delta_ticks,
        delta_tracks,
    }
}

fn edit(app: &mut App, edit: Edit, drag_id: Option<u64>) {
    app.apply(Command::Edit { edit, drag_id });
}

/// The fixture, open in an app.
fn open_fixture(dir: &Dir) -> (App, PathBuf) {
    let path = write_fixture(&dir.0);
    let mut app = App::new(None);
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    (app, path)
}

/// Where each item of the playlist is: its position and its track.
fn places(project: &Project) -> Vec<(i32, i32)> {
    let version = project.version();
    let items = &project.current_arrangement().unwrap().items;
    let place = |item: &PlaylistItem| (item.position, item.track(version));
    items.iter().map(place).collect()
}

#[test]
fn an_edit_is_saved_reloaded_and_undone() {
    let dir = Dir::new("edit");
    let (mut app, path) = open_fixture(&dir);
    let original = std::fs::read(&path).unwrap();
    assert!(!app.is_modified());

    edit(&mut app, moved(0, 96, 2), None);
    assert!(app.is_modified());
    assert_eq!(places(app.project()), [(96, 3)]);
    // What the window draws is the edited project.
    assert_eq!(places(&app.state().project), [(96, 3)]);
    if app.has_audio() {
        assert_eq!(app.error(), None);
    }

    // Save writes to the file the project came from.
    app.apply(Command::Save);
    assert_eq!(app.error(), None);
    assert!(!app.is_modified());
    assert_ne!(std::fs::read(&path).unwrap(), original);
    assert_eq!(places(&fl_rs::read(&path).unwrap()), [(96, 3)]);

    // The file has the edit now, so the project without it is the changed
    // one.
    app.apply(Command::Undo);
    assert!(app.is_modified());
    assert_eq!(places(app.project()), [(0, 1)]);
    app.apply(Command::Redo);
    assert!(!app.is_modified());
    app.apply(Command::Undo);
    app.apply(Command::Save);
    assert_eq!(std::fs::read(&path).unwrap(), original);

    // An edit that changes nothing is no step, so what was undone is still
    // there to redo.
    edit(&mut app, moved(5, 96, 0), None);
    edit(&mut app, Edit::DeleteItems { ids: Vec::new() }, None);
    assert!(!app.is_modified());
    app.apply(Command::Redo);
    assert_eq!(places(app.project()), [(96, 3)]);
}

#[test]
fn a_drag_is_one_undo_step() {
    let dir = Dir::new("drag");
    let (mut app, _) = open_fixture(&dir);

    // Each edit of a drag says how far the drag is from where it began.
    for ticks in [96, 192, 288] {
        edit(&mut app, moved(0, ticks, 1), Some(1));
    }
    assert_eq!(places(app.project()), [(288, 2)]);
    // The next drag starts from where the first one left the item.
    edit(&mut app, moved(0, 96, 0), Some(2));
    edit(&mut app, moved(0, -96, 0), Some(2));
    assert_eq!(places(app.project()), [(192, 2)]);

    app.apply(Command::Undo);
    assert_eq!(places(app.project()), [(288, 2)]);
    app.apply(Command::Undo);
    assert_eq!(places(app.project()), [(0, 1)]);
    assert!(!app.is_modified());
    // There is nothing more to undo.
    app.apply(Command::Undo);
    assert_eq!(places(app.project()), [(0, 1)]);
    app.apply(Command::Redo);
    app.apply(Command::Redo);
    assert_eq!(places(app.project()), [(192, 2)]);

    // An edit after an undo drops what was undone.
    app.apply(Command::Undo);
    edit(&mut app, moved(0, 0, 3), None);
    app.apply(Command::Redo);
    assert_eq!(places(app.project()), [(288, 5)]);
}

#[test]
fn a_drag_with_ctrl_copies_and_moves_in_one_step() {
    let dir = Dir::new("copy");
    let (mut app, _) = open_fixture(&dir);

    // The copy is item 1, and the drag goes on with it.
    edit(&mut app, Edit::DuplicateItems { ids: vec![0] }, Some(1));
    for ticks in [96, 384] {
        edit(&mut app, moved(1, ticks, 1), Some(1));
    }
    assert_eq!(places(app.project()), [(0, 1), (384, 2)]);
    app.apply(Command::Undo);
    assert_eq!(places(app.project()), [(0, 1)]);
    app.apply(Command::Redo);

    let items = |app: &App| app.project().current_arrangement().unwrap().items.clone();
    edit(&mut app, Edit::DeleteItems { ids: vec![0] }, None);
    assert!(items(&app)[0].is_deleted());
    // A deleted item keeps its record, so the copy is still item 1.
    let ids = vec![1];
    edit(&mut app, Edit::SetMuted { ids, muted: true }, None);
    assert!(items(&app)[1].muted());
    app.apply(Command::Undo);
    app.apply(Command::Undo);
    assert_eq!(items(&app)[0].source(), Some(ItemSource::Pattern(1)));
    assert!(!items(&app)[1].muted());
}

#[test]
fn a_selection_is_no_change_to_save_or_undo() {
    let dir = Dir::new("select");
    let (mut app, _) = open_fixture(&dir);
    let select = |ids: Vec<usize>| Edit::SelectItems { ids, add: false };
    let selected = |app: &App| app.project().current_arrangement().unwrap().items[0].selected();

    edit(&mut app, select(vec![0]), None);
    assert!(selected(&app));
    assert!(!app.is_modified());
    app.apply(Command::Undo);
    assert!(selected(&app));

    // The move is the step, and undoing it leaves the selection.
    edit(&mut app, moved(0, 96, 0), None);
    edit(&mut app, select(Vec::new()), None);
    assert!(app.is_modified() && !selected(&app));
    app.apply(Command::Undo);
    assert!(!app.is_modified() && selected(&app));
    assert_eq!(places(app.project()), [(0, 1)]);
}

/// The steps of pattern 1 that are lit for channel 0.
fn lit(project: &Project) -> Vec<usize> {
    let Some(pattern) = project.pattern(1) else {
        return Vec::new();
    };
    let steps = pattern.steps(0, project.header.ppq).into_iter();
    let lit = steps
        .enumerate()
        .filter_map(|(step, lit)| lit.then_some(step));
    lit.collect()
}

fn set_step(step: u32, on: bool) -> Edit {
    Edit::SetStep {
        channel: 0,
        step,
        on,
    }
}

#[test]
fn a_step_is_saved_reloaded_and_undone() {
    let dir = Dir::new("step");
    let (mut app, path) = open_fixture(&dir);
    let original = std::fs::read(&path).unwrap();
    // The fixture has a note on every beat.
    assert_eq!(lit(app.project()), [0, 4, 8, 12]);

    edit(&mut app, set_step(2, true), None);
    assert!(app.is_modified());
    assert_eq!(lit(&app.state().project), [0, 2, 4, 8, 12]);
    if app.has_audio() {
        // The stream took the project with the step in it.
        assert_eq!(app.error(), None);
    }
    app.apply(Command::Save);
    assert_eq!(app.error(), None);
    let saved = fl_rs::read(&path).unwrap();
    assert_eq!(lit(&saved), [0, 2, 4, 8, 12]);
    // The step is FL's: a note without a length at the step's first tick.
    let note = saved.patterns[0].notes[1];
    assert_eq!((note.position, note.length, note.key), (48, 0, 60));

    edit(&mut app, set_step(4, false), None);
    assert_eq!(lit(app.project()), [0, 2, 8, 12]);
    app.apply(Command::Undo);
    app.apply(Command::Undo);
    app.apply(Command::Save);
    assert_eq!(std::fs::read(&path).unwrap(), original);

    // A step that is lit already, a step of a channel that is not there
    // and a step to clear in a pattern that is not there change nothing.
    edit(&mut app, set_step(0, true), None);
    let other = Edit::SetStep {
        channel: 7,
        step: 1,
        on: true,
    };
    edit(&mut app, other, None);
    app.apply(Command::SelectPattern(5));
    edit(&mut app, set_step(0, false), None);
    assert!(!app.is_modified());
    // A step in a pattern that is not there makes the pattern.
    edit(&mut app, set_step(3, true), None);
    assert!(app.is_modified());
    let project = app.project();
    assert_eq!(project.pattern(5).unwrap().steps(0, 96)[3..], [true]);
    assert_eq!(lit(project), [0, 4, 8, 12]);
}

#[test]
fn a_drag_across_steps_or_on_a_knob_is_one_undo_step() {
    let dir = Dir::new("paint");
    let (mut app, _) = open_fixture(&dir);

    // A drag across steps sends an edit for each, and each one stays.
    for step in [1, 2, 3] {
        edit(&mut app, set_step(step, true), Some(1));
    }
    assert_eq!(lit(app.project()), [0, 1, 2, 3, 4, 8, 12]);
    // A knob sends a value for each turn, and the last one counts.
    for volume in [9000, 8000, 7000] {
        let edit = Edit::SetChannelVolume { channel: 0, volume };
        self::edit(&mut app, edit, Some(2));
    }
    let channel = |app: &App| app.project().channel(0).unwrap().clone();
    assert_eq!(channel(&app).volume(), Some(7000));
    // The velocity lane sends a velocity for each step it crosses.
    for (step, velocity) in [(1, 30), (2, 60), (2, 61), (5, 90)] {
        let edit = Edit::SetStepVelocity {
            channel: 0,
            step,
            velocity,
        };
        self::edit(&mut app, edit, Some(3));
    }
    let velocities = |app: &App| -> Vec<u8> {
        let notes = &app.project().pattern(1).unwrap().notes;
        notes.iter().map(|note| note.velocity).collect()
    };
    assert_eq!(velocities(&app), [100, 30, 61, 100, 100, 100, 100]);

    app.apply(Command::Undo);
    assert_eq!(velocities(&app), [100; 7]);
    app.apply(Command::Undo);
    assert_eq!(channel(&app).volume(), Some(10_000));
    app.apply(Command::Undo);
    assert_eq!(lit(app.project()), [0, 4, 8, 12]);
    assert!(!app.is_modified());

    // The switch, the pan and the insert.
    for edit in [
        Edit::SetChannelEnabled {
            channel: 0,
            enabled: false,
        },
        Edit::SetChannelPan {
            channel: 0,
            pan: 3200,
        },
        Edit::SetChannelInsert {
            channel: 0,
            insert: 4,
        },
    ] {
        self::edit(&mut app, edit, None);
    }
    let edited = channel(&app);
    assert_eq!(edited.enabled, Some(false));
    assert_eq!((edited.pan(), edited.insert()), (Some(3200), Some(4)));
    // The fixture's channel has no 0, so the file gets one.
    let path = dir.0.join("edited.flp");
    app.apply(Command::SaveAs(path.clone()));
    assert_eq!(app.error(), None);
    let saved = fl_rs::read(&path).unwrap();
    assert_eq!(saved.channel(0).unwrap().enabled, Some(false));
    assert_eq!(saved.channel(0).unwrap().insert(), Some(4));
    for _ in 0..3 {
        app.apply(Command::Undo);
    }
    assert_eq!(channel(&app).enabled, None);
}

fn add_note(position: u64, length: u64, key: u8) -> Edit {
    Edit::AddNote {
        channel: 0,
        position,
        length,
        key,
    }
}

/// The notes of pattern 1, deleted records and all: position, length and
/// key.
fn notes(project: &Project) -> Vec<(i32, i32, i16)> {
    let notes = project.pattern(1).map_or(&[][..], |p| &p.notes);
    let place = |note: &Note| (note.position, note.length, note.key);
    notes.iter().map(place).collect()
}

#[test]
fn a_note_is_saved_reloaded_and_undone() {
    let dir = Dir::new("note");
    let (mut app, path) = open_fixture(&dir);
    let original = std::fs::read(&path).unwrap();
    let steps = [(0, 0, 60), (96, 0, 60), (192, 0, 60), (288, 0, 60)];
    assert_eq!(notes(app.project()), steps);

    edit(&mut app, add_note(120, 48, 67), None);
    assert!(app.is_modified());
    let mut with_note = steps.to_vec();
    with_note.insert(2, (120, 48, 67));
    assert_eq!(notes(&app.state().project), with_note);
    if app.has_audio() {
        // The stream took the project with the note in it.
        assert_eq!(app.error(), None);
    }
    app.apply(Command::Save);
    assert_eq!(app.error(), None);
    let saved = fl_rs::read(&path).unwrap();
    assert_eq!(notes(&saved), with_note);
    // The note is FL's new note but for where it is and how long.
    let new = Note {
        position: 120,
        length: 48,
        key: 67,
        ..Note::default()
    };
    assert_eq!(saved.patterns[0].notes[2], new);

    app.apply(Command::Undo);
    assert_eq!(notes(app.project()), steps);
    app.apply(Command::Save);
    assert_eq!(std::fs::read(&path).unwrap(), original);

    // A note for a channel that is not there, a note further along than
    // a position goes, and an edit of a note in a pattern that is not
    // there change nothing.
    let other = Edit::AddNote {
        channel: 7,
        position: 0,
        length: 24,
        key: 60,
    };
    edit(&mut app, other, None);
    edit(&mut app, add_note(u64::MAX, 24, 60), None);
    app.apply(Command::SelectPattern(5));
    edit(&mut app, Edit::DeleteNotes { ids: vec![0] }, None);
    let (ids, add) = (vec![0], false);
    edit(&mut app, Edit::SelectNotes { ids, add }, None);
    assert!(!app.is_modified());
    assert!(app.project().pattern(5).is_none());
    // A note in a pattern that is not there makes the pattern.
    edit(&mut app, add_note(24, 24, 72), None);
    assert!(app.is_modified());
    let made = &app.project().pattern(5).unwrap().notes;
    assert_eq!((made.len(), made[0].position, made[0].key), (1, 24, 72));
    assert_eq!(notes(app.project()), steps);
}

#[test]
fn the_piano_roll_s_drags_are_one_undo_step_each() {
    let dir = Dir::new("notes");
    let (mut app, _) = open_fixture(&dir);
    let note = |app: &App, id: usize| app.project().pattern(1).unwrap().notes[id];

    // A drag that draws says the note again for each length, and it is
    // one note.
    for length in [24, 48, 96] {
        edit(&mut app, add_note(120, length, 67), Some(1));
    }
    assert_eq!(notes(app.project()).len(), 5);
    assert_eq!(notes(app.project())[2], (120, 96, 67));

    // Each edit of a move says how far the drag is from where it began.
    // The note goes past the last step, and to the end of the list.
    for ticks in [24, 96, 200] {
        let moved = Edit::MoveNotes {
            ids: vec![2],
            delta_ticks: ticks,
            delta_keys: -7,
        };
        edit(&mut app, moved, Some(2));
    }
    assert_eq!(notes(app.project())[3..], [(288, 0, 60), (320, 96, 60)]);
    for ticks in [-48, -90] {
        let resized = Edit::ResizeNotes {
            ids: vec![4],
            delta_ticks: ticks,
        };
        edit(&mut app, resized, Some(3));
    }
    assert_eq!(note(&app, 4).length, 6);

    // A drag across the lane sets a value for each note, and each stays.
    let field = ui::NoteField::Velocity;
    for (id, value) in [(4, 90), (0, 30), (0, 200)] {
        edit(&mut app, Edit::SetNoteField { id, field, value }, Some(4));
    }
    assert_eq!((note(&app, 4).velocity, note(&app, 0).velocity), (90, 128));

    let (ids, muted) = (vec![0, 4], true);
    edit(&mut app, Edit::SetNotesMuted { ids, muted }, None);
    assert!(note(&app, 0).muted() && note(&app, 4).muted());
    // A deleted note keeps its record, so the others keep their places.
    edit(&mut app, Edit::DeleteNotes { ids: vec![1] }, None);
    assert!(note(&app, 1).is_deleted());
    assert_eq!(notes(app.project()).len(), 5);
    assert_eq!(lit(app.project()), [8, 12]);

    // A selection is no step.
    let (ids, add) = (vec![4], false);
    edit(&mut app, Edit::SelectNotes { ids, add }, None);
    assert!(note(&app, 4).selected());
    app.apply(Command::Undo);
    assert!(!note(&app, 1).is_deleted() && !note(&app, 4).selected());
    app.apply(Command::Undo);
    assert!(!note(&app, 0).muted());
    app.apply(Command::Undo);
    assert_eq!((note(&app, 4).velocity, note(&app, 0).velocity), (100, 100));
    app.apply(Command::Undo);
    assert_eq!(note(&app, 4).length, 96);
    app.apply(Command::Undo);
    assert_eq!(notes(app.project())[2], (120, 96, 67));
    assert!(app.is_modified());
    app.apply(Command::Undo);
    assert!(!app.is_modified());
    assert_eq!(notes(app.project()).len(), 4);
}

#[test]
fn a_deleted_note_keeps_its_place_until_the_save_leaves_it_out() {
    let dir = Dir::new("compact");
    let (mut app, path) = open_fixture(&dir);
    let original = std::fs::read(&path).unwrap();

    edit(&mut app, Edit::DeleteNotes { ids: vec![1] }, None);
    // The record is there, so the note behind it is still note 2.
    assert_eq!(notes(app.project()).len(), 4);
    let (ids, muted) = (vec![2], true);
    edit(&mut app, Edit::SetNotesMuted { ids, muted }, None);
    assert!(app.project().patterns[0].notes[2].muted());
    assert_eq!(lit(app.project()), [0, 12]);

    app.apply(Command::Save);
    assert_eq!(app.error(), None);
    // The file has three notes, and the open project its four records.
    let saved = fl_rs::read(&path).unwrap();
    assert_eq!(notes(&saved), [(0, 0, 60), (192, 0, 60), (288, 0, 60)]);
    assert!(saved.patterns[0].notes[1].muted());
    assert_eq!(notes(app.project()).len(), 4);
    assert_eq!(std::fs::read(&path).unwrap().len(), original.len() - 24);

    app.apply(Command::Undo);
    app.apply(Command::Undo);
    app.apply(Command::Save);
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn a_sample_s_path_is_stored_the_way_fl_stores_its_own() {
    let stored =
        |file: &str, root: Option<&str>| fl_rs::stored_path(Path::new(file), root.map(Path::new));
    // The root is FL's `Data` directory, or the directory that is in.
    assert_eq!(
        stored("/fl/Data/Patches/Packs/Kick.wav", Some("/fl/Data")),
        r"%FLStudioFactoryData%\Data\Patches\Packs\Kick.wav"
    );
    assert_eq!(
        stored("/fl/Data/Patches/Packs/Kick.wav", Some("/fl")),
        r"%FLStudioFactoryData%\Data\Patches\Packs\Kick.wav"
    );
    // A file that is not under the root keeps its own path.
    assert_eq!(
        stored("/home/me/Kick.wav", Some("/fl/Data")),
        "/home/me/Kick.wav"
    );
    assert_eq!(stored("/home/me/Kick.wav", None), "/home/me/Kick.wav");
    let relative = stored("Kick.wav", None);
    assert!(Path::new(&relative).is_absolute(), "{relative}");
}

/// What the engine plays of a project in its first second, at 48 kHz: the
/// loudest sample, and how many notes played one.
fn rendered(project: &Project, sample_root: &Path) -> (f32, usize) {
    let options = engine::RenderOptions {
        sample_rate: 48_000,
        sample_root: sample_root.to_path_buf(),
        max_seconds: 1.0,
        plugin_dir: None,
    };
    let out = engine::render(project, &options).unwrap();
    let peak = out.left.iter().chain(&out.right);
    (peak.fold(0f32, |peak, v| peak.max(v.abs())), out.voices)
}

#[test]
fn a_sample_goes_to_a_channel_or_to_a_new_one_and_plays_after_a_reload() {
    let dir = Dir::new("sample");
    let path = write_fixture(&dir.0);
    let original = std::fs::read(&path).unwrap();
    // A second click, half as loud, to tell from the first.
    let quiet = dir.0.join("Quiet click.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&quiet, spec).unwrap();
    for _ in 0..64 {
        writer.write_sample(i16::MAX / 2).unwrap();
    }
    writer.finalize().unwrap();

    // The folder is the data root, so the paths are stored from there.
    let mut app = App::new(Some(dir.0.clone()));
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    let (loud, _) = rendered(app.project(), &dir.0);

    let set = Edit::SetChannelSample {
        channel: 0,
        path: quiet.clone(),
    };
    edit(&mut app, set, None);
    let stored = r"%FLStudioFactoryData%\Quiet click.wav";
    let sample =
        |project: &Project, channel: u16| project.channel(channel).unwrap().sample_path.clone();
    assert_eq!(sample(app.project(), 0).as_deref(), Some(stored));
    app.apply(Command::Save);
    assert_eq!(app.error(), None);
    let saved = fl_rs::read(&path).unwrap();
    assert_eq!(sample(&saved, 0).as_deref(), Some(stored));
    let (quieter, voices) = rendered(&saved, &dir.0);
    assert!(
        voices > 0 && quieter > 0.0 && quieter < loud,
        "{quieter} {loud}"
    );

    // A channel that is not there takes no sample, and is no step.
    let nowhere = Edit::SetChannelSample {
        channel: 7,
        path: quiet.clone(),
    };
    edit(&mut app, nowhere, None);
    assert!(!app.is_modified());
    app.apply(Command::Undo);
    app.apply(Command::Save);
    assert_eq!(std::fs::read(&path).unwrap(), original);

    // A new channel for the sample, a note for it, and the fixture's own
    // channel switched off: what sounds after a reload is the new one.
    let add = Edit::AddSamplerChannel {
        path: quiet.clone(),
    };
    edit(&mut app, add, None);
    assert_eq!(app.project().channels.len(), 2);
    let channel = app.project().channels[1].index;
    assert_eq!(
        app.state().project.channels[1].name.as_deref(),
        Some("Quiet click")
    );
    let note = Edit::AddNote {
        channel,
        position: 24,
        length: 0,
        key: 60,
    };
    edit(&mut app, note, None);
    let off = Edit::SetChannelEnabled {
        channel: 0,
        enabled: false,
    };
    edit(&mut app, off, None);
    if app.has_audio() {
        assert_eq!(app.error(), None);
    }
    let copy = dir.0.join("copy.flp");
    app.apply(Command::SaveAs(copy.clone()));
    assert_eq!(app.error(), None);
    let saved = fl_rs::read(&copy).unwrap();
    assert_eq!(saved.header.channel_count, 2);
    assert_eq!(sample(&saved, channel).as_deref(), Some(stored));
    assert_eq!(
        saved.channel(channel).unwrap().kind,
        Some(ChannelKind::Sampler)
    );
    let (peak, voices) = rendered(&saved, &dir.0);
    assert!(peak > 0.0 && voices == 1, "peak {peak}, {voices} voices");

    // Three undos take the channel away again, and the file's bytes are
    // the ones it had.
    for _ in 0..3 {
        app.apply(Command::Undo);
    }
    app.apply(Command::SaveAs(copy.clone()));
    assert_eq!(std::fs::read(&copy).unwrap(), original);
}

#[test]
fn a_preview_plays_beside_the_project() {
    let dir = Dir::new("preview");
    let (mut app, _) = open_fixture(&dir);
    if !app.has_audio() {
        eprintln!("no audio stream, preview test skipped: {:?}", app.error());
        return;
    }
    // Poll until the sample is decoded and its stream plays, or says why
    // it does not.
    let land = |app: &mut App| {
        let started = Instant::now();
        while !app.is_previewing() && app.error().is_none() {
            assert!(started.elapsed() < Duration::from_secs(30), "no preview");
            std::thread::sleep(Duration::from_millis(5));
            app.poll();
        }
    };
    app.apply(Command::Preview(dir.0.join("click.wav")));
    land(&mut app);
    if let Some(error) = app.error() {
        // A device that takes one stream at a time.
        assert!(error.starts_with("no sound: "), "{error}");
        eprintln!("no second stream, preview test skipped: {error}");
        return;
    }
    // The project is as it was: not changed, not playing, at its start.
    assert!(app.is_previewing() && !app.is_playing() && !app.is_modified());
    assert_eq!(app.position(), 0);
    app.apply(Command::Stop);
    assert!(!app.is_previewing());

    // A file that is no sample says so, and nothing plays.
    app.apply(Command::Preview(dir.0.join("song.flp")));
    land(&mut app);
    let error = app.state().last_error.unwrap();
    assert!(error.starts_with("cannot decode "), "{error}");
    assert!(!app.is_previewing());
}

/// The fixture with the mixer FL 20.8 would write, cut down to the master
/// and inserts 1 to 3: every insert switched on, the three sending to the
/// master, and an empty 225. The channel plays into insert 1.
fn mixer_fixture() -> Project {
    let mut project = fixture();
    let mut pos = 100;
    let mut field = |id: Id| {
        pos += 1;
        Placed {
            pos,
            item: Item::Field(id),
        }
    };
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
    project.channels[0].routed_to = Some(1);
    project
}

/// The mixer fixture, open in an app.
fn open_mixer_fixture(dir: &Dir) -> (App, PathBuf) {
    let path = write_fixture(&dir.0);
    fl_rs::save(&mixer_fixture(), &path).unwrap();
    let mut app = App::new(None);
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    (app, path)
}

#[test]
fn the_mixer_s_edits_are_saved_reloaded_and_undone() {
    let dir = Dir::new("mixer");
    let (mut app, path) = open_mixer_fixture(&dir);
    let original = std::fs::read(&path).unwrap();

    // A fader sends a value for each move, and the last one counts.
    for volume in [12_000, 11_000, 9600] {
        let edit = Edit::SetInsertVolume { insert: 1, volume };
        self::edit(&mut app, edit, Some(1));
    }
    let name = "Bass".to_owned();
    let edits = [
        Edit::SetInsertPan {
            insert: 1,
            pan: -3200,
        },
        Edit::SetInsertEnabled {
            insert: 2,
            enabled: false,
        },
        Edit::SetInsertName { insert: 2, name },
        Edit::SetRoute {
            from: 2,
            to: 3,
            on: true,
        },
        Edit::SetSendLevel {
            from: 2,
            to: 3,
            level: 6400,
        },
        Edit::SetSlotEnabled {
            insert: 1,
            slot: 0,
            enabled: false,
        },
        Edit::SetSlotMix {
            insert: 1,
            slot: 0,
            mix: 3200,
        },
    ];
    for edit in &edits {
        self::edit(&mut app, edit.clone(), None);
    }
    let check = |project: &Project| {
        let (drums, bass) = (project.insert_mix(1), project.insert_mix(2));
        assert_eq!((drums.volume_raw(), drums.pan_raw()), (9600, -3200));
        assert!(drums.enabled() && !bass.enabled());
        assert_eq!(bass.insert().unwrap().name.as_deref(), Some("Bass"));
        assert_eq!(bass.destinations(), [0, 3]);
        assert_eq!(bass.send_level_raw(3), Some(6400));
        assert_eq!(drums.slot_enabled(0), Some(false));
        assert_eq!(drums.slot_mix_raw(0), Some(3200));
    };
    check(app.project());
    if app.has_audio() {
        assert_eq!(app.error(), None);
    }
    app.apply(Command::Save);
    assert_eq!(app.error(), None);
    check(&fl_rs::read(&path).unwrap());

    // The drag is one step, and each edit after it another.
    for _ in 0..edits.len() {
        app.apply(Command::Undo);
    }
    assert_eq!(app.project().insert_mix(1).volume_raw(), 9600);
    app.apply(Command::Undo);
    app.apply(Command::Save);
    assert!(std::fs::read(&path).unwrap() == original);

    // A route that leads an insert back into itself, an insert the
    // project lacks and a value that reads already are no steps, so what
    // was undone is still there to redo.
    for edit in [
        Edit::SetRoute {
            from: 1,
            to: 1,
            on: true,
        },
        Edit::SetInsertVolume {
            insert: 9,
            volume: 1,
        },
        Edit::SetInsertVolume {
            insert: 1,
            volume: 12_800,
        },
    ] {
        self::edit(&mut app, edit, None);
    }
    assert!(!app.is_modified());
    app.apply(Command::Redo);
    assert_eq!(app.project().insert_mix(1).volume_raw(), 9600);
}

#[test]
fn the_meters_follow_the_inserts_that_play() {
    let dir = Dir::new("meters");
    let (mut app, _) = open_mixer_fixture(&dir);
    if !app.has_audio() {
        eprintln!("no audio stream, meter test skipped: {:?}", app.error());
        assert!(app.state().insert_peaks.is_empty());
        return;
    }
    app.apply(Command::Play);
    std::thread::sleep(Duration::from_millis(300));
    app.apply(Command::Stop);
    std::thread::sleep(Duration::from_millis(100));
    // The click played into insert 1, which sends to the master, and a
    // peak holds until it is read.
    let peaks = app.state().insert_peaks;
    assert_eq!(peaks.len(), 502);
    assert!(peaks[0].0 > 0.5 && peaks[1].0 > 0.5, "{:?}", &peaks[..4]);
    assert_eq!(peaks[2..], [(0.0, 0.0); 500]);
    // A meter falls from where it was.
    let fallen = app.state().insert_peaks[1].0;
    assert!(fallen > 0.0 && fallen < peaks[1].0, "{fallen}");
}

fn corpus() -> Option<(PathBuf, Vec<PathBuf>)> {
    fn find(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                find(&path, out);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("flp"))
            {
                out.push(path);
            }
        }
    }
    let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        eprintln!("FL_RS_CORPUS unset, corpus test skipped");
        return None;
    };
    assert!(
        root.is_dir(),
        "FL_RS_CORPUS={} is not a directory",
        root.display()
    );
    let mut files = Vec::new();
    find(&root, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no .flp files under {}", root.display());
    Some((root, files))
}

#[test]
fn every_corpus_project_saves_back_byte_for_byte() {
    let Some((root, files)) = corpus() else {
        return;
    };
    let dir = Dir::new("corpus-save");
    let copy = dir.0.join("copy.flp");
    for path in &files {
        let name = path.strip_prefix(&root).unwrap().display();
        let project = fl_rs::read(path).unwrap_or_else(|e| panic!("{name}: {e}"));
        fl_rs::save(&project, &copy).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (written, original) = (std::fs::read(&copy).unwrap(), std::fs::read(path).unwrap());
        assert!(written == original, "{name}: the saved file differs");
    }
    eprintln!("corpus save: {} projects byte for byte", files.len());
}

#[test]
fn a_corpus_project_opens_plays_and_saves_back() {
    let Some((root, files)) = corpus() else {
        return;
    };
    // The smallest project is the quickest to decode the samples of.
    let size = |path: &&PathBuf| std::fs::metadata(path).unwrap().len();
    let path = files.iter().min_by_key(size).unwrap();
    let mut app = App::new(Some(root));
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    assert_eq!(app.path(), Some(path.as_path()), "{:?}", app.error());

    let dir = Dir::new("corpus-play");
    let copy = dir.0.join("copy.flp");
    app.apply(Command::SaveAs(copy.clone()));
    assert!(std::fs::read(&copy).unwrap() == std::fs::read(path).unwrap());

    if !app.has_audio() {
        eprintln!("no audio stream, play test skipped: {:?}", app.error());
        return;
    }
    app.apply(Command::Play);
    std::thread::sleep(Duration::from_millis(300));
    let position = app.position();
    eprintln!("{} at tick {position} after 300 ms", path.display());
    assert!(position > 0, "the clock stood still");
    app.apply(Command::Stop);
}

#[test]
fn a_move_in_a_corpus_project_changes_one_record_of_the_file() {
    let Some((root, mut files)) = corpus() else {
        return;
    };
    // The smallest project with an item in its playlist.
    files.sort_by_key(|path| std::fs::metadata(path).unwrap().len());
    let first_item = |path: &PathBuf| {
        let project = fl_rs::read(path).ok()?;
        let items = &project.current_arrangement()?.items;
        let id = items.iter().position(|item| item.source().is_some())?;
        Some((path.clone(), id))
    };
    let (path, id) = files.iter().find_map(first_item).unwrap();
    let original = std::fs::read(&path).unwrap();

    let mut app = App::new(Some(root));
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    assert_eq!(app.path(), Some(path.as_path()), "{:?}", app.error());
    let before = places(app.project());
    let (position, track) = before[id];
    let dir = Dir::new("corpus-edit");
    let copy = dir.0.join("copy.flp");
    let saved = |app: &mut App| {
        app.apply(Command::SaveAs(copy.clone()));
        assert_eq!(app.error(), None);
        std::fs::read(&copy).unwrap()
    };

    edit(&mut app, moved(id, 96, 1), None);
    let written = saved(&mut app);
    // The file parses, and the item has moved in it.
    let mut after = before.clone();
    after[id] = (position + 96, track + 1);
    assert_eq!(places(&fl_rs::read(&copy).unwrap()), after);
    // One event differs, the playlist, and of it only that item's record.
    let was = flp::Project::parse(&original).unwrap();
    let now = flp::Project::parse(&written).unwrap();
    assert!(was.header == now.header);
    assert_eq!(was.events.len(), now.events.len());
    let pairs = was.events.iter().zip(&now.events);
    let changed: Vec<_> = pairs.filter(|(was, now)| was != now).collect();
    let [(was, now)] = changed[..] else {
        panic!("{} events differ", changed.len());
    };
    assert_eq!((was.id, now.id), (Id::ArrangementPlaylist.0, was.id));
    assert_eq!(was.data.len(), now.data.len());
    let size = was.data.len() / before.len();
    let records = was.data.chunks(size).zip(now.data.chunks(size));
    let differing: Vec<usize> = records
        .enumerate()
        .filter_map(|(at, (was, now))| (was != now).then_some(at))
        .collect();
    assert_eq!(differing, [id]);

    app.apply(Command::Undo);
    assert!(saved(&mut app) == original, "undo left a change");

    // Three edits of one drag are one step.
    for ticks in [96, 192, 288] {
        edit(&mut app, moved(id, ticks, 0), Some(1));
    }
    after[id] = (position + 288, track);
    assert_eq!(places(app.project()), after);
    assert!(saved(&mut app) != original);
    app.apply(Command::Undo);
    assert!(saved(&mut app) == original, "undo left part of the drag");
    eprintln!("{}: moved item {id} and back", path.display());
}

/// The project with every event at position 0. Events that were added
/// share a position with the event in front of them until the file is
/// read back, so two projects that write the same file differ in these.
fn without_positions(mut project: Project) -> Project {
    fn clear(order: &mut [Placed]) {
        for placed in order {
            placed.pos = 0;
        }
    }
    clear(&mut project.order);
    for channel in &mut project.channels {
        clear(&mut channel.order);
    }
    for pattern in &mut project.patterns {
        clear(&mut pattern.order);
        for marker in &mut pattern.time_markers {
            clear(&mut marker.order);
        }
    }
    for arrangement in &mut project.arrangements {
        clear(&mut arrangement.order);
        for track in &mut arrangement.tracks {
            clear(&mut track.order);
        }
        for marker in &mut arrangement.time_markers {
            clear(&mut marker.order);
        }
    }
    for insert in &mut project.inserts {
        clear(&mut insert.order);
        for slot in &mut insert.slots {
            clear(&mut slot.order);
        }
    }
    for link in &mut project.controller_links {
        clear(&mut link.order);
    }
    project
}

/// The first step that channel `channel` has dark in the pattern.
fn dark_step(project: &Project, pattern: u16, channel: u16) -> u32 {
    let steps = project
        .pattern(pattern)
        .map(|p| p.steps(channel, project.header.ppq));
    let steps = steps.unwrap_or_default();
    let dark = steps.iter().position(|lit| !lit).unwrap_or(steps.len());
    dark as u32
}

#[test]
fn a_step_in_any_corpus_project_adds_one_record_and_moves_no_event() {
    let Some((root, files)) = corpus() else {
        return;
    };
    let (mut edited, mut made, mut added) = (0, 0, 0);
    for path in &files {
        let name = path.strip_prefix(&root).unwrap().display();
        let project = fl_rs::read(path).unwrap();
        let Some(channel) = project.channels.first().map(|channel| channel.index) else {
            continue;
        };
        let was = model::to_flp(&project).unwrap().events;
        // Pattern 1, which 37 projects do not have, and the last pattern.
        let last = project.patterns.last().map_or(1, |pattern| pattern.index);
        for pattern in [1, last] {
            let step = dark_step(&project, pattern, channel);
            let mut changed = project.clone();
            let ppq = changed.header.ppq;
            changed
                .pattern_to_edit(pattern)
                .set_step(channel, step, true, ppq);
            let written = model::to_flp(&changed).unwrap();
            let read = model::from_flp(&written);
            // Reading the file back gives the project that was written:
            // no event went to another object.
            assert!(
                without_positions(read.clone()) == without_positions(changed),
                "{name}: pattern {pattern} reads back as another project"
            );
            let lit = read.pattern(pattern).map(|p| p.steps(channel, ppq));
            if lit.is_none_or(|lit| lit.get(step as usize) != Some(&true)) {
                // The one pattern whose 224 is not whole records.
                assert!(written.events == was, "{name}: pattern {pattern}");
                continue;
            }

            // The events that are not in the file as they were. What is
            // left of the file's events is in the order it had.
            let mut left = was.iter().peekable();
            let mut new = Vec::new();
            for event in &written.events {
                if left.peek() == Some(&event) {
                    left.next();
                } else if left
                    .peek()
                    .is_some_and(|was| was.id == event.id && event.id == 224)
                {
                    new.push((event, left.next()));
                } else {
                    new.push((event, None));
                }
            }
            assert!(left.next().is_none(), "{name}: an event is gone");
            match new[..] {
                [(now, Some(was))] => {
                    assert_eq!(now.data.len(), was.data.len() + 24, "{name}");
                    edited += 1;
                }
                [(notes, None)] => {
                    assert_eq!((notes.id, notes.data.len()), (224, 24), "{name}");
                    added += 1;
                }
                [(pattern_new, None), (notes, None)] => {
                    assert_eq!((pattern_new.id, pattern_new.data.len()), (65, 2), "{name}");
                    assert_eq!((notes.id, notes.data.len()), (224, 24), "{name}");
                    made += 1;
                }
                _ => panic!("{name}: pattern {pattern} changes {} events", new.len()),
            }
        }
    }
    eprintln!("corpus steps: {edited} in a 224, {added} with a new 224, {made} with a new pattern");
    assert!(edited > 0 && made > 0);
}

#[test]
fn the_channel_edits_in_any_corpus_project_change_values_only() {
    let Some((root, files)) = corpus() else {
        return;
    };
    for path in &files {
        let name = path.strip_prefix(&root).unwrap().display();
        let project = fl_rs::read(path).unwrap();
        let was = model::to_flp(&project).unwrap().events;
        let mut changed = project.clone();
        for channel in project.channels.iter().map(|channel| channel.index) {
            changed.set_channel_enabled(channel, false);
            changed.set_channel_volume(channel, 5000);
            changed.set_channel_pan(channel, 1000);
            changed.set_channel_insert(channel, 2);
        }
        let written = model::to_flp(&changed).unwrap();
        let read = model::from_flp(&written);
        assert!(
            without_positions(read) == without_positions(changed),
            "{name}: reads back as another project"
        );
        // Every channel of the corpus has the four events, so none is new,
        // and only those four differ.
        assert_eq!(written.events.len(), was.len(), "{name}");
        let pairs = was.iter().zip(&written.events);
        for (was, now) in pairs.filter(|(was, now)| was != now) {
            assert!(
                was.id == now.id && [0, 22, 104, 219].contains(&now.id),
                "{name}: event {} became {}",
                was.id,
                now.id
            );
            assert_eq!(was.data.len(), now.data.len(), "{name}: event {}", now.id);
        }
    }
}

#[test]
fn a_step_in_a_corpus_project_is_saved_reloaded_and_undone() {
    let Some((root, mut files)) = corpus() else {
        return;
    };
    // The smallest project that has a pattern with notes for a sampler.
    files.sort_by_key(|path| std::fs::metadata(path).unwrap().len());
    let stepped = |path: &PathBuf| {
        let project = fl_rs::read(path).ok()?;
        let sampler = |note: &Note| {
            let channel = project.channel(note.channel)?;
            (channel.kind == Some(ChannelKind::Sampler)).then_some(channel.index)
        };
        let pattern = |pattern: &Pattern| {
            let channel = pattern.notes.iter().find_map(sampler)?;
            Some((path.clone(), pattern.index, channel))
        };
        project.patterns.iter().find_map(pattern)
    };
    let (path, pattern, channel) = files.iter().find_map(stepped).unwrap();
    let original = std::fs::read(&path).unwrap();

    let mut app = App::new(Some(root));
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    assert_eq!(app.path(), Some(path.as_path()), "{:?}", app.error());
    app.apply(Command::SelectPattern(pattern));
    let ppq = app.project().header.ppq;
    let step = dark_step(app.project(), pattern, channel);
    let notes = |project: &Project| project.pattern(pattern).unwrap().notes.clone();
    let before = notes(app.project());
    let dir = Dir::new("corpus-step");
    let copy = dir.0.join("copy.flp");
    let saved = |app: &mut App| {
        app.apply(Command::SaveAs(copy.clone()));
        assert_eq!(app.error(), None);
        std::fs::read(&copy).unwrap()
    };

    let set = |on| Edit::SetStep { channel, step, on };
    edit(&mut app, set(true), None);
    let written = saved(&mut app);
    // The file parses, and the pattern has the step's note in it.
    let read = fl_rs::read(&copy).unwrap();
    assert!(read.pattern(pattern).unwrap().steps(channel, ppq)[step as usize]);
    let after = notes(&read);
    let position = (step * u32::from(ppq / 4)) as i32;
    let at = after
        .iter()
        .position(|note| (note.position, note.channel, note.length) == (position, channel, 0))
        .unwrap();
    let mut others = after.clone();
    others.remove(at);
    assert_eq!(others, before);
    // One event differs, the pattern's 224, and everything else is the
    // file's byte for byte.
    let was = flp::Project::parse(&original).unwrap();
    let now = flp::Project::parse(&written).unwrap();
    assert!(was.header == now.header);
    assert_eq!(was.events.len(), now.events.len());
    let pairs = was.events.iter().zip(&now.events);
    let changed: Vec<_> = pairs.filter(|(was, now)| was != now).collect();
    let [(was, now)] = changed[..] else {
        panic!("{} events differ", changed.len());
    };
    assert_eq!((was.id, now.id), (Id::PatternNotes.0, Id::PatternNotes.0));
    assert_eq!(now.data, Note::list_to_bytes(&after));
    assert_eq!(was.data, Note::list_to_bytes(&before));

    app.apply(Command::Undo);
    assert!(saved(&mut app) == original, "undo left a change");

    // Lit and cleared again by hand, the file is the same too.
    edit(&mut app, set(true), None);
    edit(&mut app, set(false), None);
    assert!(
        saved(&mut app) == original,
        "clearing the step left a change"
    );
    eprintln!(
        "{}: step {step} of channel {channel} in pattern {pattern} and back",
        path.display()
    );
}

#[test]
fn a_fader_in_a_corpus_project_changes_one_record_of_the_file() {
    let Some((root, files)) = corpus() else {
        return;
    };
    // The smallest project. Every project of the corpus has an insert 1.
    let size = |path: &&PathBuf| std::fs::metadata(path).unwrap().len();
    let path = files.iter().min_by_key(size).unwrap();
    let original = std::fs::read(path).unwrap();

    let mut app = App::new(Some(root));
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    assert_eq!(app.path(), Some(path.as_path()), "{:?}", app.error());
    let dir = Dir::new("corpus-fader");
    let copy = dir.0.join("copy.flp");
    let saved = |app: &mut App| {
        app.apply(Command::SaveAs(copy.clone()));
        assert_eq!(app.error(), None);
        std::fs::read(&copy).unwrap()
    };
    let before = app.project().insert_mix(1).volume_raw();
    let volume = if before == 9600 { 4800 } else { 9600 };

    edit(&mut app, Edit::SetInsertVolume { insert: 1, volume }, None);
    let written = saved(&mut app);
    // The file parses, and the strip reads the volume back.
    let read = fl_rs::read(&copy).unwrap();
    assert_eq!(read.insert_mix(1).volume_raw(), volume);
    // One event differs, the 225, and of it only the value of the record
    // for that fader.
    let was = flp::Project::parse(&original).unwrap();
    let now = flp::Project::parse(&written).unwrap();
    assert!(was.header == now.header);
    assert_eq!(was.events.len(), now.events.len());
    let pairs = was.events.iter().zip(&now.events);
    let changed: Vec<_> = pairs.filter(|(was, now)| was != now).collect();
    let [(was, now)] = changed[..] else {
        panic!("{} events differ", changed.len());
    };
    assert_eq!((was.id, now.id), (Id::MixerParams.0, was.id));
    assert_eq!(was.data.len(), now.data.len());
    let records = was.data.chunks(12).zip(now.data.chunks(12));
    let differing: Vec<_> = records.filter(|(was, now)| was != now).collect();
    let [(was, now)] = differing[..] else {
        panic!("{} records differ", differing.len());
    };
    assert_eq!(was[..8], now[..8]);
    assert_eq!(was[8..], before.to_le_bytes());
    assert_eq!(now[8..], volume.to_le_bytes());
    let fader = RecTarget::Insert {
        index: 1,
        slot: None,
        control: model::rec::control::VOLUME,
    };
    let rec_id = u32::from_le_bytes(now[4..8].try_into().unwrap());
    assert_eq!(read.rec_target(rec_id), fader);

    app.apply(Command::Undo);
    assert!(saved(&mut app) == original, "undo left a change");

    // Three values of one drag are one step.
    for volume in [3000, 2000, 1000] {
        edit(
            &mut app,
            Edit::SetInsertVolume { insert: 1, volume },
            Some(1),
        );
    }
    assert_eq!(app.project().insert_mix(1).volume_raw(), 1000);
    assert!(saved(&mut app) != original);
    app.apply(Command::Undo);
    assert!(saved(&mut app) == original, "undo left part of the drag");
    eprintln!(
        "{}: insert 1 from {before} to {volume} and back",
        path.display()
    );
}

#[test]
fn the_mixer_edits_in_any_corpus_project_change_their_own_events_only() {
    let Some((root, files)) = corpus() else {
        return;
    };
    let (mut records, mut appended, mut added) = (0, 0, 0);
    for path in &files {
        let name = path.strip_prefix(&root).unwrap().display();
        let project = fl_rs::read(path).unwrap();
        // A new record gets the ID that FL gave the record it has.
        for record in &project.initial_values {
            let target = project.rec_target(record.rec_id);
            let id = project.rec_id(target);
            assert_eq!(id, Some(record.rec_id), "{name}: {target:?}");
        }
        records += project.initial_values.len();
        // A new event of an insert gets its place by the order FL writes
        // an insert's events in, which is this one.
        for insert in &project.inserts {
            let ids = insert.order.iter().map(|placed| match &placed.item {
                Item::Field(id) => id.0,
                Item::Raw(e) => e.id,
            });
            let order = [149, 42, 95, 204, 27, 236, 235, 165, 166, 49, 154, 147];
            let rank = |id: u32| order.iter().position(|known| *known == id);
            let ranks: Vec<_> = ids.map(rank).collect();
            assert!(ranks.iter().all(Option::is_some), "{name}: {ranks:?}");
            assert!(ranks.is_sorted(), "{name}: {ranks:?}");
        }

        let was = model::to_flp(&project).unwrap().events;
        let mut changed = project.clone();
        changed.set_insert_volume_raw(1, 4321);
        changed.set_insert_pan_raw(1, -1234);
        changed.set_send_level_raw(1, 2, 6000);
        changed.set_slot_mix_raw(1, 3, 5000);
        changed.set_slot_enabled(1, 3, false);
        changed.set_insert_enabled(1, false);
        changed.set_insert_name(1, "Renamed");
        // Insert 2 may send to insert 1, and then gets no route back.
        let routed = !project.inserts_feeding(1).contains(&2);
        changed.set_route(1, 2, true);

        let written = model::to_flp(&changed).unwrap();
        let read = model::from_flp(&written);
        assert!(
            without_positions(read.clone()) == without_positions(changed),
            "{name}: reads back as another project"
        );
        let strip = read.insert_mix(1);
        assert_eq!(
            (strip.volume_raw(), strip.pan_raw()),
            (4321, -1234),
            "{name}"
        );
        assert_eq!(strip.send_level_raw(2), Some(6000), "{name}");
        assert_eq!(strip.slot_mix_raw(3), Some(5000), "{name}");
        assert_eq!(strip.slot_enabled(3), Some(false), "{name}");
        assert!(!strip.enabled(), "{name}");
        let insert = strip.insert().unwrap();
        assert_eq!(insert.name.as_deref(), Some("Renamed"), "{name}");
        assert_eq!(strip.destinations().contains(&2), routed, "{name}");

        // What is left of the file's events is in the order it had. An
        // event that changed is one of the mixer's five, and a new one is
        // a name or a 235.
        let mut left = was.iter().peekable();
        for event in &written.events {
            if left.peek() == Some(&event) {
                left.next();
            } else if let Some(was) = left.next_if(|was| was.id == event.id) {
                assert!([27, 204, 225, 235, 236].contains(&event.id), "{name}");
                if event.id == 225 {
                    // The records the file had are there, under their IDs.
                    let pairs = was.data.chunks(12).zip(event.data.chunks(12));
                    assert!(
                        pairs.clone().all(|(was, now)| was[..8] == now[..8]),
                        "{name}"
                    );
                    appended += event.data.len() / 12 - pairs.count();
                }
            } else {
                assert!([204, 235].contains(&event.id), "{name}: a new {}", event.id);
                added += 1;
            }
        }
        assert!(left.next().is_none(), "{name}: an event is gone");
    }
    eprintln!(
        "corpus mixer: {records} records have the ID a new one would get; \
         {appended} records appended and {added} events added over {} projects",
        files.len()
    );
    assert!(appended > 0 && added > 0);
}

#[test]
fn a_note_in_a_corpus_project_is_saved_reloaded_and_undone() {
    let Some((root, mut files)) = corpus() else {
        return;
    };
    // The smallest project that has a pattern with a note in it.
    files.sort_by_key(|path| std::fs::metadata(path).unwrap().len());
    let with_notes = |path: &PathBuf| {
        let project = fl_rs::read(path).ok()?;
        let pattern = project.patterns.iter().find(|p| !p.notes.is_empty())?;
        Some((path.clone(), pattern.index, pattern.notes[0].channel))
    };
    let (path, pattern, channel) = files.iter().find_map(with_notes).unwrap();
    let original = std::fs::read(&path).unwrap();

    let mut app = App::new(Some(root));
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    assert_eq!(app.path(), Some(path.as_path()), "{:?}", app.error());
    app.apply(Command::SelectPattern(pattern));
    let ppq = app.project().header.ppq;
    let notes = |project: &Project| project.pattern(pattern).unwrap().notes.clone();
    let before = notes(app.project());
    let dir = Dir::new("corpus-note");
    let copy = dir.0.join("copy.flp");
    let saved = |app: &mut App| {
        app.apply(Command::SaveAs(copy.clone()));
        assert_eq!(app.error(), None);
        std::fs::read(&copy).unwrap()
    };

    // A G5 on the second beat, half a beat long.
    let new = Note {
        position: i32::from(ppq),
        channel,
        length: i32::from(ppq / 2),
        key: 67,
        ..Note::default()
    };
    let add = Edit::AddNote {
        channel,
        position: u64::from(ppq),
        length: u64::from(ppq / 2),
        key: 67,
    };
    edit(&mut app, add, None);
    let written = saved(&mut app);
    // The file parses, and the pattern has the note in front of the notes
    // that start where it does, with every other note as it was.
    let after = notes(&fl_rs::read(&copy).unwrap());
    let at = before.partition_point(|note| note.position < new.position);
    assert_eq!(after[at], new);
    let mut others = after.clone();
    others.remove(at);
    assert_eq!(others, before);
    // One event differs, the pattern's 224, and everything else is the
    // file's byte for byte.
    let was = flp::Project::parse(&original).unwrap();
    let now = flp::Project::parse(&written).unwrap();
    assert!(was.header == now.header);
    assert_eq!(was.events.len(), now.events.len());
    let pairs = was.events.iter().zip(&now.events);
    let changed: Vec<_> = pairs.filter(|(was, now)| was != now).collect();
    let [(was, now)] = changed[..] else {
        panic!("{} events differ", changed.len());
    };
    assert_eq!((was.id, now.id), (Id::PatternNotes.0, Id::PatternNotes.0));
    assert_eq!(now.data, Note::list_to_bytes(&after));
    assert_eq!(was.data, Note::list_to_bytes(&before));

    app.apply(Command::Undo);
    assert!(saved(&mut app) == original, "undo left a change");

    // Drawn in a drag, moved in another and deleted, the note leaves its
    // record behind in the open project and nothing in the file, and
    // three undos take the record away too.
    for length in [ppq / 4, ppq / 2] {
        let drawn = Edit::AddNote {
            channel,
            position: u64::from(ppq),
            length: u64::from(length),
            key: 67,
        };
        edit(&mut app, drawn, Some(1));
    }
    assert_eq!(notes(app.project()), after);
    for keys in [1, 5] {
        let moved = Edit::MoveNotes {
            ids: vec![at],
            delta_ticks: 0,
            delta_keys: keys,
        };
        edit(&mut app, moved, Some(2));
    }
    assert_eq!(notes(app.project())[at].key, 72);
    edit(&mut app, Edit::DeleteNotes { ids: vec![at] }, None);
    let left = notes(app.project());
    assert!(left[at].is_deleted());
    assert_eq!(left.len(), before.len() + 1);
    // FL writes no deleted record, and neither does the save.
    assert!(
        saved(&mut app) == original,
        "the deleted note left a change"
    );
    assert_eq!(notes(&fl_rs::read(&copy).unwrap()), before);
    for _ in 0..3 {
        app.apply(Command::Undo);
    }
    assert!(saved(&mut app) == original, "undo left part of the edits");
    eprintln!(
        "{}: a note for channel {channel} in pattern {pattern} and back",
        path.display()
    );
}

#[test]
fn a_deleted_note_in_a_corpus_project_is_left_out_of_the_saved_224() {
    let Some((root, mut files)) = corpus() else {
        return;
    };
    // The smallest project that has a pattern with two notes in it.
    files.sort_by_key(|path| std::fs::metadata(path).unwrap().len());
    let with_notes = |path: &PathBuf| {
        let project = fl_rs::read(path).ok()?;
        let pattern = project.patterns.iter().find(|p| p.notes.len() > 1)?;
        Some((path.clone(), pattern.index))
    };
    let (path, pattern) = files.iter().find_map(with_notes).unwrap();
    let original = std::fs::read(&path).unwrap();

    let mut app = App::new(Some(root));
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    assert_eq!(app.path(), Some(path.as_path()), "{:?}", app.error());
    app.apply(Command::SelectPattern(pattern));
    let notes = |project: &Project| project.pattern(pattern).unwrap().notes.clone();
    let before = notes(app.project());
    let dir = Dir::new("corpus-compact");
    let copy = dir.0.join("copy.flp");
    let saved = |app: &mut App| {
        app.apply(Command::SaveAs(copy.clone()));
        assert_eq!(app.error(), None);
        std::fs::read(&copy).unwrap()
    };

    edit(&mut app, Edit::DeleteNotes { ids: vec![0] }, None);
    // The open project keeps the record, so every note keeps its place.
    let open = notes(app.project());
    assert!(open[0].is_deleted());
    assert_eq!(open[1..], before[1..]);
    let written = saved(&mut app);
    assert_eq!(notes(app.project()), open);

    // The file has the other notes and no record for the deleted one.
    assert_eq!(notes(&fl_rs::read(&copy).unwrap()), before[1..]);
    // One event differs, the pattern's 224, which is a record shorter.
    // Everything else is the file's byte for byte.
    let was = flp::Project::parse(&original).unwrap();
    let now = flp::Project::parse(&written).unwrap();
    assert!(was.header == now.header);
    assert_eq!(was.events.len(), now.events.len());
    let pairs = was.events.iter().zip(&now.events);
    let changed: Vec<_> = pairs.filter(|(was, now)| was != now).collect();
    let [(was, now)] = changed[..] else {
        panic!("{} events differ", changed.len());
    };
    assert_eq!((was.id, now.id), (Id::PatternNotes.0, Id::PatternNotes.0));
    assert_eq!(now.data.len() + 24, was.data.len());
    assert_eq!(now.data, was.data[24..]);
    assert_eq!(written.len() + 24, original.len());

    app.apply(Command::Undo);
    assert!(saved(&mut app) == original, "undo left a change");
    eprintln!(
        "{}: note 0 of pattern {pattern} deleted, saved without it and undone",
        path.display()
    );
}

#[test]
fn a_new_sampler_channel_for_a_corpus_sample_plays_after_a_reload() {
    let Some((root, _)) = corpus() else {
        return;
    };
    // The first kick FL ships that is no WavPack file.
    let kicks = root.join("Patches/Packs/Drums/Kicks");
    let mut samples: Vec<PathBuf> = std::fs::read_dir(&kicks)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "wav"))
        .collect();
    samples.sort();
    let sample = samples.first().unwrap().clone();
    // FL's own empty project: one sampler without a sample, and no notes.
    let path = root.join("Templates/Empty/Empty.flp");
    let original = std::fs::read(&path).unwrap();

    let mut app = App::new(Some(root.clone()));
    app.apply(Command::Open(path.clone()));
    settle(&mut app);
    assert_eq!(app.path(), Some(path.as_path()), "{:?}", app.error());
    assert_eq!(rendered(app.project(), &root), (0.0, 0));

    let add = Edit::AddSamplerChannel {
        path: sample.clone(),
    };
    edit(&mut app, add, None);
    let channel = app.project().channels.last().unwrap().index;
    assert_eq!(channel, 1);
    let note = Edit::AddNote {
        channel,
        position: 0,
        length: 0,
        key: 60,
    };
    edit(&mut app, note, None);
    let dir = Dir::new("corpus-channel");
    let copy = dir.0.join("copy.flp");
    app.apply(Command::SaveAs(copy.clone()));
    assert_eq!(app.error(), None);

    // The path is the one FL writes for the samples it ships, and the
    // new channel has every event the file's own sampler has, and a 196.
    let saved = fl_rs::read(&copy).unwrap();
    let name = sample.file_name().unwrap().to_string_lossy();
    let stored = format!(r"%FLStudioFactoryData%\Data\Patches\Packs\Drums\Kicks\{name}");
    let new = saved.channel(channel).unwrap();
    assert_eq!(new.sample_path.as_deref(), Some(stored.as_str()));
    assert_eq!(new.kind, Some(ChannelKind::Sampler));
    let ids = |channel: &Channel| -> Vec<u32> {
        let id = |placed: &Placed| match &placed.item {
            Item::Field(id) => id.0,
            Item::Raw(e) => e.id,
        };
        channel.order.iter().map(id).collect()
    };
    let mut own = ids(saved.channel(0).unwrap());
    own.push(Id::SamplePath.0);
    assert_eq!(ids(new), own);
    // A second of it sounds.
    let (peak, voices) = rendered(&saved, &root);
    assert!(peak > 0.01 && voices == 1, "peak {peak}, {voices} voices");

    app.apply(Command::Undo);
    app.apply(Command::Undo);
    app.apply(Command::SaveAs(copy.clone()));
    assert!(
        std::fs::read(&copy).unwrap() == original,
        "undo left a change"
    );
    eprintln!("{}: a channel for {name}, peak {peak}", path.display());
}
