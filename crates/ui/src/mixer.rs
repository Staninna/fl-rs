//! The mixer: a strip per insert, the master first, and beside the strips
//! the ten effect slots of the selected insert.
//!
//! A strip has the insert's name, its pan knob, its peak meter beside its
//! fader, and the LED that switches it on and off. A click on a strip
//! selects its insert, a click on its name renames it as well, and the
//! left and right arrows move the selection.
//!
//! The bottom of a strip is what the selected insert sends to it, as FL
//! has it: an arrow that makes the route or removes it, and above a lit
//! arrow the knob for the level of the send. The master sends nowhere, so
//! with the master selected there are no arrows, and an arrow that would
//! lead an insert back into itself cannot be lit.
//!
//! The strips are the master and the inserts in use: those with a name,
//! with a plugin in a slot, or with a channel that plays into them. The
//! button above them shows every insert the project has. The current
//! track, which FL keeps behind the last insert, has no strip.
//!
//! A slot shows the name of its plugin, an LED that switches it on and
//! off, and a knob for its mix level. The plugins have no editors yet.
//!
//! Nothing here changes the project. An edit goes out as a
//! [`Command::Edit`], and the next frame draws what the app made of it.

use crate::playlist::{rgb, text_on};
use crate::top_bar::meter_fraction;
use crate::widgets::{Knob, drag_value};
use crate::{Command, Edit, UiState};
use egui::{
    Align, Align2, Color32, FontId, Key, Layout, Modifiers, Rect, Sense, Shape, Stroke, StrokeKind,
    Ui, UiBuilder, WidgetInfo, WidgetType, pos2, vec2,
};
use engine::{Curves, FlExact};
use model::rec::LAST_INSERT;
use model::{Insert, Project};
use std::collections::BTreeSet;

/// The width of a strip.
const STRIP: f32 = 54.0;
/// The height of the plate with the insert's name, and of the line with
/// its number above it.
const NAME: f32 = 18.0;
const NUMBER: f32 = 11.0;
const KNOB: f32 = 22.0;
const LED: f32 = 12.0;
/// The height of the arrow that makes a route.
const ARROW: f32 = 12.0;
/// The least and the most a fader is high. In between it takes the height
/// of the window, as FL's does.
const FADER: (f32, f32) = (60.0, 520.0);
/// The width and the height of a fader's handle.
const HANDLE: (f32, f32) = (18.0, 8.0);
/// The width of a meter's bar.
const BAR: f32 = 4.0;
/// The space between the parts of a strip.
const GAP: f32 = 4.0;
/// The width of the panel with the slots, and the height of a slot's row.
const SLOTS: f32 = 190.0;
const SLOT_ROW: f32 = 22.0;
/// The top of a fader and of a send level, and where both are at 0 dB.
const FADER_TOP: i32 = 16_000;
const UNITY: i32 = 12_800;
/// A pan all the way to the right.
const PAN_END: i32 = 6400;
/// The top of a slot's mix level.
const MIX_TOP: i32 = 12_800;
/// The colour of a lit LED, of a lit arrow and of a knob's arc.
const LIT: Color32 = Color32::from_rgb(120, 220, 110);

/// What the mixer keeps from one frame to the next.
#[derive(Clone, Default)]
struct Memory {
    /// The selected insert, by the loader's number. The master when the
    /// project has no such insert.
    selected: u16,
    /// Whether every insert has a strip, or only those in use.
    all: bool,
    rename: Option<Rename>,
}

/// A name that is being typed.
#[derive(Clone)]
struct Rename {
    insert: u16,
    text: String,
    /// Whether the field has asked for the keyboard, which it does once.
    focused: bool,
}

fn edit(edit: Edit, drag_id: Option<u64>) -> Command {
    Command::Edit { edit, drag_id }
}

/// What an insert goes by: its name, and without one what FL shows.
fn name(insert: Option<&Insert>, index: u16) -> String {
    let own = insert.and_then(|insert| insert.name.clone());
    own.unwrap_or_else(|| match index {
        0 => "Master".to_owned(),
        index => format!("Insert {index}"),
    })
}

/// The inserts the project has in front of the current track, by the
/// loader's numbers: the master and the ones after it.
fn inserts(project: &Project) -> Vec<u16> {
    let numbers = (0..project.inserts.len()).filter_map(|place| project.insert_index(place));
    numbers.filter(|index| *index < LAST_INSERT).collect()
}

/// The inserts with a name, with a plugin in a slot, or with a channel
/// that plays into them.
fn in_use(project: &Project) -> BTreeSet<u16> {
    let played = project
        .channels
        .iter()
        .filter_map(|channel| channel.insert());
    let mut used: BTreeSet<u16> = played
        .filter_map(|index| u16::try_from(index).ok())
        .collect();
    for (place, insert) in project.inserts.iter().enumerate() {
        let plugins = insert
            .slots
            .iter()
            .filter_map(|slot| slot.plugin_name.as_deref());
        let filled = insert.name.is_some() || plugins.into_iter().any(|name| !name.is_empty());
        used.extend(project.insert_index(place).filter(|_| filled));
    }
    used
}

/// A fader's value or a send level in FL's dB.
fn db_hint(what: &str, raw: i32) -> String {
    match raw {
        ..=0 => format!("{what} -∞ dB"),
        raw => format!("{what} {:.1} dB", 20.0 * FlExact.insert_gain(raw).log10()),
    }
}

/// An insert's pan as FL words it: a percentage to one side.
fn pan_hint(pan: i32) -> String {
    match (pan as f32 / PAN_END as f32 * 100.0).round() as i32 {
        0 => "Pan centred".to_owned(),
        left if left < 0 => format!("Pan {}% left", -left),
        right => format!("Pan {right}% right"),
    }
}

/// How far up its travel a fader's handle is, from 0 to 1.
fn fader_fraction(volume: i32) -> f32 {
    volume.clamp(0, FADER_TOP) as f32 / FADER_TOP as f32
}

/// An LED that switches something on and off. One that cannot be clicked
/// is drawn faint. True when it was clicked.
fn led(ui: &mut Ui, on: bool, enabled: bool, label: &str, hint: &str) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(LED, LED), sense);
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, on, label));
    let visuals = ui.visuals();
    let (fill, stroke) = match (enabled, on) {
        (false, _) => (
            visuals.extreme_bg_color,
            visuals.widgets.noninteractive.bg_stroke,
        ),
        (true, false) => (visuals.extreme_bg_color, visuals.widgets.inactive.fg_stroke),
        (true, true) => (LIT, visuals.widgets.inactive.fg_stroke),
    };
    ui.painter()
        .circle(rect.center(), LED / 2.0 - 1.5, fill, stroke);
    response.on_hover_text(hint).clicked()
}

pub(crate) fn show(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let project = &*state.project;
    let inserts = inserts(project);
    if inserts.is_empty() {
        ui.centered_and_justified(|ui| ui.weak("The project has no mixer"));
        return;
    }
    let memory_id = egui::Id::new("fl-rs mixer");
    let mut memory: Memory = ui.data(|data| data.get_temp(memory_id)).unwrap_or_default();
    if !inserts.contains(&memory.selected) {
        memory.selected = 0;
    }

    ui.horizontal(|ui| {
        let all = ui.selectable_label(memory.all, format!("All {}", inserts.len() - 1));
        if all
            .on_hover_text("Show every insert, or only the ones in use")
            .clicked()
        {
            memory.all = !memory.all;
        }
    });
    let used = in_use(project);
    let has_strip = |index: &u16| {
        memory.all || *index == 0 || *index == memory.selected || used.contains(index)
    };
    let shown: Vec<u16> = inserts.iter().copied().filter(has_strip).collect();
    memory
        .rename
        .take_if(|rename| !shown.contains(&rename.insert));

    // The arrows that move the selection.
    let mut moved = false;
    if ui.memory(|egui| egui.focused().is_none()) {
        let pressed = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
        let at = shown.iter().position(|index| *index == memory.selected);
        let at = at.unwrap_or(0);
        if pressed(Key::ArrowLeft) && at > 0 {
            (memory.selected, moved) = (shown[at - 1], true);
        }
        if pressed(Key::ArrowRight) && at + 1 < shown.len() {
            (memory.selected, moved) = (shown[at + 1], true);
        }
    }

    egui::Panel::right("fl-rs mixer slots")
        .exact_size(SLOTS)
        .show(ui, |ui| slots(ui, project, memory.selected, out));
    let strips = Strips {
        project,
        peaks: &state.insert_peaks,
        selected: memory.selected,
        sends: project.insert_mix(memory.selected).destinations(),
        feeding: project.inserts_feeding(memory.selected),
    };
    egui::ScrollArea::horizontal()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
            ui.horizontal_top(|ui| {
                let fixed = NUMBER + NAME + 2.0 * KNOB + LED + ARROW + 6.0 * GAP + 4.0;
                let height = ui
                    .available_height()
                    .clamp(fixed + FADER.0, fixed + FADER.1);
                for index in shown {
                    let scroll_to = moved && index == strips.selected;
                    strips.strip(ui, index, height, scroll_to, &mut memory, out);
                    // FL sets the master apart from the inserts.
                    if index == 0 {
                        ui.add_space(6.0);
                    }
                }
            });
        });
    ui.data_mut(|data| data.insert_temp(memory_id, memory));
}

/// What every strip is drawn from.
struct Strips<'a> {
    project: &'a Project,
    /// The peak of every insert, by its number.
    peaks: &'a [(f32, f32)],
    selected: u16,
    /// Where the selected insert sends to.
    sends: Vec<u16>,
    /// The inserts that send to the selected one, straight or through
    /// others. A route to one of them would close a loop.
    feeding: BTreeSet<u16>,
}

impl Strips<'_> {
    /// The strip of an insert, `height` points high.
    fn strip(
        &self,
        ui: &mut Ui,
        index: u16,
        height: f32,
        scroll_to: bool,
        memory: &mut Memory,
        out: &mut Vec<Command>,
    ) {
        let (rect, background) = ui.allocate_exact_size(vec2(STRIP, height), Sense::click());
        if scroll_to {
            background.scroll_to_me(None);
        }
        // A project can have 500 inserts, and a window shows twenty.
        if !ui.is_rect_visible(rect) {
            return;
        }
        let mix = self.project.insert_mix(index);
        let name = name(mix.insert(), index);
        let selected = index == self.selected;
        let label = format!("{name} strip");
        background.widget_info(|| WidgetInfo::selected(WidgetType::Other, true, selected, &label));
        if background.clicked() {
            memory.selected = index;
        }
        let visuals = ui.visuals();
        // FL lights the selected strip a shade, and keeps the orange for
        // what is switched on.
        let fill = if selected {
            visuals.widgets.inactive.bg_fill
        } else {
            visuals.faint_bg_color
        };
        ui.painter().rect_filled(rect, 2.0, fill);
        if selected {
            let edge = Stroke::new(1.0, visuals.selection.bg_fill.gamma_multiply(0.7));
            ui.painter()
                .rect_stroke(rect, 2.0, edge, StrokeKind::Inside);
        }

        // An ID of its own, so that a strip's widgets stay the same ones
        // when the strips in front of it scroll out of sight.
        let builder = UiBuilder::new()
            .id(ui.id().with(("strip", index)))
            .max_rect(rect.shrink(2.0))
            .layout(Layout::top_down(Align::Center));
        let ui = &mut ui.new_child(builder);
        ui.spacing_mut().item_spacing = vec2(0.0, GAP);

        // FL heads each strip with its number, and the master with an M.
        let (number, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), NUMBER), Sense::hover());
        let text = match index {
            0 => "M".to_owned(),
            index => index.to_string(),
        };
        let ink = if selected {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().text(
            number.center(),
            Align2::CENTER_CENTER,
            text,
            FontId::monospace(9.0),
            ink,
        );
        self.name_plate(ui, index, &name, memory, out);
        let pan = mix.pan_raw();
        let knob = Knob {
            label: &format!("{name} pan"),
            value: pan,
            range: -PAN_END..=PAN_END,
            default: 0,
            hint: pan_hint(pan),
            colour: LIT,
            enabled: true,
        };
        if let Some((pan, drag_id)) = knob.show(ui, KNOB) {
            let pan = Edit::SetInsertPan { insert: index, pan };
            out.push(edit(pan, drag_id));
        }

        let below = LED + KNOB + ARROW + 3.0 * GAP;
        let size = vec2(ui.available_width(), ui.available_height() - below);
        let (row, _) = ui.allocate_exact_size(size, Sense::hover());
        self.fader(ui, row, index, &name, mix.volume_raw(), out);

        let enabled = mix.enabled();
        if led(ui, enabled, true, &format!("{name} on"), "Insert on or off") {
            let enabled = !enabled;
            let switch = Edit::SetInsertEnabled {
                insert: index,
                enabled,
            };
            out.push(edit(switch, None));
        }
        self.send(ui, index, &name, out);
    }

    /// The plate with the insert's name, which a click turns into a field
    /// to type a new name in. Enter and a click elsewhere take the name,
    /// and Escape leaves the old one.
    fn name_plate(
        &self,
        ui: &mut Ui,
        index: u16,
        name: &str,
        memory: &mut Memory,
        out: &mut Vec<Command>,
    ) {
        let insert = self.project.insert_mix(index).insert();
        let own = insert.and_then(|insert| insert.name.as_deref());
        let size = vec2(ui.available_width(), NAME);
        if let Some(rename) = memory.rename.as_mut().filter(|r| r.insert == index) {
            let field = egui::TextEdit::singleline(&mut rename.text)
                .font(FontId::proportional(11.0))
                .margin(vec2(2.0, 1.0));
            let response = ui.add_sized(size, field);
            if !std::mem::replace(&mut rename.focused, true) {
                response.request_focus();
            }
            if response.lost_focus() {
                if !ui.input(|input| input.key_pressed(Key::Escape)) {
                    self.take_name(rename, out);
                }
                memory.rename = None;
            }
            return;
        }

        let (plate, response) = ui.allocate_exact_size(size, Sense::click());
        let selected = index == self.selected;
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, name));
        let visuals = ui.visuals();
        let plain = visuals.widgets.inactive.bg_fill;
        let fill = insert.and_then(|insert| insert.color).map_or(plain, rgb);
        let text = if self.project.insert_mix(index).enabled() {
            text_on(fill)
        } else {
            text_on(fill).gamma_multiply(0.5)
        };
        let painter = ui.painter_at(plate);
        painter.rect_filled(plate, 2.0, fill);
        painter.text(
            pos2(plate.left() + 3.0, plate.center().y),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(10.0),
            text,
        );
        if selected {
            painter.rect_stroke(plate, 2.0, Stroke::new(1.5, LIT), StrokeKind::Inside);
        }
        if response.on_hover_text("Click to rename").clicked() {
            // The field of a strip further right has not seen this click
            // yet, and will not be drawn to see it.
            if let Some(typed) = &memory.rename {
                self.take_name(typed, out);
            }
            memory.selected = index;
            memory.rename = Some(Rename {
                insert: index,
                text: own.unwrap_or_default().to_owned(),
                focused: false,
            });
        }
    }

    /// Send the name that was typed, unless it is the one the insert has.
    fn take_name(&self, typed: &Rename, out: &mut Vec<Command>) {
        let insert = self.project.insert_mix(typed.insert).insert();
        let own = insert.and_then(|insert| insert.name.as_deref());
        let name = typed.text.trim();
        if name != own.unwrap_or_default() {
            let named = Edit::SetInsertName {
                insert: typed.insert,
                name: name.to_owned(),
            };
            out.push(edit(named, None));
        }
    }

    /// The fader in `row`, with the insert's meter on its left and FL's
    /// two marks beside that: 0 dB and silence.
    ///
    /// A drag moves the handle with the pointer, and a tenth as far with
    /// Ctrl held. A double click puts it back at 0 dB.
    fn fader(
        &self,
        ui: &mut Ui,
        row: Rect,
        index: u16,
        name: &str,
        volume: i32,
        out: &mut Vec<Command>,
    ) {
        let (top, bottom) = (row.top() + HANDLE.1 / 2.0, row.bottom() - HANDLE.1 / 2.0);
        let y = |volume: i32| bottom - fader_fraction(volume) * (bottom - top);
        let track = row.right() - HANDLE.0 / 2.0 - 2.0;
        let grip = Rect::from_x_y_ranges(
            track - HANDLE.0 / 2.0..=track + HANDLE.0 / 2.0,
            row.y_range(),
        );
        let response = ui.interact(grip, ui.id().with("fader"), Sense::click_and_drag());
        let label = format!("{name} volume");
        response.widget_info(|| WidgetInfo::slider(true, f64::from(volume), &label));
        if response.double_clicked() {
            if volume != UNITY {
                let volume = Edit::SetInsertVolume {
                    insert: index,
                    volume: UNITY,
                };
                out.push(edit(volume, None));
            }
        } else if let Some((volume, drag)) =
            drag_value(ui, &response, volume, 0..=FADER_TOP, bottom - top)
        {
            let volume = Edit::SetInsertVolume {
                insert: index,
                volume,
            };
            out.push(edit(volume, Some(drag)));
        }

        let visuals = ui.visuals();
        let painter = ui.painter_at(row);
        let ink = visuals.weak_text_color();
        for (mark, at) in [("0", UNITY), ("-∞", 0)] {
            let left = pos2(row.left(), y(at));
            painter.text(left, Align2::LEFT_CENTER, mark, FontId::monospace(9.0), ink);
            painter.hline(
                track - HANDLE.0 / 2.0 - 2.0..=track,
                y(at),
                Stroke::new(1.0, ink),
            );
        }
        let (left, right) = self
            .peaks
            .get(usize::from(index))
            .copied()
            .unwrap_or_default();
        for (side, peak) in [left, right].into_iter().enumerate() {
            let x = row.left() + 15.0 + side as f32 * (BAR + 1.0);
            let bar = Rect::from_x_y_ranges(x..=x + BAR, top..=bottom);
            painter.rect_filled(bar, 0.0, visuals.extreme_bg_color);
            let colour = if peak > 1.0 {
                Color32::from_rgb(230, 70, 60)
            } else {
                Color32::from_rgb(110, 200, 90)
            };
            let reached = bar.bottom() - bar.height() * meter_fraction(peak);
            painter.rect_filled(bar.with_min_y(reached), 0.0, colour);
        }
        painter.vline(
            track,
            top..=bottom,
            Stroke::new(2.0, visuals.extreme_bg_color),
        );
        let widget = ui.style().interact(&response);
        let handle = Rect::from_center_size(pos2(track, y(volume)), vec2(HANDLE.0, HANDLE.1));
        painter.rect(
            handle,
            2.0,
            widget.bg_fill,
            widget.fg_stroke,
            StrokeKind::Inside,
        );
        painter.hline(handle.x_range().shrink(3.0), handle.center().y, (1.0, LIT));

        let hint = db_hint("Volume", volume);
        if response.dragged() {
            response.show_tooltip_text(hint);
        } else {
            response.on_hover_text(hint);
        }
    }

    /// The bottom of a strip: the arrow that routes the selected insert
    /// here, and above it the knob for the level of that send when the
    /// route is there. The selected insert's own strip has neither, and
    /// with the master selected no strip has.
    fn send(&self, ui: &mut Ui, index: u16, name: &str, out: &mut Vec<Command>) {
        let from = self.selected;
        if from == 0 || from == index {
            return;
        }
        let on = self.sends.contains(&index);
        if on {
            let strip = self.project.insert_mix(from);
            let level = strip.send_level_raw(index).unwrap_or(UNITY);
            let knob = Knob {
                label: &format!("Send to {name}"),
                value: level,
                range: 0..=FADER_TOP,
                default: UNITY,
                hint: db_hint("Send", level),
                colour: LIT,
                enabled: true,
            };
            if let Some((level, drag_id)) = knob.show(ui, KNOB) {
                let level = Edit::SetSendLevel {
                    from,
                    to: index,
                    level,
                };
                out.push(edit(level, drag_id));
            }
        } else {
            ui.allocate_exact_size(vec2(KNOB, KNOB), Sense::hover());
        }

        // An insert that sends to the selected one cannot get it back. Its
        // arrow takes the click all the same, which would select the strip
        // otherwise.
        let allowed = on || !self.feeding.contains(&index);
        let (rect, response) = ui.allocate_exact_size(vec2(HANDLE.0, ARROW), Sense::click());
        let label = format!("Route to {name}");
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, allowed, on, &label));
        let visuals = ui.visuals();
        let arrow = rect.shrink2(vec2(3.0, 2.0));
        let points = vec![
            arrow.center_top(),
            arrow.right_bottom(),
            arrow.left_bottom(),
        ];
        let (fill, line) = match (on, allowed) {
            (true, _) => (LIT, LIT),
            (false, true) => (Color32::TRANSPARENT, visuals.text_color()),
            (false, false) => (
                Color32::TRANSPARENT,
                visuals.widgets.noninteractive.bg_stroke.color,
            ),
        };
        ui.painter()
            .add(Shape::convex_polygon(points, fill, Stroke::new(1.0, line)));
        let hint = match (on, allowed) {
            (true, _) => "The selected insert sends here. Click to remove the route",
            (false, true) => "Click to send the selected insert here",
            (false, false) => "This insert sends to the selected one already",
        };
        if response.on_hover_text(hint).clicked() && allowed {
            let route = Edit::SetRoute {
                from,
                to: index,
                on: !on,
            };
            out.push(edit(route, None));
        }
    }
}

/// The ten effect slots of an insert: for each its switch, the name of
/// its plugin and its mix knob. An empty slot has nothing to switch.
fn slots(ui: &mut Ui, project: &Project, index: u16, out: &mut Vec<Command>) {
    let mix = project.insert_mix(index);
    let held = mix.insert().map_or(&[][..], |insert| &insert.slots);
    ui.strong(format!("{} slots", name(mix.insert(), index)));
    ui.spacing_mut().item_spacing = vec2(GAP, 1.0);
    for slot in 0..10u8 {
        let label = format!("Slot {}", slot + 1);
        let held = held.iter().find(|held| held.index == i32::from(slot));
        let named = |name: &Option<String>| name.clone().filter(|name| !name.is_empty());
        let plugin = held.and_then(|held| named(&held.plugin_name));
        let shown = held.and_then(|held| named(&held.name)).or(plugin.clone());
        let filled = plugin.is_some();
        ui.horizontal(|ui| {
            let enabled = mix.slot_enabled(slot).unwrap_or(true);
            if led(
                ui,
                enabled,
                filled,
                &format!("{label} on"),
                "Slot on or off",
            ) {
                let enabled = !enabled;
                let switch = Edit::SetSlotEnabled {
                    insert: index,
                    slot,
                    enabled,
                };
                out.push(edit(switch, None));
            }

            let size = vec2(ui.available_width() - KNOB - GAP, SLOT_ROW - 2.0);
            let (plate, response) = ui.allocate_exact_size(size, Sense::hover());
            let shown = shown.as_deref().unwrap_or(&label);
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, shown));
            let visuals = ui.visuals();
            let painter = ui.painter_at(plate);
            painter.rect_filled(plate, 2.0, visuals.extreme_bg_color);
            let ink = match (filled, enabled) {
                (true, true) => visuals.text_color(),
                _ => visuals.weak_text_color(),
            };
            painter.text(
                pos2(plate.left() + 5.0, plate.center().y),
                Align2::LEFT_CENTER,
                shown,
                FontId::proportional(11.0),
                ink,
            );

            let level = mix.slot_mix_raw(slot).unwrap_or(MIX_TOP);
            let knob = Knob {
                label: &format!("{label} mix"),
                value: level,
                range: 0..=MIX_TOP,
                default: MIX_TOP,
                hint: format!("Mix {:.0}%", level as f32 / MIX_TOP as f32 * 100.0),
                colour: LIT,
                enabled: filled,
            };
            if let Some((mix, drag_id)) = knob.show(ui, KNOB - 2.0) {
                let mix = Edit::SetSlotMix {
                    insert: index,
                    slot,
                    mix,
                };
                out.push(edit(mix, drag_id));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{Channel, Slot};

    fn project(version: &str, count: Option<u16>, inserts: usize) -> Project {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 0,
            ppq: 96,
        });
        project.writer_version = Some(version.to_owned());
        project.insert_count = count;
        project.inserts = vec![Insert::default(); inserts];
        project
    }

    #[test]
    fn the_strips_are_the_inserts_in_front_of_the_current_track() {
        assert_eq!(inserts(&project("25.1.0.4858", Some(5), 5)), [0, 1, 2, 3]);
        let legacy = inserts(&project("20.8.4.2576", None, 127));
        assert_eq!((legacy.len(), legacy.last()), (126, Some(&125)));
        // A project built in code may have no mixer at all.
        assert!(inserts(&project("20.8.4.2576", None, 0)).is_empty());
    }

    #[test]
    fn an_insert_is_in_use_with_a_name_a_plugin_or_a_channel() {
        let mut project = project("25.1.0.4858", Some(8), 8);
        project.inserts[2].name = Some("Drums".to_owned());
        let slot = |plugin: &str| Slot {
            plugin_name: Some(plugin.to_owned()),
            ..Slot::default()
        };
        project.inserts[3].slots = vec![slot("")];
        project.inserts[4].slots = vec![slot(""), slot("Fruity Limiter")];
        // The current track is the last insert of the file.
        project.inserts[7].name = Some("Current".to_owned());
        project.channels = vec![Channel {
            target_insert: Some(6),
            ..Channel::default()
        }];
        assert_eq!(in_use(&project), BTreeSet::from([2, 4, 6, 501]));
        assert_eq!(name(Some(&project.inserts[2]), 2), "Drums");
        assert_eq!(name(Some(&project.inserts[3]), 3), "Insert 3");
        assert_eq!(name(None, 0), "Master");
    }

    #[test]
    fn the_hints_are_in_fl_s_words() {
        assert_eq!(db_hint("Volume", 12_800), "Volume 0.0 dB");
        assert_eq!(db_hint("Volume", 16_000), "Volume 5.6 dB");
        assert_eq!(db_hint("Send", 0), "Send -∞ dB");
        assert_eq!(pan_hint(0), "Pan centred");
        assert_eq!(pan_hint(-6400), "Pan 100% left");
        assert_eq!(pan_hint(3200), "Pan 50% right");
    }

    #[test]
    fn a_fader_is_at_four_fifths_of_its_travel_at_0_db() {
        assert_eq!(fader_fraction(0), 0.0);
        assert_eq!(fader_fraction(UNITY), 0.8);
        assert_eq!(fader_fraction(FADER_TOP), 1.0);
        assert_eq!(fader_fraction(99_999), 1.0);
        assert_eq!(fader_fraction(-5), 0.0);
    }
}
