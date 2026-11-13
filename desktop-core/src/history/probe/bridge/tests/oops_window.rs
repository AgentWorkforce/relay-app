//! "Don't upload" under an operator policy with an oops window: refused once
//! the session's window has ended, and only for an exclusion.
use super::super::selection_validation::WINDOW_ENDED;
use super::*;

/// The window these tests' policy sets, in milliseconds.
const WINDOW_MS: i64 = 60_000;

/// Caches an operator policy with a one-minute oops window for the
/// fixture's workspace, in the app data directory of the child process
/// `home` belongs to: the child runs its test alone.
fn cache_window(home: &Path) {
    let app_data = home.join("app-data");
    fs::create_dir_all(&app_data).unwrap();
    std::env::set_var(crate::history::cloud::APP_DATA_DIR_VARIABLE, &app_data);
    let policy = json!({ "workspaces": { "workspace": {
        "workspace_id": "workspace", "revision": 1, "fetched_at": 1,
        "settings": { "upload_control": "operator", "oops_window_seconds": WINDOW_MS / 1000 },
    } } });
    fs::write(
        app_data.join(crate::history::operator_policy::FILE),
        serde_json::to_vec(&policy).unwrap(),
    )
    .unwrap();
}

#[test]
fn dont_upload_is_refused_once_the_window_has_ended() {
    let Some(home) = crate::history::probe::live::tests::isolated_home(
        "history::probe::bridge::tests::oops_window::dont_upload_is_refused_once_the_window_has_ended",
    ) else {
        return;
    };
    cache_window(&home);
    let (dir, _config) = fixture(SharingMode::All);
    let conn = db(dir.path()).unwrap();
    // `codex:recent` was active just now and waits; `claude:old` was last
    // active long before the window.
    delivery::test_evidence(&conn)
        .execute(
            "UPDATE sessions SET last_activity_ms=? WHERE source='codex' AND session_id='recent'",
            [collector::now()],
        )
        .unwrap();

    let error = change(dir.path(), None, &["claude:old".into()], false).unwrap_err();
    assert_eq!(error.to_string(), WINDOW_ENDED);
    let old = parse_key("claude:old").unwrap();
    assert!(!delivery::is_explicitly_excluded(&conn, &old).unwrap());
    // Asked together with a waiting one, the refusal still stands.
    let both = ["codex:recent".to_string(), "claude:old".to_string()];
    assert_eq!(
        change(dir.path(), None, &both, false)
            .unwrap_err()
            .to_string(),
        WINDOW_ENDED
    );

    // Inside its window, a session may still be kept back.
    change(dir.path(), None, &["codex:recent".into()], false).unwrap();
    let recent = parse_key("codex:recent").unwrap();
    assert!(delivery::is_explicitly_excluded(&conn, &recent).unwrap());

    // Including and deleting are never refused for the window.
    change(dir.path(), None, &["claude:old".into()], true).unwrap();
    let deletion =
        change_with_cloud(dir.path(), None, &["claude:old".into()], false, true).unwrap_err();
    assert_ne!(deletion.to_string(), WINDOW_ENDED);
}

#[test]
fn session_rows_list_waiting_sessions_until_their_window_ends() {
    let Some(home) = crate::history::probe::live::tests::isolated_home(
        "history::probe::bridge::tests::oops_window::session_rows_list_waiting_sessions_until_their_window_ends",
    ) else {
        return;
    };
    cache_window(&home);
    let (dir, config) = fixture(SharingMode::All);
    let conn = db(dir.path()).unwrap();
    let active = collector::now();
    // `codex:recent` and `claude:kept` were just active; `claude:old` was
    // last active long before the window and still has a copy held.
    delivery::test_evidence(&conn)
        .execute(
            "UPDATE sessions SET last_activity_ms=? WHERE source='codex' AND session_id='recent'",
            [active],
        )
        .unwrap();
    delivery::test_evidence(&conn).execute(
        "INSERT INTO sessions(source,session_id,first_prompt,last_activity_ms) VALUES ('claude','kept','Private',?)",
        [active],
    )
    .unwrap();
    delivery::set_session_excluded(&conn, &parse_key("claude:kept").unwrap(), true).unwrap();
    conn.execute(
        "INSERT INTO delivery_held_records(job_id,revision_id,revision,source,session_id,record_json,key_json,held_ms) VALUES (?,'held-1',1,'claude','old','{}','[]',10)",
        [&config.job_id],
    )
    .unwrap();

    let rows = session_rows(dir.path(), &config, 10).unwrap();
    let row = |session: &str| {
        rows.iter()
            .find(|row| row["session_id"] == session)
            .unwrap_or_else(|| panic!("{session} is not listed: {rows:?}"))
            .clone()
    };
    let recent = row("recent");
    assert_eq!(recent["status"], "waiting", "{recent}");
    assert_eq!(recent["upload_hold_until_ms"], active + WINDOW_MS);
    // Its window has ended with a copy still held: queued, with no end.
    let old = row("old");
    assert_eq!(old["status"], "queued", "{old}");
    assert!(old.get("upload_hold_until_ms").is_none(), "{old}");
    // Kept back: not shared, and nothing about a hold.
    let kept = row("kept");
    assert_eq!(kept["status"], "not_shared", "{kept}");
    assert!(kept.get("upload_hold_until_ms").is_none(), "{kept}");
}
