//! Your agents on other computers and in Cloud, on the sessions list: the
//! main screen shows every agent you can reach, not only this computer's
//! sessions. The group follows the list's search, and opening one opens
//! Chat with it.

use super::super::app::App;
use super::super::widgets::{self, clear, hbox, text, vbox, LabelStyle};
use crate::models::{
    chat_elsewhere_agents, chat_elsewhere_more, ChatAgent, CHAT_ELSEWHERE_SECTION,
};
use gtk::prelude::*;
use gtk4 as gtk;

/// How many show before anything is searched.
const SHOWN_UNSEARCHED: usize = 6;

/// The group's box, placed above the sessions list once it is shown.
pub fn elsewhere_box() -> gtk::Box {
    let group = vbox(2, &["elsewhere-agents"]);
    widgets::pad(&group, 8, Some(16), Some(4));
    group.set_visible(false);
    group
}

/// Whether `agent` matches the sessions list's search: every plain word
/// matches; qualifiers such as `is:active` are the list's own.
fn matches_search(agent: &ChatAgent, search: &str) -> bool {
    search
        .split_whitespace()
        .filter(|word| !word.contains(':'))
        .all(|word| agent.matches(word))
}

impl App {
    /// What every redraw of the sessions list also draws: the open
    /// conversation's controls, and your agents elsewhere above the list.
    pub(in crate::ui) fn render_session_extras(&self) {
        self.render_transcript_controls();
        self.render_elsewhere_agents();
    }

    /// Draws your agents elsewhere above the sessions list.
    pub(in crate::ui) fn render_elsewhere_agents(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let group = &shell.pages.chat.elsewhere;
        if group.parent().is_none() {
            shell.pages.sessions.table.prepend(group);
        }
        let (roster, search) =
            self.with_state(|state| (state.roster.clone(), state.session_search.clone()));
        let agents: Vec<ChatAgent> = chat_elsewhere_agents(&roster)
            .into_iter()
            .filter(|agent| matches_search(agent, &search))
            .collect();
        clear(group);
        group.set_visible(!agents.is_empty());
        if agents.is_empty() {
            return;
        }
        group.append(&widgets::section_title(CHAT_ELSEWHERE_SECTION));
        // Until a search narrows them, a few keep the sessions in view.
        let searching = search.split_whitespace().any(|word| !word.contains(':'));
        let shown = if searching {
            agents.len()
        } else {
            SHOWN_UNSEARCHED
        };
        for agent in agents.iter().take(shown) {
            group.append(&self.elsewhere_row(agent));
        }
        if agents.len() > shown {
            let more = text(&chat_elsewhere_more(agents.len() - shown), &["caption"]);
            more.set_halign(gtk::Align::Start);
            widgets::pad(&more, 4, Some(10), None);
            group.append(&more);
        }
    }

    fn elsewhere_row(&self, agent: &ChatAgent) -> gtk::Button {
        let row = gtk::Button::new();
        widgets::add_classes(&row, &["sidebar-row", "elsewhere-row"]);
        widgets::action_cursor(&row);
        let line = hbox(10, &[]);
        widgets::pad(&line, 6, Some(4), None);
        line.append(&widgets::agent_badge(&agent.icon_source(), 24));
        let name = text(&agent.name(), &["row-title"]).ellipsized();
        line.append(&name);
        line.append(&text(
            agent.activity.label(),
            &["chat-status", agent.activity.class()],
        ));
        let glance = text(&agent.glance(), &["chat-glance"]).ellipsized();
        glance.set_hexpand(true);
        glance.set_width_chars(1);
        line.append(&glance);
        line.append(&text(&agent.meta(), &["caption"]).ellipsized());
        row.set_child(Some(&line));
        widgets::name(&row, &agent.name());
        let key = agent.key();
        let open = self.on(move |app| app.show_chat_page(Some(&key)));
        row.connect_clicked(move |_| open());
        row
    }
}
