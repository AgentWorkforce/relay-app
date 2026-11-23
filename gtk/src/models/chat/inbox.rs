//! Your unread direct messages, as the core reads them from Relaycast
//! (`messages.inbox`): the read state every device shares, so a message
//! read on the iOS app or the dashboard is read here too. Each new reading
//! is compared with the last, so a message is announced once, when it
//! arrives, and what was already unread when the app started is not.

use agent_relay_node::relay::contract::directory::MessageInbox;
use std::collections::HashMap;

/// The peer Relaycast reports for a conversation whose other side left.
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
    pub fn update(&mut self, inbox: &MessageInbox) -> Vec<ChatNotice> {
        let mut notices = Vec::new();
        let mut unread = HashMap::new();
        // Relaycast names the peer `unknown` once it left the conversation
        // (a released agent): nothing new comes from it, and no agent here
        // goes by that name.
        let live = inbox
            .unread
            .iter()
            .filter(|c| c.unread_count > 0 && c.peer != GONE);
        for conversation in live {
            let last = conversation.last_message.as_ref();
            let newest_id = last.map(|message| message.id.clone());
            if let Some(message) = last {
                let before = self
                    .newest
                    .insert(conversation.conversation_id.clone(), message.id.clone());
                if self.primed && before.as_deref() != Some(message.id.as_str()) {
                    notices.push(ChatNotice {
                        peer: conversation.peer.clone(),
                        text: notice_text(&message.text),
                    });
                }
            }
            let entry = unread.entry(conversation.peer.clone()).or_insert(Unread {
                count: 0,
                newest_id: None,
            });
            entry.count += conversation.unread_count;
            if entry.newest_id.is_none() {
                entry.newest_id = newest_id;
            }
        }
        self.unread = unread;
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

    fn inbox(conversations: serde_json::Value) -> MessageInbox {
        serde_json::from_value(serde_json::json!({
            "identity": "khaliq", "unread": conversations, "unread_channels": []
        }))
        .unwrap()
    }

    fn conversation(id: &str, peer: &str, count: u64, newest: &str) -> serde_json::Value {
        serde_json::json!({
            "conversation_id": id, "peer": peer, "unread_count": count,
            "last_message": {"id": newest, "text": format!("hi from {peer}"), "created_at": null}
        })
    }

    #[test]
    fn a_message_is_announced_once_when_it_arrives() {
        let mut chat = ChatInbox::default();
        // Unread when the app starts: counted, not announced.
        assert!(chat
            .update(&inbox(serde_json::json!([conversation(
                "c1", "ops", 2, "m1"
            )])))
            .is_empty());
        assert_eq!(chat.unread_for(&["ops".into()]).unwrap().count, 2);
        // A new message: announced once.
        let notices = chat.update(&inbox(serde_json::json!([conversation(
            "c1", "ops", 3, "m2"
        )])));
        assert_eq!(
            notices,
            vec![ChatNotice {
                peer: "ops".into(),
                text: "hi from ops".into()
            }]
        );
        assert!(chat
            .update(&inbox(serde_json::json!([conversation(
                "c1", "ops", 3, "m2"
            )])))
            .is_empty());
        // Read elsewhere (iOS): gone here too.
        chat.update(&inbox(serde_json::json!([])));
        assert!(chat.unread_for(&["ops".into()]).is_none());
        assert_eq!(chat.total(), 0);
        // A conversation with a released agent is neither counted nor
        // announced.
        chat.update(&inbox(serde_json::json!([conversation(
            "c2", "unknown", 4, "m9"
        )])));
        assert_eq!(chat.total(), 0);
        assert!(chat
            .update(&inbox(serde_json::json!([conversation(
                "c2", "unknown", 5, "m10"
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
