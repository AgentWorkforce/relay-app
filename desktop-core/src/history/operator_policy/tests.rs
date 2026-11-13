use super::*;
use agent_relay_node::relay::cloud_session::{AgentOwner, CloudAccount};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::collections::VecDeque;

const WORKSPACE: &str = "11111111-2222-3333-4444-555555555555";

/// Answers `access/me` reads from a script, in order.
#[derive(Default)]
struct FakeCloud {
    answers: Mutex<VecDeque<Result<Value, CloudError>>>,
    paths: Mutex<Vec<String>>,
}

impl FakeCloud {
    fn answering(answers: Vec<Result<Value, CloudError>>) -> FakeCloud {
        FakeCloud {
            answers: Mutex::new(answers.into()),
            paths: Mutex::default(),
        }
    }
}

impl CloudAccess for FakeCloud {
    fn account(&self) -> Option<CloudAccount> {
        None
    }

    fn agent_owner(&self) -> Option<AgentOwner> {
        None
    }

    fn request(
        &self,
        _method: &'static str,
        path: String,
        _body: Option<Value>,
    ) -> BoxFuture<'_, Result<Value, CloudError>> {
        self.paths.lock().unwrap().push(path);
        let answer = self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(CloudError::Unreachable));
        Box::pin(async move { answer })
    }
}

fn me(policy: Value) -> Result<Value, CloudError> {
    Ok(json!({ "role": "member", "grant": null, "policy": policy }))
}

fn all(revision: i64) -> Value {
    json!({ "version": 1, "revision": revision, "settings": { "sharing_mode": "all" } })
}

fn policy() -> (tempfile::TempDir, OperatorPolicy) {
    let dir = tempfile::tempdir().unwrap();
    let policy = OperatorPolicy::new(dir.path(), None);
    (dir, policy)
}

#[test]
fn the_policy_object_is_parsed() {
    let fetched = parse_access_me(&json!({ "role": "member", "policy": all(7) }));
    let Fetched::Policy(document) = fetched else {
        panic!("{fetched:?}")
    };
    assert_eq!(document.version, 1);
    assert_eq!(document.revision, 7);
    assert_eq!(document.settings[SHARING_MODE], json!("all"));
    assert_eq!(
        access_me_path(WORKSPACE),
        format!("/workspaces/{WORKSPACE}/access/me")
    );
    // A sharing mode this build cannot read relaxes nothing.
    let unreadable = json!({ "policy": { "version": 1, "revision": 8, "settings": { "sharing_mode": "every" } } });
    assert_eq!(parse_access_me(&unreadable), Fetched::Failed);
    assert_eq!(
        parse_access_me(&json!({ "policy": "all" })),
        Fetched::Failed
    );
}

#[test]
fn an_absent_policy_field_is_user_mode() {
    assert_eq!(
        parse_access_me(&json!({ "role": "member" })),
        Fetched::Absent
    );
    assert_eq!(parse_access_me(&json!({ "policy": null })), Fetched::Absent);
    let entry = apply(None, WORKSPACE, Fetched::Absent, 1);
    assert_eq!(standing(entry.as_ref()), Standing::User);
    // An install with nothing cached keeps the person's mode until its
    // first read.
    let (_dir, policy) = policy();
    assert_eq!(policy.standing(Some(WORKSPACE)), Standing::User);
    assert!(policy.enforced_settings(Some(WORKSPACE)).is_empty());
}

#[tokio::test]
async fn a_failed_read_with_nothing_cached_withholds_uploads() {
    let (_dir, policy) = policy();
    policy.sign_in(WORKSPACE);
    let cloud = FakeCloud::answering(vec![Err(CloudError::Unreachable)]);
    assert_eq!(policy.refresh(&cloud, WORKSPACE).await, Standing::Withheld);
    assert!(
        policy.due(WORKSPACE),
        "a withheld workspace is read again at once"
    );
    let refused = policy.check_resume(Some(WORKSPACE)).unwrap_err();
    assert_eq!(refused.message, WITHHELD);
    // A confirmed absent policy ends the wait.
    let cloud = FakeCloud::answering(vec![Ok(json!({ "role": "member" }))]);
    assert_eq!(policy.refresh(&cloud, WORKSPACE).await, Standing::User);
    assert!(policy.check_resume(Some(WORKSPACE)).is_ok());
    assert!(!policy.due(WORKSPACE));
}

#[tokio::test]
async fn cloud_not_serving_the_route_is_no_policy() {
    let (_dir, policy) = policy();
    policy.sign_in(WORKSPACE);
    let cloud = FakeCloud::answering(vec![Err(CloudError::Status {
        status: 404,
        message: None,
    })]);
    assert_eq!(policy.refresh(&cloud, WORKSPACE).await, Standing::User);
}

#[tokio::test]
async fn a_failed_read_with_a_cached_policy_keeps_its_mode() {
    let dir = tempfile::tempdir().unwrap();
    let policy = OperatorPolicy::new(dir.path(), None);
    policy.sign_in(WORKSPACE);
    let cloud = FakeCloud::answering(vec![me(all(3)), Err(CloudError::Unreachable)]);
    policy.refresh(&cloud, WORKSPACE).await;
    let standing = policy.refresh(&cloud, WORKSPACE).await;
    assert!(matches!(standing, Standing::Enforced(_)), "{standing:?}");
    assert_eq!(
        policy.enforced_sharing(Some(WORKSPACE)),
        Some(SharingMode::All)
    );
    assert_eq!(
        cloud.paths.lock().unwrap().as_slice(),
        [access_me_path(WORKSPACE), access_me_path(WORKSPACE)]
    );
    // The cache outlives the process, and is kept per workspace.
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(FILE)).unwrap()).unwrap();
    let entry = &saved["workspaces"][WORKSPACE];
    assert_eq!(entry["workspace_id"], WORKSPACE);
    assert_eq!(entry["revision"], 3);
    assert!(entry["fetched_at"].is_i64());
    assert_eq!(entry["settings"], json!({ "sharing_mode": "all" }));
    let reopened = OperatorPolicy::new(dir.path(), None);
    assert_eq!(
        reopened.enforced_sharing(Some(WORKSPACE)),
        Some(SharingMode::All)
    );
    assert_eq!(reopened.enforced_sharing(Some("another")), None);
}

#[test]
fn a_cached_enforced_mode_never_relaxes_offline() {
    let enforced = apply(None, WORKSPACE, Fetched::Policy(document(5)), 1);
    let offline = apply(enforced.clone(), WORKSPACE, Fetched::Failed, 2);
    assert_eq!(offline, enforced);
    assert!(matches!(standing(offline.as_ref()), Standing::Enforced(_)));
    // An older revision does not replace it either.
    let older = Document {
        settings: BTreeMap::new(),
        ..document(4)
    };
    let stale = apply(offline, WORKSPACE, Fetched::Policy(older), 3).unwrap();
    assert_eq!(stale.revision, Some(5));
    assert_eq!(stale.settings[SHARING_MODE], json!("all"));
    // The same revision applies.
    let same = Document {
        settings: BTreeMap::new(),
        ..document(5)
    };
    let applied = apply(Some(stale), WORKSPACE, Fetched::Policy(same), 4);
    assert_eq!(standing(applied.as_ref()), Standing::User);
}

#[tokio::test]
async fn a_successful_read_without_the_field_relaxes() {
    let (_dir, policy) = policy();
    let cloud = FakeCloud::answering(vec![me(all(2)), Ok(json!({ "role": "member" }))]);
    policy.refresh(&cloud, WORKSPACE).await;
    assert_eq!(
        policy.enforced_sharing(Some(WORKSPACE)),
        Some(SharingMode::All)
    );
    assert_eq!(policy.refresh(&cloud, WORKSPACE).await, Standing::User);
    assert_eq!(policy.enforced_sharing(Some(WORKSPACE)), None);
}

#[tokio::test]
async fn a_person_cannot_change_an_enforced_setting() {
    let (_dir, policy) = policy();
    let cloud = FakeCloud::answering(vec![me(all(1))]);
    policy.refresh(&cloud, WORKSPACE).await;
    let refused = policy
        .check_sharing(Some(WORKSPACE), SharingMode::Selected)
        .unwrap_err();
    assert_eq!(refused.message, ENFORCED);
    assert_eq!(refused.data.kind, ErrorKind::Refused);
    assert!(policy
        .check_sharing(Some(WORKSPACE), SharingMode::All)
        .is_ok());
    assert_eq!(
        policy.check_exclusion(Some(WORKSPACE)).unwrap_err().message,
        ENFORCED
    );
    // Another workspace is the person's.
    assert!(policy
        .check_sharing(Some("another"), SharingMode::New)
        .is_ok());
    // The person's own choice is kept for when the operator lifts it.
    policy.remember(WORKSPACE, SHARING_MODE, json!("new"));
    policy.remember(WORKSPACE, SHARING_MODE, json!("all"));
    assert_eq!(
        policy.remembered(WORKSPACE, SHARING_MODE),
        Some(json!("new"))
    );
    policy.forget(WORKSPACE, SHARING_MODE);
    assert_eq!(policy.remembered(WORKSPACE, SHARING_MODE), None);
}

#[tokio::test]
async fn the_environment_stands_in_for_cloud_in_tests() {
    let dir = tempfile::tempdir().unwrap();
    let policy = OperatorPolicy::new(dir.path(), Some(all(9).to_string()));
    let cloud = FakeCloud::default();
    policy.refresh(&cloud, WORKSPACE).await;
    assert_eq!(
        policy.enforced_sharing(Some(WORKSPACE)),
        Some(SharingMode::All)
    );
    assert!(cloud.paths.lock().unwrap().is_empty());
    let file = dir.path().join("operator.json");
    std::fs::write(&file, "null").unwrap();
    let policy = OperatorPolicy::new(dir.path(), Some(file.display().to_string()));
    assert_eq!(policy.refresh(&cloud, WORKSPACE).await, Standing::User);
}

/// A policy document enforcing `settings`.
fn enforcing(settings: Value) -> Value {
    json!({ "version": 1, "revision": 3, "policy_note": "kept", "settings": settings })
}

#[test]
fn each_operator_control_is_parsed_and_an_unreadable_value_relaxes_nothing() {
    let valid = [
        json!({ "upload_control": "operator" }),
        json!({ "upload_control": "user" }),
        json!({ "oops_window_seconds": 0 }),
        json!({ "oops_window_seconds": 900 }),
        json!({ "oops_window_seconds": 86_400 }),
        json!({ "user_retroactive_delete": true }),
        json!({ "user_retroactive_delete": false }),
        json!({ "upload_control": "operator", "oops_window_seconds": 900,
                "user_retroactive_delete": false, "sharing_mode": "all",
                "a_future_setting": { "kept": true } }),
    ];
    for settings in valid {
        let fetched = parse_access_me(&json!({ "policy": enforcing(settings.clone()) }));
        let Fetched::Policy(document) = fetched else {
            panic!("{settings}: {fetched:?}")
        };
        assert_eq!(
            Value::Object(document.settings.into_iter().collect()),
            settings
        );
    }
    let invalid = [
        json!({ "upload_control": "admin" }),
        json!({ "upload_control": true }),
        json!({ "upload_control": null }),
        json!({ "oops_window_seconds": -1 }),
        json!({ "oops_window_seconds": 86_401 }),
        json!({ "oops_window_seconds": 1.5 }),
        json!({ "oops_window_seconds": "900" }),
        json!({ "user_retroactive_delete": "false" }),
        json!({ "user_retroactive_delete": 0 }),
        json!({ "user_retroactive_delete": null }),
        json!({ "sharing_mode": "all", "upload_control": "nobody" }),
    ];
    for settings in invalid {
        assert_eq!(
            parse_access_me(&json!({ "policy": enforcing(settings.clone()) })),
            Fetched::Failed,
            "{settings}"
        );
    }
}

#[tokio::test]
async fn an_absent_operator_control_is_the_persons() {
    let (_dir, policy) = policy();
    let cloud = FakeCloud::answering(vec![
        me(json!({ "version": 1, "revision": 1, "settings": {} })),
        me(all(2)),
    ]);
    assert_eq!(policy.refresh(&cloud, WORKSPACE).await, Standing::User);
    for _ in 0..2 {
        assert!(!policy.controls_uploads(Some(WORKSPACE)));
        assert!(policy.check_pause(Some(WORKSPACE)).is_ok());
        assert!(policy.check_delete(Some(WORKSPACE)).is_ok());
        policy.refresh(&cloud, WORKSPACE).await;
    }
    assert_eq!(
        policy.enforced_settings(Some(WORKSPACE)),
        BTreeMap::from([(SHARING_MODE.to_string(), "all".to_string())])
    );
}

#[tokio::test]
async fn operator_controls_refuse_the_persons_pause_and_delete_and_are_reported() {
    let (_dir, policy) = policy();
    let cloud = FakeCloud::answering(vec![
        me(enforcing(json!({ "upload_control": "operator",
            "oops_window_seconds": 900, "user_retroactive_delete": false }))),
        me(json!({ "version": 1, "revision": 4,
            "settings": { "upload_control": "user", "user_retroactive_delete": true } })),
    ]);
    policy.refresh(&cloud, WORKSPACE).await;
    assert!(policy.controls_uploads(Some(WORKSPACE)));
    for refused in [
        policy.check_pause(Some(WORKSPACE)),
        policy.check_delete(Some(WORKSPACE)),
    ] {
        let refused = refused.unwrap_err();
        assert_eq!(refused.message, ENFORCED);
        assert_eq!(refused.data.kind, ErrorKind::Refused);
    }
    assert_eq!(
        policy.enforced_settings(Some(WORKSPACE)),
        BTreeMap::from([
            (OOPS_WINDOW_SECONDS.to_string(), "900".to_string()),
            (UPLOAD_CONTROL.to_string(), "operator".to_string()),
            (USER_RETROACTIVE_DELETE.to_string(), "false".to_string()),
        ])
    );
    // Another workspace is the person's.
    assert!(policy.check_pause(Some("another")).is_ok());
    assert!(policy.check_delete(Some("another")).is_ok());
    // The person's own values, enforced, refuse nothing.
    policy.refresh(&cloud, WORKSPACE).await;
    assert!(!policy.controls_uploads(Some(WORKSPACE)));
    assert!(policy.check_pause(Some(WORKSPACE)).is_ok());
    assert!(policy.check_delete(Some(WORKSPACE)).is_ok());
    assert_eq!(
        policy.enforced_settings(Some(WORKSPACE)),
        BTreeMap::from([
            (UPLOAD_CONTROL.to_string(), "user".to_string()),
            (USER_RETROACTIVE_DELETE.to_string(), "true".to_string()),
        ])
    );
}

#[tokio::test]
async fn an_unreadable_operator_control_keeps_the_cached_policy() {
    let (_dir, policy) = policy();
    let cloud = FakeCloud::answering(vec![
        me(enforcing(json!({ "user_retroactive_delete": false }))),
        me(json!({ "version": 1, "revision": 9,
            "settings": { "user_retroactive_delete": "yes" } })),
    ]);
    policy.refresh(&cloud, WORKSPACE).await;
    policy.refresh(&cloud, WORKSPACE).await;
    assert_eq!(
        policy.check_delete(Some(WORKSPACE)).unwrap_err().message,
        ENFORCED
    );
    assert_eq!(policy.entry(WORKSPACE).unwrap().revision, Some(3));
}

fn document(revision: i64) -> Document {
    Document {
        version: 1,
        revision,
        settings: BTreeMap::from([(SHARING_MODE.to_string(), json!("all"))]),
    }
}

#[tokio::test]
async fn the_oops_window_holds_only_under_operator_control() {
    let (dir, policy) = policy();
    // Nothing cached and not signed in: uploads are immediate.
    assert_eq!(upload_hold(dir.path(), ""), UploadHold::None);
    assert_eq!(upload_hold(dir.path(), WORKSPACE), UploadHold::None);
    let cloud = FakeCloud::answering(vec![
        me(enforcing(json!({
            "upload_control": "operator", "oops_window_seconds": 900, "sharing_mode": "all"
        }))),
        me(json!({ "version": 1, "revision": 4,
            "settings": { "upload_control": "operator", "oops_window_seconds": 0, "sharing_mode": "all" } })),
        me(json!({ "version": 1, "revision": 5,
            "settings": { "upload_control": "user", "oops_window_seconds": 900 } })),
        me(json!({ "version": 1, "revision": 6,
            "settings": { "upload_control": "operator", "sharing_mode": "all" } })),
    ]);
    policy.refresh(&cloud, WORKSPACE).await;
    assert_eq!(
        upload_hold(dir.path(), WORKSPACE),
        UploadHold::Window(900_000)
    );
    assert_eq!(policy.oops_window_ms(Some(WORKSPACE)), Some(900_000));
    assert_eq!(
        policy.enforced_settings(Some(WORKSPACE))[OOPS_WINDOW_SECONDS],
        "900"
    );
    // With a window, a session may be kept back while it waits: the
    // collector refuses it per session once its window has ended.
    assert!(policy.check_exclusion(Some(WORKSPACE)).is_ok());
    // A zero window, a window the person controls, and no window at all
    // upload at once, and the window is not reported as enforced.
    for _ in 0..3 {
        policy.refresh(&cloud, WORKSPACE).await;
        assert_eq!(upload_hold(dir.path(), WORKSPACE), UploadHold::None);
        assert_eq!(policy.oops_window_ms(Some(WORKSPACE)), None);
        assert!(!policy
            .enforced_settings(Some(WORKSPACE))
            .contains_key(OOPS_WINDOW_SECONDS));
    }
    // Everything enforced to upload, without a window: exclusion is refused.
    assert_eq!(
        policy.check_exclusion(Some(WORKSPACE)).unwrap_err().message,
        ENFORCED
    );
    // Another workspace is not held.
    assert_eq!(upload_hold(dir.path(), "another"), UploadHold::None);
}

#[test]
fn an_unreadable_cache_blocks_uploads_while_signed_in() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), "{ not json").unwrap();
    assert_eq!(upload_hold(dir.path(), WORKSPACE), UploadHold::Blocked);
    assert_eq!(upload_hold(dir.path(), ""), UploadHold::None);
    let policy = OperatorPolicy::new(dir.path(), None);
    assert_eq!(policy.standing(Some(WORKSPACE)), Standing::Withheld);
    assert_eq!(policy.standing(None), Standing::User);
    // A cached window this build cannot read is unreadable too.
    std::fs::write(
        dir.path().join(FILE),
        serde_json::to_vec(&json!({ "workspaces": { WORKSPACE: {
            "workspace_id": WORKSPACE, "revision": 3, "fetched_at": 1,
            "settings": { "upload_control": "operator", "oops_window_seconds": 86_401 } } } }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(upload_hold(dir.path(), WORKSPACE), UploadHold::Blocked);
    assert_eq!(
        OperatorPolicy::new(dir.path(), None).standing(Some(WORKSPACE)),
        Standing::Withheld
    );
}

#[tokio::test]
async fn a_withheld_policy_blocks_uploads() {
    let (dir, policy) = policy();
    // Signed in, with no policy read yet.
    policy.sign_in(WORKSPACE);
    assert_eq!(upload_hold(dir.path(), WORKSPACE), UploadHold::Blocked);
    // The first read ends the hold.
    let cloud = FakeCloud::answering(vec![Ok(json!({ "role": "member" }))]);
    policy.refresh(&cloud, WORKSPACE).await;
    assert_eq!(upload_hold(dir.path(), WORKSPACE), UploadHold::None);
}

#[test]
fn a_withheld_policy_refuses_the_persons_deletion() {
    let (_dir, policy) = policy();
    // Signed in, entry never fetched, no build defaults.
    policy.sign_in(WORKSPACE);
    assert_eq!(policy.entry(WORKSPACE).unwrap().fetched_at, None);
    assert_eq!(policy.standing(Some(WORKSPACE)), Standing::Withheld);
    let refused = policy.check_delete(Some(WORKSPACE)).unwrap_err();
    assert_eq!(
        refused.message,
        "Deleting from Cloud waits until your organization's upload policy can be checked."
    );
    assert_eq!(refused.data.kind, ErrorKind::Refused);
}

/// Build-time defaults: a profile's operator runs uploads, with a window.
fn defaults() -> BTreeMap<String, Value> {
    BTreeMap::from([
        (SHARING_MODE.to_string(), json!("all")),
        (UPLOAD_CONTROL.to_string(), json!("operator")),
        (OOPS_WINDOW_SECONDS.to_string(), json!(600)),
        (USER_RETROACTIVE_DELETE.to_string(), json!(false)),
    ])
}

fn defaulted() -> (tempfile::TempDir, OperatorPolicy) {
    let dir = tempfile::tempdir().unwrap();
    let policy = OperatorPolicy::with_defaults(dir.path(), None, Some(defaults()));
    (dir, policy)
}

#[test]
fn defaults_apply_before_the_first_read() {
    let (dir, policy) = defaulted();
    // Signed in, nothing cached: the defaults, not a withheld wait.
    assert_eq!(
        policy.standing(Some(WORKSPACE)),
        Standing::Enforced(defaults())
    );
    policy.sign_in(WORKSPACE);
    assert_eq!(
        policy.standing(Some(WORKSPACE)),
        Standing::Enforced(defaults())
    );
    assert!(!policy.withholds(Some(WORKSPACE)));
    assert!(policy.check_resume(Some(WORKSPACE)).is_ok());
    assert_eq!(
        policy.enforced_sharing(Some(WORKSPACE)),
        Some(SharingMode::All)
    );
    assert!(policy.controls_uploads(Some(WORKSPACE)));
    assert_eq!(policy.oops_window_ms(Some(WORKSPACE)), Some(600_000));
    assert_eq!(
        policy.enforced_settings(Some(WORKSPACE)),
        BTreeMap::from([
            (SHARING_MODE.to_string(), "all".to_string()),
            (UPLOAD_CONTROL.to_string(), "operator".to_string()),
            (OOPS_WINDOW_SECONDS.to_string(), "600".to_string()),
            (USER_RETROACTIVE_DELETE.to_string(), "false".to_string()),
        ])
    );
    // The person's changes are refused as under a read policy.
    assert_eq!(
        policy.check_pause(Some(WORKSPACE)).unwrap_err().message,
        ENFORCED
    );
    assert_eq!(
        policy.check_delete(Some(WORKSPACE)).unwrap_err().message,
        ENFORCED
    );
    assert_eq!(
        policy
            .check_sharing(Some(WORKSPACE), SharingMode::New)
            .unwrap_err()
            .message,
        ENFORCED
    );
    // Not signed in, nothing applies.
    assert_eq!(policy.standing(None), Standing::User);
    assert!(policy.enforced_settings(None).is_empty());
    // The first read is still wanted at every tick.
    assert!(policy.due(WORKSPACE));
    assert_eq!(
        upload_hold_with(dir.path(), WORKSPACE, Some(&defaults())),
        UploadHold::Window(600_000)
    );
}

#[tokio::test]
async fn defaults_apply_offline_and_over_an_unreadable_cache() {
    let (dir, policy) = defaulted();
    policy.sign_in(WORKSPACE);
    let cloud = FakeCloud::answering(vec![Err(CloudError::Unreachable)]);
    assert_eq!(
        policy.refresh(&cloud, WORKSPACE).await,
        Standing::Enforced(defaults())
    );
    assert!(
        policy.due(WORKSPACE),
        "the first read is tried again at once"
    );
    // A restart offline: the cache holds the sign-in, never read.
    let reopened = OperatorPolicy::with_defaults(dir.path(), None, Some(defaults()));
    assert_eq!(policy.entry(WORKSPACE).unwrap().fetched_at, None);
    assert_eq!(
        reopened.standing(Some(WORKSPACE)),
        Standing::Enforced(defaults())
    );
    // An unreadable cache.
    std::fs::write(dir.path().join(FILE), "{ not json").unwrap();
    let unreadable = OperatorPolicy::with_defaults(dir.path(), None, Some(defaults()));
    assert_eq!(
        unreadable.standing(Some(WORKSPACE)),
        Standing::Enforced(defaults())
    );
    assert!(unreadable.due(WORKSPACE));
    assert_eq!(
        upload_hold_with(dir.path(), WORKSPACE, Some(&defaults())),
        UploadHold::Window(600_000)
    );
    // Defaults that leave every setting to the person never withhold.
    let empty = OperatorPolicy::with_defaults(dir.path(), None, Some(BTreeMap::new()));
    assert_eq!(empty.standing(Some(WORKSPACE)), Standing::User);
    assert_eq!(
        upload_hold_with(dir.path(), WORKSPACE, Some(&BTreeMap::new())),
        UploadHold::None
    );
}

#[tokio::test]
async fn a_read_policy_replaces_the_defaults_wholesale() {
    let (dir, policy) = defaulted();
    policy.sign_in(WORKSPACE);
    // The operator's policy sets the sharing mode only: the defaults' upload
    // control, window and delete are the person's once it is read.
    let cloud = FakeCloud::answering(vec![
        me(json!({ "version": 1, "revision": 2, "settings": { "sharing_mode": "new" } })),
        Err(CloudError::Unreachable),
        Ok(json!({ "role": "member" })),
    ]);
    let read = BTreeMap::from([(SHARING_MODE.to_string(), json!("new"))]);
    assert_eq!(
        policy.refresh(&cloud, WORKSPACE).await,
        Standing::Enforced(read.clone())
    );
    assert!(!policy.controls_uploads(Some(WORKSPACE)));
    assert!(policy.check_pause(Some(WORKSPACE)).is_ok());
    assert!(policy.check_delete(Some(WORKSPACE)).is_ok());
    assert_eq!(policy.oops_window_ms(Some(WORKSPACE)), None);
    assert_eq!(
        policy.enforced_settings(Some(WORKSPACE)),
        BTreeMap::from([(SHARING_MODE.to_string(), "new".to_string())])
    );
    assert!(!policy.due(WORKSPACE), "read: the usual interval applies");
    assert_eq!(
        upload_hold_with(dir.path(), WORKSPACE, Some(&defaults())),
        UploadHold::None
    );
    // A failed read keeps what was read, not the defaults.
    assert_eq!(
        policy.refresh(&cloud, WORKSPACE).await,
        Standing::Enforced(read.clone())
    );
    let reopened = OperatorPolicy::with_defaults(dir.path(), None, Some(defaults()));
    assert_eq!(reopened.standing(Some(WORKSPACE)), Standing::Enforced(read));
    // No policy at all: every setting is the person's.
    assert_eq!(policy.refresh(&cloud, WORKSPACE).await, Standing::User);
    assert!(policy.enforced_settings(Some(WORKSPACE)).is_empty());
    assert_eq!(
        upload_hold_with(dir.path(), WORKSPACE, Some(&defaults())),
        UploadHold::None
    );
}

#[test]
fn a_build_without_defaults_still_withholds() {
    // The suite's own build never takes compiled defaults.
    assert_eq!(compiled_defaults(), None);
    let (dir, policy) = policy();
    policy.sign_in(WORKSPACE);
    assert_eq!(policy.standing(Some(WORKSPACE)), Standing::Withheld);
    assert_eq!(upload_hold(dir.path(), WORKSPACE), UploadHold::Blocked);
    assert_eq!(
        upload_hold_with(dir.path(), WORKSPACE, None),
        UploadHold::Blocked
    );
    let explicit = OperatorPolicy::with_defaults(dir.path(), None, None);
    assert_eq!(explicit.standing(Some(WORKSPACE)), Standing::Withheld);
    // What build.rs hands on when nothing is set: empty, no defaults.
    assert_eq!(built_defaults(Some("")), None);
    assert_eq!(built_defaults(None), None);
    assert_eq!(
        built_defaults(Some(r#"{"upload_control":"user"}"#)),
        Some(BTreeMap::from([(
            UPLOAD_CONTROL.to_string(),
            json!("user")
        )]))
    );
}

#[test]
fn the_defaults_pass_the_checks_a_read_policy_passes() {
    use settings::{canonical, parse_defaults, DefaultsError};
    assert_eq!(parse_defaults(""), Ok(None));
    assert_eq!(parse_defaults("  \n"), Ok(None));
    let parsed = parse_defaults(
        r#" { "user_retroactive_delete": true, "sharing_mode": "selected",
              "oops_window_seconds": 86400, "upload_control": "user" } "#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        canonical(&parsed),
        r#"{"oops_window_seconds":86400,"sharing_mode":"selected","upload_control":"user","user_retroactive_delete":true}"#
    );
    assert_eq!(parse_defaults("{}"), Ok(Some(BTreeMap::new())));
    for (text, error) in [
        ("{ not json", DefaultsError::NotJson),
        (r#"{"sharing_mode":"all""#, DefaultsError::NotJson),
        ("[]", DefaultsError::NotObject),
        (r#""all""#, DefaultsError::NotObject),
        ("null", DefaultsError::NotObject),
        ("7", DefaultsError::NotObject),
        (
            r#"{"sharing_mode":"all","colour":"red"}"#,
            DefaultsError::UnknownKey,
        ),
        (r#"{"version":1}"#, DefaultsError::UnknownKey),
        (
            r#"{"sharing_mode":"bogus"}"#,
            DefaultsError::Invalid(SHARING_MODE),
        ),
        (
            r#"{"sharing_mode":1}"#,
            DefaultsError::Invalid(SHARING_MODE),
        ),
        (
            r#"{"upload_control":"admin"}"#,
            DefaultsError::Invalid(UPLOAD_CONTROL),
        ),
        (
            r#"{"upload_control":true}"#,
            DefaultsError::Invalid(UPLOAD_CONTROL),
        ),
        (
            r#"{"oops_window_seconds":86401}"#,
            DefaultsError::Invalid(OOPS_WINDOW_SECONDS),
        ),
        (
            r#"{"oops_window_seconds":-1}"#,
            DefaultsError::Invalid(OOPS_WINDOW_SECONDS),
        ),
        (
            r#"{"oops_window_seconds":60.5}"#,
            DefaultsError::Invalid(OOPS_WINDOW_SECONDS),
        ),
        (
            r#"{"oops_window_seconds":"60"}"#,
            DefaultsError::Invalid(OOPS_WINDOW_SECONDS),
        ),
        (
            r#"{"user_retroactive_delete":"false"}"#,
            DefaultsError::Invalid(USER_RETROACTIVE_DELETE),
        ),
        (
            r#"{"user_retroactive_delete":0}"#,
            DefaultsError::Invalid(USER_RETROACTIVE_DELETE),
        ),
    ] {
        assert_eq!(parse_defaults(text), Err(error.clone()), "{text}");
        // Names only: the message never carries the value.
        let message = error.to_string();
        assert!(message.starts_with(settings::DEFAULTS_KEY), "{message}");
        for value in [
            "bogus", "admin", "86401", "red", "colour", "version", "60.5",
        ] {
            assert!(!message.contains(value), "{message}");
        }
    }
}

#[test]
fn the_shared_checks_know_every_sharing_mode() {
    use clap::ValueEnum;
    let modes: Vec<&str> = SharingMode::value_variants()
        .iter()
        .map(|mode| mode.as_str())
        .collect();
    assert_eq!(modes, settings::SHARING_MODES);
    for mode in SharingMode::value_variants() {
        assert_eq!(
            serde_json::to_value(mode).unwrap(),
            json!(mode.as_str()),
            "the contract spells each mode as the checks do"
        );
    }
}

/// `build.rs` refuses `DESKTOP_POLICY_DEFAULTS` unless
/// `settings::is_site_url` calls the build a site build: it must agree with
/// the core, where `links::built_in_site_url` is the origin the build kept
/// (production when empty) and a site build is one whose origin is not
/// `links::DEFAULT_SITE_URL`.
#[test]
fn the_build_calls_a_site_build_what_links_does() {
    use agent_relay_node::relay::links::DEFAULT_SITE_URL;
    assert_eq!(settings::PRODUCTION_SITE_URL, DEFAULT_SITE_URL);
    let links_site_build = |raw: &str| {
        let raw = raw.trim();
        let site = if raw.is_empty() {
            DEFAULT_SITE_URL.to_owned()
        } else {
            url::Url::parse(raw).unwrap().origin().ascii_serialization()
        };
        site != DEFAULT_SITE_URL
    };
    let production = [
        "",
        "  ",
        "https://agentrelay.com",
        "https://agentrelay.com/",
        "HTTPS://AgentRelay.com",
        "https://AGENTRELAY.COM/",
        " https://agentrelay.com:443 ",
        "https://agentrelay.com:443/",
    ];
    for raw in production {
        assert!(!links_site_build(raw), "{raw:?}");
        assert!(!settings::is_site_url(raw), "{raw:?}");
    }
    for raw in [
        "https://relay.corp.test",
        "https://Relay.Corp.Test/",
        "http://localhost:3100",
    ] {
        assert!(links_site_build(raw), "{raw:?}");
        assert!(settings::is_site_url(raw), "{raw:?}");
    }
}

#[tokio::test]
async fn a_pass_names_the_revision_it_decided_eligibility_under() {
    // Its own workspace id: pass revisions are process-wide.
    const PASS_WORKSPACE: &str = "pass-revision-workspace";
    let dir = tempfile::tempdir().unwrap();
    // Nothing read yet: no revision, so no header.
    assert_eq!(
        pass_policy_with(dir.path(), PASS_WORKSPACE, None),
        UploadHold::None
    );
    assert_eq!(pass_revision(PASS_WORKSPACE), None);
    let policy = OperatorPolicy::new(dir.path(), None);
    policy.sign_in(PASS_WORKSPACE);
    assert_eq!(
        pass_policy_with(dir.path(), PASS_WORKSPACE, None),
        UploadHold::Blocked
    );
    assert_eq!(pass_revision(PASS_WORKSPACE), None);
    let window = |revision: i64, seconds: u64| {
        me(json!({"version": 1, "revision": revision, "settings": {
            "upload_control": "operator", "oops_window_seconds": seconds}}))
    };
    let cloud = FakeCloud::answering(vec![me(all(3)), window(4, 3600)]);
    policy.refresh(&cloud, PASS_WORKSPACE).await;
    assert_eq!(
        pass_policy_with(dir.path(), PASS_WORKSPACE, None),
        UploadHold::None
    );
    assert_eq!(pass_revision(PASS_WORKSPACE), Some(3));
    // A read during the pass caches revision 4 with an oops window. The pass
    // decided under revision 3, so its uploads keep naming 3 ...
    policy.refresh(&cloud, PASS_WORKSPACE).await;
    assert_eq!(pass_revision(PASS_WORKSPACE), Some(3));
    // ... until the next pass decides under revision 4's settings.
    assert_eq!(
        pass_policy_with(dir.path(), PASS_WORKSPACE, None),
        UploadHold::Window(3_600_000)
    );
    assert_eq!(pass_revision(PASS_WORKSPACE), Some(4));
    // An unreadable cache withholds and names no revision.
    std::fs::write(dir.path().join(FILE), b"not json").unwrap();
    assert_eq!(
        pass_policy_with(dir.path(), PASS_WORKSPACE, None),
        UploadHold::Blocked
    );
    assert_eq!(pass_revision(PASS_WORKSPACE), None);
}

#[tokio::test]
async fn a_revision_zero_policy_is_named_too() {
    const ZERO_WORKSPACE: &str = "pass-revision-zero-workspace";
    let dir = tempfile::tempdir().unwrap();
    let policy = OperatorPolicy::new(dir.path(), None);
    policy.sign_in(ZERO_WORKSPACE);
    let cloud = FakeCloud::answering(vec![me(all(0))]);
    policy.refresh(&cloud, ZERO_WORKSPACE).await;
    pass_policy_with(dir.path(), ZERO_WORKSPACE, None);
    assert_eq!(pass_revision(ZERO_WORKSPACE), Some(0));
}
