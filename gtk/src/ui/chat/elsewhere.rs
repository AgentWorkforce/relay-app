//! Your agents on other computers and in Cloud, in the sessions list itself:
//! the main screen shows every agent you can reach, with an "Elsewhere"
//! tag where a local session shows what this computer knows of it. They
//! follow the list's search words, lead the list, take no part in
//! selection (those actions act on this computer's sessions), and opening
//! one opens Chat with it.

use super::super::app::App;
use super::super::widgets::{self, hbox, text, LabelStyle};
use crate::models::{chat_elsewhere_agents, chat_elsewhere_tag, ChatAgent};
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
        let (roster, search) =
            self.with_state(|state| (state.roster.clone(), state.session_search.clone()));
        let agents: Vec<ChatAgent> = chat_elsewhere_agents(&roster)
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
        list.drawing.set(false);
        *chat.elsewhere_drawn.borrow_mut() = signature;
    }

    fn elsewhere_row(&self, agent: &ChatAgent) -> gtk::ListBoxRow {
        let holder = gtk::ListBoxRow::new();
        holder.add_css_class("elsewhere-row");
        holder.set_selectable(false);
        holder.set_activatable(false);
        let open = gtk::Button::new();
        open.add_css_class("flat");
        open.add_css_class("session-open");
        widgets::action_cursor(&open);
        let line = hbox(10, &[]);
        widgets::pad(&line, 6, Some(12), None);
        line.append(&widgets::agent_badge(&agent.icon_source(), 24));
        line.append(&text(&agent.name(), &["row-title"]).ellipsized());
        let tag = text(&chat_elsewhere_tag(agent), &["elsewhere-tag"]).ellipsized();
        tag.set_tooltip_text(Some(&agent.meta()));
        line.append(&tag);
        line.append(&text(
            agent.activity.label(),
            &["chat-status", agent.activity.class()],
        ));
        let glance = text(&agent.glance(), &["chat-glance"]).ellipsized();
        glance.set_hexpand(true);
        glance.set_width_chars(1);
        line.append(&glance);
        line.append(&text(&agent.provider(), &["caption"]));
        open.set_child(Some(&line));
        widgets::name(&open, &agent.name());
        let key = agent.key();
        let chat = self.on(move |app| app.show_chat_page(Some(&key)));
        open.connect_clicked(move |_| chat());
        holder.set_child(Some(&open));
        holder
    }
}
