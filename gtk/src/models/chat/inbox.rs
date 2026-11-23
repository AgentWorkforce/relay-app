//! New messages from agents, for notifications and unread counts. Relaycast
//! counts a delivery acknowledgement as a read, and an always-on consumer of
//! your identity (Cloud's events pump, another desktop) acknowledges each
//! direct message within a second, so its unread state never shows one.
//! Instead each reading of your conversations (`messages.conversations`) is
//! compared with the last: a conversation whose newest message is new and
//! from the other side has a new message, announced once and counted
//! unread here until you open the conversation. What was there when the app
//! started is neither announced nor counted.

use agent_relay_node::relay::contract::directory::Conversations;
use std::collections::HashMap;

/// The peer name of a conversation whose other side left.
const GONE: &str = "unknown";

/// The longest a notification's text runs before it is cut.
const NOTICE_CHARACTERS: usize = 180;

/// An unread conversation: how many unread messages and the newest one,
/// which marking read starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unread {
    pub count: u64,
    pub newest_id: Option<String>,
}

/// A message to announce: who it is from and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatNotice {
    pub peer: String,
    pub text: String,
}

/// What is unread, by peer name, and what was announced.
#[derive(Clone, Debug, Default)]
pub struct ChatInbox {
    unread: HashMap<String, Unread>,
    /// The newest message of each conversation at the last reading.
    newest: HashMap<String, String>,
    /// Whether a reading has been taken: the first only records.
    primed: bool,
}

impl ChatInbox {
    /// Takes in a new reading; the messages that arrived since the last.
    pub fn update(&mut self, listing: &Conversations) -> Vec<ChatNotice> {
        let mut notices = Vec::new();
        for conversation in &listing.conversations {
            let Some(peer) = conversation.peer.as_ref().filter(|p| p.name != GONE) else {
                continue;
            };
            let last = &conversation.last_message;
            let Some(id) = last["id"].as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            let before = self.newest.insert(conversation.id.clone(), id.to_string());
            if !self.primed || before.as_deref() == Some(id) {
                continue;
            }
            // Your own message, sent from here or another device, is not news.
            let from_peer =
                peer.agent_id.is_some() && last["agent_id"].as_str() == peer.agent_id.as_deref();
            if !from_peer {
                continue;
            }
            let unread = self.unread.entry(peer.name.clone()).or_insert(Unread {
                count: 0,
                newest_id: None,
            });
            unread.count += 1;
            unread.newest_id = Some(id.to_string());
            notices.push(ChatNotice {
                peer: peer.name.clone(),
                text: notice_text(last["text"].as_str().unwrap_or_default()),
            });
        }
        self.primed = true;
        notices
    }

    /// The unread conversation with whichever of `names` is unread.
    pub fn unread_for(&self, names: &[String]) -> Option<&Unread> {
        names.iter().find_map(|name| self.unread.get(name))
    }

    /// Forgets `peer`'s unread messages once they are marked read.
    pub fn read(&mut self, peer: &str) {
        self.unread.remove(peer);
    }

    /// Every unread peer, for a count on the page.
    pub fn total(&self) -> u64 {
        self.unread.values().map(|unread| unread.count).sum()
    }
}

/// `text` on one line, cut to [`NOTICE_CHARACTERS`].
fn notice_text(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= NOTICE_CHARACTERS {
        return flat;
    }
    let cut: String = flat.chars().take(NOTICE_CHARACTERS - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(conversations: serde_json::Value) -> Conversations {
        serde_json::from_value(serde_json::json!({
            "identity": "khaliq", "conversations": conversations
        }))
        .unwrap()
    }

    /// A conversation with `peer` whose newest message is `newest`, sent by
    /// the peer or by you.
    fn conversation(id: &str, peer: &str, newest: &str, theirs: bool) -> serde_json::Value {
        let sender = if theirs {
            format!("{peer}-id")
        } else {
            "me-id".to_string()
        };
        serde_json::json!({
            "id": id, "unread_count": null,
            "peer": {"name": peer, "agent_id": format!("{peer}-id")},
            "last_message": {"id": newest, "agent_id": sender, "text": format!("hi from {peer}")}
        })
    }

    #[test]
    fn a_message_is_announced_once_when_it_arrives() {
        let mut chat = ChatInbox::default();
        // There when the app starts: neither announced nor counted.
        assert!(chat
            .update(&listing(serde_json::json!([conversation(
                "c1", "ops", "m1", true
            )])))
            .is_empty());
        assert_eq!(chat.total(), 0);
        // A new message from the agent: announced once, counted.
        let notices = chat.update(&listing(serde_json::json!([conversation(
            "c1", "ops", "m2", true
        )])));
        assert_eq!(
            notices,
            vec![ChatNotice {
                peer: "ops".into(),
                text: "hi from ops".into()
            }]
        );
        assert!(chat
            .update(&listing(serde_json::json!([conversation(
                "c1", "ops", "m2", true
            )])))
            .is_empty());
        let unread = chat.unread_for(&["ops".into()]).unwrap();
        assert_eq!((unread.count, unread.newest_id.as_deref()), (1, Some("m2")));
        // Your own reply is not news; opening the conversation reads it.
        assert!(chat
            .update(&listing(serde_json::json!([conversation(
                "c1", "ops", "m3", false
            )])))
            .is_empty());
        chat.read("ops");
        assert_eq!(chat.total(), 0);
        // A released agent's conversation is ignored.
        chat.update(&listing(serde_json::json!([conversation(
            "c2", "unknown", "m9", true
        )])));
        assert!(chat
            .update(&listing(serde_json::json!([conversation(
                "c2", "unknown", "m10", true
            )])))
            .is_empty());
    }

    #[test]
    fn a_long_message_is_cut_on_one_line() {
        let text = notice_text(&format!("line one\nline two {}", "x".repeat(400)));
        assert!(text.starts_with("line one line two"));
        assert_eq!(text.chars().count(), NOTICE_CHARACTERS);
        assert!(text.ends_with('…'));
    }
}
