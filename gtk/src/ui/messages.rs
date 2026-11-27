//! Messages: open conversations on the left, the one shown on the right,
//! opened by any name or exact `agent@machine` address; and Recent
//! Messages, what was delivered into sessions on the relay.
//!
//! Each open conversation follows its recipient on the roster
//! (`roster.get`): before a send it warns when the agent is not running,
//! after one it shows what Relaycast holds for the message
//! (`docs/agents-people.md`, "Recipient liveness and message state"), and
//! whether a reply is expected. The recipient is read when the
//! conversation is shown and two seconds after a send. On `sessions`,
//! `roster` and `messages` invalidations and every 10 s it is read
//! for the conversation shown and for any whose last message has no final
//! state yet; one read per conversation runs at a time, and a closed
//! conversation is no longer read.

use super::app::App;
use super::state::Page;
use super::widgets::{self, button, clear, hbox, text, vbox, LabelStyle};
use crate::models::{
    delivery_note, endpoint_label, queues, recipient_params, recipient_warning, reply_note,
    DeliveryTone, ReadBackoff,
};
use agent_relay_node::relay::contract::directory::{
    History, HistoryParams, MarkReadParams, Recipient, SendParams,
};
use agent_relay_node::relay::contract::method;
use agent_relay_node::relay::messaging::DirectMessage;
use agent_relay_node::relay::people::MessageState;
use agent_relay_node::relay::protocol::ErrorKind;
use gtk::{gdk, glib, prelude::*};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

mod backoff;
mod recipient;
mod send;

use backoff::Read;

/// How often an open conversation is read without being asked.
const FOLLOW_EVERY: Duration = Duration::from_secs(10);
/// How long after a send its state is read again.
const RECHECK_AFTER: Duration = Duration::from_secs(2);

/// Who you message as, until a history read names you.
const MESSAGE_AS: &str =
    "You message as your Agent Relay identity, the same one the dashboard uses.";

/// The stack's child while no conversation is open.
const EMPTY: &str = "empty";

/// A message sent, and the last state Relaycast reported for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastSend {
    pub message_id: Option<String>,
    pub state: Option<MessageState>,
}

/// What an open conversation knows.
#[derive(Default)]
pub struct ChatState {
    /// The ids of the messages shown, in order.
    pub shown: Vec<String>,
    loading: bool,
    pending: bool,
    pub sending: bool,
    pub unread: bool,
    /// The roster's latest record of the recipient.
    pub recipient: Option<Recipient>,
    recipient_reads: u64,
    recipient_loading: bool,
    recipient_pending: bool,
    pub last_send: Option<LastSend>,
    /// Why the last send failed.
    pub error: Option<String>,
    /// The message just sent, shown under "You" as on its way until the
    /// history lists it; `echo_sent` once the core took it.
    pub echoes: Vec<send::Echo>,
    echo_token: u64,
    /// Who the last history read said you message as.
    identity: String,
    /// The newest message from the other side this window marked read.
    marked_read: Option<String>,
    /// Failed reads in a row; while `backoff_until` is ahead, only the
    /// user's own asking reads the conversation. `backoff_by` is the read
    /// whose failure is still unanswered.
    backoff: ReadBackoff,
    backoff_until: Option<Instant>,
    backoff_by: Option<Read>,
    /// The wait after the first failure, when not `ReadBackoff::CADENCE`;
    /// every wait scales with it. Only tests shorten it.
    pub backoff_unit: Option<Duration>,
}

/// One open conversation: its page in the stack and its row in the list.
pub struct Chat {
    /// The name or address it was opened with.
    pub key: String,
    pub label: String,
    page: gtk::Box,
    row: gtk::ListBoxRow,
    name: gtk::Label,
    pub messages: gtk::Box,
    scroll: gtk::ScrolledWindow,
    /// Who you message as, or why the history could not be read.
    pub status: gtk::Label,
    /// Whether the recipient runs, or what the last send met.
    pub delivery: gtk::Label,
    /// Whether a reply is expected.
    pub reply: gtk::Label,
    pub text: gtk::TextView,
    pub send: gtk::Button,
    pub state: RefCell<ChatState>,
    timer: RefCell<Option<glib::SourceId>>,
    /// The read that ends a failed read's wait.
    retry: RefCell<Option<glib::SourceId>>,
    /// The echoes' rows while shown.
    echo_rows: RefCell<Vec<gtk::Box>>,
}

impl Chat {
    fn body(&self) -> String {
        let buffer = self.text.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .trim()
            .to_string()
    }
}

/// The page: the conversation list and the conversation shown.
pub struct MessagesPage {
    pub root: gtk::Box,
    pub recipient: gtk::Entry,
    pub recipient_error: gtk::Label,
    list: gtk::ListBox,
    stack: gtk::Stack,
    chats: RefCell<Vec<Rc<Chat>>>,
    shown: RefCell<Option<String>>,
    /// Most recently viewed last, so closing a conversation returns to the
    /// one read before it.
    recent: RefCell<Vec<String>>,
    /// Set while the list's selection follows the conversation shown.
    selecting: Cell<bool>,
}

impl MessagesPage {
    pub fn build() -> MessagesPage {
        let root = hbox(0, &[]);
        let side = vbox(8, &["sidebar"]);
        widgets::pad(&side, 14, None, None);
        super::zoom::size(&side, 230, -1);
        side.append(&text("Conversations", &["settings-row-title"]));
        let recipient = gtk::Entry::new();
        recipient.set_placeholder_text(Some("agent@machine or @name"));
        recipient.set_tooltip_text(Some("Open one exact endpoint, or a name"));
        widgets::name(&recipient, "Open a conversation");
        side.append(&recipient);
        let recipient_error = text("", &["error", "caption"]).wrapped();
        recipient_error.set_visible(false);
        side.append(&recipient_error);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("conversations");
        side.append(&widgets::scrolled(&list));
        root.append(&side);
        root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::None);
        stack.set_hexpand(true);
        let empty = vbox(8, &[]);
        empty.set_valign(gtk::Align::Center);
        empty.set_halign(gtk::Align::Center);
        empty.append(&text(Page::Messages.title(), &["placeholder-title"]).centered());
        empty.append(
            &text(Page::Messages.subtitle(), &["secondary"])
                .wrapped()
                .centered(),
        );
        empty.append(
            &text(
                "Type a name or an agent@machine address to open a conversation.",
                &["caption"],
            )
            .wrapped()
            .centered(),
        );
        stack.add_named(&empty, Some(EMPTY));
        root.append(&stack);
        MessagesPage {
            root,
            recipient,
            recipient_error,
            list,
            stack,
            chats: RefCell::default(),
            shown: RefCell::new(None),
            recent: RefCell::default(),
            selecting: Cell::new(false),
        }
    }

    fn chat(&self, key: &str) -> Option<Rc<Chat>> {
        self.chats
            .borrow()
            .iter()
            .find(|chat| chat.key == key)
            .cloned()
    }

    /// Stops following every conversation.
    pub fn close_all(&self) {
        let chats: Vec<Rc<Chat>> = self.chats.borrow_mut().drain(..).collect();
        for chat in chats {
            chat.stop_following();
        }
    }
}

/// Who you message as: the identity a history read named, if it did.
fn message_as(identity: &str) -> String {
    if identity.is_empty() {
        return MESSAGE_AS.to_string();
    }
    format!("You message as @{identity}, the same you as on the dashboard.")
}

/// `Oct 3, 18:45` in local time, from an ISO-8601 instant.
fn message_time(value: Option<&str>) -> String {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%b %-d, %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

impl App {
    /// Connects the page's own controls; built once with the shell.
    pub(super) fn connect_messages_page(&self, page: &MessagesPage) {
        let open = self.on(App::open_typed_chat);
        page.recipient.connect_activate(move |_| open());
        let weak = self.downgrade();
        page.list.connect_row_selected(move |_, row| {
            let (Some(app), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            let key = app.with_messages(|page| {
                if page.selecting.get() {
                    return None;
                }
                page.chats
                    .borrow()
                    .iter()
                    .find(|chat| &chat.row == row)
                    .map(|chat| chat.key.clone())
            });
            if let Some(key) = key.flatten() {
                app.select_chat(&key);
            }
        });
        // Ctrl+W closes the conversation shown.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                && matches!(key, gdk::Key::w | gdk::Key::W)
            {
                if let Some(app) = weak.upgrade() {
                    if let Some(shown) = app.shown_chat() {
                        app.close_chat(&shown);
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        });
        page.root.add_controller(keys);
    }

    fn with_messages<T>(&self, read: impl FnOnce(&MessagesPage) -> T) -> Option<T> {
        self.shell().map(|shell| read(&shell.pages.messages))
    }

    /// The open conversation with `key`.
    pub fn chat(&self, key: &str) -> Option<Rc<Chat>> {
        self.with_messages(|page| page.chat(key)).flatten()
    }

    /// The conversation shown.
    pub fn shown_chat(&self) -> Option<String> {
        self.with_messages(|page| page.shown.borrow().clone())
            .flatten()
    }

    /// The open conversations, in the list's order.
    pub fn open_chats(&self) -> Vec<String> {
        self.with_messages(|page| {
            page.chats
                .borrow()
                .iter()
                .map(|chat| chat.key.clone())
                .collect()
        })
        .unwrap_or_default()
    }

    /// Opens Messages to `endpoint`; empty, to the field that opens one.
    pub fn show_message_composer(&self, endpoint: &str) {
        self.open_message_composer();
        if !endpoint.trim().is_empty() {
            self.open_chat(endpoint);
        } else if let Some(shell) = self.shell() {
            shell.pages.messages.recipient.grab_focus();
        }
    }

    pub fn render_messages(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.messages;
        if page.chats.borrow().is_empty() {
            page.stack.set_visible_child_name(EMPTY);
        }
    }

    /// Opens the conversation typed in the field.
    pub fn open_typed_chat(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.messages;
        let endpoint = page.recipient.text().trim().to_string();
        let problem = if endpoint.is_empty() {
            Some("Enter an agent name or an agent@machine address.")
        } else if endpoint.chars().any(char::is_whitespace) {
            Some("Names and addresses cannot contain whitespace.")
        } else {
            None
        };
        page.recipient_error.set_text(problem.unwrap_or_default());
        page.recipient_error.set_visible(problem.is_some());
        if problem.is_none() {
            page.recipient.set_text("");
            self.open_chat(&endpoint);
        }
    }

    /// Opens one name or exact `agent@machine` address. The core
    /// validates it on the first read. A conversation is keyed by its
    /// label, so `alice` and `@alice` open the same one.
    pub fn open_chat(&self, endpoint: &str) {
        let Some(shell) = self.shell().filter(|_| !endpoint.trim().is_empty()) else {
            return;
        };
        let key = endpoint_label(endpoint);
        let label = key.clone();
        if self.page() != Page::Messages {
            self.go(Page::Messages);
        }
        let page = &shell.pages.messages;
        if page.chat(&key).is_some() {
            self.select_chat(&key);
            return;
        }
        let view = vbox(10, &[]);
        widgets::pad(&view, 18, None, None);
        let top = hbox(12, &[]);
        let heading = text(&label, &["headline"]).ellipsized();
        heading.set_hexpand(true);
        top.append(&heading);
        let close = button(
            "Close conversation",
            &[],
            self.on({
                let key = key.clone();
                move |app| app.close_chat(&key)
            }),
        );
        close.set_tooltip_text(Some("Ctrl+W"));
        close.set_valign(gtk::Align::Start);
        top.append(&close);
        view.append(&top);
        let messages = vbox(12, &[]);
        widgets::pad(&messages, 12, None, None);
        let scroll = widgets::scrolled(&messages);
        scroll.set_min_content_height(320);
        view.append(&scroll);
        let status = text(MESSAGE_AS, &["caption"]).wrapped();
        view.append(&status);
        let delivery = text("", &["caption"]).wrapped();
        view.append(&delivery);
        let compose = hbox(10, &[]);
        let typed = gtk::TextView::new();
        typed.set_wrap_mode(gtk::WrapMode::WordChar);
        super::zoom::size(&typed, -1, 64);
        typed.set_hexpand(true);
        typed.add_css_class("compose");
        widgets::name(&typed, "Message");
        compose.append(&typed);
        let send = gtk::Button::with_label("Send");
        crate::ui::widgets::action_cursor(&send);
        send.add_css_class("primary");
        send.set_valign(gtk::Align::End);
        compose.append(&send);
        view.append(&compose);
        // Whether a reply is expected: shown once the roster has answered.
        let reply = text("", &["caption"]).wrapped();
        reply.set_visible(false);
        view.append(&reply);
        page.stack.add_named(&view, Some(&key));

        let row = gtk::ListBoxRow::new();
        crate::ui::widgets::action_cursor(&row);
        let row_box = hbox(6, &[]);
        widgets::pad(&row_box, 6, None, None);
        let name = text(&label, &[]).ellipsized();
        name.set_hexpand(true);
        row_box.append(&name);
        let dismiss = widgets::icon_button(
            "window-close-symbolic",
            "Close conversation",
            &["icon"],
            self.on({
                let key = key.clone();
                move |app| app.close_chat(&key)
            }),
        );
        row_box.append(&dismiss);
        row.set_child(Some(&row_box));
        page.list.append(&row);

        let chat = Rc::new(Chat {
            key: key.clone(),
            label,
            page: view,
            row,
            name,
            messages,
            scroll,
            status,
            delivery,
            reply,
            text: typed,
            send,
            state: RefCell::default(),
            timer: RefCell::new(None),
            retry: RefCell::new(None),
            echo_rows: RefCell::new(Vec::new()),
        });
        page.chats.borrow_mut().push(chat.clone());

        // A new message gives the last send's note way to the recipient's
        // state, so a warning about it is never hidden.
        let weak = self.downgrade();
        let edited = key.clone();
        chat.text.buffer().connect_changed(move |buffer| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let Some(chat) = app.chat(&edited) else {
                return;
            };
            let cleared = {
                let mut state = chat.state.borrow_mut();
                let clear =
                    buffer.char_count() > 0 && (state.last_send.is_some() || state.error.is_some());
                if clear {
                    state.last_send = None;
                    state.error = None;
                }
                clear
            };
            if cleared {
                app.render_delivery(&edited);
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        let sent = key.clone();
        keys.connect_key_pressed(move |_, pressed, _, modifiers| {
            if matches!(pressed, gdk::Key::Return | gdk::Key::KP_Enter)
                && !modifiers.contains(gdk::ModifierType::SHIFT_MASK)
            {
                if let Some(app) = weak.upgrade() {
                    app.send_chat(&sent);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        chat.text.add_controller(keys);
        let send_key = key.clone();
        let send = self.on(move |app| app.send_chat(&send_key));
        chat.send.connect_clicked(move |_| send());
        // The core's invalidations wake a conversation on a reply or a
        // change in its recipient; this catches what the core does not
        // announce, such as a message handed over or a remote agent going
        // offline.
        let weak = self.downgrade();
        let followed = Rc::downgrade(&chat);
        let timer = glib::timeout_add_local(FOLLOW_EVERY, move || {
            match (weak.upgrade(), followed.upgrade()) {
                (Some(app), Some(chat)) => {
                    app.follow_chat(&chat.key);
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            }
        });
        *chat.timer.borrow_mut() = Some(timer);
        self.render_delivery(&key);
        // Shown, it looks its recipient up.
        self.select_chat(&key);
        self.refresh_chat(&key);
    }

    /// Reads the conversation and, when it is followed, its recipient.
    fn follow_chat(&self, key: &str) {
        self.follow_recipient(key);
        self.refresh_chat(key);
    }

    /// The open conversations are read when the window comes to the front:
    /// behind, they are not read (`read_chat`).
    pub(super) fn read_chats_when_active(&self) {
        let refresh = self.on(App::refresh_chats);
        self.window().connect_is_active_notify(move |window| {
            if window.is_active() {
                refresh();
            }
        });
    }

    /// The core said conversations changed: each open one is read.
    pub(super) fn refresh_chats(&self) {
        for key in self.open_chats() {
            self.refresh_chat(&key);
        }
    }

    /// Shows the conversation with `key`; newly shown, its recipient is
    /// read.
    pub fn select_chat(&self, key: &str) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.messages;
        let Some(chat) = page.chat(key) else {
            return;
        };
        page.stack.set_visible_child(&chat.page);
        let newly = page.shown.replace(Some(key.to_string())).as_deref() != Some(key);
        {
            let mut recent = page.recent.borrow_mut();
            recent.retain(|shown| shown != key);
            recent.push(key.to_string());
        }
        if page.list.selected_row().as_ref() != Some(&chat.row) {
            page.selecting.set(true);
            page.list.select_row(Some(&chat.row));
            page.selecting.set(false);
        }
        self.mark_chat(&chat, false);
        chat.text.grab_focus();
        if newly {
            // Turning to a conversation is asking for it: one waiting
            // after a failed read is read at once, from a fresh start.
            let waiting = chat.drop_backoff();
            self.look_up_recipient(key);
            if waiting {
                self.refresh_chat(key);
            }
        }
    }

    fn mark_chat(&self, chat: &Chat, unread: bool) {
        if std::mem::replace(&mut chat.state.borrow_mut().unread, unread) == unread {
            return;
        }
        if unread {
            chat.name.set_text(&format!("● {}", chat.label));
            chat.name.add_css_class("settings-row-title");
        } else {
            chat.name.set_text(&chat.label);
            chat.name.remove_css_class("settings-row-title");
        }
    }

    /// Closes the conversation with `key`; it is no longer read.
    pub fn close_chat(&self, key: &str) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.messages;
        let Some(chat) = page.chat(key) else {
            return;
        };
        page.chats
            .borrow_mut()
            .retain(|open| !Rc::ptr_eq(open, &chat));
        chat.stop_following();
        page.selecting.set(true);
        page.list.remove(&chat.row);
        page.selecting.set(false);
        page.stack.remove(&chat.page);
        let next = {
            let mut recent = page.recent.borrow_mut();
            let open: Vec<String> = page
                .chats
                .borrow()
                .iter()
                .map(|chat| chat.key.clone())
                .collect();
            recent.retain(|shown| open.contains(shown));
            recent.last().cloned().or_else(|| open.first().cloned())
        };
        *page.shown.borrow_mut() = None;
        match next {
            Some(next) => self.select_chat(&next),
            None => page.stack.set_visible_child_name(EMPTY),
        }
    }

    /// Reads the conversation, unless a failed read's wait is pending: the
    /// read that ends the wait is already set.
    pub(super) fn refresh_chat(&self, key: &str) {
        self.read_chat(key, false);
    }

    /// Reads the conversation because the user asked, wait or not.
    pub(super) fn ask_chat(&self, key: &str) {
        self.read_chat(key, true);
    }

    fn read_chat(&self, key: &str, asked: bool) {
        let Some(chat) = self.chat(key) else {
            return;
        };
        {
            let mut state = chat.state.borrow_mut();
            if !asked && state.backing_off() {
                return;
            }
            // The core marks a conversation it reads read on every device:
            // unasked, it is read only while the window is in front, so
            // your phone still shows it unread. Coming to the front reads
            // it (`read_chats_when_active`).
            if !asked && !self.window().is_active() {
                return;
            }
            if state.loading {
                // A reply that arrives while a read is in flight may be
                // newer than what that read returns: read once more when it
                // finishes.
                state.pending = true;
                return;
            }
            state.loading = true;
            state.pending = false;
        }
        let key = key.to_string();
        self.view_call::<method::MessagesHistory>(
            HistoryParams {
                endpoint: key.clone(),
                before: None,
            },
            move |app, result| {
                if app
                    .chat(&key)
                    .is_some_and(|current| Rc::ptr_eq(&current, &chat))
                {
                    app.show_chat(&chat, result);
                }
            },
        );
    }

    fn show_chat(&self, chat: &Rc<Chat>, result: Result<History, crate::bridge::CoreError>) {
        let again = {
            let mut state = chat.state.borrow_mut();
            state.loading = false;
            std::mem::take(&mut state.pending)
        };
        if again {
            let key = chat.key.clone();
            glib::idle_add_local_once(self.on(move |app| app.refresh_chat(&key)));
        }
        let history = match result {
            Err(error) => return self.back_off_chat(chat, Read::History, &error.message),
            Ok(history) => history,
        };
        chat.state.borrow_mut().identity = history.identity.clone();
        self.chat_read(chat, Read::History);
        let ids: Vec<String> = history
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect();
        // A message the core took and this read lists is no longer on its
        // way, whether or not the listing changed.
        chat.retire_echoes(&ids);
        self.mark_chat_seen(chat, &history);
        if ids == chat.state.borrow().shown {
            chat.show_echoes(false);
            return;
        }
        let new_reply = {
            let mut state = chat.state.borrow_mut();
            let new_reply = !state.shown.is_empty()
                && history
                    .messages
                    .iter()
                    .any(|message| !message.mine && !state.shown.contains(&message.id));
            state.shown = ids;
            new_reply
        };
        if new_reply && self.shown_chat().as_deref() != Some(chat.key.as_str()) {
            self.mark_chat(chat, true);
        }
        clear(&chat.messages);
        if history.messages.is_empty() {
            chat.messages.append(&text(
                &format!("No messages with {} yet.", chat.label),
                &["secondary"],
            ));
        }
        for message in &history.messages {
            chat.messages.append(&message_entry(message));
        }
        // A message the core took is listed now; one still on its way
        // stays at the end.
        chat.show_echoes(false);
        let adjustment = chat.scroll.vadjustment();
        glib::idle_add_local_once(move || adjustment.set_value(adjustment.upper()));
    }
}

impl App {
    /// Marks the conversation read on every device when you can see it: it
    /// is the one shown, on the Messages page, in the window in front.
    /// Reading the history does not mark it read, so your phone keeps
    /// showing what you have not seen here.
    fn mark_chat_seen(&self, chat: &Rc<Chat>, history: &History) {
        if !self.window().is_active()
            || self.page() != Page::Messages
            || self.shown_chat().as_deref() != Some(chat.key.as_str())
        {
            return;
        }
        // Relaycast's message ids are increasing integers in decimal.
        let Some(newest) = history
            .messages
            .iter()
            .filter(|message| !message.mine)
            .map(|message| message.id.clone())
            .max_by(|a, b| (a.len(), a).cmp(&(b.len(), b)))
        else {
            return;
        };
        {
            let mut state = chat.state.borrow_mut();
            if state.marked_read.as_ref() == Some(&newest) {
                return;
            }
            state.marked_read = Some(newest.clone());
        }
        if !self.bridge().offers(method::MessagesMarkRead::NAME) {
            return;
        }
        let chat = Rc::downgrade(chat);
        self.call::<method::MessagesMarkRead>(
            MarkReadParams { message_id: newest },
            move |_, result| {
                // Not marked: the next read tries again.
                if let (Err(_), Some(chat)) = (result, chat.upgrade()) {
                    chat.state.borrow_mut().marked_read = None;
                }
            },
        );
    }
}

/// One message of the conversation: who sent it and when, over its text;
/// yours on the right, a reply indented.
fn message_entry(message: &DirectMessage) -> gtk::Box {
    let who = if message.mine {
        "You".to_string()
    } else {
        message
            .sender_address
            .clone()
            .filter(|address| !address.is_empty())
            .unwrap_or_else(|| format!("@{}", message.sender))
    };
    let reply = if message.thread_id.is_some() {
        "↳ Reply · "
    } else {
        ""
    };
    let entry = vbox(4, &[]);
    let meta = text(
        &format!(
            "{reply}{who} · {}",
            message_time(message.created_at.as_deref())
        ),
        &["caption"],
    );
    let body = text(&message.text, &[]).wrapped().copyable();
    if message.mine {
        entry.set_margin_start(super::zoom::px(60));
        meta.set_xalign(1.0);
        body.set_xalign(1.0);
    } else {
        entry.set_margin_end(super::zoom::px(60));
    }
    if message.thread_id.is_some() {
        entry.set_margin_start(entry.margin_start().max(super::zoom::px(28)));
    }
    entry.append(&meta);
    entry.append(&body);
    entry
}
