//! The `workspace-policy.json` cache: reading it back and writing it.

use super::*;

#[test]
fn an_unreadable_cache_starts_unset_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let (saved, problems) = load(&dir.path().join(FILE));
    assert!(saved.workspaces.is_empty());
    assert!(
        problems.is_empty(),
        "no file is not a problem: {problems:?}"
    );
    std::fs::write(dir.path().join(FILE), "not json").unwrap();
    let (saved, problems) = load(&dir.path().join(FILE));
    assert!(saved.workspaces.is_empty());
    assert_eq!(problems.len(), 1, "{problems:?}");
    let store = WorkspacePolicies::new(dir.path());
    assert_eq!(store.policy(WORKSPACE), WorkspacePolicy::unset());
    store.record(WORKSPACE, Fetched::Policy(Box::new(parsed(example(2)))));
    assert_eq!(
        WorkspacePolicies::new(dir.path()).policy(WORKSPACE),
        parsed(example(2))
    );
}

#[test]
fn a_malformed_entry_is_logged_and_keeps_the_other_policies() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspacePolicies::new(dir.path());
    store.record(WORKSPACE, Fetched::Policy(Box::new(parsed(example(5)))));
    let file = dir.path().join(FILE);
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    saved["workspaces"]["another"] = json!({
        "workspace_id": "another",
        "revision": 3,
        "policy": { "version": 3, "revision": 3 },
    });
    std::fs::write(&file, saved.to_string()).unwrap();
    let (_, problems) = load(&file);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("another"), "{problems:?}");
    let reopened = WorkspacePolicies::new(dir.path());
    assert_eq!(reopened.policy(WORKSPACE), parsed(example(5)));
    assert_eq!(reopened.entry("another"), None);
}

#[test]
fn a_failed_write_is_retried_by_the_next_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(FILE);
    // A directory where the file goes makes every write fail.
    std::fs::create_dir(&file).unwrap();
    let store = WorkspacePolicies::new(dir.path());
    let read = || Fetched::Policy(Box::new(parsed(example(5))));
    store.record(WORKSPACE, read());
    assert_eq!(store.policy(WORKSPACE), parsed(example(5)));
    assert!(file.is_dir(), "the write failed");
    std::fs::remove_dir(&file).unwrap();
    // The same document again: nothing changed, but the file is behind.
    store.record(WORKSPACE, read());
    assert_eq!(
        WorkspacePolicies::new(dir.path()).policy(WORKSPACE),
        parsed(example(5))
    );
    // Once written, an unchanged read leaves the file alone.
    std::fs::remove_file(&file).unwrap();
    store.record(WORKSPACE, read());
    assert!(!file.exists());
}
