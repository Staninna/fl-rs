//! Arrangements, their playlist tracks, and time markers.

use crate::codec::object;
use crate::rec::RecTarget;
use crate::{ChannelKind, Placed, Project};
use flp::event::layout;

/// One arrangement. 99 `ArrangementID.New` selects it by index.
///
/// No corpus project from before FL 12.9 has a 99, and the playlist, tracks
/// and markers of those files still need a home. They go to an arrangement
/// with index 0 that has no 99 in its `order` (inferred: the loader starts
/// with one arrangement, and a 99 only creates another when its value
/// equals the count).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arrangement {
    /// 99 `ArrangementID.New`: u16 index.
    pub index: u16,
    /// 241 `ArrangementID.Name`: string.
    pub name: Option<String>,
    /// 233 `ArrangementID.Playlist`: the items on the playlist, each 32,
    /// 60, 80 or 88 bytes, one size per file. An item's `len` says which.
    /// [`PlaylistItem::source`](layout::PlaylistItem::source) says what an
    /// item places, and `muted`, `track`, `trim` and `clip` read the rest.
    /// A record FL deleted stays in the list, so skip those that have no
    /// source. Only the arrangement's first 233 is read.
    pub items: Vec<layout::PlaylistItem>,
    /// Tracks in the order their first event arrived.
    pub tracks: Vec<Track>,
    /// Markers closed by a 205 that did not go to a pattern.
    pub time_markers: Vec<TimeMarker>,
    /// Every event of the arrangement in the order it arrived.
    pub order: Vec<Placed>,
}

object!(Arrangement {
    key index = ArrangementNew;
    one name = ArrangementName;
    list items = ArrangementPlaylist;
});

/// Edits of the playlist. An item is named by its place in
/// [`Arrangement::items`], and no edit moves an item to another place: a
/// deleted item stays as the record FL leaves for it, and a copy goes to
/// the end. An ID that names no item, or a record FL deleted, is skipped,
/// and an ID given twice counts once.
///
/// The per-item halves of these are private functions here, and not
/// methods of the item, because [`PlaylistItem`](layout::PlaylistItem) is
/// `flp`'s type.
impl Arrangement {
    /// Move the items along the timeline and `delta_tracks` rows down.
    /// They move as a group that keeps its shape, so the group stops where
    /// the first of its items reaches tick 0, the first track or the last.
    /// `writer_version` is what [`PlaylistItem::track`](layout::PlaylistItem::track)
    /// takes, see [`Project::version`](crate::Project::version).
    pub fn move_items(
        &mut self,
        ids: &[usize],
        delta_ticks: i64,
        delta_tracks: i32,
        writer_version: u32,
    ) {
        let (mut first, mut last) = (i64::MAX, i64::MIN);
        let (mut top, mut bottom) = (i32::MAX, i32::MIN);
        self.each(ids, |item| {
            first = first.min(i64::from(item.position));
            last = last.max(i64::from(item.position));
            top = top.min(item.track(writer_version));
            bottom = bottom.max(item.track(writer_version));
        });
        if first > last {
            return;
        }
        let ticks = delta_ticks
            .min(i64::from(i32::MAX) - last)
            .max(-first.max(0));
        self.each(ids, |item| {
            let tracks = track_count(item, writer_version);
            let rows = delta_tracks.min(tracks - bottom).max(1 - top);
            let track = item.track(writer_version) + rows;
            // An item the file has outside the tracks stays where it is.
            if (1..=tracks).contains(&track) {
                item.track_word = (tracks - track) as i16;
            }
            item.position = (i64::from(item.position) + ticks) as i32;
        });
    }

    /// Give each item this length in ticks, 1 at least, by moving its end,
    /// and keep its trim in step where the trim's unit allows that without
    /// the tempo.
    ///
    /// A trim that is there keeps its start, and its end moves with the
    /// item's. A pattern item that shows its whole pattern gets a trim
    /// from tick 0, which is how FL stores an item cut short. A clip item
    /// without a trim keeps none: its trim would be in milliseconds or
    /// beats of the clip.
    pub fn resize_items(&mut self, ids: &[usize], length: i32) {
        self.each(ids, |item| set_length(item, length.max(1)));
    }

    pub fn mute_items(&mut self, ids: &[usize], muted: bool) {
        self.each(ids, |item| item.set_muted(muted));
    }

    /// Make each record what FL leaves of a deleted item: a source of all
    /// ones. The record stays in the list.
    pub fn delete_items(&mut self, ids: &[usize]) {
        self.each(ids, |item| item.source_id = u32::MAX);
    }

    /// Copy the items. The copies go to the end of the list in the order
    /// of `ids`, so the first copy's ID is the length the list had. They
    /// are selected, and the items they were copied from no longer are.
    pub fn duplicate_items(&mut self, ids: &[usize]) {
        let placed = |id: &usize| self.items.get(*id).filter(|item| item.source().is_some());
        let mut copies: Vec<layout::PlaylistItem> =
            ids.iter().filter_map(placed).copied().collect();
        self.each(ids, |item| set_selected(item, false));
        for copy in &mut copies {
            set_selected(copy, true);
            // FL numbers an item when it is inserted, and -1 is its "no
            // number yet". An item of 32 bytes has no such field.
            if copy.len > 32 {
                copy.id = -1;
            }
        }
        self.items.extend(copies);
    }

    /// Add copies of these items to the end of the list, in their order,
    /// with every field kept: their places, their tracks and their trims.
    /// They become the selection, in place of the items that were
    /// selected: what a paste and a duplicate do.
    ///
    /// An item that places nothing is left out, and so is one of another
    /// size than the items the list has, since a playlist's items are all
    /// of one size ([`PlaylistItem::size_for`](layout::PlaylistItem::size_for)).
    /// That keeps out an item copied from a project of another writer,
    /// whose track word may count from another top too.
    pub fn paste_items(&mut self, items: &[layout::PlaylistItem]) {
        let size = self.items.first().map(|item| item.len);
        let fits = |item: &&layout::PlaylistItem| {
            item.source().is_some() && size.is_none_or(|size| item.len == size)
        };
        let mut copies: Vec<layout::PlaylistItem> = items.iter().filter(fits).copied().collect();
        if copies.is_empty() {
            return;
        }
        for item in &mut self.items {
            set_selected(item, false);
        }
        for copy in &mut copies {
            set_selected(copy, true);
            // FL numbers an item when it is inserted, see duplicate_items.
            if copy.len > 32 {
                copy.id = -1;
            }
        }
        self.items.extend(copies);
    }

    /// Cut each item that tick `at` falls inside of in two at `at`. The
    /// first part keeps the item's place in the list and ends at `at`. The
    /// second is a copy that goes to the end of the list, in the order of
    /// `ids`, starts at `at` and shows its source from where the first part
    /// stops, so that the two play what the item did. Both keep the
    /// item's selection.
    ///
    /// A pattern item's trim is in ticks of the pattern, and one without a
    /// trim shows the pattern from tick 0, so the second part's trim starts
    /// as many ticks in as the first part is long. A clip item's trim is
    /// in the clip's own unit ([`Trim::Clip`](layout::Trim::Clip)), and one
    /// with a trim is cut where its share of the length is, as
    /// [`Arrangement::resize_items`] keeps it. A clip item without a trim
    /// starts at 0 of the clip, and `clip_rate` says how many of the clip's
    /// units a tick is for it; `None` leaves the item whole.
    pub fn split_items(
        &mut self,
        ids: &[usize],
        at: i32,
        clip_rate: impl Fn(&layout::PlaylistItem) -> Option<f32>,
    ) {
        let mut seen = Vec::new();
        let mut pieces = Vec::new();
        for &id in ids {
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
            let item = self.items.get_mut(id);
            let Some(item) = item.filter(|item| item.source().is_some()) else {
                continue;
            };
            let cut = i64::from(at) - i64::from(item.position);
            if cut <= 0 || cut >= i64::from(item.length) {
                continue;
            }
            let cut = cut as i32;
            let Some((first, second)) = split_trim(item, cut, &clip_rate) else {
                continue;
            };
            let mut piece = *item;
            piece.position = at;
            piece.length = item.length - cut;
            piece.set_trim(Some(second));
            // FL numbers an item when it is inserted, see duplicate_items.
            if piece.len > 32 {
                piece.id = -1;
            }
            item.length = cut;
            item.set_trim(Some(first));
            pieces.push(piece);
        }
        self.items.extend(pieces);
    }

    /// Whether the track with this number, where the first is 1, plays:
    /// see [`Track::enabled`]. A track the arrangement has no record for
    /// plays.
    pub fn track_enabled(&self, track: i32) -> bool {
        let found = self
            .tracks
            .iter()
            .find(|t| i64::from(t.index) == i64::from(track));
        found.is_none_or(Track::enabled)
    }

    /// Switch the track with this number, where the first is 1, on or
    /// off, in its 238's byte at @12. A track without a 238 is skipped:
    /// every track of a file from FL 12.9 on has one, and no event of an
    /// older file holds the switch.
    pub fn set_track_enabled(&mut self, track: i32, enabled: bool) {
        let found = self
            .tracks
            .iter_mut()
            .find(|t| i64::from(t.index) == i64::from(track));
        if let Some(data) = found.and_then(|t| t.data.as_mut()) {
            data.unknown_12 = [u8::from(enabled)];
        }
    }

    /// Select the items, on top of the selection when `add` is set and in
    /// its place when not.
    pub fn select_items(&mut self, ids: &[usize], add: bool) {
        if !add {
            for item in &mut self.items {
                set_selected(item, false);
            }
        }
        self.each(ids, |item| set_selected(item, true));
    }

    /// Change each item `ids` names that places something, once.
    fn each(&mut self, ids: &[usize], mut change: impl FnMut(&mut layout::PlaylistItem)) {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let item = self.items.get_mut(id);
            if let Some(item) = item.filter(|item| item.source().is_some()) {
                change(item);
            }
        }
    }
}

/// The trims of the two parts [`Arrangement::split_items`] cuts an item
/// into, `cut` ticks from its start, which is inside it.
///
/// A clip item's second part starts where playback is at the cut: its
/// trim's start, or 0, plus `cut` ticks at the rate `clip_rate` gives,
/// since the engine plays a clip from its start alone and at that rate
/// (`crates/engine/src/sequence.rs`). Its share of the trim would be wrong
/// whenever the trim's span was set at another tempo.
fn split_trim(
    item: &layout::PlaylistItem,
    cut: i32,
    clip_rate: impl Fn(&layout::PlaylistItem) -> Option<f32>,
) -> Option<(layout::Trim, layout::Trim)> {
    use layout::{ItemSource, Trim};
    let ticks = |start: i32, end: i32| {
        let middle = start.saturating_add(cut);
        Some((
            Trim::Ticks { start, end: middle },
            Trim::Ticks { start: middle, end },
        ))
    };
    match item.trim() {
        Some(Trim::Ticks { start, end }) => ticks(start, end),
        None if matches!(item.source(), Some(ItemSource::Pattern(_))) => ticks(0, item.length),
        trim => {
            let rate = clip_rate(item)?;
            let (start, end) = match trim {
                Some(Trim::Clip { start, end }) => (start, end),
                _ => (0.0, item.length as f32 * rate),
            };
            let middle = start + cut as f32 * rate;
            Some((
                Trim::Clip { start, end: middle },
                Trim::Clip { start: middle, end },
            ))
        }
    }
}

impl Project {
    /// [`Arrangement::split_items`] on the arrangement FL shows.
    ///
    /// A clip item counts beats when its channel is an automation clip, and
    /// milliseconds of the sample at the project's tempo when it is an audio
    /// clip that plays at its own speed: not stretched, pitched or reversed,
    /// on a channel whose pitch knob and fine tune are at 0, in a project
    /// where no link drives the tempo or that channel, which is what
    /// `end - start` of most corpus audio items with a trim comes to
    /// ([`Trim::Clip`](layout::Trim::Clip)). Any other clip item, and an
    /// audio item in a project without a tempo, is left whole, since where
    /// its sample is at the cut depends on more than the tempo.
    pub fn split_items(&mut self, ids: &[usize], at: i32) {
        let ppq = f32::from(self.header.ppq.max(1));
        let tempo = self.tempo().filter(|tempo| *tempo > 0.0);
        let targets: Vec<RecTarget> = (self.controller_links.iter())
            .filter_map(|link| link.link)
            .map(|link| self.link_target(link.target))
            .collect();
        let tempo_moves = targets
            .iter()
            .any(|t| matches!(t, RecTarget::Global { .. }));
        let moves = |channel: u16| {
            let own =
                |t: &RecTarget| matches!(t, RecTarget::Channel { index, .. } if *index == channel);
            tempo_moves || targets.iter().any(own)
        };
        let channels: Vec<(u16, Option<ChannelKind>, bool)> = self
            .channels
            .iter()
            .map(|channel| {
                let knob = channel
                    .levels
                    .map_or(0, |l| i32::from_le_bytes(l.unknown_8));
                let tuned = knob != 0 || channel.fine_tune.unwrap_or(0) != 0;
                (channel.index, channel.kind, tuned || moves(channel.index))
            })
            .collect();
        let clip_rate = |item: &layout::PlaylistItem| {
            let Some(layout::ItemSource::Channel(index)) = item.source() else {
                return None;
            };
            let (_, kind, varies) = channels.iter().find(|(at, ..)| *at == index)?;
            let settings = item.clip();
            let plain = settings.stretch == 1.0 && settings.pitch == 0.0 && !settings.reversed;
            match kind {
                Some(ChannelKind::Automation) => Some(1.0 / ppq),
                Some(ChannelKind::AudioClip) if plain && !varies => {
                    Some((60_000.0 / tempo?) as f32 / ppq)
                }
                _ => None,
            }
        };
        if let Some(arrangement) = self.current_arrangement_mut() {
            arrangement.split_items(ids, at, clip_rate);
        }
    }
}

/// How many tracks a playlist of this writer has: 199 or 500. The track
/// word counts down from there.
fn track_count(item: &layout::PlaylistItem, writer_version: u32) -> i32 {
    item.track(writer_version) + i32::from(item.track_word)
}

fn set_selected(item: &mut layout::PlaylistItem, selected: bool) {
    let bit = layout::PlaylistItem::SELECTED;
    item.flags = item.flags & !bit | if selected { bit } else { 0 };
}

/// See [`Arrangement::resize_items`].
fn set_length(item: &mut layout::PlaylistItem, length: i32) {
    use layout::{ItemSource, Trim};
    let old = item.length;
    item.length = length;
    let trim = match item.trim() {
        Some(Trim::Ticks { start, .. }) => Trim::Ticks {
            start,
            end: start.saturating_add(length),
        },
        Some(Trim::Clip { start, end }) if old > 0 => Trim::Clip {
            start,
            end: start + (end - start) * (length as f32 / old as f32),
        },
        None if matches!(item.source(), Some(ItemSource::Pattern(_))) => Trim::Ticks {
            start: 0,
            end: length,
        },
        _ => return,
    };
    item.set_trim(Some(trim));
}

/// One playlist track. 238 `TrackID.Data` makes its track the current one,
/// and 239 and 43 then address it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    /// The index the track is addressed by, 238 @0. `data.index` is what
    /// gets written, so change both together.
    pub index: u32,
    /// 238 `TrackID.Data`: struct of 13 to 70 bytes. `None` for a track that
    /// only a 239 or 43 touched.
    pub data: Option<layout::Track>,
    /// 239 `TrackID.Name`: string.
    pub name: Option<String>,
    /// 43 `TrackColorIsCustom`: bool.
    pub color_is_custom: Option<bool>,
    /// Every event of the track in the order it arrived.
    pub order: Vec<Placed>,
}

object!(Track {
    one data = TrackData;
    one name = TrackName;
    one color_is_custom = TrackColorIsCustom;
});

impl Track {
    /// Whether the track plays: the byte at @12 of its 238, which the
    /// loader keeps at `trk+0x3c` (`docs/event-dictionary.md`, "238") and
    /// PyFLP reads as the track's `enabled` flag. That it is the switch
    /// in front of the track's name, and that FL plays none of a track's
    /// items while it is 0, are PyFLP's name and inferred. A track without
    /// a 238 plays.
    pub fn enabled(&self) -> bool {
        self.data.is_none_or(|data| data.unknown_12 != [0])
    }
}

/// One time marker. 205 `TimeMarkerID.Name` closes it and takes along the
/// marker events since the previous 205.
///
/// FL's loader does not clear those values between markers. A field that is
/// `None` here was not in the file for this marker, and FL then reuses what
/// the marker before it had.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimeMarker {
    /// 205 `TimeMarkerID.Name`: string.
    pub name: Option<String>,
    /// 148 `TimeMarkerID.Position`: u32, position in the low 24 bits and
    /// marker kind in the high 8.
    pub position: Option<u32>,
    /// 33 `TimeMarkerID.Numerator`: u8.
    pub numerator: Option<u8>,
    /// 34 `TimeMarkerID.Denominator`: u8.
    pub denominator: Option<u8>,
    /// Every event of the marker in the order it arrived.
    pub order: Vec<Placed>,
}

object!(TimeMarker {
    one name = TimeMarkerName;
    one position = TimeMarkerPosition;
    one numerator = TimeMarkerNumerator;
    one denominator = TimeMarkerDenominator;
});

#[cfg(test)]
mod tests {
    use super::*;
    use layout::{ItemSource, PlaylistItem, Trim};

    /// What FL 24 writes, as [`PlaylistItem::track`] takes it.
    const VERSION: u32 = 24_001_001;

    fn arrangement(items: Vec<PlaylistItem>) -> Arrangement {
        Arrangement {
            items,
            ..Arrangement::default()
        }
    }

    /// A pattern item on a track, where the first track is 1.
    fn item(track: i16, position: i32) -> PlaylistItem {
        let mut item = PlaylistItem::new(ItemSource::Pattern(1), position, 384);
        item.track_word = 500 - track;
        item
    }

    /// Where each item is: its position and its track.
    fn places(arrangement: &Arrangement) -> Vec<(i32, i32)> {
        let place = |item: &PlaylistItem| (item.position, item.track(VERSION));
        arrangement.items.iter().map(place).collect()
    }

    #[test]
    fn a_group_moves_as_one_and_stops_at_the_edges() {
        let mut arrangement = arrangement(vec![item(2, 96), item(3, 480), item(9, 0)]);
        arrangement.move_items(&[0, 1], 192, 2, VERSION);
        assert_eq!(places(&arrangement), [(288, 4), (672, 5), (0, 9)]);

        // The first item stops at tick 0 and on track 1, and the second
        // keeps its distance. An ID given twice moves its item once.
        arrangement.move_items(&[1, 0, 1], -1000, -10, VERSION);
        assert_eq!(places(&arrangement), [(0, 1), (384, 2), (0, 9)]);

        // Track 500 is the last one.
        arrangement.move_items(&[0, 1], 0, 1000, VERSION);
        assert_eq!(places(&arrangement), [(0, 499), (384, 500), (0, 9)]);
        assert_eq!(arrangement.items[1].track_word, 0);
    }

    #[test]
    fn a_writer_before_12_9_1_has_199_tracks() {
        let mut old = PlaylistItem::new(ItemSource::Pattern(1), 0, 384);
        old.track_word = 198;
        let mut arrangement = arrangement(vec![old]);
        arrangement.move_items(&[0], 0, 1000, 12_003_000);
        assert_eq!(arrangement.items[0].track_word, 0);
        assert_eq!(arrangement.items[0].track(12_003_000), 199);
    }

    #[test]
    fn a_resize_moves_the_end_of_the_trim_along() {
        let untrimmed = item(1, 0);
        let mut trimmed = item(1, 0);
        trimmed.set_trim(Some(Trim::Ticks {
            start: 96,
            end: 480,
        }));
        let mut clip = PlaylistItem::new(ItemSource::Channel(3), 0, 384);
        clip.set_trim(Some(Trim::Clip {
            start: 1.0,
            end: 5.0,
        }));
        let whole_clip = PlaylistItem::new(ItemSource::Channel(3), 0, 384);
        let mut arrangement = arrangement(vec![untrimmed, trimmed, clip, whole_clip]);

        arrangement.resize_items(&[0, 1, 2, 3], 192);
        let items = &arrangement.items;
        assert!(items.iter().all(|item| item.length == 192));
        let trims: Vec<_> = items.iter().map(PlaylistItem::trim).collect();
        assert_eq!(
            trims,
            [
                Some(Trim::Ticks { start: 0, end: 192 }),
                Some(Trim::Ticks {
                    start: 96,
                    end: 288
                }),
                // Half the length shows half as many beats.
                Some(Trim::Clip {
                    start: 1.0,
                    end: 3.0
                }),
                None,
            ]
        );
        // No item gets shorter than a tick.
        arrangement.resize_items(&[0], -5);
        assert_eq!(arrangement.items[0].length, 1);
    }

    #[test]
    fn mute_delete_and_select_change_the_record_in_place() {
        let mut arrangement = arrangement(vec![item(1, 0), item(2, 0), item(3, 0)]);
        arrangement.mute_items(&[0, 2], true);
        let muted: Vec<bool> = arrangement.items.iter().map(PlaylistItem::muted).collect();
        assert_eq!(muted, [true, false, true]);

        let selected = |arrangement: &Arrangement| -> Vec<bool> {
            let items = arrangement.items.iter();
            items.map(PlaylistItem::selected).collect()
        };
        arrangement.select_items(&[0], false);
        arrangement.select_items(&[1], true);
        assert_eq!(selected(&arrangement), [true, true, false]);
        arrangement.select_items(&[2], false);
        assert_eq!(selected(&arrangement), [false, false, true]);

        arrangement.delete_items(&[1]);
        assert_eq!(arrangement.items.len(), 3);
        assert!(arrangement.items[1].is_deleted());
        // A deleted record is no longer there to edit, and neither is an
        // ID past the end of the list.
        let before = arrangement.clone();
        arrangement.move_items(&[1, 7], 96, 1, VERSION);
        arrangement.mute_items(&[1, 7], true);
        assert_eq!(arrangement, before);
    }

    /// A 238 of 70 bytes for track `index`, switched on.
    fn track(index: u32) -> Track {
        let mut bytes = vec![0; 70];
        bytes[..4].copy_from_slice(&index.to_le_bytes());
        bytes[12] = 1;
        Track {
            index,
            data: layout::Track::from_bytes(&bytes),
            ..Track::default()
        }
    }

    #[test]
    fn a_track_s_switch_is_the_byte_at_12_of_its_238() {
        let mut arrangement = arrangement(Vec::new());
        let named = Track {
            index: 3,
            name: Some("Vox".to_owned()),
            ..Track::default()
        };
        arrangement.tracks = vec![track(1), track(2), named];
        arrangement.set_track_enabled(2, false);
        assert_eq!(arrangement.tracks[1].data.unwrap().unknown_12, [0]);
        let on: Vec<bool> = (1..=4).map(|t| arrangement.track_enabled(t)).collect();
        assert_eq!(on, [true, false, true, true]);
        arrangement.set_track_enabled(2, true);
        assert_eq!(arrangement.tracks[1].data.unwrap().unknown_12, [1]);

        // A track without a 238 has no byte to switch, and neither has one
        // the arrangement does not have.
        let before = arrangement.clone();
        arrangement.set_track_enabled(3, false);
        arrangement.set_track_enabled(9, false);
        assert_eq!(arrangement, before);
    }

    #[test]
    fn a_paste_adds_selected_copies_at_the_end_of_the_list() {
        let mut first = item(1, 0);
        first.flags |= PlaylistItem::SELECTED;
        first.id = 3;
        let mut arrangement = arrangement(vec![first, item(2, 96)]);
        let mut copy = item(4, 768);
        copy.id = 3;
        let mut deleted = item(1, 0);
        deleted.source_id = u32::MAX;
        let short = item(5, 0).shortened(32).unwrap();
        arrangement.paste_items(&[copy, deleted, short]);
        // The copy keeps its place and its track, and gets FL's "no number
        // yet". A deleted record and an item of another size stay out.
        assert_eq!(places(&arrangement), [(0, 1), (96, 2), (768, 4)]);
        assert_eq!(arrangement.items[2].id, -1);
        let selected: Vec<bool> = arrangement
            .items
            .iter()
            .map(PlaylistItem::selected)
            .collect();
        assert_eq!(selected, [false, false, true]);

        // Nothing to paste leaves the selection as it was.
        let before = arrangement.clone();
        arrangement.paste_items(&[deleted]);
        assert_eq!(arrangement, before);
    }

    #[test]
    fn a_split_leaves_two_items_that_show_what_the_one_did() {
        let mut trimmed = item(2, 384);
        trimmed.set_trim(Some(Trim::Ticks {
            start: 96,
            end: 480,
        }));
        trimmed.id = 7;
        let mut clip = PlaylistItem::new(ItemSource::Channel(3), 0, 384);
        clip.set_trim(Some(Trim::Clip {
            start: 1.0,
            end: 5.0,
        }));
        clip.flags |= PlaylistItem::SELECTED;
        let untrimmed_clip = PlaylistItem::new(ItemSource::Channel(4), 0, 384);
        let mut deleted = item(4, 0);
        deleted.source_id = u32::MAX;
        let mut arrangement = arrangement(vec![item(1, 0), trimmed, clip, untrimmed_clip, deleted]);
        // Channel 4's clip counts a unit a tick and channel 3's a beat at
        // PPQ 96, and the deleted record and an ID given twice are skipped.
        let rate = |item: &PlaylistItem| match item.source() {
            Some(ItemSource::Channel(4)) => Some(1.0),
            Some(ItemSource::Channel(3)) => Some(1.0 / 96.0),
            _ => None,
        };
        arrangement.split_items(&[2, 1, 0, 4, 0, 3, 9], 480, rate);

        let items = &arrangement.items;
        let parts: Vec<_> = items
            .iter()
            .map(|item| (item.position, item.length))
            .collect();
        assert_eq!(
            parts,
            [
                // Each first part ends at the cut.
                (0, 384),
                (384, 96),
                (0, 384),
                (0, 384),
                (0, 384),
                // The second parts follow in the order of the IDs. Item 0
                // ends where the cut is, so it stays whole, and so do the
                // clips, which end before it.
                (480, 288),
            ]
        );
        assert_eq!(
            items[1].trim(),
            Some(Trim::Ticks {
                start: 96,
                end: 192
            })
        );
        assert_eq!(
            items[5].trim(),
            Some(Trim::Ticks {
                start: 192,
                end: 480
            })
        );
        assert_eq!(items[5].track(VERSION), 2);
        // The copy gets FL's "no number yet".
        assert_eq!((items[1].id, items[5].id), (7, -1));

        // Cut inside every item: a pattern without a trim starts its second
        // part as far into the pattern as the cut, and a clip's second part
        // starts the cut's ticks at its rate past its trim's start, or 0.
        let mut arrangement = arrangement.clone();
        arrangement.items.truncate(5);
        arrangement.split_items(&[0, 2, 3], 96, rate);
        let trims: Vec<_> = arrangement.items[5..]
            .iter()
            .map(|item| (item.position, item.length, item.trim()))
            .collect();
        assert_eq!(
            trims,
            [
                (
                    96,
                    288,
                    Some(Trim::Ticks {
                        start: 96,
                        end: 384
                    })
                ),
                (
                    96,
                    288,
                    Some(Trim::Clip {
                        start: 2.0,
                        end: 5.0
                    })
                ),
                (
                    96,
                    288,
                    Some(Trim::Clip {
                        start: 96.0,
                        end: 384.0
                    })
                ),
            ]
        );
        let items = &arrangement.items;
        assert_eq!(items[0].trim(), Some(Trim::Ticks { start: 0, end: 96 }));
        assert_eq!(
            items[2].trim(),
            Some(Trim::Clip {
                start: 1.0,
                end: 2.0
            })
        );
        assert_eq!(
            items[3].trim(),
            Some(Trim::Clip {
                start: 0.0,
                end: 96.0
            })
        );
        // The pieces keep the selection the item had.
        assert!(items[2].selected() && items[6].selected());
        assert!(!items[0].selected() && !items[5].selected());

        // A clip the rate says nothing about stays whole.
        let mut whole = arrangement.clone();
        whole.split_items(&[3], 200, |_| None);
        assert_eq!(whole, arrangement);
    }

    #[test]
    fn an_untrimmed_clip_is_split_in_beats_or_milliseconds_at_the_tempo() {
        use crate::Channel;
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 3,
            ppq: 96,
        });
        project.tempo_milli = Some(120_000);
        let channel = |index, kind| Channel {
            index,
            kind: Some(kind),
            ..Channel::default()
        };
        project.channels = vec![
            channel(0, ChannelKind::Automation),
            channel(1, ChannelKind::AudioClip),
            channel(2, ChannelKind::Sampler),
        ];
        let clip = |index| PlaylistItem::new(ItemSource::Channel(index), 0, 384);
        let mut stretched = clip(1);
        stretched.stretch = 2.0;
        project
            .arrangements
            .push(arrangement(vec![clip(0), clip(1), clip(2), stretched]));

        project.split_items(&[0, 1, 2, 3], 192);
        let items = &project.arrangements[0].items;
        assert_eq!(items.len(), 6);
        // Two beats of the automation clip, and at 120 BPM two beats are a
        // second of the sample.
        assert_eq!(
            items[4].trim(),
            Some(Trim::Clip {
                start: 2.0,
                end: 4.0
            })
        );
        assert_eq!(
            items[5].trim(),
            Some(Trim::Clip {
                start: 1000.0,
                end: 2000.0
            })
        );
        // A sampler places no clip, and a stretched one has no rate here.
        assert_eq!((items[2].length, items[3].length), (384, 384));

        // Without a tempo the audio clip stays whole.
        project.tempo_milli = None;
        project.arrangements[0].items = vec![clip(1)];
        project.split_items(&[0], 192);
        assert_eq!(project.arrangements[0].items.len(), 1);
    }

    #[test]
    fn a_trimmed_audio_clip_is_cut_where_it_plays_and_one_that_varies_stays_whole() {
        use crate::{Channel, ControllerLink};
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 2,
            ppq: 96,
        });
        // Trimmed to two seconds at 120 BPM, and played at 60: a beat in,
        // the sample is a second in, not a quarter of the trim.
        project.tempo_milli = Some(60_000);
        project.channels = vec![Channel {
            index: 1,
            kind: Some(ChannelKind::AudioClip),
            ..Channel::default()
        }];
        let mut clip = PlaylistItem::new(ItemSource::Channel(1), 0, 384);
        clip.set_trim(Some(Trim::Clip {
            start: 0.0,
            end: 2000.0,
        }));
        project.arrangements.push(arrangement(vec![clip]));
        let before = project.clone();
        project.split_items(&[0], 96);
        let items = &project.arrangements[0].items;
        assert_eq!(
            items[1].trim(),
            Some(Trim::Clip {
                start: 1000.0,
                end: 2000.0
            })
        );

        // Reversed, tuned on its channel, or with a link to the tempo, the
        // clip's place at the cut is not the tempo's alone.
        let whole = |change: &dyn Fn(&mut Project)| {
            let mut project = before.clone();
            change(&mut project);
            project.split_items(&[0], 96);
            project.arrangements[0].items.len() == 1
        };
        assert!(whole(
            &|p| p.arrangements[0].items[0].clip_flags |= PlaylistItem::REVERSED
        ));
        assert!(whole(&|p| p.channels[0].fine_tune = Some(50)));
        assert!(whole(&|p| {
            let link = layout::ControllerLink {
                source: 0,
                unknown_4: [0; 4],
                target: 0x4000_0000 | 5,
                flags: 8,
                unknown_16: [0; 4],
            };
            p.controller_links.push(ControllerLink {
                link: Some(link),
                ..ControllerLink::default()
            });
        }));
        assert!(!whole(&|_| ()));
    }

    #[test]
    fn copies_go_to_the_end_and_take_the_selection() {
        let mut first = item(1, 0);
        first.id = 41;
        let mut arrangement = arrangement(vec![first, item(2, 96), item(3, 192)]);
        arrangement.duplicate_items(&[2, 0]);

        assert_eq!(
            places(&arrangement),
            [(0, 1), (96, 2), (192, 3), (192, 3), (0, 1)]
        );
        let items = &arrangement.items;
        let selected: Vec<bool> = items.iter().map(PlaylistItem::selected).collect();
        assert_eq!(selected, [false, false, false, true, true]);
        assert_eq!((items[0].id, items[4].id), (41, -1));
    }
}
