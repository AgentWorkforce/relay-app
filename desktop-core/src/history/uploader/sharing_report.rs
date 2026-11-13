//! The sharing choice `uploader.status` reports, and the stage that goes
//! with it.
//!
//! A status that does not say what the collector shares never reads as the
//! most private choice: the person would trust "Only sessions I choose"
//! while a collector shares more. It reads as sharing everything, and a
//! running collector needs attention until a status names its mode.

use crate::contract::account::{SharingMode, Stage};
use serde_json::Value;

/// What a person is told while the running collector's sharing choice is
/// unknown.
pub const UNKNOWN_SHARING: &str = "Agent Relay can't confirm which sessions the background upload service is sharing. Quit and reopen Agent Relay to restart it.";

fn named(status: &Value) -> Option<SharingMode> {
    serde_json::from_value(status["sharing_mode"].clone()).ok()
}

fn unknown_while_running(status: &Value) -> bool {
    named(status).is_none() && status["running"].as_bool().unwrap_or(false)
}

/// The sharing choice `status` names, or the broadest one when it names none.
pub(super) fn mode(status: &Value) -> SharingMode {
    named(status).unwrap_or(SharingMode::All)
}

/// The stage `status` reports, or needs attention while a running
/// collector's sharing choice is unknown.
pub(super) fn stage(status: &Value) -> Stage {
    if unknown_while_running(status) {
        return Stage::NeedsAttention;
    }
    serde_json::from_value(status["stage"].clone()).unwrap_or(Stage::Offline)
}

/// The sentence for [`stage`].
pub(super) fn stage_message(status: &Value) -> Option<String> {
    if unknown_while_running(status) {
        return Some(UNKNOWN_SHARING.into());
    }
    status["stage_message"].as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::super::status_report;
    use super::*;
    use serde_json::json;

    fn status(running: bool, sharing_mode: Option<&str>) -> Value {
        let mut status = json!({
            "running": running, "paused": false, "stage": "up_to_date",
            "delivery": {}, "sessions": {},
        });
        if let Some(mode) = sharing_mode {
            status["sharing_mode"] = json!(mode);
        }
        status
    }

    #[test]
    fn an_unknown_sharing_mode_is_never_reported_as_selected() {
        let reported = status_report(&status(true, None));
        assert_eq!(reported.sharing_mode, SharingMode::All);
        assert_eq!(reported.stage, Stage::NeedsAttention);
        assert_eq!(reported.stage_message.as_deref(), Some(UNKNOWN_SHARING));

        let reported = status_report(&status(true, Some("selected")));
        assert_eq!(reported.sharing_mode, SharingMode::Selected);
        assert_eq!(reported.stage, Stage::UpToDate);
        assert_eq!(reported.stage_message, None);

        let reported = status_report(&status(false, None));
        assert_eq!(reported.sharing_mode, SharingMode::All);
        assert_eq!(reported.stage, Stage::UpToDate);
    }
}
