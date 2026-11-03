//! Random input at the shell never panics it.
//!
//! Each case opens one window in front, and the browser beside it, on a
//! project built in code as the other ui tests build theirs, and then
//! throws a few hundred frames of random input at it: pointer moves,
//! clicks, double clicks, drags and right clicks, with modifiers held or
//! not, scrolls, typed text, pastes, and key presses of every shortcut the
//! windows have, the F keys that show and hide the windows among them.
//!
//! The commands the shell sends are carried out between frames as the app
//! carries them out: an edit is made to the project with the model's own
//! methods, a drag's move replaces the move it sent before, undo and redo
//! step through the projects edited, and the transport's commands move
//! the clock, the mode and the pattern. So the shell sees the projects its
//! own edits make, whatever they are.
//!
//! The cases come from a small fixed-seed generator, so a failure repeats.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use model::{
    Arrangement, Channel, ChannelKind, Insert, ItemSource, Note, Pattern, PlaylistItem, Project,
    Slot, Track,
};
use std::mem::{Discriminant, discriminant};
use std::path::PathBuf;
use std::sync::Arc;
use ui::{Command, Edit, UiState, Window};

/// SplitMix64: small, fast and the same on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n`, which must not be 0.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<T: Clone>(&mut self, values: &[T]) -> T {
        values[self.below(values.len())].clone()
    }

    /// A number from 0 up to `top`.
    fn unit(&mut self, top: f32) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32 * top
    }
}

const BAR: i32 = 4 * 96;

fn channel(index: u16, kind: ChannelKind, name: &str, colour: Option<u32>) -> Channel {
    Channel {
        index,
        kind: Some(kind),
        name: Some(name.to_owned()),
        color: colour,
        ..Channel::default()
    }
}

fn note(channel: u16, position: i32, length: i32, key: i16) -> Note {
    Note {
        position,
        channel,
        length,
        key,
        ..Note::default()
    }
}

/// An item of `bars` bars on a track, where the first track is 1.
fn item(source: ItemSource, track: i16, bar: i32, bars: i32) -> PlaylistItem {
    let mut item = PlaylistItem::new(source, bar * BAR, bars * BAR);
    item.track_word = 500 - track;
    item
}

fn header(channels: u16) -> flp::Header {
    flp::Header {
        format: 0,
        channel_count: channels,
        ppq: 96,
    }
}

/// The shell's and the playlist's song: two patterns and a vocal clip
/// over eight bars, one item muted and one deleted, and a named track.
fn song() -> Project {
    let mut project = Project::new(header(2));
    project.writer_version = Some("24.1.1.4285".to_owned());
    project.tempo_milli = Some(140_000);
    project.time_sig_num = Some(4);
    project.time_sig_beat = Some(4);
    project.channels = vec![
        channel(0, ChannelKind::Sampler, "Kick", None),
        channel(1, ChannelKind::AudioClip, "Vocal", Some(0x0040_a0e0)),
    ];
    project.patterns = vec![
        Pattern {
            index: 1,
            name: Some("Drums".to_owned()),
            color: Some(0x0050_70d0),
            notes: (0..4).map(|beat| note(0, beat * 96, 0, 60)).collect(),
            ..Pattern::default()
        },
        Pattern {
            index: 2,
            color: Some(0x00d0_9050),
            ..Pattern::default()
        },
    ];
    let mut muted = item(ItemSource::Pattern(2), 2, 6, 2);
    muted.set_muted(true);
    let mut deleted = item(ItemSource::Pattern(1), 3, 0, 1);
    deleted.source_id = u32::MAX;
    project.arrangements.push(Arrangement {
        index: 0,
        items: vec![
            item(ItemSource::Pattern(1), 1, 0, 4),
            item(ItemSource::Pattern(1), 1, 4, 4),
            item(ItemSource::Pattern(2), 2, 2, 4),
            muted,
            deleted,
            item(ItemSource::Channel(1), 4, 1, 6),
        ],
        tracks: vec![Track {
            index: 2,
            name: Some("Bass".to_owned()),
            ..Track::default()
        }],
        ..Arrangement::default()
    });
    project
}

/// The channel rack's and the piano roll's bar of drums and bass, with
/// the mixer's inserts: the master, six inserts and the current track,
/// one with a plugin and a send.
fn beat() -> Project {
    let mut project = Project::new(header(4));
    project.writer_version = Some("25.1.0.4858".to_owned());
    project.tempo_milli = Some(128_500);
    project.channels = vec![
        channel(0, ChannelKind::Sampler, "Kick", Some(0x0040_60d0)),
        channel(1, ChannelKind::Sampler, "Snare", None),
        channel(2, ChannelKind::Plugin, "Bass", Some(0x0060_c080)),
        channel(3, ChannelKind::AudioClip, "Vocal", Some(0x00d0_9050)),
    ];
    project.channels[0].routed_to = Some(3);
    project.channels[1].enabled = Some(false);
    let mut notes: Vec<Note> = (0..4).map(|beat| note(0, beat * 96, 0, 60)).collect();
    notes.extend([note(1, 96, 0, 52), note(1, 288, 0, 52)]);
    notes.extend([
        note(2, 0, 96, 48),
        note(2, 96, 48, 55),
        note(2, 192, 144, 51),
    ]);
    notes.sort_by_key(|note| note.position);
    project.patterns.push(Pattern {
        index: 1,
        name: Some("Beat".to_owned()),
        notes,
        ..Pattern::default()
    });
    project.insert_count = Some(8);
    let flags = |flags: u32| flp::event::layout::InsertFlags {
        unknown_0: [0; 4],
        flags,
        unknown_8: [0; 4],
        len: 12,
    };
    project.inserts = vec![
        Insert {
            flags: Some(flags(0b1100)),
            ..Insert::default()
        };
        8
    ];
    project.inserts[1].name = Some("Drums".to_owned());
    project.inserts[2].routing = Some(vec![0, 0, 0, 0, 1]);
    project.inserts[2].slots = vec![Slot {
        index: 0,
        plugin_name: Some("Fruity Fast Dist".to_owned()),
        ..Slot::default()
    }];
    project.inserts[3].flags = Some(flags(0b0100));
    project
}

/// Values only a file gives: notes and items at the ends of their fields,
/// a PPQ of 1, and a pattern and a channel at the top of their indices.
fn ends() -> Project {
    let mut project = beat();
    project.header.ppq = 1;
    project.tempo_milli = Some(u32::MAX);
    project
        .channels
        .push(channel(u16::MAX, ChannelKind::Plugin, "", None));
    project.patterns[0].notes.extend([
        note(0, i32::MAX, i32::MAX, i16::MAX),
        note(2, i32::MIN, i32::MAX, i16::MIN),
        note(u16::MAX, 0, -1, 131),
    ]);
    project.patterns.push(Pattern {
        index: u16::MAX,
        length: Some(u32::MAX),
        notes: vec![note(0, i32::MAX - 1, 1, 60)],
        ..Pattern::default()
    });
    let mut far = item(ItemSource::Pattern(1), 1, 0, 1);
    (far.position, far.length) = (i32::MAX, i32::MAX);
    let mut low = item(ItemSource::Pattern(u16::MAX), 1, 0, 1);
    (low.position, low.track_word) = (i32::MIN, i16::MIN);
    let mut short = item(ItemSource::Channel(3), 2, 1, 1);
    short.length = i32::MIN;
    project.arrangements.push(Arrangement {
        index: 0,
        items: vec![far, low, short],
        ..Arrangement::default()
    });
    project
}

/// What the harness keeps between frames: what the shell draws from, and
/// what the app keeps to carry out the shell's commands.
struct App {
    state: UiState,
    undo: Vec<Arc<Project>>,
    redo: Vec<Arc<Project>>,
    /// The drag the last edit was part of: its ID, the kind of its last
    /// edit, and the project that edit was made to.
    drag: Option<(u64, Discriminant<Edit>, Arc<Project>)>,
    /// How many edits changed the project.
    made: usize,
}

impl App {
    fn new(project: Project, folder: PathBuf) -> Self {
        App {
            state: UiState {
                project: Arc::new(project),
                position: 0,
                peaks: (0.0, 0.0),
                insert_peaks: Vec::new(),
                playing: false,
                song_mode: true,
                looping: false,
                pattern: 1,
                data_root: None,
                project_dir: Some(folder),
                sample_rate: Some(48_000),
                last_error: None,
                busy: None,
                progress: None,
                loop_range: None,
            },
            undo: Vec::new(),
            redo: Vec::new(),
            drag: None,
            made: 0,
        }
    }

    /// Carry out a command as `fl_rs::App::apply` does, as far as the
    /// shell can see it. Files, the stream and the dialogs are left out.
    fn apply(&mut self, command: Command) {
        let state = &mut self.state;
        match command {
            Command::Play => state.playing = true,
            Command::Stop => (state.playing, state.position) = (false, 0),
            Command::Seek(ticks) => state.position = ticks,
            Command::SetLoopRange(range) => state.loop_range = range,
            Command::ToggleSongMode => state.song_mode = !state.song_mode,
            Command::ToggleLoop => state.looping = !state.looping,
            Command::SelectPattern(index) => (state.pattern, state.loop_range) = (index, None),
            Command::Undo => {
                if let Some(project) = self.undo.pop() {
                    let now = std::mem::replace(&mut state.project, project);
                    self.redo.push(now);
                    self.drag = None;
                }
            }
            Command::Redo => {
                if let Some(project) = self.redo.pop() {
                    let now = std::mem::replace(&mut state.project, project);
                    self.undo.push(now);
                    self.drag = None;
                }
            }
            Command::Edit { edit, drag_id } => self.edit(&edit, drag_id),
            _ => {}
        }
    }

    /// `fl_rs::App::edit`: a drag is one undo step, and its move or resize
    /// is made to the project its last one was made to.
    fn edit(&mut self, edit: &Edit, drag_id: Option<u64>) {
        let kind = discriminant(edit);
        let drag = self.drag.take().filter(|(id, ..)| Some(*id) == drag_id);
        let continues = drag.is_some();
        let base = match drag {
            Some((_, last, base)) if last == kind && edit.replaces_last() => base,
            _ => Arc::clone(&self.state.project),
        };
        let mut project = (*base).clone();
        apply_edit(&mut project, self.state.pattern, edit);
        if !continues {
            if project == *base {
                return;
            }
            self.undo.push(Arc::clone(&self.state.project));
            self.redo.clear();
        }
        self.drag = drag_id.map(|id| (id, kind, base));
        self.made += usize::from(project != *self.state.project);
        self.state.project = Arc::new(project);
    }
}

/// `fl_rs`'s `apply_edit`: the same model methods, called the same way.
fn apply_edit(project: &mut Project, pattern: u16, edit: &Edit) {
    let (version, ppq) = (project.version(), project.header.ppq);
    match edit.clone() {
        Edit::SetStep { channel, step, on } => {
            let has_pattern = project.pattern(pattern).is_some();
            if project.channel(channel).is_some() && (on || has_pattern) {
                project
                    .pattern_to_edit(pattern)
                    .set_step(channel, step, on, ppq);
            }
        }
        Edit::SetStepVelocity {
            channel,
            step,
            velocity,
        } => {
            if let Some(pattern) = project.patterns.iter_mut().find(|p| p.index == pattern)
                && let Some(note) = pattern.step_note(channel, step, ppq)
            {
                pattern.set_note_velocity(note, velocity);
            }
        }
        Edit::AddNote {
            channel,
            position,
            length,
            key,
        } => {
            let place = i32::try_from(position).ok().zip(i32::try_from(length).ok());
            if let Some((position, length)) = place
                && project.channel(channel).is_some()
            {
                project.pattern_to_edit(pattern).add_note(Note {
                    position,
                    channel,
                    length,
                    key: i16::from(key),
                    ..Note::default()
                });
            }
        }
        Edit::AddNotes {
            channel,
            length,
            notes,
        } => {
            if project.channel(channel).is_some() && !notes.is_empty() {
                project
                    .pattern_to_edit(pattern)
                    .add_notes(channel, length, &notes);
            }
        }
        Edit::PasteNotes { notes } => {
            let has = |note: &&Note| project.channel(note.channel).is_some();
            let notes: Vec<Note> = notes.iter().filter(has).copied().collect();
            if !notes.is_empty() {
                project.pattern_to_edit(pattern).paste_notes(&notes);
            }
        }
        Edit::SetChannelEnabled { channel, enabled } => {
            project.set_channel_enabled(channel, enabled);
        }
        Edit::SetChannelVolume { channel, volume } => project.set_channel_volume(channel, volume),
        Edit::SetChannelPan { channel, pan } => project.set_channel_pan(channel, pan),
        Edit::SetChannelInsert { channel, insert } => project.set_channel_insert(channel, insert),
        Edit::SetChannelSample { channel, path } => {
            project.set_channel_sample(channel, &path.to_string_lossy());
        }
        Edit::AddSamplerChannel { path } => {
            project.add_sampler_channel(&path.to_string_lossy());
        }
        Edit::SetChannelName { channel, name } => project.set_channel_name(channel, &name),
        Edit::SoloChannel { channel } => project.solo_channel(channel),
        Edit::CloneChannel { channel } => {
            project.clone_channel(channel);
        }
        Edit::FillSteps {
            channel,
            every,
            count,
        } => {
            if project.channel(channel).is_some() && every > 0 {
                project
                    .pattern_to_edit(pattern)
                    .fill_steps(channel, every, count, ppq);
            }
        }
        Edit::ClearSteps { channel } => {
            if let Some(pattern) = project.patterns.iter_mut().find(|p| p.index == pattern) {
                pattern.clear_steps(channel);
            }
        }
        Edit::ShiftSteps { channel, by, count } => {
            if let Some(pattern) = project.patterns.iter_mut().find(|p| p.index == pattern) {
                pattern.shift_steps(channel, by, count, ppq);
            }
        }
        Edit::SetInsertVolume { insert, volume } => project.set_insert_volume_raw(insert, volume),
        Edit::SetInsertPan { insert, pan } => project.set_insert_pan_raw(insert, pan),
        Edit::SetInsertEnabled { insert, enabled } => project.set_insert_enabled(insert, enabled),
        Edit::SoloInsert { insert } => project.solo_insert(insert),
        Edit::SetInsertSeparation { insert, separation } => {
            project.set_insert_separation_raw(insert, separation);
        }
        Edit::SetInsertPolarity { insert, reversed } => {
            project.set_insert_polarity_reversed(insert, reversed);
        }
        Edit::SetInsertSwapped { insert, swapped } => project.set_insert_swapped(insert, swapped),
        Edit::SetInsertName { insert, name } => project.set_insert_name(insert, &name),
        Edit::SetRoute { from, to, on } => project.set_route(from, to, on),
        Edit::SetSendLevel { from, to, level } => project.set_send_level_raw(from, to, level),
        Edit::SetSlotEnabled {
            insert,
            slot,
            enabled,
        } => project.set_slot_enabled(insert, slot, enabled),
        Edit::SetSlotMix { insert, slot, mix } => project.set_slot_mix_raw(insert, slot, mix),
        Edit::SetTempo { milli } => project.set_tempo_milli(milli),
        Edit::AddPattern { index } => project.add_pattern(index),
        Edit::ClonePattern { from, to } => project.clone_pattern(from, to),
        Edit::SetPatternName { pattern, name } => project.set_pattern_name(pattern, &name),
        Edit::SplitItems { ids, at } => {
            if let Ok(at) = i32::try_from(at) {
                project.split_items(&ids, at);
            }
        }
        Edit::MoveItems {
            ids,
            delta_ticks,
            delta_tracks,
        } => {
            if let Some(playlist) = project.current_arrangement_mut() {
                playlist.move_items(&ids, delta_ticks, delta_tracks, version);
            }
        }
        Edit::ResizeItems { ids, new_length } => {
            if let Some(playlist) = project.current_arrangement_mut() {
                playlist.resize_items(&ids, i32::try_from(new_length).unwrap_or(i32::MAX));
            }
        }
        Edit::SetMuted { ids, muted } => {
            if let Some(playlist) = project.current_arrangement_mut() {
                playlist.mute_items(&ids, muted);
            }
        }
        Edit::DeleteItems { ids } => {
            if let Some(playlist) = project.current_arrangement_mut() {
                playlist.delete_items(&ids);
            }
        }
        Edit::DuplicateItems { ids } => {
            if let Some(playlist) = project.current_arrangement_mut() {
                playlist.duplicate_items(&ids);
            }
        }
        Edit::SelectItems { ids, add } => {
            if let Some(playlist) = project.current_arrangement_mut() {
                playlist.select_items(&ids, add);
            }
        }
        notes => {
            if let Some(pattern) = project.patterns.iter_mut().find(|p| p.index == pattern) {
                edit_notes(pattern, notes);
            }
        }
    }
}

/// The piano roll's half of [`apply_edit`].
fn edit_notes(pattern: &mut Pattern, edit: Edit) {
    match edit {
        Edit::DeleteNotes { ids } => pattern.delete_notes(&ids),
        Edit::MoveNotes {
            ids,
            delta_ticks,
            delta_keys,
        } => pattern.move_notes(&ids, delta_ticks, delta_keys),
        Edit::ResizeNotes { ids, delta_ticks } => pattern.resize_notes(&ids, delta_ticks),
        Edit::SetNotesMuted { ids, muted } => pattern.set_notes_muted(&ids, muted),
        Edit::SetNoteField { id, field, value } => pattern.set_note_field(id, field, value),
        Edit::SelectNotes { ids, add } => pattern.select_notes(&ids, add),
        Edit::SliceNotes { ids, at } => pattern.slice_notes(&ids, at),
        Edit::SliceNotesAlong {
            ids,
            from,
            to,
            step,
        } => pattern.slice_notes_along(&ids, from, to, step),
        Edit::GlueNotes { ids } => pattern.glue_notes(&ids),
        Edit::QuantizeNotes { ids, step, lengths } => pattern.quantize_notes(&ids, step, lengths),
        Edit::ChopNotes { ids, step } => pattern.chop_notes(&ids, step),
        Edit::ArpeggiateNotes { ids, step } => pattern.arpeggiate_notes(&ids, step),
        Edit::StrumNotes { ids, delay } => pattern.strum_notes(&ids, delay),
        Edit::SnapNotesToScale { ids, keys } => pattern.snap_notes_to_scale(&ids, keys),
        Edit::ReverseNotes { ids } => pattern.reverse_notes(&ids),
        Edit::FlipNotes { ids } => pattern.flip_notes(&ids),
        Edit::LegatoNotes { ids } => pattern.legato_notes(&ids),
        _ => {}
    }
}

/// Every key a window or the shell takes, the window keys among them.
const KEYS: [Key; 31] = [
    Key::A,
    Key::B,
    Key::C,
    Key::D,
    Key::E,
    Key::L,
    Key::M,
    Key::Q,
    Key::S,
    Key::U,
    Key::V,
    Key::X,
    Key::Y,
    Key::Z,
    Key::ArrowLeft,
    Key::ArrowRight,
    Key::ArrowUp,
    Key::ArrowDown,
    Key::Delete,
    Key::Backspace,
    Key::Enter,
    Key::Escape,
    Key::Space,
    Key::Tab,
    Key::F2,
    Key::F4,
    Key::F5,
    Key::F6,
    Key::F7,
    Key::F8,
    Key::F9,
];

fn modifiers(rng: &mut Rng) -> Modifiers {
    let ctrl = Modifiers::COMMAND;
    rng.pick(&[
        Modifiers::NONE,
        Modifiers::NONE,
        Modifiers::NONE,
        ctrl,
        Modifiers::SHIFT,
        Modifiers::ALT,
        ctrl | Modifiers::SHIFT,
        ctrl | Modifiers::ALT,
        Modifiers::ALT | Modifiers::SHIFT,
    ])
}

/// A point on the screen: mostly on a widget the shell drew last frame,
/// where a click does something, and now and then just off the screen.
fn point(rng: &mut Rng, size: Vec2, widgets: &[Rect]) -> Pos2 {
    match rng.below(20) {
        0 => rng.pick(&[pos2(-5.0, -5.0), pos2(size.x + 50.0, size.y / 2.0)]),
        1..12 if !widgets.is_empty() => {
            let rect = rng.pick(widgets);
            rect.min + vec2(rng.unit(rect.width()), rng.unit(rect.height()))
        }
        _ => pos2(rng.unit(size.x), rng.unit(size.y)),
    }
}

fn button(pos: Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers,
    }
}

/// Queue one thing a user does, as the events it makes.
fn act(rng: &mut Rng, harness: &Harness<'_, App>, size: Vec2) {
    // Mostly the small ones, buttons, menu entries and a strip's knobs,
    // and else the large ones: the notes, the steps, the playlist.
    let small = |rect: &Rect| rect.area() < size.x * size.y / 16.0;
    let (small, large): (Vec<Rect>, Vec<Rect>) = widgets(harness).into_iter().partition(small);
    let widgets = if rng.below(3) == 0 { large } else { small };
    let held = modifiers(rng);
    let at = point(rng, size, &widgets);
    let mut events = vec![Event::ModifiersChanged(held), Event::PointerMoved(at)];
    let primary = PointerButton::Primary;
    match rng.below(12) {
        // A move alone, which shows what is under the pointer.
        0 | 1 => {}
        2 | 3 => events.extend([
            button(at, primary, true, held),
            button(at, primary, false, held),
        ]),
        4 => {
            for _ in 0..2 {
                events.extend([
                    button(at, primary, true, held),
                    button(at, primary, false, held),
                ]);
            }
        }
        5 => {
            let other = rng.pick(&[PointerButton::Secondary, PointerButton::Middle]);
            events.extend([
                button(at, other, true, held),
                button(at, other, false, held),
            ]);
        }
        6 | 7 => {
            // A drag, in a few steps, to anywhere or to near where it began.
            let to = match rng.below(2) {
                0 => point(rng, size, &widgets),
                _ => at + vec2(rng.unit(80.0) - 40.0, rng.unit(40.0) - 20.0),
            };
            let which = rng.pick(&[primary, primary, PointerButton::Secondary]);
            events.push(button(at, which, true, held));
            let steps = 1 + rng.below(5);
            for step in 1..=steps {
                events.push(Event::PointerMoved(at.lerp(to, step as f32 / steps as f32)));
            }
            events.push(button(to, which, false, held));
        }
        8 => events.push(Event::MouseWheel {
            unit: rng.pick(&[
                egui::MouseWheelUnit::Point,
                egui::MouseWheelUnit::Line,
                egui::MouseWheelUnit::Page,
            ]),
            delta: vec2(rng.unit(400.0) - 200.0, rng.unit(400.0) - 200.0),
            phase: egui::TouchPhase::Move,
            modifiers: held,
        }),
        9 => {
            let text = rng.pick(&["a", "7", "-", ".", "Ä", "\u{1F600}", "\0", "  ", "999999"]);
            events.push(Event::Text(text.to_owned()));
        }
        10 => {
            let text = rng.pick(&["", "C5", "fl-rs notes", "\u{1F600}"]);
            events.push(rng.pick(&[Event::Paste(text.to_owned()), Event::Copy, Event::Cut]));
        }
        _ => {
            let key = rng.pick(&KEYS);
            for pressed in [true, false] {
                events.push(Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: held,
                });
            }
        }
    }
    events.push(Event::ModifiersChanged(Modifiers::NONE));
    for event in events {
        harness.event(event);
    }
}

/// Where the shell drew its widgets last frame. Pixels per point are 1,
/// so a node's box is in points.
fn widgets(harness: &Harness<'_, App>) -> Vec<Rect> {
    let rect = |node: egui_kittest::Node<'_>| {
        let b = node.accesskit_node().bounding_box()?;
        Some(Rect::from_min_max(
            pos2(b.x0 as f32, b.y0 as f32),
            pos2(b.x1 as f32, b.y1 as f32),
        ))
    };
    harness.query_all(By::new()).filter_map(rect).collect()
}

/// Click a small widget, with either button, and then one of the widgets
/// the click brought up, if any: an entry of the menu it opened, say, or
/// of the menu's submenu. A text field it opened gets some text and Enter.
fn choose(rng: &mut Rng, harness: &mut Harness<'_, App>, size: Vec2) {
    let mut before = widgets(harness);
    before.retain(|rect| rect.area() < size.x * size.y / 16.0);
    if before.is_empty() {
        return;
    }
    let mut which = rng.pick(&[PointerButton::Primary, PointerButton::Secondary]);
    for depth in 0..3 {
        let rect = rng.pick(&before);
        let at = rect.center();
        harness.event(Event::PointerMoved(at));
        let clicks = if depth == 0 && rng.below(4) == 0 {
            2
        } else {
            1
        };
        for _ in 0..clicks {
            harness.event(button(at, which, true, Modifiers::NONE));
            harness.event(button(at, which, false, Modifiers::NONE));
        }
        harness.step();
        if rng.below(3) == 0 {
            let text = rng.pick(&["Lead", "", "\u{1F600}", "120.5", "x"]);
            harness.event(Event::Text(text.to_owned()));
            harness.key_press(rng.pick(&[Key::Enter, Key::Escape, Key::Tab]));
            harness.step();
        }
        let after = widgets(harness);
        let new: Vec<Rect> = after.into_iter().filter(|r| !before.contains(r)).collect();
        if new.is_empty() {
            return;
        }
        before = new;
        which = PointerButton::Primary;
    }
}

/// A shell of `size` on `project`, with `front` in view and the browser
/// beside it, given `actions` random things to do. How many edits the
/// shell made.
fn throw_at(rng: &mut Rng, project: Project, front: Window, size: Vec2, actions: usize) -> usize {
    let folder = std::env::temp_dir().join(format!("fl-rs-random-input-{}", std::process::id()));
    std::fs::create_dir_all(folder.join("Drums")).unwrap();
    std::fs::write(folder.join("kick.wav"), b"RIFF").unwrap();
    std::fs::write(folder.join("Drums").join("snare.wav"), b"").unwrap();
    let mut app = App::new(project, folder);
    // Now and then the meters show what a broken stream could send.
    if rng.below(4) == 0 {
        let peak = rng.pick(&[0.0, 1.3, f32::INFINITY, f32::NAN, -1.0]);
        app.state.peaks = (peak, 0.5);
        app.state.insert_peaks = vec![(peak, peak); rng.below(600)];
        app.state.busy = Some("Rendering".to_owned());
        app.state.progress = Some(rng.pick(&[0.0, 0.5, 1.5, f32::NAN]));
        app.state.position = rng.pick(&[0, u64::MAX, i32::MAX as u64]);
    }
    let mut harness = Harness::builder()
        .with_step_dt(0.05)
        .with_size(size)
        .with_theme(egui::Theme::Dark)
        .build_ui_state(
            |ui, app: &mut App| {
                let mut out = Vec::new();
                ui::shell(ui, &app.state, &mut out);
                for command in out {
                    app.apply(command);
                }
            },
            app,
        );
    harness.run_steps(2);
    for window in [Window::Browser, front] {
        if window != Window::Playlist {
            harness.key_press(window.key());
            harness.step();
        }
    }
    for _ in 0..actions {
        if rng.below(4) == 0 {
            choose(rng, &mut harness, size);
        } else {
            act(rng, &harness, size);
            harness.step();
        }
    }
    harness.state().made
}

/// Run the cases of one project, every window in front in turn, and see
/// that the shell made `edits` edits at least, so that the projects it
/// drew were edited ones.
fn every_window(seed: u64, project: fn() -> Project, edits: usize) {
    let mut rng = Rng(seed);
    let mut made = 0;
    for front in Window::ALL {
        for _ in 0..2 {
            let size = rng.pick(&[
                vec2(900.0, 480.0),
                vec2(1280.0, 720.0),
                vec2(1280.0, 720.0),
                vec2(640.0, 360.0),
                vec2(220.0, 140.0),
            ]);
            made += throw_at(&mut rng, project(), front, size, 300);
        }
    }
    assert!(made >= edits, "the shell made {made} edits");
}

#[test]
fn random_input_on_the_song_never_panics() {
    every_window(31, song, 100);
}

#[test]
fn random_input_on_the_beat_and_its_mixer_never_panics() {
    every_window(32, beat, 100);
}

#[test]
fn random_input_on_values_at_their_ends_never_panics() {
    every_window(33, ends, 100);
}

#[test]
fn random_input_on_an_empty_project_never_panics() {
    every_window(34, || Project::new(header(0)), 0);
}
