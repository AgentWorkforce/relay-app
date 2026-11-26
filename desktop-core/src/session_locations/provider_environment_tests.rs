use super::*;
use std::collections::BTreeMap;

fn captured(environment: &[(&str, &str)]) -> SessionLocation {
    let table = processes();
    {
        let mut processes = table.processes.lock().unwrap();
        let owner = processes.get_mut(&30).unwrap();
        owner.cwd = Some("/projects/original".into());
        owner.environment = environment
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
    }
    from_process(&table, 501, 30).unwrap()
}

#[test]
fn provider_store_path_whitespace_is_identity_not_formatting() {
    let origin = captured(&[("CODEX_HOME", "/private/codex store ")]);
    assert_eq!(
        origin.provider_environment.unwrap()["CODEX_HOME"],
        "/private/codex store "
    );
    let empty = captured(&[("CODEX_HOME", "")]);
    assert_eq!(empty.provider_environment_known, Some(false));
}

#[test]
fn same_terminal_different_provider_stores_remain_distinct_locations() {
    let table = processes();
    {
        let mut processes = table.processes.lock().unwrap();
        let mut second = processes[&30].clone();
        second.start = 41;
        second
            .environment
            .insert("CODEX_HOME".into(), "/other/store".into());
        processes.insert(40, second);
    }
    let owners = [table.process(30).unwrap(), table.process(40).unwrap()];
    assert_eq!(locations(&table, 501, owners.into_iter()).len(), 2);
}

#[test]
fn provider_store_paths_survive_persistence_and_both_resume_hosts() {
    let origin = captured(&[
        ("CLAUDE_CONFIG_DIR", "/private/claude work"),
        ("CODEX_HOME", "/private/codex work"),
        ("CODEX_SQLITE_HOME", "/private/codex sqlite"),
        ("GROK_HOME", "/private/grok work"),
        ("SECRET_TOKEN", "must never persist"),
    ]);
    assert_eq!(origin.provider_environment_known, Some(true));
    let environment = origin.provider_environment.clone().unwrap();
    assert_eq!(environment.len(), 4);
    assert!(!serde_json::to_string(&origin)
        .unwrap()
        .contains("must never persist"));
    let dir = tempfile::tempdir().unwrap();
    save(
        &database(dir.path()).unwrap(),
        "codex",
        "id",
        &[origin.clone()],
    )
    .unwrap();
    let persisted = origins(&database(dir.path()).unwrap(), "codex", "id")
        .unwrap()
        .1
        .unwrap();
    assert_eq!(persisted.provider_environment, Some(environment.clone()));
}

#[test]
fn effective_defaults_follow_original_home_and_override_precedence() {
    let origin = captured(&[("HOME", "/original home"), ("CODEX_HOME", "~/codex-work")]);
    let environment = origin.provider_environment.unwrap();
    assert_eq!(environment["CLAUDE_CONFIG_DIR"], "/original home/.claude");
    assert_eq!(environment["CODEX_HOME"], "/original home/codex-work");
    assert_eq!(
        environment["CODEX_SQLITE_HOME"],
        "/original home/codex-work"
    );
    assert_eq!(environment["GROK_HOME"], "/original home/.grok");
    assert!(!environment.contains_key("HOME"));
    let origin = captured(&[("CODEX_HOME", "relative-store")]);
    assert_eq!(
        origin.provider_environment.unwrap()["CODEX_HOME"],
        "/projects/original/relative-store"
    );
}

#[test]
fn known_empty_environment_is_distinct_from_legacy_and_invalid_capture() {
    let empty = captured(&[]);
    assert_eq!(empty.provider_environment, Some(BTreeMap::new()));
    assert_eq!(empty.provider_environment_known, Some(true));
    assert_eq!(pane("macos-tty").provider_environment, None);
    let invalid = captured(&[("CODEX_HOME", "~/unknown-home")]);
    assert_eq!(invalid.provider_environment_known, Some(false));
    assert_eq!(invalid.provider_environment, None);
}
