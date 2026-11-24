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
    chat_attached_images, chat_pasted_images, chat_relay_incoming, chat_relay_marker,
    chat_relay_outgoing, chat_relay_sent_marker, chat_turn_open, chat_user_text,
    transcript_entry_visible, ChatAgent, ChatPlace, SpawnHandover, CHAT_DELIVERED, CHAT_EARLIER,
    CHAT_JUMP_LATEST, CHAT_LOADING, CHAT_NO_MESSAGES, CHAT_SENDING, CHAT_WORKING,
};
use agent_relay_node::relay::contract::transcript::{LocalTranscriptCursor, LocalTranscriptEntry};
use gtk::{glib, prelude::*};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

mod earlier;
mod jump;
mod reading;

/// How often the conversation shown is read for what was written since.
const FOLLOW_EVERY: Duration = Duration::from_secs(3);

/// How many conversations are remembered, and how many entries of each.
const REMEMBERED: usize = 24;
const KEPT_ENTRIES: usize = 3000;

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
    /// The conversation as placed on the page: its scroll, with the arrow
    /// back to the end over it (`jump.rs`).
    pub view: gtk::Overlay,
    /// Shows the entries before those drawn (`earlier.rs`).
    pub earlier: gtk::Button,
    /// The pages read so far when a conversation opens, drawn at the end.
    loading: RefCell<Vec<LocalTranscriptEntry>>,
    /// How many of the entries read are drawn, newest last.
    showing: Cell<usize>,
    pub scroll: gtk::ScrolledWindow,
    /// The conversation's turns, oldest first.
    pub messages: gtk::Box,
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
    /// The agent shown, by key, and every entry read for it so far.
    shown: RefCell<String>,
    kept: RefCell<Vec<LocalTranscriptEntry>>,
    /// Conversations read before, by agent key, drawn at once when one is
    /// opened again while it is read afresh.
    remembered: RefCell<HashMap<String, Vec<LocalTranscriptEntry>>>,
    /// Text messages drawn, to tell a copy from the message.
    drawn_texts: RefCell<crate::models::ChatCopies>,
    /// Follow ticks since the conversation opened.
    ticks: Cell<u32>,
    /// Whether the conversation is with an agent elsewhere.
    elsewhere: Cell<bool>,
}

impl Thread {
    pub fn build() -> Thread {
        let (column, holder) = widgets::column(760);
        super::super::zoom::spacing(&column, 20);
        let earlier = earlier::button();
        column.append(&earlier);
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
        let view = jump::overlay(&scroll);
        Thread {
            view,
            scroll,
            earlier,
            loading: RefCell::default(),
            showing: Cell::new(0),
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
            shown: RefCell::default(),
            kept: RefCell::default(),
            remembered: RefCell::default(),
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
        self.loading.borrow_mut().clear();
        self.showing.set(0);
        self.earlier.set_visible(false);
        self.remember(agent.key());
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
    /// Keeps what was read for the agent shown and starts keeping for the
    /// agent with `key`.
    fn remember(&self, key: String) {
        let previous = self.shown.replace(key);
        let kept = std::mem::take(&mut *self.kept.borrow_mut());
        if previous.is_empty() || kept.is_empty() {
            return;
        }
        let mut remembered = self.remembered.borrow_mut();
        if remembered.len() >= REMEMBERED && !remembered.contains_key(&previous) {
            remembered.clear();
        }
        remembered.insert(previous, kept);
    }

    /// What was read before for the agent with `key`, taken.
    fn recall(&self, key: &str) -> Option<Vec<LocalTranscriptEntry>> {
        self.remembered.borrow_mut().remove(key)
    }

    fn append(&self, entries: Vec<LocalTranscriptEntry>, source: &str, person: &str) -> bool {
        {
            let mut kept = self.kept.borrow_mut();
            kept.extend(entries.iter().cloned());
            let over = kept.len().saturating_sub(KEPT_ENTRIES);
            kept.drain(..over);
        }
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
        let (body, mut images) = chat_pasted_images(&body);
        let (body, attached) = chat_attached_images(&body, &attachments_dir());
        images.extend(attached);
        // A turn that is only an attached image is still a turn.
        if body.trim().is_empty() && images.is_empty() && entry.image_path.is_none() {
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
            let (body, images) = chat_attached_images(&incoming.body, &attachments_dir());
            (
                format!("in:{}", incoming.body.trim()),
                Turn {
                    images,
                    entry: message(entry, "user", body),
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
        // The core's own record has an id with a content hash.
        self.drawn_texts.borrow_mut().copied(
            &entry.role,
            &entry.text,
            entry.timestamp_ms,
            entry.id.contains(":sha256:"),
        )
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

/// Where the core saves the files messages come with: `attachments` in its
/// data directory, the same on every platform as the core's own default.
fn attachments_dir() -> std::path::PathBuf {
    let env = |key: &str| std::env::var(key).ok();
    let home = agent_relay_node::platform::home_dir().unwrap_or_default();
    agent_relay_node::relay::app_store::default_data_dir(&env, &home).join("attachments")
}
