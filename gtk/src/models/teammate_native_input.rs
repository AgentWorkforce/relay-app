//! Whether a teammate's direct message starts a user turn in a Claude Code
//! session with Agent Relay's plugin. Matches TeammateNativeInputCopy.

pub const TEAMMATE_NATIVE_INPUT_TITLE: &str = "Native input from teammates";
pub const TEAMMATE_NATIVE_INPUT_DETAIL: &str = "Off: teammates' messages arrive as peer messages. On: a direct message from a teammate or their agent to a Claude Code session with Agent Relay's plugin starts a user turn, labelled with the sender. Channel posts and mentions stay peer messages, and tool approvals still go to you.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{apply_settings_change, settings_change_fields};
    use agent_relay_node::relay::contract::agents::{SettingsChange, SettingsValues};
    use serde_json::json;

    #[test]
    fn old_cores_do_not_offer_it_and_a_change_names_only_its_key() {
        let values: SettingsValues = serde_json::from_value(json!({
            "allow_self_registration": true, "auto_activate": false,
        }))
        .unwrap();
        assert_eq!(values.teammate_native_input, None);
        let on: SettingsValues = serde_json::from_value(json!({
            "allow_self_registration": true, "auto_activate": false, "teammate_native_input": true,
        }))
        .unwrap();
        assert_eq!(on.teammate_native_input, Some(true));
        let change = SettingsChange {
            teammate_native_input: Some(false),
            ..Default::default()
        };
        assert_eq!(settings_change_fields(&change), ["teammate_native_input"]);
        assert_eq!(
            apply_settings_change(&on, &change).teammate_native_input,
            Some(false)
        );
        assert_eq!(TEAMMATE_NATIVE_INPUT_TITLE, "Native input from teammates");
        assert!(TEAMMATE_NATIVE_INPUT_DETAIL.contains("tool approvals still go to you"));
    }
}
