pub const STARTUP_TITLE: &str = "Rounding up your agents";
pub const STARTUP_MESSAGES: [&str; 4] = [
    "Herding cats, but the cats can code.",
    "Convincing everyone to use the same group chat.",
    "Untangling a few very ambitious to-do lists.",
    "Making room for one more agent. There’s always one more.",
];
/// How long the app waits for the core before it says startup is taking
/// too long (`AppModel.startupTimeout` on macOS). A connection link held
/// while the core is away waits as long.
pub const STARTUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
pub const STARTUP_TIMEOUT_MESSAGE: &str = "Your agents are taking longer to gather than expected. Try reconnecting the background service.";

/// Where a person whose account has no workspace creates one, on `site`
/// (the site this build is for when unknown).
pub fn workspace_settings_url(site: Option<&str>) -> Option<String> {
    let mut url =
        url::Url::parse(site.unwrap_or(agent_relay_node::relay::links::built_in_site_url()))
            .ok()?;
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
        return None;
    }
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    url.set_path("/cloud/dashboard/settings");
    url.set_query(None);
    url.set_fragment(Some("workspace"));
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_settings_url_opens_the_workspace_tab_on_the_signed_in_site() {
        // Unknown, the site is the build's own (production unless
        // `AGENT_RELAY_SITE_URL` named another when it was built).
        assert_eq!(
            workspace_settings_url(None),
            workspace_settings_url(Some(agent_relay_node::relay::links::built_in_site_url()))
        );
        assert_eq!(
            workspace_settings_url(Some("https://agentrelay.com")).as_deref(),
            Some("https://agentrelay.com/cloud/dashboard/settings#workspace")
        );
        assert_eq!(
            workspace_settings_url(Some("http://user:pw@local.test:3000/x?y=1")).as_deref(),
            Some("http://local.test:3000/cloud/dashboard/settings#workspace")
        );
        assert_eq!(workspace_settings_url(Some("file:///tmp")), None);
    }
}
