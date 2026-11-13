//! Upload-selection failures, in words the person can act on.
use super::user_error;

pub(super) fn cloud_selection_error(error: anyhow::Error) -> anyhow::Error {
    user_error(match crate::history::destination::failure_of(&error) {
        Some(crate::history::delivery::DeliveryFailure::AuthenticationRequired) => {
            "Sign in to Agent Relay again to finish the Cloud upload change."
        }
        Some(crate::history::delivery::DeliveryFailure::PermissionDenied) => {
            "This computer is signed in to a different account than the one uploading here, so the Cloud upload change was not made."
        }
        Some(crate::history::delivery::DeliveryFailure::InvalidPayload) => {
            "Agent Relay Cloud could not apply this upload change. Try again later; if it continues, contact support."
        }
        _ => "Could not confirm the Cloud upload change. Check your connection and try again.",
    })
}
