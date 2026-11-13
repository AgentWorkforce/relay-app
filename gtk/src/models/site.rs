//! What the app says about the Agent Relay site and links, given what the
//! core told it. The site is fixed when the core is built; the app and the
//! core are normally one build, but the core the app talks to can be
//! another's (`RELAY_DESKTOP_PROBE`), so the core's word wins where it has
//! spoken.

use agent_relay_node::relay::links::{built_in_site_url, DEFAULT_SITE_URL};

/// The tooltip of the sign-in screen's "Development" badge: the account's
/// site when the core has reported one (`reported`), else this build's
/// own; `None` for production.
pub fn development_badge(reported: Option<&str>) -> Option<&str> {
    development_badge_for(reported, built_in_site_url())
}

/// [`development_badge`] for an app built for `built_in`.
pub fn development_badge_for<'a>(reported: Option<&'a str>, built_in: &'a str) -> Option<&'a str> {
    let site = reported.unwrap_or(built_in);
    (site != DEFAULT_SITE_URL).then_some(site)
}

/// What the person reads when the core refuses a link. For a connect link
/// for another site (`That link connects to <site>, but this app is built
/// for <built in>.`) the link's site is whatever the link carried, so it
/// is not repeated: its host alone, bounded, names which app opens the
/// link, beside the site this app is built for. Any other reason (not a
/// link, the core not up) is shown as it is.
pub fn link_refusal(reason: &str) -> String {
    let other = reason
        .strip_prefix("That link connects to ")
        .and_then(|rest| rest.split_once(", but this app is built for "));
    let Some((site, built_in)) = other else {
        return reason.to_string();
    };
    let built_in = built_in.trim_end_matches('.');
    let app = url::Url::parse(site)
        .ok()
        .and_then(|url| url.host_str().map(bounded))
        .map_or("that site's Agent Relay app".to_string(), |host| {
            format!("the Agent Relay app for {host}")
        });
    format!(
        "That link is for another Agent Relay site. Open it with {app}, or ask for a link for \
         this app's site ({built_in})."
    )
}

/// What the person reads when a link could not be parsed because the core
/// was starting, reconnecting or did not answer, after the link was held
/// and tried again once (`AppModel.linkStillStarting` on macOS).
pub const LINK_STILL_STARTING: &str =
    "Agent Relay is still starting. Open the link again in a moment.";

/// Why a link held while the core was away was dropped after `timeout`,
/// for Diagnostics, word for word as on macOS (`AppModel.linkHoldTimedOut`).
pub fn link_hold_timed_out(timeout: std::time::Duration) -> String {
    format!(
        "Agent Relay's background service was not connected within {} s.",
        timeout.as_secs()
    )
}

/// `host`, cut to a length a dialog can show.
fn bounded(host: &str) -> String {
    const MOST: usize = 64;
    if host.chars().count() <= MOST {
        host.to_string()
    } else {
        format!("{}…", host.chars().take(MOST).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_development_badge_names_any_site_but_production() {
        assert_eq!(development_badge(Some("https://agentrelay.com")), None);
        assert_eq!(
            development_badge(Some("http://localhost:3100")),
            Some("http://localhost:3100")
        );
        assert_eq!(
            development_badge(Some("https://staging.example")),
            Some("https://staging.example")
        );
        // Nothing reported yet: the build's own site decides, whichever
        // it is; once the core has reported, its word wins over the build's.
        assert_eq!(
            development_badge_for(None, "http://localhost:3100"),
            Some("http://localhost:3100")
        );
        assert_eq!(development_badge_for(None, DEFAULT_SITE_URL), None);
        assert_eq!(
            development_badge_for(Some(DEFAULT_SITE_URL), "http://localhost:3100"),
            None
        );
        assert_eq!(
            development_badge_for(Some("https://staging.example"), DEFAULT_SITE_URL),
            Some("https://staging.example")
        );
        let own = built_in_site_url();
        assert_eq!(development_badge(None), development_badge_for(None, own));
    }

    #[test]
    fn a_link_for_another_site_gets_the_remedy_and_nothing_else_does() {
        assert_eq!(
            link_refusal("That link connects to https://other.example, but this app is built for https://agentrelay.com."),
            "That link is for another Agent Relay site. Open it with the Agent Relay app for other.example, or ask for a link for this app's site (https://agentrelay.com)."
        );
        // The link's site is whatever the link carried: its host only, cut
        // short, never a path, query or user info; none at all when it is
        // no URL.
        let long = format!("{}.example", "a".repeat(80));
        assert_eq!(
            link_refusal(&format!("That link connects to https://user:pw@{long}/x?y, but this app is built for http://localhost:3100.")),
            format!("That link is for another Agent Relay site. Open it with the Agent Relay app for {}…, or ask for a link for this app's site (http://localhost:3100).", "a".repeat(64))
        );
        assert_eq!(
            link_refusal("That link connects to ???, but this app is built for https://agentrelay.com."),
            "That link is for another Agent Relay site. Open it with that site's Agent Relay app, or ask for a link for this app's site (https://agentrelay.com)."
        );
        assert_eq!(
            link_refusal("That is not an Agent Relay link this app opens."),
            "That is not an Agent Relay link this app opens."
        );
        assert_eq!(
            link_refusal("Agent Relay is starting. Try again in a moment."),
            "Agent Relay is starting. Try again in a moment."
        );
    }
}
