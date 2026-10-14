//! The channel rack: a row per channel with its switch, its pan and volume
//! knobs, its mixer insert and its name, and behind those the steps of the
//! pattern the top bar has selected.
//!
//! A click on a step lights or clears it, a right click clears it, and a
//! drag paints what its first step became. A step is a note of the
//! pattern, so a channel with notes that have a length shows those notes
//! and takes no steps: clearing a step would remove them. An audio clip
//! and an automation clip play no notes and show none.
//!
//! The graph button opens a lane under the rows with the velocity of each
//! step of the selected channel, and a drag in it draws velocities. A
//! click on a name selects its channel, and the up and down arrows move
//! the selection. Shift+wheel scrolls along the steps.
//!
//! A sample dragged out of the browser can be dropped on the row of a
//! sampler or an audio clip, which then plays it, or on the space under
//! the rows, which adds a sampler channel for it.
//!
//! Nothing here changes the project. An edit goes out as a
//! [`Command::Edit`], and the next frame draws what the app made of it.

use crate::browser::SampleDrag;
use crate::playlist::{rgb, text_on};
use crate::widgets::{Knob, drag_value, next_drag_id};
use crate::{Command, Edit, Grid, UiState};
use egui::{
    Align2, Color32, Context, FontId, Key, Modifiers, PointerButton, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use model::{Channel, ChannelKind, Pattern, Project};

/// The height of a channel's row.
const ROW: f32 = 22.0;
const LED: f32 = 12.0;
const KNOB: f32 = 20.0;
/// The width of the box with the insert's number.
const INSERT: f32 = 28.0;
/// The width of the button with the channel's name.
const NAME: f32 = 112.0;
/// The width a step takes, the gap to the next one included.
const STEP: f32 = 14.0;
/// The space between the parts of a row.
const GAP: f32 = 4.0;
/// The height of the velocity lane.
const LANE: f32 = 56.0;
/// How many points of drag move a channel to the next insert.
const INSERT_DRAG: f32 = 6.0;
/// The insert a drag stops at in a project that has no mixer: FL's last
/// insert before the current track.
const INSERTS: i32 = 125;
/// The top of a channel's volume and of its pan, and of a velocity.
const LEVEL_TOP: i32 = 12_800;
const VELOCITY_TOP: f32 = 128.0;
/// The colour of an enabled channel's LED and of a knob's arc.
const LIT: Color32 = Color32::from_rgb(120, 220, 110);

/// What the rack keeps from one frame to the next.
#[derive(Clone, Default)]
struct Memory {
    /// The selected channel, by its index. The first channel when `None`
    /// or when the project has no such channel.
    selected: Option<u16>,
    /// Whether the velocity lane is open.
    graph: bool,
    /// How far the steps are scrolled, in points.
    left: f32,
    /// The x the steps started at in the last frame, for the lane, which is
    /// drawn before the rows.
    steps_x: Option<f32>,
    paint: Option<Paint>,
}

/// A drag across steps, or across the velocity lane.
#[derive(Clone, Copy, PartialEq)]
struct Paint {
    /// What the edits of this drag carry as their `drag_id`.
    id: u64,
    channel: u16,
    /// What the drag makes of a step: lit or not, or in the lane `None`.
    on: Option<bool>,
    /// The step and the velocity the drag was at last. A drag across steps
    /// has no velocity, and keeps 0 there.
    sent: Option<(u32, u8)>,
}

/// The steps of the pattern on the screen.
#[derive(Clone, Copy)]
struct Steps {
    /// How many ticks a step is: a sixteenth note.
    ticks: u64,
    count: u32,
    /// How many steps a beat has, which is what the shading goes by.
    beat: u32,
    /// The x of step 0, which is left of the row when scrolled.
    origin: f32,
}

impl Steps {
    /// The pattern to the end of its last bar: the bar of its last note,
    /// or of its 164 length when that is further, and one bar at least.
    fn of(project: &Project, pattern: Option<&Pattern>) -> Self {
        let grid = Grid::of(project);
        let ticks = u64::from(project.header.ppq / 4).max(1);
        let placed = pattern.map_or(&[][..], |pattern| &pattern.notes);
        let ends = placed.iter().filter(|note| !note.is_deleted()).map(|note| {
            let start = u64::try_from(note.position).unwrap_or(0);
            start + u64::try_from(note.length).unwrap_or(0).max(1)
        });
        let length = pattern.and_then(|pattern| pattern.length).unwrap_or(0);
        let end = ends.max().unwrap_or(0).max(u64::from(length));
        let bars = end.div_ceil(grid.bar).max(1);
        Steps {
            ticks,
            count: u32::try_from((bars * grid.bar).div_ceil(ticks)).unwrap_or(u32::MAX),
            beat: u32::try_from(grid.beat / ticks).unwrap_or(1).max(1),
            origin: 0.0,
        }
    }

    fn width(self) -> f32 {
        self.count as f32 * STEP
    }

    /// The step at an x, when there is one.
    fn at(self, x: f32) -> Option<u32> {
        let step = ((x - self.origin) / STEP).floor();
        (step >= 0.0 && step < self.count as f32).then_some(step as u32)
    }

    /// The cell of a step in a row.
    fn cell(self, step: u32, row: Rect) -> Rect {
        let left = self.origin + step as f32 * STEP;
        Rect::from_min_max(
            pos2(left, row.top() + 1.0),
            pos2(left + STEP - 1.0, row.bottom() - 1.0),
        )
    }

    /// The steps that have a part in `row`.
    fn visible(self, row: Rect) -> std::ops::Range<u32> {
        let first = ((row.left() - self.origin) / STEP).floor().max(0.0) as u32;
        let last = ((row.right() - self.origin) / STEP).ceil().max(0.0) as u32;
        first.min(self.count)..last.min(self.count)
    }

    /// Whether the step is in a beat that gets the other shade.
    fn odd_beat(self, step: u32) -> bool {
        (step / self.beat) % 2 == 1
    }
}

fn edit(edit: Edit, drag_id: Option<u64>) -> Command {
    Command::Edit { edit, drag_id }
}

pub(crate) fn name(channel: &Channel) -> String {
    let name = channel.name.clone();
    name.unwrap_or_else(|| format!("Channel {}", channel.index))
}

/// A channel's volume as FL words it: a percentage of the knob's range,
/// so that a new channel is at 78%.
fn volume_hint(volume: i32) -> String {
    format!("Volume {:.0}%", volume as f32 / LEVEL_TOP as f32 * 100.0)
}

/// A channel's pan as FL words it: a percentage to one side.
fn pan_hint(pan: i32) -> String {
    let centre = LEVEL_TOP / 2;
    let percent = ((pan - centre) as f32 / centre as f32 * 100.0).round() as i32;
    match percent {
        0 => "Pan centred".to_owned(),
        left if left < 0 => format!("Pan {}% left", -left),
        right => format!("Pan {right}% right"),
    }
}

/// The highest insert a channel can play into: the last one in front of
/// the current track.
fn last_insert(project: &Project) -> i32 {
    let numbers = (0..project.inserts.len()).filter_map(|place| project.insert_index(place));
    let last = numbers.filter(|number| *number < 501).max();
    last.map_or(INSERTS, i32::from)
}

/// Whether the channel plays notes, and so has steps to set.
pub(crate) fn takes_notes(channel: &Channel) -> bool {
    !matches!(
        channel.kind,
        Some(ChannelKind::AudioClip | ChannelKind::Automation)
    )
}

/// Whether the channel plays a sample, and so takes one that is dropped
/// on it.
fn takes_sample(channel: &Channel) -> bool {
    matches!(
        channel.kind,
        Some(ChannelKind::Sampler | ChannelKind::AudioClip)
    )
}

/// Whether the channel has a note with a length in the pattern, which is
/// one the piano roll made.
fn has_long_notes(pattern: Option<&Pattern>, channel: u16) -> bool {
    let notes = pattern.map_or(&[][..], |pattern| &pattern.notes);
    let mut own = notes.iter().filter(|note| note.channel == channel);
    own.any(|note| !note.is_deleted() && note.length > 0)
}

fn memory_id() -> egui::Id {
    egui::Id::new("fl-rs channel rack")
}

/// The channel the rack has selected, which is the one the piano roll
/// shows: the first channel when none is selected, or one the project
/// does not have.
pub(crate) fn selected_channel<'a>(ctx: &Context, project: &'a Project) -> Option<&'a Channel> {
    let memory: Option<Memory> = ctx.data(|data| data.get_temp(memory_id()));
    let selected = memory.and_then(|memory| memory.selected);
    let mut channels = project.channels.iter();
    let found = channels.find(|channel| Some(channel.index) == selected);
    found.or(project.channels.first())
}

/// Select the channel with this index, in the rack too.
pub(crate) fn select_channel(ctx: &Context, channel: u16) {
    ctx.data_mut(|data| {
        let memory: &mut Memory = data.get_temp_mut_or_default(memory_id());
        memory.selected = Some(channel);
    });
}

pub(crate) fn show(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let project = &*state.project;
    let pattern = project.pattern(state.pattern);
    let memory_id = memory_id();
    let mut memory: Memory = ui.data(|data| data.get_temp(memory_id)).unwrap_or_default();

    ui.horizontal(|ui| {
        let name = pattern.and_then(|pattern| pattern.name.clone());
        ui.strong(name.unwrap_or_else(|| format!("Pattern {}", state.pattern)));
        let graph = ui.selectable_label(memory.graph, "Graph");
        if graph
            .on_hover_text("Show the velocity of the selected channel's steps")
            .clicked()
        {
            memory.graph = !memory.graph;
        }
    });
    if project.channels.is_empty() {
        ui.centered_and_justified(|ui| ui.weak("The project has no channels"));
        ui.data_mut(|data| data.insert_temp(memory_id, memory));
        return;
    }

    // The selection, and the arrows that move it.
    let is_selected = |channel: &Channel| Some(channel.index) == memory.selected;
    let mut selected = project.channels.iter().position(is_selected).unwrap_or(0);
    let mut moved = false;
    if ui.memory(|egui| egui.focused().is_none()) {
        let pressed = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
        if pressed(Key::ArrowUp) && selected > 0 {
            (selected, moved) = (selected - 1, true);
        }
        if pressed(Key::ArrowDown) && selected + 1 < project.channels.len() {
            (selected, moved) = (selected + 1, true);
        }
    }
    memory.selected = Some(project.channels[selected].index);

    let whole = ui.available_rect_before_wrap();
    let mut steps = Steps::of(project, pattern);
    // The step that plays. In song mode the clock is the song's.
    let playing = (state.playing && !state.song_mode).then(|| state.position / steps.ticks);
    let rack = Rack {
        project,
        pattern,
        playing,
        last_insert: last_insert(project),
    };

    if memory.graph {
        egui::Panel::bottom("fl-rs channel rack graph")
            .exact_size(LANE)
            .show(ui, |ui| {
                let left = memory.steps_x.unwrap_or(ui.max_rect().left());
                steps.origin = left - memory.left;
                let channel = &project.channels[selected];
                rack.lane(ui, channel, steps, left, &mut memory.paint, out);
            });
    }
    // A sample on its way from the browser, and whether this is the frame
    // it is let go in.
    let sample = egui::DragAndDrop::payload::<SampleDrag>(ui.ctx());
    let dropped = ui.input(|input| input.pointer.any_released());
    let mut shown = 0.0;
    // Where the last row ends, for the space under the rows.
    let mut rows_end = whole.top();
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(GAP, 1.0);
            for (place, channel) in project.channels.iter().enumerate() {
                let row = ui.horizontal(|ui| {
                    let selected = place == selected;
                    if rack.controls(ui, channel, selected, moved && selected, out) {
                        memory.selected = Some(channel.index);
                    }
                    let size = vec2(ui.available_width().max(STEP), ROW);
                    let (area, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
                    steps.origin = area.left() - memory.left;
                    memory.steps_x = Some(area.left());
                    shown = area.width();
                    rack.steps(ui, channel, steps, area, &response, &mut memory.paint, out);
                });
                let row = row.response.rect;
                rows_end = row.bottom();
                if let Some(sample) = &sample
                    && takes_sample(channel)
                    && ui.rect_contains_pointer(row)
                {
                    let stroke = Stroke::new(1.5, LIT);
                    ui.painter()
                        .rect_stroke(row, 2.0, stroke, StrokeKind::Inside);
                    if dropped {
                        let (channel, path) = (channel.index, sample.0.clone());
                        out.push(edit(Edit::SetChannelSample { channel, path }, None));
                    }
                }
            }
        });
    let free = whole.with_min_y(rows_end.clamp(whole.top(), whole.bottom()));
    if let Some(sample) = &sample
        && ui.rect_contains_pointer(free)
    {
        let stroke = Stroke::new(1.5, LIT);
        ui.painter()
            .rect_stroke(free.shrink(2.0), 2.0, stroke, StrokeKind::Inside);
        if dropped {
            let path = sample.0.clone();
            out.push(edit(Edit::AddSamplerChannel { path }, None));
        }
    }

    // The scroll area above took the wheel on its own, and what is left
    // of it goes along the steps.
    if ui.rect_contains_pointer(whole) {
        memory.left -= ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta.x));
    }
    memory.left = memory.left.clamp(0.0, (steps.width() - shown).max(0.0));
    ui.data_mut(|data| data.insert_temp(memory_id, memory));
}

/// What every row is drawn from.
struct Rack<'a> {
    project: &'a Project,
    pattern: Option<&'a Pattern>,
    /// The step the clock is in, when a pattern plays.
    playing: Option<u64>,
    last_insert: i32,
}

impl Rack<'_> {
    /// The parts of a row in front of its steps. True when a click on the
    /// name selected the channel.
    fn controls(
        &self,
        ui: &mut Ui,
        channel: &Channel,
        selected: bool,
        scroll_to: bool,
        out: &mut Vec<Command>,
    ) -> bool {
        let index = channel.index;
        let name = name(channel);
        let enabled = channel.enabled != Some(false);

        let (led, response) = ui.allocate_exact_size(vec2(LED, ROW), Sense::click());
        let label = format!("{name} on");
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, enabled, &label));
        let visuals = ui.visuals();
        let fill = if enabled {
            LIT
        } else {
            visuals.extreme_bg_color
        };
        ui.painter().circle(
            led.center(),
            LED / 2.0 - 1.5,
            fill,
            visuals.widgets.inactive.fg_stroke,
        );
        if response.on_hover_text("Channel on or off").clicked() {
            let enabled = !enabled;
            out.push(edit(
                Edit::SetChannelEnabled {
                    channel: index,
                    enabled,
                },
                None,
            ));
        }

        // An automation clip keeps its range where a channel has its pan
        // and its volume.
        let levels = channel.kind != Some(ChannelKind::Automation);
        let level = |value: Option<u32>, default: i32| {
            let value = value.filter(|_| levels);
            value.map_or(default, |value| i32::try_from(value).unwrap_or(LEVEL_TOP))
        };
        let pan = level(channel.pan(), LEVEL_TOP / 2);
        let knob = Knob {
            label: &format!("{name} pan"),
            value: pan,
            range: 0..=LEVEL_TOP,
            default: LEVEL_TOP / 2,
            hint: pan_hint(pan),
            colour: LIT,
            enabled: levels,
        };
        if let Some((pan, drag_id)) = knob.show(ui, KNOB) {
            let pan = pan as u32;
            out.push(edit(
                Edit::SetChannelPan {
                    channel: index,
                    pan,
                },
                drag_id,
            ));
        }
        let volume = level(channel.volume(), 10_000);
        let knob = Knob {
            label: &format!("{name} volume"),
            value: volume,
            range: 0..=LEVEL_TOP,
            default: 10_000,
            hint: volume_hint(volume),
            colour: LIT,
            enabled: levels,
        };
        if let Some((volume, drag_id)) = knob.show(ui, KNOB) {
            let volume = volume as u32;
            out.push(edit(
                Edit::SetChannelVolume {
                    channel: index,
                    volume,
                },
                drag_id,
            ));
        }

        let insert = channel.insert().unwrap_or(0);
        let size = vec2(INSERT, ROW - 4.0);
        let (plate, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let label = format!("{name} insert");
        response.widget_info(|| {
            let mut info = WidgetInfo::labeled(WidgetType::DragValue, true, &label);
            info.value = Some(f64::from(insert));
            info
        });
        let visuals = ui.visuals();
        ui.painter()
            .rect_filled(plate, 2.0, visuals.extreme_bg_color);
        // The master is where a channel plays when it names no insert.
        let text = match insert {
            0 => "---".to_owned(),
            insert => insert.to_string(),
        };
        ui.painter().text(
            plate.center(),
            Align2::CENTER_CENTER,
            text,
            FontId::monospace(10.0),
            visuals.text_color(),
        );
        let travel = self.last_insert as f32 * INSERT_DRAG;
        if let Some((insert, drag)) =
            drag_value(ui, &response, insert, 0..=self.last_insert, travel)
        {
            let insert = insert as u16;
            out.push(edit(
                Edit::SetChannelInsert {
                    channel: index,
                    insert,
                },
                Some(drag),
            ));
        }
        response.on_hover_text("The mixer insert the channel plays into. Drag to change it");

        let (button, response) = ui.allocate_exact_size(vec2(NAME, ROW - 2.0), Sense::click());
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, &name));
        let visuals = ui.visuals();
        let plain = visuals.widgets.inactive.bg_fill;
        // Half the channel's colour and half the button's.
        let fill = channel.color.map_or(plain, |colour| {
            let (colour, plain) = (rgb(colour).to_array(), plain.to_array());
            let mix = |at: usize| ((u16::from(colour[at]) + u16::from(plain[at])) / 2) as u8;
            Color32::from_rgb(mix(0), mix(1), mix(2))
        });
        let text = if enabled {
            text_on(fill)
        } else {
            text_on(fill).gamma_multiply(0.5)
        };
        let painter = ui.painter_at(button);
        painter.rect_filled(button, 2.0, fill);
        painter.text(
            pos2(button.left() + 5.0, button.center().y),
            Align2::LEFT_CENTER,
            &name,
            FontId::proportional(11.0),
            text,
        );
        if selected {
            let stroke = Stroke::new(1.5, LIT);
            painter.rect_stroke(button, 2.0, stroke, StrokeKind::Inside);
        }
        if scroll_to {
            response.scroll_to_me(None);
        }
        response.clicked()
    }

    /// The steps of a channel, and the clicks and drags on them.
    #[expect(clippy::too_many_arguments)]
    fn steps(
        &self,
        ui: &Ui,
        channel: &Channel,
        steps: Steps,
        area: Rect,
        response: &Response,
        paint: &mut Option<Paint>,
        out: &mut Vec<Command>,
    ) {
        let index = channel.index;
        let painter = ui.painter_at(area);
        let visuals = ui.visuals();
        let name = name(channel);
        response
            .widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, format!("{name} steps")));
        if !takes_notes(channel) {
            return;
        }
        let ppq = self.project.header.ppq;
        if has_long_notes(self.pattern, index) {
            self.notes(&painter, index, steps, area, visuals.text_color());
            response
                .clone()
                .on_hover_text("These notes have a length, so the piano roll edits them");
            return;
        }

        let lit = self.pattern.map(|pattern| pattern.steps(index, ppq));
        let lit = |step: u32| {
            let lit = lit.as_ref().and_then(|lit| lit.get(step as usize));
            lit.copied().unwrap_or(false)
        };
        for step in steps.visible(area) {
            let odd = steps.odd_beat(step);
            let fill = match (lit(step), odd) {
                (true, false) => Color32::from_rgb(196, 204, 212),
                (true, true) => Color32::from_rgb(226, 190, 186),
                (false, false) => Color32::from_rgb(58, 64, 70),
                (false, true) => Color32::from_rgb(80, 60, 60),
            };
            let cell = steps.cell(step, area);
            painter.rect_filled(cell, 2.0, fill);
            if self.playing == Some(u64::from(step)) {
                let mark = cell.with_min_y(cell.bottom() - 3.0);
                painter.rect_filled(mark, 0.0, LIT);
            }
        }

        let set = |step: u32, on: bool, drag_id: Option<u64>| {
            edit(
                Edit::SetStep {
                    channel: index,
                    step,
                    on,
                },
                drag_id,
            )
        };
        let pointer = response.interact_pointer_pos();
        let under = pointer.and_then(|pos| steps.at(pos.x));
        if let Some(step) = under.filter(|_| response.clicked()) {
            out.push(set(step, !lit(step), None));
        }
        if let Some(step) = under.filter(|_| response.secondary_clicked())
            && lit(step)
        {
            out.push(set(step, false, None));
        }

        // A drag paints from the step the button went down on.
        let pressed = ui.input(|input| input.pointer.press_origin());
        let pressed = pressed.and_then(|pos| steps.at(pos.x));
        let primary = response.drag_started_by(PointerButton::Primary);
        if (primary || response.drag_started_by(PointerButton::Secondary))
            && let Some(first) = pressed
        {
            let on = primary && !lit(first);
            let id = next_drag_id(ui.ctx());
            if lit(first) != on {
                out.push(set(first, on, Some(id)));
            }
            *paint = Some(Paint {
                id,
                channel: index,
                on: Some(on),
                sent: Some((first, 0)),
            });
        }
        let Some(drag) = paint.as_mut().filter(|drag| drag.channel == index) else {
            return;
        };
        let Some(on) = drag.on else {
            return;
        };
        if !response.dragged() {
            *paint = None;
            return;
        }
        if let Some(step) = under.filter(|step| drag.sent != Some((*step, 0))) {
            drag.sent = Some((step, 0));
            if lit(step) != on {
                out.push(set(step, on, Some(drag.id)));
            }
        }
    }

    /// The channel's notes as the piano roll has them, small: a line per
    /// note, as long as the note and as high as its key is among the
    /// channel's keys.
    fn notes(&self, painter: &egui::Painter, channel: u16, steps: Steps, area: Rect, ink: Color32) {
        let notes = self.pattern.map_or(&[][..], |pattern| &pattern.notes);
        let own = || {
            let own = notes.iter().filter(move |note| note.channel == channel);
            own.filter(|note| !note.is_deleted())
        };
        let keys = own().map(|note| note.key);
        let (low, high) = (keys.clone().min().unwrap_or(0), keys.max().unwrap_or(0));
        let span = f32::from((high - low).max(1));
        let x = |tick: i32| steps.origin + tick as f32 / steps.ticks as f32 * STEP;
        painter.rect_filled(
            area.shrink2(vec2(0.0, 1.0)),
            2.0,
            Color32::from_rgb(44, 48, 52),
        );
        for note in own() {
            let up = f32::from(note.key - low) / span;
            let y = area.bottom() - 4.0 - up * (area.height() - 8.0);
            let end = x(note.position.saturating_add(note.length.max(1)));
            let line = Rect::from_min_max(
                pos2(x(note.position), y - 1.0),
                pos2(end.max(x(note.position) + 2.0), y + 1.0),
            );
            let ink = if note.muted() {
                ink.gamma_multiply(0.3)
            } else {
                ink
            };
            painter.rect_filled(line, 0.0, ink);
        }
    }

    /// The velocity lane: a bar for each lit step of the channel, as high
    /// as the velocity of the note that lights it. A click or a drag sets
    /// the velocity of the step under the pointer to where the pointer is.
    /// `left` is the x the rows' steps start at.
    fn lane(
        &self,
        ui: &mut Ui,
        channel: &Channel,
        steps: Steps,
        left: f32,
        paint: &mut Option<Paint>,
        out: &mut Vec<Command>,
    ) {
        let index = channel.index;
        let ppq = self.project.header.ppq;
        let whole = ui.max_rect();
        let area = whole.with_min_x(left.clamp(whole.left(), whole.right()));
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        let name = name(channel);
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Other, true, format!("{name} velocity"))
        });
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, ui.visuals().extreme_bg_color);
        let editable = takes_notes(channel) && !has_long_notes(self.pattern, index);
        let Some(pattern) = self.pattern.filter(|_| editable) else {
            return;
        };

        let height = area.height() - 2.0;
        for step in steps.visible(area) {
            let Some(note) = pattern.step_note(index, step, ppq) else {
                continue;
            };
            let velocity = f32::from(pattern.notes[note].velocity) / VELOCITY_TOP;
            let cell = steps.cell(step, area);
            let bar = cell.with_min_y(cell.bottom() - (velocity * height).max(1.0));
            painter.rect_filled(bar, 0.0, LIT);
        }

        let held = response.dragged_by(PointerButton::Primary);
        if response.drag_started_by(PointerButton::Primary) {
            *paint = Some(Paint {
                id: next_drag_id(ui.ctx()),
                channel: index,
                on: None,
                sent: None,
            });
        }
        let Some(pos) = response.interact_pointer_pos() else {
            return;
        };
        let Some(step) = steps.at(pos.x) else {
            return;
        };
        let velocity = ((area.bottom() - 1.0 - pos.y) / height).clamp(0.0, 1.0) * VELOCITY_TOP;
        let velocity = velocity.round() as u8;
        let set = Edit::SetStepVelocity {
            channel: index,
            step,
            velocity,
        };
        let lit = pattern.step_note(index, step, ppq).is_some();
        if response.clicked() && lit {
            out.push(edit(set, None));
        } else if held
            && let Some(drag) = paint.as_mut().filter(|drag| drag.on.is_none())
            && drag.sent != Some((step, velocity))
        {
            drag.sent = Some((step, velocity));
            if lit {
                out.push(edit(set, Some(drag.id)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::Note;

    fn project(ppq: u16, signature: (u8, u8)) -> Project {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 0,
            ppq,
        });
        (project.time_sig_num, project.time_sig_beat) = (Some(signature.0), Some(signature.1));
        project
    }

    fn note(position: i32, length: i32) -> Note {
        Note {
            position,
            length,
            ..Note::default()
        }
    }

    #[test]
    fn the_steps_run_to_the_end_of_the_pattern_s_last_bar() {
        let four = project(96, (4, 4));
        let steps = Steps::of(&four, None);
        assert_eq!((steps.ticks, steps.count, steps.beat), (24, 16, 4));
        let mut pattern = Pattern {
            notes: vec![note(0, 0), note(383, 0)],
            ..Pattern::default()
        };
        assert_eq!(Steps::of(&four, Some(&pattern)).count, 16);
        // A note that starts in the second bar, and one that ends there.
        pattern.notes[1] = note(384, 0);
        assert_eq!(Steps::of(&four, Some(&pattern)).count, 32);
        pattern.notes[1] = note(380, 8);
        assert_eq!(Steps::of(&four, Some(&pattern)).count, 32);
        // A deleted record is no note, and the 164 length counts.
        (pattern.notes[1].tag, pattern.notes[1].channel) = (u16::MAX, u16::MAX);
        assert_eq!(Steps::of(&four, Some(&pattern)).count, 16);
        pattern.length = Some(3 * 384);
        assert_eq!(Steps::of(&four, Some(&pattern)).count, 48);

        // Six eighths are twelve steps in beats of two, and at PPQ 48 a
        // step is twelve ticks.
        let steps = Steps::of(&project(48, (6, 8)), None);
        assert_eq!((steps.ticks, steps.count, steps.beat), (12, 12, 2));
    }

    #[test]
    fn a_step_is_fourteen_points_from_the_scrolled_origin() {
        let steps = Steps {
            origin: 100.0 - 21.0,
            ..Steps::of(&project(96, (4, 4)), None)
        };
        assert_eq!(steps.at(78.0), None);
        assert_eq!(steps.at(79.0), Some(0));
        assert_eq!(steps.at(100.0), Some(1));
        assert_eq!(steps.at(79.0 + 16.0 * 14.0 - 1.0), Some(15));
        assert_eq!(steps.at(79.0 + 16.0 * 14.0), None);
        let row = Rect::from_min_max(pos2(100.0, 50.0), pos2(170.0, 72.0));
        assert_eq!(steps.visible(row), 1..7);
        assert_eq!(
            steps.cell(2, row),
            Rect::from_min_max(pos2(107.0, 51.0), pos2(120.0, 71.0))
        );
        let odd: Vec<bool> = (0..9).map(|step| steps.odd_beat(step)).collect();
        assert_eq!(
            odd,
            [false, false, false, false, true, true, true, true, false]
        );
    }

    #[test]
    fn the_hints_are_in_fl_s_words() {
        assert_eq!(volume_hint(10_000), "Volume 78%");
        assert_eq!(volume_hint(12_800), "Volume 100%");
        assert_eq!(pan_hint(6400), "Pan centred");
        assert_eq!(pan_hint(0), "Pan 100% left");
        assert_eq!(pan_hint(9600), "Pan 50% right");
    }

    #[test]
    fn a_channel_with_long_notes_or_without_notes_takes_no_steps() {
        let mut pattern = Pattern {
            notes: vec![note(0, 0), note(96, 48)],
            ..Pattern::default()
        };
        pattern.notes[1].channel = 3;
        assert!(!has_long_notes(Some(&pattern), 0));
        assert!(has_long_notes(Some(&pattern), 3));
        assert!(!has_long_notes(None, 3));
        let kind = |kind| Channel {
            kind: Some(kind),
            ..Channel::default()
        };
        assert!(takes_notes(&kind(ChannelKind::Sampler)));
        assert!(takes_notes(&kind(ChannelKind::Plugin)));
        assert!(takes_notes(&Channel::default()));
        assert!(!takes_notes(&kind(ChannelKind::AudioClip)));
        assert!(!takes_notes(&kind(ChannelKind::Automation)));
        // A sample goes to the two kinds that play one.
        assert!(takes_sample(&kind(ChannelKind::Sampler)));
        assert!(takes_sample(&kind(ChannelKind::AudioClip)));
        assert!(!takes_sample(&kind(ChannelKind::Plugin)));
        assert!(!takes_sample(&Channel::default()));
    }

    #[test]
    fn a_channel_can_go_to_any_insert_in_front_of_the_current_track() {
        let mut project = project(96, (4, 4));
        assert_eq!(last_insert(&project), 125);
        project.writer_version = Some("20.8.4.2576".to_owned());
        project.inserts = vec![model::Insert::default(); 127];
        assert_eq!(last_insert(&project), 125);
    }
}
