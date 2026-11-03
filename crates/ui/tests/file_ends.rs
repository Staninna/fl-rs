//! Values only a file gives, past what the windows set themselves, drawn
//! and edited as FL would take them.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use model::Project;
use std::sync::Arc;
use ui::{Command, Edit, UiState};

fn state(project: Project) -> UiState {
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
        project_dir: None,
        sample_rate: None,
        last_error: None,
        busy: None,
        progress: None,
        loop_range: None,
    }
}

#[test]
fn a_drag_on_a_tempo_past_the_range_starts_from_its_top() {
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 0,
        ppq: 96,
    });
    // 156 is a u32 in the file, and this one is past i32's range.
    project.tempo_milli = Some(u32::MAX);
    let state = state(project);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(900.0, 360.0))
        .build_ui_state(move |ui, out| ui::shell(ui, &state, out), Vec::new());
    harness.run();
    let from = harness.get_by_label("Tempo").rect().center();
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    // Down is slower, a BPM a point, from 522 BPM.
    harness.hover_at(from + egui::vec2(0.0, 8.0));
    harness.step();
    harness.drop_at(from + egui::vec2(0.0, 8.0));
    harness.run();
    let edit = Edit::SetTempo { milli: 514_000 };
    assert_eq!(
        harness.state(),
        &[Command::Edit {
            edit,
            drag_id: Some(1)
        }]
    );
}
