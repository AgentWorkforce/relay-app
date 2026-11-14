//! The product-named values a Windows install writes into the registry,
//! composed from plain strings so the composition is the same, and tested,
//! on every platform; `updates/windows.rs` writes them.

use super::Profile;

/// The product-named values a Windows install writes under
/// `HKEY_CURRENT_USER`, for `profile`: the link handler's entries (the core's
/// `protocol_entries`, whose name value says "URL:Agent Relay" for every
/// build) with that value naming the profile's product, then the
/// notification identity's `DisplayName` under `app_user_model_key`. Windows
/// shows that name over the app's notifications. Every product shares the
/// key (one Agent Relay app per machine), so a product installed over another
/// one renames it when it registers its launcher (the Setup program runs
/// `--install-launcher`) or shows a notification: it is always the built
/// product's, never a literal. `(key, value name, data)`, an
/// empty name being the key's default. Plain strings, so it is the same
/// composition on every platform and tested on each.
pub fn windows_product_records(
    profile: &Profile,
    app_user_model_key: &str,
    protocol_entries: Vec<(String, String, String)>,
) -> (Vec<(String, String, String)>, (String, String, String)) {
    let handler = protocol_entries
        .into_iter()
        .map(|(key, name, data)| {
            if name.is_empty() && data.starts_with("URL:") {
                (key, name, format!("URL:{}", profile.product_name))
            } else {
                (key, name, data)
            }
        })
        .collect();
    let identity = (
        app_user_model_key.to_owned(),
        "DisplayName".to_owned(),
        profile.product_name.to_owned(),
    );
    (handler, identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A placeholder profile, not the default.
    const PROFILE: Profile = Profile {
        product_name: "Example Desktop",
        releases_repository: "example-owner/example-releases",
        asset_prefix: "Desktop-example-1",
        release_feed: "github:example-owner/example-releases",
        cobrand: None,
    };
    const KEY: &str = r"Software\Classes\AppUserModelId\AgentWorkforce.AgentRelay";
    const PROTOCOL: &str = r"Software\Classes\agentrelay";

    /// The core's link handler entries, as `protocol_entries` returns them.
    fn core_entries() -> Vec<(String, String, String)> {
        let exe = r"C:\Users\a\AppData\Local\Programs\AgentRelay\1\T\bin\agent-relay.exe";
        vec![
            (PROTOCOL.into(), String::new(), "URL:Agent Relay".into()),
            (PROTOCOL.into(), "URL Protocol".into(), String::new()),
            (
                format!(r"{PROTOCOL}\DefaultIcon"),
                String::new(),
                format!("\"{exe}\",0"),
            ),
            (
                format!(r"{PROTOCOL}\shell\open\command"),
                String::new(),
                format!("\"{exe}\" \"%1\""),
            ),
        ]
    }

    #[test]
    fn a_profiles_windows_records_name_its_product() {
        let (handler, identity) = windows_product_records(&PROFILE, KEY, core_entries());
        assert_eq!(
            identity,
            (KEY.into(), "DisplayName".into(), "Example Desktop".into())
        );
        assert_eq!(handler[0].2, "URL:Example Desktop");
        // Nothing but the name changes.
        assert_eq!(handler[1..], core_entries()[1..]);
        assert!(!handler
            .iter()
            .chain([&identity])
            .any(|(_, _, data)| data.contains("Agent Relay")));
    }

    #[test]
    fn the_default_profiles_windows_records_name_agent_relay() {
        let (handler, identity) = windows_product_records(&Profile::DEFAULT, KEY, core_entries());
        assert_eq!(identity.2, "Agent Relay");
        assert_eq!(handler, core_entries());
    }

    #[test]
    fn the_built_profiles_identity_is_the_built_product_name() {
        let (_, identity) = windows_product_records(&Profile::BUILT, KEY, core_entries());
        assert_eq!(identity.2, crate::build_profile::PRODUCT_NAME);
    }
}
