//! The playlist's keys, on a project built in code. The mouse is tried in
//! `shell.rs`, where the playlist is the window that opens first.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use model::{Arrangement, Channel, ChannelKind, ItemSource, Pattern, PlaylistItem, Project};
use std::sync::Arc;
use ui::{Command, Edit, UiState};

const BAR: i32 = 4 * 96;

/// An item of `bars` bars on a track, where the first track is 1.
fn item(source: ItemSource, track: i16, bar: i32, bars: i32) -> PlaylistItem {
    let mut item = PlaylistItem::new(source, bar * BAR, bars * BAR);
    item.track_word = 500 - track;
    item
}

/// Eight bars: two drum patterns on track 1, a bass pattern on track 2,
/// a record FL deleted, and a vocal clip on track 4. The bass and the
/// vocal are selected.
fn fixture() -> Project {
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 1,
        ppq: 96,
    });
    project.writer_version = Some("24.1.1.4285".to_owned());
    project.time_sig_num = Some(4);
    project.time_sig_beat = Some(4);
    project.channels = vec![Channel {
        index: 0,
        kind: Some(ChannelKind::AudioClip),
        name: Some("Vocal".to_owned()),
        ..Channel::default()
    }];
    project.patterns = vec![
        Pattern {
            index: 1,
            name: Some("Drums".to_owned()),
            ..Pattern::default()
        },
        Pattern {
            index: 2,
            name: Some("Bass".to_owned()),
            ..Pattern::default()
        },
    ];
    let mut bass = item(ItemSource::Pattern(2), 2, 2, 4);
    bass.flags |= PlaylistItem::SELECTED;
    let mut deleted = item(ItemSource::Pattern(1), 3, 0, 1);
    deleted.source_id = u32::MAX;
    let mut vocal = item(ItemSource::Channel(0), 4, 1, 6);
    vocal.flags |= PlaylistItem::SELECTED;
    project.arrangements.push(Arrangement {
        index: 0,
        items: vec![
            item(ItemSource::Pattern(1), 1, 0, 4),
            item(ItemSource::Pattern(1), 1, 4, 4),
            bass,
            deleted,
            vocal,
        ],
        ..Arrangement::default()
    });
    project
}

fn state(project: Project) -> UiState {
    UiState {
        project: Arc::new(project),
        position: 4 * 384,
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
        loop_range: None,
    }
}

/// The shell with the playlist open and the pointer over it.
fn harness(project: Project) -> Harness<'static, Vec<Command>> {
    let state = state(project);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(900.0, 360.0))
        .with_theme(egui::Theme::Dark)
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new());
    harness.run();
    let playlist = harness.get_by_label("Playlist").rect();
    harness.hover_at(egui::pos2(400.0, playlist.bottom() + 80.0));
    harness.run();
    harness
}

fn edit(edit: Edit) -> Command {
    Command::Edit {
        edit,
        drag_id: None,
    }
}

fn select(ids: Vec<usize>) -> Command {
    edit(Edit::SelectItems { ids, add: false })
}

const CTRL: egui::Modifiers = egui::Modifiers::COMMAND;

#[test]
fn ctrl_a_selects_every_item_and_ctrl_d_none() {
    let mut harness = harness(fixture());
    harness.key_press_modifiers(CTRL, egui::Key::A);
    harness.run();
    // The deleted record is no item to select.
    assert_eq!(harness.state(), &[select(vec![0, 1, 2, 4])]);
    harness.key_press_modifiers(CTRL, egui::Key::D);
    harness.run();
    assert_eq!(harness.state()[1..], [select(Vec::new())]);
}

#[test]
fn the_keys_are_the_playlist_s_only_with_the_pointer_over_it() {
    let mut harness = harness(fixture());
    // Over the top bar.
    harness.hover_at(egui::pos2(400.0, 10.0));
    harness.run();
    harness.key_press_modifiers(CTRL, egui::Key::A);
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());

    // Nothing is selected, so there is nothing for Ctrl+D to let go of.
    let mut project = fixture();
    let items = &mut project.arrangements[0].items;
    items.iter_mut().for_each(|item| item.flags = 0);
    let mut harness = self::harness(project);
    harness.key_press_modifiers(CTRL, egui::Key::D);
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());
}

#[test]
fn the_arrows_move_the_selection_a_beat_or_a_track_a_press() {
    let mut harness = harness(fixture());
    // The eight bars fill 804 points, so a beat is 25 points and the
    // drag's step is a beat.
    for key in [
        egui::Key::ArrowRight,
        egui::Key::ArrowLeft,
        egui::Key::ArrowDown,
        egui::Key::ArrowUp,
    ] {
        harness.key_press(key);
        harness.run();
    }
    let moved = |delta_ticks, delta_tracks| {
        edit(Edit::MoveItems {
            ids: vec![2, 4],
            delta_ticks,
            delta_tracks,
        })
    };
    assert_eq!(
        harness.state(),
        &[moved(96, 0), moved(-96, 0), moved(0, 1), moved(0, -1)]
    );

    // Zoomed in, the step is less than a beat.
    harness.event(egui::Event::Zoom(8.0));
    harness.run();
    harness.key_press(egui::Key::ArrowRight);
    harness.run();
    let Some(Command::Edit {
        edit: Edit::MoveItems { delta_ticks, .. },
        ..
    }) = harness.state().last()
    else {
        panic!("{:?}", harness.state());
    };
    assert!(*delta_ticks < 96 && 96 % *delta_ticks == 0, "{delta_ticks}");
}

#[test]
fn ctrl_e_splits_the_selected_items_under_the_play_cursor() {
    // The cursor is at bar 5, inside the bass and the vocal, which are
    // selected, at the end of the first drum pattern and at the start of
    // the second, which are selected too and stay whole.
    let mut project = fixture();
    for drums in &mut project.arrangements[0].items[..2] {
        drums.flags |= PlaylistItem::SELECTED;
    }
    let mut harness = harness(project);
    harness.key_press_modifiers(CTRL, egui::Key::E);
    harness.run();
    let split = edit(Edit::SplitItems {
        ids: vec![2, 4],
        at: 4 * 384,
    });
    assert_eq!(harness.state(), &[split]);

    // Items under the cursor that are not selected stay whole.
    let mut project = fixture();
    for item in &mut project.arrangements[0].items {
        item.flags = 0;
    }
    project.arrangements[0].items[1].flags = PlaylistItem::SELECTED;
    let mut harness = self::harness(project);
    harness.key_press_modifiers(CTRL, egui::Key::E);
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());
}
