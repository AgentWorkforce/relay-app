//! Agent names in the session search: the roster section offered after `@`
//! and the note shown when a query names more than one agent.
use super::{choice, section, section_label, Choice};
use crate::models::{LocalSessionQuery, ONE_AGENT_AT_A_TIME};
use crate::ui::app::App;
use crate::ui::widgets::{self, text};
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;

pub(super) struct AgentFilter {
    section: gtk::Box,
    names: RefCell<Vec<String>>,
    note: gtk::Label,
}

impl AgentFilter {
    pub(super) fn build() -> Self {
        // Roster agents, offered after `@`; the roster arrives with Relay.
        let section = section("Agents");
        section.set_visible(false);
        // Later `@` agents stay as chips but only the first one filters.
        let note = text(ONE_AGENT_AT_A_TIME, &["caption"]);
        note.set_widget_name("session-agent-note");
        note.set_valign(gtk::Align::Center);
        note.set_visible(false);
        Self {
            section,
            names: RefCell::default(),
            note,
        }
    }

    /// The roster column in the popover.
    pub(super) fn section(&self) -> &gtk::Box {
        &self.section
    }

    /// The note beside the chips.
    pub(super) fn note(&self) -> &gtk::Label {
        &self.note
    }

    /// Rebuild the agent choices when the roster changes, and show the note
    /// when the query names more than one agent.
    pub(super) fn update(
        &self,
        app: &App,
        choices: &RefCell<Vec<Choice>>,
        query: &LocalSessionQuery,
    ) {
        let roster = app.with_state(|state| {
            state
                .roster
                .iter()
                .filter(|agent| agent.kind == "agent")
                .map(|agent| agent.name.clone())
                .collect::<Vec<_>>()
        });
        if *self.names.borrow() != roster {
            let mut choices = choices.borrow_mut();
            choices.retain(|choice| choice.field != "agent");
            widgets::clear(&self.section);
            self.section.append(&section_label("Agents"));
            for name in LocalSessionQuery::agent_suggestions("", &roster) {
                choices.push(choice(
                    app,
                    &self.section,
                    "agent",
                    &name,
                    &format!("@{name}"),
                ));
            }
            *self.names.borrow_mut() = roster;
        }
        self.note.set_visible(query.agent_note().is_some());
    }

    /// The roster agents offered for the prefix typed after `@`, as on the Mac.
    pub(super) fn offered(&self, token: &str) -> Vec<String> {
        LocalSessionQuery::agent_suggestions(token, &self.names.borrow())
    }

    /// Show the roster column only while it offers an agent.
    pub(super) fn set_offering(&self, any: bool) {
        self.section.set_visible(any);
    }
}

/// Whether an agent choice is among the offered names.
pub(super) fn offers(offered: &[String], value: &str) -> bool {
    offered.iter().any(|name| name.eq_ignore_ascii_case(value))
}
