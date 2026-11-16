//! The credential Cloud requests go out with, held for requests that must
//! all be made as one person (`spawn::claim`), and the device token a
//! gate-only install enrols its agents at relaygate with.

use super::Account;
use agent_relay_node::relay::cloud_session::CloudAccess;
use agent_relay_node::relay::workspace::Secret;

impl Account {
    /// The site and bearer the next request would use, refreshed if stale,
    /// or with `refused` (a bearer Cloud refused) the session renewed;
    /// `None` when signed out.
    pub(super) async fn pinned_credential(
        &self,
        refused: Option<Secret>,
    ) -> Option<(String, Secret)> {
        let site = self.account()?.site_url;
        let bearer = match refused {
            Some(refused) => {
                let renewed = self.renew(&site, &refused).await;
                self.note_credential(&renewed);
                renewed.ok()?.access_token
            }
            None => {
                let bearer = self.request_bearer(&site).await;
                self.note_credential(&bearer);
                bearer.ok()?.0
            }
        };
        Some((site, bearer))
    }

    /// The device token (`CloudAccess::device_token`): the bearer the next
    /// Cloud request would go out with, refreshed first when it is stale,
    /// exactly as `credential` finds it. `None` when signed out, or when the
    /// kept session could not be renewed (refused or Cloud unreachable): the
    /// node then reports the install signed out rather than failing. The
    /// node sends it to relaygate only; it is never logged here.
    pub(super) async fn device_bearer(&self) -> Option<Secret> {
        self.pinned_credential(None).await.map(|(_, bearer)| bearer)
    }
}

#[cfg(test)]
mod tests;
