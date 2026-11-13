//! How a sign-in attempt that ended without signing in is reported.

use super::Account;
use agent_relay_node::relay::cloud_login::LoginError;
use agent_relay_node::relay::state::{AccountAction, SignInState};
use crate::telemetry::{TelemetryFailure, TelemetryOperation, TelemetryOutcome};

/// What an expired device sign-in says, on every platform: the site let the
/// approval lapse or ended it, as a password reset does (`expired_token`).
pub(super) const SIGN_IN_EXPIRED: &str = "Your sign-in expired. Sign in again.";

/// A device sign-in that ended: an expired one asks to sign in again in the
/// same words everywhere; any other ending is reported as it came.
pub(super) fn device_ended(error: LoginError) -> LoginError {
    if error.state == "expired" {
        LoginError {
            message: SIGN_IN_EXPIRED.into(),
            ..error
        }
    } else {
        error
    }
}

impl Account {
    /// Ends `attempt` with `error`: its state, message and, when the person
    /// can fix it, the action that does.
    pub(super) fn fail(&self, attempt: u64, error: &LoginError) {
        let state = match error.state {
            "cancelled" => SignInState::SignedOut,
            "denied" => SignInState::Denied,
            "expired" => SignInState::Expired,
            _ => SignInState::Error,
        };
        if self.attempt.lock().unwrap().id == attempt {
            let outcome = if state == SignInState::SignedOut {
                TelemetryOutcome::Cancelled
            } else {
                TelemetryOutcome::Failed
            };
            self.track(
                TelemetryOperation::SignIn,
                outcome,
                (outcome == TelemetryOutcome::Failed).then_some(TelemetryFailure::Unexpected),
            );
        }
        self.set_link(attempt, None);
        self.set(attempt, |account| {
            account.state = state;
            account.phase = None;
            account.progress = None;
            account.error = (state != SignInState::SignedOut).then(|| error.message.clone());
            account.action =
                (error.state == "no_workspace").then_some(AccountAction::CreateWorkspace);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ended(state: &'static str) -> LoginError {
        device_ended(LoginError {
            state,
            message: "Device sign-in was declined or expired. Start sign-in again.".into(),
        })
    }

    #[test]
    fn an_expired_device_sign_in_asks_to_sign_in_again() {
        let error = ended("expired");
        assert_eq!(error.state, "expired");
        assert_eq!(error.message, SIGN_IN_EXPIRED);
    }

    #[test]
    fn other_device_endings_keep_their_message() {
        for state in ["denied", "cancelled", "error", "no_workspace"] {
            let error = ended(state);
            assert_eq!(error.state, state);
            assert_ne!(error.message, SIGN_IN_EXPIRED);
        }
    }
}
