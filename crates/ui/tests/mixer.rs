//! The mixer drawn from a project built in code.
//!
//! The snapshot is `tests/snapshots/mixer.png`, see `shell.rs` for how to
//! write it again.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use flp::event::layout::{InitialValue, InsertFlags};
use model::{Channel, ChannelKind, Insert, Project, Slot};
use std::sync::Arc;
use ui::{Command, Edit, UiState};

/// A record of the 225 for a control of an insert, as FL 25 writes its ID.
fn record(insert: u32, slot: u32, control: u32, value: i32) -> InitialValue {
    InitialValue {
        unknown_0: [0; 4],
        rec_id: 0x7000_0000 + (insert << 22) + (slot << 16) + control,
        value,
    }
}

/// A mixer as FL 25 writes one, with the master, six inserts and the
/// current track behind them.
///
/// Insert 1 is the drums, pulled down and panned half way left. Insert 2
/// is the bass, with a plugin in its first slot at half its mix level, and
/// sends to insert 4 alone, at half the fader. Insert 3 has no name, is
/// switched off, and has a channel playing into it. Insert 4 is a reverb.
/// Inserts 5 and 6 are not in use.
fn fixture() -> Project {
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 1,
        ppq: 96,
    });
    project.writer_version = Some("25.1.0.4858".to_owned());
    project.insert_count = Some(8);
    let flags = |flags: u32| InsertFlags {
        unknown_0: [0; 4],
        flags,
        unknown_8: [0; 4],
        len: 12,
    };
    project.inserts = vec![
        Insert {
            flags: Some(flags(0b1100)),
            ..Insert::default()
        };
        8
    ];
    for (insert, name, colour) in [
        (1, "Drums", Some(0x0040_60d0)),
        (2, "Bass", Some(0x0060_c080)),
        (4, "Reverb", None),
    ] {
        project.inserts[insert].name = Some(name.to_owned());
        project.inserts[insert].color = colour;
    }
    project.inserts[2].routing = Some(vec![0, 0, 0, 0, 1]);
    project.inserts[2].slots = vec![Slot {
        index: 0,
        plugin_name: Some("Fruity Fast Dist".to_owned()),
        ..Slot::default()
    }];
    project.inserts[3].flags = Some(flags(0b0100));
    project.channels = vec![Channel {
        index: 0,
        kind: Some(ChannelKind::Sampler),
        target_insert: Some(3),
        ..Channel::default()
    }];
    project.initial_values = vec![
        record(1, 0, 0x1FC0, 9600),
        record(1, 0, 0x1FC1, -3200),
        record(2, 0, 0x2000 + 4, 8000),
        record(2, 0, 0x1F01, 6400),
    ];
    project
}

fn state() -> UiState {
    let mut insert_peaks = vec![(0.0, 0.0); 502];
    insert_peaks[..3].copy_from_slice(&[(0.5, 0.4), (1.3, 0.2), (0.05, 0.05)]);
    UiState {
        project: Arc::new(fixture()),
        position: 0,
        peaks: (0.5, 0.4),
        insert_peaks,
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
    }
}

/// The shell with the mixer in front.
fn harness() -> Harness<'static, Vec<Command>> {
    let state = state();
    // Frames this short make two clicks a double click, see the channel
    // rack's tests.
    let mut harness = Harness::builder()
        .with_step_dt(0.05)
        .with_size(egui::vec2(640.0, 360.0))
        .with_theme(egui::Theme::Dark)
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new());
    harness.key_press(egui::Key::F9);
    harness.run();
    harness
}

fn edit(edit: Edit) -> Command {
    Command::Edit {
        edit,
        drag_id: None,
    }
}

/// Press, move by `moves` one after the other, and let go.
fn drag(harness: &mut Harness<'_, Vec<Command>>, from: egui::Pos2, moves: &[egui::Vec2]) {
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    for by in moves {
        harness.hover_at(from + *by);
        harness.step();
    }
    harness.drop_at(from + *moves.last().unwrap());
    harness.run();
}

/// Select the bass with the arrows: the master is selected to begin with.
fn select_bass(harness: &mut Harness<'_, Vec<Command>>) {
    for _ in 0..2 {
        harness.key_press(egui::Key::ArrowRight);
        harness.run();
    }
    assert!(harness.query_by_label("Bass slots").is_some());
}

#[test]
fn the_mixer_looks_like_its_snapshot() {
    let mut harness = harness();
    select_bass(&mut harness);
    harness.remove_cursor();
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());
    harness.snapshot("mixer");
}

#[test]
fn the_strips_are_the_inserts_in_use_until_all_are_asked_for() {
    let mut harness = harness();
    for strip in ["Master", "Drums", "Bass", "Insert 3", "Reverb"] {
        assert!(harness.query_by_label(strip).is_some(), "{strip}");
    }
    assert!(harness.query_by_label("Insert 5").is_none());
    // The current track is behind the six inserts and has no strip.
    harness.get_by_label("All 6").click();
    harness.run();
    assert!(harness.query_by_label("Insert 5").is_some());
    assert!(harness.query_by_label("Insert 6").is_some());
    assert!(harness.query_by_label("Insert 7").is_none());
    assert!(harness.state().is_empty());
}

#[test]
fn a_drag_on_the_fader_is_one_drag_of_many_values() {
    let mut harness = harness();
    let fader = harness.get_by_label("Drums volume");
    assert_eq!(fader.accesskit_node().role(), Role::Slider);
    assert_eq!(fader.accesskit_node().numeric_value(), Some(9600.0));
    // The handle travels the height of the fader less its own, for the
    // whole 16000, so an eighth of that is 2000.
    let (from, travel) = (fader.rect().center(), fader.rect().height() - 8.0);
    let up = |part: f32| egui::vec2(0.0, -travel * part);
    drag(&mut harness, from, &[up(0.125), up(0.25)]);
    let volume = |volume, drag_id| Command::Edit {
        edit: Edit::SetInsertVolume { insert: 1, volume },
        drag_id,
    };
    assert_eq!(
        harness.state(),
        &[volume(11_600, Some(1)), volume(13_600, Some(1))]
    );

    // The fader ends at 16000, and a second drag has its own number.
    drag(&mut harness, from, &[up(0.75)]);
    assert_eq!(harness.state().last(), Some(&volume(16_000, Some(2))));
    assert_eq!(harness.state().len(), 3);

    // A double click puts it at 0 dB, and is no drag.
    for _ in 0..2 {
        harness.hover_at(from);
        harness.drag_at(from);
        harness.drop_at(from);
    }
    harness.run();
    assert_eq!(harness.state().last(), Some(&volume(12_800, None)));
    assert_eq!(harness.state().len(), 4);
}

#[test]
fn the_led_switches_its_insert_and_the_knob_pans_it() {
    let mut harness = harness();
    let led = harness.get_by_label("Drums on");
    assert_eq!(led.accesskit_node().role(), Role::CheckBox);
    led.click();
    harness.run();
    // Insert 3 is off.
    harness.get_by_label("Insert 3 on").click();
    harness.run();
    let enabled = |insert, enabled| edit(Edit::SetInsertEnabled { insert, enabled });
    assert_eq!(harness.state(), &[enabled(1, false), enabled(3, true)]);

    // The drums are half way to the left, and 75 points up are half the
    // knob's range.
    let knob = harness.get_by_label("Drums pan");
    assert_eq!(knob.accesskit_node().numeric_value(), Some(-3200.0));
    let from = knob.rect().center();
    drag(&mut harness, from, &[egui::vec2(0.0, -75.0)]);
    let pan = Command::Edit {
        edit: Edit::SetInsertPan {
            insert: 1,
            pan: 3200,
        },
        drag_id: Some(1),
    };
    assert_eq!(harness.state().last(), Some(&pan));
    assert_eq!(harness.state().len(), 3);
}

#[test]
fn a_click_and_the_arrows_select_the_insert() {
    let mut harness = harness();
    assert!(harness.query_by_label("Master slots").is_some());
    // The left arrow has nowhere to go from the master.
    harness.key_press(egui::Key::ArrowLeft);
    harness.run();
    assert!(harness.query_by_label("Master slots").is_some());
    select_bass(&mut harness);
    harness.key_press(egui::Key::ArrowLeft);
    harness.run();
    assert!(harness.query_by_label("Drums slots").is_some());

    harness.get_by_label("Reverb strip").click();
    harness.run();
    assert!(harness.query_by_label("Reverb slots").is_some());
    // The reverb is the last strip, and the arrows go on from a click.
    harness.key_press(egui::Key::ArrowRight);
    harness.run();
    assert!(harness.query_by_label("Reverb slots").is_some());
    harness.key_press(egui::Key::ArrowLeft);
    harness.run();
    assert!(harness.query_by_label("Insert 3 slots").is_some());
    assert!(harness.state().is_empty());
}

#[test]
fn the_arrows_under_the_strips_route_the_selected_insert() {
    let mut harness = harness();
    // The master sends nowhere.
    assert!(harness.query_by_label("Route to Drums").is_none());
    select_bass(&mut harness);
    assert!(harness.query_by_label("Route to Bass").is_none());
    // The bass sends to the reverb at half the fader, and nowhere else.
    let send = harness.get_by_label("Send to Reverb");
    assert_eq!(send.accesskit_node().numeric_value(), Some(8000.0));
    assert!(harness.query_by_label("Send to Master").is_none());

    for strip in ["Drums", "Reverb", "Master"] {
        harness.get_by_label(&format!("Route to {strip}")).click();
        harness.run();
    }
    let route = |to, on| edit(Edit::SetRoute { from: 2, to, on });
    assert_eq!(
        harness.state(),
        &[route(1, true), route(4, false), route(0, true)]
    );

    // 150 points are the knob's whole range of 16000.
    let from = harness.get_by_label("Send to Reverb").rect().center();
    drag(&mut harness, from, &[egui::vec2(0.0, 15.0)]);
    let level = Command::Edit {
        edit: Edit::SetSendLevel {
            from: 2,
            to: 4,
            level: 6400,
        },
        drag_id: Some(1),
    };
    assert_eq!(harness.state().last(), Some(&level));
    assert_eq!(harness.state().len(), 4);

    // The reverb gets the bass, so it cannot send to it. The pointer
    // moves off the knob first, which takes the knob's hint away.
    harness.hover_at(harness.get_by_label("Reverb strip").rect().center());
    harness.run();
    harness.get_by_label("Reverb strip").click();
    harness.run();
    harness.get_by_label("Route to Bass").click();
    harness.run();
    assert_eq!(harness.state().len(), 4);
    assert!(harness.query_by_label("Reverb slots").is_some());
    harness.get_by_label("Route to Drums").click();
    harness.run();
    let route = edit(Edit::SetRoute {
        from: 4,
        to: 1,
        on: true,
    });
    assert_eq!(harness.state().last(), Some(&route));
}

#[test]
fn a_click_on_a_name_renames_the_insert() {
    let mut harness = harness();
    assert!(harness.query_by_role(Role::TextInput).is_none());
    harness.get_by_label("Drums").click();
    harness.run();
    // The click selects the insert too.
    assert!(harness.query_by_label("Drums slots").is_some());
    let field = harness.get_by_role(Role::TextInput);
    assert_eq!(field.value().as_deref(), Some("Drums"));
    field.type_text(" kit");
    harness.run();
    // The arrows are the field's while a name is typed.
    harness.key_press(egui::Key::ArrowRight);
    harness.run();
    assert!(harness.query_by_label("Drums slots").is_some());
    harness.key_press(egui::Key::Enter);
    harness.run();
    let named = |insert, name: &str| {
        let name = name.to_owned();
        edit(Edit::SetInsertName { insert, name })
    };
    assert_eq!(harness.state(), &[named(1, "Drums kit")]);
    assert!(harness.query_by_role(Role::TextInput).is_none());

    // Escape leaves the name, and so does a name that is typed as it was.
    harness.get_by_label("Reverb").click();
    harness.run();
    harness.get_by_role(Role::TextInput).type_text("s");
    harness.run();
    harness.key_press(egui::Key::Escape);
    harness.run();
    harness.get_by_label("Bass").click();
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().len(), 1);

    // An insert without a name starts from an empty field.
    harness.get_by_label("Insert 3").click();
    harness.run();
    let field = harness.get_by_role(Role::TextInput);
    assert_eq!(field.value().as_deref(), Some(""));
    field.type_text("Lead");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().last(), Some(&named(3, "Lead")));

    // A click elsewhere takes the name: on a strip to the left of the
    // field, on one to its right, and on another name.
    for (strip, elsewhere, insert) in [
        ("Reverb", "Drums strip", 4),
        ("Drums", "Reverb strip", 1),
        ("Reverb", "Drums", 4),
    ] {
        harness.key_press(egui::Key::Escape);
        harness.run();
        let before = harness.state().len();
        harness.get_by_label(strip).click();
        harness.run();
        harness.get_by_role(Role::TextInput).type_text("!");
        harness.run();
        harness.get_by_label(elsewhere).click();
        harness.run();
        let name = format!("{strip}!");
        assert_eq!(harness.state()[before..], [named(insert, &name)]);
    }
    // The last click was on a name, which is a field now.
    assert_eq!(
        harness.get_by_role(Role::TextInput).value().as_deref(),
        Some("Drums")
    );
}

#[test]
fn the_slots_are_those_of_the_selected_insert() {
    let mut harness = harness();
    // The master's slots are empty, and an empty slot has no switch.
    assert!(harness.query_by_label("Fruity Fast Dist").is_none());
    harness.get_by_label("Slot 1 on").click();
    harness.run();
    assert!(harness.state().is_empty());

    select_bass(&mut harness);
    assert!(harness.query_by_label("Fruity Fast Dist").is_some());
    assert!(harness.query_by_label("Slot 2").is_some());
    let knob = harness.get_by_label("Slot 1 mix");
    assert_eq!(knob.accesskit_node().numeric_value(), Some(6400.0));
    let from = knob.rect().center();
    harness.get_by_label("Slot 1 on").click();
    harness.run();
    harness.get_by_label("Slot 2 on").click();
    harness.run();
    let switch = edit(Edit::SetSlotEnabled {
        insert: 2,
        slot: 0,
        enabled: false,
    });
    assert_eq!(harness.state(), &[switch]);

    // 150 points are the knob's whole range of 12800.
    drag(&mut harness, from, &[egui::vec2(0.0, -37.5)]);
    let mix = Command::Edit {
        edit: Edit::SetSlotMix {
            insert: 2,
            slot: 0,
            mix: 9600,
        },
        drag_id: Some(1),
    };
    assert_eq!(harness.state().last(), Some(&mix));
    assert_eq!(harness.state().len(), 2);
}
