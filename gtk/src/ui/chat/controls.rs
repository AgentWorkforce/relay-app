//! The details panel beside a conversation. For a session on this
//! computer it holds the session's own controls (`transcript.rs`): its
//! title, opening it in its own app, taking over or ending a spawned agent,
//! receiving messages, its agent name and its upload. For an agent
//! elsewhere it offers its uploaded session on the web.

use super::super::app::App;
use super::super::widgets::{self, vbox};
use crate::models::{chat_web_url, ChatAgent, CHAT_VIEW_WEB, CHAT_VIEW_WEB_HELP};
use gtk::prelude::*;
use gtk4 as gtk;

pub struct Controls {
    /// The panel, shown or hidden by the header's Details button.
    pub root: gtk::Box,
    /// An agent elsewhere's session on the web.
    pub view_web: gtk::Button,
}

impl Controls {
    pub fn build() -> Controls {
        let root = vbox(14, &["chat-details"]);
        // A fixed column: a child that expands (the Open session button)
        // must not widen it.
        super::super::zoom::size(&root, 280, -1);
        root.set_hexpand(false);
        let view_web = gtk::Button::with_label(CHAT_VIEW_WEB);
        view_web.add_css_class("session-action");
        view_web.set_tooltip_text(Some(CHAT_VIEW_WEB_HELP));
        view_web.set_visible(false);
        widgets::action_cursor(&view_web);
        widgets::name(&view_web, CHAT_VIEW_WEB);
        root.append(&view_web);
        Controls { root, view_web }
    }
}

impl App {
    pub(super) fn connect_chat_controls(&self, controls: &Controls) {
        let web = self.on(App::view_chat_agent_on_web);
        controls.view_web.connect_clicked(move |_| web());
    }

    /// Shows `agent`'s controls: its session's own on this computer, else
    /// its uploaded session on the web when it has one.
    pub(super) fn render_chat_controls(&self, controls: &Controls, agent: Option<&ChatAgent>) {
        let row = agent.and_then(ChatAgent::row);
        self.select_session_controls(row);
        let web = agent
            .filter(|_| row.is_none())
            .and_then(|agent| self.chat_web_url(agent));
        controls.view_web.set_visible(web.is_some());
    }

    /// Where the agent shown is on the web: its uploaded session in Cloud.
    pub(super) fn chat_web_url(&self, agent: &ChatAgent) -> Option<String> {
        let (site, workspace) = self.with_state(|state| {
            let workspace = state
                .status
                .as_ref()
                .and_then(|status| status.upload_workspace.clone());
            (state.account().site_url.unwrap_or_default(), workspace)
        });
        chat_web_url(agent, &site, workspace.as_deref())
    }

    fn view_chat_agent_on_web(&self) {
        if let Some(url) = self
            .shown_chat_agent()
            .and_then(|agent| self.chat_web_url(&agent))
        {
            self.inner.desktop.open_uri(&url);
        }
    }

    pub(super) fn shown_chat_agent(&self) -> Option<ChatAgent> {
        let key = self
            .with_chat(|page| page.selected.borrow().clone())
            .flatten()?;
        self.chat_agent(&key)
    }
}
