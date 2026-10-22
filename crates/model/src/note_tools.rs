//! The piano roll's tools that make notes of notes: paint, slice, glue,
//! quantize, chop, arpeggiate, strum and snap to scale. Each one is a
//! single change to a pattern, so the app's undo takes the whole of it
//! back in one step.
//!
//! A note goes by its place in [`Pattern::notes`], as for the other note
//! edits, and a place that has no note, or a record FL deleted, is
//! skipped. The new notes are copies of the ones they came from, with every
//! field but the position and the length kept, the selection too. A note
//! that goes away is deleted as [`Pattern::delete_notes`] does it, so the
//! notes in front of it keep their places. The list stays in the order of
//! the positions.
//!
//! Where FL asks in a dialog, these do what the dialog does at its default
//! settings, and the docs of each method say which those are.

use crate::Pattern;
use flp::event::layout::Note;
use std::collections::BTreeMap;

impl Pattern {
    /// Add notes of this channel, each `length` ticks long, at the
    /// positions and on the keys given, as [`Pattern::add_note`] adds one.
    /// The paint tool lays these.
    pub fn add_notes(&mut self, channel: u16, length: u64, notes: &[(u64, u8)]) {
        let length = i32::try_from(length).unwrap_or(i32::MAX);
        for &(position, key) in notes {
            let Ok(position) = i32::try_from(position) else {
                continue;
            };
            self.add_note(Note {
                position,
                channel,
                length,
                key: i16::from(key),
                ..Note::default()
            });
        }
    }

    /// Cut each note that `at` falls inside of in two: the note ends at
    /// `at`, and a copy of it plays from there to where it ended. A note
    /// that starts or ends at `at`, and a step, which has no end, are left
    /// as they are.
    pub fn slice_notes(&mut self, notes: &[usize], at: u64) {
        let Ok(at) = i32::try_from(at) else {
            return;
        };
        let mut pieces = Vec::new();
        self.each_live(notes, |note| {
            let end = note.position.saturating_add(note.length);
            if note.length > 0 && note.position < at && at < end {
                note.length = at - note.position;
                pieces.push(Note {
                    position: at,
                    length: end - at,
                    ..*note
                });
            }
        });
        self.insert_all(pieces);
    }

    /// Join the notes that are on one key of one channel and touch or
    /// overlap: the first of each run takes the run's whole length and the
    /// others are deleted. FL's Glue does this to the notes it is given.
    /// A step has no end to join and is left as it is.
    pub fn glue_notes(&mut self, notes: &[usize]) {
        let mut runs: BTreeMap<(u16, i16), Vec<usize>> = BTreeMap::new();
        for id in self.live(notes) {
            let note = &self.notes[id];
            if note.length > 0 {
                runs.entry((note.channel, note.key)).or_default().push(id);
            }
        }
        let mut gone = Vec::new();
        for mut ids in runs.into_values() {
            ids.sort_by_key(|id| self.notes[*id].position);
            let mut head = ids[0];
            for id in ids.into_iter().skip(1) {
                let (first, next) = (self.notes[head], self.notes[id]);
                let end = i64::from(first.position) + i64::from(first.length);
                if i64::from(next.position) <= end {
                    let joined = end.max(i64::from(next.position) + i64::from(next.length));
                    let length = joined - i64::from(first.position);
                    self.notes[head].length = i32::try_from(length).unwrap_or(i32::MAX);
                    gone.push(id);
                } else {
                    head = id;
                }
            }
        }
        self.delete_notes(&gone);
    }

    /// Move each note's start to the nearest multiple of `step` ticks, and
    /// with `lengths` make each length the nearest multiple too, one `step`
    /// at least. This is FL's quick quantize: Ctrl+Q for both and Shift+Q
    /// for the starts alone. A step keeps its want of a length.
    pub fn quantize_notes(&mut self, notes: &[usize], step: u64, lengths: bool) {
        let step = i64::try_from(step.max(1)).unwrap_or(i64::MAX);
        let nearest = |ticks: i64| (ticks + step / 2).div_euclid(step) * step;
        let mut moved = false;
        self.each_live(notes, |note| {
            let position = nearest(i64::from(note.position)).clamp(0, i64::from(i32::MAX));
            moved |= position != i64::from(note.position);
            note.position = position as i32;
            if lengths && note.length > 0 {
                let length = nearest(i64::from(note.length)).max(step);
                note.length = i32::try_from(length).unwrap_or(i32::MAX);
            }
        });
        if moved {
            self.notes.sort_by_key(|note| note.position);
        }
    }

    /// Cut each note into pieces `step` ticks long from its start, the
    /// last piece being what is left. FL's Chop, at its default of the
    /// snap's length. A step, and a note no longer than `step`, stay whole.
    pub fn chop_notes(&mut self, notes: &[usize], step: u64) {
        let Ok(step) = i32::try_from(step.max(1)) else {
            return;
        };
        let mut pieces = Vec::new();
        self.each_live(notes, |note| {
            if note.length <= step {
                return;
            }
            let end = note.position.saturating_add(note.length);
            let mut at = note.position.saturating_add(step);
            while at < end {
                let length = step.min(end - at);
                pieces.push(Note {
                    position: at,
                    length,
                    ..*note
                });
                at = at.saturating_add(step);
            }
            note.length = step;
        });
        self.insert_all(pieces);
    }

    /// Play each chord as an arpeggio: the notes of a channel that start
    /// together, two at least, become a run of notes `step` ticks long,
    /// from the lowest key up and over again, for as long as the chord's
    /// longest note. A run note is a copy of the chord note on its key.
    /// This is FL's Arpeggiator at its default pattern, up, over the
    /// chord's own keys, with each note as long as the time between them.
    /// A chord of steps lasts one `step` per note.
    pub fn arpeggiate_notes(&mut self, notes: &[usize], step: u64) {
        let Ok(step) = i32::try_from(step.max(1)) else {
            return;
        };
        let mut runs = Vec::new();
        let mut gone = Vec::new();
        for chord in self.chords(notes) {
            let keys: Vec<Note> = chord.iter().map(|id| self.notes[*id]).collect();
            let start = keys[0].position;
            let longest = keys.iter().map(|note| note.length).max().unwrap_or(0);
            let span = if longest > 0 {
                longest
            } else {
                step.saturating_mul(keys.len() as i32)
            };
            let mut offset = 0;
            let mut turn = 0;
            while offset < span {
                runs.push(Note {
                    position: start.saturating_add(offset),
                    length: step.min(span - offset),
                    ..keys[turn % keys.len()]
                });
                offset = offset.saturating_add(step);
                turn += 1;
            }
            gone.extend(chord);
        }
        self.delete_notes(&gone);
        self.insert_all(runs);
    }

    /// Strum each chord: of the notes of a channel that start together,
    /// two at least, the one on the lowest key stays and each one up
    /// starts `delay` ticks after the one under it, with its end where it
    /// was. A note keeps a tick at least. FL's Strum, upwards, with its
    /// tension flat and the ends kept.
    pub fn strum_notes(&mut self, notes: &[usize], delay: u64) {
        let delay = i64::try_from(delay).unwrap_or(i64::MAX);
        let mut moved = false;
        for chord in self.chords(notes) {
            for (nth, id) in chord.into_iter().enumerate() {
                let note = &mut self.notes[id];
                let by = delay.saturating_mul(nth as i64);
                let position = (i64::from(note.position) + by).min(i64::from(i32::MAX));
                let shift = position - i64::from(note.position);
                if note.length > 0 {
                    note.length = (i64::from(note.length) - shift).max(1) as i32;
                }
                moved |= shift != 0;
                note.position = position as i32;
            }
        }
        if moved {
            self.notes.sort_by_key(|note| note.position);
        }
    }

    /// Move each note whose key is not in the scale to the nearest key
    /// that is, the one under it when two are as near. `keys` has bit n
    /// set for the keys n semitones above a C that are in the scale, and
    /// with none set nothing moves. FL's Snap to scale.
    pub fn snap_notes_to_scale(&mut self, notes: &[usize], keys: u16) {
        let keys = keys & 0xfff;
        if keys == 0 {
            return;
        }
        let top = Self::TOP_KEY;
        let fits = |key: i16| (0..=top).contains(&key) && keys & (1 << key.rem_euclid(12)) != 0;
        self.each_live(notes, |note| {
            let near = (0..12).flat_map(|by| [note.key - by, note.key + by]);
            if let Some(key) = near.into_iter().find(|key| fits(*key)) {
                note.key = key;
            }
        });
    }

    /// The chords among the notes: two or more of one channel that start
    /// together, each from the lowest key up.
    fn chords(&self, notes: &[usize]) -> Vec<Vec<usize>> {
        let mut chords: BTreeMap<(u16, i32), Vec<usize>> = BTreeMap::new();
        for id in self.live(notes) {
            let note = &self.notes[id];
            chords
                .entry((note.channel, note.position))
                .or_default()
                .push(id);
        }
        let chords = chords.into_values().filter(|chord| chord.len() > 1);
        chords
            .map(|mut chord| {
                chord.sort_by_key(|id| (self.notes[*id].key, *id));
                chord
            })
            .collect()
    }

    /// The places of the notes that name a note that is there, each once.
    fn live(&self, notes: &[usize]) -> Vec<usize> {
        let mut notes = notes.to_vec();
        notes.sort_unstable();
        notes.dedup();
        notes.retain(|id| self.notes.get(*id).is_some_and(|note| !note.is_deleted()));
        notes
    }

    fn each_live(&mut self, notes: &[usize], mut change: impl FnMut(&mut Note)) {
        for id in self.live(notes) {
            change(&mut self.notes[id]);
        }
    }

    /// Put each note behind the notes that start where it does or before.
    fn insert_all(&mut self, notes: Vec<Note>) {
        for note in notes {
            let at = self.notes.partition_point(|n| n.position <= note.position);
            self.notes.insert(at, note);
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
            channel: 1,
            ..Note::default()
        }
    }

    fn pattern(notes: &[Note]) -> Pattern {
        Pattern {
            index: 1,
            notes: notes.to_vec(),
            ..Pattern::default()
        }
    }

    /// The notes that are there, as position, length and key.
    fn placed(pattern: &Pattern) -> Vec<(i32, i32, i16)> {
        let live = pattern.notes.iter().filter(|note| !note.is_deleted());
        live.map(|note| (note.position, note.length, note.key))
            .collect()
    }

    #[test]
    fn a_slice_cuts_the_notes_it_crosses_in_two() {
        let mut first = note(0, 96, 60);
        first.velocity = 70;
        let mut notes = pattern(&[first, note(48, 24, 62), note(48, 0, 64), note(96, 48, 60)]);
        notes.slice_notes(&[0, 1, 2, 3, 9], 60);
        assert_eq!(
            placed(&notes),
            [
                (0, 60, 60),
                (48, 12, 62),
                (48, 0, 64),
                (60, 36, 60),
                (60, 12, 62),
                (96, 48, 60)
            ]
        );
        // The piece is a copy of the note it was cut from.
        assert_eq!(notes.notes[3].velocity, 70);
        // A note that ends where the line is stays whole.
        notes.slice_notes(&[0], 60);
        assert_eq!(placed(&notes).len(), 6);
    }

    #[test]
    fn glue_joins_the_notes_of_a_key_that_touch() {
        let mut notes = pattern(&[
            note(0, 48, 60),
            note(0, 24, 62),
            note(48, 24, 60),
            note(60, 48, 60),
            note(200, 10, 60),
            note(48, 24, 62),
        ]);
        notes.glue_notes(&[0, 1, 2, 3, 4, 5]);
        // The C5s from 0 to 108 are one note, the D5s are apart, and the
        // C5 at 200 is on its own.
        assert_eq!(
            placed(&notes),
            [(0, 108, 60), (0, 24, 62), (200, 10, 60), (48, 24, 62)]
        );
        assert!(notes.notes[2].is_deleted() && notes.notes[3].is_deleted());
    }

    #[test]
    fn quantize_moves_the_starts_and_maybe_the_lengths_to_the_grid() {
        let mut notes = pattern(&[note(11, 30, 60), note(13, 0, 62), note(40, 5, 64)]);
        notes.quantize_notes(&[0, 1, 2], 24, false);
        assert_eq!(placed(&notes), [(0, 30, 60), (24, 0, 62), (48, 5, 64)]);
        notes.quantize_notes(&[0, 1, 2], 24, true);
        assert_eq!(placed(&notes), [(0, 24, 60), (24, 0, 62), (48, 24, 64)]);
    }

    #[test]
    fn chop_cuts_notes_into_pieces_of_a_step() {
        let mut notes = pattern(&[note(10, 60, 60), note(100, 24, 62)]);
        notes.chop_notes(&[0, 1], 24);
        assert_eq!(
            placed(&notes),
            [(10, 24, 60), (34, 24, 60), (58, 12, 60), (100, 24, 62)]
        );
    }

    #[test]
    fn a_chord_arpeggiates_upwards_for_as_long_as_it_lasts() {
        let mut low = note(0, 96, 60);
        low.velocity = 50;
        let mut notes = pattern(&[low, note(0, 48, 67), note(0, 96, 64), note(200, 24, 72)]);
        notes.arpeggiate_notes(&[0, 1, 2, 3], 24);
        assert_eq!(
            placed(&notes),
            [
                (0, 24, 60),
                (24, 24, 64),
                (48, 24, 67),
                (72, 24, 60),
                (200, 24, 72)
            ]
        );
        let first = notes.notes.iter().find(|note| !note.is_deleted()).unwrap();
        assert_eq!(first.velocity, 50);
    }

    #[test]
    fn a_strum_starts_each_note_up_later_and_keeps_its_end() {
        let mut notes = pattern(&[
            note(0, 96, 64),
            note(0, 96, 60),
            note(0, 96, 67),
            note(0, 2, 70),
        ]);
        notes.strum_notes(&[0, 1, 2, 3], 4);
        assert_eq!(
            placed(&notes),
            [(0, 96, 60), (4, 92, 64), (8, 88, 67), (12, 1, 70)]
        );
    }

    #[test]
    fn paint_adds_notes_of_one_length() {
        let mut notes = pattern(&[note(48, 24, 60)]);
        notes.add_notes(1, 24, &[(0, 60), (24, 62), (72, 64)]);
        assert_eq!(
            placed(&notes),
            [(0, 24, 60), (24, 24, 62), (48, 24, 60), (72, 24, 64)]
        );
    }

    #[test]
    fn snap_to_scale_takes_the_nearest_key_of_the_scale() {
        let c_major = 0b1010_1011_0101;
        let keys = [60, 61, 63, 66, 0, 131];
        let mut notes = pattern(&keys.map(|key| note(0, 24, key)));
        notes.snap_notes_to_scale(&[0, 1, 2, 3, 4, 5], c_major);
        let snapped: Vec<i16> = notes.notes.iter().map(|note| note.key).collect();
        // C# and D# go down, F# goes down to F, and B10 is in C major.
        assert_eq!(snapped, [60, 60, 62, 65, 0, 131]);
        // C minor pentatonic has no B: B10 goes down to A#10.
        notes.snap_notes_to_scale(&[5], 0b0100_1010_1001);
        assert_eq!(notes.notes[5].key, 130);
        notes.snap_notes_to_scale(&[0, 1, 2, 3], 0);
        assert_eq!(notes.notes[3].key, 65);
    }
}
