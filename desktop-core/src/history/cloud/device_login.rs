//! The OAuth device flow the probe and the CLI sign in with when no kept
//! session can be reused.

use super::{auth_error, cloud_request_from, cloud_site, DeviceApproval};
use anyhow::{Context, Result};
use url::Url;

/// What the probe says when the site expired an approval it never collected,
/// as a password reset does (`expired_token`).
pub(super) const DEVICE_SIGN_IN_EXPIRED: &str =
    "Your sign-in expired. Run setup again to sign in again.";

pub(super) fn device_login(
    api_url: &str,
    client_name: &str,
    announce: &mut dyn FnMut(&DeviceApproval),
) -> Result<String> {
    let site = cloud_site(api_url)?;
    // Both calls are JSON POSTs to the site, so they name it as their origin.
    let origin = Some(site.as_str());
    let (status, grant) = cloud_request_from(
        origin,
        "POST",
        &format!("{api_url}/api/v1/auth/device/start"),
        None,
        Some(serde_json::json!({ "client_name": client_name })),
    )?;
    anyhow::ensure!(matches!(status, 200 | 201), "device start rejected");
    let device_code = grant["device_code"]
        .as_str()
        .ok_or_else(|| auth_error("Cloud did not start device sign-in."))?;
    let complete = grant["verification_uri_complete"].as_str();
    let original = grant["verification_uri"].as_str();
    let verification = Url::parse(
        complete
            .or_else(|| grant["verification_uri"].as_str())
            .context("Cloud did not return a sign-in URL.")?,
    )?;
    // Bind approval to the selected environment before anything is shown or
    // polled: a link on another origin is a phishing target, not a sign-in.
    anyhow::ensure!(
        verification.origin().ascii_serialization() == site
            && verification.username().is_empty()
            && verification.password().is_none(),
        "wrong sign-in origin"
    );
    let expires_in = grant["expires_in"].as_u64().unwrap_or(600).clamp(1, 900);
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);
    let mut interval = grant["interval"].as_u64().unwrap_or(5).clamp(1, 60);
    announce(&DeviceApproval {
        verification_uri: original.unwrap_or(verification.as_str()).to_string(),
        verification_uri_complete: complete.map(String::from),
        user_code: grant["user_code"].as_str().map(String::from),
        expires_at: expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        interval,
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires_in);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_secs(interval));
        let result = cloud_request_from(
            origin,
            "POST",
            &format!("{api_url}/api/v1/auth/device/token"),
            None,
            Some(serde_json::json!({
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                "device_code": device_code
            })),
        );
        let (status, token) = match result {
            Ok(value) => value,
            Err(_) => continue,
        };
        if status == 200 {
            anyhow::ensure!(
                token["api_url"]
                    .as_str()
                    .is_none_or(|url| url.trim_end_matches('/') == api_url),
                "Cloud origin changed"
            );
            return token["access_token"]
                .as_str()
                .filter(|value| !value.is_empty())
                .map(String::from)
                .context("Cloud sign-in did not return credentials.");
        }
        if status == 429 || token["error"] == "slow_down" {
            interval = (interval + 5).min(60);
            continue;
        }
        if status >= 500 || token["error"] == "authorization_pending" {
            continue;
        }
        if token["error"] == "expired_token" {
            return Err(auth_error(DEVICE_SIGN_IN_EXPIRED));
        }
        return Err(auth_error(
            "Device sign-in was declined or expired. Run setup again.",
        ));
    }
    Err(auth_error("Device sign-in expired. Run setup again."))
}
