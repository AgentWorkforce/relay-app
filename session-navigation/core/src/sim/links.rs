//! What each desktop app does with a URL, modelled on the facts files with
//! the app's own (independent) parsing.
use super::world::{DesktopSession, World};
use crate::model::{App, Provider, SessionRef};

fn app_local_id_ok(id: &str) -> bool {
    id.strip_prefix("local_").is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 64
            && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

fn app_uuid_ok(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

impl World {
    /// Deliver `url` to the app that handles its scheme.
    pub(crate) fn deliver_url(&mut self, app: App, url: &str) {
        if self.ensure_app(app).is_none() {
            return;
        }
        self.front_app = Some(app);
        self.front_pane = None;
        if let Some(id) = url.strip_prefix("claude://code/continue?session=") {
            self.claude_continue(id);
        } else if let Some(id) = url.strip_prefix("claude://resume?session=") {
            self.claude_resume_link(id);
        } else if let Some(id) = url.strip_prefix("codex://threads/") {
            self.codex_link(id);
        }
        // OpenCode 2.2.6 has no route consumer: the window just comes forward.
    }

    fn show(&mut self, app: App, view: Option<String>) {
        self.views.insert(app, view);
    }

    /// The app focuses record `local`: it is shown, and its `lastFocusedAt`
    /// moves forward (VERIFIED, native N1, N5).
    pub(crate) fn focus_claude_record(&mut self, local: &str) {
        let now = self.clock.saturating_mul(1000);
        if let Some(d) = self.claude.desktop.get_mut(local) {
            d.last_focused = Some(d.last_focused.map_or(now, |t| now.max(t.saturating_add(1))));
        }
        self.show(App::ClaudeDesktop, Some(local.to_owned()));
    }

    /// Non-archived match → focus it; archived or unknown → Code home, with
    /// no error and the record untouched: `isArchived` and `lastFocusedAt`
    /// stay as they were (Claude facts B, `_Gr`; VERIFIED, native N3).
    fn claude_continue(&mut self, id: &str) {
        let found = app_local_id_ok(id) && self.claude.desktop.get(id).is_some_and(|d| !d.archived);
        if !found {
            self.show(App::ClaudeDesktop, None);
            return;
        }
        self.focus_claude_record(id);
        if self.vars.claude_continue_starts_process {
            self.start_desktop_child(id);
        }
    }

    /// Whether the desktop app is the version the native checks ran on.
    fn claude_app_verified(&self) -> bool {
        self.apps
            .get(&App::ClaudeDesktop)
            .is_some_and(|a| a.version == super::world::verified_app_version(App::ClaudeDesktop))
    }

    /// `resume` on a record that owns the id revives that same record in
    /// place: `isArchived` false, same `cliSessionId`, `lastFocusedAt`
    /// updated, no new record, `unarchivedCliSessionId` not set (VERIFIED for
    /// 5.1231.2, native N5). Only another app version may still land on Code
    /// home (`claude_resume_link_unarchives` false). The process starts
    /// lazily (none within 14 s, N5) unless selecting a session starts it at
    /// once (`claude_continue_starts_process`, UNVERIFIED).
    fn unarchive_and_show(&mut self, local: &str) {
        let archived = self.claude.desktop.get(local).is_some_and(|d| d.archived);
        let unarchives = self.claude_app_verified() || self.vars.claude_resume_link_unarchives;
        if archived && !unarchives {
            self.show(App::ClaudeDesktop, None);
            return;
        }
        if let Some(d) = self.claude.desktop.get_mut(local) {
            d.archived = false;
        }
        self.focus_claude_record(local);
        if self.vars.claude_continue_starts_process {
            self.start_desktop_child(local);
        }
    }

    /// `adoptCliSession` (Claude facts B): stem or live-handle owner →
    /// unarchive it; otherwise import a NEW desktop session from the
    /// transcript.
    fn claude_resume_link(&mut self, id: &str) {
        if !app_uuid_ok(id) {
            self.show(App::ClaudeDesktop, None);
            return;
        }
        let stem = format!("local_{id}");
        let owner = self
            .claude
            .desktop
            .values()
            .find(|d| {
                d.local == stem
                    || d.current.as_deref() == Some(id)
                    || d.unarchived.as_deref() == Some(id)
            })
            .map(|d| d.local.clone());
        if let Some(local) = owner {
            self.unarchive_and_show(&local);
            return;
        }
        let prior_owner = self
            .claude
            .desktop
            .values()
            .find(|d| d.prior.iter().any(|p| p == id))
            .map(|d| d.local.clone());
        if let (Some(local), false) = (
            &prior_owner,
            self.vars.claude_resume_prior_imports_duplicate,
        ) {
            self.unarchive_and_show(local);
            return;
        }
        let Some(transcript) = self.claude.transcripts.get(id).cloned() else {
            self.show(App::ClaudeDesktop, None);
            return;
        };
        self.claude.desktop.insert(
            stem.clone(),
            DesktopSession {
                local: stem.clone(),
                current: Some(id.to_owned()),
                unarchived: None,
                prior: Vec::new(),
                archived: false,
                cwd: transcript.cwd,
                last_focused: None,
            },
        );
        self.log_created(SessionRef::new(Provider::Claude, id));
        self.focus_claude_record(&stem);
        self.start_desktop_child(&stem);
    }

    fn codex_link(&mut self, id: &str) {
        let Some(thread) = self.codex.threads.get(id).cloned() else {
            self.show(App::CodexDesktop, None);
            return;
        };
        if thread.archived {
            let view = self
                .vars
                .codex_link_archived_shows_thread
                .then(|| id.to_owned());
            self.show(App::CodexDesktop, view);
            return;
        }
        self.show(App::CodexDesktop, Some(id.to_owned()));
        if self.codex.app_loaded.contains(id) {
            return;
        }
        let session = SessionRef::new(Provider::Codex, id);
        if self.writers(&session) > 0 && self.vars.codex_second_writer_refused {
            return;
        }
        self.codex.app_loaded.insert(id.to_owned());
        self.note_writer(&session, self.engine_open, None, true);
    }
}
