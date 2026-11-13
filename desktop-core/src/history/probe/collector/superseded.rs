//! When a running collector must stop by itself: its executable was replaced
//! on disk, or the saved sharing choice is no longer the one it started with.
//!
//! A collector keeps running the build that launched it and reads the
//! install's configuration once, when it starts. Replacement normally comes
//! from a new core (`supervision`), but nothing reaches a collector whose
//! app was swapped by hand while its old core keeps running, and a collector
//! of a newer build is never replaced at all (`claims::outdated`). Either one
//! would keep uploading under rules the person has since changed. So the
//! collector checks for itself and stops through its normal cooperative
//! path; whichever core supervises the install then starts the build now on
//! disk, which reads the current choice. When in doubt it keeps running: an
//! unreadable configuration (mid-rewrite) is not a change.

use super::super::bridge::mode;
use super::read_config;
use crate::contract::account::SharingMode;
use crate::executable_identity::StartedExecutable;
use std::path::Path;

/// Why a collector stops by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reason {
    ExecutableReplaced,
    SharingChanged,
}

impl Reason {
    pub(super) fn message(self) -> &'static str {
        match self {
            Reason::ExecutableReplaced => {
                "Agent Relay was updated on disk; stopping this collector so the new build replaces it."
            }
            Reason::SharingChanged => {
                "The saved sharing choice changed; stopping this collector so it restarts with the new choice."
            }
        }
    }
}

/// What the running collector started as.
pub(super) struct Started {
    executable: Option<StartedExecutable>,
    mode: SharingMode,
}

impl Started {
    pub(super) fn now(mode: SharingMode) -> Self {
        Self::with(StartedExecutable::current(), mode)
    }

    pub(super) fn with(executable: Option<StartedExecutable>, mode: SharingMode) -> Self {
        Self { executable, mode }
    }

    /// Why this collector must stop, if it must.
    pub(super) fn superseded(&self, directory: &Path) -> Option<Reason> {
        if self
            .executable
            .as_ref()
            .is_some_and(StartedExecutable::replaced)
        {
            return Some(Reason::ExecutableReplaced);
        }
        match read_config(directory) {
            Ok(config) if mode(&config) != self.mode => Some(Reason::SharingChanged),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::delivery_status::Config;
    use std::fs;

    fn config(mode: SharingMode) -> Config {
        Config {
            version: 1,
            site_url: "https://agentrelay.com".into(),
            account_id: "account".into(),
            account_email: None,
            account_name: None,
            account_avatar_url: None,
            workspace_name: None,
            org_id: "org".into(),
            workspace_id: "workspace".into(),
            history_url: "https://history.agentrelay.com".into(),
            cloud_history_origin: None,
            delivery_account: crate::history::destination::account_id("org", Some("workspace")),
            job_id: "job".into(),
            include_existing: mode == SharingMode::All,
            sharing_mode: Some(mode),
            acknowledge_uninspected_schedules: false,
        }
    }

    fn save(directory: &Path, config: &Config) {
        crate::history::probe::save_json(&directory.join("config.json"), config).unwrap();
        // The fixture must be one the collector itself would accept.
        read_config(directory).unwrap();
    }

    #[test]
    fn an_unchanged_collector_keeps_running() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &config(SharingMode::Selected));
        let started = Started::with(None, SharingMode::Selected);
        assert_eq!(started.superseded(dir.path()), None);
    }

    #[test]
    fn a_collector_stops_when_the_person_narrows_sharing() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &config(SharingMode::Selected));
        // Started under "all", for example by a build from before the
        // person chose "only sessions I choose".
        let started = Started::with(None, SharingMode::All);
        assert_eq!(started.superseded(dir.path()), Some(Reason::SharingChanged));
    }

    #[test]
    fn an_unreadable_configuration_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.json"), b"{ half written").unwrap();
        let started = Started::with(None, SharingMode::All);
        assert_eq!(started.superseded(dir.path()), None);
    }

    #[test]
    fn a_collector_whose_executable_was_replaced_stops() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &config(SharingMode::Selected));
        let helper = dir.path().join("helper");
        fs::write(&helper, b"old build").unwrap();
        let started = Started::with(Some(StartedExecutable::at(helper.clone())), SharingMode::Selected);
        fs::remove_file(&helper).unwrap();
        fs::write(&helper, b"new build, different").unwrap();
        assert_eq!(
            started.superseded(dir.path()),
            Some(Reason::ExecutableReplaced)
        );
    }
}
