//! The bridge check's decisions, with throwaway tokens only. Its work
//! against a fake Cloud is in `workspace_recovery_tests::session_bridge`.
use super::*;

fn session(access: &str) -> CloudSession {
    CloudSession {
        access_token: Secret::new(access.to_string()),
        access_token_expires_at: None,
        refresh_token: Some(Secret::new(format!("{access}-refresh"))),
        refresh_token_expires_at: None,
        site_url: "https://example.test".into(),
    }
}

fn binding() -> CloudWorkspace {
    CloudWorkspace {
        key: Secret::new("rk_live_test-only".to_string()),
        base_url: "https://relaycast.example.test".into(),
        workspace_id: Some("rw_test".into()),
        account_workspace_id: Some("00000000-0000-4000-8000-000000000001".into()),
        ..CloudWorkspace::default()
    }
}

#[test]
fn the_flag_is_off_unless_it_says_on() {
    assert!(!Bridge::from_env(&BTreeMap::new()).is_enabled());
    for off in ["", "0", "false", "off", "no", "maybe"] {
        let env = BTreeMap::from([(FLAG.to_string(), off.to_string())]);
        assert!(!Bridge::from_env(&env).is_enabled(), "{off:?}");
    }
    for on in ["1", "true", " ON ", "yes"] {
        let env = BTreeMap::from([(FLAG.to_string(), on.to_string())]);
        assert!(Bridge::from_env(&env).is_enabled(), "{on:?}");
    }
}

#[test]
fn only_a_cloud_to_control_rotation_is_a_crossing() {
    let cloud = session("cloud-access-a");
    let control = session("sh_at_control-a");
    assert!(crossed_to_control(&cloud, &control));
    assert!(!crossed_to_control(&cloud, &session("cloud-access-b")));
    assert!(!crossed_to_control(&control, &session("sh_at_control-b")));
    assert!(!crossed_to_control(&control, &cloud));
    let elsewhere = CloudSession {
        site_url: "https://other.example.test".into(),
        ..control
    };
    assert!(!crossed_to_control(&cloud, &elsewhere));
}

#[test]
fn an_unchanged_binding_is_kept() {
    assert_eq!(binding_kept(&binding(), &binding()), Ok(()));
    // A trailing slash is the same origin.
    let after = CloudWorkspace {
        base_url: "https://relaycast.example.test/".into(),
        ..binding()
    };
    assert_eq!(binding_kept(&binding(), &after), Ok(()));
}

#[test]
fn a_changed_binding_names_what_changed() {
    let gate = CloudWorkspace {
        gate_url: Some("https://gate.example.test".into()),
        ..binding()
    };
    assert_eq!(binding_kept(&binding(), &gate), Err(Mismatch::Gate));
    let origin = CloudWorkspace {
        base_url: "https://elsewhere.example.test".into(),
        ..binding()
    };
    assert_eq!(binding_kept(&binding(), &origin), Err(Mismatch::Origin));
    let key = CloudWorkspace {
        key: Secret::new("rk_live_other-test-only".to_string()),
        ..binding()
    };
    assert_eq!(binding_kept(&binding(), &key), Err(Mismatch::Key));
    let workspace = CloudWorkspace {
        workspace_id: Some("rw_other".into()),
        ..binding()
    };
    assert_eq!(
        binding_kept(&binding(), &workspace),
        Err(Mismatch::Workspace)
    );
}

#[test]
fn a_mismatch_report_names_no_secret() {
    for mismatch in [
        Mismatch::Gate,
        Mismatch::Origin,
        Mismatch::Key,
        Mismatch::Workspace,
    ] {
        let text = mismatch.to_string();
        assert!(!text.contains("rk_live"), "{text}");
        assert!(text.contains("keeps its current connection"), "{text}");
    }
}
