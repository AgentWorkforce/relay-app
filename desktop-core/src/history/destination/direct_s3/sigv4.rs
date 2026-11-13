//! AWS Signature Version 4 for one request with a signed payload, as S3
//! takes it for a PUT with temporary credentials. Pure functions over the
//! request, the time and the keys, so AWS's published vectors pin them.
use chrono::{DateTime, Utc};
use ring::hmac;
use sha2::{Digest, Sha256};

const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// What a signature covers: the method, the path and query, every header to
/// sign (`host` included) and the hex SHA-256 of the payload.
pub(super) struct Signed<'a> {
    pub(super) method: &'a str,
    /// The path as sent, before URI encoding.
    pub(super) path: &'a str,
    /// Query parameters, decoded.
    pub(super) query: &'a [(&'a str, &'a str)],
    pub(super) headers: &'a [(&'a str, &'a str)],
    pub(super) payload_sha256: &'a str,
}

/// Where and as whom a request is signed.
pub(super) struct Scope<'a> {
    pub(super) time: DateTime<Utc>,
    pub(super) region: &'a str,
    pub(super) service: &'a str,
    pub(super) access_key_id: &'a str,
    pub(super) secret_access_key: &'a str,
}

/// The `x-amz-date` value for `time`.
pub(super) fn amz_date(time: DateTime<Utc>) -> String {
    time.format("%Y%m%dT%H%M%SZ").to_string()
}

/// The `Authorization` header value for `request`.
pub(super) fn authorization(request: &Signed<'_>, scope: &Scope<'_>) -> String {
    let (canonical, signed_headers) = canonical_request(request);
    let date = scope.time.format("%Y%m%d").to_string();
    let credential_scope = format!("{date}/{}/{}/aws4_request", scope.region, scope.service);
    let string_to_sign = format!(
        "{ALGORITHM}\n{}\n{credential_scope}\n{}",
        amz_date(scope.time),
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let key = signing_key(scope.secret_access_key, &date, scope.region, scope.service);
    let signature = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, key.as_ref()),
        string_to_sign.as_bytes(),
    );
    format!(
        "{ALGORITHM} Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={}",
        scope.access_key_id,
        hex(signature.as_ref())
    )
}

/// The canonical request and its signed-header list: header names
/// lowercased, values trimmed with inner runs of spaces collapsed, both
/// sorted by name.
fn canonical_request(request: &Signed<'_>) -> (String, String) {
    let mut headers: Vec<(String, String)> = request
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.to_ascii_lowercase(),
                value.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    headers.sort();
    let mut query: Vec<(String, String)> = request
        .query
        .iter()
        .map(|(name, value)| (uri_encode(name, true), uri_encode(value, true)))
        .collect();
    query.sort();
    let query = query
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical = format!(
        "{}\n{}\n{query}\n{canonical_headers}\n{signed_headers}\n{}",
        request.method,
        uri_encode(request.path, false),
        request.payload_sha256
    );
    (canonical, signed_headers)
}

/// The key a day's requests in `region` for `service` are signed with.
fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> hmac::Tag {
    let step = |key: &[u8], data: &str| {
        hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data.as_bytes())
    };
    let day = step(format!("AWS4{secret}").as_bytes(), date);
    let region = step(day.as_ref(), region);
    let service = step(region.as_ref(), service);
    step(service.as_ref(), "aws4_request")
}

/// Lowercase hex of `bytes`.
pub(super) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// `value` percent-encoded as SigV4 asks: every byte but the unreserved
/// ones, and `/` too when `slash` is set (a query part, not a path).
pub(super) fn uri_encode(value: &str, slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (byte == b'/' && !slash)
        {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // AWS's published Signature Version 4 test suite: these keys, this time,
    // region and service, and the expected signatures, are AWS's examples.
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn scope() -> Scope<'static> {
        Scope {
            time: chrono::DateTime::parse_from_rfc3339("2015-08-30T12:36:00Z")
                .unwrap()
                .with_timezone(&Utc),
            region: "us-east-1",
            service: "service",
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: SECRET,
        }
    }

    fn signed(method: &str, query: &[(&str, &str)], extra: &[(&str, &str)]) -> String {
        let mut headers = vec![
            ("Host", "example.amazonaws.com"),
            ("X-Amz-Date", "20150830T123600Z"),
        ];
        headers.extend_from_slice(extra);
        authorization(
            &Signed {
                method,
                path: "/",
                query,
                headers: &headers,
                payload_sha256: EMPTY_SHA256,
            },
            &scope(),
        )
    }

    fn expected(signed_headers: &str, signature: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders={signed_headers}, Signature={signature}"
        )
    }

    #[test]
    fn get_vanilla() {
        assert_eq!(
            signed("GET", &[], &[]),
            expected(
                "host;x-amz-date",
                "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
            )
        );
    }

    #[test]
    fn post_vanilla() {
        assert_eq!(
            signed("POST", &[], &[]),
            expected(
                "host;x-amz-date",
                "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
            )
        );
    }

    #[test]
    fn post_vanilla_query() {
        assert_eq!(
            signed("POST", &[("Param1", "value1")], &[]),
            expected(
                "host;x-amz-date",
                "28038455d6de14eafc1f9222cf5aa6f1a96197d7deb8263271d420d138af7f11"
            )
        );
    }

    #[test]
    fn get_header_value_trim() {
        assert_eq!(
            signed(
                "GET",
                &[],
                &[("My-Header1", " value1"), ("My-Header2", " \"a   b   c\"")]
            ),
            expected(
                "host;my-header1;my-header2;x-amz-date",
                "acc3ed3afb60bb290fc8d2dd0098b9911fcaa05412b367055dee359757a9c736"
            )
        );
    }

    /// `post-sts-header-before`: a session token signed as a header.
    #[test]
    fn post_sts_header_before() {
        let token = "AQoDYXdzEPT//////////wEXAMPLEtc764bNrC9SAPBSM22wDOk4x4HIZ8j4FZTwdQWLWsKWHGBuFqwAeMicRXmxfpSPfIeoIYRqTflfKD8YUuwthAx7mSEI/qkPpKPi/kMcGdQrmGdeehM4IC1NtBmUpp2wUE8phUZampKsburEDy0KPkyQDYwT7WZ0wq5VSXDvp75YU9HFvlRd8Tx6q6fE8YQcHNVXAkiY9q6d+xo0rKwT38xVqr7ZD0u0iPPkUL64lIZbqBAz+scqKmlzm8FDrypNC9Yjc8fPOLn9FX9KSYvKTr4rvx3iSIlTJabIQwj2ICCR/oLxBA==";
        assert_eq!(
            signed("POST", &[], &[("X-Amz-Security-Token", token)]),
            expected(
                "host;x-amz-date;x-amz-security-token",
                "85d96828115b5dc0cfc3bd16ad9e210dd772bbebba041836c64533a82be05ead"
            )
        );
    }

    /// AWS's signing-key derivation examples.
    #[test]
    fn signing_key_derivation() {
        assert_eq!(
            hex(signing_key(SECRET, "20120215", "us-east-1", "iam").as_ref()),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
        assert_eq!(
            hex(signing_key(SECRET, "20150830", "us-east-1", "iam").as_ref()),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn paths_keep_slashes_and_encode_the_rest() {
        assert_eq!(uri_encode("/a b/o=x~y", false), "/a%20b/o%3Dx~y");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
    }
}
