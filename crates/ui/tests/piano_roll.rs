//! The piano roll drawn from a project built in code.
//!
//! The snapshot is `tests/snapshots/piano_roll.png`, see `shell.rs` for
//! how to write it again.

use egui::{Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use model::{Channel, ChannelKind, Note, Pattern, Project};
use std::sync::Arc;
use ui::{Command, Edit, NoteField, UiState};

/// The bass line's place in the pattern's notes: a C4 on the first beat,
/// a G4 half as long on the second, and a D#4 from the third beat on.
const BASS: [usize; 3] = [1, 4, 6];

fn channel(index: u16, kind: ChannelKind, name: &str, colour: Option<u32>) -> Channel {
    Channel {
        index,
        kind: Some(kind),
        name: Some(name.to_owned()),
        color: colour,
        ..Channel::default()
    }
}

fn note(channel: u16, position: i32, length: i32, key: i16) -> Note {
    Note {
        position,
        channel,
        length,
        key,
        ..Note::default()
    }
}

/// The channel rack's bar of drums and bass at PPQ 96, with the snare an
/// octave down so that it shows behind the bass line.
fn fixture() -> Project {
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 4,
        ppq: 96,
    });
    project.writer_version = Some("24.1.1.4285".to_owned());
    project.channels = vec![
        channel(0, ChannelKind::Sampler, "Kick", Some(0x0040_60d0)),
        channel(1, ChannelKind::Sampler, "Snare", None),
        channel(2, ChannelKind::Plugin, "Bass", Some(0x0060_c080)),
        channel(3, ChannelKind::AudioClip, "Vocal", Some(0x00d0_9050)),
    ];
    let mut notes: Vec<Note> = (0..4).map(|beat| note(0, beat * 96, 0, 60)).collect();
    notes.extend([note(1, 96, 0, 52), note(1, 288, 0, 52)]);
    notes.extend([
        note(2, 0, 96, 48),
        note(2, 96, 48, 55),
        note(2, 192, 144, 51),
    ]);
    notes.sort_by_key(|note| note.position);
    let own = |channel: u16| {
        let notes = notes.iter().enumerate();
        let own = notes.filter(move |(_, note)| note.channel == channel);
        own.map(|(id, _)| id).collect::<Vec<_>>()
    };
    assert_eq!(own(2), BASS);
    project.patterns.push(Pattern {
        index: 1,
        name: Some("Beat".to_owned()),
        notes,
        ..Pattern::default()
    });
    project
}

/// The shell with the piano roll in front, showing the bass. The pattern
/// plays, a beat and a half in.
fn harness(project: Project) -> Harness<'static, Vec<Command>> {
    let state = UiState {
        project: Arc::new(project),
        position: 144,
        peaks: (0.0, 0.0),
        insert_peaks: Vec::new(),
        playing: true,
        song_mode: false,
        looping: false,
        pattern: 1,
        data_root: None,
        project_dir: None,
        sample_rate: Some(48_000),
        last_error: None,
        busy: None,
        progress: None,
    };
    let mut harness = Harness::builder()
        .with_size(vec2(800.0, 400.0))
        .with_theme(egui::Theme::Dark)
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new());
    harness.key_press(Key::F7);
    harness.run();
    // The rack has selected no channel yet, so the first one shows.
    harness.get_by_value("Kick").click();
    harness.run();
    harness.get_by_label("Bass").click();
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());
    harness
}

/// A point on the notes: a tick of the pattern, in the middle of a key's
/// row. A beat is 64 points wide and a row 12 high, and the piano roll
/// opens with the middle of the bass line, key 51, half way down.
fn at(harness: &Harness<'_, Vec<Command>>, tick: f32, key: i32) -> Pos2 {
    let area = harness.get_by_label("Bass notes").rect();
    let scrolled = (80.0 * 12.0 + 6.0 - area.height() / 2.0).round();
    let row = (131 - key) as f32 * 12.0;
    pos2(
        area.left() + tick * 64.0 / 96.0,
        area.top() - scrolled + row + 6.0,
    )
}

fn click(harness: &mut Harness<'_, Vec<Command>>, pos: Pos2) {
    harness.hover_at(pos);
    harness.drag_at(pos);
    harness.drop_at(pos);
    harness.run();
}

fn right_click(harness: &mut Harness<'_, Vec<Command>>, pos: Pos2) {
    harness.hover_at(pos);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run();
}

/// Press at the first point and go to each of the others, a frame apiece.
/// The button stays down.
fn drag(harness: &mut Harness<'_, Vec<Command>>, from: Pos2, to: &[Pos2]) {
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    for pos in to {
        harness.hover_at(*pos);
        harness.step();
    }
}

fn edit(edit: Edit) -> Command {
    Command::Edit {
        edit,
        drag_id: None,
    }
}

fn dragged(edit: Edit, drag_id: u64) -> Command {
    Command::Edit {
        edit,
        drag_id: Some(drag_id),
    }
}

fn select(ids: &[usize], add: bool) -> Command {
    let ids = ids.to_vec();
    edit(Edit::SelectNotes { ids, add })
}

fn moved(ids: &[usize], delta_ticks: i64, delta_keys: i32) -> Edit {
    Edit::MoveNotes {
        ids: ids.to_vec(),
        delta_ticks,
        delta_keys,
    }
}

#[test]
fn the_piano_roll_looks_like_its_snapshot() {
    let mut project = fixture();
    let notes = &mut project.patterns[0].notes;
    notes[BASS[1]].flags |= Note::SELECTED;
    notes[BASS[2]].flags |= Note::MUTED;
    notes[BASS[2]].velocity = 60;
    let mut harness = harness(project);
    // C minor darkens the rows of E, A and B and lightens the C's.
    for (menu, entry) in [("Scale", "C"), ("C Major", "Minor")] {
        harness.get_by_label(menu).click();
        harness.run();
        harness.get_by_label(entry).click();
        harness.run();
    }
    harness.remove_cursor();
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());
    harness.snapshot("piano_roll");
}

#[test]
fn a_click_with_the_draw_tool_adds_a_note_where_it_snaps() {
    let mut harness = harness(fixture());
    // The grid's step is a sixteenth, 24 ticks, and tick 250 is in the
    // one that starts at 240. A first note is a step long.
    let pos = at(&harness, 250.0, 57);
    click(&mut harness, pos);
    let add = |position, length, key| {
        edit(Edit::AddNote {
            channel: 2,
            position,
            length,
            key,
        })
    };
    assert_eq!(harness.state(), &[add(240, 24, 57)]);

    // A click on a note selects it, and the next note is as long.
    let pos = at(&harness, 110.0, 55);
    click(&mut harness, pos);
    let pos = at(&harness, 400.0, 50);
    click(&mut harness, pos);
    assert_eq!(
        harness.state()[1..],
        [select(&[BASS[1]], false), add(384, 48, 50)]
    );

    // With Alt the note starts on the tick under the pointer.
    harness.event(egui::Event::ModifiersChanged(Modifiers::ALT));
    let pos = at(&harness, 250.5, 57);
    click(&mut harness, pos);
    harness.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    harness.run();
    assert_eq!(harness.state()[3..], [add(250, 48, 57)]);

    // The snare's notes are another channel's: a click on one draws.
    let pos = at(&harness, 100.0, 52);
    click(&mut harness, pos);
    assert_eq!(harness.state()[4..], [add(96, 48, 52)]);
}

#[test]
fn a_drag_with_the_draw_tool_draws_one_note_as_long_as_the_drag() {
    let mut harness = harness(fixture());
    let from = at(&harness, 250.0, 57);
    // 48 points are 72 ticks, and the end snaps to the step at 312.
    let to = [from + vec2(48.0, 0.0), from + vec2(96.0, 30.0)];
    drag(&mut harness, from, &to);
    harness.drop_at(to[1]);
    harness.run();
    let add = |length| {
        let add = Edit::AddNote {
            channel: 2,
            position: 240,
            length,
            key: 57,
        };
        dragged(add, 1)
    };
    assert_eq!(harness.state(), &[add(72), add(144)]);

    // The next click adds a note as long as the drawn one.
    let pos = at(&harness, 0.0, 60);
    click(&mut harness, pos);
    let Some(Command::Edit { edit, drag_id }) = harness.state().last() else {
        panic!("{:?}", harness.state());
    };
    let Edit::AddNote { length, .. } = edit else {
        panic!("{edit:?}");
    };
    assert_eq!((*length, *drag_id), (144, None));
}

#[test]
fn a_drag_on_a_note_moves_it_and_each_edit_says_the_whole_way() {
    let mut harness = harness(fixture());
    // The C4, taken a quarter of the way in.
    let from = at(&harness, 30.0, 48);
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    assert!(harness.state().is_empty(), "{:?}", harness.state());

    // 48 points are 72 ticks, two rows up are two keys, and 96 points
    // are six steps.
    let to = [from + vec2(48.0, -24.0), from + vec2(96.0, -24.0)];
    drag(&mut harness, from, &to);
    harness.drop_at(to[1]);
    harness.run();
    let ids = [BASS[0]];
    assert_eq!(
        harness.state(),
        &[
            // The note was not selected, so the drag selects it first.
            select(&ids, false),
            dragged(moved(&ids, 72, 2), 1),
            dragged(moved(&ids, 144, 2), 1),
        ]
    );

    // With Alt the note goes where the pointer does: 10 points are 15
    // ticks. The drag has a number of its own.
    harness.event(egui::Event::ModifiersChanged(Modifiers::ALT));
    drag(&mut harness, from, &[from + vec2(10.0, 12.0)]);
    harness.drop_at(from + vec2(10.0, 12.0));
    harness.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    harness.run();
    assert_eq!(
        harness.state()[3..],
        [select(&ids, false), dragged(moved(&ids, 15, -1), 2)]
    );
}

#[test]
fn a_drag_on_a_selected_note_takes_the_selection_and_its_edge_resizes() {
    let mut project = fixture();
    for id in [BASS[0], BASS[2]] {
        project.patterns[0].notes[id].flags |= Note::SELECTED;
    }
    let mut harness = harness(project);
    let from = at(&harness, 30.0, 48);
    drag(&mut harness, from, &[from + vec2(-64.0, 12.0)]);
    harness.drop_at(from + vec2(-64.0, 12.0));
    harness.run();
    let ids = [BASS[0], BASS[2]];
    // The app stops the group at the start of the pattern.
    assert_eq!(harness.state(), &[dragged(moved(&ids, -96, -1), 1)]);

    // The G4 ends at tick 144, which is at 96 points, and is not
    // selected. Its end goes a beat on.
    let edge = at(&harness, 144.0, 55) - vec2(3.0, 0.0);
    drag(&mut harness, edge, &[edge + vec2(64.0, 0.0)]);
    harness.drop_at(edge + vec2(64.0, 0.0));
    harness.run();
    let resized = Edit::ResizeNotes {
        ids: vec![BASS[1]],
        delta_ticks: 96,
    };
    assert_eq!(
        harness.state()[1..],
        [select(&[BASS[1]], false), dragged(resized, 2)]
    );
}

#[test]
fn a_rubber_band_selects_the_notes_it_spans() {
    let mut harness = harness(fixture());
    harness.get_by_label("Select").click();
    harness.run();
    // From above the G4 to under the D#4, short of the C4's row.
    let from = at(&harness, 80.0, 57);
    let to = at(&harness, 300.0, 50);
    drag(&mut harness, from, &[to]);
    assert!(harness.state().is_empty(), "{:?}", harness.state());
    harness.drop_at(to);
    harness.run();
    assert_eq!(harness.state(), &[select(&[BASS[1], BASS[2]], false)]);

    // With Shift the notes join the selection, and a click beside the
    // notes has nothing to let go of.
    harness.event(egui::Event::ModifiersChanged(Modifiers::SHIFT));
    let (low, to) = (at(&harness, 80.0, 50), at(&harness, 20.0, 47));
    drag(&mut harness, low, &[to]);
    harness.drop_at(to);
    harness.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    harness.run();
    assert_eq!(harness.state()[1..], [select(&[BASS[0]], true)]);
    click(&mut harness, from);
    assert_eq!(harness.state().len(), 2);

    // With the draw tool Ctrl makes a drag a rubber band.
    harness.get_by_label("Draw").click();
    harness.run();
    harness.event(egui::Event::ModifiersChanged(Modifiers::COMMAND));
    let to = at(&harness, 120.0, 54);
    drag(&mut harness, from, &[to]);
    harness.drop_at(to);
    harness.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    harness.run();
    assert_eq!(harness.state()[2..], [select(&[BASS[1]], false)]);
}

#[test]
fn the_right_button_and_the_tools_delete_and_mute() {
    let mut project = fixture();
    project.patterns[0].notes[BASS[2]].flags |= Note::MUTED;
    let mut harness = harness(project);
    let notes = [(30.0, 48), (110.0, 55), (250.0, 51)].map(|(tick, key)| at(&harness, tick, key));
    let delete = |id: usize| Edit::DeleteNotes { ids: vec![id] };
    let mute = |id: usize, muted| Edit::SetNotesMuted {
        ids: vec![id],
        muted,
    };

    right_click(&mut harness, notes[0]);
    assert_eq!(harness.state(), &[edit(delete(BASS[0]))]);
    // Beside the notes there is nothing to delete.
    let beside = at(&harness, 30.0, 58);
    right_click(&mut harness, beside);
    assert_eq!(harness.state().len(), 1);

    harness.get_by_label("Delete").click();
    harness.run();
    click(&mut harness, notes[1]);
    assert_eq!(harness.state()[1..], [edit(delete(BASS[1]))]);
    // A drag deletes each note it passes, once, in one drag.
    drag(&mut harness, notes[0], &[notes[1], notes[2], notes[1]]);
    harness.drop_at(notes[1]);
    harness.run();
    let passed: Vec<Command> = BASS.iter().map(|id| dragged(delete(*id), 1)).collect();
    assert_eq!(harness.state()[2..], passed);

    harness.get_by_label("Mute").click();
    harness.run();
    click(&mut harness, notes[2]);
    assert_eq!(harness.state()[5..], [edit(mute(BASS[2], false))]);
    // A drag makes of every note what it made of its first: the D#4 is
    // muted already.
    drag(&mut harness, notes[0], &[notes[1], notes[2]]);
    harness.drop_at(notes[2]);
    harness.run();
    assert_eq!(
        harness.state()[6..],
        [
            dragged(mute(BASS[0], true), 2),
            dragged(mute(BASS[1], true), 2)
        ]
    );
}

#[test]
fn the_keys_act_on_the_selection_under_the_pointer() {
    let mut project = fixture();
    for id in [BASS[0], BASS[1]] {
        project.patterns[0].notes[id].flags |= Note::SELECTED;
    }
    project.patterns[0].notes[BASS[0]].flags |= Note::MUTED;
    let mut harness = harness(project);
    let ids = vec![BASS[0], BASS[1]];
    let ctrl = Modifiers::COMMAND;

    // With the pointer somewhere else the keys are not the piano roll's.
    harness.remove_cursor();
    harness.key_press(Key::Delete);
    harness.key_press_modifiers(ctrl, Key::A);
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());

    harness.hover_at(at(&harness, 30.0, 58));
    harness.run();
    let pressed = [
        (Modifiers::NONE, Key::Delete),
        // One of the two is muted, so Alt+M mutes the other too.
        (Modifiers::ALT, Key::M),
        (Modifiers::NONE, Key::ArrowUp),
        (Modifiers::NONE, Key::ArrowDown),
        (Modifiers::NONE, Key::ArrowLeft),
        (Modifiers::NONE, Key::ArrowRight),
        (ctrl, Key::ArrowUp),
        (ctrl, Key::ArrowDown),
        (ctrl, Key::A),
        (ctrl, Key::D),
    ];
    for (modifiers, key) in pressed {
        harness.key_press_modifiers(modifiers, key);
        harness.run();
    }
    let muted = true;
    let expected = [
        Edit::DeleteNotes { ids: ids.clone() },
        Edit::SetNotesMuted {
            ids: ids.clone(),
            muted,
        },
        moved(&ids, 0, 1),
        moved(&ids, 0, -1),
        moved(&ids, -24, 0),
        moved(&ids, 24, 0),
        moved(&ids, 0, 12),
        moved(&ids, 0, -12),
        Edit::SelectNotes {
            ids: BASS.to_vec(),
            add: false,
        },
        Edit::SelectNotes {
            ids: Vec::new(),
            add: false,
        },
    ];
    assert_eq!(harness.state(), &expected.map(edit));
}

#[test]
fn the_lane_sets_the_value_it_shows() {
    let mut harness = harness(fixture());
    let lane = harness.get_by_label("Bass note velocity").rect();
    let notes = harness.get_by_label("Bass notes").rect();
    assert_eq!(lane.left(), notes.left());
    // Half way up, under the G4, which starts at 64 points.
    let pos = pos2(lane.left() + 70.0, lane.center().y);
    click(&mut harness, pos);
    let [
        Command::Edit {
            edit:
                Edit::SetNoteField {
                    id,
                    field: NoteField::Velocity,
                    value,
                },
            drag_id: None,
        },
    ] = harness.state()[..]
    else {
        panic!("{:?}", harness.state());
    };
    assert_eq!(id, BASS[1]);
    assert!((62..=66).contains(&value), "velocity {value}");
    // The G4 ends at 96 points, and the D#4 starts at 128: between them
    // there is no note to set.
    click(&mut harness, pos + vec2(40.0, 0.0));
    assert_eq!(harness.state().len(), 1);

    // The lane shows the pan when asked, and a drag along the bottom
    // pans every note it comes under to the left, in one drag.
    harness.get_by_value("Velocity").click();
    harness.run();
    harness.get_by_label("Pan").click();
    harness.run();
    let lane = harness.get_by_label("Bass note pan").rect();
    let y = lane.bottom() - 1.0;
    let from = pos2(lane.left() + 10.0, y);
    let to = [10.0, 30.0, 70.0, 80.0].map(|x| pos2(lane.left() + x, y));
    drag(&mut harness, from, &to);
    harness.drop_at(to[3]);
    harness.run();
    let pan = |id| {
        let (field, value) = (NoteField::Pan, 0);
        dragged(Edit::SetNoteField { id, field, value }, 1)
    };
    assert_eq!(harness.state()[1..], [pan(BASS[0]), pan(BASS[1])]);
}

#[test]
fn a_channel_that_plays_no_notes_shows_none() {
    let mut project = fixture();
    project.channels.remove(0);
    project.channels.swap(0, 2);
    // The vocal clip is the first channel now, and the one that shows.
    let state = UiState {
        project: Arc::new(project),
        position: 0,
        peaks: (0.0, 0.0),
        insert_peaks: Vec::new(),
        playing: false,
        song_mode: true,
        looping: false,
        pattern: 1,
        data_root: None,
        project_dir: None,
        sample_rate: None,
        last_error: None,
        busy: None,
        progress: None,
    };
    let mut harness = Harness::builder()
        .with_size(vec2(800.0, 400.0))
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new());
    harness.key_press(Key::F7);
    harness.run();
    assert!(
        harness
            .query_by_label("This channel plays no notes")
            .is_some()
    );
    // The list has the channels that play notes, and choosing one shows
    // its notes.
    harness.get_by_value("Vocal").click();
    harness.run();
    assert!(harness.query_by_label("Vocal").is_none());
    harness.get_by_label("Snare").click();
    harness.run();
    assert!(harness.query_by_label("Snare notes").is_some());
    assert!(harness.state().is_empty());
}

#[test]
fn a_paint_drag_lays_a_note_at_every_step_in_one_drag() {
    let mut harness = harness(fixture());
    harness.get_by_label("Paint").click();
    harness.run();
    // A first note is a step long, 24 ticks. The press is in the step
    // at 240, 48 points on is tick 322, in the step at 312, and the last
    // point is three rows down and at tick 394.
    let from = at(&harness, 250.0, 57);
    let to = [from + vec2(48.0, 0.0), from + vec2(96.0, 30.0)];
    drag(&mut harness, from, &to);
    harness.drop_at(to[1]);
    harness.run();
    let paint = |notes: &[(u64, u8)]| {
        let notes = notes.to_vec();
        let (channel, length) = (2, 24);
        dragged(
            Edit::AddNotes {
                channel,
                length,
                notes,
            },
            1,
        )
    };
    let first = [(240, 57), (264, 57), (288, 57), (312, 57)];
    let mut all = first.to_vec();
    all.extend([(336, 54), (360, 54), (384, 54)]);
    assert_eq!(
        harness.state(),
        &[paint(&first[..1]), paint(&first), paint(&all)]
    );

    // On a note a click selects it, as the draw tool's does.
    let pos = at(&harness, 30.0, 48);
    click(&mut harness, pos);
    assert_eq!(harness.state()[3..], [select(&[BASS[0]], false)]);
}

#[test]
fn the_slice_tool_cuts_the_notes_on_the_keys_it_crosses() {
    let mut harness = harness(fixture());
    harness.get_by_label("Slice").click();
    harness.run();
    // Tick 130 snaps to 120, inside the G4 and past the end of the C4.
    let from = at(&harness, 130.0, 56);
    let to = at(&harness, 130.0, 47);
    drag(&mut harness, from, &[to]);
    assert!(harness.state().is_empty(), "{:?}", harness.state());
    harness.drop_at(to);
    harness.run();
    let slice = |ids: &[usize], at| {
        let ids = ids.to_vec();
        edit(Edit::SliceNotes { ids, at })
    };
    assert_eq!(harness.state(), &[slice(&[BASS[1]], 120)]);

    // A click cuts the note under it.
    let pos = at(&harness, 250.0, 51);
    click(&mut harness, pos);
    assert_eq!(harness.state()[1..], [slice(&[BASS[2]], 240)]);
}

#[test]
fn the_tools_menu_and_its_keys_edit_the_selection() {
    let mut project = fixture();
    for id in [BASS[0], BASS[2]] {
        project.patterns[0].notes[id].flags |= Note::SELECTED;
    }
    let mut harness = harness(project);
    let ids = vec![BASS[0], BASS[2]];
    for entry in ["Glue", "Arpeggiate"] {
        harness.get_by_label("Tools").click();
        harness.run();
        harness.get_by_label(entry).click();
        harness.run();
    }
    let step = 24;
    assert_eq!(
        harness.state(),
        &[
            edit(Edit::GlueNotes { ids: ids.clone() }),
            edit(Edit::ArpeggiateNotes {
                ids: ids.clone(),
                step
            }),
        ]
    );

    harness.hover_at(at(&harness, 30.0, 58));
    harness.run();
    for (modifiers, key) in [
        (Modifiers::COMMAND, Key::Q),
        (Modifiers::SHIFT, Key::Q),
        (Modifiers::ALT, Key::U),
        (Modifiers::ALT, Key::S),
    ] {
        harness.key_press_modifiers(modifiers, key);
        harness.run();
    }
    let quantized = |lengths| Edit::QuantizeNotes {
        ids: ids.clone(),
        step,
        lengths,
    };
    let expected = [
        quantized(true),
        quantized(false),
        Edit::ChopNotes {
            ids: ids.clone(),
            step,
        },
        // A 64th note at PPQ 96.
        Edit::StrumNotes {
            ids: ids.clone(),
            delay: 6,
        },
    ];
    assert_eq!(harness.state()[2..], expected.map(edit));
}

fn choose_snap(harness: &mut Harness<'_, Vec<Command>>, from: &str, to: &str) {
    harness.get_by_value(from).click();
    harness.run();
    harness.get_by_label(to).click();
    harness.run();
}

#[test]
fn the_snap_menu_sets_where_a_note_lands() {
    let mut harness = harness(fixture());
    let add = |position| {
        edit(Edit::AddNote {
            channel: 2,
            position,
            length: 24,
            key: 57,
        })
    };
    let pos = at(&harness, 300.0, 57);
    for (from, to) in [("Line", "1/2 beat"), ("1/2 beat", "Bar"), ("Bar", "(none)")] {
        choose_snap(&mut harness, from, to);
        click(&mut harness, pos);
    }
    // Tick 300 is in the half beat from 288, in the first bar, and on its
    // own tick.
    assert_eq!(harness.state(), &[add(288), add(0), add(300)]);

    // With the bar the arrows move the selection a bar.
    let mut project = fixture();
    project.patterns[0].notes[BASS[0]].flags |= Note::SELECTED;
    let mut harness = self::harness(project);
    choose_snap(&mut harness, "Line", "Bar");
    harness.hover_at(at(&harness, 30.0, 58));
    harness.key_press(Key::ArrowRight);
    harness.run();
    assert_eq!(harness.state(), &[edit(moved(&[BASS[0]], 384, 0))]);
}

#[test]
fn the_cell_snap_keeps_a_note_s_place_in_its_cell() {
    // The G4 starts 4 ticks into its step.
    let mut project = fixture();
    project.patterns[0].notes[BASS[1]].position = 100;
    let mut harness = harness(project);
    // 30 points are 45 ticks: the start goes to the line at 144, 44 on.
    let from = at(&harness, 110.0, 55);
    let to = from + vec2(30.0, 0.0);
    drag(&mut harness, from, &[to]);
    harness.drop_at(to);
    harness.run();
    let ids = [BASS[1]];
    assert_eq!(
        harness.state(),
        &[select(&ids, false), dragged(moved(&ids, 44, 0), 1)]
    );
    // With the cell snap it goes two whole steps, and stays 4 ticks in.
    choose_snap(&mut harness, "Line", "Cell");
    drag(&mut harness, from, &[to]);
    harness.drop_at(to);
    harness.run();
    assert_eq!(
        harness.state()[2..],
        [select(&ids, false), dragged(moved(&ids, 48, 0), 2)]
    );
}

#[test]
fn a_scale_snaps_the_notes_to_its_keys() {
    let mut harness = harness(fixture());
    // Without a scale there is nothing to snap to.
    harness.get_by_label("Tools").click();
    harness.run();
    harness.get_by_label("Snap to scale").click();
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());
    harness.key_press(Key::Escape);
    harness.run();

    for (menu, entry) in [("Scale", "D"), ("D Major", "Dorian")] {
        harness.get_by_label(menu).click();
        harness.run();
        harness.get_by_label(entry).click();
        harness.run();
    }
    assert!(harness.query_by_label("D Dorian").is_some());
    harness.get_by_label("Tools").click();
    harness.run();
    harness.get_by_label("Snap to scale").click();
    harness.run();
    // D dorian has the keys of C major.
    let snapped = Edit::SnapNotesToScale {
        ids: BASS.to_vec(),
        keys: 0b1010_1011_0101,
    };
    assert_eq!(harness.state(), &[edit(snapped)]);
}

#[test]
fn without_a_selection_a_tool_takes_every_note_of_the_channel() {
    let mut harness = harness(fixture());
    harness.get_by_label("Tools").click();
    harness.run();
    harness.get_by_label("Quantize").click();
    harness.run();
    let quantized = Edit::QuantizeNotes {
        ids: BASS.to_vec(),
        step: 24,
        lengths: true,
    };
    assert_eq!(harness.state(), &[edit(quantized)]);
}
