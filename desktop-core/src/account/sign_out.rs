//! Sign-out and dependent provider credential revocation.
use super::*;
impl Account {
    // Sign-out.

    /// Signs out: forgets and revokes the Cloud session, stops every
    /// uploader command, and disconnects this computer's install.
    pub async fn sign_out(&self) -> Result<(), RpcError> {
        // Cancelling setup releases its credential lease before waiting for
        // refresh maintenance. Sign-out must not wait for a stuck child.
        self.prepare_sign_out();
        let _providers = self.provider_credentials.lock().await;
        self.revoke_provider_credentials().await;
        let (session, clear_queued, forgotten) = {
            // A refresh in flight finishes first, then sees the new epoch.
            let _one = self.refreshing.lock().await;
            self.epoch.fetch_add(1, Ordering::SeqCst);
            // The latest saved session: another process may have rotated it.
            let outcome = match self.core() {
                Some(core) => core
                    .store
                    .update_latest(|store| {
                        let queued = store.queue_desktop_identity_clear(true);
                        (store.cloud_session.take(), queued)
                    })
                    .map_err(|error| error.to_string()),
                None => self.save_session(None).map(|_| (None, false)),
            };
            match outcome {
                Ok((session, queued)) => (session, queued, Ok(())),
                Err(error) => (self.load_session(), false, Err(error)),
            }
        };
        // Including a setup's `cloud install`, which has no install yet.
        let _ = self.uploader.runner().cancel_all().await;
        if let Some(session) = session.filter(|_| !clear_queued) {
            tokio::spawn(async move { crate::cloud_api::revoke(&session).await });
        }
        self.forget_account_in_core().await;
        *self
            .identity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .workspace_name_retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Retry::default();
        if let Some(telemetry) = &self.telemetry {
            telemetry.reset_identity();
        }
        if let Some(target) = self.uploader.target() {
            *self
                .pending_disconnect
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(target);
        }
        self.uploader.attach(None);
        self.show_install(None);
        // Messaging's push channel follows the account.
        if let Some(core) = self.core() {
            core.sync_agents().await;
        }
        forgotten.map_err(|message| RpcError::new(ErrorKind::StoreFailed, message))?;
        self.finish_pending_disconnect().await
    }
}
