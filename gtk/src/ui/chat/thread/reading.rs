//! Reading the conversation shown: from the start when it opens (drawing
//! what was read before at once), then what was written since, every few
//! seconds while the page is shown.

use super::*;

impl App {
    pub(super) fn person_name(&self) -> String {
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
        // What was read before shows at once; the read below adds what is
        // new, and what is already drawn is not drawn again.
        let person = self.person_name();
        let source = entry_source(&agent);
        self.with_chat(|page| {
            if let Some(entries) = page.thread.recall(key) {
                // Only the newest, as when it opened; the rest is kept.
                page.thread.take_page(entries);
                page.thread.finish_loading(&source, &person);
                page.thread.scroll_to_latest();
            }
        });
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
                    app.chat_read_ended();
                    app.with_chat(|chat| {
                        // A remembered conversation stays; the failure is
                        // noted under the box.
                        if !chat.thread.ids.borrow().is_empty() {
                            chat.composer.note.set_text(&error.message);
                            *chat.thread.cursor.borrow_mut() = Some(requested.clone());
                            return;
                        }
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
            app.with_chat(|chat| chat.thread.take_page(page.entries));
            match next {
                Some(next) if pages + 1 < MAX_PAGES => {
                    app.read_chat_pages(agent, Some(next), generation, pages + 1)
                }
                next => {
                    app.chat_read_ended();
                    app.with_chat(|chat| {
                        chat.thread.finish_loading(&source, &person);
                        *chat.thread.cursor.borrow_mut() = Some(next.or(follow));
                        chat.thread.scroll_to_latest();
                    });
                }
            }
        });
    }

    /// Reads what the agent shown wrote since, every few seconds while the
    /// page is shown.
    pub(in crate::ui::chat) fn follow_chat_thread(&self) {
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
        if self.page() != crate::ui::state::Page::Chat || !self.window().is_visible() {
            return;
        }
        let Some((key, cursor, generation)) = self
            .with_chat(|page| {
                let key = page.selected.borrow().clone()?;
                let cursor = page.thread.cursor.borrow().clone()?;
                let tick = page.thread.ticks.get().wrapping_add(1);
                page.thread.ticks.set(tick);
                // Reading an agent elsewhere reads your messages with it,
                // and the core marks them read on every device: only while
                // the window is in front, so your phone still shows them.
                if page.thread.elsewhere.get()
                    && (tick % ELSEWHERE_TICKS != 0 || !self.window().is_active())
                {
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
