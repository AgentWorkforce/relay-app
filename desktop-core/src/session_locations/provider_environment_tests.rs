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
    let mut report = stopped_report();
    report.latest = Some(persisted);
    for source in ["codex", "claude", "grok"] {
        for default_terminal in [false, true] {
            let mut request = open_request(source);
            request.use_default_terminal = default_terminal;
            let plan = planner::resolve(&request, &report, planner::Platform::MacOS);
            assert_eq!(plan.action, SessionOpenAction::Resume);
            assert_eq!(plan.command.unwrap().environment, Some(environment.clone()));
            assert_eq!(plan.location.is_none(), default_terminal);
        }
    }
}

#[test]
fn unreadable_original_environment_refuses_resume_even_in_a_different_terminal() {
    let table = processes();
    table.unreadable_environment.lock().unwrap().insert(30);
    let origin = from_process(&table, 501, 30).unwrap();
    assert_eq!(origin.provider_environment_known, Some(false));
    assert_eq!(origin.provider_environment, None);
    let mut report = stopped_report();
    report.latest = Some(origin);
    for default_terminal in [false, true] {
        let mut request = open_request("claude");
        request.use_default_terminal = default_terminal;
        let plan = planner::resolve(&request, &report, planner::Platform::MacOS);
        assert_eq!(plan.action, SessionOpenAction::Unavailable);
        assert!(!plan.can_resume_default);
        assert!(plan.reason.unwrap().contains("storage environment"));
    }
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
    let mut report = stopped_report();
    report.latest = Some(empty);
    let plan = planner::resolve(&open_request("claude"), &report, planner::Platform::MacOS);
    assert_eq!(plan.command.unwrap().environment, Some(BTreeMap::new()));
    report.latest = Some(pane("macos-tty"));
    let plan = planner::resolve(&open_request("claude"), &report, planner::Platform::MacOS);
    assert_eq!(plan.command.unwrap().environment, None);
    let invalid = captured(&[("CODEX_HOME", "~/unknown-home")]);
    assert_eq!(invalid.provider_environment_known, Some(false));
    report.latest = Some(invalid);
    assert_eq!(
        planner::resolve(&open_request("claude"), &report, planner::Platform::MacOS).action,
        SessionOpenAction::Unavailable
    );
}

#[test]
fn stopped_desktop_link_has_an_explicit_alternative_but_live_link_never_does() {
    let mut origin = captured(&[("CLAUDE_CONFIG_DIR", "/private/claude-work")]);
    origin.application = "Claude".into();
    origin.tty = None;
    origin.resume_capability = Some("desktop".into());
    let mut report = stopped_report();
    report.latest = Some(origin.clone());
    report.desktop_link = Some("claude://code/continue?session=local_exact".into());
    let mut request = open_request("claude");
    let plan = planner::resolve(&request, &report, planner::Platform::MacOS);
    assert_eq!(plan.action, SessionOpenAction::OpenLink);
    assert!(plan.can_resume_default);
    assert!(plan.location.is_none());
    request.use_default_terminal = true;
    let plan = planner::resolve(&request, &report, planner::Platform::MacOS);
    assert_eq!(plan.action, SessionOpenAction::Resume);
    assert_eq!(
        plan.command.unwrap().environment,
        origin.provider_environment
    );
    report.running = true;
    report.current = vec![origin];
    let plan = planner::resolve(&request, &report, planner::Platform::MacOS);
    assert_eq!(plan.action, SessionOpenAction::OpenLink);
    assert!(!plan.can_resume_default);
    assert!(plan.command.is_none());
}

#[test]
fn mapped_link_with_unknown_capture_cannot_offer_a_store_changing_fallback() {
    let mut report = stopped_report();
    report.latest = Some(SessionLocation {
        application: "Claude".into(),
        provider_environment_known: Some(false),
        cwd: Some("/project".into()),
        ..Default::default()
    });
    report.desktop_link = Some("claude://code/continue?session=local_exact".into());
    let plan = planner::resolve(&open_request("claude"), &report, planner::Platform::MacOS);
    assert_eq!(plan.action, SessionOpenAction::OpenLink);
    assert!(!plan.can_resume_default);
}
