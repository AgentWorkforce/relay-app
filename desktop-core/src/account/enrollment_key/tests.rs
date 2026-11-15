use super::*;
use crate::contract::account::SharingMode;

const KEY: &str = "sh_ek_unit-test-key-0123456789";
const SITE: &str = "https://agentrelay.com";

fn request(flow: Option<Flow>, file: Option<&str>) -> SignInRequest {
    SignInRequest {
        workspace: None,
        sharing: SharingMode::Selected,
        acknowledge_uninspected_schedules: false,
        flow,
        provider: None,
        enrollment_key_file: file.map(PathBuf::from),
    }
}

fn reply(status: u16, body: Value, retry_after: Option<u64>) -> Reply {
    Reply {
        status,
        body,
        retry_after,
    }
}

fn failure(outcome: Outcome) -> LoginError {
    match outcome {
        Outcome::Failed(error) => error,
        _ => panic!("expected a failure"),
    }
}

#[test]
fn the_flow_and_the_key_file_go_together() {
    let keyed = Some(Flow::EnrollmentKey);
    assert!(check_request(&request(keyed, Some("/tmp/key"))).is_ok());
    assert!(check_request(&request(Some(Flow::Device), None)).is_ok());
    assert!(check_request(&request(None, None)).is_ok());
    assert!(check_request(&request(keyed, None)).is_err());
    for flow in [None, Some(Flow::Device), Some(Flow::Browser)] {
        assert!(check_request(&request(flow, Some("/tmp/key"))).is_err());
    }
}

#[test]
fn the_flow_is_spelled_enrollment_key_on_the_wire() {
    let parsed: SignInRequest = serde_json::from_value(json!({
        "workspace": null, "sharing": "selected", "flow": "enrollment_key",
        "enrollment_key_file": "/tmp/key"
    }))
    .unwrap();
    assert_eq!(parsed.flow, Some(Flow::EnrollmentKey));
    assert_eq!(parsed.enrollment_key_file, Some(PathBuf::from("/tmp/key")));
    let device = serde_json::to_value(request(Some(Flow::Device), None)).unwrap();
    assert!(device.get("enrollment_key_file").is_none());
}

#[test]
fn a_key_is_read_from_an_absolute_file_and_never_echoed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("key");
    std::fs::write(&file, format!("  {KEY}\n")).unwrap();
    assert_eq!(read_key(&file).unwrap().expose(), KEY);
    assert!(read_key(Path::new("key")).is_err());
    assert!(read_key(&dir.path().join("missing")).is_err());
    assert!(read_key(dir.path()).is_err());
    for bad in [
        "".to_string(),
        " \n".into(),
        format!("{KEY} extra"),
        "x".repeat(5000),
    ] {
        std::fs::write(&file, &bad).unwrap();
        let Err(error) = read_key(&file) else {
            panic!("refused")
        };
        assert!(!error.message.contains(KEY));
    }
    std::fs::write(&file, format!("{KEY}\u{7}")).unwrap();
    let Err(error) = read_key(&file) else {
        panic!("refused")
    };
    assert!(!error.message.contains(KEY));
}

#[test]
fn the_host_name_is_trimmed_cleaned_and_cut_to_eighty_units() {
    assert_eq!(
        host_name("  Test Desk.local \n").as_deref(),
        Some("Test Desk.local")
    );
    assert_eq!(host_name("desk\u{0}top").as_deref(), Some("desktop"));
    assert_eq!(host_name(&"a".repeat(200)).unwrap().len(), 80);
    assert_eq!(
        host_name(&"é".repeat(100)).unwrap().encode_utf16().count(),
        80
    );
    assert_eq!(
        host_name(&"😀".repeat(50)).unwrap().encode_utf16().count(),
        80
    );
    assert_eq!(host_name(" \t "), None);
}

#[test]
fn a_200_builds_the_device_sign_ins_session() {
    let body = json!({"data": {"access_token": "access-1", "refresh_token": "refresh-1",
        "access_token_expires_at": "2099-01-01T00:00:00Z", "api_url": format!("{SITE}/cloud/")}});
    let Outcome::Session(session) = outcome(&reply(200, body, None), SITE) else {
        panic!("expected a session");
    };
    assert_eq!(session.access_token.expose(), "access-1");
    assert_eq!(
        session.refresh_token.as_ref().map(Secret::expose),
        Some("refresh-1")
    );
    assert_eq!(
        session.access_token_expires_at.as_deref(),
        Some("2099-01-01T00:00:00Z")
    );
    assert_eq!(session.site_url, SITE);
    let moved = json!({"access_token": "a", "api_url": "https://elsewhere.example/cloud"});
    assert_eq!(
        failure(outcome(&reply(200, moved, None), SITE)).state,
        "error"
    );
    assert_eq!(
        failure(outcome(&reply(200, json!({}), None), SITE)).state,
        "error"
    );
}

#[test]
fn every_rejection_reads_the_same() {
    let rejected =
        |status, code: &str| failure(outcome(&reply(status, json!({"error": code}), None), SITE));
    let first = rejected(400, "invalid_grant");
    assert_eq!(first.state, REJECTED);
    assert_eq!(rejected(400, "invalid_request"), first);
    assert_eq!(ended_phase(first.state).as_deref(), Some(REJECTED));
    assert!(!first.message.contains("expired") && !first.message.contains("revoked"));
}

#[test]
fn slow_down_waits_as_long_as_cloud_asks_within_bounds() {
    let wait = |status, retry_after| match outcome(
        &reply(status, json!({"error": "slow_down"}), retry_after),
        SITE,
    ) {
        Outcome::Wait(seconds) => seconds,
        _ => panic!("expected a wait"),
    };
    assert_eq!(wait(429, Some(7)), 7);
    assert_eq!(wait(429, None), DEFAULT_RETRY_AFTER);
    assert_eq!(wait(429, Some(3600)), MAX_RETRY_AFTER);
    assert_eq!(wait(400, Some(2)), 2);
}

#[test]
fn an_old_server_is_told_apart_from_other_failures() {
    let old = failure(outcome(
        &reply(404, json!({"error": "not_found"}), None),
        SITE,
    ));
    assert_eq!(old.state, UNSUPPORTED);
    assert_eq!(ended_phase(old.state).as_deref(), Some(UNSUPPORTED));
    let broken = failure(outcome(&reply(500, Value::Null, None), SITE));
    assert_eq!(broken.state, "error");
    assert_eq!(ended_phase(broken.state), None);
    assert_eq!(ended_phase(BUSY).as_deref(), Some(BUSY));
}
