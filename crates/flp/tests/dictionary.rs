//! Check the `event::Id` table against `docs/event-dictionary.md`, row by
//! row. Needs no corpus.

mod common;

use flp::RawEvent;
use flp::event::{self, Id};

#[test]
fn every_dictionary_row_has_a_matching_id() {
    let mut failures = Vec::new();
    for row in common::rows().iter().filter(|r| !r.name.is_empty()) {
        let id = Id::from(row.id);
        let label = format!("{:#x} {}", row.id, row.name);
        if row.is_artefact {
            // Never a real event, so it gets no constant.
            if let Some(name) = id.name() {
                failures.push(format!("{label}: a census artefact, yet Id::{name} exists"));
            }
            continue;
        }
        if id.name().is_none() {
            failures.push(format!("{label}: no Id constant"));
            continue;
        }
        if id.scope() != row.scope {
            failures.push(format!(
                "{label}: scope is {:?}, the row says {:?}",
                id.scope(),
                row.scope
            ));
        }
        if id.kind() != row.kind {
            failures.push(format!(
                "{label}: kind is {:?}, the row says {:?}",
                id.kind(),
                row.kind
            ));
        }
        let e = RawEvent {
            id: row.id,
            data: b"a\0".to_vec(),
        };
        if event::text(&e, false).is_some() != row.is_string {
            failures.push(format!(
                "{label}: text() and the payload {:?} disagree on whether this is a string",
                row.payload
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
