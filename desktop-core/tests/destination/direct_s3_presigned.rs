//! A store without STS (a self-hosted MinIO, say): Control answers an `sts`
//! ask with 501 `unsupported_by_backend`, and the client asks for presigned
//! URLs in the same send, puts to the URL as given, and remembers that for
//! the session in memory only.
use super::*;

const SIGNATURE: &str = "X-Amz-Signature=fixture-presigned-signature";

/// Control's presigned answer: one URL per key asked about, each into the
/// fake's own bucket, with no KMS key (the store encrypts by itself).
fn presigned(base: &str, request: &Request) -> Value {
    let ask = request.json();
    assert_eq!(ask["kind"], "presigned");
    let urls: Vec<Value> = ask["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| {
            let key = key["key"].as_str().unwrap();
            json!({"key":key,"url":format!("{base}/fixture-bucket/{PREFIX}{key}?{SIGNATURE}"),
                "expiresAt":"2099-01-01T00:15:00Z"})
        })
        .collect();
    json!({"contract":CONTRACT,"kind":"presigned","bucket":"fixture-bucket",
        "region":"us-east-1","endpoint":format!("{base}/fixture-root"),"keyPrefix":PREFIX,
        "kmsKeyId":null,"witness":"fixture.witness","witnessSeq":7,
        "maxObjectBytes":2_097_152,"refreshAfter":"2099-01-01T00:10:00Z","urls":urls})
}

/// Control for a store without STS: 501 to `sts`, URLs to `presigned`.
fn without_sts() -> Route {
    Box::new(|base, _, request| match request.json()["kind"].as_str() {
        Some("sts") => error(501, "unsupported_by_backend"),
        _ => Reply::json(200, presigned(base, request)),
    })
}

/// Each ask's kind, and the key a presigned one named.
fn asks(requests: &[Request]) -> Vec<String> {
    requests
        .iter()
        .filter(|request| request.path() == CREDENTIALS)
        .map(|request| {
            let ask = request.json();
            match ask["keys"].as_array() {
                Some(keys) => {
                    let key = keys[0]["key"].as_str().unwrap();
                    format!("presigned {}", &key[key.len() - 12..])
                }
                None => ask["kind"].as_str().unwrap().to_owned(),
            }
        })
        .collect()
}

fn another(batch: &HistoryExportBatch) -> HistoryExportBatch {
    let mut other = batch.clone();
    other.batch_id = "8b842f46550f4840c225e06f5b3d638d".into();
    other
}

/// The 501 is answered in the same send: a presigned ask naming the key
/// for the seq it knows (none yet: 0), once more for the seq the answer
/// carries, and a PUT to that URL as given, with no SSE headers for a
/// null `kmsKeyId` and nothing signed by the client. The next batch asks
/// for presigned URLs straight away, with the seq last heard.
#[test]
fn a_store_without_sts_gets_presigned_urls_in_the_same_send() {
    isolated(
        "direct_s3::presigned::a_store_without_sts_gets_presigned_urls_in_the_same_send",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let other = another(&batch);
            let fake = serve(without_sts(), stored(), admits());
            auth(home, &fake.base, "org-fixture");
            let receiver = direct(&fake.base);
            let ack = receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            assert_eq!(ack.acceptance_level, AcceptanceLevel::Queued);
            let ack = receiver
                .send(&prepare(other.clone()).unwrap(), &other, &context(&other))
                .unwrap();
            assert_eq!(ack.acceptance_level, AcceptanceLevel::Queued);
            let requests = fake.join();
            assert_eq!(
                asks(&requests),
                [
                    "sts",
                    "presigned -w0.json.zst",
                    "presigned -w7.json.zst",
                    "presigned -w7.json.zst"
                ]
            );
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
            let ask = requests
                .iter()
                .filter(|request| request.path() == CREDENTIALS)
                .nth(2)
                .unwrap()
                .json();
            let mut fields: Vec<_> = ask.as_object().unwrap().keys().cloned().collect();
            fields.sort();
            assert_eq!(fields, ["keys", "kind", "originId"]);
            assert_eq!(ask["originId"], batch.origin_id.as_str());
            let puts: Vec<_> = requests.iter().filter(|r| r.method == "PUT").collect();
            assert_eq!(puts.len(), 2);
            let put = puts[0];
            let key = relative_key(&batch.origin_id, &batch.batch_id, &put.body);
            assert_eq!(ask["keys"].as_array().unwrap().len(), 1);
            assert_eq!(ask["keys"][0]["key"], key.as_str());
            let checksum =
                base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&put.body));
            assert_eq!(ask["keys"][0]["sha256"], checksum.as_str());
            // The URL as given: its path and query untouched.
            assert_eq!(
                put.target,
                format!("/fixture-bucket/{PREFIX}{key}?{SIGNATURE}")
            );
            for (name, value) in [
                ("if-none-match", "*"),
                ("x-amz-checksum-sha256", checksum.as_str()),
                ("x-amz-meta-relay-witness", "fixture.witness"),
                ("content-encoding", "zstd"),
                ("content-type", "application/json"),
            ] {
                assert_eq!(put.header(name), Some(value), "{name}");
            }
            for name in [
                "authorization",
                "x-amz-security-token",
                "x-amz-content-sha256",
                "x-amz-date",
                "x-amz-server-side-encryption",
                "x-amz-server-side-encryption-aws-kms-key-id",
            ] {
                assert_eq!(put.header(name), None, "{name}");
            }
        },
    );
}

/// Presigned-only belongs to the session that heard the 501: the host
/// replacing the session, or a restart (a new receiver), asks `sts` first
/// again.
#[test]
fn presigned_only_is_forgotten_with_the_session_and_at_restart() {
    isolated(
        "direct_s3::presigned::presigned_only_is_forgotten_with_the_session_and_at_restart",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(without_sts(), stored(), admits());
            auth(home, &fake.base, "org-fixture");
            let receiver = direct(&fake.base);
            receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            receiver.state.forget_session();
            receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            direct(&fake.base)
                .send(&prepared, &batch, &context(&batch))
                .unwrap();
            let requests = fake.join();
            let first = ["sts", "presigned -w0.json.zst", "presigned -w7.json.zst"];
            assert_eq!(asks(&requests), [first, first, first].concat());
            assert_eq!(count(&requests, "PUT", "/"), 3);
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
        },
    );
}

/// Only a 501 whose `error` is exactly `unsupported_by_backend` leads to a
/// presigned ask; any other 501 posts the batch and is not remembered.
#[test]
fn any_other_501_posts_the_batch() {
    isolated(
        "direct_s3::presigned::any_other_501_posts_the_batch",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            for reply in [
                error(501, "not_implemented"),
                error(501, "UNSUPPORTED_BY_BACKEND"),
                Reply::json(501, json!({"error":{"code":"unsupported_by_backend"}})),
                Reply::json(501, json!({"code":"unsupported_by_backend"})),
                Reply::text(501, "unsupported_by_backend"),
            ] {
                let fake = serve(
                    Box::new(move |_, _, request| {
                        assert_eq!(request.json()["kind"], "sts");
                        reply.clone()
                    }),
                    unreached(),
                    unreached(),
                );
                auth(home, &fake.base, "org-fixture");
                let receiver = direct(&fake.base);
                for _ in 0..2 {
                    let ack = receiver.send(&prepared, &batch, &context(&batch)).unwrap();
                    assert_eq!(ack.acceptance_level, AcceptanceLevel::Durable);
                }
                let requests = fake.join();
                assert_eq!(asks(&requests), ["sts", "sts"]);
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 2);
            }
        },
    );
}

/// A presigned ask answered with session keys is unusable: the batch is
/// posted and presigned-only forgotten, so the next send asks `sts` first.
#[test]
fn a_presigned_ask_answered_with_another_kind_forgets_presigned_only() {
    isolated(
        "direct_s3::presigned::a_presigned_ask_answered_with_another_kind_forgets_presigned_only",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(
                Box::new(|base, index, _| match index {
                    0 => error(501, "unsupported_by_backend"),
                    1 => Reply::json(200, sts(base, "fixture-session-token")),
                    _ => error(503, "upload_credentials_unavailable"),
                }),
                unreached(),
                unreached(),
            );
            auth(home, &fake.base, "org-fixture");
            let receiver = direct(&fake.base);
            for _ in 0..2 {
                let ack = receiver.send(&prepared, &batch, &context(&batch)).unwrap();
                assert_eq!(ack.acceptance_level, AcceptanceLevel::Durable);
            }
            let requests = fake.join();
            assert_eq!(asks(&requests), ["sts", "presigned -w0.json.zst", "sts"]);
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 2);
        },
    );
}

/// When even the re-ask names no URL for the key its seq gives, nothing is
/// put under a key of another seq: the batch is posted.
#[test]
fn urls_that_never_cover_the_object_post_the_batch() {
    isolated(
        "direct_s3::presigned::urls_that_never_cover_the_object_post_the_batch",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(
                Box::new(|base, _, request| match request.json()["kind"].as_str() {
                    Some("sts") => error(501, "unsupported_by_backend"),
                    _ => {
                        let mut answer = presigned(base, request);
                        answer["urls"][0]["key"] = json!("o=a/b=b/d=d-w7.json.zst");
                        Reply::json(200, answer)
                    }
                }),
                unreached(),
                unreached(),
            );
            auth(home, &fake.base, "org-fixture");
            let ack = direct(&fake.base)
                .send(&prepared, &batch, &context(&batch))
                .unwrap();
            assert_eq!(ack.acceptance_level, AcceptanceLevel::Durable);
            let requests = fake.join();
            assert_eq!(
                asks(&requests),
                ["sts", "presigned -w0.json.zst", "presigned -w7.json.zst"]
            );
            assert_eq!(count(&requests, "PUT", "/"), 0);
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 1);
        },
    );
}

/// Through the worker, presigned URLs (signature and all) reach neither
/// database nor any file beside them.
#[test]
fn a_drained_presigned_batch_stores_no_url() {
    isolated(
        "direct_s3::presigned::a_drained_presigned_batch_stores_no_url",
        |home| {
            let (directory, path, conn, job_id) = queue();
            let fake = serve(without_sts(), stored(), admits());
            auth(home, &fake.base, "org-fixture");
            let drained = drain_at(&path, &direct(&fake.base), 0);
            assert!(drained.issues.is_empty(), "{:?}", drained.issues);
            let status = delivery::status(&conn, &job_id).unwrap();
            assert_eq!(status.acceptance_level.as_deref(), Some("queued"));
            drop(conn);
            let requests = fake.join();
            assert_eq!(count(&requests, "PUT", "/"), 1);
            assert!(requests
                .iter()
                .any(|request| request.target.contains(SIGNATURE)));
            assert_nothing_handed_out_is_stored(directory.path());
        },
    );
}
