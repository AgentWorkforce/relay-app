//! The transcript sidebar's Open button over the engine
//! (`agent_relay_desktop_core::session_open`): a look when a session is
//! shown, when the window becomes active and on a slow timer while it is
//! shown; one open per click; the questions an answer asks; and releasing
//! a pending launch. The engine runs through
//! [`super::super::desktop::Desktop`], off the main thread, so tests stand
//! in a fake; every sentence and title shown is the engine's. The Mac's
//! `SessionOpenButton` and `SessionOpenModel` are the reference.
use super::*;
use crate::prefs::SessionOpenPreference;

impl App {
    /// Open, its follow-up buttons, and when the button is looked at:
    /// Refresh, the window becoming active, and a slow timer while the
    /// button is shown in the active window (the Mac's `SessionOpenButton`).
    pub(super) fn connect_session_open(&self, page: &TranscriptPage) {
        let open = self.on(|app| app.open_transcript_session());
        page.open.connect_clicked(move |_| open());
        let resolve = self.on(|app| app.resolve_pending_session_resume());
        page.join.resolve_resume.connect_clicked(move |_| resolve());
        let fallback = self.on(|app| app.resume_in_default_terminal());
        page.join
            .resume_default
            .connect_clicked(move |_| fallback());
        let look = self.on(|app| app.plan_session_open());
        page.refresh.connect_clicked(move |_| look());
        let weak = self.downgrade();
        self.inner.window.connect_is_active_notify(move |window| {
            if let Some(app) = weak.upgrade().filter(|_| window.is_active()) {
                app.plan_shown_session_open();
            }
        });
        let weak = self.downgrade();
        glib::timeout_add_local(PLAN_EVERY, move || match weak.upgrade() {
            Some(app) => {
                app.plan_shown_session_open();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }

    /// The selected row, when the engine opens it.
    fn engine_row(&self) -> Option<LocalSessionRow> {
        let shell = self.shell()?;
        let row = shell.pages.sessions.transcript.selected.borrow().clone()?;
        let session = self.joined_session(&row);
        SessionOpening::may_request_plan(&row, session.as_ref()).then_some(row)
    }

    fn selected_key(&self) -> Option<String> {
        let shell = self.shell()?;
        let key = shell
            .pages
            .sessions
            .transcript
            .selected
            .borrow()
            .as_ref()
            .map(LocalSessionRow::key);
        key
    }

    fn open_request(&self, row: &LocalSessionRow, resend: &OpenResend) -> Request {
        let target = OpenTarget::new(row.source(), &row.session_id(), &row.directory());
        // "Open with: Terminal" exists for Claude and Codex, as on the Mac.
        let source = row.source();
        let prefer = matches!(source, "claude" | "codex")
            && self.prefs().session_opening(source) == SessionOpenPreference::Terminal;
        // Relay keeps no terminal choice of its own on Linux and Windows
        // yet: the engine uses the system's default terminal.
        target.request(
            &open_app_data_dir(self.prefs().path()),
            None,
            prefer,
            resend,
        )
    }

    /// Forgets what Open showed for the selected session and looks again,
    /// as selecting a session does.
    pub(in crate::ui) fn reset_session_open(&self) {
        let Some(shell) = self.shell() else { return };
        if let Some(key) = self.selected_key() {
            shell
                .pages
                .sessions
                .transcript
                .join
                .opens
                .borrow_mut()
                .forget(&key);
        }
        self.plan_session_open();
    }

    /// A timer tick or the window becoming active: look again only while
    /// the button is on screen in the active window.
    pub(in crate::ui) fn plan_shown_session_open(&self) {
        let Some(shell) = self.shell() else { return };
        if self.inner.window.is_active() && shell.pages.sessions.transcript.open.is_mapped() {
            self.plan_session_open();
        }
    }

    /// Asks the engine what Open would do for the selected session; what
    /// was shown stays until it answers.
    pub(in crate::ui) fn plan_session_open(&self) {
        let Some(shell) = self.shell() else { return };
        let Some(row) = self.engine_row() else { return };
        let key = row.key();
        let ticket = shell
            .pages
            .sessions
            .transcript
            .join
            .opens
            .borrow_mut()
            .begin_plan(&key);
        let Some(ticket) = ticket else { return };
        let planned = self
            .inner
            .desktop
            .plan_session(self.open_request(&row, &OpenResend::default()));
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let plan = planned.await;
            let Some(app) = weak.upgrade() else { return };
            let Some(shell) = app.shell() else { return };
            shell
                .pages
                .sessions
                .transcript
                .join
                .opens
                .borrow_mut()
                .planned(&key, ticket, plan);
            if app.selected_key().as_ref() == Some(&key) {
                app.render_spawn_join();
            }
        });
    }

    pub(super) fn open_transcript_session(&self) {
        let Some(shell) = self.shell() else { return };
        let Some(row) = shell.pages.sessions.transcript.selected.borrow().clone() else {
            return;
        };
        let session = self.joined_session(&row);
        if let Some(link) = SessionOpening::spawned_live_link(&row, session.as_ref()) {
            self.open_spawned_live(&link);
            return;
        }
        if !SessionOpening::may_request_plan(&row, session.as_ref()) {
            return;
        }
        let click = shell
            .pages
            .sessions
            .transcript
            .join
            .opens
            .borrow()
            .click(&row.key());
        self.answer_session_click(row.key(), click);
    }

    /// A spawned Codex thread opens live in Codex, or says Codex is needed.
    fn open_spawned_live(&self, link: &str) {
        let Some(shell) = self.shell() else { return };
        let page = &shell.pages.sessions.transcript;
        if self.inner.desktop.has_uri_handler("codex") && self.inner.desktop.open_uri(link) {
            page.show_opening_error("");
        } else {
            page.show_opening_error(SPAWN_CODEX_MISSING);
        }
    }

    pub(super) fn resume_in_default_terminal(&self) {
        let Some(key) = self.selected_key() else {
            return;
        };
        let resend = OpenResend {
            use_default_terminal: true,
            ..OpenResend::default()
        };
        self.answer_session_click(key, OpenClick::Open(resend));
    }

    /// Asks whether the earlier launch was closed; "I closed or stopped
    /// it" releases its reservation and nothing else. The person then
    /// clicks Open.
    pub(super) fn resolve_pending_session_resume(&self) {
        let Some(shell) = self.shell() else { return };
        let Some(key) = self.selected_key() else {
            return;
        };
        let view = shell
            .pages
            .sessions
            .transcript
            .join
            .opens
            .borrow()
            .view(&key);
        if !view.pending_launch || view.opening || view.releasing {
            return;
        }
        let words = view.strings;
        let question = Confirmation::new(
            &words.launch_question,
            &words.launch_detail,
            &words.launch_closed,
        )
        .cancel(&words.launch_wait);
        let asked = self.inner.desktop.confirm(&self.inner.window, question);
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let closed = asked.await;
            let Some(app) = weak.upgrade().filter(|_| closed) else {
                return;
            };
            app.release_session_launch(key);
        });
    }

    /// One release of `key`'s pending launch; its answer redraws the
    /// button. Nothing is opened.
    fn release_session_launch(&self, key: String) {
        let Some(shell) = self.shell() else { return };
        let Some(row) = self.engine_row().filter(|row| row.key() == key) else {
            return;
        };
        if !shell
            .pages
            .sessions
            .transcript
            .join
            .opens
            .borrow_mut()
            .begin_release(&key)
        {
            return;
        }
        self.render_spawn_join();
        let released = self
            .inner
            .desktop
            .release_session(self.open_request(&row, &OpenResend::default()));
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let plan = released.await;
            let Some(app) = weak.upgrade() else { return };
            let Some(shell) = app.shell() else { return };
            let answered = plan.is_some();
            shell
                .pages
                .sessions
                .transcript
                .join
                .opens
                .borrow_mut()
                .released(&key, plan);
            if app.selected_key().as_ref() != Some(&key) {
                return;
            }
            app.render_spawn_join();
            if !answered {
                app.plan_session_open();
            }
        });
    }

    /// Does what `click` asks for the session `key`: opens it, or asks the
    /// question its answer needs first, then opens with the reply. Nothing
    /// is sent for a session no longer shown.
    fn answer_session_click(&self, key: String, click: OpenClick) {
        match click {
            OpenClick::Open(resend) => self.run_session_open(key, resend),
            OpenClick::Choose {
                base,
                title,
                message,
                choices,
            } => {
                let labels = choices.iter().map(|choice| choice.label.clone()).collect();
                let picked = self
                    .inner
                    .desktop
                    .pick(&self.inner.window, &title, &message, labels);
                let weak = self.downgrade();
                glib::spawn_future_local(async move {
                    let picked = picked.await.and_then(|index| choices.get(index).cloned());
                    let (Some(app), Some(choice)) = (weak.upgrade(), picked) else {
                        return;
                    };
                    app.run_session_open(key, OpenClick::chosen(&base, &choice.id));
                });
            }
            OpenClick::Confirm {
                base,
                title,
                question,
                take_over,
            } => {
                let cancel = self.session_open_strings(&key).cancel;
                let mut asked = Confirmation::new(&title, &question, &title).cancel(&cancel);
                if take_over {
                    asked = asked.destructive();
                }
                let asked = self.inner.desktop.confirm(&self.inner.window, asked);
                let weak = self.downgrade();
                glib::spawn_future_local(async move {
                    let confirmed = asked.await;
                    let Some(app) = weak.upgrade().filter(|_| confirmed) else {
                        return;
                    };
                    app.run_session_open(key, OpenClick::confirmed(&base, take_over));
                });
            }
            OpenClick::Nothing => {}
        }
    }

    fn session_open_strings(
        &self,
        key: &str,
    ) -> agent_relay_desktop_core::session_open::contract::Strings {
        match self.shell() {
            Some(shell) => {
                shell
                    .pages
                    .sessions
                    .transcript
                    .join
                    .opens
                    .borrow()
                    .view(key)
                    .strings
            }
            None => open_strings(),
        }
    }

    /// One open of `key`, unless one already runs; its answer redraws the
    /// button, and a question it stopped at is asked next. An open that
    /// ended without an answer is said, never retried, and looked at again.
    fn run_session_open(&self, key: String, resend: OpenResend) {
        let Some(shell) = self.shell() else { return };
        let Some(row) = self.engine_row().filter(|row| row.key() == key) else {
            return;
        };
        let page = &shell.pages.sessions.transcript;
        if !page.join.opens.borrow_mut().begin_open(&key) {
            return;
        }
        self.render_spawn_join();
        let opened = self
            .inner
            .desktop
            .open_session(self.open_request(&row, &resend));
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = opened.await;
            let Some(app) = weak.upgrade() else { return };
            let Some(shell) = app.shell() else { return };
            let answered = result.is_some();
            let next = shell
                .pages
                .sessions
                .transcript
                .join
                .opens
                .borrow_mut()
                .opened(&key, &resend, result);
            if app.selected_key().as_ref() != Some(&key) {
                return;
            }
            app.render_spawn_join();
            if !answered {
                app.plan_session_open();
            }
            app.answer_session_click(key, next);
        });
    }

    /// Draws Open, its sentence, and the pending-launch and default-terminal
    /// controls for `row`.
    pub(super) fn render_session_open(
        &self,
        page: &TranscriptPage,
        row: &LocalSessionRow,
        session: Option<&LocalSession>,
    ) {
        let join = &page.join;
        let live = SessionOpening::spawned_live_link(row, session).is_some();
        let engine = SessionOpening::may_request_plan(row, session);
        page.open.set_visible(live || engine);
        let view = engine.then(|| join.opens.borrow().view(&row.key()));
        let strings = view
            .as_ref()
            .map_or_else(open_strings, |view| view.strings.clone());
        let title = match &view {
            Some(view) => view.title.clone(),
            None => SessionOpening::for_session(row, session, session)
                .map_or(strings.open_session.clone(), |opening| {
                    opening.title().to_string()
                }),
        };
        page.open
            .set_sensitive(view.as_ref().is_none_or(|view| view.sensitive));
        if live || engine {
            let terminal = view.as_ref().is_some_and(|view| view.terminal);
            page.open
                .set_child(Some(&super::super::session_open::open_session_label(
                    &row.icon_source(),
                    &title,
                    terminal,
                )));
            page.open.set_tooltip_text(Some(&title));
            widgets::name(&page.open, &title);
        }
        let view = view.unwrap_or_else(|| SessionOpens::default().view(""));
        let reason = view.reason.clone().filter(|_| engine).unwrap_or_default();
        join.open_reason.set_text(&reason);
        join.open_reason.set_visible(!reason.is_empty());
        join.resolve_resume.set_label(&strings.resolve_launch);
        join.resolve_resume
            .set_visible(engine && view.pending_launch);
        join.resolve_resume
            .set_sensitive(!view.opening && !view.releasing);
        join.resume_default
            .set_child(Some(&super::super::session_open::open_session_label(
                &row.icon_source(),
                &strings.resume_default,
                true,
            )));
        widgets::name(&join.resume_default, &strings.resume_default);
        join.resume_default
            .set_visible(engine && view.offer_default);
        join.resume_default
            .set_sensitive(!view.opening && !view.releasing);
        if engine {
            page.show_opening_error(view.notice.as_deref().unwrap_or_default());
        }
    }
}
