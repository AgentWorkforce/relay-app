//! The account's background watchers, started once the daemon runs: relay
//! uploads and their snapshots, session locations, service callers, session
//! naming and turn-end coordination advisories.

use super::Account;
use agent_relay_node::relay::daemon::Core;
use std::sync::Arc;

impl Account {
    pub(super) fn spawn_watchers(&self, core: Arc<Core>) {
        tokio::spawn(crate::history::relay_uploads::watch(
            core.clone(),
            self.uploader.clone(),
            self.relay_uploads.clone(),
        ));
        tokio::spawn(crate::history::relay_uploads::refresh_snapshots(
            core.clone(),
            self.uploader.clone(),
        ));
        if let Some(locations) = self.session_locations.clone() {
            tokio::spawn(crate::session_locations::watch(core.clone(), locations));
        }
        self.service_callers.watch();
        if let Some(advisories) = self.coordination_advisories.get().cloned() {
            tokio::spawn(crate::coordination_advisories::watch(core.clone(), advisories));
        }
        tokio::spawn(crate::history::session_naming::watch(
            core,
            self.uploader.clone(),
        ));
    }
}
