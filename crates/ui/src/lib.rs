//! The fl-rs windows, drawn with egui.
//!
//! [`shell`] draws one frame from a [`UiState`] and pushes a [`Command`] for
//! each thing the user asked for. It owns no project, no audio stream and no
//! file: the app applies the commands and comes back with the next state.
//! The one thing read from disk here is the browser's list of a folder,
//! when the user opens that folder.
//!
//! The state kept here is which windows are open and where they are docked,
//! how far the playlist is zoomed and scrolled, which channel the channel
//! rack has selected and which insert the mixer, and the drag that is under
//! way, and for the piano roll its tool, its lane, its snap, its scale and
//! how far it is zoomed and scrolled, and for the browser the folders it
//! has read and which of them are open. It lives in egui's memory, so it
//! lasts as long as the [`egui::Context`] does. Which
//! playlist items and which notes are selected is in the project: the
//! `flags` of an item and of a note say so.

mod browser;
mod channel_rack;
mod mixer;
mod piano_roll;
mod playlist;
mod top_bar;
mod widgets;
mod windows;

pub use model::NoteField;
pub use windows::Window;

use model::Project;
use std::path::PathBuf;
use std::sync::Arc;

/// A position on the song's timeline, in ticks from its start. The same
/// number as `engine::Ticks`.
pub type Ticks = u64;

/// What the user asked for. The app applies these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open the project in this file. Like `Quit` it asks first when the
    /// open project has changes that are not saved.
    Open(PathBuf),
    /// Write the project to the file it came from, or was last saved to.
    Save,
    /// Write the project to this file.
    SaveAs(PathBuf),
    /// Render the song to a WAV at this path.
    Render(PathBuf),
    /// Ask the user for the path of an `Open`, a `SaveAs` or a `Render`.
    /// The menu sends this, because the file dialog is the app's to show.
    Choose(Dialog),
    Play,
    /// Stop and go back to the start, as FL's stop button does. A sample
    /// that is previewed stops too.
    Stop,
    Seek(Ticks),
    /// Switch between pattern mode and song mode.
    ToggleSongMode,
    /// Switch between stopping at the end of the song and starting over.
    ToggleLoop,
    /// Make the pattern with this index the one that plays in pattern
    /// mode.
    SelectPattern(u16),
    /// Play this sample file once, as it is, beside whatever the project
    /// is doing. The next one cuts it off, and so does `Stop`.
    Preview(PathBuf),
    /// Change the project. This is one step for `Undo`, unless it is part
    /// of a drag.
    ///
    /// Every edit of one drag carries that drag's `drag_id`, and the whole
    /// drag is one step. An edit that moves or resizes says where the drag
    /// is now, measured from where it began, so it takes the place of the
    /// edit of the same kind that the drag sent before it. An edit that
    /// sets a value is made on top of the ones before it. See
    /// [`Edit::replaces_last`].
    Edit {
        edit: Edit,
        drag_id: Option<u64>,
    },
    Undo,
    Redo,
    /// Close the app. It asks first when the project has changes that are
    /// not saved.
    Quit,
}

/// A playlist item: its place in `items` of the arrangement FL shows. An
/// edit never moves an item to another place in that list.
pub type ItemId = usize;

/// A note of the pattern the top bar has selected: its place in the
/// pattern's `notes`, among the notes of every channel. Adding a note and
/// moving notes along the pattern give the notes behind them other places,
/// so an ID is good for the project it was read from, and for the whole of
/// a drag that began on that project. No other edit moves a note in the
/// list, and a deleted note keeps its record.
pub type NoteId = usize;

/// A change to the playlist, to the channel rack, to the piano roll or to
/// the mixer. An ID that names no item or no note, or a record FL deleted,
/// is skipped, and so is a channel index that names no channel and an
/// insert the project does not have.
///
/// The channel rack's values and the mixer's are FL's own, as `model`
/// documents them. An insert goes by the loader's number: 0 is the master.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Move the items along the timeline and `delta_tracks` rows down.
    /// They move as a group, so the group stops where its first item
    /// reaches tick 0, the first track or the last.
    MoveItems {
        ids: Vec<ItemId>,
        delta_ticks: i64,
        delta_tracks: i32,
    },
    /// Give each item this length, in ticks, by moving its end.
    ResizeItems {
        ids: Vec<ItemId>,
        new_length: Ticks,
    },
    SetMuted {
        ids: Vec<ItemId>,
        muted: bool,
    },
    DeleteItems {
        ids: Vec<ItemId>,
    },
    /// Copy the items. The copies go to the end of the list in the order of
    /// `ids`, so the first copy's ID is the length the list had. They are
    /// selected, and the items they were copied from no longer are.
    DuplicateItems {
        ids: Vec<ItemId>,
    },
    /// Select the items, on top of the selection when `add` is set and in
    /// its place when not. This is no step for `Undo` and does not count as
    /// a change to save.
    SelectItems {
        ids: Vec<ItemId>,
        add: bool,
    },
    /// Light or clear a step of the pattern the top bar has selected, for
    /// the channel with this index. Step 0 is the pattern's first
    /// sixteenth note.
    SetStep {
        channel: u16,
        step: u32,
        on: bool,
    },
    /// Switch the channel on or off: the LED beside it.
    SetChannelEnabled {
        channel: u16,
        enabled: bool,
    },
    /// 0 to 12800, where a new channel has 10000.
    SetChannelVolume {
        channel: u16,
        volume: u32,
    },
    /// 0 on the left to 12800 on the right, with 6400 in the centre.
    SetChannelPan {
        channel: u16,
        pan: u32,
    },
    /// The mixer insert the channel plays into, where 0 is the master.
    SetChannelInsert {
        channel: u16,
        insert: u16,
    },
    /// Give the channel this sample file to play. A channel that plays no
    /// sample is skipped: a plugin, a layer or an automation clip.
    SetChannelSample {
        channel: u16,
        path: PathBuf,
    },
    /// Add a sampler channel that plays this sample file, at the end of
    /// the rack and named after the file.
    AddSamplerChannel {
        path: PathBuf,
    },
    /// The velocity of the note that lights the step, 0 to 128. A step
    /// that is not lit is skipped.
    SetStepVelocity {
        channel: u16,
        step: u32,
        velocity: u8,
    },
    /// Add a note for this channel to the pattern the top bar has
    /// selected, which is made when the file has none with that index. Key
    /// 60 is C5, and 131 is the highest. A `length` of 0 is a step, which
    /// has no end.
    AddNote {
        channel: u16,
        position: Ticks,
        length: Ticks,
        key: u8,
    },
    DeleteNotes {
        ids: Vec<NoteId>,
    },
    /// Move the notes along the pattern and `delta_keys` keys up. They
    /// move as a group, so the group stops where its first note reaches
    /// tick 0, and at the lowest key and the highest.
    MoveNotes {
        ids: Vec<NoteId>,
        delta_ticks: i64,
        delta_keys: i32,
    },
    /// Make each note this much longer by moving its end, or shorter when
    /// it is negative. A note keeps a tick at least.
    ResizeNotes {
        ids: Vec<NoteId>,
        delta_ticks: i64,
    },
    SetNotesMuted {
        ids: Vec<NoteId>,
        muted: bool,
    },
    /// A value of one note, from 0 to the field's [`NoteField::top`].
    SetNoteField {
        id: NoteId,
        field: NoteField,
        value: u8,
    },
    /// Add notes for this channel, each `length` ticks long, at the
    /// positions and on the keys of `notes`: what the paint tool lays down
    /// in one drag.
    AddNotes {
        channel: u16,
        length: Ticks,
        notes: Vec<(Ticks, u8)>,
    },
    /// Cut each note that tick `at` falls inside of in two, at `at`.
    SliceNotes {
        ids: Vec<NoteId>,
        at: Ticks,
    },
    /// Join the notes on one key that touch or overlap into one.
    GlueNotes {
        ids: Vec<NoteId>,
    },
    /// Move each note's start to the nearest multiple of `step`, and with
    /// `lengths` its length too.
    QuantizeNotes {
        ids: Vec<NoteId>,
        step: Ticks,
        lengths: bool,
    },
    /// Cut each note into pieces `step` ticks long.
    ChopNotes {
        ids: Vec<NoteId>,
        step: Ticks,
    },
    /// Play each chord among the notes upwards, a note every `step` ticks.
    ArpeggiateNotes {
        ids: Vec<NoteId>,
        step: Ticks,
    },
    /// Start each note of a chord `delay` ticks after the one under it.
    StrumNotes {
        ids: Vec<NoteId>,
        delay: Ticks,
    },
    /// Move each note whose key is outside the scale to the nearest key in
    /// it. `keys` has bit n set for the keys n semitones above a C that
    /// are in the scale.
    SnapNotesToScale {
        ids: Vec<NoteId>,
        keys: u16,
    },
    /// Select the notes, on top of the selection when `add` is set and in
    /// its place when not. Like [`Edit::SelectItems`] this is no step for
    /// `Undo` and no change to save.
    SelectNotes {
        ids: Vec<NoteId>,
        add: bool,
    },
    /// An insert's fader: 0 to 16000, with 12800 at 0 dB.
    SetInsertVolume {
        insert: u16,
        volume: i32,
    },
    /// -6400 on the left to 6400 on the right.
    SetInsertPan {
        insert: u16,
        pan: i32,
    },
    /// Switch the insert on or off: the LED under its fader.
    SetInsertEnabled {
        insert: u16,
        enabled: bool,
    },
    /// Name the insert. An empty name gives it its number back.
    SetInsertName {
        insert: u16,
        name: String,
    },
    /// Make or remove the route from insert `from` to insert `to`. A route
    /// from the master, or one that would lead an insert back into itself,
    /// is not made.
    SetRoute {
        from: u16,
        to: u16,
        on: bool,
    },
    /// The level of the send from insert `from` to insert `to`: 0 to
    /// 16000, with 12800 at unity.
    SetSendLevel {
        from: u16,
        to: u16,
        level: i32,
    },
    /// Switch an effect slot of an insert on or off. The first slot is 0.
    SetSlotEnabled {
        insert: u16,
        slot: u8,
        enabled: bool,
    },
    /// How much of a slot's output is its plugin's: 0 to 12800.
    SetSlotMix {
        insert: u16,
        slot: u8,
        mix: i32,
    },
}

impl Edit {
    /// Whether, in a drag, this edit takes the place of the edit of its
    /// kind that the drag sent before it. A move and a resize do: they
    /// are measured from where the drag began. So does a note that a drag
    /// draws, which is one note however long the drag makes it, and the
    /// notes a drag paints, which are all of them so far. Every
    /// other edit says what a value is to be, and a drag that paints steps
    /// sends one for each.
    pub fn replaces_last(&self) -> bool {
        matches!(
            self,
            Edit::MoveItems { .. }
                | Edit::ResizeItems { .. }
                | Edit::AddNote { .. }
                | Edit::AddNotes { .. }
                | Edit::MoveNotes { .. }
                | Edit::ResizeNotes { .. }
        )
    }
}

/// The file dialogs behind the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Open,
    SaveAs,
    Render,
}

/// Everything [`shell`] draws from.
#[derive(Debug, Clone)]
pub struct UiState {
    pub project: Arc<Project>,
    /// Where the transport's clock is.
    pub position: Ticks,
    /// The master's peak on the left and on the right, where 1.0 is full
    /// scale.
    pub peaks: (f32, f32),
    /// The peak of every insert on the left and on the right, behind its
    /// fader, by the loader's number of the insert. An insert the list is
    /// too short for is silent, so no meters are an empty list.
    pub insert_peaks: Vec<(f32, f32)>,
    pub playing: bool,
    /// Song mode when true, pattern mode when false.
    pub song_mode: bool,
    /// Whether the song starts over when it ends.
    pub looping: bool,
    /// The index of the pattern that plays in pattern mode.
    pub pattern: u16,
    /// FL's `Data` directory or its install directory, when the app was
    /// told one: the first root of the browser.
    pub data_root: Option<PathBuf>,
    /// The folder the open project's file is in: the browser's other
    /// root. `None` while no project is open.
    pub project_dir: Option<PathBuf>,
    /// The rate of the output stream in Hz, or `None` when there is none.
    pub sample_rate: Option<u32>,
    pub last_error: Option<String>,
    /// What a worker thread is doing right now, such as loading a project
    /// or rendering one. Drawn with a spinner, or with a bar when
    /// `progress` says how far it is.
    pub busy: Option<String>,
    /// How much of `busy` is done, from 0 to 1, when that is known.
    pub progress: Option<f32>,
}

/// Draw the whole window: the top bar, the status line and the docked
/// windows between them.
pub fn shell(ui: &mut egui::Ui, state: &UiState, out: &mut Vec<Command>) {
    shortcuts(ui, out);
    egui::Panel::top("top bar").show(ui, |ui| top_bar::show(ui, state, out));
    egui::Panel::bottom("status").show(ui, |ui| status(ui, state));
    egui::CentralPanel::no_frame().show(ui, |ui| windows::show(ui, state, out));
}

/// Ctrl+Z undoes, Ctrl+Shift+Z and Ctrl+Y redo, Ctrl+S saves. On a Mac
/// they go with Cmd.
fn shortcuts(ui: &mut egui::Ui, out: &mut Vec<Command>) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    let ctrl = Modifiers::COMMAND;
    // A shortcut matches with Shift held too, so the one that wants Shift
    // is asked first.
    for (modifiers, key, command) in [
        (ctrl | Modifiers::SHIFT, Key::Z, Command::Redo),
        (ctrl, Key::Y, Command::Redo),
        (ctrl, Key::Z, Command::Undo),
        (ctrl, Key::S, Command::Save),
    ] {
        let shortcut = KeyboardShortcut::new(modifiers, key);
        if ui.input_mut(|input| input.consume_shortcut(&shortcut)) {
            out.push(command);
        }
    }
}

/// The line at the bottom: what is running, what went wrong, the playlist
/// item under the pointer, and the stream's rate.
fn status(ui: &mut egui::Ui, state: &UiState) {
    ui.horizontal(|ui| {
        let rate = match state.sample_rate {
            Some(rate) => format!("{rate} Hz"),
            None => "no audio".to_owned(),
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(rate);
            if let Some(hovered) = playlist::hovered(ui.ctx()) {
                ui.label(hovered);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if let Some(busy) = &state.busy {
                    match state.progress {
                        Some(done) => {
                            let bar = egui::ProgressBar::new(done).show_percentage();
                            ui.add(bar.desired_width(120.0));
                        }
                        None => {
                            ui.spinner();
                        }
                    }
                    ui.label(busy);
                }
                if let Some(error) = &state.last_error {
                    let text = egui::RichText::new(error).color(ui.visuals().error_fg_color);
                    ui.add(egui::Label::new(text).truncate())
                        .on_hover_text(error);
                }
            });
        });
    });
}

/// The song's grid in ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Grid {
    pub beat: u64,
    pub bar: u64,
}

impl Grid {
    /// From the PPQ and the time signature, 4/4 when the project has none.
    /// Events 17 and 18 are read as numerator and denominator. Event 35
    /// `TimeSigMode` can give 17 another meaning, and the model does not
    /// read it yet.
    pub fn of(project: &Project) -> Self {
        let (numerator, denominator) = project.time_signature().unwrap_or((4, 4));
        let whole = u64::from(project.header.ppq) * 4;
        let beat = (whole / u64::from(denominator.max(1))).max(1);
        Grid {
            beat,
            bar: beat * u64::from(numerator.max(1)),
        }
    }

    /// A position as FL counts it: bar and beat from 1, tick from 0.
    pub fn bar_beat_tick(self, position: Ticks) -> (u64, u64, u64) {
        let in_bar = position % self.bar;
        (
            position / self.bar + 1,
            in_bar / self.beat + 1,
            in_bar % self.beat,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(ppq: u16, signature: Option<(u8, u8)>) -> Project {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 0,
            ppq,
        });
        if let Some((numerator, denominator)) = signature {
            project.time_sig_num = Some(numerator);
            project.time_sig_beat = Some(denominator);
        }
        project
    }

    #[test]
    fn a_position_counts_bars_and_beats_from_one() {
        let grid = Grid::of(&project(96, None));
        assert_eq!(grid, Grid { beat: 96, bar: 384 });
        assert_eq!(grid.bar_beat_tick(0), (1, 1, 0));
        assert_eq!(grid.bar_beat_tick(95), (1, 1, 95));
        assert_eq!(grid.bar_beat_tick(96), (1, 2, 0));
        assert_eq!(grid.bar_beat_tick(384 * 2 + 96 * 3 + 5), (3, 4, 5));
    }

    #[test]
    fn the_time_signature_sets_the_beat_and_the_bar() {
        let grid = Grid::of(&project(96, Some((6, 8))));
        assert_eq!(grid, Grid { beat: 48, bar: 288 });
        // A signature of zeroes and a PPQ of zero still give a grid.
        let grid = Grid::of(&project(0, Some((0, 0))));
        assert_eq!(grid, Grid { beat: 1, bar: 1 });
    }
}
