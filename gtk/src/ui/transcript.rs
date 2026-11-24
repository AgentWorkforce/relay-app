//! The session a conversation in Chat is with: its controls in Chat's
//! details panel (its title, opening it in its own app, taking over or
//! ending a spawned agent, receiving messages, its agent name and its
//! upload), and the drawing of transcript entries Chat's thread uses.
use super::{
    app::App,
    session_links,
    widgets::{self, clear, hbox, text, vbox, LabelStyle},
};
use crate::models::{
    merge_local_sessions, session_header_identity, source_name, transcript_entry_context,
    AgentName, LocalSessionRow, SessionUploadState,
};
use agent_relay_node::relay::contract::{agents::RenameParams, method, transcript::LocalTranscriptEntry};
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};

mod entry;
use entry::copy_button;
pub(super) use entry::entry_view;
mod markdown;
use markdown::append_markdown;
mod receive_feedback;
use receive_feedback::sidebar_note;
mod controls;
mod rename;
mod title;
use title::render_identity;

/// The selected session's controls. Chat shows `sidebar` in its details
/// panel while the agent shown is a session on this computer.
pub struct TranscriptPage {
    pub sidebar: gtk::Box,
    pub open: gtk::Button,
    pub view_web: gtk::Button,
    /// Reads the conversation shown from the start again.
    pub refresh: gtk::Button,
    pub copy_name: gtk::Button,
    pub(super) opening_error: gtk::Label,
    /// Taking over and ending a spawned agent.
    pub join: super::spawn_join::SpawnJoin,
    pub save: gtk::Switch,
    pub receive: gtk::Switch,
    pub agent_name: gtk::Entry,
    pub rename: gtk::Button,
    status: gtk::Box,
    receive_status: gtk::Label,
    receive_progress: gtk::Spinner,
    control_error: gtk::Label,
    rename_status: gtk::Label,
    naming: gtk::Box,
    drawing: Cell<bool>,
    renaming: Cell<bool>,
    current_name: RefCell<Option<AgentName>>,
    title: gtk::Label,
    /// The session's title, editable, and under it the provider, agent
    /// name and presence badge.
    pub title_group: gtk::Box,
    pub(super) title_cell: RefCell<Option<super::session_titles::SessionTitleCell>>,
    pub subtitle: gtk::Label,
    identity: gtk::Box,
    pub presence_badge: gtk::Label,
    pub(super) selected: RefCell<Option<LocalSessionRow>>,
    /// Bumped whenever another session is selected: an answer for the one
    /// before is not shown.
    pub(super) generation: Cell<u64>,
}

impl TranscriptPage {
    pub fn build() -> Self {
        let title = text("", &["row-title"]).ellipsized();
        let subtitle = text("", &["caption"]).ellipsized();
        let presence_badge = text("", &["presence-badge"]);
        presence_badge.set_valign(gtk::Align::Center);
        presence_badge.set_visible(false);
        let identity = hbox(8, &[]);
        identity.append(&subtitle);
        identity.append(&presence_badge);
        let title_group = vbox(3, &[]);
        title_group.append(&title);
        title_group.append(&identity);

        let open = gtk::Button::with_label("Open in Terminal");
        widgets::action_cursor(&open);
        open.set_visible(false);
        open.set_hexpand(true);
        open.set_halign(gtk::Align::Fill);
        open.add_css_class("session-action");
        let view_web = gtk::Button::with_label("View on web");
        widgets::action_cursor(&view_web);
        view_web.set_visible(false);
        view_web.set_halign(gtk::Align::Fill);
        view_web.set_hexpand(true);
        view_web.add_css_class("session-action");
        view_web.set_tooltip_text(Some("Open this session in Cloud in your browser"));
        widgets::name(&view_web, "View on web");
        let refresh = gtk::Button::with_label("Refresh transcript");
        refresh.set_halign(gtk::Align::Fill);
        refresh.set_hexpand(true);
        refresh.add_css_class("session-action");
        widgets::action_cursor(&refresh);
        refresh.set_tooltip_text(Some("Refresh transcript"));
        widgets::name(&refresh, "Refresh transcript");
        let opening_error = text("", &["caption"]).wrapped();
        opening_error.set_visible(false);

        let join = super::spawn_join::SpawnJoin::build();
        let sidebar = vbox(12, &[]);
        sidebar.set_visible(false);
        sidebar.append(&title_group);
        let shortcuts = vbox(8, &[]);
        shortcuts.append(&open);
        shortcuts.append(&join.actions);
        shortcuts.append(&view_web);
        shortcuts.append(&refresh);
        shortcuts.append(&opening_error);
        sidebar.append(&shortcuts);
        sidebar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let messaging = vbox(10, &[]);
        messaging.append(&text("Messaging", &["row-title"]));
        let receiving = vbox(6, &[]);
        let receive = sidebar_switch(&receiving, "Receive messages");
        receive.set_tooltip_text(Some(
            "Receive messages from other agents. This also uploads the session to Cloud.",
        ));
        let (receive_status, receive_progress) = receive_feedback::build(&receiving);
        let control_error = sidebar_note("");
        control_error.set_visible(false);
        receiving.append(&control_error);
        messaging.append(&receiving);
        let naming = vbox(6, &[]);
        super::zoom::margins(&naming, 4, 0, 0, 0);
        naming.set_visible(false);
        let name_label = text("Agent name", &[]);
        name_label.set_tooltip_text(Some(
            "The name other agents see on Agent Relay. Your session title stays the same.",
        ));
        let name_heading = hbox(6, &[]);
        name_label.set_hexpand(true);
        name_heading.append(&name_label);
        let copy_name = gtk::Button::from_icon_name("edit-copy-symbolic");
        widgets::action_cursor(&copy_name);
        copy_name.set_tooltip_text(Some("Copy name"));
        widgets::name(&copy_name, "Copy name");
        copy_name.set_visible(false);
        name_heading.append(&copy_name);
        naming.append(&name_heading);
        let name_row = hbox(6, &[]);
        let agent_name = gtk::Entry::new();
        agent_name.set_placeholder_text(Some("Agent name"));
        agent_name.set_width_chars(10);
        agent_name.set_hexpand(true);
        widgets::name(&agent_name, "Agent name");
        name_row.append(&agent_name);
        let rename = gtk::Button::with_label("Rename agent");
        widgets::action_cursor(&rename);
        rename.set_halign(gtk::Align::Start);
        name_row.append(&rename);
        naming.append(&name_row);
        let rename_status = sidebar_note("");
        rename_status.set_selectable(true);
        rename_status.set_visible(false);
        naming.append(&rename_status);
        messaging.append(&naming);
        sidebar.append(&messaging);
        sidebar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let saving = vbox(6, &[]);
        let save = sidebar_switch(&saving, "Upload session");
        save.set_tooltip_text(Some("Upload this session and future changes to Cloud. While it uploads, it can join the relay by itself."));
        let status = vbox(6, &[]);
        saving.append(&status);
        sidebar.append(&saving);
        Self {
            sidebar,
            open,
            view_web,
            refresh,
            copy_name,
            opening_error,
            join,
            save,
            receive,
            agent_name,
            rename,
            status,
            receive_status,
            receive_progress,
            control_error,
            rename_status,
            naming,
            drawing: Cell::new(false),
            renaming: Cell::new(false),
            current_name: RefCell::default(),
            title,
            title_group,
            title_cell: RefCell::default(),
            subtitle,
            identity,
            presence_badge,
            selected: RefCell::default(),
            generation: Cell::new(0),
        }
    }
}

impl App {
    pub(super) fn connect_transcript(&self, page: &TranscriptPage) {
        let weak = self.downgrade();
        page.save.connect_state_set(move |switch, on| {
            let Some(app) = weak.upgrade() else {
                return gtk::glib::Propagation::Stop;
            };
            let Some(shell) = app.shell() else {
                return gtk::glib::Propagation::Stop;
            };
            let page = &shell.pages.sessions.transcript;
            if !page.drawing.get() {
                let key = page.selected.borrow().as_ref().map(|row| row.key());
                page.drawing.set(true);
                switch.set_active(switch.state());
                page.drawing.set(false);
                if let Some(key) = key {
                    app.request_upload_change(vec![key], on);
                }
            }
            gtk::glib::Propagation::Stop
        });
        let weak = self.downgrade();
        page.receive.connect_state_set(move |_, on| {
            let Some(app) = weak.upgrade() else {
                return gtk::glib::Propagation::Stop;
            };
            let Some(shell) = app.shell() else {
                return gtk::glib::Propagation::Stop;
            };
            let page = &shell.pages.sessions.transcript;
            if !page.drawing.get() {
                let session = page
                    .selected
                    .borrow()
                    .as_ref()
                    .and_then(|row| row.relay.clone());
                if let Some(session) = session {
                    app.toggle_relay(&session, on);
                }
            }
            gtk::glib::Propagation::Stop
        });
        let rename = self.on(App::rename_transcript_agent);
        page.rename.connect_clicked(move |_| rename());
        let rename = self.on(App::rename_transcript_agent);
        page.agent_name.connect_activate(move |_| rename());
        let web = self.on(App::view_transcript_on_web);
        page.view_web.connect_clicked(move |_| web());
        let refresh = self.on(App::refresh_chat_page);
        page.refresh.connect_clicked(move |button| {
            button.set_sensitive(false);
            refresh();
        });
        self.connect_spawn_join(page);
        let weak = self.downgrade();
        page.copy_name.connect_clicked(move |button| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let Some(shell) = app.shell() else {
                return;
            };
            let name = shell.pages.sessions.transcript.current_name.borrow().clone();
            if let Some(name) = name {
                button.clipboard().set_text(name.shown());
            }
        });
    }

    /// A read of the conversation shown ended: it can be refreshed again.
    pub(in crate::ui) fn chat_read_ended(&self) {
        if let Some(shell) = self.shell() {
            shell.pages.sessions.transcript.refresh.set_sensitive(true);
        }
    }

    /// Shows `row`'s controls, or none for an agent with no session here.
    /// The same session again keeps what is being typed and done.
    pub(in crate::ui) fn select_session_controls(&self, row: Option<&LocalSessionRow>) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.sessions.transcript;
        let Some(row) = row else {
            page.sidebar.set_visible(false);
            page.selected.borrow_mut().take();
            return;
        };
        page.sidebar.set_visible(true);
        let same = page
            .selected
            .borrow()
            .as_ref()
            .is_some_and(|selected| selected.key() == row.key());
        if same {
            self.render_transcript_controls();
            self.refresh_session_title_cells();
            return;
        }
        page.generation.set(page.generation.get() + 1);
        page.join.open_plan.borrow_mut().take();
        page.renaming.set(false);
        page.current_name.borrow_mut().take();
        page.rename_status.set_text("");
        page.rename_status.set_visible(false);
        self.install_transcript_title(page, row);
        page.opening_error.set_visible(false);
        *page.selected.borrow_mut() = Some(row.clone());
        self.render_transcript_controls();
        self.refresh_session_title_cells();
        self.load_session_title_capability(row);
        self.load_session_title_overview(row);
    }

    /// Opens the selected session's Cloud page with its current upload state.
    fn view_transcript_on_web(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let Some(original) = shell.pages.sessions.transcript.selected.borrow().clone() else {
            return;
        };
        let url = self.with_state(|state| {
            merge_local_sessions(&state.sessions, &state.local_sessions)
                .into_iter()
                .find(|row| row.key() == original.key())
                .unwrap_or(original)
                .upload
                .and_then(|upload| session_links::uploaded_page(state, &upload))
        });
        if let Some(url) = url {
            self.inner.desktop.open_uri(&url);
        }
    }

    pub(super) fn render_transcript_controls(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.sessions.transcript;
        let Some(original) = page.selected.borrow().clone() else {
            return;
        };
        self.with_state(|state| {
            let row = merge_local_sessions(&state.sessions, &state.local_sessions)
                .into_iter()
                .find(|row| row.key() == original.key())
                .unwrap_or(original);
            let key = row.key();
            page.drawing.set(true);
            let stopping = state.stopping.contains(&key);
            let requesting = state.requesting.contains(&key);
            let saved = !stopping
                && row
                    .upload
                    .as_ref()
                    .is_some_and(|upload| upload.included || requesting);
            page.view_web.set_visible(
                row.upload
                    .as_ref()
                    .and_then(|upload| session_links::uploaded_page(state, upload))
                    .is_some(),
            );
            // Turning an uploading session off deletes it from Cloud; when
            // the operator withdraws that, the switch is removed.
            let managed = saved && !state.can_delete_from_cloud();
            page.save.set_visible(!managed);
            page.save.set_active(saved);
            page.save.set_state(saved);
            page.save.set_sensitive(
                !self.session_hidden(&key)
                    && row.upload.is_some()
                    && !state.sessions_read_only()
                    && !stopping,
            );
            self.render_upload_status(page, state, &row, managed, requesting, stopping);
            let presence = row.relay.as_ref().map(|session| state.presence_of(session));
            let on = presence.is_some_and(|presence| presence.is_on());
            page.receive.set_active(on);
            page.receive.set_state(on);
            page.receive.set_sensitive(
                !self.session_hidden(&key)
                    && !page.renaming.get()
                    && row.relay.as_ref().is_some_and(|session| {
                        state.can_toggle(session, state.presence_of(session))
                    }),
            );
            page.receive_status
                .set_text(presence.map(|p| p.message_status()).unwrap_or("Offline"));
            receive_feedback::render(page, state, &row);
            let error = state.upload_errors.get(&key).cloned().or_else(|| {
                row.relay
                    .as_ref()
                    .and_then(|s| state.relay_error_of(s))
            });
            let help = if row
                .relay
                .as_ref()
                .is_none_or(|s| !s.live && s.registration.is_none())
            {
                "Start session to receive messages."
            } else if !state.has_workspace() {
                "Connect a workspace to receive messages."
            } else {
                ""
            };
            let control_message = error.as_deref().unwrap_or(help);
            page.control_error.set_text(control_message);
            page.control_error.set_visible(!control_message.is_empty());
            let agent = row
                .relay
                .as_ref()
                .and_then(|session| state.agent_for(session));
            let name = agent.map(AgentName::of);
            let shown_name = name
                .as_ref()
                .filter(|_| on)
                .map(|name| name.shown().to_string());
            if *page.current_name.borrow() != name {
                super::agent_names::show_in_field(&page.agent_name, name.as_ref());
                *page.current_name.borrow_mut() = name;
            }
            let busy = page.renaming.get()
                || agent.is_some_and(|agent| state.busy_targets.contains(&agent.target.key()));
            page.agent_name.set_visible(agent.is_some());
            page.agent_name.set_sensitive(!busy);
            super::transcript_notes::show_agent(&page.copy_name, &page.naming, agent);
            page.rename.set_visible(agent.is_some());
            page.rename.set_sensitive(!busy);
            page.rename.set_label(if page.renaming.get() {
                "Renaming…"
            } else {
                "Rename agent"
            });
            page.naming
                .set_visible(on && agent.is_some_and(|agent| !agent.name.is_empty()));
            render_identity(page, &row, presence, shown_name.as_deref());
            *page.selected.borrow_mut() = Some(row);
            page.drawing.set(false);
        });
        self.render_spawn_join();
    }
}

fn sidebar_switch(sidebar: &gtk::Box, title: &str) -> gtk::Switch {
    let line = hbox(10, &[]);
    let label = text(title, &[]);
    label.set_hexpand(true);
    line.append(&label);
    let switch = gtk::Switch::new();
    widgets::action_cursor(&switch);
    widgets::name(&switch, title);
    switch.set_valign(gtk::Align::Center);
    line.append(&switch);
    sidebar.append(&line);
    switch
}

pub(super) fn is_tool(entry: &LocalTranscriptEntry) -> bool {
    entry.kind == "tool_use" || entry.kind == "tool_result" || entry.role == "tool_result"
}

pub(super) fn tool_group_label(_count: usize) -> String {
    "Activity".into()
}

fn code_view(value: &str) -> gtk::ScrolledWindow {
    let label = text(value, &["mono", "transcript-code"]);
    label.set_selectable(true);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    scroll.set_child(Some(&label));
    scroll
}

#[cfg(test)]
mod tests {
    #[test]
    fn tool_groups_count_calls_and_recognize_result_roles() {
        assert_eq!(super::tool_group_label(0), "Activity");
        assert_eq!(super::tool_group_label(1), "Activity");
        assert_eq!(super::tool_group_label(3), "Activity");
        for (kind, role, expected) in [
            ("tool_use", "assistant", true),
            ("tool_result", "user", true),
            ("text", "tool_result", true),
            ("text", "user", false),
            ("thinking", "assistant", false),
            ("context", "assistant", false),
        ] {
            let entry = serde_json::from_value(serde_json::json!({
                "id": "a", "kind": kind, "role": role, "text": "", "timestamp_ms": 0, "truncated": false
            })).unwrap();
            assert_eq!(super::is_tool(&entry), expected);
        }
    }
}
