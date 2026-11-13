//! The checks an operator policy's settings pass, shared by the core
//! (`history::operator_policy`, for the policy Cloud serves) and
//! `build.rs` (for `DESKTOP_POLICY_DEFAULTS`, the defaults a site profile
//! compiles in), which includes this file by path. It depends on nothing
//! but `std` and `serde_json`.
//!
//! Messages name a setting, never a value.

use serde_json::Value;
use std::collections::BTreeMap;

/// The setting for which local sessions upload (`SharingMode`).
pub const SHARING_MODE: &str = "sharing_mode";
/// `SharingMode`'s values, as Cloud and the contract spell them.
pub const SHARING_MODES: &[&str] = &["all", "new", "selected"];
/// The setting for who may pause and resume uploads: [`UPLOAD_CONTROL_OPERATOR`]
/// or [`UPLOAD_CONTROL_USER`].
pub const UPLOAD_CONTROL: &str = "upload_control";
/// [`UPLOAD_CONTROL`]: uploads run, and the person may not pause them.
pub const UPLOAD_CONTROL_OPERATOR: &str = "operator";
/// [`UPLOAD_CONTROL`]: the person pauses and resumes uploads.
pub const UPLOAD_CONTROL_USER: &str = "user";
/// The setting for how long a finished session waits before it uploads
/// under [`UPLOAD_CONTROL_OPERATOR`], in seconds, at most
/// [`MAX_OOPS_WINDOW_SECONDS`].
pub const OOPS_WINDOW_SECONDS: &str = "oops_window_seconds";
/// The longest [`OOPS_WINDOW_SECONDS`].
pub const MAX_OOPS_WINDOW_SECONDS: u64 = 86_400;
/// The setting for whether the person may delete their own sessions from
/// Cloud (a boolean).
pub const USER_RETROACTIVE_DELETE: &str = "user_retroactive_delete";
/// Every setting this build knows.
pub const KNOWN: &[&str] = &[
    SHARING_MODE,
    UPLOAD_CONTROL,
    OOPS_WINDOW_SECONDS,
    USER_RETROACTIVE_DELETE,
];
/// The build-time defaults' variable (`DESKTOP_POLICY_DEFAULTS`), named in
/// its messages.
pub const DEFAULTS_KEY: &str = "DESKTOP_POLICY_DEFAULTS";

/// The first setting this build knows in `settings` whose value it cannot
/// read; settings it does not know pass, so a newer policy still applies.
pub fn unreadable_setting(settings: &BTreeMap<String, Value>) -> Option<&'static str> {
    let readable = |key: &str, check: fn(&Value) -> bool| settings.get(key).is_none_or(check);
    if !readable(SHARING_MODE, |mode| {
        mode.as_str()
            .is_some_and(|mode| SHARING_MODES.contains(&mode))
    }) {
        return Some(SHARING_MODE);
    }
    if !readable(UPLOAD_CONTROL, |control| {
        matches!(
            control.as_str(),
            Some(UPLOAD_CONTROL_OPERATOR | UPLOAD_CONTROL_USER)
        )
    }) {
        return Some(UPLOAD_CONTROL);
    }
    if !readable(OOPS_WINDOW_SECONDS, |window| {
        window
            .as_u64()
            .is_some_and(|seconds| seconds <= MAX_OOPS_WINDOW_SECONDS)
    }) {
        return Some(OOPS_WINDOW_SECONDS);
    }
    if !readable(USER_RETROACTIVE_DELETE, Value::is_boolean) {
        return Some(USER_RETROACTIVE_DELETE);
    }
    None
}

/// Why `DESKTOP_POLICY_DEFAULTS` was refused. Its message names the
/// variable and the setting, never a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultsError {
    /// Not JSON.
    NotJson,
    /// JSON, but not an object.
    NotObject,
    /// A key other than the settings this build knows.
    UnknownKey,
    /// A known setting with a value it cannot take.
    Invalid(&'static str),
}

impl std::fmt::Display for DefaultsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefaultsError::NotJson => write!(f, "{DEFAULTS_KEY} is not valid JSON"),
            DefaultsError::NotObject => write!(f, "{DEFAULTS_KEY} must be a JSON object"),
            DefaultsError::UnknownKey => {
                write!(f, "{DEFAULTS_KEY} may set only {}", KNOWN.join(", "))
            }
            DefaultsError::Invalid(key) => write!(f, "{DEFAULTS_KEY} has an invalid {key}"),
        }
    }
}

impl std::error::Error for DefaultsError {}

/// `DESKTOP_POLICY_DEFAULTS` read: empty (or only whitespace) is no
/// defaults; anything else must be a JSON object of settings this build
/// knows, each readable, the same checks a fetched policy passes.
pub fn parse_defaults(text: &str) -> Result<Option<BTreeMap<String, Value>>, DefaultsError> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(text).map_err(|_| DefaultsError::NotJson)?;
    let Value::Object(object) = value else {
        return Err(DefaultsError::NotObject);
    };
    let settings: BTreeMap<String, Value> = object.into_iter().collect();
    if settings.keys().any(|key| !KNOWN.contains(&key.as_str())) {
        return Err(DefaultsError::UnknownKey);
    }
    if let Some(key) = unreadable_setting(&settings) {
        return Err(DefaultsError::Invalid(key));
    }
    Ok(Some(settings))
}

/// The defaults as the build passes them on: compact JSON, keys sorted.
pub fn canonical(settings: &BTreeMap<String, Value>) -> String {
    serde_json::to_string(settings).unwrap_or_default()
}

/// The production site: a build for it is no site build.
pub const PRODUCTION_SITE_URL: &str = "https://agentrelay.com";

/// Whether a build made with `AGENT_RELAY_SITE_URL` set to `site` is a site
/// build, as `links` decides from the value the core's build keeps
/// (trimmed, lowercased, one trailing slash dropped): it names a site
/// other than production. Empty is production.
pub fn is_site_url(site: &str) -> bool {
    let site = site.trim().to_ascii_lowercase();
    let site = site.strip_suffix('/').unwrap_or(&site);
    !site.is_empty() && site != PRODUCTION_SITE_URL && site != "https://agentrelay.com:443"
}
