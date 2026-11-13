//! What the deployed History service supports, asked before a user-facing
//! command changes local state.
use super::reset::{reset_allowed, ResetUnsupported};
use super::{
    check_account, cloud, command_answer, command_budget, consent_error, fail, DeliveryFailure,
};
use anyhow::Result;
use std::time::Duration;

/// The deployed History service's published limits, for a user-facing
/// command; `refused` is what a refusal other than sign-in or account says.
fn command_limits(
    base_url: &str,
    expected_account: &str,
    refused: &'static str,
) -> Result<serde_json::Value> {
    let auth = cloud::load_selected_auth(Some(base_url))?
        .ok_or_else(|| fail(DeliveryFailure::AuthenticationRequired))?;
    let url = format!("{}/v1/delivery/limits", cloud::normalized_stage(base_url)?);
    let agent = cloud::history_agent()
        .redirects(0)
        .timeout(Duration::from_secs(15))
        .build();
    let budget = command_budget(Duration::from_secs(15));
    let response = cloud::send_with_auth_refresh_checked_with_budget(
        &auth,
        |current, remaining| {
            agent
                .get(&url)
                .timeout(remaining.unwrap_or(Duration::from_secs(15)))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .call()
                .map_err(Box::new)
        },
        consent_error(refused),
        |current| check_account(current, expected_account),
        Some(&budget),
    )?;
    command_answer(response)
}

/// Fail before changing local state if the deployed History service cannot delete.
pub fn require_session_deletion(base_url: &str, expected_account: &str) -> Result<()> {
    let value = command_limits(
        base_url,
        expected_account,
        "Could not check cloud deletion support. Try again.",
    )?;
    anyhow::ensure!(
        value["sessionDeletion"] == true,
        "This History service does not support session deletion yet. Upload was left on."
    );
    Ok(())
}

/// Refuse a cloud reset, before any reset plan is written, when the History
/// service publishes `sessionReset: false`. A limits answer that cannot be
/// read leaves the decision to the reset request itself, as before.
pub fn require_session_reset(base_url: &str, expected_account: &str) -> Result<()> {
    match command_limits(base_url, expected_account, "limits unavailable") {
        Ok(limits) if !reset_allowed(&limits) => Err(ResetUnsupported.into()),
        _ => Ok(()),
    }
}
