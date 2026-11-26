use super::providers::claude_desktop_id;
use super::*;
use agent_relay_node::relay::contract::Method;
use agent_relay_node::relay::daemon::{Core, MethodExtension};
use agent_relay_node::relay::process::{FakeProcess, FakeProcesses};
use agent_relay_node::relay::server::call;
use serde_json::json;
use std::sync::Arc;

#[path = "provider_environment_tests.rs"]
mod provider_environment_tests;
fn processes() -> FakeProcesses {
    let table = FakeProcesses::default();
    table.insert(
        10,
        FakeProcess {
            uid: 501,
            start: 11,
            executable: Some("/Applications/Ghostty.app/Contents/MacOS/ghostty".into()),
            ..Default::default()
        },
    );
    table.insert(
        20,
        FakeProcess {
            uid: 501,
            start: 21,
            parent: 10,
            ..Default::default()
        },
    );
    table.insert(
        30,
        FakeProcess {
            uid: 501,
            start: 31,
            parent: 20,
            terminal_path: Some("/dev/ttys004".into()),
            ..Default::default()
        },
    );
    table
}
#[test]
fn identifies_exact_terminal_and_process_incarnation() {
    let table = processes();
    let location = from_process(&table, 501, 30).unwrap();
    assert_eq!(location.application, "Ghostty");
    assert_eq!(location.tty.as_deref(), Some("/dev/ttys004"));
    assert_eq!((location.pid, location.process_start), (30, 31));
    // Another pane in the very same app must stay distinct.
    table.insert(
        40,
        FakeProcess {
            uid: 501,
            start: 41,
            parent: 20,
            terminal_path: Some("/dev/ttys005".into()),
            ..Default::default()
        },
    );
    assert_ne!(from_process(&table, 501, 40).unwrap().tty, location.tty);
}
#[test]
fn retains_host_without_tty_but_rejects_cross_user_zombie_and_ancestry_cycles() {
    let table = processes();
    assert_eq!(
        from_process(&table, 501, 20).unwrap().application,
        "Ghostty"
    );
    assert!(from_process(&table, 502, 30).is_none());
    table.processes.lock().unwrap().get_mut(&30).unwrap().zombie = true;
    assert!(from_process(&table, 501, 30).is_none());
    table.processes.lock().unwrap().get_mut(&30).unwrap().zombie = false;
    table.processes.lock().unwrap().get_mut(&20).unwrap().parent = 30;
    assert!(from_process(&table, 501, 30).is_none());
}
#[test]
fn history_survives_restart_and_keeps_reused_pids_separate() {
    let dir = tempfile::tempdir().unwrap();
    let open = || database(dir.path()).unwrap();
    let mut location = from_process(&processes(), 501, 30).unwrap();
    save(&open(), "codex", "same-id", &[location.clone()]).unwrap();
    location.process_start = 32;
    location.tty = Some("/dev/ttys009".into());
    save(&open(), "codex", "same-id", &[location.clone()]).unwrap();
    save(&open(), "codex", "same-id", &[location]).unwrap();
    assert_eq!(history(&open(), "codex", "same-id").unwrap().len(), 2);
    assert!(history(&open(), "claude", "same-id").unwrap().is_empty());
}
#[test]
fn claude_maps_cli_identity_not_filename_title_or_directory() {
    let root = tempfile::tempdir().unwrap();
    let account = root.path().join("org/account");
    std::fs::create_dir_all(&account).unwrap();
    let write = |file: &str, cli: &str, local: &str, archived: bool| {
        std::fs::write(account.join(file),serde_json::json!({"sessionId":local,"cliSessionId":cli,"title":"same title","cwd":"/same/repo","isArchived":archived}).to_string()).unwrap()
    };
    write("unrelated-name.json", "cli-1", "local_exact", false);
    write("other.json", "cli-2", "local_other", false);
    assert_eq!(
        claude_desktop_id(root.path(), "cli-1").as_deref(),
        Some("local_exact")
    );
    write("duplicate.json", "cli-1", "local_ambiguous", false);
    assert!(claude_desktop_id(root.path(), "cli-1").is_none());
    write("duplicate.json", "cli-1", "local_ambiguous", true);
    assert!(claude_desktop_id(root.path(), "missing").is_none());
    assert_eq!(
        claude_desktop_id(root.path(), "cli-1").as_deref(),
        Some("local_exact")
    );
}

#[test]
fn incarnation_change_between_discovery_and_location_cannot_focus_recycled_pid() {
    let table = processes();
    let observed = table.process(30).unwrap();
    table.processes.lock().unwrap().get_mut(&30).unwrap().start += 1;
    assert!(locations(&table, 501, [observed].into_iter()).is_empty());
}

#[test]
fn discovery_records_terminal_and_report_preserves_it_after_exit_without_focus() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = LocalConfig::from_env(&|_| None, dir.path());
    config.uid = 501;
    let root = dir.path().join("probe");
    let table = processes();
    let id = "550e8400-e29b-41d4-a716-446655440000";
    {
        let mut processes = table.processes.lock().unwrap();
        let codex = processes.get_mut(&30).unwrap();
        codex.executable = Some("/tools/codex".into());
        codex.arguments = vec!["codex".into(), "resume".into(), id.into()];
    }
    table.trust("/tools/codex");
    observe(
        &config,
        &root,
        &table,
        &[DiscoveredSession {
            target: agent_relay_node::relay::store::Target::new(
                agent_relay_node::relay::store::SessionKind::Codex,
                id,
            ),
            title: "Title is not identity".into(),
            directory: Some("/same/repo".into()),
            updated_at_ms: None,
            host: None,
            live: true,
            holder: None,
        }],
    );
    let current = report(&config, &root, &table, "codex", id).unwrap();
    assert!(current.running);
    assert_eq!(current.current.len(), 1);
    assert_eq!(current.current[0].tty.as_deref(), Some("/dev/ttys004"));
    table.remove(30);
    let ended = report(&config.clone(), &root, &table, "codex", id).unwrap();
    assert!(!ended.running);
    assert!(ended.current.is_empty());
    assert_eq!(ended.previous.len(), 1);
    assert_eq!(ended.previous[0].application, "Ghostty");
    // The same PID in another terminal without proof of this thread is not it.
    table.insert(
        30,
        FakeProcess {
            uid: 501,
            start: 32,
            parent: 20,
            executable: Some("/tools/codex".into()),
            arguments: vec!["codex".into()],
            terminal_path: Some("/dev/ttys009".into()),
            ..Default::default()
        },
    );
    let reused = report(&config, &root, &table, "codex", id).unwrap();
    assert!(reused.current.is_empty());
    assert_eq!(reused.previous[0].tty.as_deref(), Some("/dev/ttys004"));
}

#[test]
fn terminal_recognition_is_independent_of_navigation_adapter_support() {
    let table = processes();
    for (exe, name) in [
        ("/Applications/Warp.app/Contents/MacOS/stable", "Warp"),
        ("/Applications/kitty.app/Contents/MacOS/kitty", "Kitty"),
        (
            "/Applications/WezTerm.app/Contents/MacOS/wezterm-gui",
            "WezTerm",
        ),
        (
            "/Applications/Alacritty.app/Contents/MacOS/alacritty",
            "Alacritty",
        ),
        (
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron",
            "Visual Studio Code",
        ),
    ] {
        table
            .processes
            .lock()
            .unwrap()
            .get_mut(&10)
            .unwrap()
            .executable = Some(exe.into());
        let location = from_process(&table, 501, 30).unwrap();
        assert_eq!(location.application, name);
        assert_eq!(location.tty.as_deref(), Some("/dev/ttys004"));
    }
}

/// `SessionLocationReportTests.swift` decodes the same sample, so the
/// hand-written Swift types cannot drift from these.
#[test]
fn a_report_serializes_as_the_sample_the_mac_app_decodes() {
    let report = SessionLocationReport {
        supported: true,
        running: true,
        current: vec![SessionLocation {
            application: "Ghostty".into(),
            bundle_id: "com.mitchellh.ghostty".into(),
            pid: 30,
            process_start: 31,
            tty: Some("/dev/ttys004".into()),
            observed_at_ms: 1000,
            ..Default::default()
        }],
        previous: vec![SessionLocation {
            application: "Claude".into(),
            bundle_id: "com.anthropic.claudefordesktop".into(),
            pid: 40,
            process_start: 41,
            tty: None,
            observed_at_ms: 900,
            ..Default::default()
        }],
        desktop_link: Some("claude://code/continue?session=local_exact".into()),
        reason: None,
        original: None,
        latest: None,
        owner_state: None,
    };
    let sample: serde_json::Value =
        serde_json::from_str(include_str!("../../../shared/session-location-report.json")).unwrap();
    assert_eq!(serde_json::to_value(&report).unwrap(), sample);
}

fn test_core(locations: SessionLocations, home: &std::path::Path) -> Arc<Core> {
    use agent_relay_node::relay::daemon::Config;
    let config = Config {
        paths: agent_relay_node::relay::paths::CorePaths::for_socket(home.join("core.sock"))
            .unwrap(),
        home: home.into(),
        app_data_dir: home.join("app-data"),
        env: Default::default(),
        version: "test".into(),
        host_name: "Test Desk.local".into(),
        rediscover_every: std::time::Duration::from_secs(3600),
        push_timing: Default::default(),
        delivery_timing: Default::default(),
    };
    core_with_table(locations, config, Arc::new(processes()))
}
fn core_with_table(
    locations: SessionLocations,
    config: agent_relay_node::relay::daemon::Config,
    table: Arc<FakeProcesses>,
) -> Arc<Core> {
    use agent_relay_node::relay::daemon::Plugins;
    let plugins = Plugins {
        extensions: vec![Arc::new(locations)],
        processes: table,
        ..Plugins::default()
    };
    Core::with_plugins(config, plugins).0
}

#[tokio::test]
async fn the_core_answers_the_method_through_the_extension() {
    let home = tempfile::tempdir().unwrap();
    let locations = SessionLocations::new(None);
    assert_eq!(locations.specs()[0].name, SessionsLocation::NAME);
    let core = test_core(locations, home.path());
    let params = json!({"source": "codex", "session_id": "550e8400-e29b-41d4-a716-446655440000"});
    let answer = call(&core, SessionsLocation::NAME, params).await.unwrap();
    let report: SessionLocationReport = serde_json::from_value(answer).unwrap();
    assert_eq!(report, unavailable());
}

#[tokio::test]
async fn the_method_refuses_a_malformed_identity() {
    let home = tempfile::tempdir().unwrap();
    let core = test_core(SessionLocations::new(None), home.path());
    let params = json!({"source": "../etc", "session_id": "id"});
    assert!(call(&core, SessionsLocation::NAME, params).await.is_err());
}

#[tokio::test]
async fn the_method_reads_the_cores_process_table() {
    let home = tempfile::tempdir().unwrap();
    let config = LocalConfig::from_env(&|_| None, home.path());
    let root = home.path().join("probe");
    let core = test_core(SessionLocations::new(Some((config, root))), home.path());
    let params = json!({"source": "codex", "session_id": "550e8400-e29b-41d4-a716-446655440000"});
    let answer = call(&core, SessionsLocation::NAME, params).await.unwrap();
    let report: SessionLocationReport = serde_json::from_value(answer).unwrap();
    assert!(report.supported);
    assert!(!report.running);
    assert!(home.path().join("probe/session-locations.sqlite3").exists());
}

#[tokio::test]
async fn a_session_list_change_starts_an_observation_pass() {
    async fn created(store: &std::path::Path) {
        for _ in 0..200 {
            if store.exists()
                && Connection::open(store).ok().and_then(|db| {
                    db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                        .ok()
                }) == Some(1)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("no observation pass ran");
    }
    let home = tempfile::tempdir().unwrap();
    let config = LocalConfig::from_env(&|_| None, home.path());
    let root = home.path().join("probe");
    let locations = Arc::new(SessionLocations::new(Some((config.clone(), root))));
    let core = test_core(SessionLocations::new(None), home.path());
    let task = tokio::spawn(watch(core.clone(), locations));
    // The timer's first pass; the next is far off, so only the change can run one.
    let store = home.path().join("probe/session-locations.sqlite3");
    created(&store).await;
    Connection::open(&store)
        .unwrap()
        .execute_batch("PRAGMA user_version=0;")
        .unwrap();
    core.hub
        .invalidated(agent_relay_node::relay::state::Listing::Sessions);
    created(&store).await;
    task.abort();
}

fn pane(capability: &str) -> SessionLocation {
    SessionLocation {
        application: "Kitty".into(),
        bundle_id: "net.kovidgoyal.kitty".into(),
        pid: 30,
        process_start: 31,
        boot_id: Some("boot-1".into()),
        tty: Some("/dev/pts/7".into()),
        focus_capability: Some(capability.into()),
        resume_capability: Some("argv".into()),
        host_executable: Some("/usr/bin/kitty".into()),
        host_pid: Some(10),
        host_process_start: Some(11),
        cwd: Some("/original space/'project'".into()),
        executable: Some("/tools with space/codex".into()),
        ..Default::default()
    }
}
#[test]
fn original_survives_pruning_and_latest_survives_restart_and_boot_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let mut l = pane("kitty");
    save(&database(dir.path()).unwrap(), "codex", "id", &[l.clone()]).unwrap();
    let first = l.clone();
    for i in 1..25 {
        l.observed_at_ms = i;
        l.process_start = 100 + i as u64;
        save(&database(dir.path()).unwrap(), "codex", "id", &[l.clone()]).unwrap();
    }
    let (original, latest) = origins(&database(dir.path()).unwrap(), "codex", "id").unwrap();
    assert_eq!(original, Some(first));
    assert_eq!(latest, Some(l.clone()));
    assert_eq!(
        history(&database(dir.path()).unwrap(), "codex", "id")
            .unwrap()
            .len(),
        20
    );
    l.boot_id = Some("next-boot".into());
    l.observed_at_ms += 1;
    save(&database(dir.path()).unwrap(), "codex", "id", &[l.clone()]).unwrap();
    assert_eq!(
        origins(&database(dir.path()).unwrap(), "codex", "id")
            .unwrap()
            .1,
        Some(l)
    );
}

#[test]
fn late_observation_cannot_replace_newer_resume_host_directory_or_environment() {
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    // Two observers capture A then B, but B commits before the delayed A.
    let request_db = database(dir.path()).unwrap();
    let observer_db = database(dir.path()).unwrap();
    let mut older = pane("kitty");
    older.observed_at_ms = 100;
    older.cwd = Some("/old-project".into());
    older.provider_environment = Some(BTreeMap::from([("CODEX_HOME".into(), "/old-store".into())]));
    older.provider_environment_known = Some(true);
    let mut newer = older.clone();
    newer.observed_at_ms = 200;
    newer.pid += 1;
    newer.application = "WezTerm".into();
    newer.bundle_id = "wezterm".into();
    newer.cwd = Some("/new-project".into());
    newer.provider_environment = Some(BTreeMap::from([("CODEX_HOME".into(), "/new-store".into())]));
    save(&observer_db, "codex", "id", &[newer.clone()]).unwrap();
    save(&request_db, "codex", "id", &[older.clone()]).unwrap();
    drop((request_db, observer_db));
    let db = database(dir.path()).unwrap();
    let (_, latest) = origins(&db, "codex", "id").unwrap();
    assert_eq!(latest, Some(newer.clone()));
    assert_eq!(history(&db, "codex", "id").unwrap(), vec![newer.clone(), older]);
}

#[test]
fn stale_or_equal_time_observation_cannot_replace_same_process_history() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path()).unwrap();
    let mut newer = pane("kitty");
    newer.observed_at_ms = 200;
    newer.cwd = Some("/new-project".into());
    save(&db, "codex", "id", &[newer.clone()]).unwrap();
    for timestamp in [100, 200] {
        let mut late = newer.clone();
        late.observed_at_ms = timestamp;
        late.cwd = Some("/late-project".into());
        save(&db, "codex", "id", &[late]).unwrap();
        assert_eq!(origins(&db, "codex", "id").unwrap().1, Some(newer.clone()));
        assert_eq!(history(&db, "codex", "id").unwrap(), vec![newer.clone()]);
    }
    let mut latest = newer;
    latest.observed_at_ms += 1;
    latest.cwd = Some("/latest-project".into());
    save(&db, "codex", "id", &[latest.clone()]).unwrap();
    assert_eq!(history(&db, "codex", "id").unwrap(), vec![latest]);
}
#[test]
fn host_discovery_captures_only_navigation_identity_and_supports_linux_and_windows_paths() {
    let table = processes();
    for executable in [
        "/usr/bin/kitty",
        r"C:\Program Files\WindowsApps\WindowsTerminal.exe",
    ] {
        table
            .processes
            .lock()
            .unwrap()
            .get_mut(&10)
            .unwrap()
            .executable = Some(executable.into());
        {
            let mut processes = table.processes.lock().unwrap();
            let p = processes.get_mut(&30).unwrap();
            p.environment.insert("KITTY_WINDOW_ID".into(), "7".into());
            p.environment
                .insert("KITTY_LISTEN_ON".into(), "unix:/tmp/kitty.sock".into());
            p.environment
                .insert("SECRET_API_KEY".into(), "must not persist".into());
            p.cwd = Some("/project".into());
        }
        let location = from_process(&table, 501, 30).unwrap();
        assert_eq!(location.host_pid, Some(10));
        assert_eq!(location.host_process_start, Some(11));
        assert_eq!(location.pane_id.as_deref(), Some("7"));
        assert_eq!(
            location.environment_identity.as_deref(),
            Some("unix:/tmp/kitty.sock")
        );
        assert!(!serde_json::to_string(&location)
            .unwrap()
            .contains("must not persist"));
    }
}
#[test]
fn missing_process_lookup_retains_unknown_but_changed_incarnation_proves_old_owner_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = LocalConfig::from_env(&|_| None, dir.path());
    config.uid = 501;
    let table = FakeProcesses::default();
    let mut old = pane("kitty");
    old.boot_id = hosts::boot_id();
    assert_eq!(
        providers::owner_state(&config, &table, "codex", "id", &[old.clone()]),
        "unknown"
    );
    table.insert(
        30,
        FakeProcess {
            uid: 501,
            start: 99,
            executable: Some("/other/process".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        providers::owner_state(&config, &table, "codex", "id", &[old]),
        "stopped"
    );
}

#[test]
fn protected_system_process_does_not_block_a_positively_exited_recorded_owner() {
    use agent_relay_node::relay::process::{CommandLine, StartClock};
    use std::path::PathBuf;

    struct ProtectedInventory {
        recorded_owner_gone: bool,
    }
    impl ProcessTable for ProtectedInventory {
        fn process(&self, _: i32) -> Option<ProcessInfo> {
            None
        }
        fn process_gone(&self, pid: i32) -> bool {
            pid == 30 && self.recorded_owner_gone
        }
        fn all_pids(&self) -> Vec<i32> {
            vec![4]
        }
        fn executable(&self, _: i32) -> Option<PathBuf> {
            None
        }
        fn is_trusted_codex(&self, _: &Path) -> bool {
            false
        }
        fn command_line(&self, _: i32) -> CommandLine {
            CommandLine::default()
        }
        fn open_files(&self, _: i32) -> Vec<PathBuf> {
            vec![]
        }
        fn cwd(&self, _: i32) -> Option<PathBuf> {
            None
        }
        fn start_clock(&self) -> StartClock {
            StartClock::Windows
        }
    }
    let home = tempfile::tempdir().unwrap();
    let config = LocalConfig::from_env(&|_| None, home.path());
    let old = SessionLocation {
        pid: 30,
        process_start: 31,
        ..Default::default()
    };
    let table = ProtectedInventory {
        recorded_owner_gone: true,
    };
    assert_eq!(
        providers::owner_state(&config, &table, "codex", "id", &[old.clone()]),
        "stopped"
    );
    // The same inaccessible inventory cannot establish a never-observed session's exit.
    assert_eq!(
        providers::owner_state(&config, &table, "codex", "id", &[]),
        "unknown"
    );
    let denied_owner = ProtectedInventory {
        recorded_owner_gone: false,
    };
    assert_eq!(
        providers::owner_state(&config, &denied_owner, "codex", "id", &[old]),
        "unknown"
    );
}

#[test]
fn cross_platform_claude_records_use_native_data_roots_and_canonical_mapping() {
    for platform in [providers::Platform::Windows, providers::Platform::Linux] {
        let dir = tempfile::tempdir().unwrap();
        let mut config = LocalConfig::from_env(&|_| None, dir.path());
        config.claude.app_sessions = None;
        let base = dir.path().join("data");
        let roots = providers::claude_metadata_roots(&config, platform, &|key| {
            matches!(key, "APPDATA" | "XDG_CONFIG_HOME")
                .then(|| base.to_string_lossy().into_owned())
        });
        assert_eq!(roots, vec![base.join("Claude/claude-code-sessions")]);
        let account = roots[0].join("org/account");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(
            account.join("mapped.json"),
            r#"{"sessionId":"local_windowslinux","cliSessionId":"cli-real","isArchived":false}"#,
        )
        .unwrap();
        assert_eq!(
            providers::claude_desktop_id(&roots[0], "cli-real").as_deref(),
            Some("local_windowslinux")
        );
        assert!(providers::claude_desktop_id(&roots[0], "cli-unrelated").is_none());
    }
}
#[test]
fn windows_and_linux_gui_ancestry_does_not_confuse_the_claude_cli_with_desktop() {
    let table = processes();
    for (path, expected) in [
        (
            r"C:\Users\me\AppData\Local\AnthropicClaude\app\Claude.exe",
            "Claude",
        ),
        (r"C:\Users\me\AppData\Local\Claude\Claude.exe", "Claude"),
        ("/usr/bin/claude-desktop", "Claude"),
        ("/usr/bin/claude", "Background process"),
    ] {
        table
            .processes
            .lock()
            .unwrap()
            .get_mut(&10)
            .unwrap()
            .executable = Some(path.into());
        assert_eq!(from_process(&table, 501, 30).unwrap().application, expected);
    }
}
#[test]
fn store_rolls_back_both_origin_and_location_if_any_write_fails() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path()).unwrap();
    db.execute_batch("CREATE TRIGGER reject_location BEFORE INSERT ON locations BEGIN SELECT RAISE(FAIL,'injected failure'); END;").unwrap();
    assert!(save(&db, "codex", "id", &[pane("kitty")]).is_err());
    assert_eq!(origins(&db, "codex", "id").unwrap(), (None, None));
    assert!(history(&db, "codex", "id").unwrap().is_empty());
}
#[test]
fn old_store_backfill_runs_once_and_retains_earliest_and_latest_observations() {
    let dir = tempfile::tempdir().unwrap();
    {
        let db = Connection::open(dir.path().join("session-locations.sqlite3")).unwrap();
        db.execute_batch("CREATE TABLE locations(source TEXT,session_id TEXT,pid INTEGER,start TEXT,location TEXT,PRIMARY KEY(source,session_id,pid,start));").unwrap();
        for time in [3, 1, 2] {
            let mut l = pane("kitty");
            l.observed_at_ms = time;
            l.process_start = time as u64;
            db.execute(
                "INSERT INTO locations VALUES ('codex','id',30,?1,?2)",
                params![time.to_string(), serde_json::to_string(&l).unwrap()],
            )
            .unwrap();
        }
    }
    let db = database(dir.path()).unwrap();
    let (first, last) = origins(&db, "codex", "id").unwrap();
    assert_eq!(first.unwrap().observed_at_ms, 1);
    assert_eq!(last.unwrap().observed_at_ms, 3);
    db.execute_batch("DELETE FROM origins;").unwrap();
    drop(db);
    assert_eq!(
        origins(&database(dir.path()).unwrap(), "codex", "id").unwrap(),
        (None, None)
    );
}

#[test]
fn claude_registry_app_owner_maps_cli_id_and_reopens_without_a_stale_pid_target() {
    use agent_relay_node::relay::process::StartClock;
    let dir = tempfile::tempdir().unwrap();
    let mut config = LocalConfig::from_env(&|_| None, dir.path());
    config.uid = 501;
    let app_root = dir.path().join("Claude");
    config.claude.app_sessions = Some(app_root.join("local-agent-mode-sessions"));
    let table = FakeProcesses {
        clock: Some(StartClock::Ticks),
        ..Default::default()
    };
    table.insert(
        10,
        FakeProcess {
            uid: 501,
            start: 11,
            executable: Some("/Applications/Claude.app/Contents/MacOS/Claude".into()),
            ..Default::default()
        },
    );
    table.insert(
        30,
        FakeProcess {
            uid: 501,
            start: 31,
            parent: 10,
            executable: Some("/tools/claude".into()),
            cwd: Some("/project".into()),
            ..Default::default()
        },
    );
    let id = "550e8400-e29b-41d4-a716-446655440000";
    std::fs::create_dir_all(config.claude.registry()).unwrap();
    std::fs::write(config.claude.registry().join("30.json"),json!({"sessionId":id,"pid":30,"procStart":"31","messagingSocketPath":"/tmp/fake-claude.sock","kind":"interactive","entrypoint":"claude-desktop"}).to_string()).unwrap();
    let account = app_root.join("claude-code-sessions/org/account");
    std::fs::create_dir_all(&account).unwrap();
    std::fs::write(
        account.join("mapping.json"),
        json!({"sessionId":"local_real","cliSessionId":id,"isArchived":false}).to_string(),
    )
    .unwrap();
    let r = report(&config, &dir.path().join("origins"), &table, "claude", id).unwrap();
    assert!(r.running);
    assert_eq!(r.current[0].application, "Claude");
    assert!(r.current[0].tty.is_none());
    assert_eq!(
        r.desktop_link.as_deref(),
        Some("claude://code/continue?session=local_real")
    );
    assert_eq!(r.current[0].pid, 30);
    {
        let mut processes = table.processes.lock().unwrap();
        let p = processes.get_mut(&30).unwrap();
        p.start = 32;
        p.executable = Some("/unrelated/process".into());
    }
    let ended = report(&config, &dir.path().join("origins"), &table, "claude", id).unwrap();
    assert_eq!(ended.owner_state.as_deref(), Some("stopped"));
    assert!(ended.current.is_empty());
    assert_eq!(ended.desktop_link, r.desktop_link);
}
#[test]
fn tmux_server_is_a_verified_host_for_the_socket_and_pane_adapter() {
    let table = processes();
    {
        let mut processes = table.processes.lock().unwrap();
        processes.get_mut(&10).unwrap().executable = Some("/usr/bin/tmux".into());
        let p = processes.get_mut(&30).unwrap();
        p.environment
            .insert("TMUX".into(), "/tmp/tmux-501/default,10,0".into());
        p.environment.insert("TMUX_PANE".into(), "%7".into());
    }
    let l = from_process(&table, 501, 30).unwrap();
    assert_eq!(l.application, "tmux");
    assert_eq!(l.host_pid, Some(10));
    assert_eq!(l.focus_capability.as_deref(), Some("tmux"));
    assert_eq!(
        l.environment_identity.as_deref(),
        Some("/tmp/tmux-501/default")
    );
}
