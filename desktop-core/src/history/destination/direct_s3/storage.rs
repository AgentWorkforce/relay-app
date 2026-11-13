//! The PUT that stores a batch's object: where it goes, the headers it
//! carries (signed with session credentials, or by the presigned URL that
//! is put to as it is) and what the bucket's answer means for the batch.
use super::super::{http_error, retry_after, DeliveryFailure, ReceiverRejection, Result};
use super::{
    allowed,
    credentials::{Access, Grant, Keys},
    fresh_agent, http_agent,
    private_storage::{fresh_storage_agent, storage_agent, url_listed},
    resend_once, sigv4, Call, Upload,
};
use chrono::Utc;
use std::{io::Read, time::Duration};

/// The most of an S3 error body read to find its code.
const MAX_ERROR_XML: u64 = 4096;

/// What S3 said to a PUT.
pub(super) enum Put {
    /// Stored now (200) or before (412: the object exists, and its key names
    /// its bytes).
    Stored,
    /// The credentials expired or were revoked (dropped already).
    Expired,
    /// Not sent: the answer names no usable storage URL, so it is posted.
    Unsent,
}

pub(super) fn put(call: &Call<'_>, grant: &Grant, upload: &Upload, key: &str) -> Result<Put> {
    let Some((url, headers)) = target(grant, upload, key).filter(|(url, _)| allowed(url)) else {
        call.note("storage_url");
        return Ok(Put::Unsent);
    };
    let send = |agent: &ureq::Agent, timeout: Duration| {
        let request = headers.iter().fold(
            agent.put(&url).timeout(timeout),
            |request, (name, value)| request.set(name, value),
        );
        request.send_bytes(&upload.body).map_err(Box::new)
    };
    let timeout = call.budget.remaining()?;
    // A site build's listed private store gets the storage-only agents;
    // every other PUT the History agents, as before.
    let agents = match url_listed(&url) {
        true => [storage_agent(), fresh_storage_agent()],
        false => [http_agent(), fresh_agent()],
    };
    let sent = resend_once(&agents, &send, timeout, &|| call.budget.remaining());
    match sent.map_err(|error| *error) {
        Ok(response) if response.status() == 200 => Ok(Put::Stored),
        Ok(response) => Err(unstored(response.status(), None)),
        Err(ureq::Error::Status(412, _)) => Ok(Put::Stored),
        Err(ureq::Error::Status(403, response)) => {
            // Whatever S3 refused, the next attempt asks for fresh
            // credentials rather than putting with these again.
            call.remember(|entry| entry.grant = None);
            let at = retry_after::of(403, &response);
            match expired(response) {
                true => Ok(Put::Expired),
                false => Err(unstored(403, at)),
            }
        }
        Err(ureq::Error::Status(status, response)) => {
            Err(unstored(status, retry_after::of(status, &response)))
        }
        Err(error) => Err(http_error(error, "")),
    }
}

/// S3 answered a PUT with neither 200 nor 412: the batch stays queued and
/// is retried on the backoff schedule, never isolated or posted for it.
pub(super) fn unstored(status: u16, retry_after_ms: Option<i64>) -> anyhow::Error {
    let failure = match status {
        408 | 425 | 429 => DeliveryFailure::RateLimited,
        _ => DeliveryFailure::Transient,
    };
    ReceiverRejection {
        failure,
        status,
        recovery: None,
        retry_after_ms,
        hold: None,
    }
    .into()
}

/// Whether an S3 403 names expired or invalid credentials, or a request
/// signed with credentials that expired on the way (`AccessDenied`,
/// "Request has expired").
fn expired(response: ureq::Response) -> bool {
    let mut text = String::new();
    let read = response
        .into_reader()
        .take(MAX_ERROR_XML)
        .read_to_string(&mut text);
    read.is_ok()
        && (["<Code>ExpiredToken</Code>", "<Code>InvalidToken</Code>"]
            .iter()
            .any(|code| text.contains(code))
            || text.contains("<Code>AccessDenied</Code>") && text.contains("Request has expired"))
}

type Headers = Vec<(&'static str, String)>;

/// Where the PUT goes and the headers it carries. Session credentials sign
/// `host` and every `x-amz-*` header; a presigned URL already signs its
/// own (Control signed the same headers into it) and is put to as it is.
/// The SSE headers go only with a `kmsKeyId`: a store without KMS gets
/// neither, signed or sent.
pub(super) fn target(grant: &Grant, upload: &Upload, key: &str) -> Option<(String, Headers)> {
    let mut headers: Headers = vec![
        ("if-none-match", "*".into()),
        ("x-amz-checksum-sha256", upload.checksum()),
        ("x-amz-meta-relay-witness", grant.witness.clone()),
    ];
    if let Some(kms_key_id) = &grant.kms_key_id {
        headers.extend([
            ("x-amz-server-side-encryption", "aws:kms".into()),
            (
                "x-amz-server-side-encryption-aws-kms-key-id",
                kms_key_id.clone(),
            ),
        ]);
    }
    let url = match &grant.access {
        Access::Sts(keys) => sign(grant, keys, upload, key, &mut headers)?,
        Access::Presigned(_) => {
            let relative = key.strip_prefix(&grant.key_prefix)?;
            let url = grant.url_for(relative, Utc::now())?.to_string();
            headers.push(("host", authority(&url::Url::parse(&url).ok()?)?));
            url
        }
    };
    headers.push(("content-encoding", "zstd".into()));
    headers.push(("content-type", "application/json".into()));
    Some((url, headers))
}

/// The URL of `key` for session credentials, with `headers` signed. A
/// non-null `endpoint` is path-style: `<endpoint>/<bucket>/<key>`, its
/// path prefix kept; a prefix with anything but unreserved characters
/// would be signed in another form than it is sent, so it is not put to.
fn sign(
    grant: &Grant,
    keys: &Keys,
    upload: &Upload,
    key: &str,
    headers: &mut Headers,
) -> Option<String> {
    let (parsed, path) = match &grant.endpoint {
        Some(endpoint) => {
            let parsed = url::Url::parse(endpoint).ok()?;
            let prefix = parsed.path().trim_end_matches('/');
            let plain = prefix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~/".contains(&b));
            if !plain {
                return None;
            }
            let path = format!("{prefix}/{}/{key}", grant.bucket);
            (parsed, path)
        }
        None => (
            url::Url::parse(&format!(
                "https://{}.s3.{}.amazonaws.com",
                grant.bucket, grant.region
            ))
            .ok()?,
            format!("/{key}"),
        ),
    };
    if parsed.query().is_some() {
        return None;
    }
    let base = parsed.origin().ascii_serialization();
    let now = Utc::now();
    let payload = sigv4::hex(&upload.sha256);
    headers.extend([
        ("host", authority(&parsed)?),
        ("x-amz-content-sha256", payload.clone()),
        ("x-amz-date", sigv4::amz_date(now)),
        ("x-amz-security-token", keys.session_token.clone()),
    ]);
    let signed: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let authorization = sigv4::authorization(
        &sigv4::Signed {
            method: "PUT",
            path: &path,
            query: &[],
            headers: &signed,
            payload_sha256: &payload,
        },
        &sigv4::Scope {
            time: now,
            region: &grant.region,
            service: "s3",
            access_key_id: &keys.access_key_id,
            secret_access_key: &keys.secret_access_key,
        },
    );
    headers.push(("authorization", authorization));
    Some(format!("{base}{}", sigv4::uri_encode(&path, false)))
}

/// `host[:port]` as a request to `url` names it.
fn authority(url: &url::Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}
