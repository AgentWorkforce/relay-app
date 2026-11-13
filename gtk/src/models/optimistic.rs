//! What a Settings list shows the moment a change is asked for, before the
//! core answers: Claude Code delivery consent. Pure: the Mac's
//! `OptimisticStatus` in `RelayDesktopCore` matches these.

use agent_relay_node::relay::contract::claude_delivery::{ClaudeDeliveryStatus, ClaudeSender};

/// What the app shows before the core confirms a change.
pub trait ClaudeDeliveryStatusOptimistic: Sized {
    fn restricting(self, restricted: bool) -> Self;
    fn with_session(self, id: &str, direct: bool) -> Self;
    fn without_sender(self, address: &str) -> Self;
    fn allowing(self, sender: ClaudeSender) -> Self;
    fn without_held(self, id: &str) -> Self;
}

impl ClaudeDeliveryStatusOptimistic for ClaudeDeliveryStatus {
    fn restricting(mut self, restricted: bool) -> Self {
        self.restricted = restricted;
        self
    }

    fn with_session(mut self, id: &str, direct: bool) -> Self {
        for session in &mut self.sessions {
            if session.id == id {
                session.direct = direct;
            }
        }
        self
    }

    fn without_sender(mut self, address: &str) -> Self {
        self.allowed_senders
            .retain(|sender| sender.address != address);
        self
    }

    /// With `sender` allowed, once.
    fn allowing(mut self, sender: ClaudeSender) -> Self {
        if !self
            .allowed_senders
            .iter()
            .any(|allowed| allowed.address == sender.address)
        {
            self.allowed_senders.push(sender);
        }
        self
    }

    fn without_held(mut self, id: &str) -> Self {
        self.held.retain(|message| message.id != id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_node::relay::contract::claude_delivery::{
        ClaudeDeliverySession, ClaudeHeldDelivery,
    };

    #[test]
    fn claude_delivery_lists_change_at_once() {
        let status = ClaudeDeliveryStatus {
            restricted: false,
            sessions: vec![ClaudeDeliverySession {
                id: "s1".into(),
                agent: "ada".into(),
                title: "".into(),
                direct: false,
            }],
            allowed_senders: vec![],
            held: vec![ClaudeHeldDelivery {
                id: "h1".into(),
                agent: "ada".into(),
                session_id: "s1".into(),
                sender: "bob@m-1".into(),
                sender_name: "Bob".into(),
                text: "hi".into(),
                held_at_ms: 1,
            }],
        };
        let changed = status
            .clone()
            .restricting(true)
            .with_session("s1", true)
            .allowing(ClaudeSender {
                address: "bob@m-1".into(),
                name: "Bob".into(),
            })
            .allowing(ClaudeSender {
                address: "bob@m-1".into(),
                name: "Bob".into(),
            })
            .without_held("h1");
        assert!(changed.restricted);
        assert!(changed.sessions[0].direct);
        assert_eq!(changed.allowed_senders.len(), 1);
        assert!(changed.held.is_empty());
        assert!(changed.without_sender("bob@m-1").allowed_senders.is_empty());
        assert_eq!(status.held.len(), 1);
    }
}
