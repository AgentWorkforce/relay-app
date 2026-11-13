//! The receiver's own vectors for `relay-upload-inbox-v1`, proving this
//! build names, encodes and reads objects byte for byte as it does. The
//! fixture is a verbatim copy of `docs/contracts/relay-upload-inbox-v1.vectors.json`
//! from relayhistory-pipeline PR #525 (head
//! 5186ff1edd0b624f63b73adb2fd5d267f5e43f85, identical at its merge
//! 0d952838b91ccfa61f224bc5ca5e492743fa0800; sha256 54438c93…94723c3),
//! generated there from `src/session-ingest/upload-inbox-contract.ts`. It
//! adds `credentials.presignedSelfhost` to PR #508's copy.
//! Its user segment, witness and witness refusals are the receiver's and
//! Control's to derive and check, never this build's, so they are not used.
use super::*;
use chrono::DateTime;
use credentials::Kind;

pub(super) fn vectors() -> serde_json::Value {
    let text = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/fixtures/relay-upload-inbox-v1.vectors.json"
    ));
    serde_json::from_str(text).unwrap()
}

fn text<'a>(value: &'a serde_json::Value, path: &str) -> &'a str {
    value
        .pointer(path)
        .and_then(|value| value.as_str())
        .unwrap()
}

fn object_bytes(base64: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(base64)
        .unwrap()
}

#[test]
fn the_contract_object_is_named_and_checksummed_as_the_receiver_names_it() {
    let vectors = vectors();
    let bytes = object_bytes(text(&vectors, "/object/objectBytesBase64"));
    assert_eq!(
        bytes.len() as u64,
        vectors["object"]["objectBytes"].as_u64().unwrap()
    );
    let sha256: [u8; 32] = Sha256::digest(&bytes).into();
    assert_eq!(
        sigv4::hex(&sha256),
        text(&vectors, "/object/objectSha256Hex")
    );
    let batch_id = text(&vectors, "/inputs/batchId");
    let batch = sigv4::hex(&Sha256::digest(batch_id.as_bytes()));
    assert_eq!(&batch[..32], text(&vectors, "/object/batchSegment"));
    let key = object_key(
        text(&vectors, "/keyPrefix"),
        text(&vectors, "/inputs/originId"),
        batch_id,
        &sha256,
        vectors["parsedKey"]["witnessSeq"].as_u64().unwrap(),
    );
    assert_eq!(key, text(&vectors, "/objectKey"));
    assert_eq!(key, text(&vectors, "/parsedKey/key"));
    let relative = key
        .strip_prefix(text(&vectors, "/parsedKey/prefix"))
        .unwrap();
    assert_eq!(
        relative,
        format!(
            "o={}/b={}/d={}-w7.json.zst",
            text(&vectors, "/parsedKey/originId"),
            text(&vectors, "/parsedKey/batchSegment"),
            text(&vectors, "/parsedKey/digestSegment"),
        )
    );
    assert!(relative_key_valid(relative));
    assert!(origin_valid(text(&vectors, "/inputs/originId")));
    // The object is one zstd frame of the exact POST request body.
    let decoded = zstd::stream::decode_all(bytes.as_slice()).unwrap();
    assert_eq!(decoded, text(&vectors, "/object/requestJson").as_bytes());
}

#[test]
fn the_checksum_header_is_the_padded_base64_of_the_raw_digest() {
    let vectors = vectors();
    let request = text(&vectors, "/object/requestJson");
    let payload = PreparedPayload {
        mapping_version: MAPPING_VERSION.into(),
        content_type: "application/json".into(),
        body: request.into(),
        sha256: hash(request.as_bytes()),
        prepared_by: None,
    };
    let mut upload = Upload::of(&payload).unwrap();
    assert_eq!(upload.origin_id, text(&vectors, "/inputs/originId"));
    assert_eq!(upload.batch_id, text(&vectors, "/inputs/batchId"));
    assert_eq!(upload.revision_ids, ["rec-synthetic-1:r1"]);
    // Named and checksummed from the receiver's own frame, the upload gives
    // the receiver's key and header.
    upload.body = object_bytes(text(&vectors, "/object/objectBytesBase64"));
    upload.sha256 = Sha256::digest(&upload.body).into();
    assert_eq!(
        upload.checksum(),
        text(&vectors, "/object/checksumSha256Header")
    );
    assert_eq!(
        upload.key(text(&vectors, "/keyPrefix"), 7),
        text(&vectors, "/objectKey")
    );
}

#[test]
fn bytes_other_than_the_named_ones_get_another_digest_segment() {
    let vectors = vectors();
    let refusal = vectors["refusals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|refusal| refusal["fn"] == "checkUploadObjectBytes")
        .unwrap();
    assert_eq!(refusal["code"], "upload_digest_mismatch");
    let bytes = object_bytes(text(refusal, "/input/objectBytesBase64"));
    let sha256: [u8; 32] = Sha256::digest(&bytes).into();
    let key = object_key(
        text(&vectors, "/keyPrefix"),
        text(&vectors, "/inputs/originId"),
        text(&vectors, "/inputs/batchId"),
        &sha256,
        7,
    );
    assert_ne!(key, text(refusal, "/input/key"));
    assert!(!key.contains(text(&vectors, "/parsedKey/digestSegment")));
}

#[test]
fn keys_the_receiver_refuses_are_never_built() {
    let vectors = vectors();
    let prefix = text(&vectors, "/keyPrefix");
    for refusal in vectors["refusals"].as_array().unwrap() {
        if refusal["fn"] != "parseUploadKey" {
            continue;
        }
        assert_eq!(refusal["code"], "upload_key_invalid");
        let input = refusal["input"].as_str().unwrap();
        // Every key this build makes ends `.json.zst` under the prefix as
        // served, so neither another suffix nor another tenant comes out.
        let built = object_key(
            prefix,
            "origin-synthetic",
            "batch-synthetic-0001",
            &[0; 32],
            7,
        );
        assert!(built.ends_with(".json.zst") && built.starts_with(prefix));
        assert_ne!(built, input);
    }
    assert!(!origin_valid(".origin-synthetic"));
}

#[test]
fn the_delivery_account_matches_the_receivers() {
    let vectors = vectors();
    assert_eq!(
        super::super::account_id(
            text(&vectors, "/deliveryAccount/orgId"),
            Some(text(&vectors, "/deliveryAccount/workspaceId"))
        ),
        text(&vectors, "/deliveryAccount/account")
    );
}

fn at(value: &str) -> chrono::DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn the_sts_answer_is_taken_and_a_presigned_one_only_when_asked_for() {
    let vectors = vectors();
    let sts = credentials::parse(vectors["credentials"]["sts"].clone(), Kind::Sts).unwrap();
    assert_eq!(sts.witness_seq, 7);
    assert_eq!(sts.max_object_bytes, 2_097_152);
    assert_eq!(sts.key_prefix, text(&vectors, "/keyPrefix"));
    assert_eq!(sts.endpoint, None);
    assert!(sts.fresh(at("2026-11-11T00:44:59Z")));
    assert!(!sts.fresh(at("2026-11-11T00:45:00Z")));
    for name in ["presigned", "presignedSelfhost"] {
        let answer = vectors["credentials"][name].clone();
        // Control answers the kind asked for: presigned to an sts ask is
        // unusable, and sts to a presigned ask too.
        assert_eq!(
            credentials::parse(answer.clone(), Kind::Sts).unwrap_err(),
            "kind",
            "{name}"
        );
        let grant = credentials::parse(answer, Kind::Presigned).unwrap();
        assert_eq!(grant.kind(), Kind::Presigned);
        assert_eq!(grant.witness_seq, 7);
        assert_eq!(grant.refresh_at, at("2026-11-11T00:10:00Z"));
        let relative = text(&vectors, "/objectKey")
            .strip_prefix(text(&vectors, "/keyPrefix"))
            .unwrap();
        let path = format!("/credentials/{name}/urls/0/url");
        assert_eq!(
            grant.url_for(relative, at("2026-11-11T00:14:59Z")),
            Some(text(&vectors, &path))
        );
        assert_eq!(grant.url_for(relative, at("2026-11-11T00:15:00Z")), None);
    }
    assert_eq!(
        credentials::parse(vectors["credentials"]["sts"].clone(), Kind::Presigned).unwrap_err(),
        "kind"
    );
}

#[test]
fn the_self_host_answer_puts_to_its_url_as_is_without_sse_headers() {
    let vectors = vectors();
    let selfhost = &vectors["credentials"]["presignedSelfhost"];
    let grant = credentials::parse(selfhost.clone(), Kind::Presigned).unwrap();
    assert_eq!(
        grant.endpoint.as_deref(),
        Some("https://objects.example.test:9000/relayhistory")
    );
    assert_eq!(grant.kms_key_id, None);
    let request = text(&vectors, "/object/requestJson");
    let payload = PreparedPayload {
        mapping_version: MAPPING_VERSION.into(),
        content_type: "application/json".into(),
        body: request.into(),
        sha256: hash(request.as_bytes()),
        prepared_by: None,
    };
    let mut upload = Upload::of(&payload).unwrap();
    upload.body = object_bytes(text(&vectors, "/object/objectBytesBase64"));
    upload.sha256 = Sha256::digest(&upload.body).into();
    let mut grant = grant;
    // The vector's URL expired on 2026-11-11; move it past now to build the PUT.
    if let credentials::Access::Presigned(urls) = &mut grant.access {
        urls[0].expires_at = Utc::now() + chrono::Duration::minutes(5);
    }
    let (url, headers) =
        storage::target(&grant, &upload, &upload.key(&grant.key_prefix, 7)).unwrap();
    assert_eq!(url, text(selfhost, "/urls/0/url"));
    assert!(allowed(&url));
    let names: Vec<&str> = headers.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        [
            "if-none-match",
            "x-amz-checksum-sha256",
            "x-amz-meta-relay-witness",
            "host",
            "content-encoding",
            "content-type"
        ]
    );
    assert!(headers.contains(&("host", "objects.example.test:9000".into())));
    // The hosted presigned answer names a key: both SSE headers go with it.
    let mut hosted =
        credentials::parse(vectors["credentials"]["presigned"].clone(), Kind::Presigned).unwrap();
    if let credentials::Access::Presigned(urls) = &mut hosted.access {
        urls[0].expires_at = Utc::now() + chrono::Duration::minutes(5);
    }
    let (_, headers) =
        storage::target(&hosted, &upload, &upload.key(&hosted.key_prefix, 7)).unwrap();
    assert!(headers.contains(&("x-amz-server-side-encryption", "aws:kms".into())));
    assert!(headers.contains(&(
        "x-amz-server-side-encryption-aws-kms-key-id",
        "kms-key-placeholder".into()
    )));
}
