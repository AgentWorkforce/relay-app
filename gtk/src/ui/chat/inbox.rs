//! New messages from agents, as in Pear: a system notification for each,
//! which opens its conversation here; an unread count on the agent's row
//! until you open it; and, when the core offers it, the conversation marked
//! read on Relaycast as you open it, for the iOS app and the dashboard.
//! What counts as new is in `models/chat/inbox.rs`.

use super::super::app::App;
use crate::models::{ChatAgent, CHAT_LINK};
use agent_relay_node::relay::contract::connection::CoreNotification;
use agent_relay_node::relay::contract::directory::MarkReadParams;
use agent_relay_node::relay::contract::method;
use agent_relay_node::relay::contract::NoParams;
use gtk::{glib, prelude::*};
use gtk4 as gtk;
use std::time::Duration;

/// How often the inbox is read when no change event says to.
const READ_EVERY: Duration = Duration::from_secs(30);

impl App {
    /// Reads the inbox now and every [`READ_EVERY`], while the core offers it.
    pub(super) fn follow_chat_inbox(&self) {
        self.refresh_chat_inbox();
        let weak = self.downgrade();
        glib::timeout_add_local(READ_EVERY, move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            app.refresh_chat_inbox();
            glib::ControlFlow::Continue
        });
    }

    /// Reads what is unread, announces what arrived since the last reading
    /// and redraws the counts.
    pub fn refresh_chat_inbox(&self) {
        if !self.bridge().offers(method::MessagesConversations::NAME)
            || !self.with_state(|state| state.connected())
        {
            return;
        }
        self.call::<method::MessagesConversations>(NoParams {}, |app, result| {
            let Ok(listing) = result else {
                return;
            };
            let notices = app
                .with_chat(|page| page.inbox.borrow_mut().update(&listing))
                .unwrap_or_default();
            let reading = app.shown_peer_unread();
            for notice in notices {
                // The conversation you are reading needs no notification.
                if reading
                    .as_ref()
                    .is_some_and(|(names, _)| names.contains(&notice.peer))
                {
                    continue;
                }
                app.notify(&CoreNotification {
                    title: format!("@{}", notice.peer),
                    body: notice.text,
                    link: Some(format!("{CHAT_LINK}{}", notice.peer)),
                });
            }
            if let Some((names, _)) = reading {
                app.mark_chat_read(&names);
            }
            app.render_chat();
        });
    }

    /// The names of the agent shown and its unread messages, when the
    /// window is in front.
    fn shown_peer_unread(&self) -> Option<(Vec<String>, u64)> {
        if !self.window().is_active() || self.page() != super::super::state::Page::Chat {
            return None;
        }
        let key = self
            .with_chat(|page| page.selected.borrow().clone())
            .flatten()?;
        let names = self.chat_agent(&key)?.relay_names();
        let count = self.chat_unread(&names);
        Some((names, count))
    }

    /// How many messages from the agent going by `names` are unread.
    pub(super) fn chat_unread(&self, names: &[String]) -> u64 {
        self.with_chat(|page| page.inbox.borrow().unread_for(names).map(|u| u.count))
            .flatten()
            .unwrap_or(0)
    }

    /// Marks read everything from the agent going by `names`, here and on
    /// every other device.
    pub(super) fn mark_chat_read(&self, names: &[String]) {
        let Some((peer, newest)) = self
            .with_chat(|page| {
                let inbox = page.inbox.borrow();
                let unread = inbox.unread_for(names)?;
                let peer = names
                    .iter()
                    .find(|name| inbox.unread_for(std::slice::from_ref(name)).is_some())?
                    .clone();
                Some((peer, unread.newest_id.clone()?))
            })
            .flatten()
        else {
            return;
        };
        if !self.bridge().offers(method::MessagesMarkRead::NAME) {
            return;
        }
        self.with_chat(|page| page.inbox.borrow_mut().read(&peer));
        self.call::<method::MessagesMarkRead>(
            MarkReadParams { message_id: newest },
            |app, result| {
                // A failed mark leaves it unread; the next reading shows so.
                if result.is_err() {
                    app.refresh_chat_inbox();
                }
            },
        );
    }

    /// Opens the conversation a notification's link names.
    pub fn open_chat_link(&self, link: &str) -> bool {
        let Some(peer) = link.strip_prefix(CHAT_LINK) else {
            return false;
        };
        let key = self
            .chat_agents()
            .into_iter()
            .find(|agent: &ChatAgent| agent.relay_names().iter().any(|name| name == peer))
            .map(|agent| agent.key());
        self.window().present();
        self.show_chat_page(key.as_deref());
        true
    }
}
