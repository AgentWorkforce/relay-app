//! What an identity sync that Cloud did not accept does: a site without the
//! route is noted once, a refusal backs off, and anything else is reported.

use super::Identities;
use agent_relay_node::relay::cloud_session::{CloudAccount, CloudError};
use agent_relay_node::relay::links::DEFAULT_SITE_URL;
use agent_relay_node::relay::state::Level;
use serde_json::Value;

/// Whether `error` from the identity route means the site has none: a 404
/// beside a site other than production, which always has it. A self-hosted
/// site without it links no desktop identities to hosted MCP sessions,
/// which is not a failure of this computer's sign-in.
pub(super) fn identity_sync_unsupported(site: &str, error: &CloudError) -> bool {
    site != DEFAULT_SITE_URL && matches!(error, CloudError::Status { status: 404, .. })
}

fn sync_unsupported_message(site: &str) -> String {
    format!("Desktop identity sync is not available on {site}; this computer's agents are not linked to hosted MCP sessions.")
}

fn sync_failure_message(error: &CloudError) -> String {
    match error {
        CloudError::Status { status, .. } => {
            format!("Desktop identity sync failed (HTTP {status}): {error}")
        }
        _ => format!("Desktop identity sync failed: {error}"),
    }
}

impl Identities {
    pub(super) fn sync_failed(
        &self,
        account: CloudAccount,
        generation: u64,
        body: Value,
        error: CloudError,
    ) {
        self.state.forget_synced();
        if identity_sync_unsupported(&account.site_url, &error) {
            // The site has no identity route: nothing to sync, said once,
            // then backed off like a refusal; no failure stands against the
            // sign-in (`note_identity_sync`: did not run).
            self.note_identity_sync(None);
            let site = account.site_url.clone();
            if self.state.remember_refusal(account, generation, body, true) {
                let message = sync_unsupported_message(&site);
                agent_relay_node::relay::diagnostics::write(format_args!("relay: {message}"));
                self.hub.activity(Level::Info, message);
            }
            return;
        }
        self.note_identity_sync(Some(false));
        // A 503 is Cloud unable to verify identities with Relaycast right
        // now; sent again every minute it only adds to that load, so it
        // backs off like a refusal.
        let backs_off = matches!(
            &error,
            CloudError::Status {
                status: 403 | 503,
                ..
            }
        );
        if !backs_off || self.state.remember_refusal(account, generation, body, false) {
            self.report_sync_failure(&error);
        }
    }

    /// A failed identity sync leaves `join_relay` unable to link this
    /// desktop's agents, so it is reported, never dropped silently.
    fn report_sync_failure(&self, error: &CloudError) {
        let message = sync_failure_message(error);
        agent_relay_node::relay::diagnostics::write(format_args!("relay: {message}"));
        self.hub.activity(Level::Warning, message);
    }
}
