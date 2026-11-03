//! An event stream that no FL wrote never panics the reader, the writer or
//! the views the model offers of what it read.
//!
//! The streams start from one built in code that has an event for most of
//! the model's fields: channels of three kinds, a pattern with notes, an
//! arrangement with playlist items and tracks, inserts with slots, a time
//! marker, controller links and initial values. Each case then drops,
//! duplicates, moves or swaps events, gives an event another ID, and cuts,
//! grows or overwrites payloads, with indices set to their extremes. What
//! [`model::from_flp`] reads must write back through [`model::to_flp`] to
//! the stream it was read from, as the crate promises for any stream.
//!
//! The cases come from a small fixed-seed generator, so a failure repeats.

mod common;

use common::{Rng, fixture, header, mutate};
use flp::RawEvent;
use flp::event::{Id, Kind};
use model::{Note, Project, from_flp, to_flp};

/// Everything the model works out from what it read. None may panic.
fn look_at_everything(project: &Project) {
    let ppq = project.header.ppq;
    let version = project.version();
    let _ = (project.tempo(), project.time_signature());
    let _ = (project.song_length_ticks(), project.legacy_rec_ids());
    let _ = project.main_volume_raw();
    for binding in project.automation_bindings() {
        let _ = binding.target.raw_range();
    }
    for channel in &project.channels {
        let _ = (channel.volume(), channel.pan(), channel.insert());
        let _ = (channel.envelope_on(), channel.time_stretch_on());
        let _ = (channel.tracks_mods(), channel.echo_delay_on());
        let _ = channel.automation_range();
    }
    for pattern in &project.patterns {
        for ppq in [ppq, 1] {
            let steps = pattern.steps(0, ppq);
            assert!(steps.len() <= 1 << 16, "{} steps", steps.len());
        }
        for channel in [0, 1, 2, 500, u16::MAX] {
            for step in [0, 1, 4, 15, u32::MAX] {
                let _ = pattern.step_note(channel, step, ppq);
            }
        }
        for note in &pattern.notes {
            let _ = (note.fine_pitch_cents(), note.slide(), note.color());
        }
    }
    for arrangement in &project.arrangements {
        for item in &arrangement.items {
            let _ = (item.source(), item.track(version), item.trim(), item.clip());
        }
    }
    for value in &project.initial_values {
        let target = project.rec_target(value.rec_id);
        let _ = (target.raw_range(), target.controller_output());
        let _ = target.channel_output();
    }
    for link in project.controller_links.iter().filter_map(|l| l.link) {
        let _ = project.link_target(link.source).controller_output();
        let _ = project.link_target(link.target).raw_range();
    }
    // Each call asks every insert for its destinations, so two will do.
    for index in [0, 1] {
        let _ = project.insert_mix(index).sidechain_sources();
    }
    for position in 0..project.inserts.len() + 2 {
        let _ = project.insert_index(position);
    }
    for index in [0, 1, 2, 125, 126, 127, 500, 501, 502, 9999, u16::MAX] {
        let _ = project.insert_position(index);
        let mix = project.insert_mix(index);
        let _ = (mix.insert(), mix.volume_raw(), mix.pan_raw());
        let _ = (mix.separation_raw(), mix.flag_bits(), mix.enabled());
        let _ = (mix.polarity_reversed(), mix.swapped());
        let _ = mix.destinations();
        for dest in [0, 1, 501, u16::MAX] {
            let _ = mix.send_level_raw(dest);
        }
        for slot in [0, 9, 10, u8::MAX] {
            let _ = (mix.slot_mix_raw(slot), mix.slot_enabled(slot));
        }
        for band in [0, 2, 3, u8::MAX] {
            let _ = mix.eq_band(band);
        }
    }
}

/// Read `events`, look at what came out, and write it back.
fn check(events: Vec<RawEvent>, ppq: u16) {
    let file = flp::Project {
        header: header(ppq),
        events,
    };
    let project = from_flp(&file);
    look_at_everything(&project);
    let written = to_flp(&project);
    // A deleted note record is the one thing the writer leaves out.
    let deleted = |p: &model::Pattern| p.notes.iter().any(Note::is_deleted);
    if !project.patterns.iter().any(deleted) {
        assert_eq!(written.as_ref(), Ok(&file));
    }
}

#[test]
fn the_fixture_reads_and_writes_back() {
    let file = flp::Project {
        header: header(96),
        events: fixture(),
    };
    let project = from_flp(&file);
    assert_eq!(project.channels.len(), 3);
    assert_eq!(project.patterns[0].notes.len(), 3);
    assert_eq!(project.arrangements[0].items.len(), 2);
    assert_eq!(project.inserts[1].slots.len(), 1);
    assert_eq!(project.automation_bindings().len(), 1);
    assert_eq!(to_flp(&project).as_ref(), Ok(&file));
}

#[test]
fn a_stream_with_events_dropped_moved_or_mangled_never_panics() {
    let base = fixture();
    let ids: Vec<u32> = base.iter().map(|e| e.id).collect();
    let mut rng = Rng(11);
    for _ in 0..4000 {
        let mut events = base.clone();
        for _ in 0..1 + rng.below(6) {
            mutate(&mut rng, &mut events, &ids);
        }
        let ppq = [96, 0, 1, 3, 4, u16::MAX][rng.below(6)];
        check(events, ppq);
    }
}

#[test]
fn a_note_at_the_end_of_time_and_a_deleted_one_never_panic() {
    let mut deleted = Note::default();
    (deleted.tag, deleted.channel) = (u16::MAX, u16::MAX);
    let last = Note {
        position: i32::MAX,
        length: i32::MAX,
        ..Note::default()
    };
    let mut events = fixture();
    let notes = events
        .iter_mut()
        .find(|e| e.id == Id::PatternNotes.0)
        .unwrap();
    notes.data.extend(Note::list_to_bytes(&[deleted, last]));
    for ppq in [96, 1, 0] {
        check(events.clone(), ppq);
    }
}

#[test]
fn every_event_alone_and_twice_never_panics() {
    // Each event without the ones that give it an owner, and right after
    // itself.
    for e in fixture() {
        check(vec![e.clone()], 96);
        check(vec![e.clone(), e], 96);
    }
}

#[test]
fn every_prefix_and_suffix_of_the_stream_never_panics() {
    let base = fixture();
    for at in 0..=base.len() {
        check(base[..at].to_vec(), 96);
        check(base[at..].to_vec(), 96);
    }
}

#[test]
fn the_stream_reversed_never_panics() {
    let mut events = fixture();
    events.reverse();
    check(events, 96);
}

#[test]
fn random_events_never_panic() {
    let ids: Vec<u32> = fixture().iter().map(|e| e.id).collect();
    let mut rng = Rng(12);
    for _ in 0..3000 {
        let events = (0..rng.below(30))
            .map(|_| {
                let id = if rng.below(3) == 0 {
                    rng.byte().into()
                } else {
                    ids[rng.below(ids.len())]
                };
                let len = match flp::event::kind(id) {
                    Kind::U8 => 1,
                    Kind::U16 => 2,
                    Kind::U32 => 4,
                    Kind::Var => rng.below(100),
                };
                RawEvent {
                    id,
                    data: rng.bytes(len),
                }
            })
            .collect();
        check(events, 96);
    }
}
