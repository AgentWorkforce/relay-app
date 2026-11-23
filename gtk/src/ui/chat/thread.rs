//! The agent's conversation on the Chat page: its whole transcript, read
//! page by page, then followed every few seconds. Turns are drawn as in
//! the session reader (user bubbles, assistant Markdown, one Activity
//! disclosure per run of tool work); a relay message's framing is dropped
//! so a turn reads as it was typed, and every message that came through the
//! relay carries a marker naming who sent it. A headless agent's turns all
//! arrive over the relay's stdio input, so each is marked; an agent on
//! another computer has its direct messages as its conversation, all marked.
//! What was just sent waits at the end as a bubble marked Sending… until
//! the session's own turn for it appears.

use super::super::app::App;
use super::super::transcript::{entry_view, is_tool, tool_group_label};
use super::super::widgets::{self, clear, hbox, text, vbox, LabelStyle};
use super::sources::{entry_source, read_page, DIRECT};
use crate::models::{
    chat_pasted_images, chat_relay_incoming, chat_relay_marker, chat_relay_outgoing,
    chat_relay_sent_marker, chat_turn_open, chat_user_text, transcript_entry_visible, ChatAgent,
    ChatPlace, SpawnHandover, CHAT_DELIVERED, CHAT_LOADING, CHAT_NO_MESSAGES, CHAT_SENDING,
    CHAT_WORKING,
};
use agent_relay_node::relay::contract::transcript::{LocalTranscriptCursor, LocalTranscriptEntry};
use gtk::{glib, prelude::*};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// How often the conversation shown is read for what was written since.
const FOLLOW_EVERY: Duration = Duration::from_secs(3);

/// How far apart two records of the same turn can be.
const SAME_TURN_MS: i64 = 10_000;

/// An agent elsewhere is read every this many follow ticks: each read goes
/// to Cloud and to your messages over the network.
const ELSEWHERE_TICKS: u32 = 3;

/// How far apart the same relay text can be recorded and still be one
/// message: a tool call logged twice, or a direct message the transcript
/// shows when the agent read it, which can be minutes later.
const SAME_MESSAGE_MS: i64 = 600_000;
/// The most pages one opening reads before showing the latest.
const MAX_PAGES: usize = 200;

pub struct Thread {
    pub scroll: gtk::ScrolledWindow,
    messages: gtk::Box,
    pending: gtk::Box,
    placeholder: gtk::Label,
    /// Shown while the agent's last turn is not a reply yet.
    working: gtk::Box,
    ids: RefCell<HashSet<String>>,
    group: RefCell<Option<(gtk::Expander, gtk::Box)>>,
    /// Where following continues, once the whole read ended.
    cursor: RefCell<Option<Option<LocalTranscriptCursor>>>,
    /// Bumped by every new read: answers for an older one are dropped.
    generation: Cell<u64>,
    following: Cell<bool>,
    table_cells: Cell<usize>,
    status: RefCell<Option<String>>,
    /// Every turn the person sends reaches this agent through the relay.
    relay_only: Cell<bool>,
    /// The conversation is direct messages: replies came through it too.
    direct: Cell<bool>,
    /// Relay messages already drawn from tool work, by direction and text,
    /// with when each was last seen.
    relay_seen: RefCell<HashMap<String, i64>>,
    /// The harness an agent elsewhere's replies are drawn with (its logo),
    /// when its computer reports one; its name marks them.
    elsewhere_icon: RefCell<Option<String>>,
    /// Text messages drawn, by role and text, with when each was written.
    drawn_texts: RefCell<HashMap<String, i64>>,
    /// Follow ticks since the conversation opened.
    ticks: Cell<u32>,
    /// Whether the conversation is with an agent elsewhere.
    elsewhere: Cell<bool>,
}

impl Thread {
    pub fn build() -> Thread {
        let (column, holder) = widgets::column(760);
        super::super::zoom::spacing(&column, 20);
        let messages = vbox(20, &[]);
        column.append(&messages);
        let pending = vbox(12, &[]);
        column.append(&pending);
        let working = hbox(8, &["chat-working"]);
        let spinner = gtk::Spinner::new();
        spinner.start();
        working.append(&spinner);
        working.append(&text(CHAT_WORKING, &["secondary"]));
        working.set_visible(false);
        column.append(&working);
        let placeholder = text(CHAT_LOADING, &["secondary"]).centered();
        placeholder.set_halign(gtk::Align::Center);
        column.append(&placeholder);
        // Centered in the space, as Claude's and Pear's conversations are.
        holder.set_halign(gtk::Align::Center);
        let scroll = widgets::scrolled(&holder);
        scroll.set_vexpand(true);
        scroll.set_hexpand(true);
        Thread {
            scroll,
            messages,
            pending,
            placeholder,
            working,
            ids: RefCell::default(),
            group: RefCell::default(),
            cursor: RefCell::default(),
            generation: Cell::new(0),
            following: Cell::new(false),
            table_cells: Cell::new(4000),
            status: RefCell::default(),
            relay_only: Cell::new(false),
            direct: Cell::new(false),
            relay_seen: RefCell::default(),
            elsewhere_icon: RefCell::default(),
            drawn_texts: RefCell::default(),
            ticks: Cell::new(0),
            elsewhere: Cell::new(false),
        }
    }

    fn reset(&self, agent: &ChatAgent) -> u64 {
        self.relay_only.set(agent.relay_only());
        // Direct messages are the conversation only without a Cloud copy.
        self.direct.set(match &agent.place {
            ChatPlace::Elsewhere(remote) => remote.session_id.is_none(),
            _ => false,
        });
        clear(&self.messages);
        clear(&self.pending);
        self.ids.borrow_mut().clear();
        self.relay_seen.borrow_mut().clear();
        self.drawn_texts.borrow_mut().clear();
        self.elsewhere
            .set(matches!(agent.place, ChatPlace::Elsewhere(_)));
        *self.elsewhere_icon.borrow_mut() = match &agent.place {
            ChatPlace::Elsewhere(_) => Some(agent.icon_source()).filter(|icon| !icon.is_empty()),
            _ => None,
        };
        self.group.borrow_mut().take();
        self.cursor.borrow_mut().take();
        self.following.set(false);
        self.table_cells.set(4000);
        self.placeholder.set_text(CHAT_LOADING);
        self.placeholder.set_visible(true);
        self.working.set_visible(false);
        self.generation.set(self.generation.get() + 1);
        self.generation.get()
    }

    /// What the last send met, shown under the composer until the next.
    pub fn status(&self) -> Option<String> {
        self.status.borrow().clone()
    }

    pub fn set_status(&self, status: Option<String>) {
        *self.status.borrow_mut() = status;
    }

    fn at_latest(&self) -> bool {
        let adjustment = self.scroll.vadjustment();
        adjustment.upper() - adjustment.page_size() - adjustment.value() < 120.0
    }

    pub fn scroll_to_latest(&self) {
        let adjustment = self.scroll.vadjustment();
        glib::idle_add_local_once(move || adjustment.set_value(adjustment.upper()));
    }

    /// Shows `body` at the end as on its way.
    pub fn add_pending(&self, body: &str) {
        self.working.set_visible(true);
        let bubble = vbox(4, &["chat-pending"]);
        bubble.set_halign(gtk::Align::End);
        let words = text(body, &["transcript-text"]).wrapped().copyable();
        words.add_css_class("transcript-user");
        bubble.append(&words);
        let note = text(CHAT_SENDING, &["caption"]);
        note.set_xalign(1.0);
        bubble.append(&note);
        self.pending.append(&bubble);
        self.placeholder.set_visible(false);
        self.scroll_to_latest();
    }

    pub fn clear_pending(&self) {
        clear(&self.pending);
    }

    /// A failed send leaves nothing for the agent to work on.
    pub fn stop_working(&self) {
        self.working.set_visible(false);
    }

    /// Draws `entries`; returns whether a message waiting at the end has
    /// arrived in the session as its own turn.
    fn append(&self, entries: Vec<LocalTranscriptEntry>, source: &str, person: &str) -> bool {
        let latest = self.at_latest();
        let waiting = self.pending.first_child().is_some();
        let mut user_turn = false;
        let mut last: Option<bool> = None;
        for entry in entries {
            let Some(turn) = self.turn(entry, source, person) else {
                continue;
            };
            user_turn |= turn.from_person;
            last = Some(chat_turn_open(&turn.entry));
            self.draw(&turn, source);
        }
        if user_turn {
            // The session's own turn for what was sent has arrived.
            self.clear_pending();
        }
        if let Some(open) = last {
            self.working.set_visible(open);
        }
        let empty = self.ids.borrow().is_empty() && self.pending.first_child().is_none();
        self.placeholder.set_text(CHAT_NO_MESSAGES);
        self.placeholder.set_visible(empty);
        if latest {
            self.scroll_to_latest();
        }
        waiting && user_turn
    }

    /// `entry` as it is drawn, or `None` when it is hidden or already shown.
    fn turn(&self, mut entry: LocalTranscriptEntry, source: &str, person: &str) -> Option<Turn> {
        if !transcript_entry_visible(&entry) || !self.ids.borrow_mut().insert(entry.id.clone()) {
            return None;
        }
        if entry.kind == "text" && self.copied(&entry) {
            return None;
        }
        let from_person = entry.role == "user" && entry.kind == "text";
        if entry.id.starts_with(DIRECT) {
            return self.direct_turn(entry, source, person);
        }
        if !from_person {
            if let Some(turn) = self.relay_line(&entry, person) {
                return turn;
            }
        }
        if !from_person {
            let relay = (self.direct.get() && entry.kind == "text")
                .then(|| chat_relay_marker(Some(source)));
            return Some(Turn {
                images: Vec::new(),
                entry,
                author: person.to_string(),
                relay,
                from_person,
            });
        }
        let (sender, body) = chat_user_text(&entry.text);
        let (body, images) = chat_pasted_images(&body);
        if body.trim().is_empty() && images.is_empty() {
            return None;
        }
        let relay = (sender.is_some() || self.relay_only.get())
            .then(|| chat_relay_marker(sender.as_deref()));
        let author = sender.map_or_else(|| person.to_string(), |sender| format!("@{sender}"));
        entry.text = body;
        Some(Turn {
            images,
            entry,
            author,
            relay,
            from_person,
        })
    }

    /// A direct message with an agent elsewhere: yours read by it, or its
    /// to you. Drawn once even when its transcript also shows it.
    fn direct_turn(&self, entry: LocalTranscriptEntry, source: &str, person: &str) -> Option<Turn> {
        let mine = entry.role == "user";
        let key = format!("{}{}", if mine { "in:" } else { "out:" }, entry.text.trim());
        if !self.first_sighting(key, entry.timestamp_ms) {
            return None;
        }
        Some(Turn {
            images: Vec::new(),
            relay: Some(chat_relay_marker(Some(if mine { person } else { source }))),
            author: if mine {
                person.to_string()
            } else {
                format!("@{source}")
            },
            from_person: mine,
            entry,
        })
    }

    /// A relay message inside tool work, drawn as a message: one the agent
    /// read from its inbox, or one it sent with a command or tool. `None`
    /// when `entry` carries none; `Some(None)` for one already drawn (a
    /// tool call is often recorded twice).
    fn relay_line(&self, entry: &LocalTranscriptEntry, person: &str) -> Option<Option<Turn>> {
        // Only a tool's output can be a message the agent received; its own
        // text quoting a frame is just text.
        let incoming = (entry.kind == "tool_result")
            .then(|| chat_relay_incoming(&entry.text))
            .flatten();
        let (seen, turn) = if let Some(incoming) = incoming {
            let author = incoming
                .sender
                .as_ref()
                .map_or_else(|| person.to_string(), |sender| format!("@{sender}"));
            (
                format!("in:{}", incoming.body.trim()),
                Turn {
                    images: Vec::new(),
                    entry: message(entry, "user", incoming.body),
                    author,
                    relay: Some(chat_relay_marker(incoming.sender.as_deref())),
                    from_person: false,
                },
            )
        } else if let Some(outgoing) = (entry.kind != "tool_result")
            .then(|| chat_relay_outgoing(&entry.text))
            .flatten()
        {
            (
                format!("out:{}", outgoing.body.trim()),
                Turn {
                    images: Vec::new(),
                    entry: message(entry, "assistant", outgoing.body),
                    author: person.to_string(),
                    relay: Some(chat_relay_sent_marker(outgoing.recipient.as_deref())),
                    from_person: false,
                },
            )
        } else {
            return None;
        };
        Some(
            self.first_sighting(seen, entry.timestamp_ms)
                .then_some(turn),
        )
    }

    /// Whether `entry` is a copy of a message already drawn: the session's
    /// transcript and the core's own record of the same turn arrive under
    /// different ids, seconds apart.
    fn copied(&self, entry: &LocalTranscriptEntry) -> bool {
        let key = format!("{}|{}", entry.role, entry.text.trim());
        let mut drawn = self.drawn_texts.borrow_mut();
        let copy = drawn
            .get(&key)
            .is_some_and(|at| (entry.timestamp_ms - at).abs() <= SAME_TURN_MS);
        drawn.insert(key, entry.timestamp_ms);
        copy
    }

    /// Whether the relay message `key` (direction and text) at `at` is new:
    /// the same text again within [`SAME_MESSAGE_MS`] is the same message
    /// recorded twice; later, it is another message.
    fn first_sighting(&self, key: String, at: i64) -> bool {
        let mut seen = self.relay_seen.borrow_mut();
        let repeat = seen
            .get(&key)
            .is_some_and(|before| (at - before).abs() <= SAME_MESSAGE_MS);
        seen.insert(key, at);
        !repeat
    }

    /// Draws `turn`: a message on its own, tool work in the run's Activity.
    fn draw(&self, turn: &Turn, source: &str) {
        let mut remaining = self.table_cells.get();
        let icon = self.elsewhere_icon.borrow().clone();
        let source = icon.as_deref().unwrap_or(source);
        let widget = entry_view(&turn.entry, source, &turn.author, None, &mut remaining);
        self.table_cells.set(remaining);
        super::images::expand_attachments(&widget);
        if !is_tool(&turn.entry) {
            self.group.borrow_mut().take();
            match &turn.relay {
                Some(marker) => self.messages.append(&relay_turn(&widget, marker)),
                None => self.messages.append(&widget),
            }
            // A pasted screenshot shows under the message it was sent in.
            for path in &turn.images {
                if std::path::Path::new(path).is_file() {
                    let image = super::images::inline_image(path);
                    image.set_halign(gtk::Align::End);
                    self.messages.append(&image);
                }
            }
            return;
        }
        let mut group = self.group.borrow_mut();
        let (_, children) = group.get_or_insert_with(|| self.activity_group());
        children.append(&widget);
        // A tool's screenshot shows without opening the activity.
        if let Some(path) = &turn.entry.image_path {
            self.messages.append(&super::images::inline_image(path));
        }
    }

    /// A new Activity disclosure at the end, for a run of tool work.
    fn activity_group(&self) -> (gtk::Expander, gtk::Box) {
        let disclosure = gtk::Expander::new(Some(&tool_group_label(0)));
        disclosure.add_css_class("transcript-tool-group");
        widgets::action_cursor(&disclosure);
        let children = vbox(8, &[]);
        disclosure.set_child(Some(&children));
        self.messages.append(&disclosure);
        (disclosure, children)
    }
}

/// `entry` as a plain message from `role` reading `text`.
fn message(entry: &LocalTranscriptEntry, role: &str, text: String) -> LocalTranscriptEntry {
    LocalTranscriptEntry {
        role: role.into(),
        kind: "text".into(),
        text,
        image_path: None,
        ..entry.clone()
    }
}

/// One entry as it is drawn: who wrote it, and the relay marker over it
/// when it came through the relay.
struct Turn {
    entry: LocalTranscriptEntry,
    /// Images pasted into the person's message, drawn under it.
    images: Vec<String>,
    author: String,
    relay: Option<String>,
    /// The person's own turn (theirs or one sent to the agent).
    from_person: bool,
}

/// A message that came through the relay: its marker over it, and an
/// accent along its side.
fn relay_turn(widget: &gtk::Widget, marker: &str) -> gtk::Box {
    let turn = vbox(4, &["relay-turn"]);
    let mark = text(marker, &["relay-marker"]).ellipsized();
    mark.set_halign(gtk::Align::Start);
    turn.append(&mark);
    turn.append(widget);
    turn
}

impl App {
    fn person_name(&self) -> String {
        self.with_state(|state| state.account().person.and_then(|person| person.name))
            .unwrap_or_else(|| "You".into())
    }

    /// Reads the conversation of the agent with `key` from the start.
    pub(in crate::ui) fn load_chat_thread(&self, key: &str) {
        let Some(agent) = self.chat_agent(key) else {
            return;
        };
        let Some(generation) = self.with_chat(|page| page.thread.reset(&agent)) else {
            return;
        };
        self.read_chat_pages(agent, None, generation, 0);
    }

    fn read_chat_pages(
        &self,
        agent: ChatAgent,
        cursor: Option<LocalTranscriptCursor>,
        generation: u64,
        pages: usize,
    ) {
        let requested = cursor.clone();
        read_page(self, &agent.clone(), cursor, false, move |app, result| {
            let current = app.with_chat(|page| page.thread.generation.get() == generation);
            if current != Some(true) {
                return;
            }
            let page = match result {
                Ok(page) => page,
                Err(error) => {
                    app.with_chat(|chat| {
                        chat.thread.placeholder.set_text(&error.message);
                        chat.thread.placeholder.set_visible(true);
                        *chat.thread.cursor.borrow_mut() = Some(requested.clone());
                    });
                    return;
                }
            };
            let person = app.person_name();
            let next = page
                .next_cursor
                .clone()
                .filter(|next| Some(next) != requested.as_ref());
            let follow = SpawnHandover::follow_cursor(requested.as_ref(), &page);
            let source = entry_source(&agent);
            app.with_chat(|chat| chat.thread.append(page.entries, &source, &person));
            match next {
                Some(next) if pages + 1 < MAX_PAGES => {
                    app.read_chat_pages(agent, Some(next), generation, pages + 1)
                }
                next => {
                    app.with_chat(|chat| {
                        *chat.thread.cursor.borrow_mut() = Some(next.or(follow));
                        chat.thread.scroll_to_latest();
                    });
                }
            }
        });
    }

    /// Reads what the agent shown wrote since, every few seconds while the
    /// page is shown.
    pub(super) fn follow_chat_thread(&self) {
        let weak = self.downgrade();
        glib::timeout_add_local(FOLLOW_EVERY, move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            app.follow_chat_once();
            glib::ControlFlow::Continue
        });
    }

    fn follow_chat_once(&self) {
        if self.page() != super::super::state::Page::Chat || !self.window().is_visible() {
            return;
        }
        let Some((key, cursor, generation)) = self
            .with_chat(|page| {
                let key = page.selected.borrow().clone()?;
                let cursor = page.thread.cursor.borrow().clone()?;
                let tick = page.thread.ticks.get().wrapping_add(1);
                page.thread.ticks.set(tick);
                if page.thread.elsewhere.get() && tick % ELSEWHERE_TICKS != 0 {
                    return None;
                }
                if page.thread.following.replace(true) {
                    return None;
                }
                Some((key, cursor, page.thread.generation.get()))
            })
            .flatten()
        else {
            return;
        };
        let Some(agent) = self.chat_agent(&key) else {
            self.with_chat(|page| page.thread.following.set(false));
            return;
        };
        let requested = cursor.clone();
        let source = entry_source(&agent);
        read_page(self, &agent, cursor, true, move |app, result| {
            let person = app.person_name();
            app.with_chat(|chat| {
                let thread = &chat.thread;
                if thread.generation.get() != generation {
                    return;
                }
                thread.following.set(false);
                let Ok(page) = result else {
                    return;
                };
                let next = page
                    .next_cursor
                    .clone()
                    .filter(|next| Some(next) != requested.as_ref())
                    .or_else(|| SpawnHandover::follow_cursor(requested.as_ref(), &page));
                *thread.cursor.borrow_mut() = Some(next);
                if thread.append(page.entries, &source, &person) {
                    thread.set_status(Some(CHAT_DELIVERED.to_string()));
                    chat.composer.note.set_text(CHAT_DELIVERED);
                }
            });
        });
    }
}
