//! The signed-in pages beside Settings: Sessions and Messages. Each is built
//! once into the page stack and redrawn from the window's state
//! ([`super::state::State`]) when what it lists changes.

use super::messages::MessagesPage;
use super::sessions_page::SessionsPage;
use super::state::Page;
use gtk::prelude::*;
use gtk4 as gtk;

/// The pages' roots.
pub struct Pages {
    pub sessions: SessionsPage,
    pub messages: MessagesPage,
}

impl Pages {
    pub fn build() -> Pages {
        Pages {
            sessions: SessionsPage::build(),
            messages: MessagesPage::build(),
        }
    }

    pub fn named(&self) -> [(&'static str, gtk::Widget); 2] {
        [
            (
                Page::Sessions.stack_name(),
                self.sessions.root.clone().upcast(),
            ),
            (
                Page::Messages.stack_name(),
                self.messages.root.clone().upcast(),
            ),
        ]
    }
}
