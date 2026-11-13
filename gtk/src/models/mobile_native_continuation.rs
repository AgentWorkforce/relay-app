//! Matches the Mac's one mobile continuation control word for word: on
//! unless turned off, it sets both routes the core reports.

pub const MOBILE_NATIVE_CONTINUATION_TITLE: &str = "Continue agents from mobile";
pub const MOBILE_NATIVE_CONTINUATION_UNAVAILABLE: &str =
    "Not available on this platform yet. A supported native session connection is required.";
pub const MOBILE_NATIVE_CONTINUATION_DETAIL: &str = "On unless turned off. Your messages from the phone continue a Claude Code, Pi, OpenCode or Codex app session as your own turns, with its permissions unchanged. Claude Code's tool approvals come to your phone, and compatible Codex app windows send command and file approval requests to your phone; other prompts stay on your computer. Needs Claude Code 2.1.310 or later in the 2.1 series; Agent Relay installs its plugin in each shared Claude login folder, and a Claude session already open needs a restart or /reload-plugins once. Reading a session sends no message. Keep the computer awake and the session running. Messages already accepted by an agent may finish after you turn this off.";
/// Shown under the row until this computer's ownership is verified.
pub const MOBILE_NATIVE_CONTINUATION_WAITING: &str = "Waiting for this computer's ownership to be verified for your signed-in account. Turn on Accept spawns from Agent Relay first.";

use agent_relay_node::relay::contract::agents::{SettingsChange, SettingsValues};

pub fn mobile_continuation_enabled(settings: &SettingsValues) -> bool {
    settings.mobile_claude_handoff == Some(true)
        || settings.mobile_native_continuation == Some(true)
}

pub fn mobile_continuation_supported(settings: &SettingsValues) -> bool {
    settings.mobile_claude_handoff_supported == Some(true)
        || settings.mobile_native_continuation_supported == Some(true)
}

pub fn mobile_continuation_change(settings: &SettingsValues, enabled: bool) -> SettingsChange {
    SettingsChange {
        mobile_native_continuation: settings.mobile_native_continuation.map(|_| enabled),
        mobile_claude_handoff: settings.mobile_claude_handoff.map(|_| enabled),
        ..SettingsChange::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{apply_settings_change, settings_change_fields};
    use agent_relay_node::relay::contract::agents::SettingsValues;
    use serde_json::json;

    #[test]
    fn unified_mobile_continuation_changes_both_provider_permissions() {
        let values: SettingsValues = serde_json::from_value(json!({
            "allow_self_registration": true, "auto_activate": false,
            "mobile_claude_handoff": true,
        }))
        .unwrap();
        assert_eq!(values.mobile_native_continuation, None);
        assert_eq!(values.mobile_native_continuation_supported, None);
        assert!(mobile_continuation_enabled(&values));
        let legacy_change = mobile_continuation_change(&values, false);
        assert_eq!(
            settings_change_fields(&legacy_change),
            ["mobile_claude_handoff"]
        );
        assert_eq!(
            apply_settings_change(&values, &legacy_change).mobile_claude_handoff,
            Some(false)
        );
        let current: SettingsValues = serde_json::from_value(json!({
            "allow_self_registration": true, "auto_activate": false,
            "mobile_claude_handoff": false,
            "mobile_native_continuation": false
        }))
        .unwrap();
        let change = mobile_continuation_change(&current, true);
        assert_eq!(
            settings_change_fields(&change),
            ["mobile_claude_handoff", "mobile_native_continuation"]
        );
        let changed = apply_settings_change(&current, &change);
        assert_eq!(changed.mobile_native_continuation, Some(true));
        assert_eq!(changed.mobile_claude_handoff, Some(true));
        let off = mobile_continuation_change(&changed, false);
        assert_eq!(
            apply_settings_change(&changed, &off).mobile_native_continuation,
            Some(false)
        );
        assert_eq!(
            apply_settings_change(&changed, &off).mobile_claude_handoff,
            Some(false)
        );
        assert!(!mobile_continuation_enabled(&apply_settings_change(
            &changed, &off
        )));
        assert!(!mobile_continuation_supported(&values));
        assert_eq!(
            MOBILE_NATIVE_CONTINUATION_TITLE,
            "Continue agents from mobile"
        );
        assert!(MOBILE_NATIVE_CONTINUATION_DETAIL.starts_with("On unless turned off."));
        assert!(MOBILE_NATIVE_CONTINUATION_WAITING.contains("ownership"));
        assert!(MOBILE_NATIVE_CONTINUATION_DETAIL.contains("Claude Code 2.1.310"));
        assert!(MOBILE_NATIVE_CONTINUATION_DETAIL
            .contains("command and file approval requests to your phone"));
        assert!(MOBILE_NATIVE_CONTINUATION_DETAIL.contains("other prompts stay on your computer"));
        assert!(MOBILE_NATIVE_CONTINUATION_DETAIL.contains("may finish after you turn this off"));
    }
}
