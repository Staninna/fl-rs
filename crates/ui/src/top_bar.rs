//! The bar across the top: the menu, the transport with the pattern that
//! plays in pattern mode, the tempo and position readouts, the master's
//! peak meter and the project's title.
//!
//! The pattern selector's list ends in what changes the patterns: a new
//! empty pattern, which F4 makes too (FL's key for the next empty pattern,
//! inferred), and a clone of the selected pattern. Either goes at the
//! lowest index the project has free, and is selected. The last entry, and
//! F2 (inferred too), give the selected pattern a new name: a field takes
//! the selector's place, Enter and a click elsewhere take the name, and
//! Escape leaves it.
//!
//! A drag up or down on the tempo changes it, a BPM a point and a tenth of
//! that with Ctrl held, and a double click on it opens a field to type it
//! in. Enter and a click elsewhere take what was typed, and Escape leaves
//! the tempo as it was.

use crate::widgets::drag_value;
use crate::{Command, Dialog, Edit, Grid, UiState};
use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
use egui::{
    Align, Color32, FontId, Key, Layout, Modifiers, Rect, Response, RichText, Sense, Shape, Stroke,
    Ui, Vec2, WidgetInfo, pos2, vec2,
};
use model::TEMPO_MILLI;

/// How many points of drag take the tempo through FL's whole 10 to 522
/// BPM: a BPM a point.
const TEMPO_TRAVEL: f32 = 512.0;

/// A tempo that is being typed.
#[derive(Clone)]
struct TypedTempo {
    text: String,
    /// Whether the field has asked for the keyboard, which it does once.
    focused: bool,
}

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

        tempo(ui, state, out);
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

    pattern_selector(ui, state, out);
}

/// A pattern's name that is being typed.
#[derive(Clone)]
struct TypedName {
    pattern: u16,
    text: String,
    /// Whether the field has asked for the keyboard, which it does once.
    focused: bool,
}

/// The pattern selector: the list of the patterns, and under them a new
/// pattern, a clone of the selected one and its new name. While the name
/// is typed, a field takes the selector's place.
fn pattern_selector(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let edit = |edit: Edit| Command::Edit {
        edit,
        drag_id: None,
    };
    let own = |index: u16| state.project.pattern(index).and_then(|p| p.name.clone());
    let name = |index: u16| own(index).unwrap_or_else(|| format!("Pattern {index}"));
    let typed_id = egui::Id::new("fl-rs pattern name");
    if let Some(mut field) = ui.data(|data| data.get_temp::<TypedName>(typed_id)) {
        let text = egui::TextEdit::singleline(&mut field.text).desired_width(120.0);
        let response = ui.add(text);
        if !std::mem::replace(&mut field.focused, true) {
            response.request_focus();
        }
        if response.lost_focus() {
            let name = field.text.trim();
            let escaped = ui.input(|input| input.key_pressed(Key::Escape));
            if !escaped && name != own(field.pattern).unwrap_or_default() {
                let pattern = field.pattern;
                let name = name.to_owned();
                out.push(edit(Edit::SetPatternName { pattern, name }));
            }
            ui.data_mut(|data| data.remove::<TypedName>(typed_id));
        } else {
            ui.data_mut(|data| data.insert_temp(typed_id, field));
        }
        return;
    }

    let mut patterns: Vec<u16> = state.project.patterns.iter().map(|p| p.index).collect();
    patterns.sort_unstable();
    let free = state.project.free_pattern_index();
    let keys = !ui.ctx().egui_wants_keyboard_input();
    let pressed = |ui: &mut Ui, key| keys && ui.input_mut(|i| i.consume_key(Modifiers::NONE, key));
    let mut add = pressed(ui, Key::F4);
    let mut rename = pressed(ui, Key::F2);
    let mut clone = false;
    let has = state.project.pattern(state.pattern).is_some();
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
            ui.separator();
            let new = egui::Button::new("New pattern").shortcut_text("F4");
            add |= ui.add_enabled(free.is_some(), new).clicked();
            let copy = egui::Button::new("Clone pattern");
            clone = ui.add_enabled(free.is_some() && has, copy).clicked();
            let name = egui::Button::new("Rename pattern…").shortcut_text("F2");
            rename |= ui.add(name).clicked();
        })
        .response
        .on_hover_text("The pattern that plays in pattern mode");
    let from = state.pattern;
    let made = match free {
        Some(index) if add => Some((Edit::AddPattern { index }, index)),
        Some(to) if clone => Some((Edit::ClonePattern { from, to }, to)),
        _ => None,
    };
    if let Some((made, index)) = made {
        out.push(edit(made));
        out.push(Command::SelectPattern(index));
    } else if rename {
        let field = TypedName {
            pattern: from,
            text: own(from).unwrap_or_default(),
            focused: false,
        };
        ui.data_mut(|data| data.insert_temp(typed_id, field));
    }
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

/// The tempo's readout, which a drag changes and a double click opens a
/// field on. A project without a tempo shows dashes and takes no edit.
fn tempo(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let id = egui::Id::new("fl-rs tempo");
    let bpm = state.project.tempo();
    let milli = bpm.map(|bpm| (bpm * 1000.0).round() as u32);
    let typed: Option<TypedTempo> = ui.data(|data| data.get_temp(id));
    let set = |milli: u32, drag_id: Option<u64>| Command::Edit {
        edit: Edit::SetTempo { milli },
        drag_id,
    };
    if let Some(mut field) = typed {
        let text = egui::TextEdit::singleline(&mut field.text)
            .font(FontId::monospace(12.0))
            .desired_width(56.0);
        let response = ui.add(text);
        if !std::mem::replace(&mut field.focused, true) {
            response.request_focus();
        }
        ui.weak("BPM");
        if response.lost_focus() {
            let escaped = ui.input(|input| input.key_pressed(Key::Escape));
            let typed = parse_tempo(&field.text).filter(|_| !escaped);
            if let Some(typed) = typed.filter(|typed| Some(*typed) != milli) {
                out.push(set(typed, None));
            }
            ui.data_mut(|data| data.remove::<TypedTempo>(id));
        } else {
            ui.data_mut(|data| data.insert_temp(id, field));
        }
        return;
    }

    let text = match bpm {
        Some(bpm) => format!("{bpm:.3}"),
        None => "---.---".to_owned(),
    };
    let plate = readout(ui, &text, "BPM");
    let (Some(bpm), Some(milli)) = (bpm, milli) else {
        return;
    };
    let response = ui.interact(plate.rect, id.with("drag"), Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::slider(true, bpm, "Tempo"));
    let range = *TEMPO_MILLI.start() as i32..=*TEMPO_MILLI.end() as i32;
    if response.double_clicked() {
        let field = TypedTempo {
            text: format!("{bpm:.3}"),
            focused: false,
        };
        ui.data_mut(|data| data.insert_temp(id, field));
    } else if let Some((milli, drag)) = drag_value(ui, &response, milli as i32, range, TEMPO_TRAVEL)
    {
        out.push(set(milli as u32, Some(drag)));
    }
    let hint = "Tempo: drag up or down, with Ctrl for fine steps, or double-click to type it";
    response.on_hover_text(hint);
}

/// A typed tempo in thousandths of a BPM, held to FL's range. `None` for
/// what is not a number.
fn parse_tempo(text: &str) -> Option<u32> {
    let bpm: f64 = text
        .trim()
        .parse()
        .ok()
        .filter(|bpm: &f64| bpm.is_finite())?;
    let (low, high) = (*TEMPO_MILLI.start(), *TEMPO_MILLI.end());
    Some(
        (bpm * 1000.0)
            .round()
            .clamp(f64::from(low), f64::from(high)) as u32,
    )
}

/// A number on a dark plate with its unit beside it.
fn readout(ui: &mut Ui, value: &str, unit: &str) -> Response {
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
        })
        .response
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
    fn a_typed_tempo_is_held_to_fl_s_range() {
        assert_eq!(parse_tempo(" 128.5 "), Some(128_500));
        assert_eq!(parse_tempo("90"), Some(90_000));
        assert_eq!(parse_tempo("1"), Some(10_000));
        assert_eq!(parse_tempo("1000"), Some(522_000));
        assert_eq!(parse_tempo("fast"), None);
        assert_eq!(parse_tempo("NaN"), None);
        assert_eq!(parse_tempo("inf"), None);
    }

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
