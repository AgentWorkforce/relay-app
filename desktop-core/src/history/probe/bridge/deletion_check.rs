//! Cloud session deletion checks, failed, in words the person can act on.
use super::*;

/// Why Cloud session deletion could not be confirmed, in words the person
/// can act on: sign in again, use the account that uploads here, wait for
/// the service, or check the connection.
pub(super) fn deletion_check_error(error: anyhow::Error) -> anyhow::Error {
    use crate::history::delivery::DeliveryFailure;
    user_error(match crate::history::destination::failure_of(&error) {
        Some(DeliveryFailure::AuthenticationRequired) => {
            "Sign in to Agent Relay again, then try again. Upload was left on."
        }
        Some(DeliveryFailure::PermissionDenied) => {
            "This computer is signed in to a different account than the one uploading here. Upload was left on."
        }
        _ if error.to_string().contains("does not support session deletion") => {
            "This History service does not support session deletion yet. Upload was left on."
        }
        _ => "Could not verify Cloud session deletion. Check your connection and try again. Upload was left on.",
    })
}

#[cfg(test)]
mod deletion_check_tests {
    use super::*;
    use crate::history::delivery::DeliveryFailure;
    use crate::history::destination::TransportFailure;

    #[test]
    fn a_failed_deletion_check_says_what_to_fix() {
        let message = |error: anyhow::Error| deletion_check_error(error).to_string();
        assert!(
            message(TransportFailure(DeliveryFailure::AuthenticationRequired).into())
                .starts_with("Sign in to Agent Relay again")
        );
        assert!(
            message(TransportFailure(DeliveryFailure::PermissionDenied).into())
                .contains("different account")
        );
        assert!(message(anyhow::anyhow!(
            "This History service does not support session deletion yet. Upload was left on."
        ))
        .starts_with("This History service does not support"));
        assert!(message(anyhow::anyhow!("connection reset")).contains("Check your connection"));
    }
}
