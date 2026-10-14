//! Round-trip every project and preset in the FL Studio corpus byte for byte,
//! then hold `docs/event-dictionary.md` against the same files through the
//! typed event layer.
//!
//! The corpus is FL Studio's own `Data/` directory, which stays out of this
//! repo. Point `FL_RS_CORPUS` at it; `corpus/manifest.sha256` pins the exact
//! files. Without the variable the tests pass and say they were skipped.

mod common;

use flp::event::layout::{
    ArrangementViewState, AutomationData, ChannelSubState, ControllerEvent, ControllerLink, Delay,
    EnvelopeLfo, Filter, InitialValue, InsertFlags, LevelAdjusts, Levels, Note, Parameters,
    PlSelection, PlaylistItem, PluginWrapper, Polyphony, ProjectPairU32, Timestamp, Track,
    Tracking,
};
use flp::event::{self, Id, Value};
use flp::{Project, RawEvent};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn corpus() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("FL_RS_CORPUS")?);
    assert!(
        p.is_dir(),
        "FL_RS_CORPUS={} is not a directory",
        p.display()
    );
    Some(p)
}

fn flps(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            flps(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("flp") || e.eq_ignore_ascii_case("fst"))
        {
            out.push(path);
        }
    }
}

#[test]
fn every_corpus_file_round_trips() {
    let Some(root) = corpus() else {
        eprintln!("FL_RS_CORPUS unset, corpus round trip skipped");
        return;
    };
    let mut files = Vec::new();
    flps(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no .flp or .fst files under {}",
        root.display()
    );
    let mut failures = Vec::new();
    for path in &files {
        let bytes = std::fs::read(path).unwrap();
        match flp::Project::parse(&bytes) {
            Ok(p) if p.write() == bytes => {}
            Ok(_) => failures.push(format!("{}: write differs", path.display())),
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} files failed:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
    eprintln!("{} corpus files round-tripped", files.len());
}

/// Where one dictionary claim failed, and how often.
struct Tally {
    events: usize,
    files: usize,
    last_file: usize,
    first: String,
    oldest: u32,
    newest: u32,
}

/// Failed claims keyed by their message, so 98 presets that break one row
/// the same way print one entry.
#[derive(Default)]
struct Failures(BTreeMap<String, Tally>);

impl Failures {
    fn add(&mut self, what: String, file_index: usize, file: &str, writer: u32) {
        let t = self.0.entry(what).or_insert(Tally {
            events: 0,
            files: 0,
            last_file: usize::MAX,
            first: file.to_string(),
            oldest: writer,
            newest: writer,
        });
        t.events += 1;
        if t.last_file != file_index {
            t.last_file = file_index;
            t.files += 1;
        }
        t.oldest = t.oldest.min(writer);
        t.newest = t.newest.max(writer);
    }
}

fn dotted(v: u32) -> String {
    format!("{}.{}.{}", v / 1_000_000, v / 1_000 % 1_000, v % 1_000)
}

fn round_trip<T>(
    data: &[u8],
    from: impl Fn(&[u8]) -> Option<T>,
    to: impl Fn(&T) -> Vec<u8>,
) -> Result<(), String> {
    match from(data) {
        None => Err(format!("from_bytes rejects {} bytes", data.len())),
        Some(v) if to(&v) != data => Err(format!("to_bytes differs for {} bytes", data.len())),
        Some(_) => Ok(()),
    }
}

/// Round-trip an event through its struct. `None` for an ID without one.
fn check_layout(e: &RawEvent, writer: u32, flags169: u32) -> Option<Result<(), String>> {
    let d = e.data.as_slice();
    Some(match Id::from(e.id) {
        Id::Delay => round_trip(d, Delay::from_bytes, Delay::to_bytes),
        Id::PluginWrapper => round_trip(d, PluginWrapper::from_bytes, PluginWrapper::to_bytes),
        Id::Parameters => round_trip(d, Parameters::from_bytes, Parameters::to_bytes),
        Id::GlobalControllerEvents | Id::PatternControllers => {
            round_trip(d, ControllerEvent::list_from_bytes, |v| {
                ControllerEvent::list_to_bytes(v)
            })
        }
        Id::ArrangementsPLSelection => {
            round_trip(d, PlSelection::from_bytes, PlSelection::to_bytes)
        }
        Id::EnvelopeLFO => round_trip(d, EnvelopeLfo::from_bytes, EnvelopeLfo::to_bytes),
        Id::Levels => round_trip(d, Levels::from_bytes, Levels::to_bytes),
        Id::Filter => round_trip(d, Filter::from_bytes, Filter::to_bytes),
        Id::Polyphony => round_trip(d, Polyphony::from_bytes, Polyphony::to_bytes),
        Id::PatternNotes => round_trip(d, Note::list_from_bytes, |v| Note::list_to_bytes(v)),
        Id::MixerParams => round_trip(d, InitialValue::list_from_bytes, |v| {
            InitialValue::list_to_bytes(v)
        }),
        Id::ControllerMIDI | Id::ControllerRemote => {
            round_trip(d, ControllerLink::from_bytes, ControllerLink::to_bytes)
        }
        Id::Tracking => round_trip(d, Tracking::from_bytes, Tracking::to_bytes),
        Id::LevelAdjusts => round_trip(d, LevelAdjusts::from_bytes, LevelAdjusts::to_bytes),
        Id::ArrangementPlaylist => round_trip(
            d,
            |b| PlaylistItem::list_from_bytes(b, writer, flags169),
            |v| PlaylistItem::list_to_bytes(v),
        )
        .map_err(|err| {
            let size = PlaylistItem::size_for(writer, flags169);
            format!("{err} as {size}-byte items (169 is {flags169:#x})")
        }),
        Id::Automation => round_trip(d, AutomationData::from_bytes, AutomationData::to_bytes),
        Id::InsertFlags => round_trip(d, InsertFlags::from_bytes, InsertFlags::to_bytes),
        Id::Timestamp => round_trip(d, Timestamp::from_bytes, Timestamp::to_bytes),
        Id::TrackData => round_trip(d, Track::from_bytes, Track::to_bytes),
        Id::ArrangementViewState => round_trip(
            d,
            ArrangementViewState::from_bytes,
            ArrangementViewState::to_bytes,
        ),
        Id::ProjectPairU32 => round_trip(d, ProjectPairU32::from_bytes, ProjectPairU32::to_bytes),
        Id::ChannelSubState => {
            round_trip(d, ChannelSubState::from_bytes, ChannelSubState::to_bytes)
        }
        _ => return None,
    })
}

/// Decode every event of every corpus file through `event::Id`,
/// `event::Value`, `event::text` and the `event::layout` structs, and collect
/// each place where a file disagrees with its dictionary row.
///
/// A failure here is a wrong row or a wrong decoder. Fix that one. Do not
/// loosen the check.
#[test]
fn every_corpus_event_matches_its_dictionary_row() {
    let Some(root) = corpus() else {
        eprintln!("FL_RS_CORPUS unset, dictionary check skipped");
        return;
    };
    // A census artefact is no real event, so an occurrence counts as unknown.
    let rows: BTreeMap<u32, common::Row> = common::rows()
        .into_iter()
        .filter(|r| !r.is_artefact)
        .map(|r| (r.id, r))
        .collect();
    let mut files = Vec::new();
    flps(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no .flp or .fst files under {}",
        root.display()
    );

    let mut failures = Failures::default();
    let (mut events, mut structs, mut strings) = (0usize, 0usize, 0usize);
    let mut lone_surrogate_files = Vec::new();
    for (index, path) in files.iter().enumerate() {
        let file = path.strip_prefix(&root).unwrap().display().to_string();
        let project = Project::parse(&std::fs::read(path).unwrap()).unwrap();

        // 199 gives the version. Two-part strings count the patch as 0.
        let Some(writer) = project.writer_version() else {
            let version = event::version(&project.events);
            let what = format!("199 FLVersion: {version:?} is not a version");
            failures.add(what, index, &file, 0);
            continue;
        };
        let utf16 = event::strings_are_utf16(writer);

        // 169 describes the data events that follow it.
        let mut flags169 = 0;
        for e in &project.events {
            events += 1;
            let id = Id::from(e.id);
            let Some(row) = rows.get(&e.id) else {
                failures.add(
                    format!("{:#x}: no dictionary row", e.id),
                    index,
                    &file,
                    writer,
                );
                continue;
            };
            let label = format!("{} {}", e.id, id.name().unwrap_or("(no Id constant)"));

            // (a) the payload decodes as the row's kind
            let value = Value::decode(e);
            if id.name().is_none() || value.map(Value::kind) != Some(row.kind) {
                let what = format!("{label}: decodes as {value:?}, the row says {:?}", row.kind);
                failures.add(what, index, &file, writer);
            }
            if let (Id::DataFormatFlags, Some(Value::U32(flags))) = (id, value) {
                flags169 = flags;
            }

            // (b) the struct reads the payload and writes it back
            if let Some(result) = check_layout(e, writer, flags169) {
                structs += 1;
                if let Err(err) = result {
                    let what = format!("{label} ({}): {err}", row.payload);
                    failures.add(what, index, &file, writer);
                }
            }

            // (c) a string decodes, and its units give the payload back
            if row.is_string {
                strings += 1;
                let units = event::text_units(e, utf16);
                let exact = units
                    .as_deref()
                    .and_then(|u| event::text_bytes(id, u, utf16));
                if event::text(e, utf16).is_none() || exact.as_ref() != Some(&e.data) {
                    let encoding = if utf16 { "UTF-16" } else { "8-bit" };
                    let fault = match units {
                        None => "text_units() is None",
                        Some(_) => "text_bytes() differs",
                    };
                    let what = format!(
                        "{label} ({}): {fault} for {} bytes of {encoding}",
                        row.payload,
                        e.data.len()
                    );
                    failures.add(what, index, &file, writer);
                }
                // Cut-off emoji are FL's doing and survive in the units, so
                // they are counted, not failed.
                let lone = units.is_some_and(|u| char::decode_utf16(u).any(|c| c.is_err()));
                if lone && lone_surrogate_files.last() != Some(&index) {
                    lone_surrogate_files.push(index);
                }
            }
        }
    }

    let report: Vec<String> = failures
        .0
        .iter()
        .map(|(what, t)| {
            format!(
                "{what}\n    {} events in {} files, writers {} to {}, first in {}",
                t.events,
                t.files,
                dotted(t.oldest),
                dotted(t.newest),
                t.first
            )
        })
        .collect();
    let checked = format!(
        "{} files, {events} events, {structs} struct and record payloads, {strings} strings, \
         {} files with unpaired surrogates",
        files.len(),
        lone_surrogate_files.len()
    );
    assert!(
        report.is_empty(),
        "{} dictionary claims fail against the corpus ({checked}):\n{}",
        report.len(),
        report.join("\n")
    );
    eprintln!("every dictionary claim holds against the corpus ({checked})");
}
