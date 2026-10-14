//! Arrangements, their playlist tracks, and time markers.

use crate::Placed;
use crate::codec::object;
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
