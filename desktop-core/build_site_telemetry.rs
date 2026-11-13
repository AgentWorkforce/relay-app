//! Whether a build refuses its PostHog overrides: a build made for its own
//! site (`AGENT_RELAY_SITE_URL` names another site than production, the rule
//! in `gtk/build.rs`) reports only through its own site's authenticated
//! analytics gateway, so `AGENT_RELAY_POSTHOG_HOST` and
//! `AGENT_RELAY_POSTHOG_API_KEY` are refused for it. Shared by `build.rs` and
//! the crate's tests (`site_build`).

/// The production site; any other `AGENT_RELAY_SITE_URL` makes a site build.
const PRODUCTION_SITE_URL: &str = "https://agentrelay.com";

/// The error a build for `site_url` given `host` and `api_key` fails with,
/// or `None` when the build may go on. The error names the variables only,
/// never any of their values.
pub fn refused_posthog_override(
    site_url: Option<&str>,
    host: Option<&str>,
    api_key: Option<&str>,
) -> Option<String> {
    let set = |value: Option<&str>| value.is_some_and(|value| !value.is_empty());
    let site_build = site_url
        .is_some_and(|site| !site.is_empty() && site.trim_end_matches('/') != PRODUCTION_SITE_URL);
    (site_build && (set(host) || set(api_key))).then(|| {
        "a build for its own site reports only to that site's analytics gateway: \
         unset AGENT_RELAY_POSTHOG_HOST and AGENT_RELAY_POSTHOG_API_KEY \
         (both are refused for a site build)"
            .to_owned()
    })
}
