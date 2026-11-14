//! The cloud feed reader against a fake site: the draft's manifest
//! fixtures, re-signed at run time with a throwaway key, and small payloads
//! in place of the real assets.

use std::cell::{Cell, RefCell};
use std::io;
use std::path::PathBuf;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;

use super::*;
use crate::build_profile::CLOUD_RELEASE_FEED;
use crate::updates::http::{answer_body, REFUSAL_BODY_DEADLINE};
use crate::updates::{download_deb, CommandOutput};
use agent_relay_node::relay::links::built_in_site_url;

mod cap_tests;

const SITE: &str = "https://site.example";
const FEED: &str = "https://site.example/cloud/api/v1/desktop/feed";
const STABLE: &str = "https://site.example/cloud/api/v1/desktop/feed/manifest?channel=stable";
const BETA: &str = "https://site.example/cloud/api/v1/desktop/feed/manifest?channel=beta";
const SIGNED: &str = "https://storage.googleapis.com/bucket/object?signature=1";
const TARBALL: &[u8] = b"a cloud feed tarball";
const DEB: &[u8] = b"a cloud feed package";
const ZIP: &[u8] = b"a cloud feed zip";
const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../desktop-core/tests/fixtures/desktop-feed/"
);

/// The fixtures' profile.
const PROFILE: Profile = Profile {
    product_name: "Example Desktop",
    releases_repository: "",
    asset_prefix: "Desktop-x7k2q",
    release_feed: CLOUD_RELEASE_FEED,
    cobrand: None,
};

type Respond<'a> = dyn Fn(&FeedRequest) -> Result<FeedResponse, FetchError> + 'a;

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A manifest fixture whose per-user archives and package are the small
/// payloads.
fn manifest_value(file: &str) -> Value {
    let path = format!("{FIXTURES}manifest/{file}");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    let mut value: Value = serde_json::from_str(&text).expect("fixture");
    for asset in value["assets"].as_array_mut().expect("assets") {
        let payload = match asset["kind"].as_str() {
            Some("tarball") => TARBALL,
            Some("deb") => DEB,
            Some("zip") => ZIP,
            _ => continue,
        };
        asset["size"] = json!(payload.len());
        asset["sha256"] = json!(sha(payload));
    }
    value
}

fn envelope_with(manifest: &Value, key: &SigningKey) -> Vec<u8> {
    let canonical = feed::jcs::canonicalize(manifest).expect("canonical manifest");
    let signature = URL_SAFE_NO_PAD.encode(key.sign(&canonical).to_bytes());
    serde_json::to_vec(&json!({
        "manifest": manifest,
        "signature": signature,
        "key_id": "fk_x7k2q_2026a",
    }))
    .expect("envelope")
}

fn envelope(file: &str) -> Vec<u8> {
    envelope_with(&manifest_value(file), &signing_key())
}

fn answer(status: u16, body: impl Into<Vec<u8>>) -> Result<FeedResponse, FetchError> {
    Ok(FeedResponse {
        status,
        body: body.into(),
    })
}

fn ok(body: impl Into<Vec<u8>>) -> Result<FeedResponse, FetchError> {
    answer(200, body)
}

fn signed_url(url: &str) -> Result<FeedResponse, FetchError> {
    ok(json!({"url": url, "expires_at": "2026-11-20T10:19:31Z"}).to_string())
}

fn unexpected(request: &FeedRequest) -> Result<FeedResponse, FetchError> {
    panic!("unexpected request: {} {}", request.method, request.url)
}

/// The usual site: the stable manifest, signed URLs on an allowed host.
fn site(request: &FeedRequest) -> Result<FeedResponse, FetchError> {
    match (request.method, request.url.as_str()) {
        ("GET", STABLE) => ok(envelope("valid-stable.json")),
        ("GET", BETA) => answer(404, "{}"),
        ("POST", url) if url == format!("{FEED}/assets/a_01JD3Q4W03/download-url") => {
            signed_url(&format!("{SIGNED}&asset=tarball"))
        }
        ("POST", url) if url == format!("{FEED}/assets/a_01JD3Q4W04/download-url") => {
            signed_url(&format!("{SIGNED}&asset=deb"))
        }
        ("POST", url) if url == format!("{FEED}/assets/a_01JD3Q4W06/download-url") => {
            signed_url(&format!("{SIGNED}&asset=zip"))
        }
        ("GET", url) if url == format!("{SIGNED}&asset=tarball") => ok(TARBALL),
        ("GET", url) if url == format!("{SIGNED}&asset=deb") => ok(DEB),
        ("GET", url) if url == format!("{SIGNED}&asset=zip") => ok(ZIP),
        _ => unexpected(request),
    }
}

struct Case<'a> {
    /// The feed's site; `None` keeps [`CloudFeed::built`]'s built-in site.
    site: Option<&'a str>,
    signed_in: bool,
    public_key: Option<&'a str>,
    installed: &'a str,
}

const CASE: Case<'static> = Case {
    site: Some(SITE),
    signed_in: true,
    public_key: None,
    installed: "2026.11.8",
};

/// Runs `body` against a site answering with `respond`; returns what it
/// returned and every request made. Each header fetch carries a new token
/// and must be for the feed's site.
fn run_with<T>(
    case: Case<'_>,
    respond: &Respond<'_>,
    body: impl FnOnce(&Host<'_>) -> T,
) -> (T, Vec<FeedRequest>) {
    let requests = RefCell::new(Vec::new());
    let value = {
        let tokens = Cell::new(0);
        let expected = case.site.unwrap_or(built_in_site_url());
        let headers = |site: &str| {
            assert_eq!(site, expected, "headers for the feed's site only");
            case.signed_in.then(|| {
                tokens.set(tokens.get() + 1);
                BTreeMap::from([
                    (
                        "Authorization".to_owned(),
                        format!("Bearer access-{}", tokens.get()),
                    ),
                    ("X-Agent-Relay-Profile".to_owned(), "x7k2q".to_owned()),
                    ("X-Agent-Relay-Desktop".to_owned(), "1".to_owned()),
                ])
            })
        };
        let compiled = STANDARD.encode(signing_key().verifying_key().to_bytes());
        let built = CloudFeed::built(&headers);
        let feed = CloudFeed {
            site: case.site.map_or(built.site.clone(), str::to_owned),
            profile: PROFILE,
            profile_id: "x7k2q",
            public_key: case.public_key.unwrap_or(&compiled),
            installed_version: case.installed,
            ..built
        };
        let request = |request: &FeedRequest| {
            requests.borrow_mut().push(request.clone());
            let response = respond(request)?;
            let body = answer_body(
                response.status,
                io::Cursor::new(response.body),
                request.max_body,
                REFUSAL_BODY_DEADLINE,
            )?;
            answer(response.status, body)
        };
        let fetch = |url: &str| -> Result<Vec<u8>, FetchError> {
            panic!("a feed release fetched {url} as from GitHub")
        };
        let run = |_: &[&str], _| -> io::Result<CommandOutput> { Err(io::Error::other("none")) };
        let env = |_: &str| None;
        let host = Host {
            home: PathBuf::from("/nonexistent"),
            env: &env,
            fetch: &fetch,
            run: &run,
            request: &request,
            feed: Some(&feed),
        };
        body(&host)
    };
    (value, requests.into_inner())
}

fn run<T>(respond: &Respond<'_>, body: impl FnOnce(&Host<'_>) -> T) -> (T, Vec<FeedRequest>) {
    run_with(CASE, respond, body)
}

fn check(host: &Host<'_>) -> Result<Option<Release>, UpdateError> {
    latest_on("x64", Platform::Linux, false, host)
}

fn checked(host: &Host<'_>) -> Release {
    check(host).expect("check").expect("a release")
}

fn urls(requests: &[FeedRequest]) -> Vec<String> {
    requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.url))
        .collect()
}

fn error(sentence: &str) -> UpdateError {
    UpdateError::new(sentence)
}

#[test]
fn a_signed_release_is_checked_downloaded_and_verified() {
    let directory = tempfile::tempdir().expect("tempdir");
    let ((release, archive, package, saved), requests) = run(&site, |host| {
        let release = checked(host);
        let archive = archive_bytes(&release, host);
        let package = package_bytes(&release, host);
        let saved = download_deb(&release, Some(directory.path()), host);
        (release, archive, package, saved)
    });
    assert_eq!(release.version, "2026.11.9");
    assert_eq!(release.tarball_url, format!("{FEED}/assets/a_01JD3Q4W03"));
    assert_eq!(
        release.deb_url.as_deref(),
        Some(format!("{FEED}/assets/a_01JD3Q4W04").as_str())
    );
    assert_eq!(
        release.page_url,
        format!("{SITE}/cloud/dashboard/downloads")
    );
    let found = release.feed.as_ref().expect("feed release");
    assert_eq!(
        found.archive.name,
        "Desktop-x7k2q-2026.11.9-Linux-x64.tar.gz"
    );
    assert_eq!(archive, Ok(TARBALL.to_vec()));
    assert_eq!(
        package,
        Ok((
            DEB.to_vec(),
            "Desktop-x7k2q-2026.11.9-Linux-x64.deb".to_owned()
        ))
    );
    let saved = saved.expect("saved package");
    assert_eq!(
        saved,
        directory
            .path()
            .join("Desktop-x7k2q-2026.11.9-Linux-x64.deb")
    );
    assert_eq!(std::fs::read(saved).expect("package"), DEB);
    // Feed requests carry the headers; a signed URL never does.
    for request in &requests {
        let to_site = request.url.starts_with(SITE);
        assert_eq!(!request.headers.is_empty(), to_site, "{request:?}");
        assert_eq!(request.body, None);
    }
    assert_eq!(
        requests[0]
            .headers
            .get("X-Agent-Relay-Profile")
            .map(String::as_str),
        Some("x7k2q")
    );
    assert_eq!(urls(&requests)[0], format!("GET {STABLE}"));
}

#[test]
fn the_built_feed_is_the_built_in_site_whatever_the_session() {
    let built_in = built_in_site_url().trim_end_matches('/');
    // The fake site, answering under the built-in site's address.
    let respond = |request: &FeedRequest| {
        let url = match request.url.strip_prefix(built_in) {
            Some(path) => format!("{SITE}{path}"),
            None => request.url.clone(),
        };
        site(&FeedRequest {
            url,
            ..request.clone()
        })
    };
    let at_build = Case { site: None, ..CASE };
    let ((release, archive), requests) = run_with(at_build, &respond, |host| {
        let release = checked(host);
        let archive = archive_bytes(&release, host);
        (release, archive)
    });
    assert_eq!(archive, Ok(TARBALL.to_vec()));
    assert_eq!(
        release.page_url,
        format!("{built_in}/cloud/dashboard/downloads")
    );
    // Every feed request went to the built-in site (headers for it are
    // asserted in `run_with`); only the signed URL went elsewhere, bare.
    assert!(!requests.is_empty());
    for request in &requests {
        let to_site = request
            .url
            .starts_with(&format!("{built_in}/cloud/api/v1/desktop/feed/"));
        assert_eq!(to_site, !request.url.starts_with(SIGNED), "{request:?}");
        assert_eq!(!request.headers.is_empty(), to_site, "{request:?}");
    }
}

#[test]
fn windows_reads_the_zip_and_no_package() {
    let ((release, archive), _) = run(&site, |host| {
        let release = latest_on("arm64", Platform::Windows, false, host)
            .expect("check")
            .expect("a release");
        let archive = archive_bytes(&release, host);
        (release, archive)
    });
    let found = release.feed.as_ref().expect("feed release");
    assert_eq!(
        found.archive.name,
        "Desktop-x7k2q-2026.11.9-Windows-arm64.zip"
    );
    assert_eq!(
        (found.package.clone(), release.deb_url.clone()),
        (None, None)
    );
    assert_eq!(archive, Ok(ZIP.to_vec()));
    // No Windows x64 zip in this release.
    let (missing, _) = run(&site, |host| {
        latest_on("x64", Platform::Windows, false, host)
    });
    assert_eq!(missing, Err(error("There is no update for this computer.")));
}

#[test]
fn a_beta_subscriber_gets_the_newer_of_beta_and_stable() {
    let both = |request: &FeedRequest| match request.url.as_str() {
        BETA => ok(envelope("valid-beta.json")),
        _ => site(request),
    };
    let (release, requests) = run(&both, |host| latest_on("x64", Platform::Linux, true, host));
    assert_eq!(
        release.expect("check").expect("a release").version,
        "2026.11.10-beta.2"
    );
    assert_eq!(
        urls(&requests),
        [format!("GET {STABLE}"), format!("GET {BETA}")]
    );
    // No beta: the stable release.
    let (release, _) = run(&site, |host| latest_on("x64", Platform::Linux, true, host));
    assert_eq!(
        release.expect("check").expect("a release").version,
        "2026.11.9"
    );
    // A broken beta manifest fails the check rather than being passed over.
    let broken = |request: &FeedRequest| match request.url.as_str() {
        BETA => ok(&b"{}"[..]),
        _ => site(request),
    };
    let (release, _) = run(&broken, |host| {
        latest_on("x64", Platform::Linux, true, host)
    });
    assert_eq!(release, Err(error(FEED_UNVERIFIED)));
}

#[test]
fn a_rejected_sign_in_is_renewed_once() {
    let tries = Cell::new(0);
    let once = |request: &FeedRequest| {
        tries.set(tries.get() + 1);
        match tries.get() {
            1 => answer(401, "{}"),
            _ => site(request),
        }
    };
    let (release, requests) = run(&once, checked);
    assert_eq!(release.version, "2026.11.9");
    let tokens: Vec<_> = requests
        .iter()
        .map(|request| request.headers.get("Authorization").cloned())
        .collect();
    assert_eq!(
        tokens,
        [
            Some("Bearer access-1".to_owned()),
            Some("Bearer access-2".to_owned())
        ]
    );
}

#[test]
fn a_second_rejection_or_no_sign_in_asks_to_sign_in() {
    let rejected = |_: &FeedRequest| answer(401, "{}");
    let (result, requests) = run(&rejected, check);
    assert_eq!(result, Err(error(SIGN_IN)));
    assert_eq!(requests.len(), 2);
    let signed_out = Case {
        signed_in: false,
        ..CASE
    };
    let (result, requests) = run_with(signed_out, &unexpected, check);
    assert_eq!(result, Err(error(SIGN_IN)));
    assert!(requests.is_empty());
    // A build reading a cloud feed without one handed to it.
    let (result, _) = run(&unexpected, |host| {
        check(&Host {
            feed: None,
            home: host.home.clone(),
            ..*host
        })
    });
    assert_eq!(result, Err(error(SIGN_IN)));
    assert!(SIGN_IN.starts_with("Sign in to ") && SIGN_IN.ends_with(" to receive updates."));
}

#[test]
fn a_workspace_without_entitlement_is_told_so() {
    let refused = |_: &FeedRequest| answer(403, r#"{"error":"not_entitled"}"#);
    let (result, _) = run(&refused, check);
    assert_eq!(result, Err(error(NOT_ENTITLED)));
    assert!(NOT_ENTITLED.starts_with("This workspace is not entitled to updates for "));
    // Any other refusal is unreadable.
    let forbidden = |_: &FeedRequest| answer(403, r#"{"error":"forbidden"}"#);
    let (result, _) = run(&forbidden, check);
    assert_eq!(result, Err(error(UNREADABLE)));
    // The same for a download.
    let (result, _) = run(&site, |host| {
        let release = checked(host);
        let refused = |request: &FeedRequest| match request.method {
            "POST" => answer(403, r#"{"error":"not_entitled"}"#),
            _ => unexpected(request),
        };
        archive_bytes(
            &release,
            &Host {
                request: &refused,
                home: host.home.clone(),
                ..*host
            },
        )
    });
    assert_eq!(result, Err(error(NOT_ENTITLED)));
}

#[test]
fn no_release_or_no_newer_one_is_up_to_date() {
    let none = |_: &FeedRequest| answer(404, r#"{"error":"not_found"}"#);
    let (result, _) = run(&none, check);
    assert_eq!(result, Ok(None));
    let current = Case {
        installed: "2026.11.9",
        ..CASE
    };
    let (result, _) = run_with(current, &site, check);
    assert_eq!(result, Ok(None));
    let (result, _) = run(&|_| answer(500, "{}"), check);
    assert_eq!(result, Err(error(UNREADABLE)));
    let (result, _) = run(&|_| Err(FetchError::Unreachable("offline".into())), check);
    assert_eq!(result, Err(error(UNREADABLE)));
}

#[test]
fn an_unverified_manifest_downloads_nothing() {
    let mut tampered = manifest_value("valid-stable.json");
    let signed = envelope_with(&tampered, &signing_key());
    tampered["assets"][2]["sha256"] = json!(sha(b"something else"));
    let mut forged: Value = serde_json::from_slice(&signed).expect("envelope");
    forged["manifest"] = tampered;
    let forged = serde_json::to_vec(&forged).expect("forged");
    let other_key = envelope_with(
        &manifest_value("valid-stable.json"),
        &SigningKey::from_bytes(&[9; 32]),
    );
    for body in [forged, other_key] {
        let serve = |request: &FeedRequest| match request.url.as_str() {
            STABLE => ok(body.clone()),
            _ => unexpected(request),
        };
        let (result, requests) = run(&serve, check);
        assert_eq!(result, Err(error(FEED_UNVERIFIED)));
        assert_eq!(urls(&requests), [format!("GET {STABLE}")]);
    }
    // Without a usable compiled key nothing is even asked for.
    let keyless = Case {
        public_key: Some(""),
        ..CASE
    };
    let (result, requests) = run_with(keyless, &unexpected, check);
    assert_eq!(result, Err(error(FEED_UNVERIFIED)));
    assert!(requests.is_empty());
}

#[test]
fn a_download_url_on_a_host_the_manifest_does_not_allow_is_refused() {
    for url in [
        "https://elsewhere.example/object",
        "http://storage.googleapis.com/object",
        "https://storage.googleapis.com:8443/object",
        "not a url",
    ] {
        let (result, requests) = run(&site, |host| {
            let release = checked(host);
            let elsewhere = |request: &FeedRequest| match request.method {
                "POST" => signed_url(url),
                _ => unexpected(request),
            };
            archive_bytes(
                &release,
                &Host {
                    request: &elsewhere,
                    home: host.home.clone(),
                    ..*host
                },
            )
        });
        assert_eq!(result, Err(error(FEED_UNVERIFIED)), "{url}");
        assert_eq!(requests.len(), 1, "{url}: only the check reached the site");
    }
    // The site's own host is always allowed.
    let (result, _) = run(&site, |host| {
        let release = checked(host);
        let own = format!("{SITE}/signed/object");
        let serve = |request: &FeedRequest| match (request.method, request.url.as_str()) {
            ("POST", _) => signed_url(&own),
            ("GET", url) if url == own && request.headers.is_empty() => ok(TARBALL),
            _ => unexpected(request),
        };
        archive_bytes(
            &release,
            &Host {
                request: &serve,
                home: host.home.clone(),
                ..*host
            },
        )
    });
    assert_eq!(result, Ok(TARBALL.to_vec()));
}

#[test]
fn a_site_without_signed_urls_streams_the_asset() {
    let stream = format!("{FEED}/assets/a_01JD3Q4W03");
    let streaming = |request: &FeedRequest| match (request.method, request.url.as_str()) {
        ("POST", _) => answer(501, "{}"),
        ("GET", url) if url == stream => ok(TARBALL),
        _ => site(request),
    };
    let (result, requests) = run(&streaming, |host| archive_bytes(&checked(host), host));
    assert_eq!(result, Ok(TARBALL.to_vec()));
    let last = requests.last().expect("stream request");
    assert_eq!(
        format!("{} {}", last.method, last.url),
        format!("GET {stream}")
    );
    assert!(last.headers.contains_key("Authorization"));
}

#[test]
fn bytes_that_do_not_match_the_manifest_are_discarded() {
    let same_length = vec![b'x'; TARBALL.len()];
    let shorter = TARBALL[1..].to_vec();
    for served in [same_length, shorter] {
        let serve = |request: &FeedRequest| match (request.method, request.url.as_str()) {
            ("GET", url) if url.starts_with(SIGNED) => ok(served.clone()),
            _ => site(request),
        };
        let (result, _) = run(&serve, |host| archive_bytes(&checked(host), host));
        assert_eq!(result, Err(error(UNVERIFIED)));
    }
}
