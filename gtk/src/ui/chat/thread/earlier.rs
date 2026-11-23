//! A long conversation drawn from its end: the newest [`SHOWN`] entries
//! first, and a button above them that draws as many again. Drawing a whole
//! session at once (thousands of messages and tool calls) holds the window
//! up long enough for the system to call it not responding.

use super::*;

/// How many entries a conversation opens with, and how many more each
/// press of the button draws.
pub(super) const SHOWN: usize = 300;

pub(super) fn button() -> gtk::Button {
    let earlier = gtk::Button::with_label(CHAT_EARLIER);
    earlier.add_css_class("flat");
    earlier.set_halign(gtk::Align::Center);
    widgets::action_cursor(&earlier);
    earlier.set_visible(false);
    earlier
}

impl Thread {
    /// Keeps `entries` read while the conversation opens; drawn at the end.
    pub(super) fn take_page(&self, entries: Vec<LocalTranscriptEntry>) {
        self.loading.borrow_mut().extend(entries);
    }

    /// Draws the newest of what was read as the conversation opened, and
    /// keeps the rest for the button.
    pub(super) fn finish_loading(&self, source: &str, person: &str) {
        let all = std::mem::take(&mut *self.loading.borrow_mut());
        // A fresh read is the whole conversation: it replaces what was
        // remembered, whose entries are already drawn and not drawn again.
        self.kept.borrow_mut().clear();
        self.draw_newest(all, SHOWN.max(self.showing.get()), source, person);
    }

    /// Draws `count` more entries above those drawn.
    pub(super) fn show_earlier(&self, source: &str, person: &str) {
        let all = std::mem::take(&mut *self.kept.borrow_mut());
        let count = self.showing.get() + SHOWN;
        self.clear_drawn();
        self.draw_newest(all, count, source, person);
    }

    /// Draws the newest `count` of `all`, keeping all of it for the
    /// conversation's next opening and for the button.
    fn draw_newest(
        &self,
        mut all: Vec<LocalTranscriptEntry>,
        count: usize,
        source: &str,
        person: &str,
    ) {
        let newest = all.split_off(all.len().saturating_sub(count));
        self.earlier.set_visible(!all.is_empty());
        self.showing.set(newest.len());
        // `append` keeps what it draws after what is already kept.
        let mut kept = self.kept.borrow_mut();
        let drawn_before = std::mem::take(&mut *kept);
        *kept = all;
        kept.extend(drawn_before);
        drop(kept);
        self.append(newest, source, person);
    }

    /// Forgets what is drawn, keeping what was read.
    fn clear_drawn(&self) {
        clear(&self.messages);
        self.ids.borrow_mut().clear();
        self.relay_seen.borrow_mut().clear();
        self.drawn_texts.borrow_mut().clear();
        self.group.borrow_mut().take();
    }
}

impl App {
    /// Draws more of the conversation shown, above what is drawn.
    pub(in crate::ui::chat) fn show_earlier_chat(&self) {
        let Some(agent) = self
            .with_chat(|page| page.selected.borrow().clone())
            .flatten()
            .and_then(|key| self.chat_agent(&key))
        else {
            return;
        };
        let (source, person) = (entry_source(&agent), self.person_name());
        self.with_chat(|page| page.thread.show_earlier(&source, &person));
    }
}
