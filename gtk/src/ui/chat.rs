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
    chat_agents, chat_all_sessions, chat_elsewhere_agents, chat_filter_choices,
    merge_local_sessions, ChatAgent, CHAT_ELSEWHERE_SECTION, CHAT_EMPTY_DETAIL, CHAT_EMPTY_TITLE,
    CHAT_HIDE_LIST, CHAT_NO_AGENTS, CHAT_NO_MATCH, CHAT_OPEN_SESSION, CHAT_SEARCH, CHAT_SEND,
    CHAT_SHOW_LIST, CHAT_TITLE,
};
use gtk::{glib, prelude::*};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};

mod filters;
mod send;
mod thread;

use super::spawn_join::ChatInput;
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
    all_sessions: gtk::Button,
    stack: gtk::Stack,
    pub toggle_list: gtk::Button,
    pub header_badge: gtk::Box,
    pub header_name: gtk::Label,
    pub header_meta: gtk::Label,
    pub header_summary: gtk::Label,
    pub header_presence: gtk::Label,
    pub open_session: gtk::Button,
    pub thread: Thread,
    pub input: ChatInput,
    pub send: gtk::Button,
    pub note: gtk::Label,
    /// The agent shown (`Target::key`).
    pub selected: RefCell<Option<String>>,
    /// What the list last drew, so a redraw with nothing new keeps it.
    drawn: RefCell<Vec<String>>,
    pub sending: Cell<bool>,
}

impl ChatPage {
    pub fn build() -> ChatPage {
        let root = hbox(0, &["chat-page"]);
        let side = vbox(8, &["sidebar", "chat-sidebar"]);
        widgets::pad(&side, 14, Some(10), None);
        super::zoom::size(&side, 260, -1);
        // Its rows' names expand within it; the list itself stays narrow.
        side.set_hexpand(false);
        side.append(&text(CHAT_TITLE, &["page-title"]));
        let filters = FilterBar::build(CHAT_SEARCH);
        side.append(&filters.root);
        let list = vbox(2, &[]);
        side.append(&widgets::scrolled(&list));
        // The table of every session, where uploads and sharing are managed.
        let all_sessions = gtk::Button::with_label(&chat_all_sessions(0));
        all_sessions.add_css_class("flat");
        all_sessions.add_css_class("chat-all-sessions");
        all_sessions.set_halign(gtk::Align::Start);
        widgets::action_cursor(&all_sessions);
        side.append(&all_sessions);
        // Drag the divider to widen or narrow the list.
        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.set_hexpand(true);
        split.set_wide_handle(true);
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
        let open_session = gtk::Button::with_label(CHAT_OPEN_SESSION);
        open_session.add_css_class("flat");
        open_session.set_valign(gtk::Align::Center);
        widgets::action_cursor(&open_session);
        header.append(&open_session);
        view.append(&header);
        view.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let thread = Thread::build();
        view.append(&thread.scroll);

        let dock = vbox(6, &["chat-dock"]);
        widgets::pad(&dock, 8, Some(24), Some(14));
        let (column, holder) = widgets::column(760);
        widgets::pad(&column, 0, Some(0), Some(0));
        holder.set_halign(gtk::Align::Center);
        super::zoom::size(&column, 560, -1);
        let line = hbox(8, &["chat-composer"]);
        let input = ChatInput::build();
        line.append(&input.root);
        input.focus_within(&line);
        let send = gtk::Button::with_label(CHAT_SEND);
        widgets::action_cursor(&send);
        send.add_css_class("primary");
        send.set_valign(gtk::Align::End);
        line.append(&send);
        column.append(&line);
        let note = text("", &["caption"]).wrapped();
        column.append(&note);
        dock.append(&holder);
        view.append(&dock);
        stack.add_named(&view, Some(THREAD));
        split.set_end_child(Some(&stack));
        split.set_shrink_end_child(false);

        ChatPage {
            root,
            side,
            filters,
            list,
            all_sessions,
            stack,
            toggle_list,
            header_badge,
            header_name,
            header_meta,
            header_summary,
            header_presence,
            open_session,
            thread,
            input,
            send,
            note,
            selected: RefCell::default(),
            drawn: RefCell::default(),
            sending: Cell::new(false),
        }
    }
}

impl App {
    /// Connects the page's own controls; built once with the shell.
    pub(super) fn connect_chat_page(&self, page: &ChatPage) {
        page.filters.connect_changed(self.on(App::render_chat));
        let send = self.on(App::send_chat_message);
        page.input.connect_activate(send);
        let send = self.on(App::send_chat_message);
        page.send.connect_clicked(move |_| send());
        let open = self.on(App::open_chat_session);
        page.open_session.connect_clicked(move |_| open());
        let table = self.on(|app| app.go(Page::Sessions));
        page.all_sessions.connect_clicked(move |_| table());
        let toggle = self.on(App::toggle_chat_list);
        page.toggle_list.connect_clicked(move |_| toggle());
        self.follow_chat_thread();
    }

    pub(super) fn with_chat<T>(&self, read: impl FnOnce(&ChatPage) -> T) -> Option<T> {
        self.shell().map(|shell| read(&shell.pages.chat))
    }

    /// Every agent on this computer, then your agents elsewhere, as the
    /// page lists them now.
    pub(super) fn chat_agents(&self) -> Vec<ChatAgent> {
        self.with_state(|state| {
            let rows = merge_local_sessions(&state.sessions, &state.local_sessions);
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
                chrono::Utc::now().timestamp_millis(),
            )
            .into_iter()
            .chain(chat_elsewhere_agents(&state.roster))
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
        if self.page() != Page::Chat {
            self.go(Page::Chat);
        }
        if let Some(key) = key {
            self.select_chat_agent(key);
        }
    }

    /// Draws the filters, the list and the header from what was last read.
    pub fn render_chat(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.chat;
        let agents = self.chat_agents();
        // The table loads its rows in pages; the link does not count them.
        page.all_sessions.set_label(&chat_all_sessions(0));
        let (harnesses, projects) = chat_filter_choices(&agents);
        page.filters.offer(&harnesses, &projects);
        let filters = page.filters.read();
        let shown: Vec<&ChatAgent> = agents
            .iter()
            .filter(|agent| filters.admits(agent))
            .collect();
        // Opening Chat never shows an empty screen while an agent runs:
        // the most recently active one is shown.
        if page.selected.borrow().is_none() {
            if let Some(first) = shown.first() {
                let key = first.key();
                glib::idle_add_local_once(self.on(move |app| app.select_chat_agent(&key)));
            }
        }
        let selected = page.selected.borrow().clone();
        let drawn: Vec<String> = shown
            .iter()
            .map(|agent| {
                format!(
                    "{}|{:?}|{}|{}|{}",
                    agent.key(),
                    agent.activity,
                    agent.name(),
                    agent.glance(),
                    selected.as_deref() == Some(agent.key().as_str())
                )
            })
            .collect();
        if *page.drawn.borrow() != drawn {
            clear(&page.list);
            if shown.is_empty() {
                let note = if agents.is_empty() {
                    CHAT_NO_AGENTS
                } else {
                    CHAT_NO_MATCH
                };
                let empty = text(note, &["caption"]).wrapped();
                widgets::pad(&empty, 8, None, None);
                page.list.append(&empty);
            }
            let mut elsewhere = false;
            for agent in &shown {
                if agent.local().is_none() && !elsewhere {
                    elsewhere = true;
                    let heading = widgets::section_title(CHAT_ELSEWHERE_SECTION);
                    heading.set_margin_top(super::zoom::px(12));
                    page.list.append(&heading);
                }
                let chosen = selected.as_deref() == Some(agent.key().as_str());
                page.list.append(&self.chat_row(agent, chosen));
            }
            *page.drawn.borrow_mut() = drawn;
        }
        for agent in shown.iter().take(SUMMARIES_AHEAD) {
            if let Some((row, _)) = agent.local() {
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

    fn chat_row(&self, agent: &ChatAgent, selected: bool) -> gtk::Button {
        let row = gtk::Button::new();
        widgets::add_classes(
            &row,
            &[
                "sidebar-row",
                "chat-row",
                if selected { "selected" } else { "" },
            ],
        );
        widgets::action_cursor(&row);
        let line = hbox(10, &[]);
        widgets::pad(&line, 8, Some(2), None);
        let badge = agent_badge(agent, 34);
        badge.set_valign(gtk::Align::Start);
        line.append(&badge);
        let words = vbox(2, &[]);
        words.set_hexpand(true);
        let top = hbox(6, &[]);
        let name = text(&agent.name(), &["row-title"]).ellipsized();
        name.set_hexpand(true);
        name.set_width_chars(1);
        top.append(&name);
        let status = text(
            agent.activity.label(),
            &["chat-status", agent.activity.class()],
        );
        top.append(&status);
        words.append(&top);
        let glance = agent.glance();
        if !glance.is_empty() {
            let summary = text(&glance, &["chat-glance"]).wrapped();
            summary.set_lines(2);
            summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
            summary.set_width_chars(1);
            words.append(&summary);
        }
        let meta = text(&agent.meta(), &["caption"]).ellipsized();
        meta.set_width_chars(1);
        words.append(&meta);
        line.append(&words);
        row.set_child(Some(&line));
        row.set_tooltip_text(Some(&agent.title()));
        widgets::name(&row, &agent.name());
        let key = agent.key();
        let open = self.on(move |app| app.select_chat_agent(&key));
        row.connect_clicked(move |_| open());
        row
    }

    fn render_chat_header(&self, page: &ChatPage, agent: &ChatAgent) {
        clear(&page.header_badge);
        page.header_badge.append(&agent_badge(agent, 32));
        page.open_session.set_visible(agent.local().is_some());
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
        page.input.set_placeholder_text(Some(&agent.placeholder()));
        widgets::name(&page.input.view, &agent.placeholder());
        page.send
            .set_sensitive(agent.can_send() && !page.sending.get());
        if !page.sending.get() && page.thread.status().is_none() {
            page.note.set_text(&agent.composer_note());
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
            page.input.set_text("");
            page.thread.set_status(None);
            if let Some(agent) = self.chat_agent(key) {
                page.note.set_text(&agent.composer_note());
            }
            self.load_chat_thread(key);
        }
        self.render_chat();
        page.input.view.grab_focus();
    }

    /// Hides the agent list so the conversation takes the window, or shows
    /// it again.
    fn toggle_chat_list(&self) {
        self.with_chat(|page| {
            let shown = !page.side.is_visible();
            page.side.set_visible(shown);
            let label = if shown {
                CHAT_HIDE_LIST
            } else {
                CHAT_SHOW_LIST
            };
            page.toggle_list.set_tooltip_text(Some(label));
            widgets::name(&page.toggle_list, label);
        });
    }

    /// Opens the agent shown in the Sessions reader.
    fn open_chat_session(&self) {
        let key = self
            .with_chat(|page| page.selected.borrow().clone())
            .flatten();
        let Some(agent) = key.and_then(|key| self.chat_agent(&key)) else {
            return;
        };
        let Some((row, _)) = agent.local() else {
            return;
        };
        let row = row.clone();
        self.go(Page::Sessions);
        self.open_local_session(row);
    }
}

/// The agent's mark: its session's avatar here, else its harness's.
fn agent_badge(agent: &ChatAgent, size: i32) -> gtk::Widget {
    match agent.local() {
        Some((row, _)) => super::session_avatar::row_badge(row, size),
        None => widgets::agent_badge(&agent.icon_source(), size),
    }
}
