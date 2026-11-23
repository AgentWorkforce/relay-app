//! Opening the agent shown in its own app (Terminal, Codex, Claude…), as
//! the sessions reader's open button does: the core plans where the session
//! opens, and the same plan, chooser and handover guards run. Relay shows
//! every agent; this takes the person to an agent's native app where it
//! has one.

use super::super::app::App;
use super::super::widgets;
use crate::models::{ChatAgent, SessionOpening, SpawnHandover, CHAT_OPEN_FALLBACK};
use gtk::prelude::*;
use gtk4 as gtk;

/// The header's open button, hidden until an agent that can open is shown.
pub fn open_button() -> gtk::Button {
    let open = gtk::Button::with_label(CHAT_OPEN_FALLBACK);
    open.add_css_class("session-action");
    open.set_valign(gtk::Align::Center);
    widgets::action_cursor(&open);
    open.set_visible(false);
    open
}

impl App {
    /// Shows the open button for `agent` when its session can open in an
    /// app of its own, titled for the app the core last planned.
    pub(super) fn render_chat_open(&self, open: &gtk::Button, agent: &ChatAgent) {
        let Some(row) = agent.row() else {
            open.set_visible(false);
            return;
        };
        let session = agent.local().map(|(_, session)| session.clone());
        let spawned = SessionOpening::spawned_live_plan(row, session.as_ref());
        let can_open = SessionOpening::may_request_plan(row, session.as_ref()) || spawned.is_some();
        open.set_visible(can_open);
        if !can_open {
            return;
        }
        let last = self
            .shell()
            .map(|shell| {
                shell
                    .pages
                    .sessions
                    .transcript
                    .join
                    .open_plan
                    .borrow()
                    .clone()
            })
            .unwrap_or_default();
        let title = super::super::session_open::title_for(&row.key(), spawned.as_ref(), &last);
        open.set_child(Some(&super::super::session_open::open_session_label(
            &row.icon_source(),
            &title,
        )));
        open.set_tooltip_text(Some(&title));
        widgets::name(open, &title);
        // A spawned agent Agent Relay runs stays here until taken over.
        open.set_sensitive(SpawnHandover::of(session.as_ref()) != Some(SpawnHandover::TakeOver));
    }

    /// Opens the agent shown in its own app through the reader's planned
    /// open, which works from the reader's selected session.
    pub(super) fn open_chat_agent_natively(&self) {
        let Some(row) = self
            .with_chat(|page| page.selected.borrow().clone())
            .flatten()
            .and_then(|key| self.chat_agent(&key))
            .and_then(|agent| agent.row().cloned())
        else {
            return;
        };
        let Some(shell) = self.shell() else {
            return;
        };
        *shell.pages.sessions.transcript.selected.borrow_mut() = Some(row);
        self.open_transcript_session();
    }
}
