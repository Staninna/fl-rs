//! The project and the events that belong to no smaller object.

use crate::codec::{self, Cx, object};
use crate::pattern::write_field;
use crate::rec::{RecTarget, control};
use crate::{Arrangement, Channel, Error, Insert, Item, Pattern, Placed};
use flp::event::{Id, layout};
use std::ops::RangeInclusive;

/// FL's tempo range, 10 to 522 BPM, in thousandths of a BPM: the range its
/// tempo control maps automation onto (`docs/automation-234.md`).
pub const TEMPO_MILLI: RangeInclusive<u32> = 10_000..=522_000;

/// A whole project.
///
/// The fields up to `initial_values` come from project-scope events, and
/// `order` holds those plus every event that found no other owner.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    /// The `FLhd` chunk: format, channel count and PPQ. Nothing keeps the
    /// channel count in step with [`Project::channels`].
    pub header: flp::Header,
    /// 199 `ProjectID.FLVersion`: 8-bit string such as `24.2.99.4720`, from
    /// the file's first 199. It decides whether the other strings are
    /// UTF-16, so changing it across 11.5.4 re-encodes them all.
    pub writer_version: Option<String>,
    /// 156 `ProjectID.Tempo`: u32, BPM x 1000. See [`Project::tempo`].
    pub tempo_milli: Option<u32>,
    /// 66 `ProjectID._TempoCoarse`: u16 BPM, the old form of 156.
    pub tempo_coarse: Option<u16>,
    /// 93 `ProjectID._TempoFine`: u16 thousandths of a BPM, the fraction
    /// that goes with 66.
    pub tempo_fine: Option<u16>,
    /// 17 `ArrangementsID.TimeSigNum`: u8.
    pub time_sig_num: Option<u8>,
    /// 18 `ArrangementsID.TimeSigBeat`: u8.
    pub time_sig_beat: Option<u8>,
    /// 80 `ProjectID.Pitch`: i16 cents.
    pub pitch: Option<i16>,
    /// 194 `ProjectID.Title`: string.
    pub title: Option<String>,
    /// 195 `ProjectID.Comments`: string.
    pub comments: Option<String>,
    /// 206 `ProjectID.Genre`: string.
    pub genre: Option<String>,
    /// 207 `ProjectID.Artists`: string.
    pub artists: Option<String>,
    /// 197 `ProjectID.Url`: string.
    pub url: Option<String>,
    /// 103 `FLP_FXTrackCount`: u16, the number of mixer inserts, 3..502.
    pub insert_count: Option<u16>,
    /// 231 `DisplayGroupID.Name`: string, one event per channel display
    /// group. A group has no index of its own, only its place in this list.
    pub display_groups: Vec<String>,
    /// 225 `MixerID.Params`: 12-byte records `{u32, u32 REC id, i32 value}`,
    /// the initial values of automatable controls. Mixer volume and pan are
    /// in here. Only the first 225 of a file is read.
    pub initial_values: Vec<layout::InitialValue>,
    /// Channels in the order their first 64 arrived.
    pub channels: Vec<Channel>,
    /// Patterns in the order their first 65 arrived. Indices may have gaps;
    /// look one up with [`Project::pattern`].
    pub patterns: Vec<Pattern>,
    /// Arrangements in the order they were first addressed.
    pub arrangements: Vec<Arrangement>,
    /// Mixer inserts in file order.
    pub inserts: Vec<Insert>,
    /// Controller links in file order.
    pub controller_links: Vec<ControllerLink>,
    /// The project's own events in the order they arrived.
    pub order: Vec<Placed>,
}

object!(Project {
    one writer_version = FLVersion;
    one tempo_milli = Tempo;
    one tempo_coarse = TempoCoarse;
    one tempo_fine = TempoFine;
    one time_sig_num = ArrangementsTimeSigNum;
    one time_sig_beat = ArrangementsTimeSigBeat;
    one pitch = Pitch;
    one title = Title;
    one comments = Comments;
    one genre = Genre;
    one artists = Artists;
    one url = Url;
    one insert_count = FLP_FXTrackCount;
    each display_groups = DisplayGroupName;
    list initial_values = MixerParams;
});

impl Project {
    /// A project with a header and nothing else.
    pub fn new(header: flp::Header) -> Self {
        Project {
            header,
            writer_version: None,
            tempo_milli: None,
            tempo_coarse: None,
            tempo_fine: None,
            time_sig_num: None,
            time_sig_beat: None,
            pitch: None,
            title: None,
            comments: None,
            genre: None,
            artists: None,
            url: None,
            insert_count: None,
            display_groups: Vec::new(),
            initial_values: Vec::new(),
            channels: Vec::new(),
            patterns: Vec::new(),
            arrangements: Vec::new(),
            inserts: Vec::new(),
            controller_links: Vec::new(),
            order: Vec::new(),
        }
    }

    /// A new project, for File > New: PPQ 96, a 199 of FL 24.2.99, 120 BPM
    /// in a 156, and one sampler channel named "Sampler" that plays no
    /// sample yet, with the events of the one channel in FL's own
    /// `Empty.flp` ([`Channel::new_sampler`]), playing into the master.
    ///
    /// The rest of that template is not known here, so this has no mixer,
    /// no arrangement and no pattern: FL's 103, its inserts, its 99 and its
    /// tracks are left out, and the mixer window shows none. The edits put
    /// a pattern, a step, a note and a channel in as they do in a file,
    /// and the playlist stays empty. That FL starts its empty project at
    /// 120 BPM and PPQ 96 is inferred.
    pub fn blank() -> Self {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 1,
            ppq: 96,
        });
        project.writer_version = Some("24.2.99.4720".to_owned());
        project.tempo_milli = Some(120_000);
        project.order = [Id::FLVersion, Id::Tempo]
            .into_iter()
            .enumerate()
            .map(|(pos, id)| Placed {
                pos,
                item: Item::Field(id),
            })
            .collect();
        let mut channel = Channel::new_sampler(0, "Sampler", "", false, 2);
        // The template's channel has no 196.
        channel.sample_path = None;
        let path = Item::Field(Id::SamplePath);
        channel.order.retain(|placed| placed.item != path);
        project.channels.push(channel);
        project
    }

    /// Tempo in BPM: 156 divided by 1000, or for a file that only has the
    /// old pair, 66 plus 93 thousandths.
    pub fn tempo(&self) -> Option<f64> {
        match self.tempo_milli {
            Some(milli) => Some(f64::from(milli) / 1000.0),
            None => {
                let fine = self.tempo_fine.unwrap_or(0);
                Some(f64::from(self.tempo_coarse?) + f64::from(fine) / 1000.0)
            }
        }
    }

    /// Set the tempo, in thousandths of a BPM, held to [`TEMPO_MILLI`].
    ///
    /// The loader's tempo setter takes 156 whole, and 66 and 93 as the
    /// whole BPM and the thousandths of it (`docs/event-dictionary.md`).
    /// Each of the three the project has gets the new tempo, so the file
    /// reads as that tempo whichever of them comes last. A project with
    /// neither 156 nor 66 gets a 156 behind its 199, and a 66 without a
    /// 93 gets one behind it when the tempo has a fraction. No file of the
    /// corpus has a 225 record for the tempo, but one that does is set
    /// too, since the loader replays the records after the events.
    pub fn set_tempo_milli(&mut self, milli: u32) {
        let milli = milli.clamp(*TEMPO_MILLI.start(), *TEMPO_MILLI.end());
        let old_pair = self.tempo_coarse.is_some();
        if self.tempo_milli.is_some() || !old_pair {
            self.tempo_milli = Some(milli);
            write_field(&mut self.order, Id::FLVersion, Id::Tempo);
        }
        if old_pair {
            let (whole, fraction) = ((milli / 1000) as u16, (milli % 1000) as u16);
            self.tempo_coarse = Some(whole);
            if self.tempo_fine.is_some() || fraction != 0 {
                self.tempo_fine = Some(fraction);
                write_field(&mut self.order, Id::TempoCoarse, Id::TempoFine);
            }
        }
        let tempo = RecTarget::Global {
            control: control::TEMPO,
        };
        let legacy = self.legacy_rec_ids();
        for record in &mut self.initial_values {
            if RecTarget::decode(record.rec_id, legacy) == tempo {
                record.value = milli as i32;
            }
        }
    }

    /// 17 over 18, as the file stores them. Whether 17 counts beats per bar
    /// or is a numerator depends on 35 `TimeSigMode`, which stays raw.
    pub fn time_signature(&self) -> Option<(u8, u8)> {
        Some((self.time_sig_num?, self.time_sig_beat?))
    }

    /// The channel a 64 with this index opened.
    pub fn channel(&self, index: u16) -> Option<&Channel> {
        self.channels.iter().find(|c| c.index == index)
    }

    /// The pattern a 65 with this index selected.
    pub fn pattern(&self, index: u16) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.index == index)
    }

    /// The arrangement FL shows: the one the last 100
    /// `ArrangementsID.Current` names, or the first when the file has no 100
    /// or names an arrangement that is not there. The 100 is a raw event in
    /// [`Project::order`].
    pub fn current_arrangement(&self) -> Option<&Arrangement> {
        let current = self
            .order
            .iter()
            .rev()
            .find_map(|placed| match &placed.item {
                Item::Raw(e) if e.id == Id::ArrangementsCurrent.0 => {
                    Some(u16::from_le_bytes(e.data.as_slice().try_into().ok()?))
                }
                _ => None,
            });
        current
            .and_then(|index| self.arrangements.iter().find(|a| a.index == index))
            .or(self.arrangements.first())
    }

    /// [`Project::current_arrangement`], to change.
    pub fn current_arrangement_mut(&mut self) -> Option<&mut Arrangement> {
        let index = self.current_arrangement()?.index;
        self.arrangements.iter_mut().find(|a| a.index == index)
    }

    /// Where the last playlist item of the current arrangement ends, in
    /// ticks. 0 for an empty playlist. A muted item counts, and a record FL
    /// deleted does not.
    pub fn song_length_ticks(&self) -> u64 {
        let items = self.current_arrangement().map_or(&[][..], |a| &a.items);
        let placed = items.iter().filter(|item| item.source().is_some());
        let end = |item: &layout::PlaylistItem| {
            let ticks = |stored: i32| u64::try_from(stored).unwrap_or(0);
            ticks(item.position) + ticks(item.length)
        };
        placed.map(end).max().unwrap_or(0)
    }
}

/// The event stream a project stands for.
///
/// Every object's `order` is replayed and the events are sorted by position,
/// so a project that [`from_flp`](crate::from_flp) built and nobody changed
/// gives back the events it was built from.
///
/// The one thing left out is a note record that is deleted: FL compacts
/// those away when it saves, and no 224 it ships holds one. The record
/// stays in [`Pattern::notes`], so the notes keep their places for as long
/// as the project is open. A deleted playlist item is written, as FL
/// writes it.
///
/// The only failure is a string the file cannot hold: a character above
/// U+00FF where strings are 8-bit, or any string in a project that has no
/// [`writer_version`](Project::writer_version) to pick an encoding.
pub fn to_flp(project: &Project) -> Result<flp::Project, Error> {
    let mut cx = Cx::default();
    if let Some(version) = &project.writer_version {
        cx.set_version(version);
    }
    let cx = &cx;
    let mut out = Vec::new();
    codec::emit(project, cx, &mut out)?;
    for channel in &project.channels {
        codec::emit(channel, cx, &mut out)?;
    }
    for pattern in &project.patterns {
        if pattern.notes.iter().any(layout::Note::is_deleted) {
            let mut compact = pattern.clone();
            compact.notes.retain(|note| !note.is_deleted());
            codec::emit(&compact, cx, &mut out)?;
        } else {
            codec::emit(pattern, cx, &mut out)?;
        }
        for marker in &pattern.time_markers {
            codec::emit(marker, cx, &mut out)?;
        }
    }
    for arrangement in &project.arrangements {
        codec::emit(arrangement, cx, &mut out)?;
        for track in &arrangement.tracks {
            codec::emit(track, cx, &mut out)?;
        }
        for marker in &arrangement.time_markers {
            codec::emit(marker, cx, &mut out)?;
        }
    }
    for insert in &project.inserts {
        codec::emit(insert, cx, &mut out)?;
        for slot in &insert.slots {
            codec::emit(slot, cx, &mut out)?;
        }
    }
    for link in &project.controller_links {
        codec::emit(link, cx, &mut out)?;
    }
    out.sort_by_key(|(pos, _)| *pos);
    Ok(flp::Project {
        header: project.header,
        events: out.into_iter().map(|(_, event)| event).collect(),
    })
}

/// Which of the two events made a [`ControllerLink`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LinkKind {
    /// 226 `ControllerID.MIDI`.
    #[default]
    Midi,
    /// 227 `ControllerID.Remote`.
    Remote,
}

/// One controller link. Each 226 or 227 makes a new one, and a 230 attaches
/// to the most recent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControllerLink {
    /// Whether a 226 or a 227 made the link. This is for reading: the entry
    /// in `order` decides which event gets written.
    pub kind: LinkKind,
    /// 226 `ControllerID.MIDI` or 227 `ControllerID.Remote`: 20 bytes.
    pub link: Option<layout::ControllerLink>,
    /// 230 `ControllerFormula`: string, the link's mapping formula.
    pub formula: Option<String>,
    /// Every event of the link in the order it arrived.
    pub order: Vec<Placed>,
}

object!(ControllerLink {
    one link = ControllerMIDI | ControllerRemote;
    one formula = ControllerFormula;
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ItemSource, PlaylistItem};
    use flp::RawEvent;

    fn item(position: i32, length: i32) -> PlaylistItem {
        PlaylistItem::new(ItemSource::Pattern(1), position, length)
    }

    #[test]
    fn a_blank_project_reads_back_as_itself_and_takes_the_edits() {
        let blank = Project::blank();
        let file = to_flp(&blank).unwrap();
        let read = crate::from_flp(&file);
        assert_eq!(to_flp(&read), Ok(file.clone()));
        assert_eq!(read.tempo(), Some(120.0));
        assert_eq!(read.header.channel_count, 1);
        let channel = &read.channels[..];
        assert_eq!(channel.len(), 1);
        assert_eq!(channel[0].kind, Some(crate::ChannelKind::Sampler));
        assert_eq!(channel[0].name.as_deref(), Some("Sampler"));
        assert_eq!(
            (channel[0].sample_path.as_deref(), channel[0].insert()),
            (None, Some(0))
        );
        assert!(read.patterns.is_empty() && read.arrangements.is_empty());

        // A step makes pattern 1, and a sample makes a second channel.
        let mut edited = read;
        edited.pattern_to_edit(1).set_step(0, 0, true, 96);
        assert_eq!(edited.add_sampler_channel("kick.wav"), Some(1));
        let again = crate::from_flp(&to_flp(&edited).unwrap());
        assert!(again.pattern(1).unwrap().steps(0, 96)[0]);
        let names: Vec<_> = again.channels.iter().map(|c| c.name.clone()).collect();
        assert_eq!(names, [Some("Sampler".to_owned()), Some("kick".to_owned())]);
        assert_eq!(again.header.channel_count, 2);
    }

    fn project(items: &[PlaylistItem]) -> Project {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 0,
            ppq: 96,
        });
        project.arrangements.push(Arrangement {
            items: items.to_vec(),
            ..Arrangement::default()
        });
        project
    }

    fn select(project: &mut Project, arrangement: u16) {
        project.order.push(Placed {
            pos: 0,
            item: Item::Raw(RawEvent {
                id: Id::ArrangementsCurrent.0,
                data: arrangement.to_le_bytes().to_vec(),
            }),
        });
    }

    fn ev(id: Id, data: &[u8]) -> RawEvent {
        RawEvent {
            id: id.0,
            data: data.to_vec(),
        }
    }

    /// A file of these events after a 199 of FL 20.8, read and checked to
    /// write back the same, and what it writes once `edit` has run.
    fn edited(events: &[RawEvent], edit: impl FnOnce(&mut Project)) -> (Project, Vec<RawEvent>) {
        let mut all = vec![ev(Id::FLVersion, b"20.8.4.2576\0")];
        all.extend_from_slice(events);
        let file = flp::Project {
            header: flp::Header {
                format: 0,
                channel_count: 0,
                ppq: 96,
            },
            events: all,
        };
        let mut project = crate::from_flp(&file);
        assert_eq!(to_flp(&project), Ok(file));
        edit(&mut project);
        let written = to_flp(&project).unwrap();
        // What is written reads back as the same events.
        assert_eq!(to_flp(&crate::from_flp(&written)), Ok(written.clone()));
        (crate::from_flp(&written), written.events[1..].to_vec())
    }

    #[test]
    fn the_tempo_goes_into_156_held_to_fl_s_range() {
        let tempo = |milli: u32| ev(Id::Tempo, &milli.to_le_bytes());
        let title = ev(Id::Title, b"Song\0");
        let events = [tempo(140_000), title.clone()];
        let (read, written) = edited(&events, |p| p.set_tempo_milli(128_500));
        assert_eq!(written, [tempo(128_500), title.clone()]);
        assert_eq!(read.tempo(), Some(128.5));
        let (read, _) = edited(&events, |p| p.set_tempo_milli(5));
        assert_eq!(read.tempo(), Some(10.0));
        let (read, _) = edited(&events, |p| p.set_tempo_milli(999_999));
        assert_eq!(read.tempo(), Some(522.0));

        // A file without a tempo gets a 156 behind its 199.
        let (read, written) = edited(std::slice::from_ref(&title), |p| p.set_tempo_milli(90_000));
        assert_eq!(written, [tempo(90_000), title]);
        assert_eq!(read.tempo(), Some(90.0));
    }

    #[test]
    fn an_old_tempo_goes_into_66_and_93_and_a_225_record_too() {
        let word = |id: Id, value: u16| ev(id, &value.to_le_bytes());
        let coarse = word(Id::TempoCoarse, 128);
        let (read, written) = edited(std::slice::from_ref(&coarse), |p| p.set_tempo_milli(99_500));
        let want = [word(Id::TempoCoarse, 99), word(Id::TempoFine, 500)];
        assert_eq!(written, want);
        assert_eq!(read.tempo(), Some(99.5));
        // A whole tempo needs no 93, and one that is there is set.
        let (_, written) = edited(std::slice::from_ref(&coarse), |p| {
            p.set_tempo_milli(100_000)
        });
        assert_eq!(written, [word(Id::TempoCoarse, 100)]);
        let (_, written) = edited(&want, |p| p.set_tempo_milli(100_000));
        assert_eq!(
            written,
            [word(Id::TempoCoarse, 100), word(Id::TempoFine, 0)]
        );

        // Beside a 156 both are set, and so is a record for the tempo.
        let record = |value: i32| {
            let id = 0x4000_0005u32.to_le_bytes();
            ev(Id::MixerParams, &[[0; 4], id, value.to_le_bytes()].concat())
        };
        let both = [
            ev(Id::Tempo, &140_000u32.to_le_bytes()),
            coarse,
            record(140_000),
        ];
        let (read, written) = edited(&both, |p| p.set_tempo_milli(75_250));
        let want = [
            ev(Id::Tempo, &75_250u32.to_le_bytes()),
            word(Id::TempoCoarse, 75),
            word(Id::TempoFine, 250),
            record(75_250),
        ];
        assert_eq!(written, want);
        assert_eq!(read.tempo(), Some(75.25));
    }

    #[test]
    fn the_song_is_as_long_as_its_last_item_ends() {
        let mut muted = item(960, 384);
        muted.set_muted(true);
        let mut deleted = item(5000, 384);
        deleted.source_id = u32::MAX;
        // The item that starts last is not the one that ends last.
        let mut song = project(&[item(0, 1152), item(768, 96), deleted]);
        assert_eq!(song.song_length_ticks(), 1152);
        song.arrangements[0].items.push(muted);
        assert_eq!(song.song_length_ticks(), 1344);
        // An item at a negative position starts at 0.
        assert_eq!(project(&[item(-96, 384)]).song_length_ticks(), 384);
    }

    #[test]
    fn a_project_without_items_has_no_length() {
        let mut song = project(&[]);
        assert_eq!(song.song_length_ticks(), 0);
        song.arrangements.clear();
        assert!(song.current_arrangement().is_none());
        assert_eq!(song.song_length_ticks(), 0);
    }

    #[test]
    fn the_last_100_names_the_current_arrangement() {
        let mut song = project(&[item(0, 384)]);
        song.arrangements.push(Arrangement {
            index: 1,
            items: vec![item(0, 768)],
            ..Arrangement::default()
        });
        assert_eq!(song.current_arrangement().unwrap().index, 0);
        select(&mut song, 1);
        assert_eq!(song.current_arrangement().unwrap().index, 1);
        assert_eq!(song.current_arrangement_mut().unwrap().index, 1);
        assert_eq!(song.song_length_ticks(), 768);
        // An arrangement that is not there leaves the first.
        select(&mut song, 7);
        assert_eq!(song.current_arrangement().unwrap().index, 0);
        assert_eq!(song.song_length_ticks(), 384);
    }
}
