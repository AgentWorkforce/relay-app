//! Sending from the Chat page. A session on the relay is messaged at its
//! exact address; one that is not joins it first (`agents.register`, as
//! the Receive messages switch does), then gets the message. The box
//! clears at once and a refusal puts the text back.

use super::super::app::App;
use crate::bridge::CoreError;
use crate::models::{delivery_note, recipient_params, ChatAgent, CHAT_JOINING, CHAT_SENDING};
use agent_relay_node::relay::contract::agents::RegisterParams;
use agent_relay_node::relay::contract::directory::{SendParams, SentMessage};
use agent_relay_node::relay::contract::method;
use gtk::prelude::*;
use gtk4 as gtk;

impl App {
    /// Sends what is typed to the agent shown.
    pub(super) fn send_chat_message(&self) {
        let Some((key, body)) = self
            .with_chat(|page| {
                let key = page.selected.borrow().clone()?;
                let body = page.input.text().trim().to_string();
                (!body.is_empty() && !page.sending.get()).then_some((key, body))
            })
            .flatten()
        else {
            return;
        };
        let Some(agent) = self.chat_agent(&key) else {
            return;
        };
        self.with_chat(|page| {
            page.sending.set(true);
            page.send.set_sensitive(false);
            page.input.set_text("");
            page.thread.add_pending(&body);
            page.thread.set_status(None);
            clear_error(&page.note);
            page.note.set_text(if agent.address().is_some() {
                CHAT_SENDING
            } else {
                CHAT_JOINING
            });
        });
        match agent.address() {
            Some(address) => self.deliver_chat(&key, address, body),
            None => self.join_then_send(agent, body),
        }
    }

    fn join_then_send(&self, agent: ChatAgent, body: String) {
        let key = agent.key();
        self.update_state(|state| {
            state.wanted.insert(key.clone(), true);
        });
        self.view_call::<method::AgentsRegister>(
            RegisterParams {
                target: agent.session.target.clone(),
                name: None,
                description: None,
            },
            move |app, result| match result {
                Ok(registered) => {
                    let address = registered.address.clone();
                    app.remember_receive_registration(&key, Some(registered));
                    app.refresh_local();
                    app.deliver_chat(&key, address, body);
                }
                Err(error) => {
                    app.update_state(|state| {
                        state.wanted.remove(&key);
                    });
                    app.chat_send_failed(&key, &body, &error);
                }
            },
        );
    }

    fn deliver_chat(&self, key: &str, address: String, body: String) {
        let recipient = recipient_params(&address);
        let key = key.to_string();
        self.view_call::<method::MessagesSend>(
            SendParams {
                to: recipient.to,
                address: recipient.address,
                text: body.clone(),
                reply_to: None,
            },
            move |app, result: Result<SentMessage, CoreError>| match result {
                Ok(sent) => {
                    let (note, _) = delivery_note(sent.recipient.message, Some(&sent.recipient));
                    app.with_chat(|page| {
                        page.sending.set(false);
                        page.send.set_sensitive(true);
                        if page.selected.borrow().as_deref() == Some(key.as_str()) {
                            page.thread.set_status(Some(note.to_string()));
                            page.note.set_text(note);
                        }
                    });
                    app.render_chat();
                }
                Err(error) => app.chat_send_failed(&key, &body, &error),
            },
        );
    }

    fn chat_send_failed(&self, key: &str, body: &str, error: &CoreError) {
        self.with_chat(|page| {
            page.sending.set(false);
            page.send.set_sensitive(true);
            if page.selected.borrow().as_deref() != Some(key) {
                return;
            }
            page.thread.clear_pending();
            page.thread.stop_working();
            if page.input.text().trim().is_empty() {
                page.input.set_text(body);
            }
            page.thread.set_status(Some(error.message.clone()));
            page.note.set_text(&error.message);
            page.note.add_css_class("error");
        });
    }
}

/// The note's error look ends with the next send.
fn clear_error(note: &gtk::Label) {
    note.remove_css_class("error");
}
