//! Read every project in the FL Studio corpus into the model, write it back,
//! and compare with what `flp` parsed.
//!
//! The corpus is FL Studio's own `Data/` directory, which stays out of this
//! repo. Point `FL_RS_CORPUS` at it. Without the variable the tests pass and
//! say they were skipped. Presets (`.fst`) are not read here.

use flp::RawEvent;
use flp::event::Id;
use model::{
    Arrangement, Channel, ChannelKind, ControllerLink, Insert, Item, ItemSource, Pattern, Placed,
    Project, RecTarget, Slot, TimeMarker, Track, Trim,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn find(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            find(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("flp"))
        {
            out.push(path);
        }
    }
}

/// One corpus project.
struct Case {
    /// Path under the corpus root.
    name: String,
    bytes: Vec<u8>,
    /// What `flp` parsed out of the bytes.
    file: flp::Project,
    /// The model read from that.
    project: Project,
}

fn cases() -> Option<Vec<Case>> {
    let root = PathBuf::from(std::env::var_os("FL_RS_CORPUS")?);
    assert!(
        root.is_dir(),
        "FL_RS_CORPUS={} is not a directory",
        root.display()
    );
    let mut files = Vec::new();
    find(&root, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no .flp files under {}", root.display());
    let case = |path: &PathBuf| {
        let name = path.strip_prefix(&root).unwrap().display().to_string();
        let bytes = std::fs::read(path).unwrap();
        let file = flp::Project::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let project = model::from_flp(&file);
        Case {
            name,
            bytes,
            file,
            project,
        }
    };
    Some(files.iter().map(case).collect())
}

/// Every `order` list in a project, with the IDs its owner has fields for.
fn orders(p: &Project) -> Vec<(&'static [Id], &[Placed])> {
    let mut out = vec![(Project::IDS, &p.order[..])];
    out.extend(p.channels.iter().map(|c| (Channel::IDS, &c.order[..])));
    for pattern in &p.patterns {
        out.push((Pattern::IDS, &pattern.order));
        let markers = pattern.time_markers.iter();
        out.extend(markers.map(|m| (TimeMarker::IDS, &m.order[..])));
    }
    for arrangement in &p.arrangements {
        out.push((Arrangement::IDS, &arrangement.order));
        out.extend(
            arrangement
                .tracks
                .iter()
                .map(|t| (Track::IDS, &t.order[..])),
        );
        let markers = arrangement.time_markers.iter();
        out.extend(markers.map(|m| (TimeMarker::IDS, &m.order[..])));
    }
    for insert in &p.inserts {
        out.push((Insert::IDS, &insert.order));
        out.extend(insert.slots.iter().map(|s| (Slot::IDS, &s.order[..])));
    }
    let links = p.controller_links.iter();
    out.extend(links.map(|l| (ControllerLink::IDS, &l.order[..])));
    out
}

fn count(events: &[RawEvent], id: Id) -> usize {
    events.iter().filter(|e| e.id == id.0).count()
}

#[test]
fn every_project_round_trips_through_the_model() {
    let Some(cases) = cases() else {
        eprintln!("FL_RS_CORPUS unset, model round trip skipped");
        return;
    };
    let mut failures = Vec::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let (mut patterns, mut notes, mut inserts, mut items) = (0, 0, 0, 0);
    let (mut typed, mut raw) = (0, 0);
    // A raw event with an ID its owner has a field for: a later event
    // replaced it, or the payload would not write back the same.
    let mut passed_over: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for Case {
        name,
        bytes,
        file,
        project,
    } in &cases
    {
        match model::to_flp(project) {
            Err(e) => failures.push(format!("{name}: {e}")),
            Ok(written) => {
                if let Some(at) = (0..file.events.len().max(written.events.len()))
                    .find(|&i| file.events.get(i) != written.events.get(i))
                {
                    failures.push(format!("{name}: events differ from index {at}"));
                } else if written.write() != *bytes {
                    failures.push(format!("{name}: bytes differ"));
                }
            }
        }
        for channel in &project.channels {
            let kind = match channel.kind {
                Some(ChannelKind::Sampler) => "sampler".to_string(),
                Some(ChannelKind::Plugin) => "plugin".to_string(),
                Some(ChannelKind::Layer) => "layer".to_string(),
                Some(ChannelKind::AudioClip) => "audio clip".to_string(),
                Some(ChannelKind::Automation) => "automation".to_string(),
                Some(ChannelKind::Other(n)) => format!("type {n}"),
                None => "no type".to_string(),
            };
            *kinds.entry(kind).or_default() += 1;
        }
        patterns += project.patterns.len();
        notes += project
            .patterns
            .iter()
            .map(|p| p.notes.len())
            .sum::<usize>();
        inserts += project.inserts.len();
        let playlists = project.arrangements.iter().map(|a| a.items.len());
        items += playlists.sum::<usize>();
        for (ids, order) in orders(project) {
            for placed in order {
                match &placed.item {
                    Item::Field(_) => typed += 1,
                    Item::Raw(e) => {
                        raw += 1;
                        let id = Id::from(e.id);
                        if ids.contains(&id) {
                            let what = format!("{} {}", e.id, id.name().unwrap_or("?"));
                            let entry = passed_over.entry(what).or_insert((0, name.clone()));
                            entry.0 += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} projects failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
    let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} {n}")).collect();
    eprintln!(
        "{} projects, channels: {}, patterns {patterns}, notes {notes}, inserts {inserts}, \
         playlist items {items}",
        cases.len(),
        kinds.join(", "),
    );
    eprintln!(
        "{typed} events in typed fields, {raw} raw ({:.1}% typed)",
        100.0 * typed as f64 / (typed + raw) as f64
    );
    for (what, (n, first)) in &passed_over {
        eprintln!("raw despite a field: {what} x{n}, first in {first}");
    }
    // One project has cut-off emoji in its comments, and thirteen FL 11
    // channels have a 203 name that a later 192 replaces.
    let passed_over: Vec<(&str, usize)> = passed_over
        .iter()
        .map(|(what, (n, _))| (what.as_str(), *n))
        .collect();
    assert_eq!(passed_over, [("195 Comments", 1), ("203 PluginName", 13)]);
}

/// What `docs/event-dictionary.md` promises about a project, asked of the
/// model.
#[test]
fn the_model_holds_what_the_dictionary_promises() {
    let Some(cases) = cases() else {
        eprintln!("FL_RS_CORPUS unset, dictionary promises skipped");
        return;
    };
    let mut failures = Vec::new();
    for Case {
        name,
        file,
        project,
        ..
    } in &cases
    {
        let mut check = |ok: bool, what: String| {
            if !ok {
                failures.push(format!("{name}: {what}"));
            }
        };
        let events = &file.events;

        // 156 (or the old 66) and 199 are in every project.
        check(project.tempo().is_some(), "no tempo".into());
        check(project.writer_version.is_some(), "no writer version".into());

        // 64 carries a dense index, and a 21 from the range 0..5 follows.
        check(
            project.channels.len() == count(events, Id::ChannelNew),
            "a 64 selected a channel twice".into(),
        );
        check(
            project.channels.len() == usize::from(project.header.channel_count),
            "channels differ from the header count".into(),
        );
        for (n, channel) in project.channels.iter().enumerate() {
            let at = format!("channel {}", channel.index);
            check(
                usize::from(channel.index) == n,
                format!("{at} out of order"),
            );
            check(
                !matches!(channel.kind, None | Some(ChannelKind::Other(_))),
                format!("{at} has type {:?}", channel.kind),
            );
            // What the type names in `ChannelKind` rest on.
            let is = |kind| channel.kind == Some(kind);
            check(
                channel.automation.is_some() == is(ChannelKind::Automation),
                format!("{at}: 234 and type 5 disagree"),
            );
            check(
                channel.children.is_empty() || is(ChannelKind::Layer),
                format!("{at}: 94 outside a layer"),
            );
            check(
                !is(ChannelKind::Plugin) || channel.plugin_state.is_some(),
                format!("{at}: a plugin without 213"),
            );
            // 218 and 228 are positional per channel.
            check(
                channel.tracking.len() == 2,
                format!("{at}: {} tracking structs", channel.tracking.len()),
            );
            check(
                channel.envelopes.len() == 5,
                format!("{at}: {} envelopes", channel.envelopes.len()),
            );
        }

        // 147 closes one insert each, and 103 counts them where present.
        check(
            project.inserts.len() == count(events, Id::InsertOutput),
            "inserts differ from the 147 count".into(),
        );
        check(
            project.inserts.iter().all(|i| i.output.is_some()),
            "an insert without its 147".into(),
        );
        if let Some(n) = project.insert_count {
            check(
                project.inserts.len() == usize::from(n),
                format!("{} inserts, 103 says {n}", project.inserts.len()),
            );
        }

        // A 212 names its insert and slot. The insert is the current one,
        // in the loader's numbering where the last insert is 501.
        let last = project.inserts.len().saturating_sub(1);
        for (n, insert) in project.inserts.iter().enumerate() {
            for slot in &insert.slots {
                let at = format!("insert {n} slot {}", slot.index);
                check((0..10).contains(&slot.index), format!("{at} out of range"));
                let Some(wrapper) = slot.wrapper else {
                    check(slot.plugin_name.is_none(), format!("{at}: 201 without 212"));
                    continue;
                };
                check(wrapper.slot == slot.index, format!("{at}: wrapper slot"));
                let named = usize::try_from(wrapper.insert).ok();
                check(
                    named == Some(n) || (n == last && named == Some(501)),
                    format!("{at}: wrapper names insert {}", wrapper.insert),
                );
            }
        }

        // 224 holds 24-byte records, and every one of them is a note.
        let payloads = events.iter().filter(|e| e.id == Id::PatternNotes.0);
        let in_file: Vec<usize> = payloads.map(|e| e.data.len()).collect();
        check(
            in_file.iter().all(|len| len % 24 == 0),
            "a 224 is not whole notes".into(),
        );
        let in_model: usize = project.patterns.iter().map(|p| p.notes.len()).sum();
        check(
            in_model == in_file.iter().sum::<usize>() / 24,
            format!("{in_model} notes for {:?} bytes of 224", in_file),
        );

        // 233 items have the size the version and 169 give.
        let playlists = count(events, Id::ArrangementPlaylist);
        let read: usize = orders(project)
            .iter()
            .flat_map(|(_, order)| order.iter())
            .filter(|placed| placed.item == Item::Field(Id::ArrangementPlaylist))
            .count();
        check(
            read == playlists,
            format!("{read} of {playlists} playlists read"),
        );
    }
    assert!(
        failures.is_empty(),
        "{} claims failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// What `docs/records-224-233.md` says about notes and playlist items,
/// asked of the model.
#[test]
fn notes_and_items_read_as_the_records_doc_says() {
    let Some(cases) = cases() else {
        eprintln!("FL_RS_CORPUS unset, record claims skipped");
        return;
    };
    let mut failures = Vec::new();
    let (mut notes, mut slides, mut muted_notes, mut off_pitch) = (0, 0, 0, 0);
    let (mut patterns, mut audio, mut automation, mut muted_items) = (0, 0, 0, 0);
    let (mut trimmed_patterns, mut trimmed_clips, mut stretched, mut old_tracks) = (0, 0, 0, 0);
    for Case { name, project, .. } in &cases {
        let mut check = |ok: bool, what: String| {
            if !ok {
                failures.push(format!("{name}: {what}"));
            }
        };
        let version = project.writer_version.as_deref();
        let version = version.and_then(flp::event::version_number).unwrap_or(0);

        for pattern in &project.patterns {
            for (n, note) in pattern.notes.iter().enumerate() {
                let at = format!("pattern {} note {n}", pattern.index);
                notes += 1;
                slides += usize::from(note.slide());
                muted_notes += usize::from(note.muted());
                off_pitch += usize::from(note.fine_pitch != 120);
                check(!note.is_deleted(), format!("{at} is deleted"));
                check(
                    note.tag & !8 == 0x4000,
                    format!("{at}: tag {:#x}", note.tag),
                );
                check(
                    project.channel(note.channel).is_some(),
                    format!("{at}: no channel {}", note.channel),
                );
                check(
                    note.position >= 0 && note.length >= 0,
                    format!("{at}: at {} for {}", note.position, note.length),
                );
                check(
                    (0..=131).contains(&note.key),
                    format!("{at}: key {}", note.key),
                );
                check(
                    note.velocity <= 128 && note.pan <= 128 && note.release <= 128,
                    format!("{at}: velocity, pan or release above 128"),
                );
            }
        }

        for arrangement in &project.arrangements {
            for (n, item) in arrangement.items.iter().enumerate() {
                let at = format!("arrangement {} item {n}", arrangement.index);
                check(item.source_id & 0xffff == 0x5000, format!("{at}: low word"));
                check(
                    item.position >= 0 && item.length > 0,
                    format!("{at}: at {} for {}", item.position, item.length),
                );
                muted_items += usize::from(item.muted());
                // The track is one the arrangement has. Its number counts
                // down from 199 in files from before 12.9.1.
                let track = item.track(version);
                old_tracks += usize::from(version < 12_009_001);
                check(
                    usize::try_from(track)
                        .is_ok_and(|t| (1..=arrangement.tracks.len()).contains(&t)),
                    format!("{at}: track {track} of {}", arrangement.tracks.len()),
                );
                let clip = item.clip();
                stretched += usize::from(clip.stretch != 1.0);
                check(!clip.reversed, format!("{at} is reversed"));
                check(
                    clip.gain.is_finite() && clip.stretch > 0.0 && clip.pan == 0.0,
                    format!("{at}: {clip:?}"),
                );
                match item.source() {
                    None => check(false, format!("{at}: source {:#x}", item.source_id)),
                    // 157 items place a pattern that no 65 selects: FL
                    // writes nothing for an empty pattern.
                    Some(ItemSource::Pattern(_)) => {
                        patterns += 1;
                        match item.trim() {
                            Some(Trim::Ticks { start, end }) => {
                                trimmed_patterns += 1;
                                check(end - start == item.length, format!("{at}: trim"));
                            }
                            Some(Trim::Clip { .. }) => check(false, format!("{at}: trim")),
                            None => {}
                        }
                    }
                    Some(ItemSource::Channel(index)) => {
                        match project.channel(index).and_then(|c| c.kind) {
                            Some(ChannelKind::AudioClip) => audio += 1,
                            Some(ChannelKind::Automation) => automation += 1,
                            kind => check(false, format!("{at}: channel {index} is {kind:?}")),
                        }
                        match item.trim() {
                            Some(Trim::Clip { start, end }) => {
                                trimmed_clips += 1;
                                check(start <= end, format!("{at}: trim {start} to {end}"));
                            }
                            Some(Trim::Ticks { .. }) => check(false, format!("{at}: trim")),
                            None => {}
                        }
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} claims failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("{notes} notes: {slides} slide, {muted_notes} muted, {off_pitch} with a fine pitch");
    eprintln!(
        "playlist items: {patterns} patterns ({trimmed_patterns} trimmed), {audio} audio clips \
         and {automation} automation clips ({trimmed_clips} trimmed), {muted_items} muted, \
         {stretched} stretched, {old_tracks} with a track counted down from 199"
    );
}

/// What `docs/rec-225.md` says about the 225 records, and where the routes
/// of the mixer lead.
#[test]
fn every_initial_value_addresses_an_insert_or_the_main_volume() {
    let Some(cases) = cases() else {
        eprintln!("FL_RS_CORPUS unset, initial values skipped");
        return;
    };
    let (mut records, mut legacy, mut on_104, mut routes, mut unheard) = (0, 0, 0, 0, 0);
    for Case { name, project, .. } in &cases {
        let version = project.writer_version.as_deref();
        let version = version.and_then(flp::event::version_number).unwrap();
        legacy += usize::from(project.legacy_rec_ids());
        let mut main_volumes = 0;
        for record in &project.initial_values {
            records += 1;
            match project.rec_target(record.rec_id) {
                RecTarget::Global { control: 0 } => main_volumes += 1,
                RecTarget::Insert { index, slot, .. } => {
                    assert!(slot.is_none_or(|slot| slot < 10), "{name}: slot {slot:?}");
                    if project.insert_mix(index).insert().is_some() {
                        continue;
                    }
                    // Before 12.9.0 the current track is the file's insert
                    // 104 and its records say 104. The loader's remap only
                    // moves 126, so they stay on an insert the file lacks.
                    assert!(
                        version < 12_009_000 && index == 104,
                        "{name}: a record for insert {index}, which the project lacks"
                    );
                    on_104 += 1;
                }
                other => panic!("{name}: {:#x} reads as {other:?}", record.rec_id),
            }
        }
        assert_eq!(main_volumes, 1, "{name}");
        assert_eq!(project.main_volume_raw(), 12_800, "{name}");

        for position in 0..project.inserts.len() {
            let index = project.insert_index(position);
            let index = index.unwrap_or_else(|| panic!("{name}: no number for insert {position}"));
            let strip = project.insert_mix(index);
            assert!(strip.enabled(), "{name}: insert {index} is off");
            let destinations = strip.destinations();
            routes += destinations.len();
            unheard += usize::from(destinations.is_empty() && index != 0 && index != 501);
            for dest in destinations {
                assert_ne!(dest, index, "{name}: insert {index} sends to itself");
                let there = project.insert_mix(dest).insert().is_some();
                assert!(there, "{name}: insert {index} sends to {dest}");
            }
        }
        assert!(project.insert_mix(0).destinations().is_empty(), "{name}");
        assert!(project.insert_mix(501).destinations().is_empty(), "{name}");
    }
    eprintln!(
        "initial values: {records} records, {legacy} of {} projects in the old encoding, \
         {on_104} records left on insert 104; {routes} routes, {unheard} inserts send nowhere",
        cases.len()
    );
}

/// What `docs/automation-234.md` says about the clips, their points and the
/// links that bind them.
#[test]
fn automation_clips_read_as_the_automation_doc_says() {
    let Some(cases) = cases() else {
        eprintln!("FL_RS_CORPUS unset, automation claims skipped");
        return;
    };
    let (mut clips, mut points, mut modes, mut lfos) = (0, 0, [0usize; 13], Vec::new());
    let (mut bindings, mut unbound, mut ranged) = (0, 0, 0);
    let (mut on_insert, mut on_channel, mut on_tempo) = (0, 0, 0);
    let (mut items, mut trimmed, mut off_length) = (0, 0, 0);
    for Case { name, project, .. } in &cases {
        let bound = project.automation_bindings();
        for channel in &project.channels {
            let Some(data) = &channel.automation else {
                continue;
            };
            let at = format!("{name}: channel {}", channel.index);
            clips += 1;
            assert_eq!(data.version, 3, "{at}");
            assert_eq!(data.header[..4], [1, 0, 0, 0], "{at}");
            // The envelope is switched on. The LFO is off in all but
            // three clips.
            assert_eq!(data.envelope_tail[4..8], [0xff; 4], "{at}");
            if data.lfo_tail[4..8] != [0; 4] {
                lfos.push(at.clone());
            }
            assert!(!data.envelope.is_empty() && !data.lfo.is_empty(), "{at}");
            assert_eq!(data.envelope[0].x, 0.0, "{at}");
            for point in &data.envelope {
                points += 1;
                modes[usize::from(point.mode)] += 1;
                assert!(point.x >= 0.0 && point.x.is_finite(), "{at}: x {}", point.x);
                assert!((0.0..1.001).contains(&point.y), "{at}: y {}", point.y);
                assert!((-1.0..=1.0).contains(&point.tension), "{at}");
                assert_eq!((point.flag, point.unknown_22), (0, [0]), "{at}");
            }
            let (min, max) = channel.automation_range();
            assert!(min >= 0 && max <= 12_800, "{at}: range {min} to {max}");
            let mine = bound.iter().filter(|b| b.channel == channel.index);
            unbound += usize::from(mine.count() == 0);
        }
        for binding in &bound {
            bindings += 1;
            ranged += usize::from(binding.min_raw.is_some());
            match binding.target {
                RecTarget::Insert { index, .. } => {
                    on_insert += 1;
                    let there = project.insert_mix(index).insert().is_some();
                    assert!(there, "{name}: a clip drives insert {index}");
                }
                RecTarget::Channel { index, .. } => {
                    on_channel += 1;
                    assert!(project.channel(index).is_some(), "{name}: channel {index}");
                }
                RecTarget::Global { control: 5 } => on_tempo += 1,
                other => panic!("{name}: a clip drives {other:?}"),
            }
        }

        // An item that shows part of a clip gives the part in beats.
        let ppq = f64::from(project.header.ppq);
        for item in project.arrangements.iter().flat_map(|a| &a.items) {
            let Some(ItemSource::Channel(index)) = item.source() else {
                continue;
            };
            if project.channel(index).and_then(|c| c.kind) != Some(ChannelKind::Automation) {
                continue;
            }
            items += 1;
            if let Some(Trim::Clip { start, end }) = item.trim() {
                trimmed += 1;
                let beats = f64::from(item.length) / ppq;
                off_length += usize::from((f64::from(end - start) - beats).abs() > 0.001 * beats);
            }
        }
    }
    eprintln!(
        "automation: {clips} clips with {points} points, modes {modes:?}; {bindings} bindings \
         ({on_insert} to an insert, {on_channel} to a channel, {on_tempo} to the tempo), \
         {ranged} with a raw range, {unbound} clips unbound; {items} items, {trimmed} trimmed, \
         {off_length} of those not as long as their trim"
    );
    eprintln!("automation: the LFO is on in {lfos:?}");
    assert_eq!((clips, points, lfos.len()), (1105, 9785, 3));
    assert_eq!((bindings, unbound, ranged), (1322, 36, 117));
    assert_eq!((on_insert, on_channel, on_tempo), (820, 495, 7));
    assert_eq!((items, trimmed, off_length), (5462, 3104, 1));
}

#[test]
fn a_new_sampler_channel_moves_no_event_of_any_project() {
    let Some(cases) = cases() else {
        eprintln!("FL_RS_CORPUS unset, new channel test skipped");
        return;
    };
    let kick = r"%FLStudioFactoryData%\Data\Patches\Packs\Drums\Kick.wav";
    // What each object owns, in the order `orders` walks them.
    let owned = |project: &Project| -> Vec<Vec<Item>> {
        let lists = orders(project).into_iter();
        let items = |(_, order): (_, &[Placed])| order.iter().map(|p| p.item.clone()).collect();
        lists.map(items).collect()
    };
    let mut sampled = 0;
    for Case {
        name,
        file,
        project,
        ..
    } in &cases
    {
        let mut changed = project.clone();
        let channels = project.channels.len();
        let index = changed.add_sampler_channel(kick);
        assert_eq!(index, Some(channels as u16), "{name}");
        // A sampler without a sample gets its 196 behind its 20.
        let bare = project.channels.iter().find(|channel| {
            channel.kind == Some(ChannelKind::Sampler) && channel.sample_path.is_none()
        });
        if let Some(bare) = bare {
            changed.set_channel_sample(bare.index, kick);
            sampled += 1;
        }
        let written = model::to_flp(&changed).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(usize::from(written.header.channel_count), channels + 1);

        // The file's events are all there, in their order. The 45 of the
        // new channel are together, and a 196 is the only other new one.
        let mut left = file.events.iter().peekable();
        let mut new = Vec::new();
        for (at, event) in written.events.iter().enumerate() {
            if left.peek() == Some(&event) {
                left.next();
            } else {
                new.push((at, event));
            }
        }
        assert!(left.next().is_none(), "{name}: an event is gone");
        // The sampler is one of the file's channels, so its 196 comes first.
        if bare.is_some() {
            let (at, path) = new.remove(0);
            assert_eq!((path.id, written.events[at - 1].id), (196, 20), "{name}");
        }
        assert_eq!(new.len(), 45, "{name}");
        assert_eq!((new[0].1.id, new[44].1.id), (64, 196), "{name}");
        assert!(
            new.iter().zip(new[0].0..).all(|((at, _), n)| *at == n),
            "{name}: the channel's events are apart"
        );

        // Read back, every event has the owner it had, and the new channel
        // has its own.
        let read = model::from_flp(&written);
        assert!(owned(&read) == owned(&changed), "{name}: an event moved");
        let channel = read.channels.last().unwrap();
        assert_eq!(channel.kind, Some(ChannelKind::Sampler), "{name}");
        assert_eq!(channel.name.as_deref(), Some("Kick"), "{name}");
        assert_eq!(channel.sample_path.as_deref(), Some(kick), "{name}");
        assert_eq!(channel.order.len(), 45, "{name}");
        // The insert is in the event the file's other channels use.
        let wide = project.channels.iter().any(|c| c.target_insert.is_some());
        assert_eq!(channel.target_insert.is_some(), wide, "{name}");
        assert_eq!(channel.insert(), Some(0), "{name}");
    }
    eprintln!(
        "corpus channels: a sampler added to {} projects, a 196 to a sampler of {sampled}",
        cases.len()
    );
}
