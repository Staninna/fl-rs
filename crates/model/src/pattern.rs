//! Patterns, and what the channel rack and the piano roll change: the steps
//! of a pattern, which are notes, the notes themselves, and a channel's
//! switch, volume, pan and insert.

use crate::codec::object;
use crate::{Channel, ChannelKind, Item, Placed, Project, TimeMarker};
use flp::event::{Id, layout};

/// One pattern. 65 `PatternID.New` selects it by index and resets nothing,
/// so a file that selects the same index twice adds to the same pattern.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pattern {
    /// 65 `PatternID.New`: u16 index. The loader caps it at 999.
    pub index: u16,
    /// 193 `PatternID.Name`: string.
    pub name: Option<String>,
    /// 150 `PatternID.Color`: RGBA u32.
    pub color: Option<u32>,
    /// 164 `PatternID.Length`: u32 ticks.
    pub length: Option<u32>,
    /// 26 `PatternID.Looped`: bool.
    pub looped: Option<bool>,
    /// 224 `PatternID.Notes`: 24-byte records with position, channel,
    /// length, key, velocity and the rest as named fields. A deleted
    /// record stays in the list, so skip those where
    /// [`Note::is_deleted`](layout::Note::is_deleted).
    /// [`to_flp`](crate::to_flp) leaves them out of the file. Only the
    /// pattern's first 224 is read.
    pub notes: Vec<layout::Note>,
    /// 223 `PatternID.Controllers`: 12-byte records. Only the pattern's
    /// first 223 is read.
    pub controller_events: Vec<layout::ControllerEvent>,
    /// Markers closed by a 205 while this pattern was selected, in files
    /// written by 11.5.1 or later.
    pub time_markers: Vec<TimeMarker>,
    /// Every event of the pattern in the order it arrived.
    pub order: Vec<Placed>,
}

object!(Pattern {
    key index = PatternNew;
    one name = PatternName;
    one color = PatternColor;
    one length = PatternLength;
    one looped = PatternLooped;
    list notes = PatternNotes;
    list controller_events = PatternControllers;
});

/// The key of a step: C5 as FL counts octaves, which plays a sample at its
/// own pitch. FL's step toggle writes 60 whatever the channel's root note
/// is: the corpus has steps of key 60 on channels with a root of 56, 59,
/// 62 and 64.
const STEP_KEY: i16 = 60;

/// The top of a channel's volume and of its pan.
const LEVEL_TOP: u32 = 12_800;

/// A value of a note that the lane under the piano roll draws and sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteField {
    Velocity,
    Pan,
    FinePitch,
    Release,
    ModX,
    ModY,
}

impl NoteField {
    pub const ALL: [NoteField; 6] = [
        NoteField::Velocity,
        NoteField::Pan,
        NoteField::FinePitch,
        NoteField::Release,
        NoteField::ModX,
        NoteField::ModY,
    ];

    /// The highest value the field takes. The lowest is 0. The corpus has
    /// fine pitches from 0 to 240, which is an octave either way, and the
    /// whole byte for mod X and mod Y.
    pub fn top(self) -> u8 {
        match self {
            NoteField::Velocity | NoteField::Pan | NoteField::Release => 128,
            NoteField::FinePitch => 240,
            NoteField::ModX | NoteField::ModY => 255,
        }
    }

    /// The value that is no pan and no offset, for the fields that go
    /// both ways from one. It is what FL gives a new note.
    pub fn centre(self) -> Option<u8> {
        match self {
            NoteField::Velocity | NoteField::Release => None,
            NoteField::Pan => Some(64),
            NoteField::FinePitch => Some(120),
            NoteField::ModX | NoteField::ModY => Some(128),
        }
    }

    /// The field's value in a note.
    pub fn of(self, note: &layout::Note) -> u8 {
        match self {
            NoteField::Velocity => note.velocity,
            NoteField::Pan => note.pan,
            NoteField::FinePitch => note.fine_pitch,
            NoteField::Release => note.release,
            NoteField::ModX => note.mod_x,
            NoteField::ModY => note.mod_y,
        }
    }

    fn of_mut(self, note: &mut layout::Note) -> &mut u8 {
        match self {
            NoteField::Velocity => &mut note.velocity,
            NoteField::Pan => &mut note.pan,
            NoteField::FinePitch => &mut note.fine_pitch,
            NoteField::Release => &mut note.release,
            NoteField::ModX => &mut note.mod_x,
            NoteField::ModY => &mut note.mod_y,
        }
    }
}

/// How long a step is in ticks: a sixteenth note, and a tick at least.
fn step_ticks(ppq: u16) -> i64 {
    i64::from((ppq / 4).max(1))
}

/// The step sequencer, which is a view of the pattern's notes. This is
/// what `FUN_012d3e60` and `FUN_012d4140` of `docs/records-224-233.md` do.
///
/// Step `s` of a channel is the ticks from `s × ppq / 4` up to the next
/// step. It is lit when a note of the channel that is not muted starts in
/// there, wherever in the step and however long the note is. The note FL
/// makes for a step starts at the step's first tick with length 0 and
/// [`STEP_KEY`].
impl Pattern {
    /// One entry per step, lit or not, up to the step of the pattern's
    /// last note. That note may be another channel's, so every channel of
    /// a pattern gives as many entries. A pattern without notes gives none.
    pub fn steps(&self, channel: u16, ppq: u16) -> Vec<bool> {
        let step = step_ticks(ppq);
        let of = |note: &layout::Note| {
            let position = usize::try_from(note.position).ok()?;
            Some(position / step as usize)
        };
        let placed = || self.notes.iter().filter(|note| !note.is_deleted());
        let count = placed().filter_map(of).max().map_or(0, |last| last + 1);
        let mut steps = vec![false; count];
        let lit = placed().filter(|note| note.channel == channel && !note.muted());
        for step in lit.filter_map(of) {
            steps[step] = true;
        }
        steps
    }

    /// The note that lights the step, as its place in [`Pattern::notes`]:
    /// the first of the channel's notes in the step that is not muted.
    pub fn step_note(&self, channel: u16, step: u32, ppq: u16) -> Option<usize> {
        let mut notes = self.in_step(channel, step, ppq);
        notes.find(|note| !self.notes[*note].muted())
    }

    /// Light the step or clear it.
    ///
    /// Lighting a step that is lit changes nothing. Otherwise the step
    /// gets FL's new note, which goes in front of the notes that start
    /// where it does: every 224 in the corpus has its notes in the order
    /// of their positions. Clearing a step removes every note of the
    /// channel that starts in it, the muted and the long ones too.
    ///
    /// A pattern whose 224 the model could not read is left as it is.
    pub fn set_step(&mut self, channel: u16, step: u32, on: bool, ppq: u16) {
        if !on {
            let mut clear = self.in_step(channel, step, ppq).collect::<Vec<_>>();
            clear.reverse();
            for note in clear {
                self.notes.remove(note);
            }
            return;
        }
        let position = i32::try_from(i64::from(step) * step_ticks(ppq));
        let Ok(position) = position else {
            return;
        };
        if self.step_note(channel, step, ppq).is_some() || !self.writes_notes() {
            return;
        }
        let note = layout::Note {
            position,
            channel,
            key: STEP_KEY,
            ..layout::Note::default()
        };
        let at = self.notes.partition_point(|note| note.position < position);
        self.notes.insert(at, note);
    }

    /// Light the step when it is not lit, and clear it when it is.
    pub fn toggle_step(&mut self, channel: u16, step: u32, ppq: u16) {
        let lit = self.step_note(channel, step, ppq).is_some();
        self.set_step(channel, step, !lit, ppq);
    }

    /// Set the velocity of the note at this place in [`Pattern::notes`],
    /// 128 at most. A place without a note is skipped.
    pub fn set_note_velocity(&mut self, note: usize, velocity: u8) {
        if let Some(note) = self.placed_note(note) {
            note.velocity = velocity.min(128);
        }
    }

    /// Set the pan of the note at this place in [`Pattern::notes`], where
    /// 64 is the centre and 128 the right. A place without a note is
    /// skipped.
    pub fn set_note_pan(&mut self, note: usize, pan: u8) {
        if let Some(note) = self.placed_note(note) {
            note.pan = pan.min(128);
        }
    }

    pub(crate) fn placed_note(&mut self, note: usize) -> Option<&mut layout::Note> {
        self.notes.get_mut(note).filter(|note| !note.is_deleted())
    }

    /// The channel's notes that start in the step, as their places in
    /// [`Pattern::notes`].
    fn in_step(&self, channel: u16, step: u32, ppq: u16) -> impl Iterator<Item = usize> + '_ {
        let ticks = step_ticks(ppq);
        let from = i64::from(step) * ticks;
        let notes = self.notes.iter().enumerate();
        notes.filter_map(move |(at, note)| {
            let here = (from..from + ticks).contains(&i64::from(note.position));
            (here && note.channel == channel && !note.is_deleted()).then_some(at)
        })
    }

    /// Whether [`to_flp`](crate::to_flp) writes the notes, after giving a
    /// pattern that has no 224 one behind its first 65.
    ///
    /// A 224 that stayed a raw event says no. A second 224 beside it would
    /// leave the loader with two lists, and the notes of the raw one are
    /// not in [`Pattern::notes`] to keep. A pattern without a 65 in its
    /// `order` was not read from a file, and has no place for a 224.
    pub(crate) fn writes_notes(&mut self) -> bool {
        let notes = Id::PatternNotes;
        let has = |item: &Item| match item {
            Item::Field(id) => (*id == notes).then_some(true),
            Item::Raw(e) => (e.id == notes.0).then_some(false),
        };
        if let Some(typed) = self.order.iter().find_map(|placed| has(&placed.item)) {
            return typed;
        }
        add_after(&mut self.order, Id::PatternNew, notes);
        true
    }
}

/// The piano roll's edits. A note goes by its place in [`Pattern::notes`],
/// and a place that has no note, or a record FL deleted, is skipped.
///
/// The list stays in the order of the notes' positions, as every 224 of
/// the corpus is, so adding a note or moving notes along the pattern gives
/// the notes behind them other places. Every other edit leaves each note
/// where it is in the list.
impl Pattern {
    /// The highest key: B10 as FL counts octaves. The lowest is 0, C0.
    pub const TOP_KEY: i16 = 131;

    /// Add a note, in front of the notes that start where it does, which
    /// is where [`Pattern::set_step`] puts a step. Its position and its
    /// length are 0 at least and its key is one the piano roll has.
    ///
    /// A pattern whose 224 the model could not read is left as it is.
    pub fn add_note(&mut self, note: layout::Note) {
        if !self.writes_notes() {
            return;
        }
        let note = layout::Note {
            position: note.position.max(0),
            length: note.length.max(0),
            key: note.key.clamp(0, Self::TOP_KEY),
            ..note
        };
        let at = self.notes.partition_point(|n| n.position < note.position);
        self.notes.insert(at, note);
    }

    /// Make each record what FL leaves of a deleted note: all ones where
    /// the tag and the channel are. The record stays in the list, so the
    /// other notes keep their places, and [`to_flp`](crate::to_flp) leaves
    /// it out of the file.
    pub fn delete_notes(&mut self, notes: &[usize]) {
        self.each_note(notes, |note| {
            (note.tag, note.channel) = (u16::MAX, u16::MAX);
        });
    }

    /// Move the notes along the pattern and `delta_keys` keys up. They
    /// move as a group, so the group stops where its first note reaches
    /// tick 0, and where its lowest note reaches key 0 or its highest
    /// [`Pattern::TOP_KEY`].
    pub fn move_notes(&mut self, notes: &[usize], delta_ticks: i64, delta_keys: i32) {
        let (mut first, mut last) = (i64::MAX, i64::MIN);
        let (mut low, mut high) = (i32::MAX, i32::MIN);
        self.each_note(notes, |note| {
            first = first.min(i64::from(note.position));
            last = last.max(i64::from(note.position));
            low = low.min(i32::from(note.key));
            high = high.max(i32::from(note.key));
        });
        if first > last {
            return;
        }
        let ticks = delta_ticks
            .min(i64::from(i32::MAX) - last)
            .max(-first.max(0));
        let top = i32::from(Self::TOP_KEY);
        let keys = delta_keys.min((top - high).max(0)).max(-low.max(0));
        self.each_note(notes, |note| {
            note.position = (i64::from(note.position) + ticks) as i32;
            note.key = (i32::from(note.key) + keys) as i16;
        });
        if ticks != 0 {
            // The sort keeps notes that start together in the order they
            // had.
            self.notes.sort_by_key(|note| note.position);
        }
    }

    /// Make each note `delta_ticks` longer by moving its end, and shorter
    /// when that is negative. A note keeps a tick at least. A step, which
    /// has no length, gets one this way.
    pub fn resize_notes(&mut self, notes: &[usize], delta_ticks: i64) {
        if delta_ticks == 0 {
            return;
        }
        self.each_note(notes, |note| {
            let most = i64::from(i32::MAX) - i64::from(note.position.max(0));
            let length = i64::from(note.length).saturating_add(delta_ticks);
            note.length = length.clamp(1, most.max(1)) as i32;
        });
    }

    pub fn set_notes_muted(&mut self, notes: &[usize], muted: bool) {
        self.each_note(notes, |note| set_flag(note, layout::Note::MUTED, muted));
    }

    /// Set a value of the note at this place, [`NoteField::top`] at most.
    pub fn set_note_field(&mut self, note: usize, field: NoteField, value: u8) {
        if let Some(note) = self.placed_note(note) {
            *field.of_mut(note) = value.min(field.top());
        }
    }

    /// Select the notes, on top of the selection when `add` is set and in
    /// its place when not. The selection is the whole pattern's, so the
    /// notes of every channel let go.
    pub fn select_notes(&mut self, notes: &[usize], add: bool) {
        if !add {
            let placed = self.notes.iter_mut().filter(|note| !note.is_deleted());
            placed.for_each(|note| set_flag(note, layout::Note::SELECTED, false));
        }
        self.each_note(notes, |note| {
            set_flag(note, layout::Note::SELECTED, true);
        });
    }

    /// Change each note `notes` names, once.
    fn each_note(&mut self, notes: &[usize], mut change: impl FnMut(&mut layout::Note)) {
        let mut notes = notes.to_vec();
        notes.sort_unstable();
        notes.dedup();
        for note in notes {
            if let Some(note) = self.placed_note(note) {
                change(note);
            }
        }
    }
}

fn set_flag(note: &mut layout::Note, bit: u8, on: bool) {
    note.flags = note.flags & !bit | if on { bit } else { 0 };
}

/// Give an object the event `id` to write from its field: behind the first
/// `opening` event in `order` and at that event's position, which
/// [`to_flp`](crate::to_flp) keeps in this order. Without an `opening`
/// event the object was not read from a file and nothing is added.
fn add_after(order: &mut Vec<Placed>, opening: Id, id: Id) {
    let opens = |placed: &Placed| placed.item == Item::Field(opening);
    if let Some(at) = order.iter().position(opens) {
        let pos = order[at].pos;
        let item = Item::Field(id);
        order.insert(at + 1, Placed { pos, item });
    }
}

/// Make `id` an event the object writes from its field. An event of that
/// ID which stayed raw becomes the field's, so that the object does not
/// end up with two. Otherwise see [`add_after`].
fn write_field(order: &mut Vec<Placed>, opening: Id, id: Id) {
    if order.iter().any(|placed| placed.item == Item::Field(id)) {
        return;
    }
    let is_raw = |placed: &&mut Placed| matches!(&placed.item, Item::Raw(e) if e.id == id.0);
    match order.iter_mut().rev().find(is_raw) {
        Some(placed) => placed.item = Item::Field(id),
        None => add_after(order, opening, id),
    }
}

/// The channel rack's edits. Each one sets a field and sees to it that the
/// field has an event to be written in, see "Changing a project" in the
/// crate's docs. A channel index that names no channel is skipped.
impl Project {
    /// The pattern with this index, to change. A pattern the file has no
    /// event for is made here.
    ///
    /// A new pattern's 65 and 224 go right in front of the first 65 of the
    /// file, where FL writes the notes of its patterns, and the pattern
    /// goes in front of the others, which is where reading the file back
    /// puts it. No event after the new two changes its owner that way,
    /// because a 65 follows them. In a project without a pattern they go
    /// behind the last of the project's own events before the first
    /// channel: 134 of the 135 corpus projects that have patterns and
    /// channels have their first 65 in front of their first 64.
    pub fn pattern_to_edit(&mut self, index: u16) -> &mut Pattern {
        let found = self.patterns.iter().position(|p| p.index == index);
        let at = found.unwrap_or_else(|| {
            let opens = |placed: &&Placed| placed.item == Item::Field(Id::PatternNew);
            let first = |pattern: &Pattern| pattern.order.iter().find(opens).map(|p| p.pos);
            let pos = match self.patterns.iter().filter_map(first).min() {
                Some(pos) => Some(pos),
                None => {
                    let channels = self.channels.iter().flat_map(|c| c.order.first());
                    let channels = channels.map(|placed| placed.pos).min();
                    let before = |placed: &&Placed| channels.is_none_or(|c| placed.pos < c);
                    let own = self.order.iter().filter(before);
                    own.map(|placed| placed.pos).max()
                }
            };
            let event = |id: Id| {
                pos.map(|pos| Placed {
                    pos,
                    item: Item::Field(id),
                })
            };
            let order = [event(Id::PatternNew), event(Id::PatternNotes)];
            let pattern = Pattern {
                index,
                order: order.into_iter().flatten().collect(),
                ..Pattern::default()
            };
            self.patterns.insert(0, pattern);
            0
        });
        &mut self.patterns[at]
    }

    /// 0 `ChannelID.IsEnabled`: the LED beside the channel.
    pub fn set_channel_enabled(&mut self, channel: u16, enabled: bool) {
        if let Some(channel) = self.channel_to_edit(channel) {
            channel.enabled = Some(enabled);
            write_field(&mut channel.order, Id::ChannelNew, Id::IsEnabled);
        }
    }

    /// The volume in 219, from 0 to 12800, where a new channel has 10000.
    /// An automation clip is skipped: it keeps its range there.
    pub fn set_channel_volume(&mut self, channel: u16, volume: u32) {
        if let Some(levels) = self.levels_to_edit(channel) {
            levels.volume = volume.min(LEVEL_TOP);
        }
    }

    /// The pan in 219, from 0 on the left to 12800 on the right, with 6400
    /// in the centre. An automation clip is skipped.
    pub fn set_channel_pan(&mut self, channel: u16, pan: u32) {
        if let Some(levels) = self.levels_to_edit(channel) {
            levels.pan = pan.min(LEVEL_TOP);
        }
    }

    /// The insert the channel plays into, by the loader's number, see
    /// [`Channel::insert`](crate::Channel::insert).
    ///
    /// It goes to 104 or to 22, whichever the channel has, and for a
    /// channel with neither to 104 when the project has a 103: in the
    /// corpus the two always come together. 104 takes 0 to 501 and 22
    /// takes 0 to 127.
    pub fn set_channel_insert(&mut self, channel: u16, insert: u16) {
        let wide = self.insert_count.is_some();
        let Some(channel) = self.channel_to_edit(channel) else {
            return;
        };
        if channel.target_insert.is_some() || (wide && channel.routed_to.is_none()) {
            channel.target_insert = Some(insert.min(501));
            write_field(&mut channel.order, Id::ChannelNew, Id::FLP_TargetFXTrack16);
        } else {
            channel.routed_to = Some(insert.min(127) as i8);
            write_field(&mut channel.order, Id::ChannelNew, Id::RoutedTo);
        }
    }

    /// 196 `ChannelID.SamplePath`: the sample a sampler or an audio clip
    /// plays, as the path the file stores. A channel of another kind is
    /// skipped, because its 196 is its plugin's.
    ///
    /// A channel without a 196 gets one behind its 20, which is where every
    /// corpus channel with a sample has it, and behind its 64 when it has
    /// no 20.
    pub fn set_channel_sample(&mut self, channel: u16, path: &str) {
        let Some(channel) = self.channel_to_edit(channel) else {
            return;
        };
        let plays = matches!(
            channel.kind,
            Some(ChannelKind::Sampler | ChannelKind::AudioClip)
        );
        if !plays {
            return;
        }
        channel.sample_path = Some(path.to_owned());
        let order = &mut channel.order;
        let is = |placed: &Placed, id: Id| match &placed.item {
            Item::Field(field) => *field == id,
            Item::Raw(e) => e.id == id.0,
        };
        let has = order.iter().any(|placed| is(placed, Id::SamplePath));
        let behind = order
            .iter()
            .rposition(|placed| is(placed, Id::PingPongLoop));
        match behind.filter(|_| !has) {
            Some(at) => {
                let (pos, item) = (order[at].pos, Item::Field(Id::SamplePath));
                order.insert(at + 1, Placed { pos, item });
            }
            None => write_field(order, Id::ChannelNew, Id::SamplePath),
        }
    }

    /// Add a sampler channel that plays `sample`, a path as a 196 stores
    /// it, and give the channel's index. The channel goes to the end of
    /// the rack with the name of the file, less its extension.
    ///
    /// It gets the events FL writes for a new sampler: those of the one
    /// channel in FL's own `Empty.flp`, and a 196. They go behind the last
    /// event that any channel has in the file, so that every event after
    /// them keeps its owner, and the header counts a channel more.
    ///
    /// A project whose channels have no events was not read from a file
    /// and has no place for the new ones, so it gets no channel and this
    /// gives `None`.
    pub fn add_sampler_channel(&mut self, sample: &str) -> Option<u16> {
        let placed = self.channels.iter().flat_map(|channel| &channel.order);
        let pos = placed.map(|placed| placed.pos).max()?;
        let last = self.channels.iter().map(|channel| channel.index).max()?;
        let index = last.checked_add(1)?;
        let file = sample.rsplit(['/', '\\']).next().unwrap_or(sample);
        let stem = file.rsplit_once('.').map(|(stem, _)| stem);
        let name = stem.filter(|stem| !stem.is_empty()).unwrap_or(file);
        let wide = self.insert_count.is_some();
        let channel = Channel::new_sampler(index, name, sample, wide, pos);
        self.channels.push(channel);
        self.header.channel_count = self.header.channel_count.saturating_add(1);
        Some(index)
    }

    fn channel_to_edit(&mut self, index: u16) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.index == index)
    }

    /// The channel's 219, made as FL makes it for a new channel when the
    /// channel has none: volume 10000, pan in the centre and 256 for mod
    /// X, which is what 2136 of the corpus channels have behind the two.
    fn levels_to_edit(&mut self, channel: u16) -> Option<&mut layout::Levels> {
        let channel = self.channel_to_edit(channel)?;
        if channel.kind == Some(ChannelKind::Automation) {
            return None;
        }
        write_field(&mut channel.order, Id::ChannelNew, Id::Levels);
        Some(channel.levels.get_or_insert(layout::Levels {
            pan: LEVEL_TOP / 2,
            volume: 10_000,
            unknown_8: [0; 4],
            mod_x: 256,
            mod_y: 0,
            unknown_20: [0; 4],
            len: 24,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_flp, to_flp};
    use flp::RawEvent;

    const PPQ: u16 = 96;

    fn ev(id: Id, data: &[u8]) -> RawEvent {
        RawEvent {
            id: id.0,
            data: data.to_vec(),
        }
    }

    fn word(id: Id, value: u16) -> RawEvent {
        ev(id, &value.to_le_bytes())
    }

    /// A string as files from 11.5.5 on store it.
    fn text(id: Id, text: &str) -> RawEvent {
        let units = text.encode_utf16().chain([0]);
        ev(id, &units.flat_map(u16::to_le_bytes).collect::<Vec<u8>>())
    }

    fn note(channel: u16, position: i32, length: i32) -> layout::Note {
        layout::Note {
            position,
            channel,
            length,
            key: STEP_KEY,
            ..layout::Note::default()
        }
    }

    fn notes(notes: &[layout::Note]) -> RawEvent {
        ev(Id::PatternNotes, &layout::Note::list_to_bytes(notes))
    }

    fn levels(pan: u32, volume: u32) -> layout::Levels {
        let mut bytes = [pan, volume, 0, 256, 0, 0].map(u32::to_le_bytes).concat();
        bytes.truncate(24);
        layout::Levels::from_bytes(&bytes).unwrap()
    }

    /// Pattern 1 with a kick on every beat and a clap on the second, an
    /// empty pattern 2 that has a name, and three channels: a sampler
    /// with every event the rack sets, one with nothing but its 64, and
    /// an automation clip.
    fn file() -> flp::Project {
        let kicks: Vec<_> = (0..4).map(|beat| note(0, beat * 96, 0)).collect();
        let mut pattern = kicks.clone();
        pattern.insert(2, note(1, 96, 0));
        let events = vec![
            ev(Id::FLVersion, b"20.8.4.2576\0"),
            word(Id::PatternNew, 1),
            notes(&pattern),
            word(Id::ChannelNew, 0),
            ev(Id::ChannelType, &[0]),
            ev(Id::IsEnabled, &[1]),
            ev(Id::Levels, &levels(6400, 10_000).to_bytes()),
            ev(Id::RoutedTo, &[3]),
            word(Id::ChannelNew, 1),
            word(Id::ChannelNew, 2),
            ev(Id::ChannelType, &[5]),
            ev(Id::Levels, &levels(0, 1).to_bytes()),
            word(Id::PatternNew, 2),
            text(Id::PatternName, "Fill"),
            word(Id::PatternNew, 1),
            text(Id::PatternName, "Beat"),
        ];
        let header = flp::Header {
            format: 0,
            channel_count: 3,
            ppq: PPQ,
        };
        flp::Project { header, events }
    }

    fn project() -> Project {
        let project = from_flp(&file());
        assert_eq!(to_flp(&project), Ok(file()));
        project
    }

    /// The events of `project` that are not in [`file`] as they are there,
    /// with the place of each, and the events of the file that are gone.
    fn changes(project: &Project) -> (Vec<(usize, RawEvent)>, Vec<RawEvent>) {
        let written = to_flp(project).unwrap();
        let mut left = file().events;
        let mut new = Vec::new();
        for (at, event) in written.events.into_iter().enumerate() {
            match left.iter().position(|was| *was == event) {
                Some(found) => drop(left.remove(found)),
                None => new.push((at, event)),
            }
        }
        (new, left)
    }

    fn lit(pattern: &Pattern, channel: u16) -> Vec<usize> {
        let steps = pattern.steps(channel, PPQ).into_iter().enumerate();
        steps
            .filter_map(|(step, lit)| lit.then_some(step))
            .collect()
    }

    #[test]
    fn steps_go_into_the_notes_and_come_back_out() {
        let mut pattern = Pattern::default();
        assert!(pattern.steps(0, PPQ).is_empty());
        for step in [8, 0, 12, 4] {
            pattern.set_step(0, step, true, PPQ);
        }
        pattern.set_step(1, 4, true, PPQ);
        // Lighting a lit step again adds nothing.
        pattern.set_step(0, 4, true, PPQ);
        assert_eq!(lit(&pattern, 0), [0, 4, 8, 12]);
        assert_eq!(lit(&pattern, 1), [4]);
        // Every channel has the steps up to the pattern's last note.
        assert_eq!(pattern.steps(1, PPQ).len(), 13);
        assert_eq!(pattern.steps(7, PPQ), [false; 13]);

        // A step is FL's new note at the step's first tick, with no length
        // and key 60, and the notes stay in the order of their positions.
        let places: Vec<_> = pattern
            .notes
            .iter()
            .map(|n| (n.position, n.channel))
            .collect();
        assert_eq!(places, [(0, 0), (96, 1), (96, 0), (192, 0), (288, 0)]);
        let step = layout::Note {
            position: 96,
            channel: 1,
            key: 60,
            ..layout::Note::default()
        };
        assert_eq!(pattern.notes[1], step);
        assert_eq!((step.length, step.velocity, step.pan), (0, 100, 64));

        pattern.toggle_step(0, 4, PPQ);
        pattern.toggle_step(0, 5, PPQ);
        pattern.set_step(0, 9, false, PPQ);
        assert_eq!(lit(&pattern, 0), [0, 5, 8, 12]);
        assert_eq!(lit(&pattern, 1), [4]);
        // A step at PPQ 48 is 12 ticks.
        assert_eq!(pattern.steps(0, 48).len(), 25);
    }

    #[test]
    fn a_step_is_whatever_starts_in_its_ticks() {
        let mut muted = note(0, 48, 0);
        muted.flags |= layout::Note::MUTED;
        let mut deleted = note(0, 72, 0);
        (deleted.tag, deleted.channel) = (u16::MAX, u16::MAX);
        // A note off the grid, a long one, a muted one, a deleted record
        // and a note before the pattern's start.
        let mut pattern = Pattern {
            notes: vec![
                note(0, -24, 0),
                note(0, 30, 0),
                muted,
                deleted,
                note(0, 100, 384),
                note(1, 100, 0),
            ],
            ..Pattern::default()
        };
        assert_eq!(lit(&pattern, 0), [1, 4]);
        assert_eq!(pattern.step_note(0, 1, PPQ), Some(1));
        assert_eq!(pattern.step_note(0, 2, PPQ), None);

        // The muted note leaves its step dark, so lighting it adds a
        // note, and clearing it takes both.
        pattern.set_step(0, 2, true, PPQ);
        assert_eq!(pattern.notes.len(), 7);
        assert_eq!(pattern.step_note(0, 2, PPQ), Some(2));
        pattern.set_step(0, 2, false, PPQ);
        pattern.set_step(0, 4, false, PPQ);
        let places: Vec<_> = pattern
            .notes
            .iter()
            .map(|n| (n.position, n.channel))
            .collect();
        assert_eq!(places, [(-24, 0), (30, 0), (72, u16::MAX), (100, 1)]);

        // A velocity and a pan stop at 128, and a deleted record has
        // neither to set.
        pattern.set_note_velocity(1, 200);
        pattern.set_note_pan(1, 0);
        pattern.set_note_velocity(2, 5);
        pattern.set_note_pan(9, 5);
        let note = pattern.notes[1];
        assert_eq!((note.velocity, note.pan), (128, 0));
        assert_eq!(pattern.notes[2].velocity, 100);
    }

    #[test]
    fn a_toggled_step_changes_one_224_and_nothing_else() {
        let mut project = project();
        project.pattern_to_edit(1).toggle_step(1, 6, PPQ);
        let (new, gone) = changes(&project);
        let [(2, new)] = &new[..] else {
            panic!("{new:?}");
        };
        let [gone] = &gone[..] else {
            panic!("{gone:?}");
        };
        assert_eq!((new.id, gone.id), (Id::PatternNotes.0, Id::PatternNotes.0));
        // The list has one record more, and the others are as they were.
        let was = layout::Note::list_from_bytes(&gone.data).unwrap();
        let mut now = layout::Note::list_from_bytes(&new.data).unwrap();
        assert_eq!(now.remove(3), note(1, 144, 0));
        assert_eq!(now, was);

        project.pattern_to_edit(1).toggle_step(1, 6, PPQ);
        assert_eq!(to_flp(&project), Ok(file()));
    }

    #[test]
    fn a_pattern_without_notes_gets_a_224_behind_its_65() {
        let mut project = project();
        project.pattern_to_edit(2).set_step(0, 3, true, PPQ);
        assert_eq!(project.patterns.len(), 2);
        let (new, gone) = changes(&project);
        assert_eq!(new, [(13, notes(&[note(0, 72, 0)]))]);
        assert!(gone.is_empty());
        let read = from_flp(&to_flp(&project).unwrap());
        assert_eq!(read.pattern(2).unwrap().notes, [note(0, 72, 0)]);
        assert_eq!(read.pattern(2).unwrap().name.as_deref(), Some("Fill"));
    }

    #[test]
    fn a_new_pattern_goes_in_front_of_the_first_65() {
        let mut project = project();
        project.pattern_to_edit(5).set_step(1, 0, true, PPQ);
        let (new, gone) = changes(&project);
        assert_eq!(
            new,
            [(1, word(Id::PatternNew, 5)), (2, notes(&[note(1, 0, 0)]))]
        );
        assert!(gone.is_empty());
        // Read back, the patterns are in the order of their first 65, and
        // no other event has moved to the new one.
        let read = from_flp(&to_flp(&project).unwrap());
        let indices: Vec<u16> = read.patterns.iter().map(|p| p.index).collect();
        assert_eq!(indices, [5, 1, 2]);
        assert_eq!(
            indices,
            project.patterns.iter().map(|p| p.index).collect::<Vec<_>>()
        );
        assert_eq!(read.patterns[0].order.len(), 2);
        assert_eq!(read.pattern(1).unwrap().name.as_deref(), Some("Beat"));

        // Without a pattern the two go in front of the first channel.
        let mut file = file();
        file.events.retain(|e| ![65, 224, 193].contains(&e.id));
        let mut project = from_flp(&file);
        project.pattern_to_edit(1).set_step(0, 0, true, PPQ);
        let written = to_flp(&project).unwrap().events;
        assert_eq!(written.len(), file.events.len() + 2);
        assert_eq!(
            written[1..4],
            [
                word(Id::PatternNew, 1),
                notes(&[note(0, 0, 0)]),
                word(Id::ChannelNew, 0)
            ]
        );
    }

    #[test]
    fn a_224_the_model_could_not_read_keeps_its_pattern_as_it_is() {
        let mut file = file();
        // A record and a half.
        file.events[2].data.truncate(36);
        let mut project = from_flp(&file);
        assert!(project.patterns[0].notes.is_empty());
        project.pattern_to_edit(1).set_step(0, 0, true, PPQ);
        assert_eq!(to_flp(&project), Ok(file));
    }

    #[test]
    fn a_channel_edit_changes_the_event_of_its_field() {
        let mut project = project();
        project.set_channel_enabled(0, false);
        project.set_channel_volume(0, 7000);
        project.set_channel_pan(0, 99_999);
        project.set_channel_insert(0, 300);
        // A channel that is not there, and an automation clip, whose 219
        // is its range.
        project.set_channel_enabled(9, false);
        project.set_channel_volume(2, 7000);
        project.set_channel_pan(2, 7000);
        let (new, gone) = changes(&project);
        assert_eq!(
            new,
            [
                (5, ev(Id::IsEnabled, &[0])),
                (6, ev(Id::Levels, &levels(12_800, 7000).to_bytes())),
                // A 22 holds 127 at most.
                (7, ev(Id::RoutedTo, &[127])),
            ]
        );
        assert_eq!(gone.len(), 3);
        assert_eq!(project.channel(0).unwrap().insert(), Some(127));
    }

    #[test]
    fn a_channel_without_the_event_gets_one_behind_its_64() {
        let mut project = project();
        project.set_channel_insert(1, 4);
        project.set_channel_pan(1, 3200);
        project.set_channel_enabled(1, false);
        let (new, gone) = changes(&project);
        assert_eq!(
            new,
            [
                (9, ev(Id::IsEnabled, &[0])),
                (10, ev(Id::Levels, &levels(3200, 10_000).to_bytes())),
                (11, ev(Id::RoutedTo, &[4])),
            ]
        );
        assert!(gone.is_empty());
        let read = from_flp(&to_flp(&project).unwrap());
        let channel = read.channel(1).unwrap();
        assert_eq!(channel.enabled, Some(false));
        assert_eq!(
            (channel.pan(), channel.volume()),
            (Some(3200), Some(10_000))
        );
        assert_eq!(channel.insert(), Some(4));
        assert_eq!(read.channel(2).unwrap().order.len(), 3);

        // With a 103 the project counts its inserts in 16 bits, and the
        // channel's target is a 104.
        let mut project = self::project();
        project.insert_count = Some(127);
        project.set_channel_insert(1, 300);
        assert_eq!(project.channel(1).unwrap().target_insert, Some(300));
        let (new, _) = changes(&project);
        assert_eq!(new, [(9, word(Id::FLP_TargetFXTrack16, 300))]);

        // An event that stayed raw becomes the field's: a bool of 2 here.
        let mut file = file();
        file.events[5].data = vec![2];
        let mut project = from_flp(&file);
        assert_eq!(project.channel(0).unwrap().enabled, None);
        project.set_channel_enabled(0, true);
        file.events[5].data = vec![1];
        assert_eq!(to_flp(&project), Ok(file));
    }

    #[test]
    fn a_sample_goes_into_the_196_of_a_channel_that_plays_one() {
        let kick = r"%FLStudioFactoryData%\Data\Patches\Packs\Drums\Kick.wav";
        let mut project = project();
        project.set_channel_sample(0, kick);
        // A channel without a type, an automation clip and a channel that
        // is not there take no sample.
        for channel in [1, 2, 9] {
            project.set_channel_sample(channel, kick);
        }
        // The sampler has no 20, so its 196 goes behind its 64.
        let (new, gone) = changes(&project);
        assert_eq!(new, [(4, text(Id::SamplePath, kick))]);
        assert!(gone.is_empty());
        let read = from_flp(&to_flp(&project).unwrap());
        assert_eq!(read.channel(0).unwrap().sample_path.as_deref(), Some(kick));
        assert_eq!(read.channel(2).unwrap().sample_path, None);

        // Another sample takes the event the first one has.
        project.set_channel_sample(0, "Snare.wav");
        let (new, gone) = changes(&project);
        assert_eq!(new, [(4, text(Id::SamplePath, "Snare.wav"))]);
        assert!(gone.is_empty());

        // With a 20 the 196 goes behind it, as FL writes the two.
        let mut file = file();
        file.events.insert(8, ev(Id::PingPongLoop, &[0]));
        let mut project = from_flp(&file);
        project.set_channel_sample(0, kick);
        file.events.insert(9, text(Id::SamplePath, kick));
        assert_eq!(to_flp(&project), Ok(file));
    }

    #[test]
    fn a_new_sampler_channel_goes_behind_the_last_channel() {
        let kick = r"%FLStudioFactoryData%\Data\Patches\Packs\Drums\Kick 01.wav";
        let mut project = project();
        assert_eq!(project.add_sampler_channel(kick), Some(3));
        assert_eq!(project.header.channel_count, 4);

        // The channel's events are behind the 219 of the automation clip,
        // and every event the file had is as it was, in its order.
        let written = to_flp(&project).unwrap();
        let mut events = written.events.clone();
        let new: Vec<RawEvent> = events.drain(12..12 + 45).collect();
        assert_eq!(events, file().events);
        // FL's own set for a sampler. The file has no 103, so the insert
        // is in a 22.
        let ids: Vec<u32> = new.iter().map(|event| event.id).collect();
        assert_eq!(
            ids,
            [
                64, 21, 201, 212, 203, 155, 128, 41, 0, 209, 138, 139, 89, 97, 48, 69, 86, 71, 83,
                74, 75, 76, 85, 131, 70, 22, 50, 219, 229, 221, 215, 132, 144, 145, 32, 228, 228,
                218, 218, 218, 218, 218, 143, 20, 196
            ]
        );
        assert_eq!(new[0], word(Id::ChannelNew, 3));
        assert_eq!(new[2], text(Id::PluginInternalName, ""));
        assert_eq!(new[4], text(Id::PluginName, "Kick 01"));
        assert_eq!(new[44], text(Id::SamplePath, kick));

        // Read back, the channel is the one that was added, and the other
        // three are the file's.
        let read = from_flp(&written);
        assert_eq!(read.header.channel_count, 4);
        assert_eq!(read.channels[..3], from_flp(&file()).channels[..]);
        let channel = read.channel(3).unwrap();
        // It reads as it was made, but for where its events are.
        let unplaced = |channel: &Channel| {
            let mut channel = channel.clone();
            channel.order.iter_mut().for_each(|placed| placed.pos = 0);
            channel
        };
        assert!(unplaced(channel) == unplaced(&project.channels[3]));
        assert_eq!(channel.kind, Some(ChannelKind::Sampler));
        assert_eq!(channel.name.as_deref(), Some("Kick 01"));
        assert_eq!(channel.sample_path.as_deref(), Some(kick));
        assert_eq!(channel.enabled, Some(true));
        assert_eq!(
            (channel.volume(), channel.pan(), channel.insert()),
            (Some(10_000), Some(6400), Some(0))
        );
        assert_eq!((channel.envelopes.len(), channel.tracking.len()), (5, 2));
        assert_eq!(read.pattern(1).unwrap().name.as_deref(), Some("Beat"));

        // A second one follows the first, and with a 103 its insert is in
        // a 104.
        project.insert_count = Some(127);
        assert_eq!(project.add_sampler_channel("Loops/Break.wav"), Some(4));
        let read = from_flp(&to_flp(&project).unwrap());
        let indices: Vec<u16> = read.channels.iter().map(|c| c.index).collect();
        assert_eq!(indices, [0, 1, 2, 3, 4]);
        let channel = read.channel(4).unwrap();
        assert_eq!(channel.name.as_deref(), Some("Break"));
        assert_eq!((channel.target_insert, channel.routed_to), (Some(0), None));

        // A project that was not read from a file has no place for the
        // channel's events.
        let mut built = Project::new(file().header);
        built.channels.push(Channel::default());
        assert_eq!(built.add_sampler_channel(kick), None);
        assert_eq!(built.channels.len(), 1);
    }

    #[test]
    fn a_project_built_in_code_takes_the_edits_without_events() {
        let mut project = Project::new(file().header);
        project.channels.push(Channel::default());
        project.pattern_to_edit(1).set_step(0, 2, true, PPQ);
        project.set_channel_enabled(0, false);
        project.set_channel_volume(0, 5000);
        assert_eq!(lit(project.pattern(1).unwrap(), 0), [2]);
        let channel = &project.channels[0];
        assert_eq!(
            (channel.enabled, channel.volume()),
            (Some(false), Some(5000))
        );
        assert!(to_flp(&project).unwrap().events.is_empty());
    }

    /// Where each note is and whose it is: position, key and channel.
    fn placed(pattern: &Pattern) -> Vec<(i32, i16, u16)> {
        let place = |n: &layout::Note| (n.position, n.key, n.channel);
        pattern.notes.iter().map(place).collect()
    }

    #[test]
    fn an_unedited_project_writes_the_bytes_it_was_read_from() {
        let bytes = file().write();
        let project = from_flp(&flp::Project::parse(&bytes).unwrap());
        assert_eq!(to_flp(&project).unwrap().write(), bytes);

        // Edits that change nothing leave the bytes too: a place without a
        // note, a move and a resize by nothing, and values that are set
        // already.
        let mut edited = project.clone();
        let pattern = edited.pattern_to_edit(1);
        pattern.delete_notes(&[9]);
        pattern.move_notes(&[0, 1, 9], 0, 0);
        pattern.resize_notes(&[0], 0);
        pattern.set_notes_muted(&[0], false);
        pattern.set_note_field(0, NoteField::Velocity, 100);
        pattern.set_note_field(9, NoteField::Pan, 0);
        pattern.select_notes(&[], false);
        assert_eq!(edited, project);
        assert_eq!(to_flp(&edited).unwrap().write(), bytes);
    }

    #[test]
    fn a_note_goes_in_where_its_position_is() {
        let mut project = project();
        let pattern = project.pattern_to_edit(1);
        let long = layout::Note {
            key: 67,
            ..note(1, 96, 48)
        };
        pattern.add_note(long);
        // In front of the kick and the clap that start there.
        assert_eq!(pattern.notes[1], long);
        assert_eq!(
            placed(pattern),
            [
                (0, 60, 0),
                (96, 67, 1),
                (96, 60, 0),
                (96, 60, 1),
                (192, 60, 0),
                (288, 60, 0)
            ]
        );

        // The 224 has one record more and nothing else has changed.
        let (new, gone) = changes(&project);
        let ([(2, new)], [gone]) = (&new[..], &gone[..]) else {
            panic!("{new:?} {gone:?}");
        };
        let was = layout::Note::list_from_bytes(&gone.data).unwrap();
        let mut now = layout::Note::list_from_bytes(&new.data).unwrap();
        assert_eq!(now.remove(1), long);
        assert_eq!(now, was);
        let read = from_flp(&to_flp(&project).unwrap());
        assert_eq!(read.pattern(1).unwrap().notes, project.patterns[0].notes);

        // A note from before the pattern's start, with a length below
        // nothing and a key above the last one, is put where a note can be.
        let pattern = project.pattern_to_edit(1);
        pattern.add_note(layout::Note {
            key: 500,
            ..note(0, -5, -9)
        });
        let first = pattern.notes[0];
        assert_eq!((first.position, first.length, first.key), (0, 0, 131));
        assert_eq!(pattern.notes.len(), 7);

        // A pattern without a 224 gets one, and a 224 that stayed raw
        // keeps its pattern as it is.
        project.pattern_to_edit(2).add_note(long);
        let read = from_flp(&to_flp(&project).unwrap());
        assert_eq!(read.pattern(2).unwrap().notes, [long]);
        let mut file = file();
        file.events[2].data.truncate(36);
        let mut project = from_flp(&file);
        project.pattern_to_edit(1).add_note(long);
        assert_eq!(to_flp(&project), Ok(file));
    }

    #[test]
    fn a_deleted_note_keeps_its_record_and_the_others_their_places() {
        let mut project = project();
        let pattern = project.pattern_to_edit(1);
        let before = pattern.notes.clone();
        // The kick on the second beat, named twice, and no note at all.
        pattern.delete_notes(&[1, 1, 9]);
        assert!(pattern.notes[1].is_deleted());
        assert_eq!(pattern.notes.len(), 5);
        assert_eq!(pattern.notes[2..], before[2..]);
        assert_eq!(pattern.notes[0], before[0]);
        assert_eq!(lit(pattern, 0), [0, 8, 12]);
        assert_eq!(lit(pattern, 1), [4]);

        // No edit finds the record again.
        let deleted = pattern.notes.clone();
        pattern.move_notes(&[1], 96, 1);
        pattern.resize_notes(&[1], 96);
        pattern.set_notes_muted(&[1], true);
        pattern.set_note_field(1, NoteField::Velocity, 1);
        pattern.select_notes(&[1], true);
        pattern.delete_notes(&[1]);
        assert_eq!(pattern.notes, deleted);

        // The file gets the notes without the record, as FL compacts them:
        // the 224 is 24 bytes shorter and no other event has changed.
        let (new, gone) = changes(&project);
        let ([(2, new)], [gone]) = (&new[..], &gone[..]) else {
            panic!("{new:?} {gone:?}");
        };
        assert_eq!(new.data.len() + 24, gone.data.len());
        let mut kept = before.clone();
        kept.remove(1);
        assert_eq!(new.data, layout::Note::list_to_bytes(&kept));
        let read = from_flp(&to_flp(&project).unwrap());
        assert_eq!(read.pattern(1).unwrap().notes, kept);
        // The open project still has the record where it was.
        assert_eq!(project.patterns[0].notes, deleted);
    }

    #[test]
    fn notes_move_as_a_group_and_stay_in_the_order_of_their_positions() {
        let mut project = project();
        let pattern = project.pattern_to_edit(1);
        // The kick and the clap on the second beat go past the third.
        pattern.move_notes(&[1, 2], 120, 3);
        assert_eq!(
            placed(pattern),
            [
                (0, 60, 0),
                (192, 60, 0),
                (216, 63, 0),
                (216, 63, 1),
                (288, 60, 0)
            ]
        );
        // And back, to the places and the bytes they had.
        pattern.move_notes(&[2, 3], -120, -3);
        assert_eq!(to_flp(&project), Ok(file()));

        // The group stops where its first note reaches the start, and a
        // note that gets there goes behind the one that is there.
        let pattern = project.pattern_to_edit(1);
        pattern.move_notes(&[0, 3], -500, 0);
        assert_eq!(to_flp(&project), Ok(file()));
        let pattern = project.pattern_to_edit(1);
        pattern.move_notes(&[2, 3], -500, 0);
        assert_eq!(
            placed(pattern),
            [
                (0, 60, 0),
                (0, 60, 1),
                (96, 60, 0),
                (96, 60, 0),
                (288, 60, 0)
            ]
        );

        // It stops at the last key and at the first one too, with the
        // notes as far apart as they were.
        pattern.notes[1].key = 50;
        pattern.move_notes(&[0, 1], 0, 100);
        assert_eq!((pattern.notes[0].key, pattern.notes[1].key), (131, 121));
        pattern.move_notes(&[1, 0], 0, -200);
        assert_eq!((pattern.notes[0].key, pattern.notes[1].key), (10, 0));
        // A transposition moves no record.
        assert_eq!(pattern.notes[1].channel, 1);
    }

    #[test]
    fn a_resize_moves_each_note_s_end() {
        let mut pattern = Pattern {
            notes: vec![note(0, 0, 0), note(0, 96, 48), note(0, 192, 200)],
            ..Pattern::default()
        };
        let lengths = |pattern: &Pattern| -> Vec<i32> {
            pattern.notes.iter().map(|note| note.length).collect()
        };
        // A step gets a length this way.
        pattern.resize_notes(&[0, 1, 2], 24);
        assert_eq!(lengths(&pattern), [24, 72, 224]);
        // A note keeps a tick, and the others go on getting shorter.
        pattern.resize_notes(&[2, 1, 0, 0], -100);
        assert_eq!(lengths(&pattern), [1, 1, 124]);
        pattern.resize_notes(&[2], i64::MAX);
        assert_eq!(lengths(&pattern), [1, 1, i32::MAX - 192]);
    }

    #[test]
    fn a_note_s_flags_and_values_are_set_in_its_record() {
        let mut project = project();
        let pattern = project.pattern_to_edit(1);
        pattern.notes[0].flags = 0x13;
        pattern.set_notes_muted(&[0, 2], true);
        assert_eq!(pattern.notes[0].flags, 0x33);
        assert!(pattern.notes[2].muted() && !pattern.notes[1].muted());
        // A muted note lights no step.
        assert_eq!(lit(pattern, 0), [4, 8, 12]);
        pattern.set_notes_muted(&[0], false);
        assert_eq!(pattern.notes[0].flags, 0x13);

        // Every field has a byte of its own, and stops at its top.
        for (value, field) in (1..).zip(NoteField::ALL) {
            pattern.set_note_field(3, field, value);
            assert_eq!(field.of(&pattern.notes[3]), value);
            pattern.set_note_field(4, field, 255);
            assert_eq!(field.of(&pattern.notes[4]), field.top());
        }
        let note = pattern.notes[3];
        assert_eq!(
            (note.velocity, note.pan, note.fine_pitch),
            (1, 2, 3),
            "{note:?}"
        );
        assert_eq!((note.release, note.mod_x, note.mod_y), (4, 5, 6));
        let top = pattern.notes[4];
        assert_eq!((top.velocity, top.fine_pitch, top.mod_x), (128, 240, 255));
        // A new note is in the centre of the fields that have one.
        let new = layout::Note::default();
        for field in NoteField::ALL {
            assert!(field.centre().is_none_or(|centre| centre == field.of(&new)));
        }

        // One 224 differs, and it is as long as it was.
        let (new, gone) = changes(&project);
        let ([(2, new)], [gone]) = (&new[..], &gone[..]) else {
            panic!("{new:?} {gone:?}");
        };
        assert_eq!(new.data.len(), gone.data.len());
    }

    #[test]
    fn a_selection_takes_the_place_of_the_last_one_or_adds_to_it() {
        let mut project = project();
        let pattern = project.pattern_to_edit(1);
        let selected = |pattern: &Pattern| -> Vec<usize> {
            let notes = pattern.notes.iter().enumerate();
            notes
                .filter_map(|(at, note)| note.selected().then_some(at))
                .collect()
        };
        pattern.select_notes(&[0, 2, 9], false);
        assert_eq!(selected(pattern), [0, 2]);
        pattern.select_notes(&[3], true);
        assert_eq!(selected(pattern), [0, 2, 3]);
        // The clap is another channel's, and lets go with the kicks.
        pattern.select_notes(&[4], false);
        assert_eq!(selected(pattern), [4]);
        assert_eq!(pattern.notes[4].flags, layout::Note::SELECTED);
        pattern.select_notes(&[], false);
        assert_eq!(to_flp(&project), Ok(file()));
    }
}
