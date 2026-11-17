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
            // After the bump, so a mint in flight loses its epoch check and
            // cannot keep a token again: a gate-only install's relay-node
            // tokens leave the keychain and are deleted at Control with the
            // session's bearer. Awaited before the session leaves the store:
            // from then on the queued identity clear, or the revoke below,
            // revokes it, and a DELETE after that would be refused, leaving
            // the token live. Each DELETE is bounded by its timeout.
            self.delete_relay_node_tokens().await;
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
        // Revoked by the identity clear when one was queued; the relay-node
        // tokens' DELETEs have already been answered either way.
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
        // The organization's settings policy goes with the account.
        self.apply_workspace_settings();
        // Messaging's push channel follows the account.
        if let Some(core) = self.core() {
            core.sync_agents().await;
        }
        forgotten.map_err(|message| RpcError::new(ErrorKind::StoreFailed, message))?;
        self.finish_pending_disconnect().await
    }

    /// Takes this sign-in's relay-node tokens out of the keychain and
    /// deletes each at Control with the latest saved session's bearer, best
    /// effort. With no token nothing is sent; with no session, no `DELETE`.
    async fn delete_relay_node_tokens(&self) {
        let node_tokens = self.forget_relay_node_token().await;
        if node_tokens.is_empty() {
            return;
        }
        let session = self.core().and_then(|core| {
            let _ = core.store.reload();
            core.store.read(|s| s.cloud_session.clone())
        });
        let Some(session) = session else {
            return;
        };
        for (workspace, id) in node_tokens {
            crate::cloud_api::self_tokens::delete(
                &session.site_url,
                &session.access_token,
                &workspace,
                &id,
            )
            .await;
        }
    }
}
