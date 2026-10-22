//! The piano roll: the notes one channel plays in the pattern the top bar
//! has selected, a row per key and a rectangle per note, with a lane under
//! them for a value of each note.
//!
//! The channel is the one the channel rack has selected, and the list in
//! the toolbar selects another. The notes of the pattern's other channels
//! are drawn faint behind it.
//!
//! With the draw tool a click adds a note as long as the last one that was
//! drawn, resized or clicked, and a drag draws one as long as the drag.
//! With the select tool a drag selects the notes in the rectangle it spans,
//! as a drag with Ctrl does with the draw tool. With either, a click on a
//! note selects it, a drag on a note moves the selection along the pattern
//! and up and down the keys, and a drag by a note's right edge resizes it.
//! Shift adds to the selection. The delete tool removes the notes it is
//! dragged over, as the right button does with any tool, and the mute tool
//! mutes them or unmutes them. A drag snaps to the grid unless Alt is held.
//!
//! The paint tool lays a note at every step of the length the draw tool
//! would give a note, along the drag, each on the key the pointer was on
//! when it got there: one undo step for the whole drag, as FL's paint does.
//! On a note it moves and resizes as the draw tool does. The slice tool
//! cuts the notes in two where it is pressed, snapped to the grid, on
//! every key from the one it is pressed on to the one it is let go on: FL
//! cuts along the line a drag draws, and this keeps that line upright. A
//! click with it cuts the note under the pointer.
//!
//! The Tools menu does what FL's Tools menu does at its default settings,
//! to the selected notes, or to all of the channel's notes when none is
//! selected, as FL does. Each is one undo step:
//!
//! - Glue joins the notes on one key that touch or overlap.
//! - Quantize (Ctrl+Q) moves the starts and the lengths to the grid's
//!   step, and Quantize starts (Shift+Q) the starts alone: FL's quick
//!   quantize.
//! - Chop (Alt+U) cuts the notes into pieces a step of the grid long, from
//!   each note's start. FL's chopper has patterns too; this is its default.
//! - Arpeggiate (Alt+A) plays each chord upwards, a note every step of the
//!   grid, for as long as the chord lasts. FL's arpeggiator opens on that
//!   pattern; its range and gate are left at the chord and at a full step.
//! - Strum (Alt+S) starts each note of a chord a 64th note after the one
//!   under it and keeps its end. FL's strummer has a time knob and a
//!   tension; this is a fixed time with no tension.
//!
//! With the pointer over the piano roll, Delete removes the selection,
//! Alt+M mutes it or unmutes it, the arrows move it by a step of the grid
//! or by a key, Ctrl+Up and Ctrl+Down by an octave, Ctrl+A selects every
//! note of the channel and Ctrl+D none. Ctrl+wheel zooms around the
//! pointer, the wheel scrolls through the keys and Shift+wheel along the
//! pattern.
//!
//! The snap menu has FL's choices, which [`Snap`] explains, and every
//! drag, click, arrow key and tool goes by it. The scale menu takes a
//! root and a scale, and darkens the rows of the keys outside it and
//! lightens the root's, as FL's scale highlighting does. With a scale
//! chosen, Snap to scale in the Tools menu moves the notes outside it to
//! the nearest key in it, the lower one when two are as near.
//!
//! Nothing here changes the project. An edit goes out as a
//! [`Command::Edit`], and the next frame draws what the app made of it.

mod snap;

use crate::channel_rack::{name as channel_name, select_channel, selected_channel, takes_notes};
use crate::playlist::{Lane as Timeline, bar_step, rgb, snap};
use crate::widgets::next_drag_id;
use crate::{Command, Edit, Grid, NoteField, NoteId, UiState};
use egui::{
    Align2, Color32, CursorIcon, FontId, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use model::{Channel, Note, Pattern};
use snap::{Scale, Snap};
use std::ops::RangeInclusive;

/// The width of the keyboard down the left.
const KEYS: f32 = 48.0;
/// The height of a key's row.
const KEY: f32 = 12.0;
const RULER: f32 = 18.0;
/// The height of the lane under the notes.
const LANE: f32 = 56.0;
const TOP_KEY: i32 = Pattern::TOP_KEY as i32;
/// The key the view opens on when the channel has no notes: C5, which FL
/// labels as the middle C.
const MIDDLE_KEY: i32 = 60;
/// A press this near a note's right edge resizes it.
const EDGE: f32 = 5.0;
/// How wide a beat is to begin with, in points, and how wide the zoom
/// makes it.
const BEAT: f32 = 64.0;
const BEAT_WIDTH: RangeInclusive<f32> = 8.0..=512.0;
/// Empty bars past the end of the pattern that the view scrolls to.
const SPARE_BARS: u64 = 4;
/// The colour of the notes of a channel that has none.
const PLAIN: Color32 = Color32::from_rgb(92, 112, 128);
const PLAYHEAD: Color32 = Color32::from_rgb(120, 220, 110);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Draw,
    Paint,
    Delete,
    Mute,
    Slice,
    Select,
}

impl Tool {
    /// In the order of FL's toolbar.
    const ALL: [Tool; 6] = [
        Tool::Draw,
        Tool::Paint,
        Tool::Delete,
        Tool::Mute,
        Tool::Slice,
        Tool::Select,
    ];

    fn name(self) -> &'static str {
        match self {
            Tool::Draw => "Draw",
            Tool::Paint => "Paint",
            Tool::Select => "Select",
            Tool::Slice => "Slice",
            Tool::Delete => "Delete",
            Tool::Mute => "Mute",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tool::Draw => "Click to add a note, drag to draw one. Drag a note to move it",
            Tool::Paint => "Drag to lay a note at every step. Drag a note to move it",
            Tool::Select => "Drag to select notes. Drag a note to move the selection",
            Tool::Slice => "Press where to cut and drag over the keys to cut there",
            Tool::Delete => "Click or drag over notes to delete them",
            Tool::Mute => "Click or drag over notes to mute or unmute them",
        }
    }

    /// The tools that lay and move notes as the draw tool does.
    fn draws(self) -> bool {
        matches!(self, Tool::Draw | Tool::Paint)
    }
}

/// An entry of the Tools menu: an edit of the selection, or of all of the
/// channel's notes when none is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Glue,
    Quantize,
    QuantizeStarts,
    Chop,
    Arpeggiate,
    Strum,
    SnapToScale,
}

impl Action {
    const ALL: [Action; 7] = [
        Action::Glue,
        Action::Quantize,
        Action::QuantizeStarts,
        Action::Chop,
        Action::Arpeggiate,
        Action::Strum,
        Action::SnapToScale,
    ];

    fn name(self) -> &'static str {
        match self {
            Action::Glue => "Glue",
            Action::Quantize => "Quantize",
            Action::QuantizeStarts => "Quantize starts",
            Action::Chop => "Chop",
            Action::Arpeggiate => "Arpeggiate",
            Action::Strum => "Strum",
            Action::SnapToScale => "Snap to scale",
        }
    }

    /// FL's shortcut, when it has one. Ctrl+Q comes before Shift+Q in
    /// [`Action::ALL`], since a shortcut without Shift matches with it
    /// held.
    fn shortcut(self) -> Option<(Modifiers, Key)> {
        match self {
            Action::Glue | Action::SnapToScale => None,
            Action::Quantize => Some((Modifiers::COMMAND, Key::Q)),
            Action::QuantizeStarts => Some((Modifiers::SHIFT, Key::Q)),
            Action::Chop => Some((Modifiers::ALT, Key::U)),
            Action::Arpeggiate => Some((Modifiers::ALT, Key::A)),
            Action::Strum => Some((Modifiers::ALT, Key::S)),
        }
    }

    /// The edit, with `step` the snap's step in ticks, `ppq` the
    /// project's and `keys` the keys of the scale, as
    /// [`Edit::SnapNotesToScale`] has them.
    fn edit(self, ids: Vec<NoteId>, step: u64, ppq: u16, keys: u16) -> Edit {
        match self {
            Action::SnapToScale => Edit::SnapNotesToScale { ids, keys },
            Action::Glue => Edit::GlueNotes { ids },
            Action::Quantize | Action::QuantizeStarts => Edit::QuantizeNotes {
                ids,
                step,
                lengths: self == Action::Quantize,
            },
            Action::Chop => Edit::ChopNotes { ids, step },
            Action::Arpeggiate => Edit::ArpeggiateNotes { ids, step },
            // A 64th note.
            Action::Strum => Edit::StrumNotes {
                ids,
                delay: u64::from(ppq / 16).max(1),
            },
        }
    }
}

fn field_name(field: NoteField) -> &'static str {
    match field {
        NoteField::Velocity => "Velocity",
        NoteField::Pan => "Pan",
        NoteField::FinePitch => "Fine pitch",
        NoteField::Release => "Release",
        NoteField::ModX => "Mod X",
        NoteField::ModY => "Mod Y",
    }
}

/// What the piano roll keeps from one frame to the next.
#[derive(Clone)]
struct Memory {
    tool: Tool,
    /// The value the lane shows.
    lane: NoteField,
    snap: Snap,
    /// The root, 0 for C, and the scale the rows are highlighted for.
    scale: Option<(u8, Scale)>,
    /// Whether a note has the colour of its colour group, and not the
    /// channel's.
    colours: bool,
    /// How wide a beat is, in points.
    beat: f32,
    /// The tick at the left edge of the notes.
    left: f32,
    /// How far the keys are scrolled down from the highest, in points.
    top: f32,
    /// The pattern and the channel the keys were last scrolled to.
    shown: Option<(u16, u16)>,
    /// How long the next note is that a click adds, in ticks. A step of
    /// the step sequencer when `None`.
    length: Option<u64>,
    drag: Option<Drag>,
    /// A drag across the lane.
    paint: Option<Paint>,
    /// Where the piano roll was drawn and in which frame, for
    /// [`keys`], which runs before it.
    area: Option<(Rect, u64)>,
}

impl Default for Memory {
    fn default() -> Self {
        Memory {
            tool: Tool::Draw,
            lane: NoteField::Velocity,
            snap: Snap::Line,
            scale: None,
            colours: false,
            beat: BEAT,
            left: 0.0,
            top: 0.0,
            shown: None,
            length: None,
            drag: None,
            paint: None,
            area: None,
        }
    }
}

fn memory_id() -> egui::Id {
    egui::Id::new("fl-rs piano roll")
}

impl Memory {
    /// How many ticks the snap's step is at the zoom.
    fn step(&self, grid: Grid) -> u64 {
        self.snap.ticks(grid, self.beat / grid.beat as f32)
    }

    /// The keys of the scale, as [`Scale::keys`] has them. None without a
    /// scale.
    fn keys(&self) -> u16 {
        self.scale.map_or(0, |(root, scale)| scale.keys(root))
    }
}

/// A drag across the notes that is under way.
#[derive(Clone)]
struct Drag {
    /// What the edits of this drag carry as their `drag_id`.
    id: u64,
    gesture: Gesture,
    /// The edit that went out last, and to begin with the one that changes
    /// nothing, when the gesture has one.
    sent: Option<Edit>,
    /// The notes the drag has deleted or muted so far.
    done: Vec<NoteId>,
}

/// What a drag does.
#[derive(Clone, Debug, PartialEq)]
enum Gesture {
    /// Move the notes. `start` is where the note under the pointer began,
    /// and `tick` and `key` are what the pointer went down on.
    Move {
        ids: Vec<NoteId>,
        start: i64,
        tick: f32,
        key: i32,
    },
    /// Move the notes' ends. `start`, `end` and `length` are those of the
    /// note under the pointer when the drag began. For a step, which has
    /// no length, `end` is where it is drawn to end.
    Resize {
        ids: Vec<NoteId>,
        start: i64,
        end: i64,
        length: i64,
        tick: f32,
    },
    /// Draw a note from `position` to where the pointer is.
    Draw {
        channel: u16,
        position: u64,
        key: u8,
    },
    /// Lay notes `length` long along the drag, at `origin` and every
    /// `length` either side of it. `notes` are the ones laid so far.
    Paint {
        channel: u16,
        length: u64,
        origin: u64,
        notes: Vec<(u64, u8)>,
    },
    /// Cut the notes at `at`, on the keys from `key` to the pointer's.
    Slice { at: u64, key: i32 },
    /// Select the notes in the rectangle from where the pointer went down:
    /// that tick, and that many points under the top of the highest key.
    Band { tick: f32, y: f32 },
    /// Delete the notes the pointer passes.
    Erase,
    /// Mute the notes the pointer passes, or unmute them.
    Mute { muted: bool },
}

impl Gesture {
    /// The edit that takes the notes to where the pointer is, snapped to
    /// `step` ticks, for the gestures that say one thing over and over.
    fn edit(&self, tick: f32, key: i32, step: u64) -> Option<Edit> {
        match self {
            Gesture::Move {
                ids,
                start,
                tick: from,
                key: from_key,
            } => Some(Edit::MoveNotes {
                ids: ids.clone(),
                delta_ticks: snap(*start as f32 + tick - from, step) - start,
                delta_keys: key - from_key,
            }),
            Gesture::Resize {
                ids,
                start,
                end,
                length,
                tick: from,
            } => {
                let new = snap(*end as f32 + tick - from, step) - start;
                // An end dragged to the start or past it leaves one step.
                let new = if new < 1 { step as i64 } else { new };
                Some(Edit::ResizeNotes {
                    ids: ids.clone(),
                    delta_ticks: new - length,
                })
            }
            Gesture::Draw {
                channel,
                position,
                key,
            } => {
                let length = snap(tick, step) - *position as i64;
                Some(Edit::AddNote {
                    channel: *channel,
                    position: *position,
                    length: length.max(step as i64) as u64,
                    key: *key,
                })
            }
            Gesture::Paint { .. }
            | Gesture::Slice { .. }
            | Gesture::Band { .. }
            | Gesture::Erase
            | Gesture::Mute { .. } => None,
        }
    }

    /// For a paint, lay the notes the pointer has come to at `tick` on
    /// `key`: every slot from the last one laid to the one under the
    /// pointer that has none yet. The edit that says every note laid, when
    /// this laid one.
    fn paint(&mut self, tick: f32, key: i32) -> Option<Edit> {
        let Gesture::Paint {
            channel,
            length,
            origin,
            notes,
        } = self
        else {
            return None;
        };
        let key = u8::try_from(key).ok()?.min(TOP_KEY as u8);
        let (origin, length) = (*origin as i64, (*length).max(1) as i64);
        let slot = |tick: i64| origin + (tick - origin).div_euclid(length) * length;
        let here = slot(tick.floor() as i64).max(0);
        let last = notes.last().map_or(here, |(position, _)| *position as i64);
        let (from, to) = (last.min(here), last.max(here));
        let before = notes.len();
        for position in (from..=to).step_by(length as usize) {
            let position = position as u64;
            if !notes.iter().any(|(laid, _)| *laid == position) {
                notes.push((position, key));
            }
        }
        (notes.len() > before).then(|| Edit::AddNotes {
            channel: *channel,
            length: length as u64,
            notes: notes.clone(),
        })
    }
}

/// A drag across the lane.
#[derive(Clone, Copy)]
struct Paint {
    id: u64,
    /// Where the notes start that the drag set last, and to what.
    sent: Option<(i64, u8)>,
}

/// Where the ticks and the keys are on the screen.
#[derive(Clone, Copy)]
struct View {
    time: Timeline,
    /// The y of the top of the highest key's row, which is above the notes
    /// when the keys are scrolled.
    top: f32,
    /// How long a note without a length is drawn, in ticks: a step of the
    /// step sequencer.
    step: i64,
}

impl View {
    /// The key whose row a y is in. Above the highest key and below the
    /// lowest it goes on counting.
    fn key(self, y: f32) -> i32 {
        TOP_KEY - ((y - self.top) / KEY).floor() as i32
    }

    /// The top of a key's row.
    fn y(self, key: i32) -> f32 {
        self.top + (TOP_KEY - key) as f32 * KEY
    }

    /// The tick a note is drawn to end at.
    fn end(self, note: &Note) -> i64 {
        let length = if note.length > 0 {
            i64::from(note.length)
        } else {
            self.step
        };
        i64::from(note.position) + length
    }

    fn rect(self, note: &Note) -> Rect {
        let left = self.time.x(note.position as f32);
        // A point short of its end, so that two notes in a row show as
        // two, and never thinner than two points.
        let right = (self.time.x(self.end(note) as f32) - 1.0).max(left + 2.0);
        let top = self.y(i32::from(note.key));
        Rect::from_min_max(pos2(left, top + 1.0), pos2(right, top + KEY))
    }
}

/// The start of the grid's step that a tick is in.
fn floor(tick: f32, step: u64) -> u64 {
    (tick.max(0.0) / step as f32).floor() as u64 * step
}

fn is_black(key: i32) -> bool {
    matches!(key.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

/// A key as FL names it: key 60 is C5.
fn key_name(key: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[key.rem_euclid(12) as usize],
        key.div_euclid(12)
    )
}

/// The colour of a note's colour group, 0 to 15. FL has a palette of its
/// own for these, which this is not: sixteen hues, with green for group 0.
fn group_colour(group: u8) -> Color32 {
    let hue = (1.0 / 3.0 + f32::from(group & 0xf) / 16.0).fract();
    egui::ecolor::Hsva::new(hue, 0.55, 0.85, 1.0).into()
}

fn edit(edit: Edit, drag_id: Option<u64>) -> Command {
    Command::Edit { edit, drag_id }
}

/// The notes the channel has in the pattern, with the place of each.
fn own(pattern: Option<&Pattern>, channel: u16) -> Vec<(NoteId, &Note)> {
    let notes = pattern.map_or(&[][..], |pattern| &pattern.notes);
    let placed = notes.iter().enumerate();
    placed
        .filter(|(_, note)| !note.is_deleted() && note.channel == channel)
        .collect()
}

fn selected(own: &[(NoteId, &Note)]) -> Vec<NoteId> {
    let selected = own.iter().filter(|(_, note)| note.selected());
    selected.map(|(id, _)| *id).collect()
}

/// What an entry of the Tools menu acts on: the selection, or every note
/// of the channel when none is selected.
fn targets(own: &[(NoteId, &Note)]) -> Vec<NoteId> {
    let chosen = selected(own);
    if chosen.is_empty() {
        own.iter().map(|(id, _)| *id).collect()
    } else {
        chosen
    }
}

/// The keys the piano roll takes when the pointer is over it. This runs
/// before the windows are drawn, and what it takes no other window sees.
pub(crate) fn keys(ui: &Ui, state: &UiState, out: &mut Vec<Command>) {
    let memory: Memory = ui
        .data(|data| data.get_temp(memory_id()))
        .unwrap_or_default();
    let Some((area, frame)) = memory.area else {
        return;
    };
    let pointer = ui.input(|input| input.pointer.hover_pos());
    let over = pointer.is_some_and(|pos| area.contains(pos));
    // A piano roll that is no longer drawn has no pointer over it.
    let drawn = frame + 1 >= ui.ctx().cumulative_frame_nr();
    if !over || !drawn || ui.memory(|egui| egui.focused().is_some()) {
        return;
    }
    let project = &*state.project;
    let Some(channel) = selected_channel(ui.ctx(), project) else {
        return;
    };
    let own = own(project.pattern(state.pattern), channel.index);
    let pressed = |modifiers, key| ui.input_mut(|input| input.consume_key(modifiers, key));
    let ctrl = Modifiers::COMMAND;

    if pressed(ctrl, Key::A) && !own.is_empty() {
        let ids = own.iter().map(|(id, _)| *id).collect();
        out.push(edit(Edit::SelectNotes { ids, add: false }, None));
    }
    let step = memory.step(Grid::of(project));
    if !own.is_empty() {
        for action in Action::ALL {
            if let Some((modifiers, key)) = action.shortcut()
                && pressed(modifiers, key)
            {
                let ppq = project.header.ppq;
                let tool = action.edit(targets(&own), step, ppq, memory.keys());
                out.push(edit(tool, None));
            }
        }
    }
    // Without a selection the other keys are another window's.
    let ids = selected(&own);
    if ids.is_empty() {
        return;
    }
    if pressed(ctrl, Key::D) {
        let ids = Vec::new();
        out.push(edit(Edit::SelectNotes { ids, add: false }, None));
    }
    if pressed(Modifiers::NONE, Key::Delete) {
        out.push(edit(Edit::DeleteNotes { ids: ids.clone() }, None));
    }
    if pressed(Modifiers::ALT, Key::M) {
        let chosen = || own.iter().filter(|(_, note)| note.selected());
        let muted = !chosen().all(|(_, note)| note.muted());
        let ids = ids.clone();
        out.push(edit(Edit::SetNotesMuted { ids, muted }, None));
    }
    let step = step as i64;
    // Ctrl does not match a shortcut without it, so the order is free.
    for (modifiers, key, delta_ticks, delta_keys) in [
        (ctrl, Key::ArrowUp, 0, 12),
        (ctrl, Key::ArrowDown, 0, -12),
        (Modifiers::NONE, Key::ArrowUp, 0, 1),
        (Modifiers::NONE, Key::ArrowDown, 0, -1),
        (Modifiers::NONE, Key::ArrowLeft, -step, 0),
        (Modifiers::NONE, Key::ArrowRight, step, 0),
    ] {
        if pressed(modifiers, key) {
            let moved = Edit::MoveNotes {
                ids: ids.clone(),
                delta_ticks,
                delta_keys,
            };
            out.push(edit(moved, None));
        }
    }
}

/// The row above the notes: the channel, the tools, the Tools menu, what
/// the lane shows and how the notes are coloured.
fn toolbar(
    ui: &mut Ui,
    state: &UiState,
    channel: Option<&Channel>,
    memory: &mut Memory,
    out: &mut Vec<Command>,
) {
    let project = &*state.project;
    ui.horizontal(|ui| {
        let shown = channel.map_or_else(|| "No channel".to_owned(), channel_name);
        egui::ComboBox::from_id_salt("fl-rs piano roll channel")
            .width(120.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for other in project.channels.iter().filter(|c| takes_notes(c)) {
                    let chosen = channel.is_some_and(|channel| channel.index == other.index);
                    let entry = ui.selectable_label(chosen, channel_name(other));
                    if entry.clicked() && !chosen {
                        select_channel(ui.ctx(), other.index);
                    }
                }
            })
            .response
            .on_hover_text("The channel whose notes the piano roll shows");
        ui.separator();
        for tool in Tool::ALL {
            let button = ui.selectable_label(memory.tool == tool, tool.name());
            if button.on_hover_text(tool.hint()).clicked() {
                memory.tool = tool;
            }
        }
        let own = channel.map_or_else(Vec::new, |channel| {
            own(project.pattern(state.pattern), channel.index)
        });
        ui.add_enabled_ui(!own.is_empty(), |ui| {
            ui.menu_button("Tools", |ui| {
                let step = memory.step(Grid::of(project));
                let keys = memory.keys();
                for action in Action::ALL {
                    let usable = action != Action::SnapToScale || keys != 0;
                    if ui
                        .add_enabled(usable, egui::Button::new(action.name()))
                        .clicked()
                    {
                        let ppq = project.header.ppq;
                        let tool = action.edit(targets(&own), step, ppq, keys);
                        out.push(edit(tool, None));
                    }
                }
            })
            .response
            .on_hover_text("Edit the selected notes, or all of them when none is selected");
        });
        ui.separator();
        egui::ComboBox::from_id_salt("fl-rs piano roll snap")
            .width(72.0)
            .height(320.0)
            .selected_text(memory.snap.name())
            .show_ui(ui, |ui| {
                for snap in Snap::ALL {
                    ui.selectable_value(&mut memory.snap, snap, snap.name());
                }
            })
            .response
            .on_hover_text("What a note snaps to");
        let scale = match memory.scale {
            Some((root, scale)) => format!("{} {}", ROOTS[usize::from(root)], scale.name()),
            None => "Scale".to_owned(),
        };
        ui.menu_button(scale, |ui| scale_menu(ui, memory))
            .response
            .on_hover_text("The scale whose keys the rows show");
        ui.separator();
        egui::ComboBox::from_id_salt("fl-rs piano roll lane")
            .width(80.0)
            .selected_text(field_name(memory.lane))
            .show_ui(ui, |ui| {
                for field in NoteField::ALL {
                    ui.selectable_value(&mut memory.lane, field, field_name(field));
                }
            })
            .response
            .on_hover_text("The value of each note that the lane under the notes shows");
        let colours = ui.selectable_label(memory.colours, "Colours");
        if colours
            .on_hover_text("Colour each note by its colour group, and not by its channel")
            .clicked()
        {
            memory.colours = !memory.colours;
        }
    });
}

/// The names of the roots, from C.
const ROOTS: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The scale menu: no scale, the roots and the scales.
fn scale_menu(ui: &mut Ui, memory: &mut Memory) {
    if ui
        .selectable_label(memory.scale.is_none(), "No scale")
        .clicked()
    {
        memory.scale = None;
    }
    let (root, scale) = memory.scale.unwrap_or((0, Scale::Major));
    ui.separator();
    egui::Grid::new("fl-rs piano roll roots").show(ui, |ui| {
        for (at, name) in ROOTS.iter().enumerate() {
            let chosen = memory.scale.is_some() && usize::from(root) == at;
            if ui.selectable_label(chosen, *name).clicked() {
                memory.scale = Some((at as u8, scale));
            }
            if at % 6 == 5 {
                ui.end_row();
            }
        }
    });
    ui.separator();
    for other in Scale::ALL {
        let chosen = memory.scale.is_some() && other == scale;
        if ui.selectable_label(chosen, other.name()).clicked() {
            memory.scale = Some((root, other));
        }
    }
}

pub(crate) fn show(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let project = &*state.project;
    let memory_id = memory_id();
    let mut memory: Memory = ui.data(|data| data.get_temp(memory_id)).unwrap_or_default();
    let channel = selected_channel(ui.ctx(), project);
    toolbar(ui, state, channel, &mut memory, out);
    let Some(channel) = channel.filter(|channel| takes_notes(channel)) else {
        let why = if project.channels.is_empty() {
            "The project has no channels"
        } else {
            "This channel plays no notes"
        };
        ui.centered_and_justified(|ui| ui.weak(why));
        (memory.drag, memory.paint, memory.area) = (None, None, None);
        ui.data_mut(|data| data.insert_temp(memory_id, memory));
        return;
    };

    let pattern = project.pattern(state.pattern);
    let notes = pattern.map_or(&[][..], |pattern| &pattern.notes);
    let own = own(pattern, channel.index);
    let grid = Grid::of(project);
    // The pattern to the end of its last bar, as the channel rack has it.
    let ends = notes.iter().filter(|note| !note.is_deleted()).map(|note| {
        let start = u64::try_from(note.position).unwrap_or(0);
        start + u64::try_from(note.length).unwrap_or(0).max(1)
    });
    let length = pattern.and_then(|pattern| pattern.length).unwrap_or(0);
    let bars = ends.max().unwrap_or(0).max(u64::from(length));
    let bars = bars.div_ceil(grid.bar).max(1);

    // The ruler on top, the lane at the bottom, and between them the keys
    // and the notes.
    let whole = ui.available_rect_before_wrap();
    ui.advance_cursor_after_rect(whole);
    let ruler = whole.with_max_y((whole.top() + RULER).min(whole.bottom()));
    let lane_top = (whole.bottom() - LANE).max(ruler.bottom());
    let left = (whole.left() + KEYS).min(whole.right());
    let area = Rect::from_min_max(pos2(left, ruler.bottom()), pos2(whole.right(), lane_top));
    let keyboard = Rect::from_min_max(pos2(whole.left(), ruler.bottom()), pos2(left, lane_top));
    let lane = Rect::from_min_max(pos2(left, lane_top), whole.max);
    let gutter = Rect::from_min_max(pos2(whole.left(), lane_top), pos2(left, whole.bottom()));

    // A channel or a pattern that was not shown before opens on the middle
    // of its notes.
    let showing = (state.pattern, channel.index);
    if memory.shown != Some(showing) {
        memory.shown = Some(showing);
        let keys = own.iter().map(|(_, note)| i32::from(note.key));
        let middle = match (keys.clone().min(), keys.max()) {
            (Some(low), Some(high)) => (low + high) / 2,
            _ => MIDDLE_KEY,
        };
        let row = (TOP_KEY - middle) as f32 * KEY;
        memory.top = (row + KEY / 2.0 - area.height() / 2.0).round();
    }
    let tall = (TOP_KEY + 1) as f32 * KEY;
    memory.top = memory.top.clamp(0.0, (tall - area.height()).max(0.0));

    let scale = memory.beat / grid.beat as f32;
    let time = Timeline {
        origin: area.left(),
        left: memory.left,
        scale,
    };
    let roll = Roll {
        channel,
        notes,
        own,
        view: View {
            time,
            top: area.top() - memory.top,
            step: i64::from((project.header.ppq / 4).max(1)),
        },
        grid,
        // In song mode the clock is the song's, and says nothing about
        // where in the pattern it is.
        playhead: (!state.song_mode).then(|| time.x(state.position as f32)),
        colours: memory.colours,
        step: memory.step(grid),
        cell: memory.snap == Snap::Cell,
        scale: memory.scale.map(|(root, scale)| (root, scale.keys(root))),
    };
    roll.backdrop(ui, ruler, area, keyboard);
    roll.notes(ui, area, &mut memory, out);
    roll.lane(ui, gutter, lane, &mut memory, out);

    // Ctrl+wheel zooms around the pointer, the wheel scrolls through the
    // keys and Shift+wheel along the pattern.
    if ui.rect_contains_pointer(whole) {
        let (zoom, scroll) = ui.input_mut(|input| {
            let scroll = std::mem::take(&mut input.smooth_scroll_delta);
            (input.zoom_delta(), scroll)
        });
        let pointer = ui.input(|input| input.pointer.hover_pos());
        if zoom != 1.0
            && memory.drag.is_none()
            && let Some(pointer) = pointer
        {
            let under = time.tick(pointer.x);
            memory.beat = (memory.beat * zoom).clamp(*BEAT_WIDTH.start(), *BEAT_WIDTH.end());
            let scale = memory.beat / grid.beat as f32;
            memory.left = under - (pointer.x - time.origin) / scale;
        }
        memory.left -= scroll.x / scale;
        memory.top -= scroll.y;
    }
    let wide = (bars + SPARE_BARS) * grid.bar;
    let shown = area.width().max(1.0) / (memory.beat / grid.beat as f32);
    memory.left = memory.left.clamp(0.0, (wide as f32 - shown).max(0.0));
    memory.top = memory.top.clamp(0.0, (tall - area.height()).max(0.0));
    memory.area = Some((whole, ui.ctx().cumulative_frame_nr()));
    ui.data_mut(|data| data.insert_temp(memory_id, memory));
}

/// What the piano roll is drawn from.
struct Roll<'a> {
    channel: &'a Channel,
    /// Every note of the pattern.
    notes: &'a [Note],
    /// The channel's own notes, with the place of each.
    own: Vec<(NoteId, &'a Note)>,
    view: View,
    grid: Grid,
    /// The x of the playhead, when a pattern plays.
    playhead: Option<f32>,
    /// Whether a note has the colour of its colour group, and not the
    /// channel's.
    colours: bool,
    /// The snap's step in ticks.
    step: u64,
    /// Whether a move goes in whole steps from where the notes are, and
    /// not to the grid.
    cell: bool,
    /// The root of the scale and its keys, as [`Scale::keys`] has them.
    scale: Option<(u8, u16)>,
}

impl Roll<'_> {
    /// A note's colour, which is a shade of itself for a muted note.
    fn colour(&self, note: &Note) -> Color32 {
        let colour = if self.colours {
            group_colour(note.color())
        } else {
            self.channel.color.map_or(PLAIN, rgb)
        };
        if note.muted() {
            colour.gamma_multiply(0.3)
        } else {
            colour
        }
    }

    fn playhead(&self, painter: &egui::Painter, rect: Rect) {
        if let Some(x) = self.playhead {
            painter.vline(x, rect.y_range(), Stroke::new(1.5, PLAYHEAD));
        }
    }

    /// The note at a point and whether the point is on its right edge. The
    /// note drawn last is on top.
    fn at(&self, pos: Pos2) -> Option<(NoteId, &Note, bool)> {
        let mut on_top = self.own.iter().rev();
        let (id, note) = on_top.find(|(_, note)| self.view.rect(note).contains(pos))?;
        let rect = self.view.rect(note);
        let edge = pos.x >= rect.right() - EDGE.min(rect.width() / 3.0);
        Some((*id, *note, edge))
    }

    /// What is behind the channel's notes: the ruler with a number on
    /// every few bars, the keyboard with the name of every C, a row for
    /// every key, a line for every beat that has the room, and the notes
    /// of the other channels, faint.
    fn backdrop(&self, ui: &Ui, ruler: Rect, area: Rect, keyboard: Rect) {
        let (view, time, grid) = (self.view, self.view.time, self.grid);
        let visuals = ui.visuals();
        let font = FontId::proportional(10.0);
        let line = visuals.widgets.noninteractive.bg_stroke;
        let faint = Stroke::new(1.0, line.color.gamma_multiply(0.4));

        let painter = ui.painter_at(ruler);
        painter.rect_filled(ruler, 0.0, visuals.faint_bg_color);
        let painter = ui.painter_at(ruler.with_min_x(area.left()));
        let every = bar_step(grid.bar as f32 * time.scale);
        for (bar, x) in time.bars(grid, every, ruler.right()) {
            painter.vline(x, ruler.y_range(), line);
            let at = pos2(x + 3.0, ruler.center().y);
            let ink = visuals.weak_text_color();
            painter.text(at, Align2::LEFT_CENTER, bar + 1, font.clone(), ink);
        }
        self.playhead(&painter, ruler);

        // The rows are lighter for the white keys, with a line under
        // every C.
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, visuals.extreme_bg_color);
        let white = visuals.text_color().gamma_multiply(0.05);
        let lowest = view.key(area.bottom() - 1.0).max(0);
        let highest = view.key(area.top()).min(TOP_KEY);
        let keys = ui.painter_at(keyboard);
        for key in lowest..=highest {
            let top = view.y(key);
            let (fill, ink) = if is_black(key) {
                (Color32::from_gray(40), Color32::from_gray(200))
            } else {
                let row = Rect::from_min_max(pos2(area.left(), top), pos2(area.right(), top + KEY));
                painter.rect_filled(row, 0.0, white);
                (Color32::from_gray(225), Color32::from_gray(40))
            };
            let plate = Rect::from_min_max(
                pos2(keyboard.left(), top + 0.5),
                pos2(keyboard.right(), top + KEY - 0.5),
            );
            keys.rect_filled(plate, 0.0, fill);
            if let Some((root, scale)) = self.scale {
                let row = Rect::from_min_max(pos2(area.left(), top), pos2(area.right(), top + KEY));
                let class = key.rem_euclid(12);
                if scale & (1 << class) == 0 {
                    painter.rect_filled(row, 0.0, Color32::from_black_alpha(110));
                } else if class == i32::from(root) {
                    let tint = visuals.selection.bg_fill.gamma_multiply(0.18);
                    painter.rect_filled(row, 0.0, tint);
                }
            }
            if key % 12 == 0 {
                painter.hline(area.x_range(), top + KEY, faint);
                let at = pos2(plate.right() - 4.0, plate.center().y);
                keys.text(at, Align2::RIGHT_CENTER, key_name(key), font.clone(), ink);
            }
        }
        let unit = if grid.beat as f32 * time.scale >= 12.0 {
            grid.beat
        } else {
            grid.bar
        };
        let line_at = |x: f32| time.tick(x).max(0.0) as u64 / unit;
        for tick in (line_at(area.left())..=line_at(area.right())).map(|n| n * unit) {
            let stroke = if tick.is_multiple_of(grid.bar) {
                line
            } else {
                faint
            };
            painter.vline(time.x(tick as f32), area.y_range(), stroke);
        }

        let ghost = visuals.text_color().gamma_multiply(0.2);
        let placed = self.notes.iter().filter(|note| !note.is_deleted());
        for note in placed.filter(|note| note.channel != self.channel.index) {
            let rect = view.rect(note);
            if area.intersects(rect) {
                painter.rect_filled(rect, 1.0, ghost);
            }
        }
    }

    /// The channel's notes, and the clicks and drags on them.
    fn notes(&self, ui: &Ui, area: Rect, memory: &mut Memory, out: &mut Vec<Command>) {
        let (view, time, own) = (self.view, self.view.time, &self.own);
        let channel = self.channel.index;
        let strong = ui.visuals().strong_text_color();
        let painter = ui.painter_at(area);
        for (_, note) in own {
            let rect = view.rect(note);
            if !area.intersects(rect) {
                continue;
            }
            painter.rect_filled(rect, 1.0, self.colour(note));
            if note.selected() {
                let stroke = Stroke::new(1.5, strong);
                painter.rect_stroke(rect, 1.0, stroke, StrokeKind::Inside);
            }
        }
        self.playhead(&painter, area);

        let id = memory_id().with("notes");
        let response = ui.interact(area, id, Sense::click_and_drag());
        let label = format!("{} notes", channel_name(self.channel));
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, &label));
        let modifiers = ui.input(|input| input.modifiers);
        let step = if modifiers.alt { 1 } else { self.step };
        let (tool, shift) = (memory.tool, modifiers.shift);

        if (tool.draws() || tool == Tool::Select)
            && memory.drag.is_none()
            && let Some((_, _, true)) = response.hover_pos().and_then(|pos| self.at(pos))
        {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }

        let pressed = ui.input(|input| input.pointer.press_origin());
        let primary = response.drag_started_by(PointerButton::Primary);
        let started = primary || response.drag_started_by(PointerButton::Secondary);
        if started && let Some(press) = pressed {
            let hit = self.at(press);
            let (tick, key) = (time.tick(press.x), view.key(press.y));
            let mut sent = None;
            let gesture = if !primary || tool == Tool::Delete {
                Some(Gesture::Erase)
            } else if tool == Tool::Mute {
                hit.map(|(_, note, _)| Gesture::Mute {
                    muted: !note.muted(),
                })
            } else if tool == Tool::Slice {
                let at = snap(tick, step).max(0) as u64;
                Some(Gesture::Slice { at, key })
            } else if modifiers.command || (tool == Tool::Select && hit.is_none()) {
                let y = press.y - view.top;
                Some(Gesture::Band { tick, y })
            } else if let Some((id, note, edge)) = hit {
                // With the note under it selected, a drag takes the whole
                // selection, and otherwise it selects that note first.
                // Shift takes the selection along too.
                let mut ids = vec![id];
                if note.selected() {
                    ids = selected(own);
                } else {
                    let ids = ids.clone();
                    out.push(edit(Edit::SelectNotes { ids, add: shift }, None));
                }
                if !note.selected() && shift {
                    ids.splice(0..0, selected(own));
                }
                let start = i64::from(note.position);
                // With the cell snap a move goes from the start of the
                // note's cell, so the note keeps its place in its cell.
                let from = if self.cell {
                    floor(start as f32, step) as i64
                } else {
                    start
                };
                Some(if edge {
                    sent = Some(Edit::ResizeNotes {
                        ids: ids.clone(),
                        delta_ticks: 0,
                    });
                    Gesture::Resize {
                        ids,
                        start,
                        end: view.end(note),
                        length: i64::from(note.length),
                        tick,
                    }
                } else {
                    sent = Some(Edit::MoveNotes {
                        ids: ids.clone(),
                        delta_ticks: 0,
                        delta_keys: 0,
                    });
                    Gesture::Move {
                        ids,
                        start: from,
                        tick,
                        key,
                    }
                })
            } else if tool == Tool::Paint {
                Some(Gesture::Paint {
                    channel,
                    length: memory.length.unwrap_or(view.step as u64).max(1),
                    origin: floor(tick, step),
                    notes: Vec::new(),
                })
            } else {
                u8::try_from(key).ok().map(|key| Gesture::Draw {
                    channel,
                    position: floor(tick, step),
                    key: key.min(TOP_KEY as u8),
                })
            };
            memory.drag = gesture.map(|gesture| Drag {
                id: next_drag_id(ui.ctx()),
                gesture,
                sent,
                done: Vec::new(),
            });
        }

        let held = response.dragged();
        let dropped = response.drag_stopped();
        if let Some(drag) = &mut memory.drag
            && let Some(pos) = response.interact_pointer_pos().filter(|_| held || dropped)
        {
            let drag_id = Some(drag.id);
            // A drag starts a few points from where the button went down,
            // and the note under that point is the drag's too.
            let passed = pressed.filter(|_| started).into_iter().chain([pos]);
            let passed = passed.filter_map(|pos| self.at(pos));
            match &mut drag.gesture {
                gesture @ Gesture::Paint { .. } => {
                    let pointer = pressed.filter(|_| started).into_iter().chain([pos]);
                    for pos in pointer {
                        if let Some(laid) = gesture.paint(time.tick(pos.x), view.key(pos.y)) {
                            out.push(edit(laid.clone(), drag_id));
                            drag.sent = Some(laid);
                        }
                    }
                }
                Gesture::Slice { at, key } => {
                    let (at, under) = (*at, view.key(pos.y));
                    let (low, high) = ((*key).min(under), (*key).max(under));
                    let rows = egui::Rangef::new(view.y(high), view.y(low) + KEY);
                    painter.vline(time.x(at as f32), rows, Stroke::new(1.5, strong));
                    if dropped {
                        let cut = |(_, note): &&(NoteId, &Note)| {
                            let at = at as i64;
                            (low..=high).contains(&i32::from(note.key))
                                && note.length > 0
                                && i64::from(note.position) < at
                                && at < view.end(note)
                        };
                        let ids: Vec<NoteId> = own.iter().filter(cut).map(|(id, _)| *id).collect();
                        if !ids.is_empty() {
                            out.push(edit(Edit::SliceNotes { ids, at }, None));
                        }
                    }
                }
                Gesture::Erase => {
                    for (id, ..) in passed {
                        if !drag.done.contains(&id) {
                            drag.done.push(id);
                            out.push(edit(Edit::DeleteNotes { ids: vec![id] }, drag_id));
                        }
                    }
                }
                Gesture::Mute { muted } => {
                    let muted = &*muted;
                    for (id, note, _) in passed {
                        if note.muted() != *muted && !drag.done.contains(&id) {
                            drag.done.push(id);
                            let (ids, muted) = (vec![id], *muted);
                            out.push(edit(Edit::SetNotesMuted { ids, muted }, drag_id));
                        }
                    }
                }
                Gesture::Band { tick, y } => {
                    let band = Rect::from_two_pos(pos2(time.x(*tick), view.top + *y), pos);
                    let fill = ui.visuals().selection.bg_fill.gamma_multiply(0.25);
                    let stroke = Stroke::new(1.0, strong);
                    painter.rect(band, 0.0, fill, stroke, StrokeKind::Inside);
                    if dropped {
                        let inside =
                            |(_, note): &&(NoteId, &Note)| view.rect(note).intersects(band);
                        let ids = own.iter().filter(inside).map(|(id, _)| *id).collect();
                        out.push(edit(Edit::SelectNotes { ids, add: shift }, None));
                    }
                }
                gesture => {
                    let gesture = &*gesture;
                    let moved = gesture.edit(time.tick(pos.x), view.key(pos.y), step);
                    if let Some(moved) = moved.filter(|moved| drag.sent.as_ref() != Some(moved)) {
                        out.push(edit(moved.clone(), drag_id));
                        drag.sent = Some(moved);
                    }
                    if matches!(gesture, Gesture::Resize { .. }) {
                        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                    }
                }
            }
        }
        if !held && let Some(drag) = memory.drag.take() {
            // The next note a click adds is as long as the one this drag
            // drew or resized.
            match (drag.gesture, drag.sent) {
                (Gesture::Draw { .. }, Some(Edit::AddNote { length, .. }))
                | (Gesture::Paint { .. }, Some(Edit::AddNotes { length, .. })) => {
                    memory.length = Some(length);
                }
                (Gesture::Resize { length, .. }, Some(Edit::ResizeNotes { delta_ticks, .. }))
                    if delta_ticks != 0 =>
                {
                    memory.length = u64::try_from(length + delta_ticks).ok();
                }
                _ => {}
            }
        }

        let pointer = response.interact_pointer_pos();
        if response.clicked()
            && let Some(pos) = pointer
        {
            match (tool, self.at(pos)) {
                (Tool::Delete, Some((id, ..))) => {
                    out.push(edit(Edit::DeleteNotes { ids: vec![id] }, None));
                }
                (Tool::Mute, Some((id, note, _))) => {
                    let (ids, muted) = (vec![id], !note.muted());
                    out.push(edit(Edit::SetNotesMuted { ids, muted }, None));
                }
                (Tool::Draw | Tool::Paint | Tool::Select, Some((id, note, _))) => {
                    let ids = vec![id];
                    out.push(edit(Edit::SelectNotes { ids, add: shift }, None));
                    if note.length > 0 {
                        memory.length = u64::try_from(note.length).ok();
                    }
                }
                (Tool::Slice, Some((id, note, _))) => {
                    let at = snap(time.tick(pos.x), step).max(0) as u64;
                    if note.length > 0 && i64::from(note.position) < at as i64 {
                        let cut = Edit::SliceNotes { ids: vec![id], at };
                        out.push(edit(cut, None));
                    }
                }
                (Tool::Draw | Tool::Paint, None) => {
                    if let Ok(key) = u8::try_from(view.key(pos.y)) {
                        let add = Edit::AddNote {
                            channel,
                            position: floor(time.tick(pos.x), step),
                            length: memory.length.unwrap_or(view.step as u64),
                            key: key.min(TOP_KEY as u8),
                        };
                        out.push(edit(add, None));
                    }
                }
                // A click beside the notes lets go of them.
                (Tool::Select, None) if !shift && !selected(own).is_empty() => {
                    let ids = Vec::new();
                    out.push(edit(Edit::SelectNotes { ids, add: false }, None));
                }
                _ => {}
            }
        }
        if response.secondary_clicked()
            && let Some((id, ..)) = pointer.and_then(|pos| self.at(pos))
        {
            out.push(edit(Edit::DeleteNotes { ids: vec![id] }, None));
        }
    }

    /// The lane: a stick for every note of the channel, as high as the
    /// value the lane shows. A field with a centre has its sticks start
    /// there.
    ///
    /// A click or a drag sets the value of the notes under the pointer:
    /// the ones that start last in front of it and have not ended there,
    /// and of those the selected ones when there are any.
    fn lane(&self, ui: &Ui, gutter: Rect, lane: Rect, memory: &mut Memory, out: &mut Vec<Command>) {
        let (view, time, own) = (self.view, self.view.time, &self.own);
        let field = memory.lane;
        let visuals = ui.visuals();
        let line = visuals.widgets.noninteractive.bg_stroke;
        let painter = ui.painter_at(gutter.union(lane));
        painter.rect_filled(gutter, 0.0, visuals.faint_bg_color);
        painter.rect_filled(lane, 0.0, visuals.extreme_bg_color);
        painter.hline(gutter.union(lane).x_range(), lane.top(), line);
        let at = pos2(gutter.left() + 4.0, gutter.center().y);
        let (font, ink) = (FontId::proportional(10.0), visuals.weak_text_color());
        painter.text(at, Align2::LEFT_CENTER, field_name(field), font, ink);

        let painter = ui.painter_at(lane);
        let (top, height) = (f32::from(field.top()), (lane.height() - 4.0).max(1.0));
        let y_of = |value: u8| lane.bottom() - 2.0 - f32::from(value) / top * height;
        let base = y_of(field.centre().unwrap_or(0));
        if field.centre().is_some() {
            let faint = Stroke::new(1.0, line.color.gamma_multiply(0.4));
            painter.hline(lane.x_range(), base, faint);
        }
        for (_, note) in own {
            let x = time.x(note.position as f32) + 1.0;
            if !lane.x_range().expand(4.0).contains(x) {
                continue;
            }
            let ink = if note.selected() {
                visuals.strong_text_color()
            } else {
                self.colour(note)
            };
            let y = y_of(field.of(note));
            painter.line_segment([pos2(x, base), pos2(x, y)], Stroke::new(2.0, ink));
            let head = Rect::from_center_size(pos2(x + 1.5, y), vec2(5.0, 3.0));
            painter.rect_filled(head, 0.0, ink);
        }
        self.playhead(&painter, lane);

        let id = memory_id().with("lane");
        let response = ui.interact(lane, id, Sense::click_and_drag());
        let name = channel_name(self.channel);
        let label = format!("{name} note {}", field_name(field).to_lowercase());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, &label));
        if response.drag_started_by(PointerButton::Primary) {
            let id = next_drag_id(ui.ctx());
            memory.paint = Some(Paint { id, sent: None });
        }
        let held = response.dragged_by(PointerButton::Primary);
        if !held {
            memory.paint = None;
        }
        let pointer = response.interact_pointer_pos();
        let Some(pos) = pointer.filter(|_| held || response.clicked()) else {
            return;
        };
        // Three points of grace, so that a press just left of a stick
        // still takes it.
        let tick = time.tick(pos.x + 3.0).floor() as i64;
        let starts = own.iter().map(|(_, note)| i64::from(note.position));
        let Some(start) = starts.filter(|start| *start <= tick).max() else {
            return;
        };
        let there = || {
            let there = own
                .iter()
                .filter(move |(_, note)| i64::from(note.position) == start);
            there.filter(move |(_, note)| tick < view.end(note) + 3)
        };
        let chosen = there().any(|(_, note)| note.selected());
        let value = ((lane.bottom() - 2.0 - pos.y) / height).clamp(0.0, 1.0) * top;
        let value = value.round() as u8;
        // A drag says a value once for the notes it is over.
        let said = Some((start, value));
        if memory.paint.is_some_and(|paint| paint.sent == said) {
            return;
        }
        if let Some(paint) = &mut memory.paint {
            paint.sent = said;
        }
        let drag_id = memory.paint.map(|paint| paint.id);
        for (id, note) in there().filter(|(_, note)| !chosen || note.selected()) {
            if field.of(note) != value {
                let id = *id;
                out.push(edit(Edit::SetNoteField { id, field, value }, drag_id));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(position: i32, length: i32, key: i16) -> Note {
        Note {
            position,
            length,
            key,
            ..Note::default()
        }
    }

    /// Half a point a tick from x 100, with the highest key's row at y 50.
    fn view() -> View {
        View {
            time: Timeline {
                origin: 100.0,
                left: 0.0,
                scale: 0.5,
            },
            top: 50.0,
            step: 24,
        }
    }

    #[test]
    fn the_keys_count_down_from_the_top() {
        let view = view();
        assert_eq!(view.key(50.0), 131);
        assert_eq!(view.key(61.9), 131);
        assert_eq!(view.key(62.0), 130);
        assert_eq!(view.y(131), 50.0);
        assert_eq!(view.y(60), 50.0 + 71.0 * 12.0);
        assert_eq!(view.key(view.y(60) + 6.0), 60);
        // Past either end the rows go on, and are no keys.
        assert_eq!(view.key(49.0), 132);
        assert_eq!(view.key(view.y(0) + 12.0), -1);
        assert_eq!(
            u8::try_from(view.key(49.0)).ok().map(|key| key.min(131)),
            Some(131)
        );
        assert!(u8::try_from(view.key(view.y(0) + 12.0)).is_err());
    }

    #[test]
    fn a_note_is_a_rectangle_in_its_key_s_row() {
        let view = view();
        let rect = view.rect(&note(96, 48, 131));
        assert_eq!(
            rect,
            Rect::from_min_max(pos2(148.0, 51.0), pos2(171.0, 62.0))
        );
        // A step has no length and is drawn a step long.
        let step = note(96, 0, 130);
        assert_eq!(view.end(&step), 120);
        assert_eq!(view.rect(&step).x_range(), egui::Rangef::new(148.0, 159.0));
        // A note of one tick is still there to grab.
        assert_eq!(view.rect(&note(96, 1, 130)).width(), 2.0);
    }

    #[test]
    fn the_black_keys_and_the_names_are_fl_s() {
        let black: Vec<i32> = (60..72).filter(|key| is_black(*key)).collect();
        assert_eq!(black, [61, 63, 66, 68, 70]);
        assert_eq!(key_name(60), "C5");
        assert_eq!(key_name(0), "C0");
        assert_eq!(key_name(131), "B10");
        assert_eq!(key_name(58), "A#4");
        assert_ne!(group_colour(0), group_colour(1));
        assert_eq!(group_colour(16), group_colour(0));
    }

    #[test]
    fn a_click_starts_a_note_at_the_step_it_is_in() {
        assert_eq!(floor(47.9, 24), 24);
        assert_eq!(floor(48.0, 24), 48);
        assert_eq!(floor(-5.0, 24), 0);
        assert_eq!(floor(47.9, 1), 47);
    }

    #[test]
    fn a_drag_says_how_far_it_is_from_where_it_began() {
        let moves = Gesture::Move {
            ids: vec![4, 7],
            start: 100,
            tick: 110.0,
            key: 60,
        };
        let moved = |delta_ticks, delta_keys| {
            Some(Edit::MoveNotes {
                ids: vec![4, 7],
                delta_ticks,
                delta_keys,
            })
        };
        assert_eq!(moves.edit(110.0, 60, 1), moved(0, 0));
        assert_eq!(moves.edit(160.0, 63, 1), moved(50, 3));
        // The start of the note under the pointer lands on the grid.
        assert_eq!(moves.edit(160.0, 48, 24), moved(44, -12));
        assert_eq!(moves.edit(0.0, 60, 24), moved(-100, 0));

        // A note from 100 to 148, taken by its end.
        let resizes = |length| Gesture::Resize {
            ids: vec![4],
            start: 100,
            end: 148,
            length,
            tick: 146.0,
        };
        let resized = |delta_ticks| {
            let ids = vec![4];
            Some(Edit::ResizeNotes { ids, delta_ticks })
        };
        assert_eq!(resizes(48).edit(146.0, 60, 1), resized(0));
        // The end snaps and the start stays.
        assert_eq!(resizes(48).edit(170.0, 9, 24), resized(20));
        // An end dragged past the start leaves one step.
        assert_eq!(resizes(48).edit(-900.0, 60, 24), resized(-24));
        // A step is drawn to an end it does not have, and gets the length
        // it is dragged to.
        assert_eq!(resizes(0).edit(170.0, 60, 24), resized(68));

        let draws = Gesture::Draw {
            channel: 2,
            position: 96,
            key: 64,
        };
        let drawn = |length| {
            Some(Edit::AddNote {
                channel: 2,
                position: 96,
                length,
                key: 64,
            })
        };
        assert_eq!(draws.edit(200.0, 3, 24), drawn(96));
        // A note is a step of the grid at least.
        assert_eq!(draws.edit(100.0, 64, 24), drawn(24));
        assert_eq!(draws.edit(0.0, 64, 1), drawn(1));
        assert_eq!(Gesture::Erase.edit(0.0, 0, 1), None);
    }
}
