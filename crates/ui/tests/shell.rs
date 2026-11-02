//! The shell drawn from a project built in code.
//!
//! The snapshot is `tests/snapshots/shell.png`. Run the tests with
//! `UPDATE_SNAPSHOTS=true` to write it again after a change that is meant
//! to show. Drawing it needs a wgpu adapter, which on Linux can be Mesa's
//! lavapipe.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use model::{Arrangement, Channel, ChannelKind, ItemSource, Pattern, PlaylistItem, Project, Track};
use std::sync::Arc;
use ui::{Command, Dialog, Edit, UiState};

const BAR: i32 = 4 * 96;

/// An item of `bars` bars on a track, where the first track is 1.
fn item(source: ItemSource, track: i16, bar: i32, bars: i32) -> PlaylistItem {
    let mut item = PlaylistItem::new(source, bar * BAR, bars * BAR);
    item.track_word = 500 - track;
    item
}

/// A song of eight bars at 140 BPM: a drum pattern on track 1, a bass
/// pattern on track 2, the second half of it muted, and a vocal clip on
/// track 4.
fn fixture() -> Project {
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 2,
        ppq: 96,
    });
    project.writer_version = Some("24.1.1.4285".to_owned());
    project.tempo_milli = Some(140_000);
    project.time_sig_num = Some(4);
    project.time_sig_beat = Some(4);
    project.title = Some("Fixture song".to_owned());
    project.channels = vec![
        Channel {
            index: 0,
            kind: Some(ChannelKind::Sampler),
            name: Some("Kick".to_owned()),
            ..Channel::default()
        },
        Channel {
            index: 1,
            kind: Some(ChannelKind::AudioClip),
            name: Some("Vocal".to_owned()),
            color: Some(0x0040_a0e0),
            ..Channel::default()
        },
    ];
    project.patterns = vec![
        Pattern {
            index: 1,
            name: Some("Drums".to_owned()),
            color: Some(0x0050_70d0),
            ..Pattern::default()
        },
        Pattern {
            index: 2,
            color: Some(0x00d0_9050),
            ..Pattern::default()
        },
    ];
    let mut muted = item(ItemSource::Pattern(2), 2, 6, 2);
    muted.set_muted(true);
    project.arrangements.push(Arrangement {
        index: 0,
        items: vec![
            item(ItemSource::Pattern(1), 1, 0, 4),
            item(ItemSource::Pattern(1), 1, 4, 4),
            item(ItemSource::Pattern(2), 2, 2, 4),
            muted,
            item(ItemSource::Channel(1), 4, 1, 6),
        ],
        tracks: vec![Track {
            index: 2,
            name: Some("Bass".to_owned()),
            ..Track::default()
        }],
        ..Arrangement::default()
    });
    project
}

/// The fixture playing in song mode, two bars and a beat in.
fn state() -> UiState {
    UiState {
        project: Arc::new(fixture()),
        position: 2 * 384 + 96,
        peaks: (0.5, 1.3),
        insert_peaks: Vec::new(),
        playing: true,
        song_mode: true,
        looping: false,
        pattern: 1,
        data_root: None,
        project_dir: None,
        sample_rate: Some(48_000),
        last_error: Some("2 samples not found under /samples".to_owned()),
        busy: None,
        progress: None,
        loop_range: None,
    }
}

fn harness(state: UiState) -> Harness<'static, Vec<Command>> {
    Harness::builder()
        .with_size(egui::vec2(900.0, 360.0))
        .with_theme(egui::Theme::Dark)
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new())
}

#[test]
fn the_shell_looks_like_its_snapshot() {
    let mut harness = harness(state());
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());
    harness.snapshot("shell");
}

#[test]
fn the_transport_buttons_send_their_commands() {
    let mut harness = harness(UiState {
        playing: false,
        song_mode: false,
        ..state()
    });
    harness.get_by_label("Play").click();
    harness.run();
    assert_eq!(harness.state(), &[Command::Play]);

    harness.get_by_label("Stop").click();
    harness.run();
    harness.get_by_label("SONG").click();
    harness.run();
    // Pattern mode is on already, so its button sends nothing.
    harness.get_by_label("PAT").click();
    harness.run();
    harness.get_by_label("LOOP").click();
    harness.run();
    assert_eq!(
        harness.state(),
        &[
            Command::Play,
            Command::Stop,
            Command::ToggleSongMode,
            Command::ToggleLoop
        ]
    );
}

#[test]
fn the_pattern_selector_lists_the_patterns() {
    let mut harness = harness(state());
    harness.run();
    // The selector shows the name of pattern 1, and the list has the
    // pattern without a name by its number.
    let selector = harness.get_by_role(Role::ComboBox);
    assert_eq!(selector.value().as_deref(), Some("Drums"));
    selector.click();
    harness.run();
    harness.get_by_label("Pattern 2").click();
    harness.run();
    assert_eq!(harness.state(), &[Command::SelectPattern(2)]);
}

/// The shell in frames short enough that two clicks make a double click:
/// the harness runs a frame for every event, and a click is four events.
fn quick_harness(state: UiState) -> Harness<'static, Vec<Command>> {
    Harness::builder()
        .with_step_dt(0.05)
        .with_size(egui::vec2(900.0, 360.0))
        .with_theme(egui::Theme::Dark)
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new())
}

fn tempo(milli: u32, drag_id: Option<u64>) -> Command {
    Command::Edit {
        edit: Edit::SetTempo { milli },
        drag_id,
    }
}

#[test]
fn a_drag_on_the_tempo_changes_it_a_bpm_a_point() {
    let mut harness = harness(state());
    harness.run();
    let readout = harness.get_by_label("Tempo");
    assert_eq!(readout.accesskit_node().role(), Role::Slider);
    assert_eq!(readout.accesskit_node().numeric_value(), Some(140.0));
    let from = readout.rect().center();
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    for up in [8.0, 20.0] {
        harness.hover_at(from - egui::vec2(0.0, up));
        harness.step();
    }
    harness.drop_at(from - egui::vec2(0.0, 20.0));
    harness.run();
    assert_eq!(
        harness.state(),
        &[tempo(148_000, Some(1)), tempo(160_000, Some(1))]
    );

    // With Ctrl, a tenth of a BPM a point, and down is slower.
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
    harness.drag_at(from);
    harness.step();
    harness.hover_at(from + egui::vec2(0.0, 15.0));
    harness.step();
    harness.drop_at(from + egui::vec2(0.0, 15.0));
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    harness.run();
    assert_eq!(harness.state().last(), Some(&tempo(138_500, Some(2))));
    assert_eq!(harness.state().len(), 3);
}

#[test]
fn a_double_click_on_the_tempo_types_it() {
    let mut harness = quick_harness(state());
    harness.run();
    let open = |harness: &mut Harness<'_, Vec<Command>>| {
        let at = harness.get_by_label("Tempo").rect().center();
        for _ in 0..2 {
            harness.hover_at(at);
            harness.drag_at(at);
            harness.drop_at(at);
        }
        harness.run();
    };
    open(&mut harness);
    let field = harness.get_by_role(Role::TextInput);
    assert_eq!(field.value().as_deref(), Some("140.000"));
    // Space is the field's while it is typed in, and plays nothing.
    harness.key_press(egui::Key::Space);
    harness.run();
    for _ in 0.."140.000 ".len() {
        harness.key_press(egui::Key::Backspace);
        harness.run();
    }
    harness.get_by_role(Role::TextInput).type_text("97.5");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state(), &[tempo(97_500, None)]);
    assert!(harness.query_by_role(Role::TextInput).is_none());

    // Escape leaves the tempo, and so does what is not a number. Past
    // FL's range is its end.
    for (text, key) in [("60", egui::Key::Escape), ("x", egui::Key::Enter)] {
        open(&mut harness);
        harness.get_by_role(Role::TextInput).type_text(text);
        harness.run();
        harness.key_press(key);
        harness.run();
    }
    assert_eq!(harness.state().len(), 1);
    open(&mut harness);
    harness.key_press(egui::Key::Home);
    harness.run();
    harness.get_by_role(Role::TextInput).type_text("9");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().last(), Some(&tempo(522_000, None)));
}

fn edit_of(edit: Edit) -> Command {
    Command::Edit {
        edit,
        drag_id: None,
    }
}

#[test]
fn a_new_pattern_takes_the_lowest_free_index_and_is_selected() {
    let mut harness = harness(state());
    harness.run();
    harness.get_by_role(Role::ComboBox).click();
    harness.run();
    harness.get_by_label_contains("New pattern").click();
    harness.run();
    harness.key_press(egui::Key::F4);
    harness.run();
    let new = [
        edit_of(Edit::AddPattern { index: 3 }),
        Command::SelectPattern(3),
    ];
    assert_eq!(harness.state(), &[new.clone(), new].concat());
}

#[test]
fn a_clone_of_the_selected_pattern_is_selected() {
    let mut harness = harness(UiState {
        pattern: 2,
        ..state()
    });
    harness.run();
    harness.get_by_role(Role::ComboBox).click();
    harness.run();
    harness.get_by_label("Clone pattern").click();
    harness.run();
    let clone = edit_of(Edit::ClonePattern { from: 2, to: 3 });
    assert_eq!(harness.state(), &[clone, Command::SelectPattern(3)]);

    // A pattern the project does not have has nothing to clone.
    let mut harness = crate::harness(UiState {
        pattern: 7,
        ..state()
    });
    harness.run();
    harness.get_by_role(Role::ComboBox).click();
    harness.run();
    harness.get_by_label("Clone pattern").click();
    harness.run();
    assert!(harness.state().is_empty());
}

#[test]
fn f2_or_the_list_renames_the_selected_pattern() {
    let mut harness = harness(state());
    harness.run();
    assert!(harness.query_by_role(Role::TextInput).is_none());
    harness.key_press(egui::Key::F2);
    harness.run();
    let field = harness.get_by_role(Role::TextInput);
    assert_eq!(field.value().as_deref(), Some("Drums"));
    field.type_text(" 2 ");
    harness.run();
    // The name is trimmed.
    harness.key_press(egui::Key::Enter);
    harness.run();
    let named = |pattern, name: &str| {
        let name = name.to_owned();
        edit_of(Edit::SetPatternName { pattern, name })
    };
    assert_eq!(harness.state(), &[named(1, "Drums 2")]);
    assert!(harness.query_by_role(Role::ComboBox).is_some());

    // Escape leaves the name, and so does the name it had.
    for key in [egui::Key::Escape, egui::Key::Enter] {
        harness.key_press(egui::Key::F2);
        harness.run();
        if key == egui::Key::Escape {
            harness.get_by_role(Role::TextInput).type_text("s");
            harness.run();
        }
        harness.key_press(key);
        harness.run();
    }
    assert_eq!(harness.state().len(), 1);

    // A pattern without a name starts from an empty field.
    let mut harness = crate::harness(UiState {
        pattern: 2,
        ..state()
    });
    harness.run();
    harness.get_by_role(Role::ComboBox).click();
    harness.run();
    harness.get_by_label_contains("Rename pattern").click();
    harness.run();
    let field = harness.get_by_role(Role::TextInput);
    assert_eq!(field.value().as_deref(), Some(""));
    field.type_text("Bass");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state(), &[named(2, "Bass")]);
}

#[test]
fn a_render_shows_how_far_it_is() {
    let mut harness = harness(UiState {
        playing: false,
        busy: Some("Rendering song.wav".to_owned()),
        progress: Some(0.25),
        ..state()
    });
    harness.run();
    assert!(harness.query_by_label("Rendering song.wav").is_some());
    let bar = harness.get_by_role(Role::ProgressIndicator);
    assert_eq!(bar.accesskit_node().numeric_value(), Some(25.0));
}

#[test]
fn the_menu_asks_for_paths_and_quits() {
    let mut harness = harness(state());
    for (entry, command) in [
        ("Open…", Command::Choose(Dialog::Open)),
        ("Save", Command::Save),
        ("Save As…", Command::Choose(Dialog::SaveAs)),
        ("Render to WAV…", Command::Choose(Dialog::Render)),
        ("Quit", Command::Quit),
    ] {
        harness.get_by_label("File").click();
        harness.run();
        harness.get_by_label(entry).click();
        harness.run();
        assert_eq!(harness.state().last(), Some(&command));
    }
    assert_eq!(harness.state().len(), 5);
}

#[test]
fn the_keys_undo_redo_and_save() {
    let mut harness = harness(state());
    let ctrl = egui::Modifiers::COMMAND;
    for (modifiers, key) in [
        (ctrl, egui::Key::Z),
        (ctrl | egui::Modifiers::SHIFT, egui::Key::Z),
        (ctrl, egui::Key::Y),
        (ctrl, egui::Key::S),
    ] {
        harness.key_press_modifiers(modifiers, key);
        harness.run();
    }
    assert_eq!(
        harness.state(),
        &[Command::Undo, Command::Redo, Command::Redo, Command::Save]
    );
}

#[test]
fn a_click_on_the_ruler_seeks() {
    let mut harness = harness(state());
    harness.run();
    // The ruler starts 96 points in, right of the track names, and the
    // eight bars share the other 804. Half way along is the start of bar 5.
    let ruler = harness.get_by_label("Playlist").rect().bottom() + 9.0;
    let at = egui::pos2(96.0 + 402.0, ruler);
    harness.hover_at(at);
    harness.drag_at(at);
    harness.drop_at(at);
    harness.run();
    let [Command::Seek(tick)] = harness.state()[..] else {
        panic!("{:?}", harness.state());
    };
    assert!((4 * 384 - 4..=4 * 384 + 4).contains(&tick), "tick {tick}");
}

/// A point on the playlist: `bars` from the start of the song, in the
/// middle of the row of `track`, where the first track is 1. The rows are
/// 22 points high and start 3 points under the ruler, which is 18 high.
fn at(harness: &Harness<'_, Vec<Command>>, bars: f32, track: usize) -> egui::Pos2 {
    let ruler = harness.get_by_label("Playlist").rect().bottom();
    let row = (track - 1) as f32;
    egui::pos2(96.0 + bars * 100.5, ruler + 21.0 + row * 22.0 + 11.0)
}

fn button(harness: &Harness<'_, Vec<Command>>, pos: egui::Pos2, pressed: bool) {
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
}

fn edit(edit: Edit) -> Command {
    Command::Edit {
        edit,
        drag_id: None,
    }
}

#[test]
fn a_click_selects_the_item_under_it() {
    let mut harness = harness(state());
    harness.run();
    // The bass pattern on track 2 runs from bar 3 to bar 7, and is the
    // third item of the list.
    let bass = at(&harness, 3.0, 2);
    harness.hover_at(bass);
    harness.drag_at(bass);
    harness.drop_at(bass);
    harness.run();
    let select = |ids: Vec<usize>, add| edit(Edit::SelectItems { ids, add });
    assert_eq!(harness.state(), &[select(vec![2], false)]);

    // With Shift the item joins the selection.
    let vocal = at(&harness, 2.0, 4);
    harness.hover_at(vocal);
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
    harness.drag_at(vocal);
    harness.drop_at(vocal);
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    harness.run();
    assert_eq!(harness.state().last(), Some(&select(vec![4], true)));

    // Track 3 is empty, and nothing is selected to let go of.
    let empty = at(&harness, 3.0, 3);
    harness.hover_at(empty);
    harness.drag_at(empty);
    harness.drop_at(empty);
    harness.run();
    assert_eq!(harness.state().len(), 2);
}

#[test]
fn a_drag_moves_the_item_by_whole_beats() {
    let mut harness = harness(state());
    harness.run();
    // The first drum pattern, taken half a bar in.
    let from = at(&harness, 0.5, 1);
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    assert!(harness.state().is_empty(), "{:?}", harness.state());

    // 60 points are 2.4 beats, so the item goes two beats along, and it
    // goes a row down.
    let to = from + egui::vec2(60.0, 22.0);
    harness.hover_at(to);
    harness.step();
    harness.drop_at(to);
    harness.run();
    assert_eq!(
        harness.state(),
        &[
            // The item was not selected, so the drag selects it first.
            edit(Edit::SelectItems {
                ids: vec![0],
                add: false
            }),
            Command::Edit {
                edit: Edit::MoveItems {
                    ids: vec![0],
                    delta_ticks: 192,
                    delta_tracks: 1
                },
                drag_id: Some(1)
            },
        ]
    );

    // A second drag has an ID of its own, and each change in where it is
    // sends the whole distance again.
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    for bars in [1.0, 2.0] {
        harness.hover_at(from + egui::vec2(bars * 100.5, 0.0));
        harness.step();
    }
    harness.drop_at(from + egui::vec2(201.0, 0.0));
    harness.run();
    let moved = |delta_ticks| Command::Edit {
        edit: Edit::MoveItems {
            ids: vec![0],
            delta_ticks,
            delta_tracks: 0,
        },
        drag_id: Some(2),
    };
    assert_eq!(harness.state()[3..], [moved(384), moved(768)]);
}

#[test]
fn a_right_click_mutes_and_unmutes() {
    let mut harness = harness(state());
    harness.run();
    // The bass pattern, and its muted second half from bar 7 on.
    for (bars, id, muted) in [(3.0, 2, true), (7.5, 3, false)] {
        let pos = at(&harness, bars, 2);
        harness.hover_at(pos);
        button(&harness, pos, true);
        button(&harness, pos, false);
        harness.run();
        let ids = vec![id];
        assert_eq!(
            harness.state().last(),
            Some(&edit(Edit::SetMuted { ids, muted }))
        );
    }
    assert_eq!(harness.state().len(), 2);
}

#[test]
fn the_window_keys_show_and_hide_their_windows() {
    let mut harness = harness(state());
    harness.run();
    assert!(harness.query_by_label("Mixer").is_none());
    harness.key_press(egui::Key::F9);
    harness.run();
    // The tab carries the name, and the fixture has no inserts to show.
    assert!(harness.query_by_label("Mixer").is_some());
    assert!(harness.query_by_label("The project has no mixer").is_some());
    assert!(harness.query_by_label("Playlist").is_some());

    harness.key_press(egui::Key::F9);
    harness.run();
    assert!(harness.query_by_label("Mixer").is_none());
    harness.key_press(egui::Key::F5);
    harness.run();
    // With every window closed, the welcome screen lists their keys.
    assert!(harness.query_by_label("No window is open").is_some());
    assert!(harness.state().is_empty());
}
