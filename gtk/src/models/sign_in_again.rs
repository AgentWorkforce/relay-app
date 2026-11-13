//! The notice shown while the core has no Cloud session to act as the
//! person, worded as the Mac's (`WorkspaceShell.signInAgainBanner`).

use agent_relay_node::relay::state::{AccountStatus, SignInState};

pub const SIGN_IN_AGAIN_TITLE: &str = "Agent Relay needs you to sign in again.";
pub const SIGN_IN_AGAIN_MESSAGE: &str =
    "Messages, session summaries and subscriptions wait until you do.";
pub const SIGN_IN_AGAIN_ACTION: &str = "Sign in again";
pub const SIGN_IN_AGAIN_WAITING: &str = "Waiting for the browser…";

/// Whether to offer signing in again: connected (or renewing in the
/// browser while connected), with no Cloud session the core may use.
pub fn needs_sign_in_again(account: &AccountStatus, renewing: bool) -> bool {
    account.needs_sign_in
        && (account.state == SignInState::SignedIn
            || (renewing && account.state == SignInState::PendingApproval))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(state: SignInState, needs_sign_in: bool) -> AccountStatus {
        AccountStatus {
            state,
            needs_sign_in,
            ..AccountStatus::default()
        }
    }

    #[test]
    fn offered_only_while_connected_without_a_session() {
        assert!(needs_sign_in_again(
            &account(SignInState::SignedIn, true),
            false
        ));
        assert!(!needs_sign_in_again(
            &account(SignInState::SignedIn, false),
            false
        ));
        assert!(!needs_sign_in_again(
            &account(SignInState::SignedOut, true),
            false
        ));
        assert!(needs_sign_in_again(
            &account(SignInState::PendingApproval, true),
            true
        ));
        assert!(!needs_sign_in_again(
            &account(SignInState::PendingApproval, true),
            false
        ));
    }

    #[test]
    fn worded_as_the_mac() {
        assert_eq!(
            SIGN_IN_AGAIN_TITLE,
            "Agent Relay needs you to sign in again."
        );
        assert_eq!(
            SIGN_IN_AGAIN_MESSAGE,
            "Messages, session summaries and subscriptions wait until you do."
        );
    }
}
