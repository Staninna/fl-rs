//! The screenshots in the README, `docs/screenshots/*.png`: the start
//! screen and each main window, drawn from FL's Future House demo song.
//!
//! They are written, not compared, so this test is ignored. Run it again
//! after any change to how a window looks, and commit the pictures:
//!
//! ```sh
//! FL_RS_CORPUS="<FL Studio>/Data" cargo test -p ui --test screenshots -- --ignored
//! ```

use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::PathBuf;
use std::sync::Arc;
use ui::UiState;

/// The song a little way in, playing, with the meters lit.
fn state(project: model::Project, data: Option<PathBuf>) -> UiState {
    UiState {
        project: Arc::new(project),
        position: 16 * 96 * 4 + 200,
        peaks: (0.6, 0.55),
        insert_peaks: vec![(0.5, 0.45); 20],
        playing: true,
        song_mode: true,
        looping: false,
        pattern: 1,
        loop_range: None,
        data_root: data,
        project_dir: None,
        sample_rate: Some(48_000),
        last_error: None,
        busy: None,
        progress: None,
    }
}

/// Draw the shell with `keys` pressed and save it as `name`.png.
fn shot(name: &str, state: UiState, keys: &[Key]) {
    shot_after(name, state, |harness| press(harness, keys));
}

fn press(harness: &mut Harness<'_, Vec<ui::Command>>, keys: &[Key]) {
    for key in keys {
        harness.key_press(*key);
        harness.run();
    }
}

/// Draw the shell, let `act` press keys and click, and save it as
/// `name`.png.
fn shot_after(name: &str, state: UiState, act: impl FnOnce(&mut Harness<'_, Vec<ui::Command>>)) {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots");
    let mut harness = Harness::builder()
        .with_size(vec2(1280.0, 720.0))
        .with_theme(egui::Theme::Dark)
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new());
    harness.run();
    act(&mut harness);
    harness.run();
    harness
        .render()
        .unwrap()
        .save(out.join(format!("{name}.png")))
        .unwrap();
}

#[test]
#[ignore]
fn screenshots() {
    let Some(data) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        panic!("FL_RS_CORPUS names FL's Data directory, which the demo song is read from");
    };
    let song = data.join("Demo projects/Demo songs/Asher Postman - Future House.flp");
    let load = || model::from_flp(&flp::Project::parse(&std::fs::read(&song).unwrap()).unwrap());
    let empty = model::Project::new(flp::Header {
        format: 0,
        channel_count: 0,
        ppq: 96,
    });
    shot("start", state(empty, Some(data.clone())), &[]);
    shot("playlist", state(load(), Some(data.clone())), &[]);
    shot(
        "channel-rack",
        state(load(), Some(data.clone())),
        &[Key::F6],
    );
    // The piano roll shows the Outro Synth pattern's lead, which the rack
    // selects first.
    let outro = UiState {
        pattern: 8,
        ..state(load(), Some(data.clone()))
    };
    shot_after("piano-roll", outro, |harness| {
        press(harness, &[Key::F6]);
        harness.get_by_label("Wavvyy Synth").click();
        harness.run();
        press(harness, &[Key::F6, Key::F7]);
        // The pointer leaves, out of the picture's way.
        harness.event(egui::Event::PointerGone);
    });
    shot("browser", state(load(), Some(data.clone())), &[Key::F8]);
    shot("mixer", state(load(), Some(data.clone())), &[Key::F9]);
}
