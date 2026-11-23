use super::*;
use agent_relay_desktop_core::session_locations::{
    SessionLocation, SessionOpenAction, SessionOpenPlan, SessionOpenRequest, SessionsOpenPlan,
};

impl App {
    pub(super) fn open_transcript_session(&self) {
        self.plan_transcript_open(None);
    }

    fn plan_transcript_open(&self, selected: Option<SessionLocation>) {
        self.request_session_open(selected, false);
    }
    fn request_session_open(&self, selected: Option<SessionLocation>, use_default_terminal: bool) {
        let Some(shell) = self.shell() else { return };
        let page = &shell.pages.sessions.transcript;
        let Some(row) = page.selected.borrow().clone() else {
            return;
        };
        // Keep the spawned/headless handover guard in front of every plan,
        // including a chooser response after the holder may have changed.
        let session = self.joined_session(&row);
        let spawned_plan = SessionOpening::spawned_live_plan(&row, session.as_ref());
        if !SessionOpening::may_request_plan(&row, session.as_ref()) && spawned_plan.is_none() {
            return;
        }
        let key = row.key();
        if page.join.opening.borrow().contains(&key) {
            return;
        }
        page.join.opening.borrow_mut().insert(key.clone());
        page.open.set_sensitive(false);
        page.show_opening_error("");
        if let Some(plan) = spawned_plan {
            self.dispatch_session_plan(key, plan);
            return;
        }
        let request = SessionOpenRequest {
            use_default_terminal,
            source: row.source().into(),
            session_id: row.session_id(),
            directory: Some(row.directory()),
            selected,
            prefer_terminal: self.prefs().session_opening(row.source())
                == crate::prefs::SessionOpenPreference::Terminal,
        };
        let generation = self.with_state(|state| state.generation);
        self.call::<SessionsOpenPlan>(request, move |app,result| {
            let Some(shell) = app.shell() else { return };
            let page = &shell.pages.sessions.transcript;
            if app.with_state(|state|state.generation)!=generation {
                page.join.opening.borrow_mut().remove(&key);
                if page.selected.borrow().as_ref().map(|row|row.key()).as_ref()==Some(&key) {page.open.set_sensitive(true);}
                return;
            }
            if page.selected.borrow().as_ref().map(|row|row.key()).as_ref()!=Some(&key) {
                page.join.opening.borrow_mut().remove(&key); return;
            }
            if let Ok(plan) = &result {
                *page.join.open_plan.borrow_mut() = Some((key.clone(), plan.clone()));
                let observed = page.join.pending_resumes.borrow_mut().observe(&key, plan);
                observed.unwrap_or_else(|error| page.show_opening_error(&error));
                app.render_spawn_join();
                if use_default_terminal && (plan.action != SessionOpenAction::Resume || plan.location.is_some()) {
                    page.join.opening.borrow_mut().remove(&key);
                    page.open.set_sensitive(true);
                    page.show_opening_error("This session’s state changed. Refresh before resuming it in a default terminal.");
                    return;
                }
            }
            match result {
                Ok(plan) if plan.action==SessionOpenAction::Choose => {
                    page.join.opening.borrow_mut().remove(&key);
                    page.open.set_sensitive(true);
                    app.choose_session_location(key,plan.choices);
                }
                Ok(plan) => app.dispatch_session_plan(key,plan),
                Err(error) => app.fail_session_open(&key, &error),
            }
        });
    }

    fn fail_session_open(&self, key: &str, error: &impl std::fmt::Display) {
        let Some(shell) = self.shell() else { return };
        let page = &shell.pages.sessions.transcript;
        // Release the opening guard first so the re-render below leaves
        // "Resolve earlier launch" sensitive.
        page.join.opening.borrow_mut().remove(key);
        let stale = page
            .join
            .open_plan
            .borrow()
            .as_ref()
            .is_some_and(|(row, _)| row == key);
        if stale {
            page.join.open_plan.borrow_mut().take();
            self.render_spawn_join();
        }
        page.open.set_sensitive(true);
        page.show_opening_error(&format!(
            "Couldn’t verify session opening: {error}. No replacement session was started."
        ));
    }

    fn choose_session_location(&self, key: String, choices: Vec<SessionLocation>) {
        let dialog = gtk::Dialog::builder()
            .transient_for(&self.inner.window)
            .modal(true)
            .title("Choose session location")
            .build();
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        for (index, location) in choices.iter().enumerate() {
            let label = format!(
                "{} · {} · process {}",
                location.application,
                location.tty.as_deref().unwrap_or("application"),
                location.pid
            );
            dialog.add_button(&label, gtk::ResponseType::Other(index as u16));
        }
        let generation = self.with_state(|state| state.generation);
        let weak = self.downgrade();
        dialog.connect_response(move |dialog, response| {
            dialog.close();
            let gtk::ResponseType::Other(index) = response else {
                return;
            };
            let Some(app) = weak.upgrade() else { return };
            if app.with_state(|state| state.generation) != generation {
                return;
            }
            let Some(shell) = app.shell() else { return };
            if shell
                .pages
                .sessions
                .transcript
                .selected
                .borrow()
                .as_ref()
                .map(|row| row.key())
                .as_ref()
                != Some(&key)
            {
                return;
            }
            if let Some(location) = choices.get(index as usize) {
                app.plan_transcript_open(Some(location.clone()));
            }
        });
        dialog.present();
    }

    fn dispatch_session_plan(&self, key: String, plan: SessionOpenPlan) {
        let Some(shell) = self.shell() else { return };
        let page = &shell.pages.sessions.transcript;
        let reserved = page.join.pending_resumes.borrow_mut().begin(&key, &plan);
        match reserved {
            Ok(true) => {}
            result => {
                page.join.opening.borrow_mut().remove(&key);
                page.open.set_sensitive(true);
                let error = result.err().unwrap_or_else(|| PENDING_RESUME_MESSAGE.into());
                page.show_opening_error(&error);
                self.render_spawn_join();
                return;
            }
        }
        page.join.resume_failures.borrow_mut().remove(&key);
        self.render_spawn_join();
        let may_retry = plan.can_resume_default;
        let opened = self.inner.desktop.open_session(plan);
        let generation = self.with_state(|state| state.generation);
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = opened.await;
            let Some(app) = weak.upgrade() else { return };
            let Some(shell) = app.shell() else { return };
            let page = &shell.pages.sessions.transcript;
            page.join.opening.borrow_mut().remove(&key);
            app.render_spawn_join();
            if app.with_state(|state| state.generation) != generation {
                if page
                    .selected
                    .borrow()
                    .as_ref()
                    .map(|row| row.key())
                    .as_ref()
                    == Some(&key)
                {
                    page.open.set_sensitive(true);
                }
                return;
            }
            if page
                .selected
                .borrow()
                .as_ref()
                .map(|row| row.key())
                .as_ref()
                != Some(&key)
            {
                return;
            }
            page.open.set_sensitive(true);
            page.show_opening_error(match &result {
                Ok(crate::session_navigation::Outcome::Verified) => "Original window selected.",
                Ok(crate::session_navigation::Outcome::Dispatched) => {
                    "Opening request sent. The application has not confirmed session selection."
                }
                Err(error) => error,
            });
            if let Err(error) = result {
                if may_retry {
                    page.join.resume_failures.borrow_mut().insert(key.clone(), error.clone());
                    if !page.join.pending_resumes.borrow().contains(&key) {
                        app.offer_default_terminal(key, error);
                    }
                }
            }
        });
    }

    pub(super) fn resolve_pending_session_resume(&self) {
        let Some(shell) = self.shell() else { return };
        let page = &shell.pages.sessions.transcript;
        let Some(key) = page.selected.borrow().as_ref().map(|row| row.key()) else {
            return;
        };
        if !page.join.pending_resumes.borrow().contains(&key)
            || page.join.opening.borrow().contains(&key)
        {
            return;
        }
        let dialog = gtk::MessageDialog::builder()
            .transient_for(&self.inner.window)
            .modal(true)
            .text("Did you close or stop the earlier launch?")
            .secondary_text("The earlier terminal or application may still be starting the provider. Close or stop that launch before allowing another resume request.")
            .buttons(gtk::ButtonsType::None)
            .build();
        dialog.add_button("Keep waiting", gtk::ResponseType::Cancel);
        dialog.add_button("I closed or stopped it", gtk::ResponseType::Accept);
        let generation = self.with_state(|state| state.generation);
        let weak = self.downgrade();
        dialog.connect_response(move |dialog, response| {
            dialog.close();
            if response != gtk::ResponseType::Accept {
                return;
            }
            let Some(app) = weak.upgrade() else { return };
            if app.with_state(|state| state.generation) != generation {
                return;
            }
            let Some(shell) = app.shell() else { return };
            let page = &shell.pages.sessions.transcript;
            if page
                .selected
                .borrow()
                .as_ref()
                .map(|row| row.key())
                .as_ref()
                != Some(&key)
            {
                return;
            }
            let opening = page.join.opening.borrow().contains(&key);
            let resolved = page
                .join
                .pending_resumes
                .borrow_mut()
                .resolve_closed_launch(&key, opening);
            if let Err(error) = &resolved {
                page.show_opening_error(error);
                app.render_spawn_join();
            }
            if resolved == Ok(true) {
                page.show_opening_error("");
                app.render_spawn_join();
                let failure = page.join.resume_failures.borrow_mut().remove(&key);
                if let Some(error) = failure {
                    app.offer_default_terminal(key.clone(), error);
                }
            }
        });
        dialog.present();
    }

    fn offer_default_terminal(&self, key: String, error: String) {
        let dialog = gtk::MessageDialog::builder()
            .transient_for(&self.inner.window)
            .modal(true)
            .text("The original application couldn’t resume this session")
            .secondary_text(&error)
            .buttons(gtk::ButtonsType::None)
            .build();
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Resume in default terminal", gtk::ResponseType::Accept);
        let generation = self.with_state(|state| state.generation);
        let weak = self.downgrade();
        dialog.connect_response(move |dialog, response| {
            dialog.close();
            if response != gtk::ResponseType::Accept { return; }
            let Some(app) = weak.upgrade() else { return };
            if app.with_state(|state| state.generation) != generation { return; }
            let Some(shell) = app.shell() else { return };
            let page = &shell.pages.sessions.transcript;
            if page.selected.borrow().as_ref().map(|row| row.key()).as_ref() != Some(&key)
                || page.join.pending_resumes.borrow().contains(&key) { return; }
            // A fresh core request must prove the previous owner remains stopped.
            app.request_session_open(None, true);
        });
        dialog.present();
    }
}
