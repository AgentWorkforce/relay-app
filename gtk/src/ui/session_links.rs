//! A session's page in Agent Relay Cloud, in the workspace the install
//! uploads to. The Mac's `SessionNavigation.cloudURL` matches it.
use super::state::State;
use crate::models::{cloud_session_url, uploaded_session_url};
use agent_relay_desktop_core::contract::account::UploadSession;

/// The site, and the workspace the attached install uploads to, if known.
fn site_and_workspace(state: &State) -> (String, Option<String>) {
    let site = state.account().site_url.unwrap_or_default();
    let workspace = state
        .status
        .as_ref()
        .and_then(|status| status.upload_workspace.clone());
    (site, workspace)
}

/// `session`'s Cloud page, whatever its upload state.
pub(super) fn cloud_page(state: &State, session: &UploadSession) -> Option<String> {
    let (site, workspace) = site_and_workspace(state);
    let source = Some(session.source.as_str());
    cloud_session_url(&site, &session.session_id, source, workspace.as_deref())
}

/// `session`'s Cloud page once it has finished uploading.
pub(super) fn uploaded_page(state: &State, session: &UploadSession) -> Option<String> {
    let (site, workspace) = site_and_workspace(state);
    uploaded_session_url(&site, session, workspace.as_deref())
}
