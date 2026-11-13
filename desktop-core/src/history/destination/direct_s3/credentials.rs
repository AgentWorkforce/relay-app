//! Control's upload-credentials route: what is asked, what an answer must
//! hold before a batch is put with it, and how a refused ask is told apart.
//! Nothing here is ever logged: keys, the session token, presigned URLs and
//! the witness only reach the request they sign or are.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The upload contract this build speaks; an answer naming another is
/// newer or older than this build, and the batch is posted instead.
pub(super) const CONTRACT: &str = "relay-upload-inbox-v1";

/// The kind of credentials asked for. Control answers an ask with the kind
/// asked for or an error, never another kind, so an answer of another kind
/// is unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// Session keys that sign each PUT; asked for first.
    Sts,
    /// One presigned PUT URL per named key, for a store without STS.
    Presigned,
}
impl Kind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Sts => "sts",
            Self::Presigned => "presigned",
        }
    }
}

/// The longest witness an object's metadata carries.
const MAX_WITNESS_BYTES: usize = 1024;
/// The largest object the receiver admits (its `MAX_DELIVERY_BYTES`); a
/// larger `maxObjectBytes` is held to it.
pub(super) const MAX_OBJECT_BYTES: u64 = 2_097_152;
/// The largest witness seq a key may name: the receiver reads it as a
/// JavaScript safe integer.
const MAX_WITNESS_SEQ: u64 = (1 << 53) - 1;
/// The longest `endpoint` and `kmsKeyId` an answer may carry.
const MAX_ENDPOINT: usize = 2048;
const MAX_KMS_KEY_ID: usize = 2048;
/// The most presigned URLs an answer may carry, and the longest each.
const MAX_URLS: usize = 64;
const MAX_URL: usize = 8192;

/// The body of `POST …/relayhistory/upload-credentials`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GrantRequest<'a> {
    pub(super) origin_id: &'a str,
    pub(super) kind: &'static str,
    /// The one object a presigned ask names; never sent with `sts`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) keys: Option<[KeyRequest; 1]>,
}
/// A key relative to `keyPrefix` and the base64 SHA-256 of its bytes,
/// which Control signs into the URL as `x-amz-checksum-sha256`.
#[derive(Serialize)]
pub(super) struct KeyRequest {
    pub(super) key: String,
    pub(super) sha256: String,
}

/// An answer as Control sends it, before it is checked.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    contract: Option<String>,
    kind: String,
    bucket: String,
    region: String,
    #[serde(default)]
    endpoint: Option<String>,
    key_prefix: String,
    /// Always present; `null` only with `presigned` (a store without KMS).
    kms_key_id: Option<String>,
    max_object_bytes: u64,
    refresh_after: String,
    witness: String,
    witness_seq: Option<u64>,
    #[serde(default)]
    credentials: Option<RawKeys>,
    #[serde(default)]
    urls: Option<Vec<RawUrl>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUrl {
    key: String,
    url: String,
    expires_at: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawKeys {
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
    expires_at: String,
}

/// A checked answer: where the workspace's upload inbox is and what a PUT
/// into it must carry.
#[derive(Clone)]
pub(super) struct Grant {
    pub(super) bucket: String,
    pub(super) region: String,
    /// A path-style endpoint in place of the bucket's AWS host: an http(s)
    /// origin with an optional path prefix, checked by [`endpoint_valid`].
    pub(super) endpoint: Option<String>,
    /// The upload inbox prefix, used verbatim.
    pub(super) key_prefix: String,
    /// `None` for a store without KMS: no SSE header is sent or signed.
    pub(super) kms_key_id: Option<String>,
    pub(super) witness: String,
    pub(super) witness_seq: u64,
    pub(super) max_object_bytes: u64,
    /// When to ask again: `refreshAfter`, or the earliest expiry if sooner.
    pub(super) refresh_at: DateTime<Utc>,
    pub(super) access: Access,
}
/// What authorizes a PUT: session keys that sign it, or the URLs Control
/// presigned for the keys it was asked about.
#[derive(Clone)]
pub(super) enum Access {
    Sts(Keys),
    Presigned(Vec<Presigned>),
}
/// A presigned PUT URL for `key` (relative to `keyPrefix`). The URL is a
/// credential: memory only, never logged or stored.
#[derive(Clone)]
pub(super) struct Presigned {
    pub(super) key: String,
    pub(super) url: String,
    pub(super) expires_at: DateTime<Utc>,
}
#[derive(Clone)]
pub(super) struct Keys {
    pub(super) access_key_id: String,
    pub(super) secret_access_key: String,
    pub(super) session_token: String,
}
impl std::fmt::Debug for Grant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Grant")
            .field("bucket", &self.bucket)
            .field("key_prefix", &self.key_prefix)
            .field("refresh_at", &self.refresh_at)
            .finish_non_exhaustive()
    }
}
impl Grant {
    /// Whether it may still be used at `now`.
    pub(super) fn fresh(&self, now: DateTime<Utc>) -> bool {
        now < self.refresh_at
    }
    #[cfg(test)]
    pub(super) fn kind(&self) -> Kind {
        match self.access {
            Access::Sts(_) => Kind::Sts,
            Access::Presigned(_) => Kind::Presigned,
        }
    }
    /// The unexpired presigned URL for `key` (relative to `keyPrefix`).
    pub(super) fn url_for(&self, key: &str, now: DateTime<Utc>) -> Option<&str> {
        let Access::Presigned(urls) = &self.access else {
            return None;
        };
        urls.iter()
            .find(|presigned| presigned.key == key && now < presigned.expires_at)
            .map(|presigned| presigned.url.as_str())
    }
    /// Whether it can put the object `key` (relative to `keyPrefix`):
    /// session keys put any key, presigned URLs only their own.
    pub(super) fn covers(&self, key: &str, now: DateTime<Utc>) -> bool {
        match self.access {
            Access::Sts(_) => true,
            Access::Presigned(_) => self.url_for(key, now).is_some(),
        }
    }
}

/// Why Control's answer is not used for this batch; the batch is posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Declined {
    /// 404 `history_upload_direct_off`: direct upload is off for this user
    /// or workspace.
    Off,
    /// 429 or 503, with the time `Retry-After` asked to wait until.
    Busy(Option<i64>),
    /// 501 `unsupported_by_backend`: the store has no STS, so the same ask
    /// is made again with kind `presigned`.
    PresignedOnly,
    /// Any other answer, or one that fails the checks below.
    Unusable(&'static str),
}
impl std::fmt::Display for Declined {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => formatter.write_str("direct upload is off"),
            Self::Busy(_) => formatter.write_str("upload credentials are busy"),
            Self::PresignedOnly => formatter.write_str("only presigned uploads are offered"),
            Self::Unusable(cause) => write!(formatter, "upload credentials unusable: {cause}"),
        }
    }
}
impl std::error::Error for Declined {}

/// Check an answer to an ask for `kind`. The reason a check fails is a
/// fixed string, safe to log. Presigned URLs whose key names another
/// witness seq than the answer's are kept but never match an object key
/// built on `witnessSeq`, so no such URL is ever put to.
pub(super) fn parse(answer: serde_json::Value, kind: Kind) -> Result<Grant, &'static str> {
    // `kmsKeyId` is always present, even as `null`.
    check(answer.get("kmsKeyId").is_some(), "kmsKeyId")?;
    let raw: Raw = serde_json::from_value(answer).map_err(|_| "unparseable")?;
    if raw.contract.as_deref() != Some(CONTRACT) {
        return Err("contract");
    }
    check(raw.kind == kind.name(), "kind")?;
    let witness_seq = raw.witness_seq.ok_or("witnessSeq")?;
    check(witness_seq <= MAX_WITNESS_SEQ, "witnessSeq")?;
    fields_safe(&raw, kind)?;
    let refresh_after = time(&raw.refresh_after).ok_or("refreshAfter")?;
    let (access, expires_at) = match kind {
        Kind::Sts => sts(raw.credentials.ok_or("credentials")?)?,
        Kind::Presigned => presigned(raw.urls.ok_or("urls")?)?,
    };
    Ok(Grant {
        bucket: raw.bucket,
        region: raw.region,
        endpoint: raw.endpoint,
        key_prefix: raw.key_prefix,
        kms_key_id: raw.kms_key_id,
        witness: raw.witness,
        witness_seq,
        max_object_bytes: raw.max_object_bytes.min(MAX_OBJECT_BYTES),
        refresh_at: refresh_after.min(expires_at),
        access,
    })
}

/// The answer's fields that go into headers, keys and URLs, each as the
/// contract bounds it. A null `kmsKeyId` goes only with presigned URLs.
fn fields_safe(raw: &Raw, kind: Kind) -> Result<(), &'static str> {
    check(
        header_safe(&raw.witness) && raw.witness.len() <= MAX_WITNESS_BYTES,
        "witness",
    )?;
    check(
        match &raw.kms_key_id {
            Some(id) => header_safe(id) && id.len() <= MAX_KMS_KEY_ID,
            None => kind == Kind::Presigned,
        },
        "kmsKeyId",
    )?;
    check(
        raw.endpoint.as_deref().is_none_or(endpoint_valid),
        "endpoint",
    )?;
    check(key_prefix_safe(&raw.key_prefix), "keyPrefix")?;
    check(bucket_safe(&raw.bucket), "bucket")?;
    check(region_safe(&raw.region), "region")?;
    check(raw.max_object_bytes > 0, "maxObjectBytes")
}

/// Session keys and when they expire.
fn sts(keys: RawKeys) -> Result<(Access, DateTime<Utc>), &'static str> {
    let expires_at = time(&keys.expires_at).ok_or("expiresAt")?;
    check(
        [
            &keys.access_key_id,
            &keys.secret_access_key,
            &keys.session_token,
        ]
        .iter()
        .all(|value| header_safe(value)),
        "credentials",
    )?;
    let keys = Keys {
        access_key_id: keys.access_key_id,
        secret_access_key: keys.secret_access_key,
        session_token: keys.session_token,
    };
    Ok((Access::Sts(keys), expires_at))
}

/// Presigned URLs (1 to 64, distinct keys, each a key Control's rule takes
/// and an absolute http(s) URL with no userinfo or fragment) and the
/// earliest of their expiries.
fn presigned(urls: Vec<RawUrl>) -> Result<(Access, DateTime<Utc>), &'static str> {
    check((1..=MAX_URLS).contains(&urls.len()), "urls")?;
    let mut checked: Vec<Presigned> = Vec::with_capacity(urls.len());
    for raw in urls {
        let expires_at = time(&raw.expires_at).ok_or("expiresAt")?;
        check(
            super::relative_key_valid(&raw.key)
                && !checked.iter().any(|seen| seen.key == raw.key)
                && url_valid(&raw.url),
            "urls",
        )?;
        checked.push(Presigned {
            key: raw.key,
            url: raw.url,
            expires_at,
        });
    }
    let earliest = checked
        .iter()
        .map(|presigned| presigned.expires_at)
        .min()
        .ok_or("urls")?;
    Ok((Access::Presigned(checked), earliest))
}

/// A presigned URL as the contract allows it: printable, at most 8192
/// characters, absolute http(s) with a host, no userinfo and no fragment.
fn url_valid(value: &str) -> bool {
    value.len() <= MAX_URL
        && header_safe(value)
        && !value.contains('#')
        && url::Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

/// An `endpoint` as the contract allows it: an http(s) origin
/// `scheme://host[:port]`, bare (with or without its `/`) or with a path
/// prefix, at most 2048 characters, with no userinfo, query, fragment (not
/// even an empty `?` or `#`), backslash, dot segment or trailing slash
/// after a path prefix.
pub(super) fn endpoint_valid(value: &str) -> bool {
    if value.len() > MAX_ENDPOINT || !header_safe(value) || value.contains(['?', '#', '\\']) {
        return false;
    }
    let Some((_, rest)) = value.split_once("://") else {
        return false;
    };
    let dot_segment = rest.split('/').skip(1).any(|segment| {
        let segment = segment.to_ascii_lowercase().replace("%2e", ".");
        segment == "." || segment == ".."
    });
    if dot_segment {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let path = url.path();
    let path_ok = path == "/"
        || (value.ends_with(path)
            && path.split('/').skip(1).all(|segment| {
                !segment.is_empty()
                    && segment
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._~!$&'()*+,;=:@%-".contains(&b))
            }));
    matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && path_ok
}

fn check(condition: bool, cause: &'static str) -> Result<(), &'static str> {
    if condition {
        Ok(())
    } else {
        Err(cause)
    }
}

fn time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

/// A value that can go in a header as it is: printable ASCII, no spaces.
fn header_safe(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// A prefix the key is built on verbatim: relative, ending in `/`, with no
/// empty or parent segment and nothing a URL would read as a query,
/// fragment or escape.
fn key_prefix_safe(prefix: &str) -> bool {
    header_safe(prefix)
        && prefix.ends_with('/')
        && !prefix.starts_with('/')
        && !prefix.contains("//")
        && !prefix.split('/').any(|segment| segment == "..")
        && !prefix.contains(['?', '#', '%', '\\'])
}

fn bucket_safe(bucket: &str) -> bool {
    (3..=63).contains(&bucket.len())
        && bucket
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
}

fn region_safe(region: &str) -> bool {
    (1..=64).contains(&region.len())
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer() -> serde_json::Value {
        serde_json::json!({
            "contract": CONTRACT,
            "kind": "sts",
            "bucket": "fixture-bucket",
            "region": "us-east-1",
            "endpoint": null,
            "keyPrefix": "fixture-inbox/ws=ws-fixture/u=u1/",
            "kmsKeyId": "fixture-kms-key",
            "credentials": {
                "accessKeyId": "AKIDFIXTURE",
                "secretAccessKey": "fixture-secret",
                "sessionToken": "fixture-session-token",
                "expiresAt": "2030-01-01T01:00:00Z"
            },
            "witness": "fixture.witness",
            "witnessSeq": 7,
            "maxObjectBytes": 2_097_152,
            "refreshAfter": "2030-01-01T00:45:00Z"
        })
    }

    #[test]
    fn an_sts_answer_refreshes_at_the_sooner_of_its_times() {
        let grant = parse(answer(), Kind::Sts).unwrap();
        assert_eq!(grant.witness_seq, 7);
        assert_eq!(grant.refresh_at, time("2030-01-01T00:45:00Z").unwrap());
        let mut sooner = answer();
        sooner["credentials"]["expiresAt"] = "2030-01-01T00:30:00Z".into();
        assert_eq!(
            parse(sooner, Kind::Sts).unwrap().refresh_at,
            time("2030-01-01T00:30:00Z").unwrap()
        );
    }

    #[test]
    fn an_answer_missing_what_a_put_needs_is_unusable() {
        let cases: [(&str, serde_json::Value, &str); 8] = [
            ("contract", "relay-upload-inbox-v2".into(), "contract"),
            ("witnessSeq", serde_json::Value::Null, "witnessSeq"),
            ("witnessSeq", 9_007_199_254_740_992_u64.into(), "witnessSeq"),
            ("region", "a".repeat(65).into(), "region"),
            ("witness", "has space".into(), "witness"),
            ("keyPrefix", "../escape/".into(), "keyPrefix"),
            ("keyPrefix", "no-trailing-slash".into(), "keyPrefix"),
            ("bucket", "Fixture_Bucket".into(), "bucket"),
        ];
        for (field, value, cause) in cases {
            let mut changed = answer();
            changed[field] = value;
            assert_eq!(parse(changed, Kind::Sts).unwrap_err(), cause, "{field}");
        }
        let mut unnamed = answer();
        unnamed.as_object_mut().unwrap().remove("contract");
        assert_eq!(parse(unnamed, Kind::Sts).unwrap_err(), "contract");
    }

    #[test]
    fn the_largest_safe_seq_is_kept_and_object_size_is_held_to_the_receivers() {
        let mut answer = answer();
        answer["witnessSeq"] = 9_007_199_254_740_991_u64.into();
        answer["maxObjectBytes"] = 8_388_608.into();
        let grant = parse(answer, Kind::Sts).unwrap();
        assert_eq!(grant.witness_seq, 9_007_199_254_740_991);
        assert_eq!(grant.max_object_bytes, MAX_OBJECT_BYTES);
    }

    fn presigned_answer() -> serde_json::Value {
        let mut answer = answer();
        let object = answer.as_object_mut().unwrap();
        object.remove("credentials");
        object.insert("kind".into(), "presigned".into());
        object.insert("kmsKeyId".into(), serde_json::Value::Null);
        object.insert(
            "endpoint".into(),
            "https://objects.example.test:9000/relayhistory".into(),
        );
        object.insert(
            "urls".into(),
            serde_json::json!([{
                "key": "o=origin-1/b=1/d=2-w7.json.zst",
                "url": "https://objects.example.test:9000/relayhistory/fixture-bucket/x?X-Amz-Signature=fixture-signature",
                "expiresAt": "2030-01-01T00:30:00Z"
            }]),
        );
        answer
    }

    #[test]
    fn an_answer_of_another_kind_than_asked_or_without_keys_is_unusable() {
        assert_eq!(parse(presigned_answer(), Kind::Sts).unwrap_err(), "kind");
        assert_eq!(parse(answer(), Kind::Presigned).unwrap_err(), "kind");
        let mut keyless = answer();
        keyless.as_object_mut().unwrap().remove("credentials");
        assert_eq!(parse(keyless, Kind::Sts).unwrap_err(), "credentials");
        let mut urlless = presigned_answer();
        urlless.as_object_mut().unwrap().remove("urls");
        assert_eq!(parse(urlless, Kind::Presigned).unwrap_err(), "urls");
    }

    #[test]
    fn a_presigned_answer_covers_only_its_unexpired_keys() {
        let grant = parse(presigned_answer(), Kind::Presigned).unwrap();
        assert_eq!(grant.kind(), Kind::Presigned);
        assert_eq!(grant.kms_key_id, None);
        // The URL's expiry is sooner than refreshAfter.
        assert_eq!(grant.refresh_at, time("2030-01-01T00:30:00Z").unwrap());
        let before = time("2030-01-01T00:00:00Z").unwrap();
        assert!(grant.covers("o=origin-1/b=1/d=2-w7.json.zst", before));
        assert!(!grant.covers("o=origin-1/b=1/d=2-w8.json.zst", before));
        assert!(!grant.covers("o=origin-1/b=1/d=2-w7.json.zst", grant.refresh_at));
        let mut twice = presigned_answer();
        let url = twice["urls"][0].clone();
        twice["urls"].as_array_mut().unwrap().push(url);
        assert_eq!(parse(twice, Kind::Presigned).unwrap_err(), "urls");
        for url in [
            "https://user:pass@objects.example.test/x",
            "https://objects.example.test/x#frag",
            "ftp://objects.example.test/x",
            "/relative",
        ] {
            let mut changed = presigned_answer();
            changed["urls"][0]["url"] = url.into();
            assert_eq!(
                parse(changed, Kind::Presigned).unwrap_err(),
                "urls",
                "{url}"
            );
        }
    }

    #[test]
    fn a_null_kms_key_is_taken_only_with_presigned_and_must_be_present() {
        let mut keyless = answer();
        keyless["kmsKeyId"] = serde_json::Value::Null;
        assert_eq!(parse(keyless, Kind::Sts).unwrap_err(), "kmsKeyId");
        let mut absent = presigned_answer();
        absent.as_object_mut().unwrap().remove("kmsKeyId");
        assert_eq!(parse(absent, Kind::Presigned).unwrap_err(), "kmsKeyId");
        let mut long = answer();
        long["kmsKeyId"] = "k".repeat(2049).into();
        assert_eq!(parse(long, Kind::Sts).unwrap_err(), "kmsKeyId");
    }

    #[test]
    fn an_endpoint_is_an_origin_with_an_optional_path_prefix() {
        for endpoint in [
            "https://objects.example.test:9000/relayhistory",
            "https://objects.example.test:9000/a/b",
            "https://objects.example.test",
            "https://objects.example.test/",
            "http://127.0.0.1:9000",
        ] {
            assert!(endpoint_valid(endpoint), "{endpoint}");
        }
        let long = format!("https://objects.example.test/{}", "a".repeat(2048));
        for endpoint in [
            "https://objects.example.test:9000/relayhistory/",
            "https://objects.example.test:9000/relayhistory?",
            "https://objects.example.test/?x=1",
            "https://objects.example.test/#",
            "https://user:pass@objects.example.test",
            "https://user@objects.example.test",
            "https://objects.example.test/a/../b",
            "https://objects.example.test/a/./b",
            "https://objects.example.test/a/%2E%2e/b",
            "https://objects.example.test/a\\b",
            "https://objects.example.test//a",
            "ftp://objects.example.test",
            "objects.example.test",
            long.as_str(),
        ] {
            assert!(!endpoint_valid(endpoint), "{endpoint}");
        }
        let mut answer = answer();
        answer["endpoint"] = "https://objects.example.test/a/".into();
        assert_eq!(parse(answer, Kind::Sts).unwrap_err(), "endpoint");
    }

    #[test]
    fn its_debug_never_shows_keys_or_the_witness() {
        let shown = format!("{:?}", parse(answer(), Kind::Sts).unwrap());
        for secret in [
            "fixture-secret",
            "fixture-session-token",
            "fixture.witness",
            "AKIDFIXTURE",
        ] {
            assert!(!shown.contains(secret), "{shown}");
        }
    }
}
