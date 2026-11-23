//! The details sidebar beside a conversation, laid out as the session
//! reader's was: opening the session in its own app, viewing it on the web,
//! then its messaging settings (whether it receives messages, its agent name
//! on the relay) and whether it uploads. Each uses the same core call and
//! confirmation as before; the shown state is set without acting on it.

use super::super::app::App;
use super::super::widgets::{self, hbox, text, vbox};
use crate::models::{
    chat_web_url, ChatAgent, CHAT_MESSAGING, CHAT_NAME, CHAT_NAME_HELP, CHAT_RECEIVES,
    CHAT_RECEIVES_HELP, CHAT_UPLOAD, CHAT_UPLOAD_HELP, CHAT_VIEW_WEB, CHAT_VIEW_WEB_HELP,
};
use agent_relay_node::relay::contract::agents::RenameParams;
use agent_relay_node::relay::contract::method;
use gtk::{glib, prelude::*};
use gtk4 as gtk;
use std::cell::Cell;
use std::rc::Rc;

pub struct Controls {
    /// The sidebar, shown or hidden by the header's Details button.
    pub root: gtk::Box,
    /// Opens the session in its own app (`open.rs`).
    pub open: gtk::Button,
    pub view_web: gtk::Button,
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
    let title = text(label, &[]);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Start);
    row.append(&title);
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    switch.set_tooltip_text(Some(help));
    widgets::name(&switch, label);
    row.append(&switch);
    (row, switch)
}

impl Controls {
    pub fn build() -> Controls {
        let root = vbox(12, &["chat-details"]);
        super::super::zoom::size(&root, 264, -1);
        widgets::pad(&root, 16, Some(16), None);
        let open = super::open::open_button();
        open.set_halign(gtk::Align::Fill);
        open.set_hexpand(true);
        let view_web = gtk::Button::with_label(CHAT_VIEW_WEB);
        view_web.add_css_class("session-action");
        view_web.set_tooltip_text(Some(CHAT_VIEW_WEB_HELP));
        widgets::action_cursor(&view_web);
        widgets::name(&view_web, CHAT_VIEW_WEB);
        let actions = vbox(8, &[]);
        actions.append(&open);
        actions.append(&view_web);
        root.append(&actions);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let messaging = vbox(10, &[]);
        messaging.append(&text(CHAT_MESSAGING, &["row-title"]));
        let (receives_row, receives) = switch_row(CHAT_RECEIVES, CHAT_RECEIVES_HELP);
        messaging.append(&receives_row);
        let name_row = vbox(6, &[]);
        let name_label = text(CHAT_NAME, &[]);
        name_label.set_halign(gtk::Align::Start);
        name_label.set_tooltip_text(Some(CHAT_NAME_HELP));
        name_row.append(&name_label);
        let name = gtk::Entry::new();
        name.set_tooltip_text(Some(CHAT_NAME_HELP));
        widgets::name(&name, CHAT_NAME);
        name_row.append(&name);
        messaging.append(&name_row);
        root.append(&messaging);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let (upload_row, upload) = switch_row(CHAT_UPLOAD, CHAT_UPLOAD_HELP);
        root.append(&upload_row);
        Controls {
            root,
            open,
            view_web,
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
    pub fn show(&self, agent: &ChatAgent, upload: Option<bool>, web: Option<&str>) {
        self.syncing.set(true);
        self.view_web.set_visible(web.is_some());
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
        let web = self.on(App::view_chat_agent_on_web);
        controls.view_web.connect_clicked(move |_| web());
    }

    /// Where the agent shown is on the web: its uploaded session in Cloud.
    pub(super) fn chat_web_url(&self, agent: &ChatAgent) -> Option<String> {
        let site = self.with_state(|state| state.account().site_url.unwrap_or_default());
        chat_web_url(agent, &site)
    }

    fn view_chat_agent_on_web(&self) {
        if let Some(url) = self
            .shown_chat_agent()
            .and_then(|agent| self.chat_web_url(&agent))
        {
            self.inner.desktop.open_uri(&url);
        }
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
