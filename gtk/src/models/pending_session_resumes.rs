use agent_relay_desktop_core::session_locations::{SessionOpenAction, SessionOpenPlan};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

pub const PENDING_RESUME_MESSAGE: &str = "A resume request was already sent. The provider has not confirmed it opened. Check the earlier launch before trying again.";

/// Terminal dispatch does not prove provider ownership. Persist its reservation
/// before dispatch so a desktop restart cannot authorize a duplicate launch.
pub struct PendingSessionResumes {
    path: Option<PathBuf>,
    sessions: BTreeSet<String>,
    uncertain: BTreeSet<String>,
    error: Option<String>,
}

impl Default for PendingSessionResumes {
    fn default() -> Self {
        Self {
            path: None,
            sessions: BTreeSet::new(),
            uncertain: BTreeSet::new(),
            error: Some(
                "Resume protection has not loaded. Restart Agent Relay before resuming a session."
                    .into(),
            ),
        }
    }
}

impl PendingSessionResumes {
    /// Use the app's preference directory, including an isolated directory in
    /// tests. This file is separate from preferences, whose loader is lenient.
    pub fn load(preferences_path: &Path) -> Self {
        let path = preferences_path.with_file_name("pending-session-resumes.json");
        let mut pending = Self {
            path: Some(path.clone()),
            sessions: BTreeSet::new(),
            uncertain: BTreeSet::new(),
            error: None,
        };
        match read(&path) {
            Ok(sessions) => pending.sessions = sessions,
            Err(error) => {
                pending.record_error(error);
            }
        }
        pending
    }

    pub fn contains(&self, key: &str) -> bool {
        self.sessions.contains(key) || self.uncertain.contains(key)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Only a fresh planner response can acknowledge provider ownership.
    pub fn observe(&mut self, key: &str, plan: &SessionOpenPlan) -> Result<(), String> {
        let owned = match plan.action {
            SessionOpenAction::Focus
            | SessionOpenAction::ShowApplication
            | SessionOpenAction::OpenLink => plan.location.is_some(),
            SessionOpenAction::Choose => !plan.choices.is_empty(),
            _ => false,
        };
        if owned && self.contains(key) {
            self.update(key, false)?;
        }
        Ok(())
    }

    /// Never clear on native dispatch success or failure: neither proves that
    /// a partially launched provider no longer exists. Errors forbid dispatch.
    pub fn begin(&mut self, key: &str, plan: &SessionOpenPlan) -> Result<bool, String> {
        if plan.action != SessionOpenAction::Resume {
            return Ok(true);
        }
        self.update(key, true)
    }

    pub fn resolve_closed_launch(&mut self, key: &str, opening: bool) -> Result<bool, String> {
        if opening || !self.contains(key) {
            return Ok(false);
        }
        self.update(key, false)
    }

    fn update(&mut self, key: &str, insert: bool) -> Result<bool, String> {
        let Some(path) = self.path.clone() else {
            return Err(self.error.clone().unwrap_or_default());
        };
        let result = self.store_update(&path, key, insert, save);
        match result {
            Ok(changed) => {
                self.error = None;
                Ok(changed)
            }
            Err(error) => Err(self.record_error(error)),
        }
    }

    fn store_update(
        &mut self,
        path: &Path,
        key: &str,
        insert: bool,
        persist: impl FnOnce(&Path, &BTreeSet<String>) -> io::Result<()>,
    ) -> io::Result<bool> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("Missing resume protection directory"))?;
        agent_relay_node::platform::fs::create_private_dir_all(directory)?;
        // The desktop is single-instance. A stable sidecar lock also serializes
        // read/modify/replace across guards and processes; lock the sidecar,
        // never the data file that atomic replacement changes underneath us.
        let lock = agent_relay_node::platform::fs::private(&mut OpenOptions::new())
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.lock()?;
        // Cached reservations can have been cleared by another guard. Only the
        // locked file is authoritative for writes; never merge the cache back.
        let mut sessions = read(path)?;
        let changed = if insert {
            !self.uncertain.contains(key) && sessions.insert(key.into())
        } else {
            let removed = sessions.remove(key);
            removed || self.uncertain.contains(key)
        };
        if changed {
            if let Err(error) = persist(path, &sessions) {
                // Atomic replacement may succeed before directory sync fails.
                // Block this guard's retry until explicit confirmation or fresh
                // ownership clears it, without restoring it in unrelated writes.
                self.uncertain.insert(key.into());
                return Err(error);
            }
            self.uncertain.remove(key);
        }
        self.sessions = sessions;
        Ok(changed)
    }

    fn record_error(&mut self, error: io::Error) -> String {
        let path = self
            .path
            .as_deref()
            .unwrap_or_else(|| Path::new("resume protection"));
        let message = format!("Couldn’t read or save pending session launches. Resume is blocked. Check access to {}, then restart Agent Relay. If the file is damaged, close or stop all earlier launches before restoring it. {error}", path.display());
        self.error = Some(message.clone());
        message
    }
}

fn read(path: &Path) -> io::Result<BTreeSet<String>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(error) => Err(error),
    }
}

fn save(path: &Path, sessions: &BTreeSet<String>) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(sessions).map_err(io::Error::other)?;
    // Existing durable storage convention: private temp file, fsync, atomic
    // replacement, then directory fsync on Unix; Windows uses atomic persist.
    agent_relay_node::platform::fs::write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_desktop_core::session_locations::SessionLocation;

    fn plan(action: SessionOpenAction) -> SessionOpenPlan {
        SessionOpenPlan {
            action,
            location: None,
            url: None,
            command: None,
            choices: vec![],
            reason: None,
            can_resume_default: false,
        }
    }

    #[test]
    fn slow_provider_cannot_receive_a_second_resume_after_terminal_dispatch_completes() {
        let directory = tempfile::tempdir().unwrap();
        let mut pending = PendingSessionResumes::load(&directory.path().join("desktop.json"));
        let stopped = plan(SessionOpenAction::Resume);
        let mut dispatches = 0;
        for _ in 0..20 {
            // Startup still reports no provider owner, even after the native
            // terminal callback succeeds or returns an ambiguous failure.
            pending.observe("claude:slow-shell", &stopped).unwrap();
            if pending.begin("claude:slow-shell", &stopped).unwrap() {
                dispatches += 1;
            }
        }
        assert_eq!(dispatches, 1);
        assert!(pending.contains("claude:slow-shell"));
        assert!(pending.begin("claude:another-session", &stopped).unwrap());
    }

    #[test]
    fn fresh_owner_acknowledges_pending_launch() {
        for action in [
            SessionOpenAction::Focus,
            SessionOpenAction::ShowApplication,
            SessionOpenAction::OpenLink,
            SessionOpenAction::Choose,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut pending = PendingSessionResumes::load(&directory.path().join("desktop.json"));
            pending
                .begin("session", &plan(SessionOpenAction::Resume))
                .unwrap();
            let mut owned = plan(action.clone());
            if action == SessionOpenAction::Choose {
                owned.choices.push(SessionLocation::default());
            } else {
                owned.location = Some(SessionLocation::default());
            }
            pending.observe("session", &owned).unwrap();
            assert!(!pending.contains("session"), "{action:?}");
        }
    }

    #[test]
    fn missing_owner_evidence_never_acknowledges_pending_launch() {
        let directory = tempfile::tempdir().unwrap();
        let mut pending = PendingSessionResumes::load(&directory.path().join("desktop.json"));
        pending
            .begin("session", &plan(SessionOpenAction::Resume))
            .unwrap();
        for action in [
            SessionOpenAction::Focus,
            SessionOpenAction::ShowApplication,
            SessionOpenAction::OpenLink,
            SessionOpenAction::Choose,
            SessionOpenAction::Unavailable,
            SessionOpenAction::Resume,
        ] {
            let mut unowned = plan(action);
            unowned.can_resume_default = true;
            pending.observe("session", &unowned).unwrap();
            assert!(pending.contains("session"));
        }
    }

    #[test]
    fn explicit_closed_launch_confirmation_allows_a_fresh_resume() {
        let directory = tempfile::tempdir().unwrap();
        let mut pending = PendingSessionResumes::load(&directory.path().join("desktop.json"));
        let stopped = plan(SessionOpenAction::Resume);
        pending.begin("session", &stopped).unwrap();
        assert!(!pending.resolve_closed_launch("session", true).unwrap());
        assert!(pending.contains("session"));
        assert!(pending.resolve_closed_launch("session", false).unwrap());
        assert!(pending.begin("session", &stopped).unwrap());
        assert!(pending.contains("session"));
    }

    #[test]
    fn desktop_restart_during_slow_shell_start_keeps_the_same_session_reserved() {
        let directory = tempfile::tempdir().unwrap();
        let preferences = directory.path().join("desktop.json");
        let stopped = plan(SessionOpenAction::Resume);
        let mut dispatches = 0;
        for _ in 0..3 {
            let mut fresh_app = PendingSessionResumes::load(&preferences);
            fresh_app.observe("claude:slow-shell", &stopped).unwrap();
            if fresh_app.begin("claude:slow-shell", &stopped).unwrap() {
                // The terminal has opened, but shell startup has not reported
                // provider ownership before this app instance exits.
                dispatches += 1;
            }
        }
        assert_eq!(dispatches, 1);
        let mut restarted = PendingSessionResumes::load(&preferences);
        assert!(restarted
            .resolve_closed_launch("claude:slow-shell", false)
            .unwrap());
        let mut after_resolution = PendingSessionResumes::load(&preferences);
        assert!(!after_resolution.contains("claude:slow-shell"));
        assert!(after_resolution
            .begin("claude:slow-shell", &stopped)
            .unwrap());
    }

    #[test]
    fn fresh_owner_clear_survives_another_restart() {
        let directory = tempfile::tempdir().unwrap();
        let preferences = directory.path().join("desktop.json");
        let mut first = PendingSessionResumes::load(&preferences);
        first
            .begin("session", &plan(SessionOpenAction::Resume))
            .unwrap();
        let mut restarted = PendingSessionResumes::load(&preferences);
        let mut owned = plan(SessionOpenAction::Focus);
        owned.location = Some(SessionLocation::default());
        restarted.observe("session", &owned).unwrap();
        assert!(!PendingSessionResumes::load(&preferences).contains("session"));
    }

    #[test]
    fn corrupt_or_unreadable_store_fails_closed_without_overwriting_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let preferences = directory.path().join("desktop.json");
        let path = preferences.with_file_name("pending-session-resumes.json");
        for corrupt in ["{", "null", "{}", "[42]"] {
            fs::write(&path, corrupt).unwrap();
            let mut pending = PendingSessionResumes::load(&preferences);
            assert!(pending.error().unwrap().contains(path.to_str().unwrap()));
            assert!(pending
                .begin("session", &plan(SessionOpenAction::Resume))
                .is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);
            assert!(pending
                .begin("session", &plan(SessionOpenAction::Focus))
                .unwrap());
        }
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let mut pending = PendingSessionResumes::load(&preferences);
        assert!(pending
            .begin("session", &plan(SessionOpenAction::Resume))
            .is_err());
        assert!(path.is_dir());
    }

    #[test]
    fn storage_failure_prevents_dispatch_and_failed_resolution_retains_reservation() {
        let directory = tempfile::tempdir().unwrap();
        let preferences = directory.path().join("desktop.json");
        let path = preferences.with_file_name("pending-session-resumes.json");
        let lock = path.with_extension("lock");
        let mut pending = PendingSessionResumes::load(&preferences);
        fs::create_dir(&lock).unwrap();
        assert!(pending
            .begin("session", &plan(SessionOpenAction::Resume))
            .is_err());
        assert!(!path.exists());
        fs::remove_dir(&lock).unwrap();
        assert!(pending
            .begin("session", &plan(SessionOpenAction::Resume))
            .unwrap());
        let mut restarted = PendingSessionResumes::load(&preferences);
        let saved = fs::read(&path).unwrap();
        fs::remove_file(&lock).unwrap();
        fs::create_dir(&lock).unwrap();
        assert!(restarted.resolve_closed_launch("session", false).is_err());
        assert!(restarted.contains("session"));
        assert_eq!(fs::read(&path).unwrap(), saved);
        assert!(PendingSessionResumes::load(&preferences).contains("session"));
        fs::remove_dir(&lock).unwrap();
        assert!(restarted.resolve_closed_launch("session", false).unwrap());
        assert!(!PendingSessionResumes::load(&preferences).contains("session"));
    }

    #[test]
    fn multiple_guards_serialize_reservations_without_losing_other_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let preferences = directory.path().join("desktop.json");
        let guards: Vec<_> = (0..8)
            .map(|_| {
                let preferences = preferences.clone();
                std::thread::spawn(move || {
                    let mut pending = PendingSessionResumes::load(&preferences);
                    pending
                        .begin("same-session", &plan(SessionOpenAction::Resume))
                        .unwrap()
                })
            })
            .collect();
        let dispatches = guards
            .into_iter()
            .map(|guard| usize::from(guard.join().unwrap()))
            .sum::<usize>();
        assert_eq!(dispatches, 1);
        let mut stale = PendingSessionResumes::load(&preferences);
        let mut other = PendingSessionResumes::load(&preferences);
        assert!(other
            .begin("another-session", &plan(SessionOpenAction::Resume))
            .unwrap());
        assert!(stale.resolve_closed_launch("same-session", false).unwrap());
        let reloaded = PendingSessionResumes::load(&preferences);
        assert!(!reloaded.contains("same-session"));
        assert!(reloaded.contains("another-session"));
    }

    fn stale_guard_after_clear(owner_observed: bool, insert: bool) {
        let directory = tempfile::tempdir().unwrap();
        let preferences = directory.path().join("desktop.json");
        let stopped = plan(SessionOpenAction::Resume);
        let mut clearing = PendingSessionResumes::load(&preferences);
        assert!(clearing.begin("cleared", &stopped).unwrap());
        assert!(clearing.begin("removing", &stopped).unwrap());
        let mut stale = PendingSessionResumes::load(&preferences);
        if owner_observed {
            let mut owned = plan(SessionOpenAction::Focus);
            owned.location = Some(SessionLocation::default());
            clearing.observe("cleared", &owned).unwrap();
        } else {
            assert!(clearing.resolve_closed_launch("cleared", false).unwrap());
        }
        assert!(clearing.begin("newer", &stopped).unwrap());
        if insert {
            assert!(stale.begin("unrelated", &stopped).unwrap());
        } else {
            assert!(stale.resolve_closed_launch("removing", false).unwrap());
        }
        let reloaded = PendingSessionResumes::load(&preferences);
        assert!(!reloaded.contains("cleared"));
        assert!(reloaded.contains("newer"));
        assert_eq!(reloaded.contains("unrelated"), insert);
        assert_eq!(reloaded.contains("removing"), insert);
    }

    #[test]
    fn stale_guard_reservation_does_not_restore_confirmed_closed_launch() {
        stale_guard_after_clear(false, true);
    }

    #[test]
    fn stale_guard_clear_does_not_restore_confirmed_closed_launch() {
        stale_guard_after_clear(false, false);
    }

    #[test]
    fn stale_guard_reservation_does_not_restore_observed_owner_launch() {
        stale_guard_after_clear(true, true);
    }

    #[test]
    fn stale_guard_clear_does_not_restore_observed_owner_launch() {
        stale_guard_after_clear(true, false);
    }

    #[test]
    fn uncertain_reservation_does_not_restore_another_guards_clear() {
        let directory = tempfile::tempdir().unwrap();
        let preferences = directory.path().join("desktop.json");
        let path = preferences.with_file_name("pending-session-resumes.json");
        let stopped = plan(SessionOpenAction::Resume);
        let mut stale = PendingSessionResumes::load(&preferences);
        assert!(stale
            .store_update(&path, "cleared", true, |path, sessions| {
                save(path, sessions)?;
                Err(io::Error::other("injected directory sync failure"))
            })
            .is_err());
        let mut clearing = PendingSessionResumes::load(&preferences);
        assert!(clearing.resolve_closed_launch("cleared", false).unwrap());
        assert!(stale.begin("unrelated", &stopped).unwrap());
        assert!(stale.contains("cleared"));
        assert!(!stale.begin("cleared", &stopped).unwrap());
        let reloaded = PendingSessionResumes::load(&preferences);
        assert!(!reloaded.contains("cleared"));
        assert!(reloaded.contains("unrelated"));
        assert!(stale.resolve_closed_launch("cleared", false).unwrap());
        assert!(stale.begin("cleared", &stopped).unwrap());
    }

    #[test]
    fn failed_write_uncertainty_blocks_retry_across_unrelated_updates() {
        for insert in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let preferences = directory.path().join("desktop.json");
            let path = preferences.with_file_name("pending-session-resumes.json");
            let stopped = plan(SessionOpenAction::Resume);
            let mut pending = PendingSessionResumes::load(&preferences);
            if !insert {
                assert!(pending.begin("session", &stopped).unwrap());
            }
            assert!(pending
                .store_update(&path, "session", insert, |path, sessions| {
                    if !insert {
                        save(path, sessions)?;
                    }
                    Err(io::Error::other("injected write failure"))
                })
                .is_err());
            assert!(pending.begin("unrelated", &stopped).unwrap());
            assert!(pending.contains("session"));
            assert!(!pending.begin("session", &stopped).unwrap());
            assert!(!PendingSessionResumes::load(&preferences).contains("session"));
            assert!(pending.resolve_closed_launch("session", false).unwrap());
            assert!(pending.begin("session", &stopped).unwrap());
        }
    }

    #[test]
    fn failed_atomic_write_denies_reservation_and_failed_clear_keeps_pending() {
        let directory = tempfile::tempdir().unwrap();
        let preferences = directory.path().join("desktop.json");
        let path = preferences.with_file_name("pending-session-resumes.json");
        let mut pending = PendingSessionResumes::load(&preferences);
        // Exercise the write-error branch deterministically, including on
        // privileged test runners where read-only permissions do not fail.
        let failed_write =
            |_: &Path, _: &BTreeSet<String>| Err(io::Error::other("injected atomic write failure"));
        assert!(pending
            .store_update(&path, "session", true, failed_write)
            .is_err());
        assert!(pending.contains("session"));
        assert!(!path.exists());
        assert!(pending.resolve_closed_launch("session", false).unwrap());
        assert!(pending
            .begin("session", &plan(SessionOpenAction::Resume))
            .unwrap());
        let saved = fs::read(&path).unwrap();
        assert!(pending
            .store_update(&path, "session", false, failed_write)
            .is_err());
        assert!(pending.contains("session"));
        assert_eq!(fs::read(&path).unwrap(), saved);
        assert!(PendingSessionResumes::load(&preferences).contains("session"));
    }
}
