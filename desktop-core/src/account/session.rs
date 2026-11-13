//! The kept session as the store holds it. Refreshing it, under the
//! session refresh locks, is in `session_refresh`.
use super::Account;
use agent_relay_node::relay::store::CloudSession;

impl Account {
    pub fn site_url(&self) -> &str {
        &self.site_url
    }

    /// Keeps a session the person just approved (a sign-in or a workspace
    /// connection) and wakes supervision.
    pub(super) fn keep_session(&self, session: &CloudSession) -> Result<(), String> {
        self.save_session(Some(session))?;
        self.sync_needs_sign_in();
        self.update(|account| {
            account.auth_health = None;
            account.auth_error = None;
            account.auth_checked_at = None;
        });
        self.wake.notify_one();
        Ok(())
    }

    pub(super) fn load_session(&self) -> Option<CloudSession> {
        self.core()?.store.read(|s| s.cloud_session.clone())
    }

    pub(super) fn save_session(&self, session: Option<&CloudSession>) -> Result<(), String> {
        let core = self.core().ok_or("The core is stopping.")?;
        core.store
            .update_latest(|s| s.cloud_session = session.cloned())
            .map_err(|error| error.to_string())
    }
}

impl Account {
    /// The shared account, lending its session to its telemetry (a site
    /// build's own collector, `crate::telemetry::SessionBearer`).
    pub(super) fn shared(self) -> std::sync::Arc<Account> {
        let account = std::sync::Arc::new(self);
        if let Some(telemetry) = &account.telemetry {
            let bearer = std::sync::Arc::downgrade(&account);
            let _ = telemetry
                .session()
                .set(bearer as std::sync::Weak<dyn crate::telemetry::SessionBearer>);
        }
        account
    }
}

/// A site's own telemetry collector takes the kept session as its bearer:
/// only while signed in with an attached install on this site, only a
/// session still fresh (renewal stays with supervision) and never one the
/// environment's token conflicts with.
impl crate::telemetry::SessionBearer for Account {
    fn bearer(&self) -> Option<agent_relay_node::relay::workspace::Secret> {
        if self.status().state != agent_relay_node::relay::state::SignInState::SignedIn {
            return None;
        }
        let site = self.uploader.target()?.site_url;
        let session = self.load_session().filter(|s| s.site_url == site)?;
        let now = agent_relay_node::relay::state::now_ms();
        (!self.environment_conflicts_with(&session.access_token)
            && super::access_is_fresh(&session, super::FRESH_MARGIN, now))
        .then_some(session.access_token)
    }
}
