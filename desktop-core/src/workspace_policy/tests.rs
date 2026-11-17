use super::*;
use crate::history::operator_policy::{self, OperatorPolicy, Standing};
use agent_relay_node::relay::cloud_session::{AgentOwner, CloudAccess, CloudAccount};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::collections::VecDeque;

mod cache;

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

/// The spec's example document at `revision`.
fn example(revision: i64) -> Value {
    json!({
        "version": 2,
        "revision": revision,
        "managed": false,
        "consent": ["spawn_full_access_claude", "spawn_requester_approvals"],
        "sections": {
            "capture":   { "locked": { "sharing_mode": "selected" }, "defaults": {} },
            "spawn":     { "locked": { "spawn_max_agents": 8 }, "defaults": { "accept_spawns": true } },
            "join":      { "locked": { "auto_join_repositories": ["acme/web"] }, "defaults": { "auto_join_general": true } },
            "delivery":  { "locked": { "admin_senders": ["lead", "lead@cloud-example"], "inbound_default": "hold" }, "defaults": {} },
            "analytics": { "locked": { "analytics_enabled": false }, "defaults": {} }
        }
    })
}

fn capture_v1(revision: i64) -> Value {
    json!({ "version": 1, "revision": revision, "settings": { "sharing_mode": "all" } })
}

fn me(workspace_policy: Option<Value>) -> Result<Value, CloudError> {
    let mut body = json!({ "role": "member", "grant": null, "policy": capture_v1(3) });
    if let Some(document) = workspace_policy {
        body[FIELD] = document;
    }
    Ok(body)
}

fn parsed(document: Value) -> WorkspacePolicy {
    parse_document(&document).unwrap()
}

fn policies() -> (tempfile::TempDir, OperatorPolicy) {
    let dir = tempfile::tempdir().unwrap();
    let policy = OperatorPolicy::new(dir.path(), None);
    (dir, policy)
}

#[test]
fn the_specs_example_document_is_read_fully() {
    let Fetched::Policy(policy) = parse_access_me(&me(Some(example(5))).unwrap()) else {
        panic!("not read")
    };
    assert_eq!(policy.version, 2);
    assert_eq!(policy.revision(), 5);
    assert!(!policy.managed());
    assert_eq!(
        policy.consent,
        BTreeSet::from([
            "spawn_full_access_claude".to_string(),
            "spawn_requester_approvals".to_string()
        ])
    );
    assert_eq!(policy.locked(Key::SharingMode), Some(&json!("selected")));
    assert_eq!(policy.locked(Key::SpawnMaxAgents), Some(&json!(8)));
    assert_eq!(policy.default(Key::AcceptSpawns), Some(&json!(true)));
    assert!(!policy.is_locked(Key::AcceptSpawns));
    assert_eq!(
        policy.locked(Key::AutoJoinRepositories),
        Some(&json!(["acme/web"]))
    );
    assert_eq!(policy.default(Key::AutoJoinGeneral), Some(&json!(true)));
    assert_eq!(
        policy.locked(Key::AdminSenders),
        Some(&json!(["lead", "lead@cloud-example"]))
    );
    assert_eq!(policy.locked(Key::InboundDefault), Some(&json!("hold")));
    assert_eq!(policy.locked(Key::AnalyticsEnabled), Some(&json!(false)));
    assert!(policy.is_locked(Key::AnalyticsEnabled));
    // Consent counts only for person-only keys it names.
    assert!(policy.needs_consent(Key::SpawnFullAccessClaude));
    assert!(policy.needs_consent(Key::SpawnRequesterApprovals));
    assert!(!policy.needs_consent(Key::AcceptSpawns));
    assert!(!policy.needs_consent(Key::SpawnMaxAgents));
    // Nothing else is set.
    let set: usize = SectionName::ALL
        .into_iter()
        .map(|name| {
            let section = policy.sections.get(name);
            section.locked.len() + section.defaults.len()
        })
        .sum();
    assert_eq!(set, 8);
    // The wording is the capture policy's.
    assert_eq!(ENFORCED, operator_policy::ENFORCED);
    assert_eq!(MANAGED, operator_policy::MANAGED);
}

#[test]
fn every_allowlisted_key_round_trips_by_name() {
    for key in Key::ALL {
        assert_eq!(Key::from_name(key.name()), Some(key));
        assert_eq!(serde_json::to_value(key).unwrap(), json!(key.name()));
    }
    assert_eq!(Key::from_name("spawn_anything"), None);
}

#[test]
fn the_unset_policy_sets_nothing() {
    let unset = parsed(json!({
        "version": 2, "revision": 1, "managed": false, "consent": [],
        "sections": {
            "capture": { "locked": {}, "defaults": {} },
            "spawn": { "locked": {}, "defaults": {} },
            "join": { "locked": {}, "defaults": {} },
            "delivery": { "locked": {}, "defaults": {} },
            "analytics": { "locked": {}, "defaults": {} }
        }
    }));
    assert_eq!(unset, WorkspacePolicy::unset());
    assert_eq!(
        <WorkspacePolicy as Default>::default(),
        WorkspacePolicy::unset()
    );
    for key in Key::ALL {
        assert_eq!(unset.locked(key), None);
        assert_eq!(unset.default(key), None);
        assert!(!unset.needs_consent(key));
    }
    // Missing fields are empty too.
    assert_eq!(
        parsed(json!({ "version": 2, "revision": 1 })),
        WorkspacePolicy::unset()
    );
}

#[test]
fn each_unreadable_entry_is_dropped_and_the_rest_applies() {
    let policy = parsed(json!({
        "version": 2,
        "revision": 4,
        "managed": true,
        "future_field": { "anything": 1 },
        "consent": ["accept_spawns", "spawn_max_agents", "nonsense", 7, "accept_spawns"],
        "sections": {
            "telemetry": { "locked": { "analytics_enabled": false } },
            "capture": {
                "locked": { "sharing_mode": "every", "upload_control": "operator", "accept_spawns": true },
                "defaults": { "sharing_mode": "all" },
                "suggested": { "sharing_mode": "all" }
            },
            "spawn": {
                "locked": { "spawn_max_agents": 33, "spawn_full_access_codex": "yes", "spawn_billing_claude": "api" },
                "defaults": {
                    "spawn_max_agents": 4,
                    "spawn_repositories": ["acme/web", ""],
                    "spawn_billing_codex": "free",
                    "spawn_allow_teammates": false
                }
            },
            "join": {
                "locked": {
                    "auto_join_repositories": ["Acme/Web", "acme/*", "acme/web", "bad repo/x", "acme", "a/b/c", 3, "*/x"]
                },
                "defaults": { "auto_join_general": "true", "trust_workspace_members": true }
            },
            "delivery": {
                "locked": { "inbound_default": "deliver" },
                "defaults": { "admin_senders": ["lead"], "trusted_relay_senders": ["ops"], "inbound_default": "hold" }
            },
            "analytics": { "locked": "off", "defaults": { "analytics_enabled": true } }
        }
    }));
    assert_eq!(policy.revision(), 4);
    assert!(policy.managed());
    // Only person-only keys stay in consent, once.
    assert_eq!(
        policy.consent,
        BTreeSet::from(["accept_spawns".to_string()])
    );
    // Unknown section and tier, a wrong-section key, a wrong value and a
    // capture key in defaults are dropped.
    assert_eq!(
        policy.sections.capture.locked,
        BTreeMap::from([(Key::UploadControl, json!("operator"))])
    );
    assert!(policy.sections.capture.defaults.is_empty());
    assert_eq!(policy.locked(Key::SpawnMaxAgents), None);
    assert_eq!(policy.default(Key::SpawnMaxAgents), Some(&json!(4)));
    assert_eq!(policy.locked(Key::SpawnFullAccessCodex), None);
    assert_eq!(policy.locked(Key::SpawnBillingClaude), Some(&json!("api")));
    assert_eq!(policy.default(Key::SpawnRepositories), None);
    assert_eq!(policy.default(Key::SpawnBillingCodex), None);
    assert_eq!(
        policy.default(Key::SpawnAllowTeammates),
        Some(&json!(false))
    );
    // Repository entries are lower-cased, collapsed and kept one by one.
    assert_eq!(
        policy.locked(Key::AutoJoinRepositories),
        Some(&json!(["acme/web", "acme/*"]))
    );
    assert_eq!(policy.default(Key::AutoJoinGeneral), None);
    assert_eq!(
        policy.default(Key::TrustWorkspaceMembers),
        Some(&json!(true))
    );
    // admin_senders is read in delivery.locked only; a key locked and
    // defaulted keeps its locked value.
    assert_eq!(policy.default(Key::AdminSenders), None);
    assert_eq!(policy.locked(Key::AdminSenders), None);
    assert_eq!(policy.locked(Key::InboundDefault), Some(&json!("deliver")));
    assert_eq!(policy.default(Key::InboundDefault), None);
    assert_eq!(
        policy.default(Key::TrustedRelaySenders),
        Some(&json!(["ops"]))
    );
    // A tier that is not an object is dropped; the other tier applies.
    assert_eq!(policy.default(Key::AnalyticsEnabled), Some(&json!(true)));
    assert_eq!(policy.locked(Key::AnalyticsEnabled), None);
}

#[test]
fn list_and_range_limits_are_pinned() {
    let names = |count: usize, entry: &str| -> Value { json!(vec![entry; count]) };
    let spawn = |repositories: Value| {
        parsed(json!({ "version": 2, "revision": 2, "sections": {
            "spawn": { "locked": { "spawn_repositories": repositories } }
        }}))
        .locked(Key::SpawnRepositories)
        .cloned()
    };
    assert!(spawn(names(200, "a")).is_some());
    assert!(spawn(names(201, "a")).is_none());
    assert!(spawn(names(1, &"x".repeat(200))).is_some());
    assert!(spawn(names(1, &"x".repeat(201))).is_none());
    assert_eq!(spawn(json!([])), Some(json!([])));
    for (seconds, kept) in [(0, true), (86_400, true), (86_401, false), (-1, false)] {
        let policy = parsed(json!({ "version": 2, "revision": 2, "sections": {
            "capture": { "locked": { "oops_window_seconds": seconds } }
        }}));
        assert_eq!(policy.is_locked(Key::OopsWindowSeconds), kept, "{seconds}");
    }
    for (agents, kept) in [(0, false), (1, true), (32, true), (33, false)] {
        let policy = parsed(json!({ "version": 2, "revision": 2, "sections": {
            "spawn": { "defaults": { "spawn_max_agents": agents } }
        }}));
        assert_eq!(
            policy.default(Key::SpawnMaxAgents).is_some(),
            kept,
            "{agents}"
        );
    }
    assert_eq!(keys::repository("acme/*"), Some("acme/*".into()));
    assert!(keys::repository(&format!("{}/x", "a".repeat(100))).is_some());
    assert_eq!(keys::repository(&format!("{}/x", "a".repeat(101))), None);
    assert_eq!(keys::repository("acme/"), None);
    assert_eq!(keys::repository("acme/we b"), None);
}

#[test]
fn a_malformed_document_is_a_failed_read() {
    for document in [
        json!({ "version": 1, "revision": 5 }),
        json!({ "version": 3, "revision": 5 }),
        json!({ "revision": 5 }),
        json!({ "version": 2 }),
        json!({ "version": 2, "revision": "5" }),
        json!({ "version": 2, "revision": 5, "managed": "no" }),
        json!({ "version": 2, "revision": 5, "consent": "accept_spawns" }),
        json!({ "version": 2, "revision": 5, "sections": [] }),
        json!("v2"),
    ] {
        assert_eq!(
            parse_access_me(&json!({ FIELD: document })),
            Fetched::Failed,
            "{document}"
        );
    }
    assert_eq!(
        parse_access_me(&json!({ "role": "member" })),
        Fetched::Absent
    );
    assert_eq!(parse_access_me(&json!({ FIELD: null })), Fetched::Absent);
}

#[test]
fn a_lower_revision_is_refused() {
    let five = apply(
        None,
        WORKSPACE,
        Fetched::Policy(Box::new(parsed(example(5)))),
    );
    let older = Fetched::Policy(Box::new(WorkspacePolicy {
        revision: 4,
        ..WorkspacePolicy::unset()
    }));
    assert_eq!(apply(five.clone(), WORKSPACE, older), five);
    assert_eq!(apply(five.clone(), WORKSPACE, Fetched::Failed), five);
    let same = WorkspacePolicy {
        revision: 5,
        ..WorkspacePolicy::unset()
    };
    let applied = apply(
        five.clone(),
        WORKSPACE,
        Fetched::Policy(Box::new(same.clone())),
    )
    .unwrap();
    assert_eq!(applied.policy, same);
    // No policy is the unset one, and keeps the revision reached.
    let absent = apply(five, WORKSPACE, Fetched::Absent).unwrap();
    assert_eq!(absent.policy, WorkspacePolicy::unset());
    assert_eq!(absent.revision, 5);
    let older = Fetched::Policy(Box::new(parsed(example(4))));
    assert_eq!(apply(Some(absent.clone()), WORKSPACE, older), Some(absent));
    assert_eq!(apply(None, WORKSPACE, Fetched::Absent), None);
}

#[tokio::test]
async fn one_read_feeds_both_policies_and_the_cache_survives() {
    let (dir, policy) = policies();
    let cloud = FakeCloud::answering(vec![me(Some(example(5)))]);
    let standing = policy.refresh(&cloud, WORKSPACE).await;
    assert!(matches!(standing, Standing::Enforced(_)), "{standing:?}");
    assert_eq!(cloud.paths.lock().unwrap().len(), 1, "one request");
    assert_eq!(policy.workspace_policy(WORKSPACE), parsed(example(5)));
    assert_eq!(policy.workspace_policy("another"), WorkspacePolicy::unset());
    // The v1 cache keeps its own revision; the v2 one is in its own file.
    let v1: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(operator_policy::FILE)).unwrap())
            .unwrap();
    assert_eq!(v1["workspaces"][WORKSPACE]["revision"], 3);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(FILE)).unwrap()).unwrap();
    let entry = &saved["workspaces"][WORKSPACE];
    assert_eq!(entry["workspace_id"], WORKSPACE);
    assert_eq!(entry["revision"], 5);
    assert_eq!(
        entry["policy"]["sections"]["spawn"]["locked"]["spawn_max_agents"],
        8
    );
    let reopened = OperatorPolicy::new(dir.path(), None);
    assert_eq!(reopened.workspace_policy(WORKSPACE), parsed(example(5)));
    let store = WorkspacePolicies::new(dir.path());
    assert_eq!(store.policy(WORKSPACE), parsed(example(5)));
}

#[tokio::test]
async fn a_read_without_the_document_clears_it_while_the_capture_policy_applies() {
    let (_dir, policy) = policies();
    let cloud = FakeCloud::answering(vec![me(Some(example(5))), me(None)]);
    policy.refresh(&cloud, WORKSPACE).await;
    let standing = policy.refresh(&cloud, WORKSPACE).await;
    assert!(matches!(standing, Standing::Enforced(_)), "{standing:?}");
    assert_eq!(
        policy.enforced_sharing(Some(WORKSPACE)),
        Some(crate::contract::account::SharingMode::All)
    );
    assert_eq!(policy.workspace_policy(WORKSPACE), WorkspacePolicy::unset());
}

#[tokio::test]
async fn cloud_not_serving_the_route_is_no_policy() {
    let (_dir, policy) = policies();
    let not_found = || {
        Err(CloudError::Status {
            status: 404,
            message: None,
        })
    };
    let cloud = FakeCloud::answering(vec![me(Some(example(5))), not_found()]);
    policy.refresh(&cloud, WORKSPACE).await;
    policy.refresh(&cloud, WORKSPACE).await;
    assert_eq!(policy.workspace_policy(WORKSPACE), WorkspacePolicy::unset());
}

#[tokio::test]
async fn a_failed_read_keeps_the_cached_document() {
    let (dir, policy) = policies();
    let cloud = FakeCloud::answering(vec![
        me(Some(example(5))),
        Err(CloudError::Unreachable),
        Err(CloudError::Status {
            status: 500,
            message: None,
        }),
        me(Some(json!({ "version": 3, "revision": 9 }))),
        me(Some(json!({ "version": 2, "revision": 4 }))),
    ]);
    for _ in 0..5 {
        policy.refresh(&cloud, WORKSPACE).await;
        assert_eq!(policy.workspace_policy(WORKSPACE), parsed(example(5)));
    }
    let reopened = WorkspacePolicies::new(dir.path());
    assert_eq!(reopened.entry(WORKSPACE).unwrap().revision, 5);
}

#[tokio::test]
async fn the_environment_stands_in_for_cloud_in_tests() {
    let dir = tempfile::tempdir().unwrap();
    let mut document = capture_v1(9);
    document[FIELD] = example(6);
    let policy = OperatorPolicy::new(dir.path(), Some(document.to_string()));
    let cloud = FakeCloud::default();
    policy.refresh(&cloud, WORKSPACE).await;
    assert!(cloud.paths.lock().unwrap().is_empty());
    assert_eq!(policy.workspace_policy(WORKSPACE), parsed(example(6)));
    assert!(policy
        .entry(WORKSPACE)
        .is_some_and(|entry| entry.revision == Some(9)));
    // A file without the object is no workspace policy.
    let file = dir.path().join("operator.json");
    std::fs::write(&file, capture_v1(10).to_string()).unwrap();
    let policy = OperatorPolicy::new(dir.path(), Some(file.display().to_string()));
    policy.refresh(&cloud, WORKSPACE).await;
    assert_eq!(policy.workspace_policy(WORKSPACE), WorkspacePolicy::unset());
    assert_eq!(from_environment("null"), Fetched::Absent);
    assert_eq!(
        from_environment(&dir.path().join("missing.json").display().to_string()),
        Fetched::Failed
    );
}


/// A failure to hand the settings policy over is reported once until it
/// changes or the hand-over succeeds.
#[test]
fn a_repeated_settings_failure_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let cache = WorkspacePolicies::new(dir.path());
    assert!(cache.settings_failure_is_new("store failed"));
    assert!(!cache.settings_failure_is_new("store failed"));
    assert!(cache.settings_failure_is_new("another failure"));
    cache.settings_applied(None);
    assert!(cache.settings_failure_is_new("another failure"));
}


fn seeded(revision: i64) -> WorkspacePolicy {
    parse_document(&json!({
        "version": 2, "revision": revision, "managed": true, "consent": [],
        "sections": {"spawn": {"locked": {"spawn_max_agents": 3}, "defaults": {}}}
    }))
    .unwrap()
}

/// An enrollment key's policy is cached only for the attempt that held it
/// and a named workspace, provisionally: it never replaces a workspace
/// already read, and the first read of its workspace replaces it whatever
/// the revision, or clears it when that workspace has none.
#[test]
fn a_seed_is_provisional_and_bound_to_its_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let cache = WorkspacePolicies::new(dir.path());
    // Another attempt's seed, or no workspace, caches nothing.
    cache.hold_seed(1, Some(seeded(9)));
    cache.take_seed(2, "ws_a");
    assert!(cache.entry("ws_a").is_none());
    cache.hold_seed(1, Some(seeded(9)));
    cache.take_seed(1, "");
    assert!(cache.entry("").is_none());
    // Cached provisionally for the workspace the attempt connects to.
    cache.hold_seed(1, Some(seeded(9)));
    cache.take_seed(1, "ws_a");
    let entry = cache.entry("ws_a").unwrap();
    assert!(entry.provisional);
    assert_eq!(entry.revision, 9);
    // The workspace's own policy replaces it, though its revision is lower.
    let mut own = seeded(3);
    own.managed = false;
    cache.record("ws_a", Fetched::Policy(Box::new(own.clone())));
    let entry = cache.entry("ws_a").unwrap();
    assert!(!entry.provisional);
    assert_eq!((entry.revision, entry.policy.managed), (3, false));
    // A seed never replaces a workspace already read.
    cache.hold_seed(1, Some(seeded(9)));
    cache.take_seed(1, "ws_a");
    assert_eq!(cache.entry("ws_a").unwrap().revision, 3);
    // A workspace with no policy clears a seed, revision and all.
    cache.hold_seed(5, Some(seeded(9)));
    cache.take_seed(5, "ws_b");
    cache.record("ws_b", Fetched::Absent);
    let entry = cache.entry("ws_b").unwrap();
    assert!(!entry.provisional);
    assert_eq!(entry.policy, WorkspacePolicy::unset());
    assert_eq!(entry.revision, UNSET_REVISION);
    // A failed read keeps the seed.
    cache.hold_seed(6, Some(seeded(9)));
    cache.take_seed(6, "ws_c");
    cache.record("ws_c", Fetched::Failed);
    assert!(cache.entry("ws_c").unwrap().provisional);
}
