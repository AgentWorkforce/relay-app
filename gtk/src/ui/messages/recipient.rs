//! Following a conversation's recipient: whether it runs, and what it
//! met after a send, read from the core and shown under the conversation.
use super::*;

/// The line under a conversation before anything is known.
const EXACT_ADDRESS: &str = "An exact address reaches only that agent on that machine.";

fn tone_class(tone: DeliveryTone) -> &'static str {
    match tone {
        DeliveryTone::Delivered => "ok",
        DeliveryTone::Waiting => "warn",
        DeliveryTone::Failed => "error",
        DeliveryTone::Unknown => "secondary",
    }
}

/// Once a message is picked up or has no recipient, nothing more is
/// learned about it. One Claude Code holds for approval is followed until
/// it reaches the session.
fn settled(state: Option<MessageState>) -> bool {
    matches!(
        state,
        Some(MessageState::PickedUp | MessageState::NoSuchAgent)
    )
}

impl App {
    /// Looks the recipient up again for the conversation shown, and for one
    /// whose last message may still change state.
    pub(super) fn follow_recipient(&self, key: &str) {
        let Some(chat) = self.chat(key) else {
            return;
        };
        if chat.state.borrow().backing_off() {
            return;
        }
        let shown = self.shown_chat().as_deref() == Some(key);
        let unsettled = chat
            .state
            .borrow()
            .last_send
            .as_ref()
            .is_some_and(|sent| !settled(sent.state));
        if shown || unsettled {
            self.look_up_recipient(key);
        }
    }

    /// The core said something a followed recipient depends on changed.
    pub(in crate::ui) fn follow_recipients(&self) {
        for key in self.open_chats() {
            self.follow_recipient(&key);
        }
    }

    /// Reads the recipient and, after a send, what Relaycast holds for the
    /// message. Asked again while a read runs, it reads once more when that
    /// one answers. A read that cannot tell keeps the last state Relaycast
    /// reported; an answer for a closed conversation, or older than a send,
    /// is dropped.
    pub fn look_up_recipient(&self, key: &str) {
        let Some(chat) = self.chat(key) else {
            return;
        };
        if !self.bridge().offers(method::RosterGet::NAME) {
            return;
        }
        let (read, message_id) = {
            let mut state = chat.state.borrow_mut();
            if state.recipient_loading {
                state.recipient_pending = true;
                return;
            }
            state.recipient_loading = true;
            state.recipient_pending = false;
            state.recipient_reads += 1;
            (
                state.recipient_reads,
                state
                    .last_send
                    .as_ref()
                    .and_then(|sent| sent.message_id.clone()),
            )
        };
        let mut params = recipient_params(key);
        params.message_id = message_id.clone();
        let key = key.to_string();
        self.view_call::<method::RosterGet>(params, move |app, result| {
            if !app
                .chat(&key)
                .is_some_and(|current| Rc::ptr_eq(&current, &chat))
            {
                return;
            }
            let again = {
                let mut state = chat.state.borrow_mut();
                state.recipient_loading = false;
                std::mem::take(&mut state.recipient_pending)
            };
            if again {
                let key = key.clone();
                glib::idle_add_local_once(app.on(move |app| app.look_up_recipient(&key)));
            }
            if read != chat.state.borrow().recipient_reads {
                return;
            }
            let recipient = match result {
                Ok(recipient) => recipient,
                Err(error) => return app.back_off_chat(&chat, Read::Recipient, &error.message),
            };
            app.chat_read(&chat, Read::Recipient);
            {
                let mut state = chat.state.borrow_mut();
                if let (Some(message), Some(sent)) = (recipient.message, state.last_send.as_mut()) {
                    if sent.message_id == message_id {
                        sent.state = Some(message);
                    }
                }
                state.recipient = Some(recipient);
            }
            app.render_delivery(&key);
        });
    }

    /// Before a send: whether the recipient runs. After one: what it met,
    /// updated as the recipient comes and goes.
    pub(super) fn render_delivery(&self, key: &str) {
        let Some(chat) = self.chat(key) else {
            return;
        };
        let state = chat.state.borrow();
        let recipient = state.recipient.as_ref();
        let (line, class) = if let Some(error) = &state.error {
            (error.clone(), "error")
        } else if let Some(sent) = &state.last_send {
            let (note, tone) = delivery_note(sent.state, recipient);
            (note.to_string(), tone_class(tone))
        } else if let Some((recipient, warning)) = recipient
            .and_then(|recipient| recipient_warning(recipient).map(|warning| (recipient, warning)))
        {
            (
                warning,
                if recipient.agent.is_none() {
                    "error"
                } else {
                    "warn"
                },
            )
        } else {
            (EXACT_ADDRESS.to_string(), "secondary")
        };
        for name in ["ok", "warn", "error", "secondary"] {
            chat.delivery.remove_css_class(name);
        }
        chat.delivery.add_css_class(class);
        chat.delivery.set_text(&line);
        let note = recipient.and_then(|recipient| reply_note(recipient, super::super::now_ms()));
        chat.reply.set_text(note.as_deref().unwrap_or_default());
        chat.reply.set_visible(note.is_some());
        chat.send.set_label(if state.sending {
            "Sending…"
        } else if queues(recipient) {
            "Queue Anyway"
        } else {
            "Send"
        });
    }
}
