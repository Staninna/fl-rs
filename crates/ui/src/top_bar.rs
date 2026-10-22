//! The bar across the top: the menu, the transport with the pattern that
//! plays in pattern mode, the tempo and position readouts, the master's
//! peak meter and the project's title.

use crate::{Command, Dialog, Grid, UiState};
use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
use egui::{Align, Color32, Layout, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};

pub(crate) fn show(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    ui.horizontal(|ui| {
        ui.menu_button("File", |ui| {
            if ui.button("Open…").clicked() {
                out.push(Command::Choose(Dialog::Open));
            }
            if ui.button("Save").clicked() {
                out.push(Command::Save);
            }
            if ui.button("Save As…").clicked() {
                out.push(Command::Choose(Dialog::SaveAs));
            }
            if ui.button("Render to WAV…").clicked() {
                out.push(Command::Choose(Dialog::Render));
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                out.push(Command::Quit);
            }
        });
        ui.separator();
        transport(ui, state, out);
        ui.separator();

        let tempo = match state.project.tempo() {
            Some(bpm) => format!("{bpm:.3}"),
            None => "---.---".to_owned(),
        };
        readout(ui, &tempo, "BPM");
        let (bar, beat, tick) = Grid::of(&state.project).bar_beat_tick(state.position);
        readout(ui, &format!("{bar:03}:{beat:02}:{tick:03}"), "B:B:T");
        ui.separator();
        meter(ui, state.peaks);
        ui.separator();
        windows(ui);

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let title = state.project.title.as_deref().unwrap_or_default();
            ui.add(egui::Label::new(RichText::new(title).strong()).truncate());
        });
    });
}

/// A button per main window, lit while it is in view, as FL's row of
/// window buttons, each with a small picture of its window.
fn windows(ui: &mut Ui) {
    use crate::windows::{Window, shown, toggle};
    let mut style = icon_style(ui.visuals());
    style.selected = ButtonVisuals {
        fill: crate::theme::ACCENT,
        stroke: Stroke::NONE,
        icon: ui.visuals().selection.stroke.color,
    };
    let size = Vec2::splat(ui.spacing().interact_size.y + 4.0);
    for window in Window::ALL {
        let name = format!("Show {}", window.name().to_lowercase());
        let button = IconButton::new(&name, size, &style).selected(shown(ui.ctx(), window));
        let button = button.show(ui, |ui, rect, tint| window_icon(ui, rect, tint, window));
        let hint = format!("{} ({:?})", window.name(), window.key());
        if button.on_hover_text(hint).clicked() {
            toggle(ui.ctx(), window);
        }
    }
}

/// The picture on a window's button.
fn window_icon(ui: &Ui, rect: Rect, tint: Color32, window: crate::windows::Window) {
    use crate::windows::Window;
    let painter = ui.painter();
    let icon = Rect::from_center_size(rect.center(), rect.size() * 0.55);
    let (left, top, w, h) = (icon.left(), icon.top(), icon.width(), icon.height());
    let block = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(
            pos2(left + w * x0, top + h * y0),
            pos2(left + w * x1, top + h * y1),
        )
    };
    match window {
        // Clips staggered on tracks.
        Window::Playlist => {
            for (row, (from, to)) in [(0.0, 0.5), (0.3, 0.9), (0.55, 1.0)]
                .into_iter()
                .enumerate()
            {
                let y = row as f32 / 3.0;
                painter.rect_filled(block(from, y + 0.04, to, y + 0.29), 1.0, tint);
            }
        }
        // A row of steps per channel.
        Window::ChannelRack => {
            for row in 0..3 {
                for step in 0..4 {
                    let (x, y) = (step as f32 / 4.0, row as f32 / 3.0);
                    let lit = (row + step) % 2 == 0;
                    let colour = if lit { tint } else { tint.gamma_multiply(0.35) };
                    painter.rect_filled(block(x + 0.03, y + 0.04, x + 0.22, y + 0.29), 1.0, colour);
                }
            }
        }
        // Notes at different heights.
        Window::PianoRoll => {
            for (from, to, y) in [(0.0, 0.4, 0.6), (0.35, 0.7, 0.3), (0.6, 1.0, 0.0)] {
                painter.rect_filled(block(from, y + 0.05, to, y + 0.35), 1.0, tint);
            }
        }
        // A folder.
        Window::Browser => {
            painter.rect_filled(block(0.0, 0.1, 0.45, 0.3), 1.0, tint);
            painter.rect_filled(block(0.0, 0.25, 1.0, 0.9), 1.0, tint);
        }
        // Faders on their tracks.
        Window::Mixer => {
            for (i, knob) in [0.6, 0.25, 0.45].into_iter().enumerate() {
                let x = left + w * (0.17 + i as f32 * 0.33);
                painter.vline(
                    x,
                    icon.y_range(),
                    Stroke::new(1.5, tint.gamma_multiply(0.5)),
                );
                let y = top + h * knob;
                painter.rect_filled(
                    Rect::from_center_size(pos2(x, y), vec2(w * 0.24, h * 0.16)),
                    1.0,
                    tint,
                );
            }
        }
    }
}

/// Play, stop, the switch between pattern mode and song mode, the loop
/// switch and the pattern selector.
fn transport(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let style = icon_style(ui.visuals());
    let size = Vec2::splat(ui.spacing().interact_size.y + 4.0);
    let play = IconButton::new("Play", size, &style).selected(state.playing);
    let play = play.show(ui, |ui, rect, tint| {
        let icon = Rect::from_center_size(rect.center(), rect.size() * 0.45);
        let points = vec![icon.left_top(), icon.right_center(), icon.left_bottom()];
        ui.painter()
            .add(Shape::convex_polygon(points, tint, Stroke::NONE));
    });
    if play.on_hover_text("Play").clicked() {
        out.push(Command::Play);
    }
    let stop = IconButton::new("Stop", size, &style).show(ui, |ui, rect, tint| {
        let icon = Rect::from_center_size(rect.center(), rect.size() * 0.4);
        ui.painter().rect_filled(icon, 1.0, tint);
    });
    if stop.on_hover_text("Stop").clicked() {
        out.push(Command::Stop);
    }

    for (label, song) in [("PAT", false), ("SONG", true)] {
        let chosen = state.song_mode == song;
        if ui.selectable_label(chosen, label).clicked() && !chosen {
            out.push(Command::ToggleSongMode);
        }
    }
    let looping = ui.selectable_label(state.looping, "LOOP");
    if looping
        .on_hover_text("Start over at the end of the song")
        .clicked()
    {
        out.push(Command::ToggleLoop);
    }

    let name = |index: u16| {
        let pattern = state.project.pattern(index);
        let name = pattern.and_then(|pattern| pattern.name.clone());
        name.unwrap_or_else(|| format!("Pattern {index}"))
    };
    let mut patterns: Vec<u16> = state.project.patterns.iter().map(|p| p.index).collect();
    patterns.sort_unstable();
    egui::ComboBox::from_id_salt("pattern")
        .width(120.0)
        .selected_text(name(state.pattern))
        .show_ui(ui, |ui| {
            for index in patterns {
                let chosen = index == state.pattern;
                if ui.selectable_label(chosen, name(index)).clicked() && !chosen {
                    out.push(Command::SelectPattern(index));
                }
            }
        })
        .response
        .on_hover_text("The pattern that plays in pattern mode");
}

/// The transport buttons in the colours of egui's own buttons.
fn icon_style(visuals: &egui::Visuals) -> IconButtonStyle {
    let look = |widget: &egui::style::WidgetVisuals| ButtonVisuals {
        fill: widget.weak_bg_fill,
        stroke: widget.bg_stroke,
        icon: widget.fg_stroke.color,
    };
    IconButtonStyle {
        normal: look(&visuals.widgets.inactive),
        hovered: look(&visuals.widgets.hovered),
        pressed: Some(look(&visuals.widgets.active)),
        // Play lights green while the song plays, as in FL.
        selected: ButtonVisuals {
            fill: Color32::from_rgb(70, 150, 80),
            stroke: Stroke::NONE,
            icon: Color32::from_rgb(225, 255, 225),
        },
        corner_radius: visuals.widgets.inactive.corner_radius,
        focus_stroke: visuals.selection.stroke,
    }
}

/// A number on a dark plate with its unit beside it.
fn readout(ui: &mut Ui, value: &str, unit: &str) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(vec2(6.0, 2.0))
        .corner_radius(2.0)
        .show(ui, |ui| {
            ui.label(
                RichText::new(value)
                    .monospace()
                    .strong()
                    .color(crate::theme::READOUT),
            );
            ui.weak(unit);
        });
}

/// The bottom of the meter, in dB below full scale.
const METER_FLOOR_DB: f32 = -48.0;
/// The top of the meter, in dB above full scale.
const METER_TOP_DB: f32 = 6.0;

/// How far along the meter a peak reaches, from 0 to 1.
pub(crate) fn meter_fraction(peak: f32) -> f32 {
    if peak.is_nan() || peak <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * peak.log10();
    ((db - METER_FLOOR_DB) / (METER_TOP_DB - METER_FLOOR_DB)).clamp(0.0, 1.0)
}

/// Two bars, left above right, with a mark at full scale. A bar turns red
/// when its peak is over.
fn meter(ui: &mut Ui, (left, right): (f32, f32)) {
    let size = vec2(140.0, ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    let full_scale = rect.left() + rect.width() * meter_fraction(1.0);
    let bar_height = (rect.height() - 3.0) / 2.0;
    for (row, peak) in [left, right].into_iter().enumerate() {
        let top = rect.top() + 1.0 + row as f32 * (bar_height + 1.0);
        let end = rect.left() + rect.width() * meter_fraction(peak);
        let bar = Rect::from_min_max(pos2(rect.left(), top), pos2(end, top + bar_height));
        let colour = if peak > 1.0 {
            Color32::from_rgb(230, 70, 60)
        } else {
            Color32::from_rgb(110, 200, 90)
        };
        painter.rect_filled(bar, 0.0, colour);
    }
    painter.vline(
        full_scale,
        rect.y_range(),
        Stroke::new(1.0, visuals.weak_text_color()),
    );
    let db = |peak: f32| {
        if peak > 0.0 {
            format!("{:.1} dB", 20.0 * peak.log10())
        } else {
            "silent".to_owned()
        }
    };
    response.on_hover_text(format!(
        "Master peak: left {}, right {}",
        db(left),
        db(right)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meter_runs_from_minus_48_to_plus_6_db() {
        assert_eq!(meter_fraction(0.0), 0.0);
        assert_eq!(meter_fraction(-1.0), 0.0);
        assert_eq!(meter_fraction(f32::NAN), 0.0);
        assert_eq!(meter_fraction(0.001), 0.0);
        assert!((meter_fraction(1.0) - 48.0 / 54.0).abs() < 1e-6);
        assert_eq!(meter_fraction(4.0), 1.0);
    }
}
