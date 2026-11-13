//! Upload-selection refusal classification and explicit receiver confirmation.
use super::{command_answer, consent_error, require, DeliveryFailure, ReceiverRejection};
use anyhow::Result;

/// A refused selection contract is a service/client compatibility problem,
/// not evidence that the person's connection dropped. Keep remote bodies private.
pub(super) fn selection_error(error: ureq::Error) -> anyhow::Error {
    if let ureq::Error::Status(status, _) = &error {
        eprintln!("receiver rejected upload selection status={status}");
    }
    match error {
        ureq::Error::Status(status @ (400 | 422), _) => ReceiverRejection {
            failure: DeliveryFailure::InvalidPayload,
            status,
            recovery: None,
            retry_after_ms: None,
            hold: None,
        }
        .into(),
        other => consent_error("Could not confirm the Cloud upload change. Try again.")(other),
    }
}

pub(super) fn selection_answer(response: ureq::Response) -> Result<()> {
    require(response.status() == 200, DeliveryFailure::Transient)?;
    let value = command_answer(response)?;
    require(value["updated"] == true, DeliveryFailure::InvalidPayload)
}

#[cfg(test)]
mod tests {
    use super::super::failure_of;
    use super::*;

    #[test]
    fn selection_refusals_distinguish_contract_identity_and_temporary_failures() {
        for (status, expected) in [
            (400, Some(DeliveryFailure::InvalidPayload)),
            (422, Some(DeliveryFailure::InvalidPayload)),
            (401, Some(DeliveryFailure::AuthenticationRequired)),
            (403, Some(DeliveryFailure::PermissionDenied)),
            (404, None),
            (429, None),
            (503, None),
        ] {
            let response = ureq::Response::new(status, "refused", "private server detail").unwrap();
            let error = selection_error(ureq::Error::Status(status, response));
            assert_eq!(failure_of(&error), expected, "HTTP {status}");
            assert!(!error.to_string().contains("private server detail"));
        }
    }

    #[test]
    fn selection_requires_an_explicit_confirmation() {
        for body in [r#"{}"#, r#"{"updated":false}"#, r#"{"updated":"true"}"#] {
            let error =
                selection_answer(ureq::Response::new(200, "OK", body).unwrap()).unwrap_err();
            assert_eq!(failure_of(&error), Some(DeliveryFailure::InvalidPayload));
        }
        selection_answer(ureq::Response::new(200, "OK", r#"{"updated":true}"#).unwrap()).unwrap();
        assert!(selection_answer(ureq::Response::new(200, "OK", "not JSON").unwrap()).is_err());
        let error =
            selection_answer(ureq::Response::new(302, "Redirect", r#"{"updated":true}"#).unwrap())
                .unwrap_err();
        assert_eq!(failure_of(&error), Some(DeliveryFailure::Transient));
    }
}
