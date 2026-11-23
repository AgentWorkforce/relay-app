//! Where an agent elsewhere falls among the sessions list's rows under the
//! list's sort, so the list reads as one ordering. A column the agent has
//! no value for sorts it after every row that has one, as the list's own
//! sort does with missing values.

use super::*;
use crate::models::{LocalSessionSort, LocalSessionSortColumn};
use std::cmp::Ordering;

/// A value a column sorts by.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Text(String),
    Time(i64),
}

fn text(value: String) -> Option<Key> {
    Some(value.to_lowercase())
        .filter(|value| !value.is_empty() && value != "—")
        .map(Key::Text)
}

fn agent_key(agent: &ChatAgent, column: LocalSessionSortColumn) -> Option<Key> {
    match column {
        LocalSessionSortColumn::Session => text(agent.name()),
        LocalSessionSortColumn::Project => text(agent.project()),
        LocalSessionSortColumn::Harness => text(agent.icon_source()),
        // Bucketed: a heartbeat must not move the row.
        LocalSessionSortColumn::Updated => chat_order_bucket(agent.last_activity_ms)
            .map(|bucket| Key::Time(bucket * CHAT_ORDER_BUCKET_MS)),
        _ => None,
    }
}

fn row_key(row: &LocalSessionRow, column: LocalSessionSortColumn) -> Option<Key> {
    match column {
        LocalSessionSortColumn::Session => text(row.title()),
        LocalSessionSortColumn::Project => text(row.project()),
        LocalSessionSortColumn::Harness => text(row.source().to_string()),
        LocalSessionSortColumn::Updated => row.updated().map(Key::Time),
        LocalSessionSortColumn::Created => row.created_at().map(Key::Time),
        LocalSessionSortColumn::Directory => text(row.directory()),
        LocalSessionSortColumn::Model => text(row.model_name()),
        _ => None,
    }
}

/// How `left` sorts against `right`; a missing value sorts last.
fn compare(left: Option<Key>, right: Option<Key>, ascending: bool) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, _) => Ordering::Greater,
        (_, None) => Ordering::Less,
        (Some(a), Some(b)) if ascending => a.cmp(&b),
        (Some(a), Some(b)) => b.cmp(&a),
    }
}

/// Whether `agent` comes before the session `row` under `sort`.
pub fn chat_sorts_before(agent: &ChatAgent, row: &LocalSessionRow, sort: LocalSessionSort) -> bool {
    compare(
        agent_key(agent, sort.column),
        row_key(row, sort.column),
        sort.ascending,
    ) == Ordering::Less
}

/// Orders agents elsewhere among themselves under `sort`, keeping the
/// activity order for ties.
pub fn chat_sort_elsewhere(agents: &mut [ChatAgent], sort: LocalSessionSort) {
    agents.sort_by(|a, b| {
        compare(
            agent_key(a, sort.column),
            agent_key(b, sort.column),
            sort.ascending,
        )
        .then_with(|| a.name().to_lowercase().cmp(&b.name().to_lowercase()))
    });
}
