//! The channel rack drawn from a project built in code.
//!
//! The snapshot is `tests/snapshots/channel_rack.png`, see `shell.rs` for
//! how to write it again.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use flp::event::layout::Levels;
use model::{Channel, ChannelKind, Note, Pattern, Project};
use std::sync::Arc;
use ui::{Command, Edit, UiState};

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

/// A bar of drums and bass at PPQ 96: a kick on every beat, a snare on
/// the second and the fourth, quieter the second time, a bass line from
/// the piano roll, and a vocal clip that plays no notes.
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
    project.channels[0].levels = Some(Levels {
        pan: 3200,
        volume: 8000,
        unknown_8: [0; 4],
        mod_x: 256,
        mod_y: 0,
        unknown_20: [0; 4],
        len: 24,
    });
    project.channels[0].routed_to = Some(3);
    project.channels[1].enabled = Some(false);
    let mut notes: Vec<Note> = (0..4).map(|beat| note(0, beat * 96, 0, 60)).collect();
    notes.extend([note(1, 96, 0, 60), note(1, 288, 0, 60)]);
    notes[5].velocity = 50;
    notes.extend([
        note(2, 0, 96, 48),
        note(2, 96, 48, 55),
        note(2, 192, 144, 51),
    ]);
    notes.sort_by_key(|note| note.position);
    project.patterns.push(Pattern {
        index: 1,
        name: Some("Beat".to_owned()),
        notes,
        ..Pattern::default()
    });
    project
}

fn state() -> UiState {
    UiState {
        project: Arc::new(fixture()),
        position: 0,
        peaks: (0.0, 0.0),
        insert_peaks: Vec::new(),
        playing: false,
        song_mode: true,
        looping: false,
        pattern: 1,
        data_root: None,
        project_dir: None,
        sample_rate: Some(48_000),
        last_error: None,
        busy: None,
        progress: None,
        loop_range: None,
    }
}

/// The shell with the channel rack in front.
fn harness() -> Harness<'static, Vec<Command>> {
    let state = state();
    // The harness runs a frame for every event and a click is four events,
    // so frames this short make two clicks a double click.
    let mut harness = Harness::builder()
        .with_step_dt(0.05)
        .with_size(egui::vec2(640.0, 300.0))
        .with_theme(egui::Theme::Dark)
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new());
    harness.key_press(egui::Key::F6);
    harness.run();
    harness
}

/// The middle of a step of a channel's row. A step is 14 points wide.
fn step(harness: &Harness<'_, Vec<Command>>, channel: &str, step: u32) -> egui::Pos2 {
    let row = harness.get_by_label(&format!("{channel} steps")).rect();
    egui::pos2(row.left() + step as f32 * 14.0 + 7.0, row.center().y)
}

fn click(harness: &mut Harness<'_, Vec<Command>>, pos: egui::Pos2) {
    harness.hover_at(pos);
    harness.drag_at(pos);
    harness.drop_at(pos);
    harness.run();
}

fn right_click(harness: &mut Harness<'_, Vec<Command>>, pos: egui::Pos2) {
    harness.hover_at(pos);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.run();
}

fn edit(edit: Edit) -> Command {
    Command::Edit {
        edit,
        drag_id: None,
    }
}

fn set_step(channel: u16, step: u32, on: bool) -> Edit {
    Edit::SetStep { channel, step, on }
}

#[test]
fn the_rack_looks_like_its_snapshot() {
    let mut harness = harness();
    harness.get_by_label("Graph").click();
    harness.remove_cursor();
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());
    harness.snapshot("channel_rack");
}

#[test]
fn a_click_on_a_step_lights_it_or_clears_it() {
    let mut harness = harness();
    let dark = step(&harness, "Snare", 2);
    click(&mut harness, dark);
    assert_eq!(harness.state(), &[edit(set_step(1, 2, true))]);
    // The kick's second beat is lit.
    let lit = step(&harness, "Kick", 4);
    click(&mut harness, lit);
    assert_eq!(harness.state().last(), Some(&edit(set_step(0, 4, false))));

    // A right click clears, and a dark step has nothing to clear.
    right_click(&mut harness, lit);
    assert_eq!(harness.state().len(), 3);
    assert_eq!(harness.state().last(), Some(&edit(set_step(0, 4, false))));
    right_click(&mut harness, dark);
    assert_eq!(harness.state().len(), 3);

    // The bass has notes from the piano roll and the vocal is a clip:
    // neither takes steps.
    for channel in ["Bass", "Vocal"] {
        let pos = step(&harness, channel, 2);
        click(&mut harness, pos);
        right_click(&mut harness, pos);
    }
    assert_eq!(harness.state().len(), 3);
}

#[test]
fn a_drag_paints_what_its_first_step_became() {
    let mut harness = harness();
    // From the snare's dark first step over its lit fifth to the sixth.
    let from = step(&harness, "Snare", 0);
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    for to in 1..=5 {
        harness.hover_at(step(&harness, "Snare", to));
        harness.step();
    }
    harness.drop_at(step(&harness, "Snare", 5));
    harness.run();
    let painted: Vec<Command> = [0, 1, 2, 3, 5]
        .into_iter()
        .map(|step| Command::Edit {
            edit: set_step(1, step, true),
            drag_id: Some(1),
        })
        .collect();
    assert_eq!(harness.state(), &painted);

    // A drag from a lit step clears, and has a number of its own.
    let from = step(&harness, "Kick", 4);
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    let to = step(&harness, "Kick", 8);
    harness.hover_at(to);
    harness.step();
    harness.drop_at(to);
    harness.run();
    let cleared = |step| Command::Edit {
        edit: set_step(0, step, false),
        drag_id: Some(2),
    };
    assert_eq!(harness.state()[5..], [cleared(4), cleared(8)]);
}

#[test]
fn a_drag_on_the_volume_knob_is_one_drag_of_many_values() {
    let mut harness = harness();
    let knob = harness.get_by_label("Kick volume");
    assert_eq!(knob.accesskit_node().numeric_value(), Some(8000.0));
    let from = knob.rect().center();
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    // 150 points are the whole range of 12800, so 15 points up are 1280.
    for up in [15.0, 30.0] {
        harness.hover_at(from - egui::vec2(0.0, up));
        harness.step();
    }
    harness.drop_at(from - egui::vec2(0.0, 30.0));
    harness.run();
    let volume = |volume| Command::Edit {
        edit: Edit::SetChannelVolume { channel: 0, volume },
        drag_id: Some(1),
    };
    assert_eq!(harness.state(), &[volume(9280), volume(10_560)]);

    // With Ctrl the knob turns a tenth as fast.
    harness.hover_at(from);
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
    harness.drag_at(from);
    harness.step();
    harness.hover_at(from + egui::vec2(0.0, 50.0));
    harness.step();
    harness.drop_at(from + egui::vec2(0.0, 50.0));
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    harness.run();
    let fine = Command::Edit {
        edit: Edit::SetChannelVolume {
            channel: 0,
            volume: 7573,
        },
        drag_id: Some(2),
    };
    assert_eq!(harness.state().last(), Some(&fine));
    assert_eq!(harness.state().len(), 3);

    // A double click puts the default back, and is no drag.
    for _ in 0..2 {
        harness.hover_at(from);
        harness.drag_at(from);
        harness.drop_at(from);
    }
    harness.run();
    let default = edit(Edit::SetChannelVolume {
        channel: 0,
        volume: 10_000,
    });
    assert_eq!(harness.state().last(), Some(&default));
    assert_eq!(harness.state().len(), 4);
}

#[test]
fn the_switch_the_pan_and_the_insert_send_their_edits() {
    let mut harness = harness();
    // The snare is off.
    let switch = harness.get_by_label("Snare on");
    assert_eq!(switch.accesskit_node().role(), Role::CheckBox);
    switch.click();
    harness.run();
    harness.get_by_label("Kick on").click();
    harness.run();
    let enabled = |channel, enabled| edit(Edit::SetChannelEnabled { channel, enabled });
    assert_eq!(harness.state(), &[enabled(1, true), enabled(0, false)]);

    // The kick is half way to the left. 75 points down are half the range.
    let from = harness.get_by_label("Kick pan").rect().center();
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    harness.hover_at(from + egui::vec2(0.0, 75.0));
    harness.step();
    harness.drop_at(from + egui::vec2(0.0, 75.0));
    harness.run();
    let pan = Command::Edit {
        edit: Edit::SetChannelPan { channel: 0, pan: 0 },
        drag_id: Some(1),
    };
    assert_eq!(harness.state().last(), Some(&pan));

    // The kick plays into insert 3, and six points up are an insert.
    let from = harness.get_by_label("Kick insert").rect().center();
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    harness.hover_at(from - egui::vec2(0.0, 12.0));
    harness.step();
    harness.drop_at(from - egui::vec2(0.0, 12.0));
    harness.run();
    let insert = Command::Edit {
        edit: Edit::SetChannelInsert {
            channel: 0,
            insert: 5,
        },
        drag_id: Some(2),
    };
    assert_eq!(harness.state().last(), Some(&insert));
    assert_eq!(harness.state().len(), 4);
}

#[test]
fn the_graph_shows_the_selected_channel_and_sets_velocities() {
    let mut harness = harness();
    assert!(harness.query_by_label("Kick velocity").is_none());
    harness.get_by_label("Graph").click();
    harness.run();
    // The first channel is the selected one, and the arrows move on.
    let lane = harness.get_by_label("Kick velocity").rect();
    harness.key_press(egui::Key::ArrowUp);
    harness.run();
    assert!(harness.query_by_label("Kick velocity").is_some());
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    assert!(harness.query_by_label("Snare velocity").is_some());
    harness.get_by_label("Kick").click();
    harness.run();
    assert!(harness.query_by_label("Kick velocity").is_some());

    // The lane is under the steps, and a click half way up the kick's
    // second beat sets half the velocity.
    let steps = harness.get_by_label("Kick steps").rect();
    assert_eq!(lane.left(), steps.left());
    let pos = egui::pos2(lane.left() + 4.5 * 14.0, lane.center().y);
    click(&mut harness, pos);
    let [
        Command::Edit {
            edit:
                Edit::SetStepVelocity {
                    channel: 0,
                    step: 4,
                    velocity,
                },
            drag_id: None,
        },
    ] = harness.state()[..]
    else {
        panic!("{:?}", harness.state());
    };
    assert!((62..=66).contains(&velocity), "velocity {velocity}");
    // A dark step has no note to set.
    click(&mut harness, pos + egui::vec2(14.0, 0.0));
    assert_eq!(harness.state().len(), 1);
}
