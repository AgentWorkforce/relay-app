//! A task the cloud Skip lead described, as a card under its reply: what
//! the task is, its next step, blocker and check, then the agents on it.
//! Worded as Skip.app's task breakdown.

use super::super::widgets::{hbox, text, vbox, LabelStyle};
use crate::models::{SkipTask, SkipTaskAgent};
use gtk::prelude::*;
use gtk4 as gtk;

pub fn task_card(task: &SkipTask) -> gtk::Box {
    let card = vbox(8, &["skip-task"]);
    card.append(&text(&task.title, &["skip-task-title"]).wrapped());
    card.append(
        &text(
            &format!(
                "{} · {} · {} · Updated {}",
                task.task_id,
                task.owner,
                task.state,
                date_label(&task.observed_at)
            ),
            &["caption", "secondary"],
        )
        .wrapped(),
    );
    for (label, value) in [
        ("NEXT", task.next_action.clone().unwrap_or("Unknown".into())),
        (
            "BLOCKER",
            task.blocker.clone().unwrap_or("None reported".into()),
        ),
        (
            "CHECK",
            task.next_check_at
                .as_deref()
                .map_or("Not scheduled".into(), date_label),
        ),
    ] {
        let fact = hbox(8, &[]);
        let name = text(label, &["skip-task-fact"]);
        name.set_width_chars(8);
        fact.append(&name);
        fact.append(&text(&value, &[]).wrapped());
        card.append(&fact);
    }
    let total = task.agents.len() + task.agents_omitted as usize;
    card.append(&text(&format!("AGENTS · {total}"), &["skip-task-fact"]));
    if task.agents.is_empty() && task.agents_omitted == 0 {
        card.append(&text(
            "No agent assignment verified for this task.",
            &["secondary"],
        ));
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    for agent in &task.agents {
        card.append(&agent_row(agent, now_ms));
    }
    if task.agents_omitted > 0 {
        card.append(
            &text(
                &format!(
                    "{} more agents omitted from this reply. Ask Skip for the rest.",
                    task.agents_omitted
                ),
                &["caption", "secondary"],
            )
            .wrapped(),
        );
    }
    card
}

fn agent_row(agent: &SkipTaskAgent, now_ms: i64) -> gtk::Box {
    let row = vbox(2, &["skip-task-agent"]);
    let head = hbox(6, &[]);
    head.append(&text(&agent.name, &["skip-task-agent-name"]).ellipsized());
    let status = agent
        .lifecycle
        .as_deref()
        .map_or("Status unknown".into(), capitalized);
    head.append(&text(&status, &["presence-badge"]));
    row.append(&head);
    let assignment = agent
        .assignment
        .as_deref()
        .or(agent.summary.as_deref())
        .unwrap_or("Assignment unknown");
    row.append(&text(assignment, &[]).wrapped());
    let facts = format!(
        "{} · {} · {} · {}",
        agent.machine.as_deref().unwrap_or("Machine unknown"),
        agent.model.as_deref().unwrap_or("Model unknown"),
        agent.harness.as_deref().unwrap_or("Harness unknown"),
        agent
            .active_for(now_ms)
            .map_or("Age unknown".into(), |age| format!("Active {age}")),
    );
    row.append(&text(&facts, &["caption", "secondary"]).wrapped());
    if let Some(evidence) = &agent.evidence {
        row.append(&text(&format!("Source: {evidence}"), &["caption", "secondary"]).wrapped());
    }
    row
}

/// An ISO time in local time, or as given when it is not one.
fn date_label(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value).map_or_else(
        |_| value.to_string(),
        |at| {
            at.with_timezone(&chrono::Local)
                .format("%b %-d, %H:%M")
                .to_string()
        },
    )
}

fn capitalized(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}
