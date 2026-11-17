//! How Settings labels the settings the organization's workspace policy
//! decides (`settings.get`'s `organization_policy`), and what it says about
//! messages the organization holds. The Mac's `OrganizationPolicyCopy` is the
//! reference, word for word.

use agent_relay_node::relay::contract::agents::OrganizationPolicyState;

/// A setting the organization set: the person cannot change it.
pub const ORGANIZATION_LOCKED: &str = "This setting is set by your organization.";
/// A setting whose value came from the organization as a starting point.
pub const ORGANIZATION_MANAGED: &str = "Managed by your organization.";
/// A setting the organization would turn on, which only takes effect on a
/// computer it enrolled with consent for that setting.
pub const ORGANIZATION_MISMATCHED: &str =
    "Your organization turns this on only on computers it enrolls. This computer keeps your own choice.";
/// Why a sender cannot be allowed for good while the organization holds
/// messages from senders who are not its admins.
pub const ORGANIZATION_HELD: &str =
    "Your organization holds messages from senders who are not its admins. Approve each message as it arrives.";
/// The message approvals section while the organization holds messages in
/// every harness, not only Claude Code's.
pub const APPROVALS_TITLE: &str = "Message approvals";
/// The repositories whose sessions join the relay by themselves.
pub const REPOSITORIES_TITLE: &str = "Repositories that join the relay";
pub const REPOSITORIES_DETAIL: &str =
    "Sessions working in these repositories join the relay by themselves.";
/// The empty list.
pub const REPOSITORIES_NONE: &str = "None";

/// How the message approvals section reads, with or without the
/// organization's hold (`claude_delivery.status` `organization_hold`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Approvals {
    pub title: &'static str,
    /// The note under its description, while the organization holds.
    pub note: Option<&'static str>,
    /// Whether "Always allow" can be used.
    pub always_allow: bool,
}

pub fn approvals(organization_hold: bool) -> Approvals {
    if organization_hold {
        Approvals {
            title: APPROVALS_TITLE,
            note: Some(ORGANIZATION_HELD),
            always_allow: false,
        }
    } else {
        Approvals {
            title: "Claude Code message approvals",
            note: None,
            always_allow: true,
        }
    }
}

/// The note under `key`'s row, when the organization decides it.
pub fn organization_note(
    policy: Option<&OrganizationPolicyState>,
    key: &str,
) -> Option<&'static str> {
    let policy = policy?;
    let has = |keys: &[String]| keys.iter().any(|k| k == key);
    if has(&policy.locked) {
        Some(ORGANIZATION_LOCKED)
    } else if has(&policy.mismatched) {
        Some(ORGANIZATION_MISMATCHED)
    } else if has(&policy.defaults) {
        Some(ORGANIZATION_MANAGED)
    } else {
        None
    }
}

/// Whether the organization locks `key`, so the person cannot change it.
pub fn organization_locked(policy: Option<&OrganizationPolicyState>, key: &str) -> bool {
    policy.is_some_and(|policy| policy.locked.iter().any(|k| k == key))
}

/// The first note among `keys` (a row that changes several settings).
pub fn organization_note_any(
    policy: Option<&OrganizationPolicyState>,
    keys: &[&str],
) -> Option<&'static str> {
    keys.iter().find_map(|key| organization_note(policy, key))
}

/// Whether the organization locks any of `keys`.
pub fn organization_locked_any(policy: Option<&OrganizationPolicyState>, keys: &[&str]) -> bool {
    keys.iter().any(|key| organization_locked(policy, key))
}

/// The repository an agent's session is in, as its registration names it
/// (`agents.list` `repository`).
pub fn repository_line(name: &str) -> String {
    format!("Repository: {name}")
}

/// The repositories list as one line.
pub fn repositories_line(entries: &[String]) -> String {
    if entries.is_empty() {
        REPOSITORIES_NONE.to_string()
    } else {
        entries.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> OrganizationPolicyState {
        OrganizationPolicyState {
            locked: vec!["spawn_max_agents".into(), "analytics_enabled".into()],
            defaults: vec!["auto_join_general".into()],
            mismatched: vec!["accept_spawns".into()],
            managed: false,
        }
    }

    #[test]
    fn each_setting_gets_its_own_note() {
        let policy = policy();
        let policy = Some(&policy);
        assert_eq!(
            organization_note(policy, "spawn_max_agents"),
            Some(ORGANIZATION_LOCKED)
        );
        assert_eq!(
            organization_note(policy, "auto_join_general"),
            Some(ORGANIZATION_MANAGED)
        );
        assert_eq!(
            organization_note(policy, "accept_spawns"),
            Some(ORGANIZATION_MISMATCHED)
        );
        assert_eq!(organization_note(policy, "spawn_native_input"), None);
        assert_eq!(organization_note(None, "spawn_max_agents"), None);
        assert_eq!(
            organization_note_any(policy, &["spawn_full_access_codex", "spawn_max_agents"]),
            Some(ORGANIZATION_LOCKED)
        );
    }

    #[test]
    fn only_a_lock_keeps_the_person_from_changing_it() {
        let policy = policy();
        let policy = Some(&policy);
        assert!(organization_locked(policy, "analytics_enabled"));
        assert!(!organization_locked(policy, "auto_join_general"));
        assert!(!organization_locked(policy, "accept_spawns"));
        assert!(!organization_locked(None, "analytics_enabled"));
        assert!(organization_locked_any(policy, &["x", "spawn_max_agents"]));
    }

    #[test]
    fn the_wording_matches_the_mac() {
        assert_eq!(
            ORGANIZATION_LOCKED,
            "This setting is set by your organization."
        );
        assert_eq!(ORGANIZATION_MANAGED, "Managed by your organization.");
        assert_eq!(APPROVALS_TITLE, "Message approvals");
        assert_eq!(
            ORGANIZATION_MISMATCHED,
            "Your organization turns this on only on computers it enrolls. This computer keeps your own choice."
        );
        assert_eq!(
            ORGANIZATION_HELD,
            "Your organization holds messages from senders who are not its admins. Approve each message as it arrives."
        );
        assert_eq!(REPOSITORIES_TITLE, "Repositories that join the relay");
        assert_eq!(
            REPOSITORIES_DETAIL,
            "Sessions working in these repositories join the relay by themselves."
        );
        assert_eq!(
            repositories_line(&["acme/web".into(), "tools/*".into()]),
            "acme/web, tools/*"
        );
        assert_eq!(repositories_line(&[]), "None");
        assert_eq!(repository_line("acme/web"), "Repository: acme/web");
    }

    #[test]
    fn approvals_read_as_the_organization_holds() {
        let held = approvals(true);
        assert_eq!(held.title, "Message approvals");
        assert_eq!(held.note, Some(ORGANIZATION_HELD));
        assert!(!held.always_allow);
        let open = approvals(false);
        assert_eq!(open.title, "Claude Code message approvals");
        assert_eq!(open.note, None);
        assert!(open.always_allow);
    }
}
