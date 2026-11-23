//! What the window reads again when the core says a listing changed.

use super::*;

impl App {
    pub(super) fn invalidated(&self, what: agent_relay_node::relay::state::Listing) {
        use agent_relay_node::relay::state::Listing;
        if !self.with_state(|state| state.connected()) {
            return;
        }
        match what {
            Listing::Uploader => {
                let sessions = self.page() == Page::Sessions;
                self.refresh_uploader(sessions);
            }
            Listing::Sessions => self.refresh_local(),
            Listing::Agents => {
                self.refresh_agents();
                self.refresh_local();
            }
            Listing::Roster => self.refresh_roster(),
            // The workspace's people are not shown.
            Listing::People => {}
            // An agent changed a setting (`PATCH /settings`).
            Listing::Settings => self.load_settings(),
            Listing::Activity => self.refresh_claude_delivery(),
            Listing::Messages => {
                self.refresh_chats();
                self.refresh_chat_inbox();
            }
            // Skip chat, projects and CLI fleet brokers are not read by this
            // window.
            Listing::Chat | Listing::Projects | Listing::Fleet => {}
        }
        // A session or reply changing can change whether a followed
        // conversation's recipient runs and what became of its message.
        if matches!(
            what,
            Listing::Sessions | Listing::Roster | Listing::Messages
        ) {
            self.follow_recipients();
        }
    }
}
