//! Ending a session Agent Relay spawned and still owns. The core stops or
//! archives the exact harness session, confirms it ended, then removes its
//! relay identity. The Mac's `SpawnEndButton` is the reference.

use super::app::App;
use super::desktop::Confirmation;
use super::transcript::TranscriptPage;
use super::widgets;
use crate::models::*;
use agent_relay_node::relay::contract::agents::NameParams;
use agent_relay_node::relay::contract::method;
use gtk4::{glib, prelude::*};

pub(super) fn end_button() -> gtk4::Button {
    let button = gtk4::Button::with_label(SPAWN_END_BUTTON);
    widgets::action_cursor(&button);
    button.add_css_class("session-action");
    button.add_css_class("destructive");
    button.set_hexpand(true);
    button.set_tooltip_text(Some(SPAWN_END_HELP));
    widgets::name(&button, SPAWN_END_BUTTON);
    button.set_visible(false);
    button
}

impl App {
    pub(super) fn connect_end_agent(&self, page: &TranscriptPage) {
        let end = self.on(|app| app.confirm_end_agent());
        page.join.end.connect_clicked(move |_| end());
    }

    fn confirm_end_agent(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let Some(row) = shell.pages.sessions.transcript.selected.borrow().clone() else {
            return;
        };
        let Some(session) = self.joined_session(&row) else {
            return;
        };
        if !SpawnHandover::can_end(Some(&session)) {
            return;
        }
        let Some(name) = session.registration.as_ref().map(|r| r.name.clone()) else {
            return;
        };
        let answer = self.confirm(
            Confirmation::new(
                &spawn_end_title(&name),
                &spawn_end_message(&name),
                SPAWN_END_BUTTON,
            )
            .destructive(),
        );
        let weak = self.downgrade();
        let key = row.key();
        glib::spawn_future_local(async move {
            if let (true, Some(app)) = (answer.await, weak.upgrade()) {
                app.end_spawned_agent(name, key);
            }
        });
    }

    fn end_spawned_agent(&self, name: String, key: String) {
        let Some(shell) = self.shell() else {
            return;
        };
        let join = &shell.pages.sessions.transcript.join;
        if !join.ending.borrow_mut().insert(key.clone()) {
            return;
        }
        join.end_errors.borrow_mut().remove(&key);
        shell.pages.sessions.transcript.show_opening_error("");
        self.render_transcript_controls();
        self.view_call::<method::AgentsEnd>(NameParams { name }, move |app, result| {
            if let Some(shell) = app.shell() {
                let join = &shell.pages.sessions.transcript.join;
                join.ending.borrow_mut().remove(&key);
                if let Err(error) = result {
                    join.end_errors.borrow_mut().insert(
                        key.clone(),
                        format!("{SPAWN_END_FAILED}: {}", error.message),
                    );
                }
            }
            app.refresh_local();
            app.refresh_agents();
            app.render_transcript_controls();
        });
    }
}
