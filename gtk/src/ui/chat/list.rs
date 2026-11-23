//! The Chat list as drawn: agents with new messages first under their own
//! heading, then every other agent, those on this computer before your
//! agents elsewhere.

use super::super::app::App;
use super::super::widgets::{self, clear, hbox, text, vbox, LabelStyle};
use super::ChatPage;
use crate::models::{
    ChatAgent, ChatPlace, CHAT_ALL_SECTION, CHAT_ELSEWHERE_SECTION, CHAT_NO_AGENTS, CHAT_NO_MATCH,
    CHAT_UNREAD_SECTION,
};
use gtk::prelude::*;
use gtk4 as gtk;

/// A section heading in the list, spaced from what is above it.
fn heading(title: &str, first: bool) -> gtk::Label {
    let heading = widgets::section_title(title);
    if !first {
        heading.set_margin_top(super::super::zoom::px(12));
    }
    heading
}

impl App {
    /// Draws `shown` (unread agents first) into the list; a note when
    /// there is none, saying whether any agent exists at all.
    pub(super) fn draw_chat_list(
        &self,
        page: &ChatPage,
        shown: &[&ChatAgent],
        none_at_all: bool,
        selected: Option<&str>,
    ) {
        clear(&page.list);
        if shown.is_empty() {
            let note = if none_at_all {
                CHAT_NO_AGENTS
            } else {
                CHAT_NO_MATCH
            };
            let empty = text(note, &["caption"]).wrapped();
            widgets::pad(&empty, 8, None, None);
            page.list.append(&empty);
            return;
        }
        let unread = shown
            .iter()
            .take_while(|agent| self.chat_unread(&agent.relay_names()) > 0)
            .count();
        if unread > 0 {
            page.list.append(&heading(CHAT_UNREAD_SECTION, true));
        }
        let mut elsewhere = false;
        for (index, agent) in shown.iter().enumerate() {
            if unread > 0 && index == unread {
                page.list.append(&heading(CHAT_ALL_SECTION, false));
            }
            let remote = matches!(agent.place, ChatPlace::Elsewhere(_));
            if index >= unread && remote && !elsewhere {
                elsewhere = true;
                page.list.append(&heading(CHAT_ELSEWHERE_SECTION, false));
            }
            let chosen = selected == Some(agent.key().as_str());
            page.list.append(&self.chat_row(agent, chosen));
        }
    }

    fn chat_row(&self, agent: &ChatAgent, selected: bool) -> gtk::Button {
        let row = gtk::Button::new();
        widgets::add_classes(
            &row,
            &[
                "sidebar-row",
                "chat-row",
                if selected { "selected" } else { "" },
            ],
        );
        widgets::action_cursor(&row);
        let line = hbox(10, &[]);
        widgets::pad(&line, 8, Some(2), None);
        let badge = super::agent_badge(agent, 34);
        badge.set_valign(gtk::Align::Start);
        line.append(&badge);
        let words = vbox(2, &[]);
        words.set_hexpand(true);
        let top = hbox(6, &[]);
        let name = text(&agent.name(), &["row-title"]).ellipsized();
        name.set_hexpand(true);
        name.set_width_chars(1);
        top.append(&name);
        let status = text(
            agent.activity.label(),
            &["chat-status", agent.activity.class()],
        );
        top.append(&status);
        let unread = self.chat_unread(&agent.relay_names());
        if unread > 0 {
            top.append(&super::inbox::unread_count(unread));
        }
        words.append(&top);
        let glance = agent.glance();
        if !glance.is_empty() {
            let summary = text(&glance, &["chat-glance"]).wrapped();
            summary.set_lines(2);
            summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
            summary.set_width_chars(1);
            words.append(&summary);
        }
        let meta = text(&agent.meta(), &["caption"]).ellipsized();
        meta.set_width_chars(1);
        words.append(&meta);
        line.append(&words);
        row.set_child(Some(&line));
        row.set_tooltip_text(Some(&agent.title()));
        widgets::name(&row, &agent.name());
        let key = agent.key();
        let open = self.on(move |app| {
            let row = app.chat_agent(&key).and_then(|agent| agent.row().cloned());
            if !row.is_some_and(|row| app.open_in_its_app(&row)) {
                app.select_chat_agent(&key);
            }
        });
        row.connect_clicked(move |_| open());
        row
    }
}
