//! Chat: every agent on this computer in one place, as in a desktop chat
//! app. The list shows each agent's name, whether it is working or idle,
//! and its session summary; filters narrow it by status, harness and
//! project. The agent shown has its live conversation in a centered column
//! and a message box under it that sends through the relay; a session not
//! on the relay joins it on the first send. The Mac's `ChatView` is the
//! reference.

use super::app::App;
use super::state::Page;
use super::widgets::{self, clear, hbox, text, vbox, LabelStyle};
use crate::models::{
    chat_agents, chat_elsewhere_agents, chat_filter_choices, merge_local_sessions, ChatAgent,
    CHAT_DETAILS, CHAT_EMPTY_DETAIL, CHAT_EMPTY_TITLE, CHAT_HIDE_LIST, CHAT_SEARCH, CHAT_SHOW_LIST,
    CHAT_TITLE,
};
use gtk::{glib, prelude::*};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};

mod composer;
mod controls;
mod elsewhere;
mod filters;
mod images;
mod inbox;
mod list;
pub(in crate::ui) use inbox::with_unread;
mod open;
mod send;
mod sources;
mod thread;

use controls::Controls;
use filters::FilterBar;
pub use thread::Thread;

/// The stack's child while no agent is shown.
const EMPTY: &str = "empty";
const THREAD: &str = "thread";
/// How many listed agents have their summaries read ahead.
const SUMMARIES_AHEAD: usize = 24;

/// The page's widgets and what it shows.
pub struct ChatPage {
    pub root: gtk::Box,
    side: gtk::Box,
    filters: FilterBar,
    list: gtk::Box,
    stack: gtk::Stack,
    pub toggle_list: gtk::Button,
    /// Back to the sessions list, which every conversation opens from: at
    /// the head of the list, and in the header while the list is hidden.
    pub back: gtk::Button,
    back_header: gtk::Button,
    /// Opens the agent in its own app, where it has one.
    pub open: gtk::Button,
    pub header_badge: gtk::Box,
    pub header_name: gtk::Label,
    pub header_meta: gtk::Label,
    pub header_summary: gtk::Label,
    pub header_presence: gtk::Label,
    /// The session's name, Receives messages and Upload.
    pub controls: Controls,
    pub thread: Thread,
    /// The message box: attachments, input, Send and the note under them.
    pub composer: composer::Composer,
    /// The agent shown (`Target::key`).
    pub selected: RefCell<Option<String>>,
    /// What the list last drew, so a redraw with nothing new keeps it.
    drawn: RefCell<Vec<String>>,
    pub sending: Cell<bool>,
    /// The last send dropped its files (see `send.rs`).
    pub unsent_files: Cell<bool>,
    /// Your agents elsewhere as drawn at the head of the sessions list.
    elsewhere_rows: RefCell<Vec<gtk::ListBoxRow>>,
    elsewhere_drawn: RefCell<Vec<String>>,
    /// Your unread messages from agents (`inbox.rs`).
    pub inbox: RefCell<crate::models::ChatInbox>,
}

impl ChatPage {
    pub fn build() -> ChatPage {
        let root = hbox(0, &["chat-page"]);
        let side = vbox(8, &["sidebar", "chat-sidebar"]);
        widgets::pad(&side, 14, Some(10), None);
        super::zoom::size(&side, 260, -1);
        // Its rows' names expand within it; the list itself stays narrow.
        side.set_hexpand(false);
        // Back to the sessions list, at the head of the list as in Claude's
        // and Codex's apps, above the page's title.
        let back = super::session_open::back_to_sessions_button();
        side.append(&back);
        side.append(&text(CHAT_TITLE, &["page-title"]));
        let filters = FilterBar::build(CHAT_SEARCH);
        side.append(&filters.root);
        let list = vbox(2, &[]);
        side.append(&widgets::scrolled(&list));
        // Drag the divider to widen or narrow the list.
        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.set_hexpand(true);
        split.set_wide_handle(true);
        split.add_css_class("chat-split");
        split.set_start_child(Some(&side));
        split.set_resize_start_child(false);
        split.set_shrink_start_child(false);
        split.set_position(super::zoom::px(340));
        root.append(&split);

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::None);
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        let empty = vbox(10, &[]);
        empty.set_valign(gtk::Align::Center);
        empty.set_halign(gtk::Align::Center);
        empty.append(&text(CHAT_EMPTY_TITLE, &["placeholder-title"]).centered());
        let detail = text(CHAT_EMPTY_DETAIL, &["secondary"]).wrapped().centered();
        detail.set_max_width_chars(48);
        empty.append(&detail);
        stack.add_named(&empty, Some(EMPTY));

        let view = vbox(0, &[]);
        let header = hbox(12, &["chat-header"]);
        widgets::pad(&header, 10, Some(20), None);
        // Kenny's ask: hide the list and give the conversation the window.
        let toggle_list =
            widgets::icon_button("sidebar-show-symbolic", CHAT_HIDE_LIST, &["flat"], || {});
        toggle_list.set_valign(gtk::Align::Center);
        // The header's own back button shows only while the list is hidden.
        let back_header = super::session_open::back_to_sessions_button();
        back_header.set_visible(false);
        header.append(&back_header);
        header.append(&toggle_list);
        let header_badge = hbox(0, &[]);
        header_badge.set_valign(gtk::Align::Center);
        header.append(&header_badge);
        let words = vbox(2, &[]);
        words.set_hexpand(true);
        words.set_valign(gtk::Align::Center);
        let name_line = hbox(8, &[]);
        let header_name = text("", &["row-title"]).ellipsized();
        name_line.append(&header_name);
        let header_presence = text("", &["presence-badge"]);
        header_presence.set_valign(gtk::Align::Center);
        name_line.append(&header_presence);
        words.append(&name_line);
        let header_meta = text("", &["caption"]).ellipsized();
        words.append(&header_meta);
        let header_summary = text("", &["secondary"]).wrapped();
        header_summary.set_lines(3);
        header_summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        words.append(&header_summary);
        header.append(&words);
        let controls = Controls::build();
        let open = controls.open.clone();
        let details = gtk::ToggleButton::with_label(CHAT_DETAILS);
        details.add_css_class("flat");
        details.set_active(true);
        details.set_valign(gtk::Align::Center);
        widgets::action_cursor(&details);
        let shown = controls.root.clone();
        details.connect_toggled(move |toggle| shown.set_visible(toggle.is_active()));
        header.append(&details);
        view.append(&header);
        view.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let body = hbox(0, &[]);
        body.set_vexpand(true);
        let conversation = vbox(0, &[]);
        conversation.set_hexpand(true);
        body.append(&conversation);
        let edge = gtk::Separator::new(gtk::Orientation::Vertical);
        controls
            .root
            .bind_property("visible", &edge, "visible")
            .sync_create()
            .build();
        body.append(&edge);
        body.append(&controls.root);
        view.append(&body);
        let thread = Thread::build();
        conversation.append(&thread.view);

        let composer = composer::Composer::build();
        conversation.append(&composer.dock);
        stack.add_named(&view, Some(THREAD));
        split.set_end_child(Some(&stack));
        split.set_shrink_end_child(false);

        ChatPage {
            root,
            side,
            filters,
            list,
            stack,
            toggle_list,
            back,
            back_header,
            open,
            header_badge,
            header_name,
            header_meta,
            header_summary,
            header_presence,
            controls,
            thread,
            composer,
            selected: RefCell::default(),
            drawn: RefCell::default(),
            sending: Cell::new(false),
            unsent_files: Cell::new(false),
            elsewhere_rows: RefCell::default(),
            elsewhere_drawn: RefCell::default(),
            inbox: RefCell::default(),
        }
    }
}

impl App {
    /// Connects the page's own controls; built once with the shell.
    pub(super) fn connect_chat_page(&self, page: &ChatPage) {
        page.filters.connect_changed(self.on(App::render_chat));
        self.connect_chat_composer(&page.composer);
        let send = self.on(App::send_chat_message);
        page.composer.input.connect_activate(send);
        let send = self.on(App::send_chat_message);
        page.composer.send.connect_clicked(move |_| send());
        self.connect_chat_controls(&page.controls);
        let home = self.on(|app| app.open_section(super::state::Section::Mine));
        page.back.connect_clicked(move |_| home());
        let home = self.on(|app| app.open_section(super::state::Section::Mine));
        page.back_header.connect_clicked(move |_| home());
        let native = self.on(App::open_chat_agent_natively);
        page.open.connect_clicked(move |_| native());
        let toggle = self.on(App::toggle_chat_list);
        page.toggle_list.connect_clicked(move |_| toggle());
        let earlier = self.on(App::show_earlier_chat);
        page.thread.earlier.connect_clicked(move |_| earlier());
        self.follow_chat_thread();
        self.follow_chat_inbox();
    }

    pub(super) fn with_chat<T>(&self, read: impl FnOnce(&ChatPage) -> T) -> Option<T> {
        self.shell().map(|shell| read(&shell.pages.chat))
    }

    /// Every agent on this computer, then your agents elsewhere, as the
    /// page lists them now.
    pub(super) fn chat_agents(&self) -> Vec<ChatAgent> {
        // Someone else's agent is listed when the search finds it, while
        // it is the one shown, and whenever it sent you a message you have
        // not opened.
        let (query, shown) = self
            .with_chat(|page| (page.filters.read().query, page.selected.borrow().clone()))
            .unwrap_or_default();
        let others = |agent: &ChatAgent| {
            (!query.trim().is_empty() && agent.matches(&query))
                || shown.as_deref() == Some(agent.key().as_str())
                || self.chat_unread(&agent.relay_names()) > 0
        };
        self.with_state(|state| {
            // Every session here, whatever the sessions list's search shows
            // (Chat has its own search); sessions that search found beyond
            // them are kept, so one opened from its results is listed.
            let mut local = state.local_sessions.clone();
            for found in state.session_query.local_sessions.iter().flatten() {
                if !local.iter().any(|s| s.target.key() == found.target.key()) {
                    local.push(found.clone());
                }
            }
            let rows = merge_local_sessions(&state.sessions, &local);
            let now_ms = chrono::Utc::now().timestamp_millis();
            chat_agents(
                &rows,
                |session| {
                    let name = session.registration.as_ref()?.name.clone();
                    state
                        .local_agents
                        .iter()
                        .find(|agent| agent.name == name)
                        .and_then(|agent| agent.spawn.as_ref())
                        .map(|spawn| spawn.work.state.clone())
                },
                |row| {
                    state.title_overviews.get(&row.key()).map(|overview| {
                        if overview.description.trim().is_empty() {
                            overview.title.clone()
                        } else {
                            overview.description.clone()
                        }
                    })
                },
                now_ms,
            )
            .into_iter()
            .chain(chat_elsewhere_agents(&state.roster, &rows, now_ms, &others))
            .collect()
        })
    }

    pub(super) fn chat_agent(&self, key: &str) -> Option<ChatAgent> {
        self.chat_agents()
            .into_iter()
            .find(|agent| agent.key() == key)
    }

    /// Opens the Chat page, on `key` when given.
    pub fn show_chat_page(&self, key: Option<&str>) {
        // The agent is chosen before the page shows, so its first drawing
        // is of that agent.
        if let Some(key) = key {
            self.select_chat_agent(key);
        }
        if self.page() != Page::Chat {
            self.go(Page::Chat);
        }
    }

    /// Opening Chat never shows an empty screen while an agent runs: the
    /// most recently active one (`first`) is shown, unless something is
    /// chosen meanwhile, such as a row opened from the sessions list.
    fn pick_first_chat_agent(&self, first: Option<String>) {
        let Some(key) = first else {
            return;
        };
        glib::idle_add_local_once(self.on(move |app| {
            let chosen = app
                .with_chat(|page| page.selected.borrow().is_some())
                .unwrap_or(true);
            if !chosen {
                app.select_chat_agent(&key);
            }
        }));
    }

    /// Opens a sessions-table row as a conversation with its agent.
    pub fn open_session_conversation(&self, row: &crate::models::LocalSessionRow) {
        // A session in an app of its own (a Grok Bot chat) opens there when
        // the app is installed.
        if !self.open_in_its_app(row) {
            self.show_chat_page(Some(&crate::models::chat_key(row)));
        }
    }

    /// Opens `row`'s session in its own app when it is one with an app of
    /// its own and that app is installed; whether it did.
    fn open_in_its_app(&self, row: &crate::models::LocalSessionRow) -> bool {
        crate::models::chat_app_link(row)
            .is_some_and(|link| app_installed_for(&link) && launch_raised(&link))
    }

    /// Draws the filters, the list and the header from what was last read.
    pub fn render_chat(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.chat;
        let agents = self.chat_agents();
        let (harnesses, projects) = chat_filter_choices(&agents);
        page.filters.offer(&harnesses, &projects);
        let filters = page.filters.read();
        let mut shown: Vec<&ChatAgent> = agents
            .iter()
            .filter(|agent| filters.admits(agent))
            .collect();
        // Agents with unread messages first, newest activity among them
        // first as before; the rest keep their order.
        shown.sort_by_key(|agent| self.chat_unread(&agent.relay_names()) == 0);
        if page.selected.borrow().is_none() {
            self.pick_first_chat_agent(shown.first().map(|agent| agent.key()));
        }
        let selected = page.selected.borrow().clone();
        let drawn: Vec<String> = shown
            .iter()
            .map(|agent| {
                format!(
                    "{}|{:?}|{}|{}|{}|{}",
                    agent.key(),
                    agent.activity,
                    agent.name(),
                    agent.glance(),
                    selected.as_deref() == Some(agent.key().as_str()),
                    self.chat_unread(&agent.relay_names())
                )
            })
            .collect();
        if *page.drawn.borrow() != drawn {
            self.draw_chat_list(page, &shown, agents.is_empty(), selected.as_deref());
            *page.drawn.borrow_mut() = drawn;
        }
        for agent in shown.iter().take(SUMMARIES_AHEAD) {
            if let Some(row) = agent.row() {
                self.load_session_title_overview(row);
            }
        }
        match selected.and_then(|key| agents.iter().find(|agent| agent.key() == key).cloned()) {
            Some(agent) => {
                page.stack.set_visible_child_name(THREAD);
                self.render_chat_header(page, &agent);
            }
            None => page.stack.set_visible_child_name(EMPTY),
        }
    }

    fn render_chat_header(&self, page: &ChatPage, agent: &ChatAgent) {
        clear(&page.header_badge);
        page.header_badge.append(&agent_badge(agent, 32));
        let upload = agent.upload_key().map(|key| {
            self.with_state(|state| {
                state.requesting.contains(&key)
                    || (!state.stopping.contains(&key)
                        && agent
                            .row()
                            .and_then(|row| row.upload.as_ref())
                            .is_some_and(|upload| upload.included))
            })
        });
        page.controls
            .show(agent, upload, self.chat_web_url(agent).as_deref());
        self.render_chat_open(&page.open, agent);
        page.header_name.set_text(&agent.name());
        let mut meta = agent.meta();
        if let Some(address) = agent.address() {
            meta = format!("{meta} · {address}");
        }
        page.header_meta.set_text(&meta);
        page.header_meta.set_tooltip_text(Some(&agent.title()));
        let glance = agent.glance();
        page.header_summary.set_text(&glance);
        page.header_summary.set_visible(!glance.is_empty());
        page.header_presence.set_text(agent.activity.label());
        for class in ["working", "idle", "offline"] {
            page.header_presence.remove_css_class(class);
        }
        page.header_presence.add_css_class(agent.activity.class());
        page.composer
            .input
            .set_placeholder_text(Some(&agent.placeholder()));
        widgets::name(&page.composer.input.view, &agent.placeholder());
        page.composer
            .send
            .set_sensitive(agent.can_send() && !page.sending.get());
        if !page.sending.get() && page.thread.status().is_none() {
            page.composer.note.set_text(&agent.composer_note());
        }
    }

    /// Shows the agent with `key`: its conversation is read from the start.
    pub fn select_chat_agent(&self, key: &str) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.chat;
        let changed = page.selected.replace(Some(key.to_string())).as_deref() != Some(key);
        if changed {
            page.composer.input.set_text("");
            page.thread.set_status(None);
            if let Some(agent) = self.chat_agent(key) {
                page.composer.note.set_text(&agent.composer_note());
            }
            self.load_chat_thread(key);
            // Opening a conversation reads it, here and on your other
            // devices.
            if let Some(agent) = self.chat_agent(key) {
                self.mark_chat_read(&agent.relay_names());
            }
        }
        self.render_chat();
        page.composer.input.view.grab_focus();
    }

    /// Reads the sessions, the roster and the conversation shown again.
    pub(super) fn refresh_chat_page(&self) {
        self.refresh_local();
        self.refresh_roster();
        if let Some(key) = self
            .with_chat(|page| page.selected.borrow().clone())
            .flatten()
        {
            self.load_chat_thread(&key);
        }
    }

    /// Hides the agent list so the conversation takes the window, or shows
    /// it again.
    fn toggle_chat_list(&self) {
        self.with_chat(|page| {
            let shown = !page.side.is_visible();
            page.side.set_visible(shown);
            page.back_header.set_visible(!shown);
            let label = if shown {
                CHAT_HIDE_LIST
            } else {
                CHAT_SHOW_LIST
            };
            page.toggle_list.set_tooltip_text(Some(label));
            widgets::name(&page.toggle_list, label);
        });
    }
}

/// The agent's mark: its session's avatar here, else its harness's.
fn agent_badge(agent: &ChatAgent, size: i32) -> gtk::Widget {
    match agent.row() {
        Some(row) => super::session_avatar::row_badge(row, size),
        None => widgets::agent_badge(&agent.icon_source(), size),
    }
}

/// Opens `link` in its app with this display's launch context, which hands
/// the app an activation token: on Wayland an app may raise its window only
/// with one, and without it a click seems to do nothing.
fn launch_raised(link: &str) -> bool {
    let context = gtk::gdk::Display::default().map(|display| display.app_launch_context());
    gtk::gio::AppInfo::launch_default_for_uri(link, context.as_ref()).is_ok()
}

/// Whether an installed app takes `link`'s scheme.
fn app_installed_for(link: &str) -> bool {
    link.split_once(':')
        .is_some_and(|(scheme, _)| gtk::gio::AppInfo::default_for_uri_scheme(scheme).is_some())
}
