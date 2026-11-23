//! The session's own settings in the Chat header, as the sessions table had
//! them: its agent name on the relay, whether it receives messages, and
//! whether it uploads to Cloud. Each uses the same core call and the same
//! confirmation as before; the shown state is set without acting on it.

use super::super::app::App;
use super::super::widgets::{self, hbox, text};
use crate::models::{
    ChatAgent, CHAT_NAME, CHAT_NAME_HELP, CHAT_RECEIVES, CHAT_RECEIVES_HELP, CHAT_UPLOAD,
    CHAT_UPLOAD_HELP,
};
use agent_relay_node::relay::contract::agents::RenameParams;
use agent_relay_node::relay::contract::method;
use gtk::{glib, prelude::*};
use gtk4 as gtk;
use std::cell::Cell;
use std::rc::Rc;

pub struct Controls {
    pub root: gtk::Box,
    name_row: gtk::Box,
    name: gtk::Entry,
    receives_row: gtk::Box,
    receives: gtk::Switch,
    upload_row: gtk::Box,
    upload: gtk::Switch,
    /// Set while the controls show the session's state, so that is not a
    /// change.
    syncing: Rc<Cell<bool>>,
}

fn switch_row(label: &str, help: &str) -> (gtk::Box, gtk::Switch) {
    let row = hbox(6, &[]);
    row.set_valign(gtk::Align::Center);
    row.append(&text(label, &["caption"]));
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    switch.set_tooltip_text(Some(help));
    widgets::name(&switch, label);
    row.append(&switch);
    (row, switch)
}

impl Controls {
    pub fn build() -> Controls {
        let root = hbox(16, &["chat-controls"]);
        root.set_valign(gtk::Align::Center);
        let name_row = hbox(6, &[]);
        name_row.set_valign(gtk::Align::Center);
        name_row.append(&text(CHAT_NAME, &["caption"]));
        let name = gtk::Entry::new();
        name.set_width_chars(16);
        name.set_tooltip_text(Some(CHAT_NAME_HELP));
        widgets::name(&name, CHAT_NAME);
        name_row.append(&name);
        root.append(&name_row);
        let (receives_row, receives) = switch_row(CHAT_RECEIVES, CHAT_RECEIVES_HELP);
        root.append(&receives_row);
        let (upload_row, upload) = switch_row(CHAT_UPLOAD, CHAT_UPLOAD_HELP);
        root.append(&upload_row);
        Controls {
            root,
            name_row,
            name,
            receives_row,
            receives,
            upload_row,
            upload,
            syncing: Rc::new(Cell::new(false)),
        }
    }

    /// Shows `agent`'s settings: only a session on this computer has them,
    /// a name only once it is on the relay, Upload only once history has it.
    pub fn show(&self, agent: &ChatAgent, upload: Option<bool>) {
        self.syncing.set(true);
        let local = agent.local().is_some();
        self.receives_row.set_visible(local);
        self.receives.set_active(agent.address().is_some());
        let named = local && agent.address().is_some();
        self.name_row.set_visible(named);
        if named && !self.name.has_focus() {
            self.name.set_text(&agent.name());
        }
        self.upload_row.set_visible(upload.is_some());
        self.upload.set_active(upload.unwrap_or(false));
        self.syncing.set(false);
    }
}

impl App {
    pub(super) fn connect_chat_controls(&self, controls: &Controls) {
        for (switch, upload) in [(&controls.receives, false), (&controls.upload, true)] {
            let weak = self.downgrade();
            let syncing = controls.syncing.clone();
            switch.connect_state_set(move |_, on| {
                if let (Some(app), false) = (weak.upgrade(), syncing.get()) {
                    if upload {
                        app.set_chat_upload(on);
                    } else {
                        app.set_chat_receives(on);
                    }
                }
                glib::Propagation::Proceed
            });
        }
        let rename = self.on(App::rename_chat_agent);
        controls.name.connect_activate(move |_| rename());
    }

    fn shown_chat_agent(&self) -> Option<ChatAgent> {
        let key = self
            .with_chat(|page| page.selected.borrow().clone())
            .flatten()?;
        self.chat_agent(&key)
    }

    /// Puts the session shown on the relay or takes it off, as Receives
    /// messages did in the sessions table.
    fn set_chat_receives(&self, on: bool) {
        if let Some((_, session)) = self.shown_chat_agent().as_ref().and_then(ChatAgent::local) {
            self.toggle_relay(session, on);
        }
    }

    /// Uploads the session shown or stops, asking before a cloud copy is
    /// deleted, as Upload did in the sessions table.
    fn set_chat_upload(&self, on: bool) {
        let Some(agent) = self.shown_chat_agent() else {
            return;
        };
        if let Some(key) = agent.upload_key() {
            self.request_upload_change(vec![key], on);
            glib::idle_add_local_once(self.on(App::render_chat));
        }
    }

    /// Renames the session shown's agent on the relay to what is typed.
    fn rename_chat_agent(&self) {
        let Some(agent) = self.shown_chat_agent() else {
            return;
        };
        let Some(current) = agent.registered_name() else {
            return;
        };
        let typed = self
            .with_chat(|page| page.controls.name.text().to_string())
            .unwrap_or_default();
        let requested = typed.trim().trim_matches('@').to_lowercase();
        if requested.is_empty() || requested == agent.name() {
            return;
        }
        self.view_call::<method::AgentsRename>(
            RenameParams {
                name: current,
                new_name: requested,
            },
            |app, result| {
                let note = match result {
                    Ok(_) => None,
                    Err(error) => Some(error.message),
                };
                app.with_chat(|page| {
                    page.controls.name.set_text("");
                    if let Some(note) = &note {
                        page.note.set_text(note);
                    }
                });
                app.refresh_agents();
                app.refresh_local();
            },
        );
    }
}
