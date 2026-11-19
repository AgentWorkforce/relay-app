//! The workspace operator's upload policy (`history::operator_policy`):
//! read at sign-in, at a workspace refresh and every
//! `operator_policy::REFRESH_EVERY` while signed in, and applied to the
//! attached install.
use super::*;
use crate::contract::account::SharingMode;
use crate::history::operator_policy::{OperatorPolicy, ENVIRONMENT_KEY, SHARING_MODE};

/// The account's operator policy, which the uploader's gates also use.
pub(super) fn for_uploader(
    env: &std::collections::BTreeMap<String, String>,
    app_data_dir: &std::path::Path,
    uploader: &Uploader,
) -> Arc<OperatorPolicy> {
    let policy = Arc::new(OperatorPolicy::new(
        app_data_dir,
        env.get(ENVIRONMENT_KEY).cloned(),
    ));
    uploader.set_operator_policy(policy.clone());
    policy
}

/// What applying the policy did: whether it changed the install, and the
/// last change that failed.
#[derive(Default)]
struct Applied {
    changed: bool,
    failure: Option<String>,
}

impl Applied {
    /// Records one change; answers whether it was made.
    fn record(&mut self, result: Result<Value, RpcError>) -> bool {
        match result {
            Ok(_) => {
                self.changed = true;
                true
            }
            Err(error) => {
                self.failure = Some(error.message);
                false
            }
        }
    }
}

impl Account {
    /// Before a sign-in sets up `target`'s workspace: reads its policy,
    /// which a workspace with nothing cached waits for, and answers the
    /// setup's arguments with the sharing mode the operator enforces. With
    /// the policy unknown, the setup withholds uploads until a read
    /// succeeds.
    pub(super) async fn operator_policy_install(
        &self,
        target: &Target,
        request: &SignInRequest,
    ) -> (Vec<String>, Option<SharingMode>) {
        let workspace = target.workspace_id.as_deref().unwrap_or_default();
        let policy = &self.operator_policy;
        policy.sign_in(workspace);
        self.take_enrollment_seed(workspace);
        self.refresh_operator_policy(workspace).await;
        let enforced = policy.enforced_sharing(Some(workspace));
        let mut install = crate::history::uploader::command::cloud_install(
            target,
            enforced.unwrap_or(request.sharing),
            request.acknowledge_uninspected_schedules,
        );
        if policy.withholds(Some(workspace)) {
            install.push("--withhold-uploads".into());
        }
        // No sign-in surface lets the person pick a mode, so the requested
        // one is the app's default; a fresh install records it as unchosen.
        if enforced.is_none() {
            install.push("--sharing-is-app-default".into());
        }
        (install, enforced)
    }

    /// One setup event: `withheld` records that the setup paused uploads
    /// for the policy, so the first successful read resumes them.
    pub(super) fn operator_policy_event(&self, workspace: &str, event: &Value) {
        if event["event"] == "withheld" {
            self.operator_policy.set_withheld(workspace, true);
        }
    }

    /// After a sign-in attaches `found`: when the setup asked for the
    /// enforced mode in place of the person's requested one, and the
    /// install kept no other saved mode, remembers `requested` so lifting
    /// the policy gives it back. Then applies the policy.
    pub(super) async fn enforce_at_sign_in(
        &self,
        found: &Install,
        request: &SignInRequest,
        enforced: Option<SharingMode>,
    ) {
        let requested = request.sharing;
        if let (Some(mode), Some(workspace)) = (enforced, found.target.workspace_id.as_deref()) {
            if mode != requested && found.sharing_mode.is_none_or(|saved| saved == mode) {
                self.operator_policy
                    .remember(workspace, SHARING_MODE, json!(requested));
            }
        }
        self.enforce_operator_policy().await;
    }

    /// Reads the attached workspace's policy when it is due, then applies it.
    pub(super) async fn operator_policy_tick(&self) {
        if self.busy() {
            return;
        }
        let Some(workspace) = self.uploader.target().and_then(|t| t.workspace_id) else {
            // Signed out: a settings policy the core still holds (a lift at
            // sign-out that failed) is lifted now.
            self.apply_workspace_settings();
            return;
        };
        // The periodic read never renews the session itself: the uploads'
        // renewal backs off while Cloud refuses it, and this read waits
        // for a fresh session instead.
        let fresh = self.env_token.is_some()
            || self.load_session().is_some_and(|session| {
                access_is_fresh(
                    &session,
                    FRESH_MARGIN,
                    agent_relay_node::relay::state::now_ms(),
                )
            });
        // A newly applied revision is read again at once, so Cloud hears it;
        // while the session is not fresh it waits for the next fresh tick.
        if fresh {
            let report = self.operator_policy.workspace_policies().take_report_due();
            if report || self.operator_policy.due(&workspace) {
                self.refresh_operator_policy(&workspace).await;
            }
        }
        self.enforce_operator_policy().await;
    }

    async fn refresh_operator_policy(&self, workspace: &str) {
        if let Some(core) = self.core() {
            self.operator_policy
                .refresh(core.plugins.cloud.as_ref(), workspace)
                .await;
        }
    }

    /// The organization's workspace policy (v2) for `workspace_id`, as last
    /// read with the capture policy; unset when none was read.
    pub fn workspace_policy(&self, workspace_id: &str) -> crate::workspace_policy::WorkspacePolicy {
        self.operator_policy.workspace_policy(workspace_id)
    }

    /// Hands the core the attached workspace's settings policy (spawning,
    /// joining, delivery, analytics) when it changed: the core locks and
    /// defaults its settings by it (`Core::apply_settings_policy`). Nothing is
    /// handed over before the workspace's policy was first read, so a lock the
    /// core keeps is never lifted while offline; a read that found no policy
    /// lifts it, and so does signing out: a person who is signed out has no
    /// organization.
    pub(super) fn apply_workspace_settings(&self) {
        let Some(core) = self.core() else {
            return;
        };
        let cache = self.operator_policy.workspace_policies();
        // With the revision it comes from, which Cloud is told once applied.
        let (wanted, revision) = match self.uploader.target().and_then(|t| t.workspace_id) {
            Some(workspace) => {
                let Some(entry) = cache.entry(&workspace) else {
                    return;
                };
                let policy = crate::workspace_policy::settings::settings_policy(&entry.policy);
                let wanted = crate::workspace_policy::settings::applies(&policy).then_some(policy);
                (wanted, Some((workspace, entry.revision)))
            }
            None if self.env_token.is_none() && self.load_session().is_none() => (None, None),
            // Signed in with no workspace attached yet: nothing changes.
            None => return,
        };
        if !cache.settings_changed(&wanted) {
            return cache.revision_applied(revision);
        }
        match core.apply_settings_policy(wanted.clone()) {
            Ok(()) => cache.settings_applied(wanted, revision),
            // Tried again on the next supervision tick, signed in or out; the
            // warning shows once per failure, not on every tick.
            Err(error) => {
                if cache.settings_failure_is_new(&wanted, &error.message) {
                    core.hub.activity(
                        Level::Warning,
                        format!(
                            "Could not apply your organization's settings policy: {}",
                            error.message
                        ),
                    );
                }
            }
        }
    }

    /// A workspace refresh reads the attached workspace's policy again.
    pub(super) async fn refresh_attached_operator_policy(&self) {
        if let Some(workspace) = self.uploader.target().and_then(|t| t.workspace_id) {
            self.refresh_operator_policy(&workspace).await;
            self.enforce_operator_policy().await;
        }
    }

    /// Applies the policy to the attached install: an enforced sharing mode
    /// replaces the person's, which is kept and given back when the operator
    /// lifts it; uploads withheld at sign-in resume once the policy is read,
    /// and uploads the operator runs resume from the person's earlier pause.
    pub(super) async fn enforce_operator_policy(&self) {
        self.apply_workspace_settings();
        let Some(install) = self.uploader.install() else {
            return;
        };
        let Some(workspace) = install.target.workspace_id.clone() else {
            return;
        };
        if self.operator_policy.withholds(Some(&workspace)) {
            return;
        }
        let mut applied = Applied::default();
        self.resume_withheld_uploads(&workspace, &mut applied).await;
        self.resume_operator_uploads(&workspace, &mut applied).await;
        self.apply_operator_sharing(&workspace, &install, &mut applied)
            .await;
        if let Some(core) = self.core() {
            if applied.changed {
                core.hub.invalidated(Listing::Uploader);
            }
            // Tried again on the next supervision tick.
            if let Some(message) = applied.failure {
                core.hub.activity(Level::Warning, message);
            }
        }
    }

    /// Resumes the uploads the setup paused while the policy was unknown;
    /// a pause the person made since stays.
    async fn resume_withheld_uploads(&self, workspace: &str, applied: &mut Applied) {
        let policy = &self.operator_policy;
        if policy.entry(workspace).is_some_and(|entry| entry.withheld)
            && applied.record(self.uploader.pause_unchecked(false).await)
        {
            policy.set_withheld(workspace, false);
        }
    }

    /// Resumes uploads the person paused once the operator runs them.
    async fn resume_operator_uploads(&self, workspace: &str, applied: &mut Applied) {
        if !self.operator_policy.controls_uploads(Some(workspace)) {
            return;
        }
        let paused = self
            .uploader
            .cached_status()
            .await
            .is_ok_and(|status| status["paused"] == Value::Bool(true));
        if paused {
            applied.record(self.uploader.pause_unchecked(false).await);
        }
    }

    /// Switches the install to the enforced sharing mode, remembering its
    /// current one, or gives the remembered mode back once none is enforced.
    /// An install still on the app's default is set even to the mode it
    /// already has, so it counts as chosen and no suggestion returns once
    /// the lock is lifted.
    async fn apply_operator_sharing(
        &self,
        workspace: &str,
        install: &Install,
        applied: &mut Applied,
    ) {
        let policy = &self.operator_policy;
        let current = install.sharing_mode;
        match policy.enforced_sharing(Some(workspace)) {
            Some(mode) if current != Some(mode) || install.sharing_unchosen => {
                if let Some(current) = current.filter(|current| *current != mode) {
                    policy.remember(workspace, SHARING_MODE, json!(current));
                }
                applied.record(self.uploader.sharing_set_unchecked(mode).await);
            }
            Some(_) => {}
            None => self.give_back_sharing(workspace, current, applied).await,
        }
    }

    /// Gives back the person's remembered sharing mode, then forgets it.
    async fn give_back_sharing(
        &self,
        workspace: &str,
        current: Option<SharingMode>,
        applied: &mut Applied,
    ) {
        let policy = &self.operator_policy;
        let Some(own) = policy
            .remembered(workspace, SHARING_MODE)
            .and_then(|value| serde_json::from_value::<SharingMode>(value).ok())
        else {
            return;
        };
        if current == Some(own) || applied.record(self.uploader.sharing_set_unchecked(own).await) {
            policy.forget(workspace, SHARING_MODE);
        }
    }
}
