//! The playlist: a row per track, a rectangle per item, a line for the
//! playhead, and the mouse edits.
//!
//! A click selects an item, and with Shift adds it to the selection. A drag
//! moves the selection, a drag by an item's right edge resizes it, and
//! Ctrl+drag moves a copy. A right click mutes or unmutes and Delete
//! removes. A drag snaps to the grid unless Alt is held. Ctrl+wheel zooms
//! around the pointer, the wheel scrolls through the tracks and Shift+wheel
//! along the timeline. A click on the ruler seeks.
//!
//! Nothing here changes the project. An edit goes out as a
//! [`Command::Edit`], and the next frame draws what the app made of it.

use crate::widgets::next_drag_id;
use crate::{Command, Edit, Grid, ItemId, UiState};
use egui::{
    Align2, Color32, Context, CursorIcon, FontId, Key, PointerButton, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, Ui, pos2, vec2,
};
use engine::automation::Curve;
use flp::event::layout::Point;
use model::{ChannelKind, ItemSource, Note, PlaylistItem, Project, Trim};

/// The width of the column of track names.
const NAMES: f32 = 96.0;
const RULER: f32 = 18.0;
/// The lowest and the highest a row gets. Rows that do not fit scroll.
const ROW: std::ops::RangeInclusive<f32> = 8.0..=22.0;
/// Empty rows under the last track that has an item, to move items to.
const SPARE_ROWS: usize = 4;
/// Empty bars past the end of the song that the timeline scrolls to.
const SPARE_BARS: u64 = 4;
/// The colour of an item whose pattern or channel has none.
const PLAIN: Color32 = Color32::from_rgb(92, 112, 128);
/// A press this near an item's right edge resizes it.
const EDGE: f32 = 5.0;
/// A drag snaps to steps that are at least this wide, in points.
const SNAP: f32 = 16.0;
/// How far the timeline zooms out, where 1 fits the song in the window.
const ZOOM_OUT: f32 = 0.25;
/// How wide a beat gets when zoomed all the way in, in points.
const BEAT_WIDTH: f32 = 256.0;

/// One item as the playlist draws it.
struct Block<'a> {
    id: ItemId,
    /// The row, where track 1 is row 0.
    row: usize,
    start: u64,
    end: u64,
    colour: Color32,
    name: String,
    muted: bool,
    selected: bool,
    /// Of an automation clip: its points, and how many beats into them the
    /// item starts.
    curve: Option<(&'a [Point], f32)>,
    /// Of a pattern: its notes, and the tick of the pattern the item
    /// starts at.
    notes: Option<(&'a [Note], i64)>,
}

/// The items of the current arrangement that place something.
fn blocks(project: &Project) -> Vec<Block<'_>> {
    let items = project.current_arrangement().map_or(&[][..], |a| &a.items);
    let version = project.version();
    let placed = items.iter().enumerate();
    placed
        .filter_map(|(id, item)| block(project, id, item, version))
        .collect()
}

fn block<'a>(
    project: &'a Project,
    id: ItemId,
    item: &PlaylistItem,
    version: u32,
) -> Option<Block<'a>> {
    let (mut curve, mut notes) = (None, None);
    let (colour, name) = match item.source()? {
        ItemSource::Pattern(index) => {
            let pattern = project.pattern(index);
            let offset = match item.trim() {
                Some(Trim::Ticks { start, .. }) => i64::from(start),
                _ => 0,
            };
            notes = pattern.map(|pattern| (&pattern.notes[..], offset));
            let name = pattern.and_then(|pattern| pattern.name.clone());
            (
                pattern.and_then(|pattern| pattern.color),
                name.unwrap_or_else(|| format!("Pattern {index}")),
            )
        }
        ItemSource::Channel(index) => {
            let channel = project.channel(index);
            let name = channel.and_then(|channel| channel.name.clone());
            let automation = channel
                .filter(|channel| channel.kind == Some(ChannelKind::Automation))
                .and_then(|channel| channel.automation.as_ref());
            if let Some(automation) = automation {
                let offset = match item.trim() {
                    Some(Trim::Clip { start, .. }) => start,
                    _ => 0.0,
                };
                curve = Some((&automation.envelope[..], offset));
            }
            (
                channel.and_then(|channel| channel.color),
                name.unwrap_or_else(|| format!("Channel {index}")),
            )
        }
    };
    let start = u64::try_from(item.position).unwrap_or(0);
    Some(Block {
        id,
        row: usize::try_from(item.track(version) - 1).ok()?,
        start,
        end: start + u64::try_from(item.length).unwrap_or(0),
        colour: colour.map_or(PLAIN, rgb),
        name,
        muted: item.muted(),
        selected: item.selected(),
        curve,
        notes,
    })
}

/// A colour as FL stores it: red in the low byte, then green, then blue.
pub(crate) fn rgb(colour: u32) -> Color32 {
    let [red, green, blue, _] = colour.to_le_bytes();
    Color32::from_rgb(red, green, blue)
}

/// Black or white, whichever reads better on `fill`.
pub(crate) fn text_on(fill: Color32) -> Color32 {
    let luma =
        0.299 * f32::from(fill.r()) + 0.587 * f32::from(fill.g()) + 0.114 * f32::from(fill.b());
    if luma > 140.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// How many bars lie between two ruler labels, so that the labels are at
/// least 48 points apart.
pub(crate) fn bar_step(bar_width: f32) -> u64 {
    let mut step = 1;
    while bar_width * (step as f32) < 48.0 && step < 1 << 20 {
        step *= 2;
    }
    step
}

/// The step a drag snaps to, in ticks: a beat, halved for as long as the
/// half is `SNAP` points wide.
pub(crate) fn snap_step(grid: Grid, scale: f32) -> u64 {
    let mut step = grid.beat;
    while step.is_multiple_of(2) && (step / 2) as f32 * scale >= SNAP {
        step /= 2;
    }
    step
}

/// The multiple of `step` nearest to `tick`.
pub(crate) fn snap(tick: f32, step: u64) -> i64 {
    (tick / step as f32).round() as i64 * step as i64
}

/// Where the timeline's ticks are on the screen.
#[derive(Clone, Copy)]
pub(crate) struct Lane {
    /// The x of the timeline's left edge, right of the track names.
    pub origin: f32,
    /// The tick at that edge.
    pub left: f32,
    /// Points per tick.
    pub scale: f32,
}

impl Lane {
    pub fn x(self, tick: f32) -> f32 {
        self.origin + (tick - self.left) * self.scale
    }

    pub fn tick(self, x: f32) -> f32 {
        (x - self.origin) / self.scale + self.left
    }

    /// The bars that get a line, one in `step`, with the x of each, up to
    /// the x `right`.
    pub fn bars(self, grid: Grid, step: u64, right: f32) -> impl Iterator<Item = (u64, f32)> {
        let bar_of = |x: f32| self.tick(x).max(0.0) as u64 / grid.bar;
        let first = bar_of(self.origin) / step * step;
        (first..=bar_of(right - 1.0))
            .step_by(step as usize)
            .map(move |bar| (bar, self.x((bar * grid.bar) as f32)))
    }
}

/// What the playlist keeps from one frame to the next.
#[derive(Clone)]
struct Memory {
    /// How far the timeline is zoomed in, where 1 fits the song in the
    /// window.
    zoom: f32,
    /// The tick at the timeline's left edge.
    left: f32,
    drag: Option<Drag>,
}

impl Default for Memory {
    fn default() -> Self {
        Memory {
            zoom: 1.0,
            left: 0.0,
            drag: None,
        }
    }
}

/// A drag of items that is under way.
#[derive(Clone)]
struct Drag {
    /// What the edits of this drag carry as their `drag_id`.
    id: u64,
    /// Whether the drag moves the items' ends, and not the items.
    resizes: bool,
    ids: Vec<ItemId>,
    /// The start and the end of the item under the pointer, as they were
    /// when the drag began.
    start: u64,
    end: u64,
    /// The tick and the row the pointer went down on.
    tick: f32,
    row: i32,
    /// Points per tick and the height of a row as they were when the drag
    /// began. A drag that makes the song longer or uses another track
    /// would otherwise move the grid under the pointer.
    scale: f32,
    height: f32,
    /// The edit that went out last, and to begin with the one that changes
    /// nothing.
    sent: Edit,
}

impl Drag {
    /// The edit that takes the items to where the pointer is, snapped to
    /// `step` ticks.
    fn edit(&self, tick: f32, row: i32, step: u64) -> Edit {
        let moved = tick - self.tick;
        let ids = self.ids.clone();
        if self.resizes {
            let length = snap(self.end as f32 + moved, step) - self.start as i64;
            // An end dragged to the start or past it leaves one step.
            let length = if length < 1 { step as i64 } else { length };
            Edit::ResizeItems {
                ids,
                new_length: length as u64,
            }
        } else {
            Edit::MoveItems {
                ids,
                delta_ticks: snap(self.start as f32 + moved, step) - self.start as i64,
                delta_tracks: row - self.row,
            }
        }
    }
}

/// The item under the pointer, for the status line.
#[derive(Clone, PartialEq)]
struct Hovered {
    text: Option<String>,
    /// The frame the playlist was drawn in.
    frame: u64,
}

fn hovered_id() -> egui::Id {
    egui::Id::new("fl-rs playlist hovered")
}

/// The name and the position of the item under the pointer. The status
/// line is drawn before the playlist, so this is what the frame before saw.
pub(crate) fn hovered(ctx: &Context) -> Option<String> {
    let hovered: Hovered = ctx.data(|data| data.get_temp(hovered_id()))?;
    // A playlist that is no longer drawn has nothing under the pointer.
    (hovered.frame + 1 >= ctx.cumulative_frame_nr())
        .then_some(hovered.text)
        .flatten()
}

fn set_hovered(ctx: &Context, text: Option<String>) {
    let before = hovered(ctx);
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|data| {
        data.insert_temp(
            hovered_id(),
            Hovered {
                text: text.clone(),
                frame,
            },
        )
    });
    if before != text {
        // The status line has drawn the old text already.
        ctx.request_repaint();
    }
}

/// The line of an automation clip across its item, a point for every two
/// points of the width that lies inside `visible`. `offset` is how many
/// beats into the clip the item starts.
fn curve(
    curve: &Curve,
    offset: f32,
    rect: Rect,
    beat_width: f32,
    visible: egui::Rangef,
) -> Vec<Pos2> {
    let (from, to) = (rect.left().max(visible.min), rect.right().min(visible.max));
    if to <= from {
        return Vec::new();
    }
    let steps = ((to - from) / 2.0).ceil() as usize;
    let point = |step: usize| {
        let x = (from + step as f32 * 2.0).min(to);
        let beats = offset + (x - rect.left()) / beat_width;
        let value = curve.value_at(f64::from(beats)).clamp(0.0, 1.0) as f32;
        pos2(x, rect.bottom() - 1.0 - value * (rect.height() - 2.0))
    };
    (0..=steps).map(point).collect()
}

/// The notes of a pattern that an item plays, each as its key and its
/// first and last tick on the song's timeline. A step, which has no length,
/// lasts a sixteenth.
fn notes_in<'a>(
    notes: &'a [Note],
    offset: i64,
    block: &Block,
    ppq: u16,
) -> impl Iterator<Item = (i16, i64, i64)> + Clone + 'a {
    let (start, end) = (block.start as i64, block.end as i64);
    let step = i64::from((ppq / 4).max(1));
    let placed = notes
        .iter()
        .filter(|note| !note.is_deleted() && !note.muted());
    placed.filter_map(move |note| {
        let from = start + i64::from(note.position) - offset;
        let to = from + i64::from(note.length).max(step);
        (to > start && from < end).then_some((note.key, from.max(start), to.min(end)))
    })
}

/// The notes an item plays, small, in `body`: a line per note, as long as
/// the note and as high as its key is among the item's keys.
fn pattern_notes(
    painter: &egui::Painter,
    notes: impl Iterator<Item = (i16, i64, i64)> + Clone,
    lane: Lane,
    body: Rect,
    visible: egui::Rangef,
    ink: Color32,
) {
    let keys = notes.clone().map(|(key, _, _)| key);
    let (low, high) = (keys.clone().min().unwrap_or(0), keys.max().unwrap_or(0));
    let span = f32::from((high - low).max(1));
    let room = (body.height() - 3.0).max(0.0);
    for (key, from, to) in notes {
        let (left, right) = (lane.x(from as f32), lane.x(to as f32));
        if right < visible.min || left > visible.max {
            continue;
        }
        let up = if high == low {
            0.5
        } else {
            f32::from(key - low) / span
        };
        let y = body.bottom() - 1.5 - up * room;
        let line = Rect::from_min_max(pos2(left, y - 0.5), pos2(right.max(left + 1.0), y + 0.5));
        painter.rect_filled(line, 0.0, ink);
    }
}

fn edit(edit: Edit) -> Command {
    Command::Edit {
        edit,
        drag_id: None,
    }
}

pub(crate) fn show(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let project = &*state.project;
    let grid = Grid::of(project);
    let blocks = blocks(project);
    // The song to the end of its last bar, and four bars for an empty one.
    let bars = project.song_length_ticks().div_ceil(grid.bar).max(4);
    let ticks = bars * grid.bar;
    let used = blocks.iter().map(|block| block.row + 1).max().unwrap_or(0);
    let selected = || -> Vec<ItemId> {
        let selected = blocks.iter().filter(|block| block.selected);
        selected.map(|block| block.id).collect()
    };

    let font = FontId::proportional(10.0);
    let line = ui.visuals().widgets.noninteractive.bg_stroke;
    let weak = ui.visuals().weak_text_color();
    let playhead = Stroke::new(1.5, Color32::from_rgb(120, 220, 110));

    let memory_id = egui::Id::new("fl-rs playlist");
    let mut memory: Memory = ui.data(|data| data.get_temp(memory_id)).unwrap_or_default();
    let whole = ui.available_rect_before_wrap();
    let width = ui.available_width();
    let fit = (width - NAMES).max(1.0) / ticks as f32;
    let dragged = memory.drag.as_ref();
    let scale = dragged.map_or(fit * memory.zoom, |drag| drag.scale);
    let left = memory.left;
    let lane = |edge: f32| Lane {
        origin: edge + NAMES,
        left,
        scale,
    };
    let step = bar_step(grid.bar as f32 * scale);

    // The ruler: a number on every `step` bars, and a click seeks.
    let (ruler, response) = ui.allocate_exact_size(vec2(width, RULER), Sense::click());
    let ruler_lane = lane(ruler.left());
    let painter = ui.painter_at(ruler);
    painter.rect_filled(ruler, 0.0, ui.visuals().faint_bg_color);
    let painter = ui.painter_at(ruler.with_min_x(ruler_lane.origin));
    for (bar, x) in ruler_lane.bars(grid, step, ruler.right()) {
        painter.vline(x, ruler.y_range(), line);
        let at = pos2(x + 3.0, ruler.center().y);
        painter.text(at, Align2::LEFT_CENTER, bar + 1, font.clone(), weak);
    }
    if let Some(pointer) = response
        .interact_pointer_pos()
        .filter(|_| response.clicked())
    {
        let tick = ruler_lane.tick(pointer.x);
        out.push(Command::Seek(tick.clamp(0.0, ticks as f32) as u64));
    }
    painter.vline(
        ruler_lane.x(state.position as f32),
        ruler.y_range(),
        playhead,
    );

    if used == 0 {
        set_hovered(ui.ctx(), None);
        ui.centered_and_justified(|ui| ui.weak("The playlist is empty"));
        return;
    }

    let nothing_focused = ui.memory(|egui| egui.focused().is_none());
    if nothing_focused && ui.input(|input| input.key_pressed(Key::Delete)) {
        let ids = selected();
        if !ids.is_empty() {
            out.push(edit(Edit::DeleteItems { ids }));
        }
    }

    let arrangement = project.current_arrangement();
    let names = arrangement.map_or(&[][..], |a| &a.tracks);
    let item_count = arrangement.map_or(0, |a| a.items.len());
    let rows = used + SPARE_ROWS;
    let mut hover = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let fitted = ui.available_height() / rows as f32;
            let fitted = fitted.clamp(*ROW.start(), *ROW.end());
            let row = memory.drag.as_ref().map_or(fitted, |drag| drag.height);
            let size = vec2(width, row * rows as f32);
            let (area, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let lane = lane(area.left());
            let painter = ui.painter_at(area);
            painter.rect_filled(area, 0.0, ui.visuals().extreme_bg_color);

            for index in 0..rows {
                let top = area.top() + index as f32 * row;
                let name = Rect::from_min_size(pos2(area.left(), top), vec2(NAMES, row));
                painter.rect_filled(name, 0.0, ui.visuals().faint_bg_color);
                painter.hline(area.x_range(), top, line);
                if row >= 12.0 {
                    let track = names.iter().find(|track| track.index as usize == index + 1);
                    // A track with a name of its own reads brighter than
                    // one with its number.
                    let (text, ink) = match track.and_then(|track| track.name.as_deref()) {
                        Some(name) => (name.to_owned(), ui.visuals().text_color()),
                        None => (format!("Track {}", index + 1), weak),
                    };
                    let at = pos2(name.left() + 4.0, name.center().y);
                    painter.with_clip_rect(name).text(
                        at,
                        Align2::LEFT_CENTER,
                        text,
                        font.clone(),
                        ink,
                    );
                }
            }
            // From here on everything stays right of the track names.
            let painter = ui.painter_at(area.with_min_x(lane.origin));
            // A faint line on every beat when the beats have the room.
            let beat = grid.beat as f32 * scale;
            if step == 1 && beat >= 8.0 {
                let faint = Stroke::new(1.0, line.color.gamma_multiply(0.45));
                for (_, x) in lane.bars(grid, 1, area.right()) {
                    for n in 1..grid.bar / grid.beat {
                        painter.vline(x + n as f32 * beat, area.y_range(), faint);
                    }
                }
            }
            for (_, x) in lane.bars(grid, step, area.right()) {
                painter.vline(x, area.y_range(), line);
            }

            let rect_of = |block: &Block| {
                let top = area.top() + block.row as f32 * row;
                let left = lane.x(block.start as f32);
                // A point short of its end, so that two items in a row
                // show as two, and never thinner than a point.
                let right = (lane.x(block.end as f32) - 1.0).max(left + 1.0);
                Rect::from_min_max(pos2(left, top + 1.0), pos2(right, top + row))
            };
            for block in &blocks {
                let rect = rect_of(block);
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                // A muted item is a shade of its colour with faint text.
                let (fill, text) = if block.muted {
                    (block.colour.gamma_multiply(0.3), weak)
                } else {
                    (block.colour, text_on(block.colour))
                };
                // As in FL, a row with the room has the name on a strip of
                // the item's colour, over a darker body that shows what the
                // item plays.
                let titled = row >= 16.0;
                let title = if titled {
                    rect.with_max_y(rect.top() + (rect.height() * 0.55).clamp(10.0, 14.0))
                } else {
                    rect
                };
                let body = rect.with_min_y(title.bottom());
                let shade = fill.lerp_to_gamma(Color32::BLACK, 0.45);
                painter.rect_filled(rect, 1.0, shade);
                painter.rect_filled(title, 1.0, fill);
                let inside = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
                let visible = inside.clip_rect().x_range();
                let ink = fill.lerp_to_gamma(Color32::WHITE, 0.25);
                if let Some((points, offset)) = block.curve
                    && row >= 12.0
                {
                    let (rect, ink) = if titled { (body, ink) } else { (rect, text) };
                    let beat = f32::from(project.header.ppq) * scale;
                    let points = curve(&Curve::new(points), offset, rect, beat, visible);
                    inside.add(Shape::line(points, Stroke::new(1.0, ink)));
                }
                if let Some((notes, offset)) = block.notes
                    && titled
                {
                    let ppq = project.header.ppq;
                    let placed = notes_in(notes, offset, block, ppq);
                    pattern_notes(&inside, placed, lane, body, visible, ink);
                }
                if row >= 12.0 && rect.width() >= 32.0 {
                    // The name stays in view when the item starts left of it.
                    let left = rect.left().max(lane.origin);
                    let at = pos2(left + 3.0, title.center().y);
                    inside.text(at, Align2::LEFT_CENTER, &block.name, font.clone(), text);
                }
                if block.selected {
                    let stroke = Stroke::new(1.5, ui.visuals().strong_text_color());
                    painter.rect_stroke(rect, 1.0, stroke, StrokeKind::Inside);
                }
            }
            painter.vline(lane.x(state.position as f32), area.y_range(), playhead);

            // The item at a point and whether the point is on its right
            // edge. The item drawn last is on top.
            let at = |pos: Pos2| {
                let mut on_top = blocks.iter().rev();
                let block = on_top.find(|block| rect_of(block).contains(pos))?;
                let rect = rect_of(block);
                let edge = pos.x >= rect.right() - EDGE.min(rect.width() / 3.0);
                (pos.x >= lane.origin).then_some((block, edge))
            };
            let row_at = |pos: Pos2| ((pos.y - area.top()) / row).floor() as i32;
            let modifiers = ui.input(|input| input.modifiers);
            // With the item under it selected, a press acts on the whole
            // selection, and otherwise on that item alone.
            let targets = |block: &Block| {
                if block.selected {
                    selected()
                } else {
                    vec![block.id]
                }
            };

            if let Some((block, edge)) = response.hover_pos().and_then(at) {
                let (bar, beat, tick) = grid.bar_beat_tick(block.start);
                hover = Some(format!("{} at {bar}:{beat:02}:{tick:03}", block.name));
                if edge {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                }
            }

            let pressed = ui.input(|input| input.pointer.press_origin());
            if response.drag_started_by(PointerButton::Primary)
                && let Some(press) = pressed
                && let Some((block, edge)) = at(press)
            {
                let mut ids = targets(block);
                if !block.selected {
                    // The drag takes the item along, so it is selected
                    // first, and Shift takes the selection along too.
                    let add = modifiers.shift;
                    out.push(edit(Edit::SelectItems {
                        ids: ids.clone(),
                        add,
                    }));
                    if add {
                        ids.splice(0..0, selected());
                    }
                }
                let drag_id = next_drag_id(ui.ctx());
                if modifiers.command && !edge {
                    out.push(Command::Edit {
                        edit: Edit::DuplicateItems { ids: ids.clone() },
                        drag_id: Some(drag_id),
                    });
                    // The drag goes on with the copies, which are at the
                    // end of the list.
                    ids = (item_count..item_count + ids.len()).collect();
                }
                let mut drag = Drag {
                    id: drag_id,
                    resizes: edge,
                    ids,
                    start: block.start,
                    end: block.end,
                    tick: lane.tick(press.x),
                    row: row_at(press),
                    scale,
                    height: row,
                    sent: Edit::DeleteItems { ids: Vec::new() },
                };
                drag.sent = drag.edit(drag.tick, drag.row, 1);
                memory.drag = Some(drag);
            }
            let held = response.dragged_by(PointerButton::Primary);
            if let Some(drag) = &mut memory.drag {
                let dropped = response.drag_stopped_by(PointerButton::Primary);
                if let Some(pos) = response.interact_pointer_pos().filter(|_| held || dropped) {
                    let step = if modifiers.alt {
                        1
                    } else {
                        snap_step(grid, drag.scale)
                    };
                    let edit = drag.edit(lane.tick(pos.x), row_at(pos), step);
                    if edit != drag.sent {
                        out.push(Command::Edit {
                            edit: edit.clone(),
                            drag_id: Some(drag.id),
                        });
                        drag.sent = edit;
                    }
                }
                if drag.resizes {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                }
            }
            if !held {
                memory.drag = None;
            }

            let pointer = response.interact_pointer_pos();
            if response.clicked()
                && let Some(pos) = pointer
            {
                match at(pos) {
                    Some((block, _)) => out.push(edit(Edit::SelectItems {
                        ids: vec![block.id],
                        add: modifiers.shift,
                    })),
                    // A click beside the items lets go of them.
                    None if !modifiers.shift && !selected().is_empty() => {
                        out.push(edit(Edit::SelectItems {
                            ids: Vec::new(),
                            add: false,
                        }));
                    }
                    None => {}
                }
            }
            if response.secondary_clicked()
                && let Some((block, _)) = pointer.and_then(at)
            {
                out.push(edit(Edit::SetMuted {
                    ids: targets(block),
                    muted: !block.muted,
                }));
            }
        });
    set_hovered(ui.ctx(), hover);

    // Ctrl+wheel zooms around the pointer and Shift+wheel scrolls. The
    // scroll area above took the wheel on its own.
    let wide = ticks + SPARE_BARS * grid.bar;
    if ui.rect_contains_pointer(whole) {
        let (zoom, scroll) = ui.input_mut(|input| {
            let scroll = std::mem::take(&mut input.smooth_scroll_delta.x);
            (input.zoom_delta(), scroll)
        });
        let pointer = ui.input(|input| input.pointer.hover_pos());
        if zoom != 1.0
            && memory.drag.is_none()
            && let Some(pointer) = pointer
        {
            let lane = lane(whole.left());
            let under = lane.tick(pointer.x);
            let most = (BEAT_WIDTH / (grid.beat as f32 * fit)).max(1.0);
            memory.zoom = (memory.zoom * zoom).clamp(ZOOM_OUT, most);
            memory.left = under - (pointer.x - lane.origin) / (fit * memory.zoom);
        }
        memory.left -= scroll / scale;
    }
    let shown = (width - NAMES).max(1.0) / (fit * memory.zoom);
    memory.left = memory.left.clamp(0.0, (wide as f32 - shown).max(0.0));
    ui.data_mut(|data| data.insert_temp(memory_id, memory));
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::Arrangement;

    fn project() -> Project {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 0,
            ppq: 96,
        });
        project.writer_version = Some("24.1.0.1".to_owned());
        project
    }

    fn arrangement(index: u16, items: Vec<PlaylistItem>) -> Arrangement {
        Arrangement {
            index,
            items,
            ..Arrangement::default()
        }
    }

    #[test]
    fn items_become_blocks_on_their_tracks() {
        let mut project = project();
        let mut second = PlaylistItem::new(ItemSource::Channel(7), 384, 96);
        second.track_word = 497;
        second.set_muted(true);
        second.flags |= PlaylistItem::SELECTED;
        let mut deleted = PlaylistItem::new(ItemSource::Pattern(1), 0, 96);
        deleted.source_id = u32::MAX;
        let first = PlaylistItem::new(ItemSource::Pattern(2), 0, 768);
        project
            .arrangements
            .push(arrangement(0, vec![first, deleted, second]));

        let blocks = blocks(&project);
        assert_eq!(blocks.len(), 2);
        assert_eq!((blocks[0].row, blocks[0].start, blocks[0].end), (0, 0, 768));
        assert_eq!(blocks[0].name, "Pattern 2");
        assert!(!blocks[0].muted && !blocks[0].selected);
        assert_eq!(
            (blocks[1].row, blocks[1].start, blocks[1].end),
            (2, 384, 480)
        );
        assert_eq!(blocks[1].name, "Channel 7");
        assert!(blocks[1].muted && blocks[1].selected);
        // An item's ID is its place in the list, deleted records included.
        assert_eq!((blocks[0].id, blocks[1].id), (0, 2));
    }

    #[test]
    fn a_colour_has_red_in_its_low_byte() {
        assert_eq!(rgb(0x0056_5148), Color32::from_rgb(0x48, 0x51, 0x56));
    }

    #[test]
    fn ruler_labels_keep_their_distance() {
        assert_eq!(bar_step(60.0), 1);
        assert_eq!(bar_step(30.0), 2);
        assert_eq!(bar_step(5.0), 16);
        assert_eq!(bar_step(0.0), 1 << 20);
    }

    #[test]
    fn a_drag_snaps_to_beats_and_to_less_when_zoomed_in() {
        let grid = Grid { beat: 96, bar: 384 };
        // A beat of 24 points has no room for two steps of 16.
        assert_eq!(snap_step(grid, 0.25), 96);
        assert_eq!(snap_step(grid, 0.5), 48);
        assert_eq!(snap_step(grid, 2.0), 12);
        // 96 halves down to 3 and no further.
        assert_eq!(snap_step(grid, 100.0), 3);
        assert_eq!(snap(229.0, 96), 192);
        assert_eq!(snap(-50.0, 96), -96);
        assert_eq!(snap(229.4, 1), 229);
    }

    #[test]
    fn a_drag_says_how_far_it_is_from_where_it_began() {
        let mut drag = Drag {
            id: 1,
            resizes: false,
            ids: vec![4],
            start: 100,
            end: 484,
            tick: 150.0,
            row: 2,
            scale: 0.25,
            height: 22.0,
            sent: Edit::DeleteItems { ids: Vec::new() },
        };
        let moved = |delta_ticks, delta_tracks| Edit::MoveItems {
            ids: vec![4],
            delta_ticks,
            delta_tracks,
        };
        // Without a step the item goes where the pointer does. With one
        // its start lands on the grid, so an item that was off it moves by
        // an odd number.
        assert_eq!(drag.edit(150.0, 2, 1), moved(0, 0));
        assert_eq!(drag.edit(250.0, 0, 1), moved(100, -2));
        assert_eq!(drag.edit(250.0, 3, 96), moved(92, 1));
        // Past the start of the song, where the app stops the items.
        assert_eq!(drag.edit(0.0, 2, 96), moved(-196, 0));

        drag.resizes = true;
        let resized = |new_length| Edit::ResizeItems {
            ids: vec![4],
            new_length,
        };
        assert_eq!(drag.edit(150.0, 2, 1), resized(384));
        // The end snaps and the start stays.
        assert_eq!(drag.edit(250.0, 9, 96), resized(476));
        // An end dragged past the start leaves one step.
        assert_eq!(drag.edit(-900.0, 2, 96), resized(96));
    }

    #[test]
    fn an_automation_clip_s_line_follows_its_curve_from_the_item_s_trim() {
        let mut bent = Point::new(2.0, 1.0);
        bent.tension = 0.5;
        let rect = Rect::from_min_max(pos2(100.0, 10.0), pos2(200.0, 32.0));
        let all = egui::Rangef::EVERYTHING;
        // A beat is 20 points wide and the item starts a beat into the
        // clip, half way up a straight line that ends a beat later.
        let straight = Curve::new(&[Point::new(0.0, 0.0), Point::new(2.0, 1.0)]);
        let line = curve(&straight, 1.0, rect, 20.0, all);
        assert_eq!(line.len(), 51);
        assert_eq!(line[0], pos2(100.0, 21.0));
        assert_eq!(line[5], pos2(110.0, 16.0));
        // From its last point on the clip holds its value.
        assert_eq!(line[10], pos2(120.0, 11.0));
        assert_eq!(line[50], pos2(200.0, 11.0));

        // A tension bends the line away from the straight one.
        let bent = Curve::new(&[Point::new(0.0, 0.0), bent]);
        let line = curve(&bent, 1.0, rect, 20.0, all);
        assert!((line[5].y - 16.0).abs() > 1.0, "{:?}", line[5]);
        assert_eq!(line[10], pos2(120.0, 11.0));

        // Only what is in view gets points.
        let part = curve(&straight, 1.0, rect, 20.0, egui::Rangef::new(150.0, 160.0));
        assert_eq!(part.len(), 6);
        assert_eq!((part[0].x, part[5].x), (150.0, 160.0));
        assert!(curve(&straight, 1.0, rect, 20.0, egui::Rangef::new(300.0, 400.0)).is_empty());
    }
}
