//! The workspace policy's settings sections, as the core takes them
//! (`Core::apply_settings_policy`): spawning, joining, delivery and
//! analytics, each tier merged by key. The capture section is not passed:
//! the operator upload policy (`history::operator_policy`) enforces it.

use super::{SectionName, WorkspacePolicy};
use agent_relay_node::relay::settings_policy::SettingsPolicy;
use serde_json::Map;

/// The sections the core applies to settings.
const SECTIONS: [SectionName; 4] = [
    SectionName::Spawn,
    SectionName::Join,
    SectionName::Delivery,
    SectionName::Analytics,
];

/// `policy`'s settings sections for the core.
pub fn settings_policy(policy: &WorkspacePolicy) -> SettingsPolicy {
    let mut locked = Map::new();
    let mut defaults = Map::new();
    for name in SECTIONS {
        let section = policy.sections.get(name);
        for (key, value) in &section.locked {
            locked.insert(key.name().to_string(), value.clone());
        }
        for (key, value) in &section.defaults {
            defaults.insert(key.name().to_string(), value.clone());
        }
    }
    SettingsPolicy {
        locked,
        defaults,
        managed: policy.managed,
        consent: policy.consent.clone(),
    }
}

/// Whether `policy` asks anything of the core's settings: an unset policy
/// does not, so a workspace without one leaves the settings alone.
pub fn applies(policy: &SettingsPolicy) -> bool {
    !policy.locked.is_empty() || !policy.defaults.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_policy::parse_document;
    use serde_json::{json, Value};

    #[test]
    fn the_settings_sections_are_merged_and_capture_is_left_out() {
        let policy = parse_document(&json!({
            "version": 2, "revision": 4, "managed": true,
            "consent": ["accept_spawns"],
            "sections": {
                "capture": {"locked": {"upload_control": "operator"}, "defaults": {"sharing_mode": "new"}},
                "spawn": {"locked": {"accept_spawns": true}, "defaults": {"spawn_max_agents": 4}},
                "join": {"locked": {"auto_join_repositories": ["acme/*"]}, "defaults": {}},
                "delivery": {"locked": {"inbound_default": "hold", "admin_senders": ["lead"]}, "defaults": {}},
                "analytics": {"locked": {}, "defaults": {"analytics_enabled": false}}
            }
        }))
        .unwrap();
        // A capture default is kept for the uploader and still never
        // reaches the settings.
        assert!(policy
            .default(crate::workspace_policy::Key::SharingMode)
            .is_some());
        let settings = settings_policy(&policy);
        assert_eq!(
            Value::Object(settings.locked.clone()),
            json!({
                "accept_spawns": true,
                "auto_join_repositories": ["acme/*"],
                "inbound_default": "hold",
                "admin_senders": ["lead"],
            })
        );
        assert_eq!(
            Value::Object(settings.defaults.clone()),
            json!({"spawn_max_agents": 4, "analytics_enabled": false})
        );
        assert!(settings.managed);
        assert!(settings.consent.contains("accept_spawns"));
        assert!(applies(&settings));
        assert!(!settings.locked.contains_key("sharing_mode"));
    }

    #[test]
    fn an_unset_policy_asks_nothing_of_the_settings() {
        let settings = settings_policy(&WorkspacePolicy::unset());
        assert!(!applies(&settings));
    }
}
