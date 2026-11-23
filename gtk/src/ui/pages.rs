//! The signed-in pages beside Settings: Sessions, Chat and Messages. Each is built
//! once into the page stack and redrawn from the window's state
//! ([`super::state::State`]) when what it lists changes.

use super::chat::ChatPage;
use super::messages::MessagesPage;
use super::sessions_page::SessionsPage;
use super::state::Page;
use gtk::prelude::*;
use gtk4 as gtk;

/// The pages' roots.
pub struct Pages {
    pub sessions: SessionsPage,
    pub chat: ChatPage,
    pub messages: MessagesPage,
}

impl Pages {
    pub fn build() -> Pages {
        Pages {
            sessions: SessionsPage::build(),
            chat: ChatPage::build(),
            messages: MessagesPage::build(),
        }
    }

    pub fn named(&self) -> [(&'static str, gtk::Widget); 3] {
        [
            (
                Page::Sessions.stack_name(),
                self.sessions.root.clone().upcast(),
            ),
            (Page::Chat.stack_name(), self.chat.root.clone().upcast()),
            (
                Page::Messages.stack_name(),
                self.messages.root.clone().upcast(),
            ),
        ]
    }
}
