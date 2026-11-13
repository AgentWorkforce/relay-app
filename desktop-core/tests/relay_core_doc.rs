//! docs/relay-core.md documents exactly the contract this app's core
//! answers: the pinned relay core's methods, errors and topics.

use agent_relay_node::relay::contract::{topics, METHODS};
use agent_relay_node::relay::protocol::ErrorKind;
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn document() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/relay-core.md"),
    )
    .unwrap()
}

/// The lines of the section that starts with `heading`, up to the next
/// heading of the same or a higher level.
fn section<'a>(document: &'a str, heading: &str) -> Vec<&'a str> {
    let level = heading.chars().take_while(|c| *c == '#').count();
    document
        .lines()
        .skip_while(|line| *line != heading)
        .skip(1)
        .take_while(|line| {
            let hashes = line.chars().take_while(|c| *c == '#').count();
            hashes == 0 || hashes > level || !line[hashes..].starts_with(' ')
        })
        .collect()
}

/// Each table row's cells, without the header and the rule.
fn rows<'a>(lines: &[&'a str]) -> Vec<Vec<&'a str>> {
    lines
        .iter()
        .filter(|line| line.starts_with("| `"))
        .map(|line| line.trim_matches('|').split(" | ").map(str::trim).collect())
        .collect()
}

/// The names a table cell lists: "`a.b` / `.c` (Linux)" is `a.b`, `a.c`.
fn names(cell: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for part in cell.split(" / ") {
        let name = part.split('`').nth(1).unwrap_or_default();
        match (name.strip_prefix('.'), names.last()) {
            (Some(suffix), Some(previous)) => {
                let parent = previous.rsplit_once('.').map_or("", |(parent, _)| parent);
                names.push(format!("{parent}.{suffix}"));
            }
            _ => names.push(name.to_string()),
        }
    }
    names
}

#[test]
fn the_method_tables_list_exactly_the_registry() {
    let document = document();
    let documented: BTreeSet<String> = rows(&section(&document, "### Methods"))
        .iter()
        .flat_map(|row| names(row[0]))
        .collect();
    let added = agent_relay_desktop_core::contract::all_specs();
    let registered: BTreeSet<String> = METHODS
        .iter()
        .chain(&added)
        .map(|m| m.name.to_string())
        .collect();
    assert_eq!(documented, registered);
    assert_eq!(
        registered.len(),
        METHODS.len() + added.len(),
        "a method is registered twice"
    );
}

#[test]
fn the_error_table_matches_every_kind() {
    let document = document();
    let documented: Vec<(String, i64, bool)> = rows(&section(&document, "### Errors"))
        .iter()
        .map(|row| {
            (
                names(row[0]).remove(0),
                row[1].parse().unwrap(),
                row[2] == "yes",
            )
        })
        .collect();
    let schema = serde_json::to_value(schemars::schema_for!(ErrorKind)).unwrap();
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    for variant in schema["oneOf"].as_array().into_iter().flatten() {
        kinds.extend(variant["const"].as_str().map(str::to_string));
        kinds.extend(
            variant["enum"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }
    assert_eq!(
        documented
            .iter()
            .map(|d| d.0.clone())
            .collect::<BTreeSet<_>>(),
        kinds
    );
    for (name, code, retryable) in documented {
        let kind: ErrorKind = serde_json::from_value(json!(name)).unwrap();
        assert_eq!((kind.code(), kind.retryable()), (code, retryable), "{name}");
    }
}

#[test]
fn the_topic_table_lists_every_topic() {
    let document = document();
    let documented: BTreeSet<String> = rows(&section(&document, "### State and events"))
        .iter()
        .flat_map(|row| names(row[0]))
        .collect();
    let registered: BTreeSet<String> = topics()
        .into_iter()
        .map(|topic| {
            serde_json::to_value(topic)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(documented, registered);
}
