//! Shared authentication health presentation and refresh handling.

use super::*;

/// The sidebar header's and account dot's connection: label, color, help,
/// and whether all is well.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub label: &'static str,
    pub color: Color,
    pub help: String,
    pub healthy: bool,
}

impl State {
    pub fn prepare_account_update(&mut self, next: &AccountStatus) -> AccountStatus {
        let previous = self.account();
        if previous.auth_checked_at != next.auth_checked_at {
            self.account_health_error = None;
        }
        previous
    }
}

/// Shared account health wording for the header and Account settings.
pub fn account_health(account: &AccountStatus) -> (&'static str, String, bool) {
    let (label, detail, healthy) = match account.auth_health.as_deref() {
        Some("ready") => (
            "Connected",
            "Account and workspace access verified. Sign-in is checked automatically.",
            true,
        ),
        Some("sign_in_required") => (
            "Sign in required",
            "Sign in again to restore account and workspace access.",
            false,
        ),
        Some("offline") => (
            "Cannot verify sign-in",
            "Cannot reach Agent Relay. Sign-in will be checked again automatically.",
            false,
        ),
        _ => (
            "Checking sign-in",
            "Checking your account and workspace access.",
            false,
        ),
    };
    (
        label,
        if matches!(
            account.auth_health.as_deref(),
            Some("sign_in_required" | "offline")
        ) {
            account
                .auth_error
                .clone()
                .unwrap_or_else(|| detail.to_string())
        } else {
            detail.to_string()
        },
        healthy,
    )
}

#[cfg(test)]
mod account_health_tests {
    use super::*;

    #[test]
    fn only_verified_authentication_is_healthy() {
        let mut account = AccountStatus::default();
        assert_eq!(account_health(&account).0, "Checking sign-in");
        assert!(!account_health(&account).2);
        account.auth_health = Some("ready".into());
        assert_eq!(account_health(&account).0, "Connected");
        assert!(account_health(&account).2);
        account.auth_health = Some("sign_in_required".into());
        assert_eq!(account_health(&account).0, "Sign in required");
        assert!(!account_health(&account).2);
        account.auth_health = Some("offline".into());
        account.auth_error = Some("Network unavailable".into());
        assert_eq!(account_health(&account).1, "Network unavailable");
        assert!(!account_health(&account).2);
        account.auth_health = Some("future_state".into());
        assert!(!account_health(&account).2);
    }
}
