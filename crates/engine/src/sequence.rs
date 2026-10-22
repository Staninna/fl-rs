//! The playlist as flat lists of notes, audio clips and automation clips
//! on the song's timeline.

use model::{ChannelKind, ClipSettings, ItemSource, Note, Project, Trim};
use std::collections::BTreeMap;

/// One note placed on the song's timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Played {
    /// Note-on, in ticks from the start of the song.
    pub tick: u64,
    /// Ticks until note-off. `None` for a step sequencer note, which plays
    /// its sample to the end.
    pub length: Option<u64>,
    pub note: Note,
}

/// One audio clip placed on the song's timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlayedClip {
    /// Where the clip starts, in ticks from the start of the song.
    pub tick: u64,
    /// Ticks the clip lasts.
    pub length: u64,
    /// The audio clip channel, by its index.
    pub channel: u16,
    /// Where in the sample the clip starts, in milliseconds of the sample
    /// at its own speed. For a clip that plays backwards they count from
    /// the sample's end.
    pub offset_ms: f32,
    pub settings: ClipSettings,
}

/// One automation clip placed on the song's timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlayedAutomation {
    /// Where the clip starts, in ticks from the start of the song.
    pub tick: u64,
    /// Ticks the clip lasts.
    pub length: u64,
    /// The automation channel, by its index.
    pub channel: u16,
    /// Where in the clip the item starts, in beats.
    pub offset: f32,
}

#[derive(Debug, Default)]
pub(crate) struct Song {
    /// In the order they start.
    pub notes: Vec<Played>,
    /// In the order they start.
    pub clips: Vec<PlayedClip>,
    /// In the order they start.
    pub automation: Vec<PlayedAutomation>,
    /// Where the last item ends, in ticks.
    pub end: u64,
    /// The notes whose pitch an arpeggiator moves as they sound, by their
    /// place in `notes`: from each tick on, the note is this many cents
    /// off the pitch it started at ([`crate::arp`]).
    pub glides: BTreeMap<usize, Vec<(u64, f32)>>,
}

/// Ticks in one bar. 17 and 18 are read as a time signature, 4/4 when the
/// project has none.
fn bar_ticks(project: &Project) -> u64 {
    let (num, beat) = match project.time_signature() {
        Some((num, beat)) if num > 0 && beat > 0 => (u64::from(num), u64::from(beat)),
        _ => (4, 4),
    };
    (u64::from(project.header.ppq) * 4 * num / beat).max(1)
}

/// A position or a length from a record. No corpus record has a negative
/// one, and one that does counts as 0.
fn ticks(stored: i32) -> u64 {
    u64::try_from(stored).unwrap_or(0)
}

/// Walk the current arrangement's items. A project with an empty playlist
/// plays pattern 1 once, which is what FL does in pattern mode.
///
/// A muted item adds nothing, and neither does a record FL deleted. A
/// pattern item adds the pattern's notes that are not muted. An item that
/// places an audio clip channel adds a clip, and one that places an
/// automation channel adds an automation clip.
pub(crate) fn sequence(project: &Project) -> Song {
    let items = project.current_arrangement().map_or(&[][..], |a| &a.items);
    let mut song = Song::default();
    if items.is_empty() {
        pattern_mode(project, 1, &mut song);
    }
    for item in items {
        let Some(source) = item.source() else {
            continue;
        };
        let position = ticks(item.position);
        let length = ticks(item.length);
        song.end = song.end.max(position + length);
        if item.muted() {
            continue;
        }
        match source {
            ItemSource::Pattern(index) => {
                let Some(pattern) = project.pattern(index) else {
                    continue;
                };
                // The item shows `length` ticks of the pattern from `start`
                // on. A note that begins before the window is dropped,
                // though FL plays the rest of it when 30
                // `PlayTruncatedNotes` is on. A note that runs past the
                // window ends with it.
                let start = match item.trim() {
                    Some(Trim::Ticks { start, .. }) => ticks(start),
                    _ => 0,
                };
                let end = start + length;
                for note in pattern.notes.iter().filter(|note| sounds(note)) {
                    let at = ticks(note.position);
                    if at < start || at >= end {
                        continue;
                    }
                    let held = ticks(note.length);
                    song.notes.push(Played {
                        tick: position + at - start,
                        length: (held > 0).then(|| held.min(end - at)),
                        note: *note,
                    });
                }
            }
            ItemSource::Channel(index) => {
                let kind = project.channel(index).and_then(|channel| channel.kind);
                // Milliseconds into an audio clip's sample, and beats into
                // an automation clip.
                let offset = match item.trim() {
                    Some(Trim::Clip { start, .. }) => start,
                    _ => 0.0,
                };
                if kind == Some(ChannelKind::Automation) {
                    song.automation.push(PlayedAutomation {
                        tick: position,
                        length,
                        channel: index,
                        offset,
                    });
                }
                let settings = item.clip();
                // A file can hold any bits where these three floats are.
                let numbers = [settings.gain, settings.pitch, settings.pan];
                if kind != Some(ChannelKind::AudioClip) || numbers.iter().any(|v| !v.is_finite()) {
                    continue;
                }
                song.clips.push(PlayedClip {
                    tick: position,
                    length,
                    channel: index,
                    offset_ms: offset,
                    settings,
                });
            }
        }
    }
    song.notes.sort_by_key(|played| played.tick);
    song.clips.sort_by_key(|clip| clip.tick);
    song.automation.sort_by_key(|clip| clip.tick);
    song
}

/// Whether a note record plays: FL has not deleted it and it is not muted.
fn sounds(note: &Note) -> bool {
    !note.is_deleted() && !note.muted()
}

/// One pattern alone, as FL's PAT mode plays it: from its first tick, as
/// long as its notes need rounded up to a whole bar, and a bar when it has
/// no notes or is not in the project. The playlist adds nothing.
pub(crate) fn pattern(project: &Project, index: u16) -> Song {
    let mut song = Song::default();
    pattern_mode(project, index, &mut song);
    song.end = song.end.max(bar_ticks(project));
    song
}

/// A pattern from its first tick, as long as its notes need rounded up to a
/// whole bar. A muted note counts towards that length and does not play.
fn pattern_mode(project: &Project, index: u16, song: &mut Song) {
    let Some(pattern) = project.pattern(index) else {
        return;
    };
    let mut last = 0;
    for note in pattern.notes.iter().filter(|note| !note.is_deleted()) {
        let at = ticks(note.position);
        let length = ticks(note.length);
        last = last.max(at + length.max(1));
        if note.muted() {
            continue;
        }
        song.notes.push(Played {
            tick: at,
            length: (length > 0).then_some(length),
            note: *note,
        });
    }
    let bar = bar_ticks(project);
    song.end = last.div_ceil(bar) * bar;
}

#[cfg(test)]
mod tests {
    use super::*;
    use flp::RawEvent;
    use flp::event::Id;
    use model::{Arrangement, Channel, Item, Pattern, Placed, PlaylistItem};

    fn note(position: i32, length: i32) -> Note {
        Note {
            position,
            length,
            key: 60,
            ..Note::default()
        }
    }

    fn item(position: i32, pattern: u16, length: i32) -> PlaylistItem {
        PlaylistItem::new(ItemSource::Pattern(pattern), position, length)
    }

    fn clip(position: i32, channel: u16, length: i32) -> PlaylistItem {
        PlaylistItem::new(ItemSource::Channel(channel), position, length)
    }

    fn project(patterns: &[(u16, &[Note])], items: &[PlaylistItem]) -> Project {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 1,
            ppq: 96,
        });
        for (index, notes) in patterns {
            project.patterns.push(Pattern {
                index: *index,
                notes: notes.to_vec(),
                ..Pattern::default()
            });
        }
        project.arrangements.push(Arrangement {
            items: items.to_vec(),
            ..Arrangement::default()
        });
        project
    }

    fn ticks(song: &Song) -> Vec<(u64, Option<u64>)> {
        song.notes.iter().map(|p| (p.tick, p.length)).collect()
    }

    #[test]
    fn an_item_offsets_its_pattern_s_notes() {
        let notes = [note(0, 0), note(96, 48)];
        let song = sequence(&project(
            &[(1, &notes)],
            &[item(0, 1, 384), item(768, 1, 384)],
        ));
        assert_eq!(
            ticks(&song),
            [(0, None), (96, Some(48)), (768, None), (864, Some(48))]
        );
        assert_eq!(song.end, 1152);
    }

    #[test]
    fn an_item_plays_only_its_window() {
        let notes = [note(0, 0), note(96, 480), note(192, 0), note(288, 0)];
        let mut trimmed = item(1000, 1, 192);
        trimmed.set_trim(Some(Trim::Ticks {
            start: 96,
            end: 288,
        }));
        let song = sequence(&project(&[(1, &notes)], &[trimmed]));
        // The note at 96 would run to 576 and stops at the window's 288.
        assert_eq!(ticks(&song), [(1000, Some(192)), (1096, None)]);
        let song = sequence(&project(&[(1, &notes)], &[item(0, 1, 192)]));
        assert_eq!(ticks(&song), [(0, None), (96, Some(96))]);
        // A trim that starts at 0 is the same window as no trim.
        let mut from_start = item(0, 1, 192);
        from_start.set_trim(Some(Trim::Ticks { start: 0, end: 192 }));
        let song = sequence(&project(&[(1, &notes)], &[from_start]));
        assert_eq!(ticks(&song), [(0, None), (96, Some(96))]);
    }

    #[test]
    fn muted_items_deleted_items_and_missing_patterns_add_no_notes() {
        let notes = [note(0, 0)];
        let mut muted = item(0, 1, 384);
        muted.set_muted(true);
        let mut deleted = item(2000, 1, 384);
        deleted.source_id = u32::MAX;
        let items = [muted, clip(384, 0, 384), item(768, 9, 384), deleted];
        let song = sequence(&project(&[(1, &notes)], &items));
        assert_eq!(ticks(&song), []);
        // The channel the clip names is not in the project.
        assert_eq!(song.clips, []);
        // The deleted record is no part of the song.
        assert_eq!(song.end, 1152);
    }

    #[test]
    fn muted_notes_and_deleted_notes_do_not_play() {
        let mut muted = note(96, 0);
        muted.flags = Note::MUTED | 3;
        let mut deleted = note(192, 0);
        (deleted.tag, deleted.channel) = (u16::MAX, u16::MAX);
        let mut coloured = note(288, 0);
        coloured.flags = Note::PORTA | Note::SELECTED | 0xf;
        let notes = [note(0, 0), muted, deleted, coloured, note(700, 0)];
        let song = sequence(&project(&[(1, &notes)], &[item(0, 1, 384)]));
        assert_eq!(ticks(&song), [(0, None), (288, None)]);
        assert_eq!(song.notes[1].note, coloured);

        // In pattern mode a muted note still counts towards the length.
        let mut last = note(400, 0);
        last.flags = Note::MUTED;
        let deleted = Note {
            position: 900,
            ..deleted
        };
        let song = sequence(&project(&[(1, &[note(0, 0), last, deleted])], &[]));
        assert_eq!(ticks(&song), [(0, None)]);
        assert_eq!(song.end, 768);
    }

    #[test]
    fn an_item_on_an_audio_clip_channel_is_a_clip() {
        let mut p = project(&[], &[]);
        for (index, kind) in [
            (0, ChannelKind::Sampler),
            (1, ChannelKind::AudioClip),
            (2, ChannelKind::Automation),
        ] {
            p.channels.push(Channel {
                index,
                kind: Some(kind),
                ..Channel::default()
            });
        }
        let mut trimmed = clip(96, 1, 48);
        trimmed.set_trim(Some(Trim::Clip {
            start: 250.0,
            end: 500.0,
        }));
        trimmed.gain = 0.5;
        trimmed.clip_flags = PlaylistItem::REVERSED;
        let mut muted = clip(0, 1, 96);
        muted.set_muted(true);
        let mut broken = clip(0, 1, 96);
        broken.gain = f32::NAN;
        p.arrangements[0].items = vec![
            clip(960, 1, 96),
            clip(0, 0, 96),
            clip(0, 2, 96),
            muted,
            broken,
            trimmed,
        ];
        let song = sequence(&p);
        let placed = |clip: &PlayedClip| (clip.tick, clip.length, clip.channel, clip.offset_ms);
        let clips: Vec<_> = song.clips.iter().map(placed).collect();
        assert_eq!(clips, [(96, 48, 1, 250.0), (960, 96, 1, 0.0)]);
        // The item on channel 2 is an automation clip, and none of the
        // others is.
        let automation = PlayedAutomation {
            tick: 0,
            length: 96,
            channel: 2,
            offset: 0.0,
        };
        assert_eq!(song.automation, [automation]);
        assert_eq!(song.clips[0].settings, trimmed.clip());
        assert!(song.clips[0].settings.reversed && song.clips[0].settings.gain == 0.5);
        assert_eq!(song.notes, []);
        assert_eq!(song.end, 1056);
    }

    #[test]
    fn an_item_on_an_automation_channel_is_an_automation_clip() {
        let mut p = project(&[], &[]);
        p.channels.push(Channel {
            index: 3,
            kind: Some(ChannelKind::Automation),
            ..Channel::default()
        });
        // Two beats of the clip, from its third beat on.
        let mut trimmed = clip(960, 3, 192);
        trimmed.set_trim(Some(Trim::Clip {
            start: 2.0,
            end: 4.0,
        }));
        let mut muted = clip(0, 3, 96);
        muted.set_muted(true);
        // The floats an audio clip plays by mean nothing here.
        let mut odd = clip(96, 3, 384);
        odd.gain = f32::NAN;
        p.arrangements[0].items = vec![trimmed, muted, odd];
        let song = sequence(&p);
        let placed = |clip: &PlayedAutomation| (clip.tick, clip.length, clip.channel, clip.offset);
        let clips: Vec<_> = song.automation.iter().map(placed).collect();
        assert_eq!(clips, [(96, 384, 3, 0.0), (960, 192, 3, 2.0)]);
        assert_eq!(song.clips, []);
        assert_eq!(song.end, 1152);
    }

    #[test]
    fn an_empty_playlist_plays_pattern_1_once() {
        let first = [note(0, 0), note(400, 0)];
        let second = [note(48, 0)];
        let mut p = project(&[(2, &second), (1, &first)], &[]);
        let song = sequence(&p);
        assert_eq!(ticks(&song), [(0, None), (400, None)]);
        assert_eq!(song.end, 768);
        p.arrangements.clear();
        assert_eq!(ticks(&sequence(&p)), [(0, None), (400, None)]);
        p.patterns.clear();
        assert_eq!(ticks(&sequence(&p)), []);
    }

    #[test]
    fn a_pattern_alone_ignores_the_playlist() {
        let first = [note(0, 0)];
        let second = [note(48, 0), note(400, 96)];
        let p = project(&[(1, &first), (2, &second)], &[item(960, 1, 384)]);
        let song = pattern(&p, 2);
        assert_eq!(ticks(&song), [(48, None), (400, Some(96))]);
        assert_eq!(song.end, 768);
        // A pattern without notes is a bar long, and so is one that is not
        // in the project.
        let song = pattern(&p, 9);
        assert_eq!((ticks(&song), song.end), (vec![], 384));
    }

    #[test]
    fn the_last_100_picks_the_arrangement() {
        let notes = [note(0, 0)];
        let mut p = project(&[(1, &notes)], &[item(0, 1, 384)]);
        p.arrangements.push(Arrangement {
            index: 1,
            items: vec![item(96, 1, 384)],
            ..Arrangement::default()
        });
        assert_eq!(ticks(&sequence(&p)), [(0, None)]);
        p.order.push(Placed {
            pos: 0,
            item: Item::Raw(RawEvent {
                id: Id::ArrangementsCurrent.0,
                data: vec![1, 0],
            }),
        });
        assert_eq!(ticks(&sequence(&p)), [(96, None)]);
    }
}
