//! Updates from the signed release feed of a `cloud` build, on Linux and
//! Windows: the check reads the manifest from the site this build was
//! made for (fixed at build time, like its branding) with the signed-in
//! account's headers and verifies it ([`feed::verify`]); the download asks
//! the site for a signed URL to the manifest's asset, or streams it from
//! the site, and keeps the bytes only when the manifest's size and SHA-256
//! match.
//!
//! **Proposed routes, not confirmed.** The paths under
//! `{site}/cloud/api/v1/desktop/feed` and the answers read here (`404` for
//! no release, `403 {"error":"not_entitled"}`, `501` from `download-url`
//! for a site that only streams, `{url, expires_at}`) follow the Cloud
//! team's draft (`docs/enterprise-distribution.md`); they may change and
//! are not a stable contract.
//!
//! Every feed request fetches its headers first (`account.feed_headers`);
//! a `401` fetches them once more, which renews a rejected sign-in, and
//! tries once more. Feed requests follow no redirect, so the headers reach
//! only the site, and a signed URL is fetched without them. Every body is
//! counted as it arrives and dropped once it passes its budget:
//! [`MAX_FEED_ANSWER`] for a manifest or `download-url` answer, the
//! manifest's size for an asset.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::feed::{self, Arch, Asset, Channel, Expected, FeedError, Kind, Manifest};
use super::{
    download_error, is_newer, verified_download, FeedRequest, FeedResponse, FetchError, Host,
    Platform, Release, UpdateError, MAX_DOWNLOAD, NO_COMPATIBLE_UPDATE, TOO_LARGE, UNREADABLE,
    UNVERIFIED,
};
use crate::build_profile::{self, Profile};

// What a cloud feed check or download says. The Mac's counterparts are in
// `AppUpdateMetadata` (macos/Sources/RelayDesktopCore/AppUpdate.swift) and
// must read the same, word for word.

/// The account is signed out, or its sign-in could not be renewed.
pub const SIGN_IN: &str = concat!(
    "Sign in to ",
    env!("DESKTOP_PRODUCT_NAME"),
    " to receive updates."
);
/// The site answered `403 {"error":"not_entitled"}`.
pub const NOT_ENTITLED: &str = concat!(
    "This workspace is not entitled to updates for ",
    env!("DESKTOP_PRODUCT_NAME"),
    "."
);
/// The manifest failed verification, or a download URL named a host the
/// manifest does not allow; nothing is downloaded.
pub const FEED_UNVERIFIED: &str = "The update could not be verified.";
const NO_PACKAGE: &str = "The latest release has no Debian package for this computer.";

/// The most a `200` manifest or `download-url` answer may be; a signed
/// manifest is a few kilobytes. The Mac's is `CloudFeed.maxFeedAnswerSize`.
pub const MAX_FEED_ANSWER: u64 = 1024 * 1024;

/// The headers for one feed request to the given site; `None` when the
/// account cannot supply them (signed out, signed in on another site or as
/// another account, or a core without `account.feed_headers`).
pub type Headers<'a> = dyn Fn(&str) -> Option<BTreeMap<String, String>> + 'a;

/// The build's site feed and what this build expects of it.
pub struct CloudFeed<'a> {
    /// The site whose feed is read: the built-in site, never the signed-in
    /// account's; a session on another site gets no headers for it.
    pub site: String,
    pub headers: &'a Headers<'a>,
    pub profile: Profile,
    /// The compiled profile id the manifest and its key must name.
    pub profile_id: &'a str,
    /// The compiled feed key, standard base64.
    pub public_key: &'a str,
    /// The running version, which a release must be newer than.
    pub installed_version: &'a str,
}

impl<'a> CloudFeed<'a> {
    /// The built-in site's feed as this build was compiled to read it.
    pub fn built(headers: &'a Headers<'a>) -> CloudFeed<'a> {
        CloudFeed {
            site: agent_relay_node::relay::links::built_in_site_url().to_owned(),
            headers,
            profile: Profile::BUILT,
            profile_id: build_profile::profile_id(),
            public_key: build_profile::feed_public_key(),
            installed_version: agent_relay_node::VERSION,
        }
    }
}

/// A release read from the feed: its verified manifest and the assets for
/// this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedRelease {
    pub manifest: Manifest,
    /// The per-user archive: the tarball on Linux, the zip on Windows.
    pub archive: Asset,
    /// The Debian package, on Linux, when the release has one.
    pub package: Option<Asset>,
}

/// The proposed feed route `path` on `site`.
fn endpoint(site: &str, path: &str) -> String {
    format!(
        "{}/cloud/api/v1/desktop/feed{path}",
        site.trim_end_matches('/')
    )
}

/// Why an authorized request produced no answer.
enum Refusal {
    SignIn,
    Fetch(FetchError),
}

impl Refusal {
    fn checking(self) -> UpdateError {
        UpdateError::new(match self {
            Refusal::SignIn => SIGN_IN,
            Refusal::Fetch(FetchError::TooLarge) => TOO_LARGE,
            Refusal::Fetch(FetchError::Unreachable(_)) => UNREADABLE,
        })
    }

    fn downloading(self) -> UpdateError {
        match self {
            Refusal::SignIn => UpdateError::new(SIGN_IN),
            Refusal::Fetch(error) => download_error(error),
        }
    }
}

/// Sends `method url` with fresh headers and a `200` body budget of
/// `max_body`; a `401` gets fresh headers and one more try, and a second
/// `401` means signing in again.
fn authorized(
    feed: &CloudFeed<'_>,
    host: &Host<'_>,
    method: &'static str,
    url: &str,
    max_body: u64,
) -> Result<FeedResponse, Refusal> {
    for _ in 0..2 {
        let headers = (feed.headers)(&feed.site).ok_or(Refusal::SignIn)?;
        let request = FeedRequest {
            method,
            url: url.to_owned(),
            headers,
            body: None,
            max_body,
        };
        let response = (host.request)(&request).map_err(Refusal::Fetch)?;
        if response.status != 401 {
            return Ok(response);
        }
    }
    Err(Refusal::SignIn)
}

fn not_entitled(response: &FeedResponse) -> bool {
    response.status == 403
        && serde_json::from_slice::<Value>(&response.body)
            .ok()
            .is_some_and(|body| body.get("error").and_then(Value::as_str) == Some("not_entitled"))
}

/// The newest release for `arch` on this platform; `None` when there is no
/// newer one. A beta subscriber reads the beta and the stable manifest and
/// gets the newer of the two, as the GitHub reader does; either failing
/// fails the check, so a broken manifest is never passed over silently.
pub fn latest(
    arch: &str,
    include_beta: bool,
    host: &Host<'_>,
) -> Result<Option<Release>, UpdateError> {
    latest_on(arch, Platform::CURRENT, include_beta, host)
}

/// [`latest`] for `platform`.
pub(super) fn latest_on(
    arch: &str,
    platform: Platform,
    include_beta: bool,
    host: &Host<'_>,
) -> Result<Option<Release>, UpdateError> {
    let feed = host.feed.ok_or_else(|| UpdateError::new(SIGN_IN))?;
    // Without the compiled key nothing could be verified: no request.
    let key = feed::decode_public_key(feed.public_key)
        .ok_or_else(|| UpdateError::new(FEED_UNVERIFIED))?;
    let arch = match arch {
        "x64" => Arch::X64,
        "arm64" => Arch::Arm64,
        _ => return Err(UpdateError::new(NO_COMPATIBLE_UPDATE)),
    };
    let channels: &[Channel] = if include_beta {
        &[Channel::Stable, Channel::Beta]
    } else {
        &[Channel::Stable]
    };
    let mut newest: Option<Manifest> = None;
    for &channel in channels {
        if let Some(manifest) = manifest(feed, host, &key, channel)? {
            if newest
                .as_ref()
                .is_none_or(|old| is_newer(&manifest.version, &old.version))
            {
                newest = Some(manifest);
            }
        }
    }
    newest
        .map(|manifest| release_of(feed, manifest, platform, arch))
        .transpose()
}

/// `channel`'s verified manifest; `None` for none (`404`) or one that is
/// not newer than the running version.
fn manifest(
    feed: &CloudFeed<'_>,
    host: &Host<'_>,
    key: &[u8; 32],
    channel: Channel,
) -> Result<Option<Manifest>, UpdateError> {
    let name = match channel {
        Channel::Stable => "stable",
        Channel::Beta => "beta",
    };
    let url = endpoint(&feed.site, &format!("/manifest?channel={name}"));
    let response =
        authorized(feed, host, "GET", &url, MAX_FEED_ANSWER).map_err(Refusal::checking)?;
    match response.status {
        200 => {}
        404 => return Ok(None),
        _ if not_entitled(&response) => return Err(UpdateError::new(NOT_ENTITLED)),
        _ => return Err(UpdateError::new(UNREADABLE)),
    }
    let expected = Expected {
        profile: feed.profile_id,
        channel,
        installed_version: feed.installed_version,
    };
    match feed::verify(&response.body, key, &expected) {
        Ok(manifest) => Ok(Some(manifest)),
        Err(FeedError::NotNewer) => Ok(None),
        Err(error) => Err(UpdateError::new(error.message())),
    }
}

/// The [`Release`] for `manifest`'s assets for this computer. Its URLs are
/// the streamed downloads, for display; the download itself goes through
/// [`archive_bytes`] and [`package_bytes`].
fn release_of(
    feed: &CloudFeed<'_>,
    manifest: Manifest,
    platform: Platform,
    arch: Arch,
) -> Result<Release, UpdateError> {
    let refused = |error: FeedError| UpdateError::new(error.message());
    let select = |platform, kind| {
        manifest
            .select(feed.profile.asset_prefix, platform, arch, kind, None)
            .cloned()
    };
    let (archive, package) = match platform {
        Platform::Linux => {
            let package = match select(feed::Platform::Linux, Kind::Deb) {
                Ok(asset) => Some(asset),
                Err(FeedError::NoAsset) => None,
                Err(error) => return Err(refused(error)),
            };
            (
                select(feed::Platform::Linux, Kind::Tarball).map_err(refused)?,
                package,
            )
        }
        Platform::Windows => (
            select(feed::Platform::Windows, Kind::Zip).map_err(refused)?,
            None,
        ),
    };
    let stream = |asset: &Asset| endpoint(&feed.site, &format!("/assets/{}", asset.id));
    Ok(Release {
        version: manifest.version.clone(),
        tarball_url: stream(&archive),
        tarball_checksum_url: String::new(),
        deb_url: package.as_ref().map(stream),
        deb_checksum_url: None,
        page_url: build_profile::cloud_downloads_page(&feed.site),
        feed: Some(FeedRelease {
            manifest,
            archive,
            package,
        }),
    })
}

/// The verified per-user archive of `release`: from the feed for a feed
/// release, else GitHub's with its published checksum.
pub(super) fn archive_bytes(release: &Release, host: &Host<'_>) -> Result<Vec<u8>, UpdateError> {
    match &release.feed {
        Some(found) => download(found, &found.archive, host),
        None => verified_download(
            &release.tarball_url,
            &release.tarball_checksum_url,
            host.fetch,
        ),
    }
}

/// The verified Debian package of `release` and its file name.
pub(super) fn package_bytes(
    release: &Release,
    host: &Host<'_>,
) -> Result<(Vec<u8>, String), UpdateError> {
    let missing = || UpdateError::new(NO_PACKAGE);
    if let Some(found) = &release.feed {
        let asset = found.package.as_ref().ok_or_else(missing)?;
        return Ok((download(found, asset, host)?, asset.name.clone()));
    }
    let (Some(url), Some(checksum_url)) = (&release.deb_url, &release.deb_checksum_url) else {
        return Err(missing());
    };
    let data = verified_download(url, checksum_url, host.fetch)?;
    let name = url.rsplit('/').next().unwrap_or(url).to_owned();
    Ok((data, name))
}

/// An answer that is not the one asked for: not entitled, or a failed
/// download.
fn refused_download(response: &FeedResponse) -> UpdateError {
    if not_entitled(response) {
        return UpdateError::new(NOT_ENTITLED);
    }
    download_error(FetchError::Unreachable(format!("HTTP {}", response.status)))
}

/// `asset`'s bytes: from the signed URL the site hands out when its host is
/// one the manifest allows, else (`501`) streamed from the site, of at most
/// the manifest's size; kept only when its size and SHA-256 match.
fn download(found: &FeedRelease, asset: &Asset, host: &Host<'_>) -> Result<Vec<u8>, UpdateError> {
    let feed = host.feed.ok_or_else(|| UpdateError::new(SIGN_IN))?;
    if asset.size > MAX_DOWNLOAD {
        return Err(UpdateError::new(TOO_LARGE));
    }
    let path = format!("/assets/{}", asset.id);
    let signing = endpoint(&feed.site, &format!("{path}/download-url"));
    let answer =
        authorized(feed, host, "POST", &signing, MAX_FEED_ANSWER).map_err(Refusal::downloading)?;
    let data = match answer.status {
        200 => {
            let url = signed_url(&answer.body)
                .filter(|url| feed::download_host_allowed(url, &found.manifest, &feed.site))
                .ok_or_else(|| UpdateError::new(FEED_UNVERIFIED))?;
            let request = FeedRequest {
                method: "GET",
                url,
                headers: BTreeMap::new(),
                body: None,
                max_body: asset.size,
            };
            let response = (host.request)(&request).map_err(download_error)?;
            if response.status != 200 {
                return Err(refused_download(&response));
            }
            response.body
        }
        501 => {
            let url = endpoint(&feed.site, &path);
            let response =
                authorized(feed, host, "GET", &url, asset.size).map_err(Refusal::downloading)?;
            if response.status != 200 {
                return Err(refused_download(&response));
            }
            response.body
        }
        _ => return Err(refused_download(&answer)),
    };
    let actual: String = Sha256::digest(&data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if data.len() as u64 != asset.size || actual != asset.sha256 {
        return Err(UpdateError::new(UNVERIFIED));
    }
    Ok(data)
}

/// The `url` of a `download-url` answer `{url, expires_at}`. The expiry is
/// the site's to enforce; this clock is not trusted for it.
fn signed_url(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value.get("url")?.as_str().map(str::to_owned)
}
