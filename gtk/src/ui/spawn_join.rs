//! A spawned agent's controls in Chat's details panel: the open button,
//! the take-over and end buttons (`spawn_take_over`, `spawn_end`) and the
//! note on who runs it. Chat's own message box sends into the running
//! agent. The Mac's `SessionOpenButton` is the reference.

use super::app::App;
use super::spawn_end::end_button;
use super::transcript::TranscriptPage;
use super::widgets::{self, text, vbox, LabelStyle};
use crate::bridge::CoreError;
use crate::models::*;
use agent_relay_node::relay::contract::agents::LocalSession;
use agent_relay_node::relay::contract::method;
use agent_relay_node::relay::contract::spawned::AgentTranscriptParams;
use agent_relay_node::relay::contract::transcript::{LocalTranscriptPage, LocalTranscriptParams};
use gtk::{glib, prelude::*};
use gtk4 as gtk;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

mod chat_input;
mod location;
pub use chat_input::ChatInput;

/// The widgets and state of joining a spawned agent.
pub struct SpawnJoin {
    /// In the details panel, under the open button.
    pub actions: gtk::Box,
    pub take_over: gtk::Button,
    pub end: gtk::Button,
    pub note: gtk::Label,
    pub interrupt: gtk::Button,
    pub(super) taking: RefCell<Option<PendingTakeOver>>,
    pub(super) ending: RefCell<HashSet<String>>,
    pub(super) opening: RefCell<HashSet<String>>,
    /// Last verified current-row decision, for wording only. Every Open replans.
    pub(super) open_plan: RefCell<Option<(String, agent_relay_desktop_core::session_locations::SessionOpenPlan)>>,
    pub(super) pending_resumes: RefCell<PendingSessionResumes>,
    pub(super) resume_failures: RefCell<HashMap<String, String>>,
    pub(super) pending_resume_note: gtk::Label,
    pub(super) resolve_resume: gtk::Button,
    /// Failures survive switching transcripts, as on macOS.
    pub(super) take_over_errors: RefCell<HashMap<String, String>>,
    pub(super) end_errors: RefCell<HashMap<String, String>>,
}

pub(super) struct PendingTakeOver {
    pub name: String,
    pub key: String,
    pub working: bool,
}

impl SpawnJoin {
    pub fn build() -> SpawnJoin {
        let actions = vbox(6, &[]);
        let pending_resume_note = text(PENDING_RESUME_MESSAGE, &["caption"]).wrapped();
        pending_resume_note.set_visible(false);
        actions.append(&pending_resume_note);
        let resolve_resume = gtk::Button::with_label("Resolve earlier launch…");
        widgets::action_cursor(&resolve_resume);
        resolve_resume.set_visible(false);
        actions.append(&resolve_resume);
        let take_over = gtk::Button::with_label(SPAWN_TAKE_OVER_TITLE);
        widgets::action_cursor(&take_over);
        take_over.add_css_class("session-action");
        take_over.set_hexpand(true);
        take_over.set_tooltip_text(Some(SPAWN_TAKE_OVER_HELP));
        widgets::name(&take_over, SPAWN_TAKE_OVER_TITLE);
        take_over.set_visible(false);
        actions.append(&take_over);
        let end = end_button();
        actions.append(&end);
        let note = text("", &["caption"]).wrapped();
        note.set_max_width_chars(28);
        note.set_visible(false);
        actions.append(&note);
        let interrupt = gtk::Button::with_label(SPAWN_INTERRUPT_NOW);
        widgets::action_cursor(&interrupt);
        interrupt.set_halign(gtk::Align::Start);
        interrupt.set_visible(false);
        actions.append(&interrupt);
        SpawnJoin {
            actions,
            take_over,
            end,
            note,
            interrupt,
            taking: RefCell::default(),
            ending: RefCell::default(),
            opening: RefCell::default(),
            open_plan: RefCell::default(),
            pending_resumes: RefCell::default(),
            resume_failures: RefCell::default(),
            pending_resume_note,
            resolve_resume,
            take_over_errors: RefCell::default(),
            end_errors: RefCell::default(),
        }
    }

    pub(super) fn show_note(&self, value: &str) {
        self.note.set_text(value);
        self.note.set_visible(!value.is_empty());
    }
}

impl App {
    pub(super) fn connect_spawn_join(&self, page: &TranscriptPage) {
        *page.join.pending_resumes.borrow_mut() = PendingSessionResumes::load(self.prefs().path());
        self.connect_end_agent(page);
        self.connect_take_over(page);
        let open = self.on(|app| app.open_transcript_session());
        page.open.connect_clicked(move |_| open());
        let resolve = self.on(|app| app.resolve_pending_session_resume());
        page.join.resolve_resume.connect_clicked(move |_| resolve());
    }

    /// The transcript page's session as last read: a spawned agent's changes
    /// as it is taken over.
    pub(super) fn joined_session(&self, row: &LocalSessionRow) -> Option<LocalSession> {
        let relay = row.relay.as_ref()?;
        self.listed_session(row).or_else(|| Some(relay.clone()))
    }

    /// The row's session as the session list has it now, without the
    /// row's own copy standing in for one the list no longer has. What
    /// holds a session is read from this: a row keeps the copy it was
    /// built with, and a session that has ended holds nothing.
    pub(super) fn listed_session(&self, row: &LocalSessionRow) -> Option<LocalSession> {
        let key = row.relay.as_ref()?.target.key();
        self.with_state(|state| {
            state
                .local_sessions
                .iter()
                .find(|session| session.target.key() == key)
                .cloned()
        })
    }

    /// One page of the transcript page's session: a spawned agent's own
    /// output while Agent Relay runs it headless (`agent`), else the
    /// session's transcript.
    pub(super) fn transcript_page(
        &self,
        agent: Option<String>,
        params: LocalTranscriptParams,
        done: impl FnOnce(&App, Result<LocalTranscriptPage, CoreError>) + 'static,
    ) {
        match agent {
            Some(name) => self.view_call::<method::AgentsTranscript>(
                AgentTranscriptParams {
                    name,
                    cursor: params.cursor,
                },
                done,
            ),
            None => self
                .view_call::<agent_relay_desktop_core::contract::method::SessionsTranscript>(
                    params, done,
                ),
        }
    }

    /// Shows the open, take-over and end buttons and the hosted note for
    /// the selected session as last read.
    pub(super) fn render_spawn_join(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let page = &shell.pages.sessions.transcript;
        let Some(row) = page.selected.borrow().clone() else {
            return;
        };
        let session = self.joined_session(&row);
        let listed = self.listed_session(&row);
        let opening = SessionOpening::for_session(&row, session.as_ref(), listed.as_ref());
        let spawned_plan = SessionOpening::spawned_live_plan(&row, session.as_ref());
        let can_open = SessionOpening::may_request_plan(&row, session.as_ref()) || spawned_plan.is_some();
        page.open.set_visible(can_open);
        page.open
            .set_sensitive(!page.join.opening.borrow().contains(&row.key()));
        if can_open {
            let title = super::session_open::title_for(&row.key(), spawned_plan.as_ref(), &page.join.open_plan.borrow());
            page.open.set_child(Some(&super::session_open::open_session_label(&row.icon_source(), &title)));
            page.open.set_tooltip_text(Some(&title));
            widgets::name(&page.open, &title);
        }
        let join = &page.join;
        let pending_resumes = join.pending_resumes.borrow();
        let resume_pending = pending_resumes.contains(&row.key());
        join.pending_resume_note
            .set_text(pending_resumes.error().unwrap_or(PENDING_RESUME_MESSAGE));
        join.pending_resume_note
            .set_visible(resume_pending || pending_resumes.error().is_some());
        join.resolve_resume.set_visible(resume_pending);
        join.resolve_resume
            .set_sensitive(!join.opening.borrow().contains(&row.key()));
        let handover = SpawnHandover::of(session.as_ref());
        let headless = handover == Some(SpawnHandover::TakeOver);
        join.take_over.set_visible(headless);
        let endable = SpawnHandover::can_end(session.as_ref());
        join.end.set_visible(endable);
        let taking = join.taking.borrow();
        join.take_over.set_sensitive(taking.is_none());
        let pending = taking.as_ref().filter(|pending| pending.key == row.key());
        let pending_end = join.ending.borrow().contains(&row.key());
        join.end.set_sensitive(!pending_end);
        join.end.set_label(if pending_end {
            SPAWN_ENDING
        } else {
            SPAWN_END_BUTTON
        });
        join.interrupt
            .set_visible(pending.is_some_and(|pending| pending.working));
        if pending_end {
            join.show_note(SPAWN_ENDING);
        } else if let Some(pending) = pending {
            join.show_note(&if pending.working {
                spawn_waiting(&pending.name)
            } else {
                SPAWN_STOPPING.to_string()
            });
        } else {
            join.show_note(&match handover {
                Some(SpawnHandover::TakenOver) => spawn_taken_over(&registration_name(&session)),
                _ => SpawnHandover::hosted_note(session.as_ref(), opening.as_ref())
                    .or_else(|| SessionOpening::held_note(listed.as_ref()))
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        let error = join
            .end_errors
            .borrow()
            .get(&row.key())
            .or(join.take_over_errors.borrow().get(&row.key()))
            .cloned();
        if let Some(error) = error {
            page.show_opening_error(&error);
        }
    }
}

/// The agent name of `session`'s registration; empty without one.
fn registration_name(session: &Option<LocalSession>) -> String {
    session
        .as_ref()
        .and_then(|session| session.registration.as_ref())
        .map(|registration| registration.name.clone())
        .unwrap_or_default()
}

impl TranscriptPage {
    /// Shows why opening the session failed; empty hides it.
    pub(super) fn show_opening_error(&self, error: &str) {
        self.opening_error.set_text(error);
        self.opening_error.set_visible(!error.is_empty());
    }
}
