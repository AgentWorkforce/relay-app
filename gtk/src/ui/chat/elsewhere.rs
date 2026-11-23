//! Your agents on other computers and in Cloud, in the sessions list itself:
//! the main screen shows every agent you can reach, with an "Elsewhere"
//! tag where a local session shows what this computer knows of it. They
//! follow the list's search words, lead the list, take no part in
//! selection (those actions act on this computer's sessions), and opening
//! one opens Chat with it.

use super::super::app::App;
use super::super::widgets::{self, hbox, text, vbox, LabelStyle};
use crate::models::{
    chat_elsewhere_agents, chat_elsewhere_tag, merge_local_sessions, relative_ms, ChatActivity,
    ChatAgent, Presence, SessionColumn,
};
use gtk::{glib, prelude::*};
use gtk4 as gtk;

/// Whether `agent` matches the sessions list's search: every plain word
/// matches; qualifiers such as `is:active` are the list's own.
fn matches_search(agent: &ChatAgent, search: &str) -> bool {
    search
        .split_whitespace()
        .filter(|word| !word.contains(':'))
        .all(|word| agent.matches(word))
}

/// Alternates the rows' background down the whole list, agents elsewhere
/// and sessions alike, starting again under each group's header as the
/// sessions list does.
fn restripe(list: &gtk::ListBox) {
    let mut index = 0;
    let mut child = list.first_child();
    while let Some(row) = child {
        if row.has_css_class("session-group-holder") {
            index = 0;
        } else if row.has_css_class("session-row") {
            if index % 2 == 0 {
                row.add_css_class("striped");
            } else {
                row.remove_css_class("striped");
            }
            index += 1;
        }
        child = row.next_sibling();
    }
}

/// What a drawn row shows, to redraw only on a change.
fn drawn(agent: &ChatAgent) -> String {
    format!(
        "{}|{:?}|{}|{}",
        agent.key(),
        agent.activity,
        agent.name(),
        agent.glance()
    )
}

impl App {
    /// What every redraw of the sessions list also draws: the open
    /// conversation's controls, then, once the list is drawn, your agents
    /// elsewhere at its head.
    pub(in crate::ui) fn render_session_extras(&self) {
        self.render_transcript_controls();
        glib::idle_add_local_once(self.on(App::render_elsewhere_rows));
    }

    fn render_elsewhere_rows(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let (list, chat) = (&shell.pages.sessions, &shell.pages.chat);
        let (roster, search, rows) = self.with_state(|state| {
            (
                state.roster.clone(),
                state.session_search.clone(),
                merge_local_sessions(&state.sessions, &state.local_sessions),
            )
        });
        let agents: Vec<ChatAgent> = chat_elsewhere_agents(&roster, &rows)
            .into_iter()
            .filter(|agent| matches_search(agent, &search))
            .collect();
        let signature: Vec<String> = agents.iter().map(drawn).collect();
        let mut rows = chat.elsewhere_rows.borrow_mut();
        // A rebuilt list dropped the rows and their keys with it.
        let present = rows.first().is_some_and(|row| row.parent().is_some());
        if present && *chat.elsewhere_drawn.borrow() == signature {
            return;
        }
        list.drawing.set(true);
        if present {
            for row in rows.iter() {
                list.list.remove(row);
            }
            let mut keys = list.keys.borrow_mut();
            let drop = rows.len().min(keys.len());
            keys.drain(..drop);
        }
        rows.clear();
        for (index, agent) in agents.iter().enumerate() {
            let row = self.elsewhere_row(agent);
            list.list.insert(&row, index as i32);
            rows.push(row);
        }
        list.keys
            .borrow_mut()
            .splice(0..0, std::iter::repeat_n(String::new(), rows.len()));
        restripe(&list.list);
        list.drawing.set(false);
        *chat.elsewhere_drawn.borrow_mut() = signature;
    }

    /// A row laid out as a session's: the same height, selection lane,
    /// avatar and column cells at the columns' widths. Where a session
    /// shows this computer's details, it shows where the agent runs.
    fn elsewhere_row(&self, agent: &ChatAgent) -> gtk::ListBoxRow {
        let content = hbox(16, &[]);
        widgets::pad(&content, 0, Some(12), None);
        super::super::zoom::size(&content, -1, 64);
        // The selection lane stays empty: bulk actions are this computer's.
        let lane = hbox(0, &[]);
        super::super::zoom::size(&lane, 20, -1);
        content.append(&lane);
        let badge = widgets::agent_badge(&agent.icon_source(), 24);
        badge.set_valign(gtk::Align::Center);
        content.append(&badge);
        let layout = self.live_session_columns();
        for column in layout.visible() {
            let cell = self.elsewhere_cell(agent, column);
            cell.set_hexpand(true);
            let frame =
                super::super::sessions_page::session_column_frame(&cell, layout.width(column));
            frame.set_hexpand(layout.expanding_column() == Some(column));
            content.append(&frame);
        }
        let row = gtk::ListBoxRow::new();
        row.add_css_class("session-row");
        row.add_css_class("elsewhere-row");
        row.set_selectable(false);
        row.set_activatable(false);
        widgets::action_cursor(&row);
        row.set_child(Some(&content));
        row.set_tooltip_text(Some(&agent.meta()));
        widgets::name(&row, &format!("Open conversation: {}", agent.name()));
        let key = agent.key();
        let chat = self.on(move |app| app.show_chat_page(Some(&key)));
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| chat());
        row.add_controller(click);
        row
    }

    /// One column's cell for an agent elsewhere; a column only this
    /// computer knows reads "—".
    fn elsewhere_cell(&self, agent: &ChatAgent, column: SessionColumn) -> gtk::Widget {
        let caption = |value: &str| -> gtk::Widget {
            let label = text(if value.is_empty() { "—" } else { value }, &["caption"]).ellipsized();
            label.set_halign(gtk::Align::Start);
            label.set_valign(gtk::Align::Center);
            label.upcast()
        };
        match column {
            SessionColumn::Session => {
                let words = vbox(3, &[]);
                words.set_valign(gtk::Align::Center);
                words.append(&text(&agent.name(), &["row-title"]).ellipsized());
                let line = hbox(8, &[]);
                line.append(&text(&chat_elsewhere_tag(agent), &["elsewhere-tag"]));
                let glance = text(&agent.glance(), &["caption", "secondary"]).ellipsized();
                glance.set_width_chars(1);
                line.append(&glance);
                words.append(&line);
                words.upcast()
            }
            SessionColumn::Status => caption(agent.activity.label()),
            SessionColumn::Project => caption(&agent.project()),
            SessionColumn::Updated => {
                caption(&relative_ms(agent.last_activity_ms, super::super::now_ms()))
            }
            SessionColumn::Harness => {
                let identity = hbox(8, &[]);
                identity.set_valign(gtk::Align::Center);
                identity.append(&widgets::agent_badge(&agent.icon_source(), 20));
                identity.append(&text(&agent.provider(), &["caption"]));
                identity.upcast()
            }
            SessionColumn::Relay => {
                let presence = if agent.activity == ChatActivity::Offline {
                    Presence::Offline
                } else {
                    Presence::Online
                };
                let status = hbox(6, &[]);
                status.set_halign(gtk::Align::Center);
                status.set_valign(gtk::Align::Center);
                status.append(&widgets::glyph(
                    presence.message_symbol(),
                    14,
                    presence.message_class(),
                ));
                status.append(&text(presence.message_status(), &["caption"]));
                status.upcast()
            }
            _ => caption(""),
        }
    }
}
