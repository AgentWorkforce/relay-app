//! Direct uploads into the upload inbox: one fake stands in for Control's
//! upload-credentials route, the bucket and the receiver, and each test
//! reads back what the client sent where.
use super::fake_s3::{FakeS3, Reply, Request};
use super::{accepted, auth, batch, context, failure, isolated, receiver};
use agent_relay_desktop_core::history::delivery::{
    self, worker, worker::Receiver, AcceptanceLevel, DeliveryFailure, HistoryExportBatch,
    StoredUpload,
};
use agent_relay_desktop_core::history::destination::{
    account_id, prepare, DirectUploadSource, ReceiverState, RelayHistoryReceiver, MAPPING_VERSION,
};
use base64::Engine;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
#[path = "direct_s3_presigned.rs"]
mod presigned;

const CREDENTIALS: &str = "/api/v1/workspaces/ws-fixture/relayhistory/upload-credentials";
const PREFIX: &str = "fixture-inbox/ws=ws-fixture/u=u1/";
const CONTRACT: &str = "relay-upload-inbox-v1";

/// The probe's receiver for `base`, with Control at the same address.
fn direct(base: &str) -> RelayHistoryReceiver {
    RelayHistoryReceiver {
        direct_upload: Some(DirectUploadSource {
            api_url: base.to_owned(),
            workspace_id: "ws-fixture".into(),
            admission_base_url: None,
        }),
        ..receiver(base)
    }
}

/// Session credentials for the fake's own bucket, path style.
fn sts(base: &str, token: &str) -> Value {
    json!({"contract":CONTRACT,"kind":"sts","bucket":"fixture-bucket","region":"us-east-1",
        "endpoint":format!("{base}/"),"keyPrefix":PREFIX,"kmsKeyId":"fixture-kms-key",
        "credentials":{"accessKeyId":"AKIDFIXTURE","secretAccessKey":"fixture-secret",
            "sessionToken":token,"expiresAt":"2099-01-01T01:00:00Z"},
        "witness":"fixture.witness","witnessSeq":7,"maxObjectBytes":2_097_152,
        "refreshAfter":"2099-01-01T00:50:00Z"})
}

fn admission(request: &Request, state: &str, code: Option<&str>) -> Reply {
    let mut answer = json!({"contract":CONTRACT,"uploadKey":request.query("uploadKey").unwrap(),
        "state":state});
    if let Some(code) = code {
        answer["code"] = json!(code);
    }
    Reply::json(200, answer)
}

fn error(status: u16, code: &str) -> Reply {
    Reply::json(status, json!({ "error": code }))
}

/// What each route answers, by the route's own request count.
type Route = Box<dyn FnMut(&str, usize, &Request) -> Reply + Send>;

/// Serve Control, the bucket (any PUT) and admission from `routes`, and
/// accept every batch posted.
fn serve(control: Route, bucket: Route, admitted: Route) -> FakeS3 {
    let mut routes = [control, bucket, admitted];
    let mut counts = [0; 3];
    FakeS3::serve(move |base, request| {
        let route = match (request.method.as_str(), request.path()) {
            ("POST", CREDENTIALS) => 0,
            ("PUT", _) => 1,
            ("GET", "/v1/delivery/admission") => 2,
            ("POST", "/v1/delivery/batches") => {
                let batch: HistoryExportBatch =
                    serde_json::from_value(request.json()["batch"].clone()).unwrap();
                return Reply::json(200, accepted(&batch));
            }
            _ => return Reply::json(404, json!({})),
        };
        counts[route] += 1;
        routes[route](base, counts[route] - 1, request)
    })
}

fn stored() -> Route {
    Box::new(|_, _, _| Reply::text(200, ""))
}
fn admits() -> Route {
    Box::new(|_, _, request| admission(request, "admitted", None))
}
fn grants() -> Route {
    Box::new(|base, _, _| Reply::json(200, sts(base, "fixture-session-token")))
}
fn unreached() -> Route {
    Box::new(|_, _, request| panic!("unexpected {} {}", request.method, request.target))
}

fn count(requests: &[Request], method: &str, path: &str) -> usize {
    requests
        .iter()
        .filter(|request| request.method == method && request.path().starts_with(path))
        .count()
}

/// The key a batch's object goes under for witness seq 7, relative to the
/// prefix, read from the bytes the client put.
fn relative_key(origin: &str, batch_id: &str, body: &[u8]) -> String {
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    format!(
        "o={origin}/b={}/d={}-w7.json.zst",
        &hex(&Sha256::digest(batch_id.as_bytes()))[..32],
        &hex(&Sha256::digest(body))[..32]
    )
}

#[test]
fn a_batch_is_put_once_and_a_replayed_put_is_queued_on_412() {
    isolated(
        "direct_s3::a_batch_is_put_once_and_a_replayed_put_is_queued_on_412",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(
                grants(),
                Box::new(|_, index, _| match index {
                    0 => Reply::text(200, ""),
                    _ => Reply::text(412, "<Error><Code>PreconditionFailed</Code></Error>"),
                }),
                admits(),
            );
            auth(home, &fake.base, "org-fixture");
            let receiver = direct(&fake.base);
            for _ in 0..2 {
                let ack = receiver.send(&prepared, &batch, &context(&batch)).unwrap();
                assert_eq!(ack.acceptance_level, AcceptanceLevel::Queued);
                assert_eq!(ack.accepted_revision_ids.len(), batch.records.len());
                assert!(ack.isolation_frontier.is_empty());
                assert!(!format!("{ack:?}").contains("fixture-secret"));
            }
            let requests = fake.join();
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
            // The kept credentials put the replay: Control is asked once.
            let asks: Vec<_> = requests
                .iter()
                .filter(|request| request.path() == CREDENTIALS)
                .collect();
            assert_eq!(asks.len(), 1);
            let ask = asks[0].json();
            let mut fields: Vec<_> = ask.as_object().unwrap().keys().cloned().collect();
            fields.sort();
            assert_eq!(fields, ["kind", "originId"]);
            assert_eq!(ask["kind"], "sts");
            assert_eq!(ask["originId"], batch.origin_id.as_str());
            assert_eq!(
                asks[0].header("authorization"),
                Some("Bearer rth_at_fixture")
            );

            let puts: Vec<_> = requests.iter().filter(|r| r.method == "PUT").collect();
            assert_eq!(puts.len(), 2);
            assert_eq!(puts[0].target, puts[1].target);
            assert_eq!(puts[0].body, puts[1].body);
            let put = puts[0];
            let expected = format!(
                "/fixture-bucket/{PREFIX}{}",
                relative_key(&batch.origin_id, &batch.batch_id, &put.body)
            );
            // SigV4's canonical path escapes `=`, as S3 reads it.
            assert_eq!(put.target.replace("%3D", "="), expected);
            let checksum =
                base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&put.body));
            for (name, value) in [
                ("if-none-match", "*"),
                ("x-amz-checksum-sha256", checksum.as_str()),
                ("x-amz-meta-relay-witness", "fixture.witness"),
                ("x-amz-server-side-encryption", "aws:kms"),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    "fixture-kms-key",
                ),
                ("x-amz-security-token", "fixture-session-token"),
                ("content-encoding", "zstd"),
            ] {
                assert_eq!(put.header(name), Some(value), "{name}");
            }
            let authorization = put.header("authorization").unwrap();
            assert!(authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIDFIXTURE/"));
            assert!(authorization.contains("/us-east-1/s3/aws4_request"));
            assert!(!authorization.contains("fixture-secret"));
            // The object is the batch request the POST path would send.
            let object: Value =
                serde_json::from_slice(&zstd::bulk::decompress(&put.body, 1 << 24).unwrap())
                    .unwrap();
            assert_eq!(object["protocolVersion"], 1);
            assert_eq!(object["batch"]["batch_id"], batch.batch_id.as_str());
            let polled = requests
                .iter()
                .find(|request| request.path() == "/v1/delivery/admission")
                .unwrap();
            assert_eq!(
                polled.query("uploadKey").as_deref(),
                Some(&expected["/fixture-bucket/".len()..])
            );
        },
    );
}

#[test]
fn expired_credentials_are_renewed_once_and_the_object_put_again() {
    isolated(
        "direct_s3::expired_credentials_are_renewed_once_and_the_object_put_again",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            for refusal in [
                "<Error><Code>ExpiredToken</Code></Error>",
                "<Error><Code>AccessDenied</Code><Message>Request has expired</Message></Error>",
            ] {
                let fake = serve(
                    Box::new(|base, index, _| {
                        Reply::json(200, sts(base, &format!("fixture-session-token-{index}")))
                    }),
                    Box::new(move |_, index, _| match index {
                        0 => Reply::text(403, refusal),
                        _ => Reply::text(200, ""),
                    }),
                    admits(),
                );
                auth(home, &fake.base, "org-fixture");
                let ack = direct(&fake.base)
                    .send(&prepared, &batch, &context(&batch))
                    .unwrap();
                assert_eq!(ack.acceptance_level, AcceptanceLevel::Queued);
                let requests = fake.join();
                assert_eq!(count(&requests, "POST", CREDENTIALS), 2);
                let tokens: Vec<_> = requests
                    .iter()
                    .filter(|r| r.method == "PUT")
                    .map(|r| r.header("x-amz-security-token").unwrap().to_owned())
                    .collect();
                assert_eq!(
                    tokens,
                    ["fixture-session-token-0", "fixture-session-token-1"]
                );
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
            }
        },
    );
}

/// Control answers an `sts` ask with `sts` or an error, never another
/// kind: an answer of another kind is unusable, and the batch is posted
/// after that one ask.
#[test]
fn an_answer_of_another_kind_than_asked_posts_the_batch_after_one_ask() {
    isolated(
        "direct_s3::an_answer_of_another_kind_than_asked_posts_the_batch_after_one_ask",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(
                Box::new(|base, _, request| {
                    assert_eq!(request.json()["kind"], "sts");
                    let mut answer = sts(base, "fixture-session-token");
                    answer["kind"] = json!("presigned");
                    answer["urls"] = json!([{"key":"o=a/b=b/d=d-w7.json.zst",
                        "url":format!("{base}/presigned"),"expiresAt":"2099-01-01T00:15:00Z"}]);
                    Reply::json(200, answer)
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
            assert_eq!(count(&requests, "POST", CREDENTIALS), 1);
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 1);
        },
    );
}

/// 404 `history_upload_direct_off` is the only "off": it is kept, so the
/// next batch is posted without asking. 503 posts this batch only; 429
/// posts and keeps its wait.
#[test]
fn control_declining_posts_the_batch_and_only_off_and_a_wait_are_kept() {
    isolated(
        "direct_s3::control_declining_posts_the_batch_and_only_off_and_a_wait_are_kept",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            for (reply, asks) in [
                (error(404, "history_upload_direct_off"), 1),
                (error(503, "upload_credentials_unavailable"), 2),
                (error(503, "privacy_journal_unavailable"), 2),
                (error(429, "rate_limited").with("Retry-After", "120"), 1),
            ] {
                let fake = serve(
                    Box::new(move |_, _, _| reply.clone()),
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
                assert_eq!(count(&requests, "POST", CREDENTIALS), asks);
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 2);
            }
        },
    );
}

/// Any 404 but `history_upload_direct_off` is the workspace route's own:
/// the send fails as a batch POST's 404 does, nothing is posted, and
/// nothing is remembered, so the next send asks again.
#[test]
fn a_404_other_than_off_fails_the_send_as_the_sync_path_does() {
    isolated(
        "direct_s3::a_404_other_than_off_fails_the_send_as_the_sync_path_does",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            for reply in [error(404, "workspace_not_found")] {
                let fake = serve(
                    Box::new(move |_, _, _| reply.clone()),
                    unreached(),
                    unreached(),
                );
                auth(home, &fake.base, "org-fixture");
                let receiver = direct(&fake.base);
                for _ in 0..2 {
                    let result = receiver.send(&prepared, &batch, &context(&batch));
                    assert_eq!(failure(result), DeliveryFailure::UnsupportedEvidence);
                }
                let requests = fake.join();
                assert_eq!(count(&requests, "POST", CREDENTIALS), 2);
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
            }
        },
    );
}

/// A 404 showing Control has no upload-credentials route at all (a site's
/// HTML page, an empty or code-less body, a router's `not_found`) is "off":
/// the batch is posted, and the answer is kept so the next send does not
/// ask again. A hosted site whose Control predates direct uploads answers
/// this way, and must never stop uploads.
#[test]
fn a_404_without_the_route_is_off_and_posts_the_batch() {
    isolated(
        "direct_s3::a_404_without_the_route_is_off_and_posts_the_batch",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            for reply in [
                Reply::text(404, "<!DOCTYPE html><html><body>Not found</body></html>"),
                Reply::text(404, ""),
                Reply::json(404, json!({})),
                error(404, "not_found"),
            ] {
                let fake = serve(
                    Box::new(move |_, _, _| reply.clone()),
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
                assert_eq!(count(&requests, "POST", CREDENTIALS), 1);
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 2);
            }
        },
    );
}

/// Credentials are kept for the session token that asked: another user
/// signing in to the same workspace asks for their own, and never puts
/// under the previous user's prefix or witness.
#[test]
fn another_session_token_never_reuses_the_kept_credentials() {
    isolated(
        "direct_s3::another_session_token_never_reuses_the_kept_credentials",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(
                Box::new(|base, index, _| {
                    Reply::json(200, sts(base, &format!("fixture-session-token-{index}")))
                }),
                stored(),
                admits(),
            );
            auth(home, &fake.base, "org-fixture");
            let receiver = direct(&fake.base);
            receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            std::fs::write(
                home.join("stages")
                    .join(format!("{}.auth.json", super::stage_key(&fake.base))),
                json!({"base_url":fake.base,"access_token":"rth_at_other","org_id":"org-fixture"})
                    .to_string(),
            )
            .unwrap();
            receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            let requests = fake.join();
            let bearers: Vec<_> = requests
                .iter()
                .filter(|r| r.path() == CREDENTIALS)
                .map(|r| r.header("authorization").unwrap())
                .collect();
            assert_eq!(bearers, ["Bearer rth_at_fixture", "Bearer rth_at_other"]);
            let tokens: Vec<_> = requests
                .iter()
                .filter(|r| r.method == "PUT")
                .map(|r| r.header("x-amz-security-token").unwrap())
                .collect();
            assert_eq!(
                tokens,
                ["fixture-session-token-0", "fixture-session-token-1"]
            );
        },
    );
}

/// A session Control refuses fails the send as a batch POST would; it is
/// never read as "off".
#[test]
fn a_session_control_refuses_fails_the_send() {
    isolated(
        "direct_s3::a_session_control_refuses_fails_the_send",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            for (reply, expected) in [
                (
                    error(401, "unauthorized"),
                    DeliveryFailure::AuthenticationRequired,
                ),
                (error(403, "forbidden"), DeliveryFailure::PermissionDenied),
                (
                    error(403, "handoff_not_exchanged"),
                    DeliveryFailure::PermissionDenied,
                ),
            ] {
                let fake = serve(
                    Box::new(move |_, _, _| reply.clone()),
                    unreached(),
                    unreached(),
                );
                auth(home, &fake.base, "org-fixture");
                let result = direct(&fake.base).send(&prepared, &batch, &context(&batch));
                assert_eq!(failure(result), expected);
                let requests = fake.join();
                assert!(count(&requests, "POST", CREDENTIALS) >= 1);
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
            }
        },
    );
}

/// Once a PUT is made, any answer but 200 or 412 leaves the batch queued
/// for a retry under the same key: it is neither posted nor isolated.
#[test]
fn a_put_the_bucket_does_not_store_keeps_the_batch_for_a_retry() {
    isolated(
        "direct_s3::a_put_the_bucket_does_not_store_keeps_the_batch_for_a_retry",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            for (status, body, expected) in [
                (
                    500,
                    "<Error><Code>InternalError</Code></Error>",
                    DeliveryFailure::Transient,
                ),
                (
                    503,
                    "<Error><Code>SlowDown</Code></Error>",
                    DeliveryFailure::Transient,
                ),
                (
                    403,
                    "<Error><Code>AccessDenied</Code></Error>",
                    DeliveryFailure::Transient,
                ),
                (
                    400,
                    "<Error><Code>BadDigest</Code></Error>",
                    DeliveryFailure::Transient,
                ),
                (429, "", DeliveryFailure::RateLimited),
            ] {
                let fake = serve(
                    grants(),
                    Box::new(move |_, _, _| Reply::text(status, body)),
                    unreached(),
                );
                auth(home, &fake.base, "org-fixture");
                let receiver = direct(&fake.base);
                for _ in 0..2 {
                    let result = receiver.send(&prepared, &batch, &context(&batch));
                    assert_eq!(failure(result), expected, "{status}");
                }
                let requests = fake.join();
                // Any 403 drops the credentials: the next send asks again.
                let asks = if status == 403 { 2 } else { 1 };
                assert_eq!(count(&requests, "POST", CREDENTIALS), asks, "{status}");
                assert_eq!(count(&requests, "PUT", "/"), 2);
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
            }
        },
    );
}

/// A refusal is the terminal failure a rejected batch gets, keeping the
/// object's key; a code this build does not know is a refusal all the same.
#[test]
fn a_refused_object_fails_its_batch_as_receiver_rejected() {
    isolated(
        "direct_s3::a_refused_object_fails_its_batch_as_receiver_rejected",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            for code in [
                "delivery_privacy_superseded",
                "upload_unencrypted",
                "a_code_from_a_newer_contract",
            ] {
                let fake = serve(
                    grants(),
                    stored(),
                    Box::new(move |_, _, request| admission(request, "refused", Some(code))),
                );
                auth(home, &fake.base, "org-fixture");
                let refused = direct(&fake.base)
                    .send(&prepared, &batch, &context(&batch))
                    .unwrap_err();
                assert_eq!(refused.failure, DeliveryFailure::ReceiverRejected);
                assert!(matches!(
                    refused.upload,
                    Some(StoredUpload::Pending(ref key)) if key.starts_with(PREFIX)
                ));
                let requests = fake.join();
                assert_eq!(count(&requests, "PUT", "/"), 1);
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
            }
        },
    );
}

/// A pending answer, or a busy receiver, is polled again; an answer that
/// leaves nothing to wait on keeps the stored object queued. A 403 is the
/// session's (`upload_tenant_mismatch`) and is never read as queued.
#[test]
fn admission_is_polled_until_decided_and_no_answer_keeps_the_batch_queued() {
    isolated(
        "direct_s3::admission_is_polled_until_decided_and_no_answer_keeps_the_batch_queued",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(
                grants(),
                stored(),
                Box::new(|_, index, request| match index {
                    0 => admission(request, "pending", None),
                    1 => error(503, "delivery_unavailable").with("Retry-After", "0"),
                    _ => admission(request, "admitted", None),
                }),
            );
            auth(home, &fake.base, "org-fixture");
            let ack = direct(&fake.base)
                .send(&prepared, &batch, &context(&batch))
                .unwrap();
            assert_eq!(ack.accepted_revision_ids.len(), batch.records.len());
            assert_eq!(count(&fake.join(), "GET", "/v1/delivery/admission"), 3);

            for reply in [error(404, "upload_unknown"), error(400, "invalid_delivery")] {
                let fake = serve(grants(), stored(), Box::new(move |_, _, _| reply.clone()));
                auth(home, &fake.base, "org-fixture");
                let ack = direct(&fake.base)
                    .send(&prepared, &batch, &context(&batch))
                    .unwrap();
                assert_eq!(ack.acceptance_level, AcceptanceLevel::Queued);
                assert_eq!(ack.accepted_revision_ids.len(), batch.records.len());
                assert_eq!(count(&fake.join(), "GET", "/v1/delivery/admission"), 1);
            }
        },
    );
}

/// An admission 403 fails the send as a refused session does: the batch is
/// neither queued nor posted, and the object's key is forgotten, so a send
/// after another sign-in stores it again under that session.
#[test]
fn an_admission_403_fails_the_send_and_is_never_queued() {
    isolated(
        "direct_s3::an_admission_403_fails_the_send_and_is_never_queued",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(
                grants(),
                stored(),
                Box::new(|_, _, _| error(403, "upload_tenant_mismatch")),
            );
            auth(home, &fake.base, "org-fixture");
            let refused = direct(&fake.base)
                .send(&prepared, &batch, &context(&batch))
                .unwrap_err();
            assert_eq!(refused.failure, DeliveryFailure::PermissionDenied);
            assert_eq!(refused.upload, Some(StoredUpload::Forgotten));
            let requests = fake.join();
            assert_eq!(count(&requests, "GET", "/v1/delivery/admission"), 1);
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
        },
    );
}

/// Still pending after the polls: the send fails for a retry that keeps
/// the object's key, so the batch is not acknowledged until admitted.
#[test]
fn an_object_still_pending_keeps_its_key_and_is_not_acknowledged() {
    isolated(
        "direct_s3::an_object_still_pending_keeps_its_key_and_is_not_acknowledged",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(
                grants(),
                stored(),
                Box::new(|_, _, request| admission(request, "pending", None)),
            );
            auth(home, &fake.base, "org-fixture");
            let pending = direct(&fake.base)
                .send(&prepared, &batch, &context(&batch))
                .unwrap_err();
            assert_eq!(pending.failure, DeliveryFailure::Transient);
            let requests = fake.join();
            let put = requests.iter().find(|r| r.method == "PUT").unwrap();
            let key = format!(
                "{PREFIX}{}",
                relative_key(&batch.origin_id, &batch.batch_id, &put.body)
            );
            assert_eq!(pending.upload, Some(StoredUpload::Pending(key)));
            assert_eq!(count(&requests, "GET", "/v1/delivery/admission"), 3);
        },
    );
}

/// The host replacing the session takes the credentials asked with the old
/// one along, even when its token is unchanged: the next send asks Control
/// again and never puts under the old credentials.
#[test]
fn a_replaced_session_never_puts_under_the_old_credentials() {
    isolated(
        "direct_s3::a_replaced_session_never_puts_under_the_old_credentials",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(
                Box::new(|base, index, _| {
                    Reply::json(200, sts(base, &format!("fixture-session-token-{index}")))
                }),
                stored(),
                admits(),
            );
            auth(home, &fake.base, "org-fixture");
            let receiver = direct(&fake.base);
            receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            receiver.state.forget_session();
            receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            let requests = fake.join();
            assert_eq!(count(&requests, "POST", CREDENTIALS), 2);
            let tokens: Vec<_> = requests
                .iter()
                .filter(|r| r.method == "PUT")
                .map(|r| r.header("x-amz-security-token").unwrap())
                .collect();
            assert_eq!(
                tokens,
                ["fixture-session-token-0", "fixture-session-token-1"]
            );
        },
    );
}

/// Credentials that arrive after the host replaced the session that asked
/// for them are neither used nor kept: that batch is posted, and the next
/// send asks again.
#[test]
fn credentials_answered_across_a_session_replacement_are_dropped() {
    isolated(
        "direct_s3::credentials_answered_across_a_session_replacement_are_dropped",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let state = Arc::new(OnceLock::<ReceiverState>::new());
            let replaced = Arc::clone(&state);
            let fake = serve(
                Box::new(move |base, index, _| {
                    if index == 0 {
                        replaced.get().unwrap().forget_session();
                    }
                    Reply::json(200, sts(base, &format!("fixture-session-token-{index}")))
                }),
                stored(),
                admits(),
            );
            auth(home, &fake.base, "org-fixture");
            let receiver = direct(&fake.base);
            state.set(receiver.state.clone()).ok().unwrap();
            let posted = receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            assert_eq!(posted.acceptance_level, AcceptanceLevel::Durable);
            let queued = receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            assert_eq!(queued.acceptance_level, AcceptanceLevel::Queued);
            let requests = fake.join();
            assert_eq!(count(&requests, "POST", CREDENTIALS), 2);
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 1);
            let tokens: Vec<_> = requests
                .iter()
                .filter(|r| r.method == "PUT")
                .map(|r| r.header("x-amz-security-token").unwrap())
                .collect();
            assert_eq!(tokens, ["fixture-session-token-1"]);
        },
    );
}

/// Kept credentials put only for the batch's account: a session of
/// another account fails the send as a POST would, before any PUT.
#[test]
fn kept_credentials_never_put_for_another_account() {
    isolated(
        "direct_s3::kept_credentials_never_put_for_another_account",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let fake = serve(grants(), stored(), admits());
            auth(home, &fake.base, "org-fixture");
            let receiver = direct(&fake.base);
            receiver.send(&prepared, &batch, &context(&batch)).unwrap();
            // The same token, now the session of another account.
            auth(home, &fake.base, "org-other-fixture");
            let result = receiver.send(&prepared, &batch, &context(&batch));
            assert_eq!(failure(result), DeliveryFailure::PermissionDenied);
            let requests = fake.join();
            assert_eq!(count(&requests, "POST", CREDENTIALS), 1);
            assert_eq!(count(&requests, "PUT", "/"), 1);
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
        },
    );
}

/// An answer for another contract, without one, or naming storage the
/// client may not send credentials to is not used: the batch is posted.
#[test]
fn an_answer_this_build_cannot_use_posts_the_batch() {
    isolated(
        "direct_s3::an_answer_this_build_cannot_use_posts_the_batch",
        |home| {
            let batch = batch();
            let prepared = prepare(batch.clone()).unwrap();
            let changes: [fn(&mut Value); 7] = [
                |answer| answer["contract"] = json!("relay-upload-inbox-v2"),
                |answer| {
                    answer.as_object_mut().unwrap().remove("contract");
                },
                |answer| {
                    answer.as_object_mut().unwrap().remove("witnessSeq");
                },
                |answer| answer["endpoint"] = json!("http://10.0.0.5:9000/"),
                // Session keys always name a KMS key; null goes only with
                // presigned URLs.
                |answer| answer["kmsKeyId"] = Value::Null,
                |answer| {
                    answer.as_object_mut().unwrap().remove("kmsKeyId");
                },
                // A path prefix ends without a slash.
                |answer| {
                    let prefixed = format!("{}root/", answer["endpoint"].as_str().unwrap());
                    answer["endpoint"] = json!(prefixed);
                },
            ];
            for change in changes {
                let fake = serve(
                    Box::new(move |base, _, _| {
                        let mut answer = sts(base, "fixture-session-token");
                        change(&mut answer);
                        Reply::json(200, answer)
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
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 1);
            }
        },
    );
}

/// Through the worker, a stored object settles its batch as queued, and
/// nothing Control handed out reaches either database: credentials and the
/// witness are kept in memory only.
#[test]
fn a_drained_batch_is_queued_and_no_credential_is_stored() {
    isolated(
        "direct_s3::a_drained_batch_is_queued_and_no_credential_is_stored",
        |home| {
            let (directory, path, conn, job_id) = queue();
            let fake = serve(grants(), stored(), admits());
            auth(home, &fake.base, "org-fixture");
            let drained = drain_at(&path, &direct(&fake.base), 0);
            assert!(drained.issues.is_empty(), "{:?}", drained.issues);
            let status = delivery::status(&conn, &job_id).unwrap();
            assert_eq!(status.acceptance_level.as_deref(), Some("queued"));
            assert_eq!(status.acknowledged_records, 3);
            assert_eq!(status.pending_records, 0);
            drop(conn);
            let requests = fake.join();
            assert_eq!(count(&requests, "PUT", "/"), 1);
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
            assert_nothing_handed_out_is_stored(directory.path());
        },
    );
}

fn job_config() -> delivery::DeliveryJobConfig {
    delivery::DeliveryJobConfig {
        destination_id: "relayhistory".into(),
        instance_id: "probe".into(),
        account_id: account_id("org-fixture", None),
        mapping_version: MAPPING_VERSION.into(),
        selection: delivery::ExportSelection {
            sources: vec!["claude".into()],
            kinds: vec!["session_event".into()],
            ..Default::default()
        },
        limits: delivery::DeliveryLimits::default(),
    }
}

/// A queue over three session events and one job for them.
fn queue() -> (tempfile::TempDir, PathBuf, Connection, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    let conn = delivery::open_db(&path).unwrap();
    let evidence = Connection::open(&path).unwrap();
    for id in ["a", "b", "c"] {
        evidence.execute("INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude',?1,?1,42,'user','text',?1)", params![id]).unwrap();
    }
    drop(evidence);
    let job = delivery::create_job(&conn, &job_config(), 0).unwrap();
    (directory, path, conn, job.job_id)
}

/// One drain through `receiver`, its clock `ahead_ms` past now so a retry
/// wait an earlier pass recorded has passed.
fn drain_at(path: &Path, receiver: &RelayHistoryReceiver, ahead_ms: i64) -> worker::DrainResult {
    let receivers = worker::SingleReceiver::new("relayhistory", "probe", receiver);
    let options = worker::DrainOptions::new("direct-s3-test");
    let clock = move || worker::system_clock() + ahead_ms;
    worker::drain(path, &receivers, &options, &clock, &|| false).unwrap()
}

/// Each batch's state and the object key kept on it.
fn batches(conn: &Connection, job_id: &str) -> Vec<(String, Option<String>)> {
    conn.prepare("SELECT state,upload_key FROM delivery_batches WHERE job_id=?")
        .unwrap()
        .query_map([job_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

const DAY_MS: i64 = 86_400_000;

/// A stored object still pending keeps its batch unacknowledged with the
/// object's key (and nothing Control handed out); a later pass, in this
/// process or after a restart (a new receiver over the same queue), asks
/// admission only. A refusal then sets the batch aside as a rejected one,
/// never posted; an admission settles it as queued.
#[test]
fn a_pending_object_is_decided_on_a_later_pass_without_another_put() {
    isolated(
        "direct_s3::a_pending_object_is_decided_on_a_later_pass_without_another_put",
        |home| {
            for (restart, decision) in [(false, "refused"), (true, "refused"), (true, "admitted")] {
                let (directory, path, conn, job_id) = queue();
                let fake = serve(
                    grants(),
                    stored(),
                    Box::new(move |_, index, request| match index {
                        0..=2 => admission(request, "pending", None),
                        _ if decision == "refused" => {
                            admission(request, "refused", Some("upload_digest_mismatch"))
                        }
                        _ => admission(request, "admitted", None),
                    }),
                );
                auth(home, &fake.base, "org-fixture");
                let first = direct(&fake.base);
                drain_at(&path, &first, 0);
                let status = delivery::status(&conn, &job_id).unwrap();
                assert_eq!(status.acknowledged_records, 0, "{decision}");
                assert_eq!(status.failure.as_deref(), Some("transient"));
                let pending = batches(&conn, &job_id);
                assert_eq!(pending.len(), 1);
                assert_eq!(pending[0].0, "retry_wait");
                assert!(pending[0]
                    .1
                    .as_deref()
                    .is_some_and(|key| key.starts_with(PREFIX)));
                assert_nothing_handed_out_is_stored(directory.path());

                let later = if restart {
                    drop(first);
                    direct(&fake.base)
                } else {
                    first
                };
                drain_at(&path, &later, DAY_MS);
                let status = delivery::status(&conn, &job_id).unwrap();
                let settled = batches(&conn, &job_id);
                if decision == "refused" {
                    assert_eq!(status.state, "blocked");
                    assert_eq!(status.failure.as_deref(), Some("receiver_rejected"));
                    assert_eq!(status.acknowledged_records, 0);
                    assert_eq!(settled[0].0, "blocked");
                    // Set aside: a later pass sends nothing at all.
                    drain_at(&path, &later, 2 * DAY_MS);
                } else {
                    assert_eq!(status.acceptance_level.as_deref(), Some("queued"));
                    assert_eq!(status.acknowledged_records, 3);
                    // An acknowledged batch keeps no key, if it is kept.
                    assert!(settled.iter().all(|(_, key)| key.is_none()));
                }
                drop(conn);
                let requests = fake.join();
                assert_eq!(count(&requests, "POST", CREDENTIALS), 1);
                assert_eq!(count(&requests, "PUT", "/"), 1);
                assert_eq!(count(&requests, "GET", "/v1/delivery/admission"), 4);
                assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
                assert_nothing_handed_out_is_stored(directory.path());
            }
        },
    );
}

/// Through the worker, a refusal sets the batch aside as receiver_rejected:
/// a subsequent drain neither posts its records nor puts it again.
#[test]
fn a_refused_object_sets_its_batch_aside_and_is_never_posted() {
    isolated(
        "direct_s3::a_refused_object_sets_its_batch_aside_and_is_never_posted",
        |home| {
            let (directory, path, conn, job_id) = queue();
            let fake = serve(
                grants(),
                stored(),
                Box::new(|_, _, request| {
                    admission(request, "refused", Some("delivery_privacy_superseded"))
                }),
            );
            auth(home, &fake.base, "org-fixture");
            let receiver = direct(&fake.base);
            drain_at(&path, &receiver, 0);
            drain_at(&path, &receiver, DAY_MS);
            let status = delivery::status(&conn, &job_id).unwrap();
            assert_eq!(status.state, "blocked");
            assert_eq!(status.failure.as_deref(), Some("receiver_rejected"));
            assert_eq!(status.acknowledged_records, 0);
            assert_eq!(batches(&conn, &job_id)[0].0, "blocked");
            drop(conn);
            let requests = fake.join();
            assert_eq!(count(&requests, "PUT", "/"), 1);
            assert_eq!(count(&requests, "GET", "/v1/delivery/admission"), 1);
            assert_eq!(count(&requests, "POST", "/v1/delivery/batches"), 0);
            assert_nothing_handed_out_is_stored(directory.path());
        },
    );
}

/// Every value of every row in every database under `directory`, and the
/// raw bytes of every file there (journals included), are free of what
/// Control handed out.
fn assert_nothing_handed_out_is_stored(directory: &std::path::Path) {
    const HANDED_OUT: [&str; 7] = [
        "AKIDFIXTURE",
        "fixture-secret",
        "fixture-session-token",
        "fixture.witness",
        // A presigned URL's signature, and the query that carries it.
        "fixture-presigned-signature",
        "X-Amz-Signature",
        "fixture-bucket/",
    ];
    let mut databases = 0;
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        for secret in HANDED_OUT {
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "{secret} in {}",
                path.display()
            );
        }
        if path.extension().is_some_and(|ext| ext == "db") {
            databases += 1;
            let conn = rusqlite::Connection::open(&path).unwrap();
            let tables: Vec<String> = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table'")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for table in tables {
                let mut rows = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
                let columns = rows.column_count();
                let mut cursor = rows.query([]).unwrap();
                while let Some(row) = cursor.next().unwrap() {
                    for column in 0..columns {
                        let value = match row.get_ref(column).unwrap() {
                            rusqlite::types::ValueRef::Text(text) => text.to_vec(),
                            rusqlite::types::ValueRef::Blob(blob) => blob.to_vec(),
                            _ => continue,
                        };
                        let value = String::from_utf8_lossy(&value);
                        for secret in HANDED_OUT {
                            assert!(!value.contains(secret), "{secret} in {table}");
                        }
                    }
                }
            }
        }
    }
    // The job lives in delivery-queue.db beside history.db.
    assert!(databases >= 2, "{databases}");
}
