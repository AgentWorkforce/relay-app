//! Local configuration refusals wait a fixed interval without blocking the job.
//! The wait is this computer's own, not a receiver Retry-After, so Retry wakes
//! it at once once the person has fixed the configuration.
use super::{
    cloud, fail, failure_of, transport_log, DeliveryFailure, ReceiverFailure, ReceiverRejection,
};

const CONFIGURATION_RETRY_MS: i64 = 60_000;

pub(super) fn configuration_failure(error: &anyhow::Error) -> Option<DeliveryFailure> {
    configuration_verdict(error).map(|verdict| verdict.failure)
}

pub(super) fn configuration_verdict(error: &anyhow::Error) -> Option<ReceiverFailure> {
    cloud::HistoryTlsConfigurationRefused::caused_by(error).then_some(ReceiverFailure {
        failure: DeliveryFailure::Transient,
        retry_after_ms: None,
        local_wait_ms: Some(CONFIGURATION_RETRY_MS),
        recovery: None,
        hold: None,
        upload: None,
    })
}

pub(super) fn transport_error(error: ureq::Transport) -> anyhow::Error {
    if cloud::certificate_override_refused(&error) {
        return cloud::HistoryTlsConfigurationRefused.into();
    }
    eprintln!("{}", transport_log(&error));
    fail(DeliveryFailure::Transient)
}

/// Receiver decisions on the batch remain permanent; a local configuration
/// refusal says nothing about the batch and uses a fixed local wait instead.
pub(super) fn verdict(error: &anyhow::Error) -> ReceiverFailure {
    if let Some(rejection) = error.downcast_ref::<ReceiverRejection>() {
        return ReceiverFailure {
            failure: rejection.failure,
            retry_after_ms: rejection.retry_after_ms,
            local_wait_ms: None,
            recovery: rejection.recovery.clone(),
            hold: rejection.hold.clone(),
            upload: None,
        };
    }
    configuration_verdict(error).unwrap_or_else(|| {
        failure_of(error)
            .unwrap_or(DeliveryFailure::Transient)
            .into()
    })
}

#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod tests;
