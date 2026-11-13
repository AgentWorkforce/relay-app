use super::*;
use crate::history::operator_policy::{OperatorPolicy, ENFORCED, FILE, WITHHELD};
use std::collections::BTreeMap;

/// Answers every command and records each one run.
#[derive(Default)]
struct RecordingProbe {
    commands: Mutex<Vec<Vec<String>>>,
}

impl RecordingProbe {
    /// Each command's name: its words before the first flag.
    fn names(&self) -> Vec<String> {
        let commands = self.commands.lock().unwrap();
        commands
            .iter()
            .map(|args| {
                let words = args.iter().take_while(|arg| !arg.starts_with("--"));
                words.cloned().collect::<Vec<_>>().join(" ")
            })
            .collect()
    }
}

impl ProbeRunner for RecordingProbe {
    fn status(
        &self,
        _: Option<std::path::PathBuf>,
        _: crate::history::uploader::HistoryReader,
    ) -> BoxFuture<'_, Result<Value, UploaderError>> {
        self.run(vec!["status".into(), "--json".into()], None)
    }

    fn run(
        &self,
        args: Vec<String>,
        _timeout: Option<Duration>,
    ) -> BoxFuture<'_, Result<Value, UploaderError>> {
        self.commands.lock().unwrap().push(args);
        Box::pin(async { Ok(json!({})) })
    }

    fn install(
        &self,
        _args: Vec<String>,
        _bearer: Option<Secret>,
    ) -> Result<InstallRun, UploaderError> {
        Err(UploaderError::new("unused"))
    }

    fn cancel_all(&self) -> BoxFuture<'_, Result<(), UploaderError>> {
        Box::pin(async { Ok(()) })
    }
}

/// An uploader attached to workspace `w1`, whose cached policy entry is
/// `entry`, with the directory the probe's exclusions need.
fn uploader(
    entry: Value,
) -> (
    tempfile::TempDir,
    Arc<RecordingProbe>,
    Uploader,
    Arc<OperatorPolicy>,
) {
    uploader_with(entry, None)
}

/// [`uploader`] on a site build compiled with policy `defaults`.
fn uploader_with(
    entry: Value,
    defaults: Option<BTreeMap<String, Value>>,
) -> (
    tempfile::TempDir,
    Arc<RecordingProbe>,
    Uploader,
    Arc<OperatorPolicy>,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(FILE),
        serde_json::to_vec(&json!({ "workspaces": { "w1": entry } })).unwrap(),
    )
    .unwrap();
    for name in ["history.db", "config.json"] {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }
    let probe = Arc::new(RecordingProbe::default());
    let uploader = Uploader::new(probe.clone());
    let policy = Arc::new(OperatorPolicy::with_defaults(dir.path(), None, defaults));
    uploader.set_operator_policy(policy.clone());
    uploader.attach(Install::from_json(&json!({
        "site_url": "https://example.test", "account_id": "u1", "workspace_id": "w1",
        "directory": dir.path().to_str().unwrap(), "sharing_mode": "all",
    })));
    (dir, probe, uploader, policy)
}

#[tokio::test]
async fn an_enforced_mode_refuses_the_persons_changes_and_runs_nothing() {
    let (_dir, probe, uploader, _policy) = uploader(json!({
        "workspace_id": "w1", "revision": 1, "fetched_at": 1,
        "settings": { "sharing_mode": "all" },
    }));
    let target = uploader.target().unwrap();
    let keys = vec!["codex:s1".to_string()];

    let error = uploader.sharing_set(SharingMode::New).await.unwrap_err();
    assert_eq!(error.message, ENFORCED);
    let error = uploader
        .sharing_set(SharingMode::Selected)
        .await
        .unwrap_err();
    assert_eq!(error.message, ENFORCED);
    let error = uploader.sessions_change(false, &keys).await.unwrap_err();
    assert_eq!(error.message, ENFORCED);
    let error = uploader
        .sessions_change_for(&target, false, &keys)
        .await
        .unwrap_err();
    assert_eq!(error.message, ENFORCED);
    assert!(probe.names().is_empty(), "{:?}", probe.names());

    // What the operator enforces anyway, including every session, and a
    // resume are still the person's.
    uploader.sharing_set(SharingMode::All).await.unwrap();
    uploader.sessions_change(true, &keys).await.unwrap();
    uploader.pause(false).await.unwrap();
    assert_eq!(probe.names(), ["sharing set", "sessions include", "resume"]);
    assert_eq!(
        uploader.enforced_settings(),
        [("sharing_mode".to_string(), "all".to_string())].into()
    );
    assert!(!uploader.uploads_withheld());
}

#[tokio::test]
async fn withheld_uploads_refuse_a_resume_and_the_persons_pause_keeps_them_paused() {
    let (_dir, probe, uploader, policy) = uploader(json!({
        "workspace_id": "w1", "revision": null, "fetched_at": null,
        "settings": {}, "withheld": true,
    }));
    assert!(uploader.uploads_withheld());
    assert!(uploader.enforced_settings().is_empty());

    let error = uploader.pause(false).await.unwrap_err();
    assert_eq!(error.message, WITHHELD);
    assert!(probe.names().is_empty(), "{:?}", probe.names());

    // The person's own pause: the first read of the policy leaves it.
    uploader.pause(true).await.unwrap();
    assert_eq!(probe.names(), ["pause"]);
    assert!(!policy.entry("w1").unwrap().withheld);
    assert!(uploader.uploads_withheld());
}

#[tokio::test]
async fn operator_run_uploads_refuse_the_persons_pause_and_resume_but_not_the_policys() {
    let (_dir, probe, uploader, _policy) = uploader(json!({
        "workspace_id": "w1", "revision": 1, "fetched_at": 1,
        "settings": { "upload_control": "operator", "oops_window_seconds": 0 },
    }));
    for paused in [true, false] {
        let error = uploader.pause(paused).await.unwrap_err();
        assert_eq!(error.message, ENFORCED);
    }
    assert!(probe.names().is_empty(), "{:?}", probe.names());
    // The policy's own resume still runs.
    uploader.pause_unchecked(false).await.unwrap();
    assert_eq!(probe.names(), ["resume"]);
    // A zero window is not honoured, so it is not reported.
    assert_eq!(
        uploader.enforced_settings(),
        [("upload_control".to_string(), "operator".to_string())].into()
    );
}

#[tokio::test]
async fn a_withdrawn_delete_refuses_the_persons_deletion_and_runs_nothing() {
    let (_dir, probe, uploader, _policy) = uploader(json!({
        "workspace_id": "w1", "revision": 1, "fetched_at": 1,
        "settings": { "user_retroactive_delete": false },
    }));
    let keys = vec!["codex:s1".to_string()];
    let error = uploader.sessions_delete(&keys).await.unwrap_err();
    assert_eq!(error.message, ENFORCED);
    assert!(probe.names().is_empty(), "{:?}", probe.names());
    // Pausing and resuming are still the person's.
    uploader.pause(true).await.unwrap();
    uploader.pause(false).await.unwrap();
    assert_eq!(probe.names(), ["pause", "resume"]);
}

#[tokio::test]
async fn a_withdrawn_delete_refuses_the_reset_too() {
    let (_dir, probe, uploader, _policy) = uploader(json!({
        "workspace_id": "w1", "revision": 1, "fetched_at": 1,
        "settings": { "user_retroactive_delete": false },
    }));
    let delete = uploader
        .sessions_delete(&["codex:s1".to_string()])
        .await
        .unwrap_err();
    let reset = uploader
        .sessions_reset("DELETE ALL SESSIONS")
        .await
        .unwrap_err();
    assert_eq!(reset, delete);
    assert_eq!(reset.message, ENFORCED);
    assert!(probe.names().is_empty(), "{:?}", probe.names());
}

#[tokio::test]
async fn an_allowed_delete_and_person_run_uploads_change_nothing() {
    let (_dir, probe, uploader, _policy) = uploader(json!({
        "workspace_id": "w1", "revision": 1, "fetched_at": 1,
        "settings": { "user_retroactive_delete": true, "upload_control": "user" },
    }));
    uploader
        .sessions_delete(&["codex:s1".to_string()])
        .await
        .unwrap();
    uploader.pause(true).await.unwrap();
    assert_eq!(probe.names(), ["sessions delete", "pause"]);
}

#[tokio::test]
async fn defaults_stand_in_for_the_policy_until_it_is_read() {
    let pending = json!({
        "workspace_id": "w1", "revision": null, "fetched_at": null,
        "settings": {}, "withheld": true,
    });
    let defaults = BTreeMap::from([
        ("sharing_mode".to_string(), json!("all")),
        ("upload_control".to_string(), json!("operator")),
        ("oops_window_seconds".to_string(), json!(300)),
    ]);
    let (_dir, probe, uploader, _policy) = uploader_with(pending, Some(defaults));
    // Uploads go ahead under the defaults, which are reported.
    assert!(!uploader.uploads_withheld());
    assert_eq!(
        uploader.enforced_settings(),
        [
            ("oops_window_seconds".to_string(), "300".to_string()),
            ("sharing_mode".to_string(), "all".to_string()),
            ("upload_control".to_string(), "operator".to_string()),
        ]
        .into()
    );
    // And refuse what the person may not change.
    let error = uploader.sharing_set(SharingMode::New).await.unwrap_err();
    assert_eq!(error.message, ENFORCED);
    for paused in [true, false] {
        let error = uploader.pause(paused).await.unwrap_err();
        assert_eq!(error.message, ENFORCED);
    }
    assert!(probe.names().is_empty(), "{:?}", probe.names());
}
