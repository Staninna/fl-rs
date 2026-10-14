//! The browser drawn over a folder made for the test, beside the channel
//! rack of a project built in code.
//!
//! The snapshot is `tests/snapshots/browser.png`, see `shell.rs` for how
//! to write it again.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use model::{Channel, ChannelKind, Project};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui::{Command, Edit, UiState};

/// A directory under the system's temp directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    /// A project's folder: a project and a preset, an empty folder, and
    /// drums of every kind beside a file the browser leaves out.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("fl-rs-ui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for file in [
            "Drums/Snare.ogg",
            "Drums/Kick.wav",
            "Drums/Pad.wv",
            "Drums/Notes.txt",
            "Song.flp",
            "Lead.fst",
            "readme.md",
        ] {
            let file = dir.join(file);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, b"x").unwrap();
        }
        std::fs::create_dir_all(dir.join("Loops")).unwrap();
        Dir(dir)
    }

    fn file(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn channel(index: u16, kind: ChannelKind, name: &str) -> Channel {
    Channel {
        index,
        kind: Some(kind),
        name: Some(name.to_owned()),
        ..Channel::default()
    }
}

/// Two samplers and a plugin, open from a file in `folder`.
fn state(folder: &Path) -> UiState {
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 3,
        ppq: 96,
    });
    project.channels = vec![
        channel(0, ChannelKind::Sampler, "Kick"),
        channel(1, ChannelKind::Sampler, "Snare"),
        channel(2, ChannelKind::Plugin, "Bass"),
    ];
    UiState {
        project: Arc::new(project),
        position: 0,
        peaks: (0.0, 0.0),
        insert_peaks: Vec::new(),
        playing: false,
        song_mode: true,
        looping: false,
        pattern: 1,
        data_root: None,
        project_dir: Some(folder.to_path_buf()),
        sample_rate: Some(48_000),
        last_error: None,
        busy: None,
        progress: None,
    }
}

/// The shell with the browser open in its column, and the channel rack
/// beside it when `rack` is set.
fn harness(folder: &Path, rack: bool) -> Harness<'static, Vec<Command>> {
    let state = state(folder);
    // Frames this short make two clicks a double click, see the channel
    // rack's tests.
    let mut harness = Harness::builder()
        .with_step_dt(0.05)
        .with_size(egui::vec2(900.0, 360.0))
        .with_theme(egui::Theme::Dark)
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new());
    harness.key_press(egui::Key::F8);
    harness.run();
    if rack {
        harness.key_press(egui::Key::F6);
        harness.run();
    }
    harness
}

fn click(harness: &mut Harness<'_, Vec<Command>>, label: &str) {
    let pos = harness.get_by_label(label).rect().center();
    harness.hover_at(pos);
    harness.drag_at(pos);
    harness.drop_at(pos);
    harness.run();
}

fn double_click(harness: &mut Harness<'_, Vec<Command>>, label: &str) {
    let pos = harness.get_by_label(label).rect().center();
    harness.hover_at(pos);
    for _ in 0..2 {
        harness.drag_at(pos);
        harness.drop_at(pos);
    }
    harness.run();
}

/// Press on the row with this label and let go at `to`.
fn drag(harness: &mut Harness<'_, Vec<Command>>, label: &str, to: egui::Pos2) {
    let from = harness.get_by_label(label).rect().center();
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    harness.hover_at(to);
    harness.step();
    harness.drop_at(to);
    harness.run();
}

/// The rows with these labels, from the top.
fn tops(harness: &Harness<'_, Vec<Command>>, labels: &[&str]) -> Vec<f32> {
    let top = |label: &&str| harness.get_by_label(label).rect().top();
    labels.iter().map(top).collect()
}

fn shows(harness: &Harness<'_, Vec<Command>>, label: &str) -> bool {
    harness.query_by_label(label).is_some()
}

#[test]
fn the_browser_looks_like_its_snapshot() {
    let dir = Dir::new("snapshot");
    let mut harness = harness(&dir.0, true);
    click(&mut harness, "Drums");
    click(&mut harness, "Kick.wav");
    harness.remove_cursor();
    harness.run();
    assert!(harness.state().is_empty(), "{:?}", harness.state());
    harness.snapshot("browser");
}

#[test]
fn a_folder_lists_what_it_holds_when_it_is_opened() {
    let dir = Dir::new("expand");
    let mut harness = harness(&dir.0, false);
    // The root is open, and its folders come before its files.
    let root = ["Current project", "Drums", "Loops", "Lead.fst", "Song.flp"];
    assert!(tops(&harness, &root).is_sorted());
    assert!(!shows(&harness, "readme.md") && !shows(&harness, "Kick.wav"));

    click(&mut harness, "Drums");
    // The samples by name, the WavPack one among them, and no text file.
    let open = [
        "Drums",
        "Kick.wav",
        "Pad.wv",
        "Snare.ogg",
        "Loops",
        "Lead.fst",
    ];
    assert!(tops(&harness, &open).is_sorted());
    assert!(!shows(&harness, "Notes.txt"));
    // An empty folder opens on nothing.
    click(&mut harness, "Loops");
    assert!(tops(&harness, &["Snare.ogg", "Loops", "Lead.fst"]).is_sorted());

    click(&mut harness, "Drums");
    assert!(!shows(&harness, "Kick.wav") && shows(&harness, "Song.flp"));
    assert!(harness.state().is_empty(), "{:?}", harness.state());
}

#[test]
fn a_double_click_plays_a_sample_and_opens_a_project() {
    let dir = Dir::new("activate");
    let mut harness = harness(&dir.0, false);
    click(&mut harness, "Drums");
    // A click selects, and plays nothing.
    click(&mut harness, "Snare.ogg");
    assert!(harness.state().is_empty());
    double_click(&mut harness, "Kick.wav");
    assert_eq!(
        harness.state(),
        &[Command::Preview(dir.file("Drums/Kick.wav"))]
    );
    double_click(&mut harness, "Song.flp");
    assert_eq!(
        harness.state().last(),
        Some(&Command::Open(dir.file("Song.flp")))
    );
    // WavPack does not play, and a preset does not load.
    double_click(&mut harness, "Pad.wv");
    double_click(&mut harness, "Lead.fst");
    assert_eq!(harness.state().len(), 2);
}

#[test]
fn the_arrows_walk_the_tree_and_enter_plays_or_opens() {
    let dir = Dir::new("keys");
    let mut harness = harness(&dir.0, false);
    let press = |harness: &mut Harness<'_, Vec<Command>>, key| {
        harness.key_press(key);
        harness.run();
    };
    // The keys are the browser's while the pointer is over it.
    press(&mut harness, egui::Key::ArrowDown);
    press(&mut harness, egui::Key::Enter);
    assert!(shows(&harness, "Drums"));
    let over = harness.get_by_label("Lead.fst").rect().center();
    harness.hover_at(over);
    harness.run();

    // Down to the root and to Drums, right to open it and to step in.
    for key in [
        egui::Key::ArrowDown,
        egui::Key::ArrowDown,
        egui::Key::ArrowRight,
    ] {
        press(&mut harness, key);
    }
    assert!(shows(&harness, "Kick.wav"));
    press(&mut harness, egui::Key::ArrowRight);
    press(&mut harness, egui::Key::Enter);
    assert_eq!(
        harness.state(),
        &[Command::Preview(dir.file("Drums/Kick.wav"))]
    );
    // Up stops at the folder's first file, left goes to the folder and
    // then closes it.
    press(&mut harness, egui::Key::ArrowDown);
    press(&mut harness, egui::Key::ArrowUp);
    press(&mut harness, egui::Key::Enter);
    assert_eq!(harness.state().len(), 2);
    assert_eq!(harness.state()[0], harness.state()[1]);
    press(&mut harness, egui::Key::ArrowLeft);
    assert!(shows(&harness, "Kick.wav"));
    press(&mut harness, egui::Key::ArrowLeft);
    assert!(!shows(&harness, "Kick.wav"));

    // Past Loops and the preset to the project, which Enter opens.
    for _ in 0..3 {
        press(&mut harness, egui::Key::ArrowDown);
    }
    press(&mut harness, egui::Key::Enter);
    assert_eq!(
        harness.state().last(),
        Some(&Command::Open(dir.file("Song.flp")))
    );
}

#[test]
fn the_search_lists_what_is_loaded_by_name() {
    use egui::accesskit::Role;
    let dir = Dir::new("search");
    let mut harness = harness(&dir.0, false);
    let search = |harness: &mut Harness<'_, Vec<Command>>, text: &str| {
        harness.get_by_role(Role::TextInput).click();
        harness.run();
        harness.get_by_role(Role::TextInput).type_text(text);
        harness.run();
    };
    // Whatever has the letters in its name, a folder too, and no root.
    search(&mut harness, "O");
    assert!(shows(&harness, "Song.flp") && shows(&harness, "Loops"));
    assert!(!shows(&harness, "Drums") && !shows(&harness, "Lead.fst"));
    assert!(!shows(&harness, "Current project"));
    // Drums was not opened, so its snare is not loaded.
    assert!(!shows(&harness, "Snare.ogg"));
    for _ in 0..2 {
        harness.key_press(egui::Key::Backspace);
    }
    harness.run();
    assert!(shows(&harness, "Current project"));

    click(&mut harness, "Drums");
    search(&mut harness, "sn");
    assert!(shows(&harness, "Snare.ogg"));
    assert!(!shows(&harness, "Kick.wav") && !shows(&harness, "Drums"));
    // A result plays as a row of the tree does.
    double_click(&mut harness, "Snare.ogg");
    assert_eq!(
        harness.state(),
        &[Command::Preview(dir.file("Drums/Snare.ogg"))]
    );
}

#[test]
fn a_sample_dragged_onto_the_rack_goes_to_a_channel_or_makes_one() {
    let dir = Dir::new("drag");
    let mut harness = harness(&dir.0, true);
    click(&mut harness, "Drums");
    let edit = |edit| Command::Edit {
        edit,
        drag_id: None,
    };
    let path = dir.file("Drums/Kick.wav");

    // Onto the snare's row, by its steps and by its name.
    let steps = harness.get_by_label("Snare steps").rect().center();
    drag(&mut harness, "Kick.wav", steps);
    let set = edit(Edit::SetChannelSample {
        channel: 1,
        path: path.clone(),
    });
    assert_eq!(harness.state(), std::slice::from_ref(&set));
    let name = harness.get_by_label("Snare").rect().center();
    drag(&mut harness, "Kick.wav", name);
    assert_eq!(harness.state(), &[set.clone(), set]);

    // A plugin plays no sample, and WavPack is not dragged.
    let bass = harness.get_by_label("Bass steps").rect().center();
    drag(&mut harness, "Kick.wav", bass);
    drag(&mut harness, "Pad.wv", steps);
    assert_eq!(harness.state().len(), 2);

    // Under the rows the sample makes a channel of its own.
    let under = bass + egui::vec2(0.0, 80.0);
    drag(&mut harness, "Kick.wav", under);
    assert_eq!(
        harness.state().last(),
        Some(&edit(Edit::AddSamplerChannel { path }))
    );
    // Let go over the browser it goes nowhere.
    let back = harness.get_by_label("Loops").rect().center();
    drag(&mut harness, "Kick.wav", back);
    assert_eq!(harness.state().len(), 3);
}
