//! The rows of `docs/event-dictionary.md`, read at test time. The tests
//! check the code against the document itself, not against a copy of it.

#![allow(dead_code)]

use flp::event::{Kind, Scope};
use std::path::Path;

/// One event ID as the dictionary describes it.
pub struct Row {
    pub id: u32,
    pub kind: Kind,
    /// The name cell, `ChannelID.IsEnabled` or `Unknown1 (coined)`.
    pub name: String,
    pub scope: Scope,
    /// The payload cell, empty for the extended IDs, which have none.
    pub payload: String,
    /// Whether the row gives the payload as a string.
    pub is_string: bool,
    /// Whether the notes open with "Census artefact": the ID was a misread
    /// byte of another event and no file holds it.
    pub is_artefact: bool,
}

/// Every row of the main table and of the extended-ID table.
pub fn rows() -> Vec<Row> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/event-dictionary.md");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut rows = Vec::new();
    for line in text.lines().filter(|l| l.starts_with('|')) {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        let Some(id) = id(cells[0]) else { continue };
        // The main table has nine columns. The extended one has six: no
        // census, payload or notes, and the string claim sits in the action.
        let (kind, name, scope, payload, is_string) = match cells.len() {
            9 => {
                let p = cells[6];
                let s = p.starts_with("string") || p.starts_with("8-bit") || p.starts_with("text");
                (cells[1], cells[3], cells[4], p, s)
            }
            6 => (
                cells[1],
                cells[2],
                cells[3],
                "",
                cells[4].contains("string"),
            ),
            n => panic!("dictionary row for {id} has {n} cells"),
        };
        rows.push(Row {
            id,
            kind: match kind {
                "u8" | "i8" => Kind::U8,
                "u16" | "i16" => Kind::U16,
                "u32" | "i32" | "f32" => Kind::U32,
                "var" => Kind::Var,
                other => panic!("dictionary row for {id} has kind {other:?}"),
            },
            name: name.to_string(),
            scope: scope_of(id, scope),
            payload: payload.to_string(),
            is_string,
            is_artefact: cells.len() == 9 && cells[8].starts_with("Census artefact"),
        });
    }
    assert!(
        rows.len() > 150,
        "only {} dictionary rows parsed",
        rows.len()
    );
    rows
}

fn id(cell: &str) -> Option<u32> {
    match cell.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => cell.parse().ok(),
    }
}

/// The scope cell without its parenthesised qualifier.
fn scope_of(id: u32, cell: &str) -> Scope {
    match cell.split(" (").next().unwrap() {
        "channel" => Scope::Channel,
        "channel x pattern" | "pattern x channel" => Scope::ChannelPattern,
        "channel x pattern step" => Scope::ChannelPatternStep,
        "project" => Scope::Project,
        "pattern" => Scope::Pattern,
        "insert" => Scope::Insert,
        "insert slot" => Scope::InsertSlot,
        "plugin" => Scope::Plugin,
        "arrangement" => Scope::Arrangement,
        "track" => Scope::Track,
        "time marker" => Scope::TimeMarker,
        "controller link" => Scope::ControllerLink,
        "display group" => Scope::DisplayGroup,
        "stream" => Scope::Stream,
        "?" | "" => Scope::Unknown,
        other => panic!("dictionary row for {id} has scope {other:?}"),
    }
}
