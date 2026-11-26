//! Whether a build refuses its PostHog overrides or its PostHog key: a build made for its own
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

/// The error a build given `api_key` fails with when the value is not a
/// PostHog project (capture) token, or `None` when no key is given (a
/// keyless development or site build) or the key is one. Every way the core
/// is compiled runs this (`build.rs`), so a personal API key (`phx_`), which
/// grants account access, never ships. The rule is the collector's
/// (`telemetry::posthog::PostHogProject::new`): `phc_` and 1 to 251 ASCII
/// letters, digits or underscores. The error never includes the value.
pub fn refused_posthog_key(api_key: Option<&str>) -> Option<String> {
    let key = api_key.filter(|key| !key.is_empty())?;
    if key.starts_with("phx_") {
        return Some(
            "AGENT_RELAY_POSTHOG_API_KEY holds a PostHog personal API key (phx_); \
             a build embeds only the project token (phc_)"
                .to_owned(),
        );
    }
    let project = key.strip_prefix("phc_").is_some_and(|body| {
        !body.is_empty()
            && body.len() <= 251
            && body
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    });
    (!project).then(|| {
        "AGENT_RELAY_POSTHOG_API_KEY is not a PostHog project token \
         (phc_ followed by 1 to 251 letters, digits or underscores)"
            .to_owned()
    })
}
