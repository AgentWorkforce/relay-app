//! Why a bridge call failed, as the app shows it.

use agent_relay_node::relay::contract::plan::PlanLimitExceeded;
use agent_relay_node::relay::protocol::ErrorKind;
use serde_json::Value;

/// Why a call failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The core answered with this contract error.
    Core(ErrorKind),
    /// No core answered.
    Unavailable,
    /// The core's answer does not fit this build's contract.
    Unreadable,
}

/// A failed call: `message` is a sentence safe to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreError {
    pub failure: Failure,
    pub message: String,
    /// Whether repeating the same call later can succeed.
    pub retryable: bool,
    /// With `plan_limit_exceeded`: which cap was reached and how full it
    /// is, when the core's answer said.
    pub plan_limit: Option<PlanLimitExceeded>,
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CoreError {}

impl CoreError {
    pub fn unavailable() -> CoreError {
        CoreError {
            failure: Failure::Unavailable,
            message: "Agent Relay is starting. Try again in a moment.".into(),
            retryable: true,
            plan_limit: None,
        }
    }

    pub(crate) fn unavailable_because(message: &str) -> CoreError {
        CoreError {
            failure: Failure::Unavailable,
            message: message.into(),
            retryable: false,
            plan_limit: None,
        }
    }

    pub(super) fn reconnecting() -> CoreError {
        CoreError {
            failure: Failure::Unavailable,
            message:
                "Agent Relay is reconnecting to its background service. Try again in a moment."
                    .into(),
            retryable: true,
            plan_limit: None,
        }
    }

    pub(crate) fn interrupted() -> CoreError {
        CoreError {
            failure: Failure::Unavailable,
            message: "Connection to Agent Relay was interrupted. Check whether the action completed before trying again.".into(),
            retryable: false,
            plan_limit: None,
        }
    }

    /// A read the core did not answer within its deadline.
    pub(super) fn took_too_long() -> CoreError {
        CoreError {
            failure: Failure::Unavailable,
            message: "The background service took too long to respond. Try again.".into(),
            retryable: true,
            plan_limit: None,
        }
    }

    /// Whether this is [`CoreError::took_too_long`].
    pub fn timed_out(&self) -> bool {
        *self == CoreError::took_too_long()
    }

    pub(super) fn core(kind: ErrorKind, message: &str) -> CoreError {
        CoreError {
            failure: Failure::Core(kind),
            message: message.into(),
            retryable: kind.retryable(),
            plan_limit: None,
        }
    }

    pub fn kind(&self) -> Option<ErrorKind> {
        match self.failure {
            Failure::Core(kind) => Some(kind),
            _ => None,
        }
    }

    /// A JSON-RPC error object as the core sends it. A kind this build does
    /// not know reads as `internal`, the contract's fallback.
    pub(super) fn from_wire(error: &Value) -> CoreError {
        let data = &error["data"];
        CoreError {
            failure: Failure::Core(
                serde_json::from_value(data["kind"].clone()).unwrap_or(ErrorKind::Internal),
            ),
            message: error["message"]
                .as_str()
                .unwrap_or("Agent Relay hit a problem.")
                .to_string(),
            retryable: data["retryable"].as_bool().unwrap_or(false),
            plan_limit: serde_json::from_value(data["plan_limit"].clone()).ok(),
        }
    }

    pub(super) fn unreadable(method: &str) -> CoreError {
        CoreError {
            failure: Failure::Unreadable,
            message: format!("Agent Relay could not read the answer to {method}."),
            retryable: false,
            plan_limit: None,
        }
    }
}
