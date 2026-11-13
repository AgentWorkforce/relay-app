//! Authenticated account refresh and opt-in Cloud profile synchronization.

use super::*;
use agent_relay_node::relay::contract::plan::WorkspacePlan;

impl Account {
    /// Refresh account display metadata independently of Relaycast. Bind the
    /// response to the attached person and workspace; cache it for five minutes.
    pub(super) async fn refresh_workspace_name(&self) {
        if self.busy() {
            return;
        }
        let Some(target) = self.uploader.target() else {
            return;
        };
        let now = tokio::time::Instant::now();
        {
            let mut retry = self.workspace_name_retry.lock().unwrap();
            if retry.not_before.is_some_and(|at| now < at) {
                return;
            }
            retry.failures = retry.failures.saturating_add(1);
            retry.not_before = Some(now + Duration::from_secs(5 * (1 << retry.failures.min(6))));
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        // Display metadata must not renew credentials or wake upload sign-in.
        let bearer = if let Some(token) = &self.env_token {
            Some(token.clone())
        } else if let Some(session) = self
            .load_session()
            .filter(|s| s.site_url == target.site_url)
        {
            access_is_fresh(&session, Duration::ZERO, agent_relay_node::relay::state::now_ms())
                .then_some(session.access_token)
        } else {
            None
        };
        let Some(bearer) = bearer else {
            return;
        };
        let analytics_enabled = self
            .telemetry
            .as_ref()
            .is_some_and(|t| t.profile_sync_enabled());
        let Ok(identity) =
            cloud_session::whoami_with_analytics(&target.site_url, &bearer, analytics_enabled)
                .await
        else {
            return;
        };
        if target.account_id.as_deref() != Some(identity.user_id.as_str())
            || target.workspace_id != identity.current_workspace_id
            || self.epoch.load(Ordering::SeqCst) != epoch
        {
            return;
        }
        let name = identity
            .current_workspace_name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        {
            let mut retry = self.workspace_name_retry.lock().unwrap();
            retry.failures = 0;
            retry.not_before = Some(now + Duration::from_secs(300));
        }
        self.update(|account| {
            if self.epoch.load(Ordering::SeqCst) == epoch
                && account.site_url.as_deref() == Some(target.site_url.as_str())
                && account.workspace_id == target.workspace_id
                && account
                    .person
                    .as_ref()
                    .and_then(|person| person.id.as_ref())
                    == target.account_id.as_ref()
            {
                if let Some(name) = name {
                    account.workspace_name = Some(name);
                }
                account.workspace_count = identity.workspace_count;
                account.workspace_avatar_url = identity.current_workspace_avatar_url.clone();
            }
        });
        if let Some(core) = self.core() {
            if self.epoch.load(Ordering::SeqCst) == epoch {
                core.record_workspace_plan(identity.current_workspace_plan)
                    .await;
            }
        }
    }

    /// Publishes the plan a sign-in's whoami read, once its workspace is
    /// adopted; the usage counts follow with the next refresh.
    pub(super) fn publish_workspace_plan(&self, plan: Option<WorkspacePlan>) {
        if let Some(core) = self.core() {
            core.hub.update(|status| status.workspace.plan = plan);
        }
    }

    /// Drops what the core kept for the account that signs out: Skip's
    /// conversation, messaging, team access, and the plan (the next
    /// sign-in reads its own).
    pub(super) async fn forget_account_in_core(&self) {
        let Some(core) = self.core() else {
            return;
        };
        if let Some(account) = self.account() {
            crate::cloud_chat::forget(&core.config().app_data_dir, &account);
            self.service_callers.callers.forget(&account).await;
        }
        *core
            .messaging
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Default::default();
        self.contractors(&core).reset().await;
        core.hub.invalidated(Listing::People);
        core.hub.update(|status| {
            status.workspace.plan = None;
            status.workspace.usage = None;
        });
    }
}
