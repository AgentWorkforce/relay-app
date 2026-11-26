//! The OS-independent Grok observer, tool resolution and the per-platform
//! provider-path defaults, over `FakeSystem`. Builds with any feature set.
use serde_json::json;
use session_navigation::evidence::{Fact, GrokActive, GrokSession, ProviderEvidence};
use session_navigation::model::{Provider, SessionRef, Tool};
use session_navigation_native::common::config::{CommonConfig, Programs, ProviderPaths, Timeouts};
use session_navigation_native::common::parse::grok_version_from_name;
use session_navigation_native::common::tools::ToolResolver;
use session_navigation_native::common::{grok, observe_provider};
use session_navigation_native::system::{Call, FakeSystem, Output, Run, RunError};

const HOME: &str = "/home/example";
const GROK: &str = "/home/example/.grok";
const ID: &str = "0b5a3c1e-1111-4222-8333-444455556666";
const PROJECT: &str = "/home/example/code/project";

fn paths() -> ProviderPaths {
    let at = |rel: &str| format!("{HOME}/{rel}");
    ProviderPaths {
        claude_home: at(".claude"),
        claude_desktop_support: at(".config/Claude"),
        codex_home: at(".codex"),
        codex_sqlite_home: at(".codex"),
        opencode_data: at(".local/share/opencode"),
        opencode_state: at(".local/state/opencode"),
        opencode_channel: "latest".into(),
        opencode_desktop_support: at(".local/share/ai.opencode.desktop"),
        grok_home: GROK.into(),
        reservation_dir: at(".local/state/session-navigation/reservations"),
        tool_search_path: vec![at(".grok/bin"), "/usr/bin".into()],
    }
}

fn cfg() -> CommonConfig {
    CommonConfig {
        paths: paths(),
        programs: Programs {
            sqlite3: "/usr/bin/sqlite3".into(),
            curl: "/usr/bin/curl".into(),
            lsof: "/usr/bin/lsof".into(),
        },
        timeouts: Timeouts::default(),
    }
}

fn summary(id: &str, subagent: bool) -> Vec<u8> {
    let mut value = json!({ "info": { "id": id, "cwd": PROJECT } });
    if subagent {
        value["session_kind"] = "subagent".into();
    }
    value.to_string().into_bytes()
}

fn summary_path(id: &str) -> String {
    format!("{GROK}/sessions/%2Fhome%2Fexample%2Fcode%2Fproject/{id}/summary.json")
}

fn active_json() -> Vec<u8> {
    json!([
        { "session_id": ID, "pid": 4242, "cwd": PROJECT, "opened_at": "2026-11-11T10:00:00Z" },
        { "session_id": "not-a-uuid", "pid": 1 },
    ])
    .to_string()
    .into_bytes()
}

fn session() -> SessionRef {
    SessionRef::new(Provider::Grok, ID)
}

fn evidence(fake: &FakeSystem, session: &SessionRef) -> (ProviderEvidence, Option<String>, bool) {
    let found = observe_provider(fake, &cfg(), session, None, &Fact::Unknown);
    (found.evidence, found.cwd, found.found)
}

#[test]
fn found_with_its_registry_record() {
    let fake = FakeSystem::new();
    fake.add_file(&format!("{GROK}/active_sessions.json"), active_json(), 1);
    fake.add_file(&summary_path(ID), summary(ID, false), 1);
    fake.add_dir(&format!("{GROK}/sessions/%2Ftmp"));
    let (evidence, cwd, found) = evidence(&fake, &session());
    assert_eq!(
        evidence,
        ProviderEvidence::Grok(session_navigation::evidence::GrokEvidence {
            active: Fact::Known(vec![GrokActive {
                session_id: ID.into(),
                pid: 4242,
                cwd: Some(PROJECT.into()),
                opened_at: Some(1_794_391_200),
            }]),
            session: Fact::Known(Some(GrokSession {
                cwd: PROJECT.into(),
                subagent: false,
            })),
        })
    );
    assert_eq!((cwd.as_deref(), found), (Some(PROJECT), true));
    // Grok itself is never run.
    assert!(fake.runs().is_empty());
}

#[test]
fn upper_case_ids_find_the_lower_case_directory() {
    let fake = FakeSystem::new();
    fake.add_file(&summary_path(ID), summary(ID, false), 1);
    let upper = SessionRef::new(Provider::Grok, ID.to_ascii_uppercase());
    assert!(evidence(&fake, &upper).2);
}

#[test]
fn not_found_only_after_a_complete_listing() {
    // No home at all: no TUI running, no session.
    let fake = FakeSystem::new();
    let (evidence, cwd, found) = evidence(&fake, &session());
    assert_eq!(
        evidence,
        ProviderEvidence::Grok(session_navigation::evidence::GrokEvidence {
            active: Fact::Known(vec![]),
            session: Fact::Known(None),
        })
    );
    assert_eq!((cwd, found), (None, false));

    // Other sessions only.
    let fake = FakeSystem::new();
    let other = "0b5a3c1e-1111-4222-8333-000000000000";
    fake.add_file(&summary_path(other), summary(other, false), 1);
    assert_eq!(
        grok::session(&fake, &paths(), ID),
        Fact::Known(None),
        "listed every cwd directory"
    );

    // An unreadable cwd directory could hold it.
    fake.make_unreadable(&format!(
        "{GROK}/sessions/%2Fhome%2Fexample%2Fcode%2Fproject"
    ));
    assert_eq!(grok::session(&fake, &paths(), ID), Fact::Unknown);

    // So could an unreadable sessions directory.
    let fake = FakeSystem::new();
    fake.add_dir(&format!("{GROK}/sessions"));
    fake.make_unreadable(&format!("{GROK}/sessions"));
    assert_eq!(grok::session(&fake, &paths(), ID), Fact::Unknown);
}

#[test]
fn a_session_without_a_good_summary_or_found_twice_is_unknown() {
    for bytes in [
        b"{".to_vec(),
        b"{\"info\":{}}".to_vec(),
        summary("0b5a3c1e-1111-4222-8333-000000000000", false),
    ] {
        let fake = FakeSystem::new();
        fake.add_file(&summary_path(ID), bytes, 1);
        assert_eq!(grok::session(&fake, &paths(), ID), Fact::Unknown);
    }
    let fake = FakeSystem::new();
    fake.add_dir(&format!(
        "{GROK}/sessions/%2Fhome%2Fexample%2Fcode%2Fproject/{ID}"
    ));
    assert_eq!(
        grok::session(&fake, &paths(), ID),
        Fact::Unknown,
        "no summary"
    );

    let fake = FakeSystem::new();
    fake.add_file(&summary_path(ID), summary(ID, false), 1);
    fake.add_file(
        &format!("{GROK}/sessions/%2Ftmp/{ID}/summary.json"),
        summary(ID, false),
        1,
    );
    assert_eq!(
        grok::session(&fake, &paths(), ID),
        Fact::Unknown,
        "two cwds"
    );
}

#[test]
fn unreadable_or_corrupt_registry_is_unknown() {
    let registry = format!("{GROK}/active_sessions.json");
    for bytes in [b"{}".to_vec(), b"[".to_vec(), Vec::new()] {
        let fake = FakeSystem::new();
        fake.add_file(&registry, bytes, 1);
        assert_eq!(grok::active(&fake, &paths()), Fact::Unknown);
    }
    let fake = FakeSystem::new();
    fake.add_file(&registry, active_json(), 1);
    fake.make_unreadable(&registry);
    fake.add_file(&summary_path(ID), summary(ID, false), 1);
    let (evidence, _, found) = evidence(&fake, &session());
    let ProviderEvidence::Grok(evidence) = evidence else {
        panic!("grok evidence");
    };
    assert_eq!(evidence.active, Fact::Unknown);
    assert!(evidence.session.is_known() && found);
}

#[test]
fn subagent_sessions_are_marked() {
    let fake = FakeSystem::new();
    fake.add_file(&summary_path(ID), summary(ID, true), 1);
    assert_eq!(
        grok::session(&fake, &paths(), ID),
        Fact::Known(Some(GrokSession {
            cwd: PROJECT.into(),
            subagent: true,
        }))
    );
}

#[test]
fn invalid_ids_touch_nothing() {
    for id in [
        "../../etc/passwd",
        "",
        "0b5a3c1e-1111-4222-8333-44445555666/",
        "x",
    ] {
        let fake = FakeSystem::new();
        let (evidence, cwd, found) = evidence(&fake, &SessionRef::new(Provider::Grok, id));
        assert_eq!(
            evidence,
            ProviderEvidence::Grok(session_navigation::evidence::GrokEvidence {
                active: Fact::Unknown,
                session: Fact::Unknown,
            })
        );
        assert_eq!((cwd, found), (None, false));
        assert!(fake.calls().is_empty(), "{id}: {:?}", fake.calls());
    }
}

#[test]
fn grok_version_from_its_canonical_name() {
    assert_eq!(
        grok_version_from_name("grok-0.1.23-darwin-arm64").as_deref(),
        Some("0.1.23")
    );
    assert_eq!(
        grok_version_from_name("grok-1.2.3-windows-x86_64.exe").as_deref(),
        Some("1.2.3")
    );
    for bad in [
        "grok",
        "grok-0.1.23",
        "grok-0.1.23-darwin",
        "grok-latest-darwin-arm64",
        "grok-1-darwin-arm64",
        "grok-0.1.23-darwin-arm64-extra",
        "grok-0..1-darwin-arm64",
        "claude-0.1.2-darwin-arm64",
    ] {
        assert_eq!(grok_version_from_name(bad), None, "{bad}");
    }
}

#[test]
fn tool_versions_for_grok_and_tmux() {
    let fake = FakeSystem::new();
    let link = format!("{HOME}/.grok/bin/grok");
    let real = format!("{HOME}/.grok/downloads/grok-0.1.23-linux-x86_64");
    fake.add_executable(&link, 1);
    fake.add_executable(&real, 1);
    fake.add_symlink(&link, &real);
    fake.add_executable("/usr/bin/tmux", 1);
    fake.on(
        &Run::new("/usr/bin/tmux", &["-V"]),
        vec![Ok(Output::ok("tmux 3.5a\n"))],
    );
    let mut tools = ToolResolver::new();
    let all = tools.all(&fake, &cfg());
    assert_eq!(
        all.iter().map(|t| t.tool).collect::<Vec<_>>(),
        Tool::ALL.to_vec()
    );
    let info = |tool: Tool| all.iter().find(|t| t.tool == tool).cloned();
    assert_eq!(
        info(Tool::GrokCli).map(|t| (t.version, t.path)),
        Some((Fact::Known(Some("0.1.23".into())), Some(link.clone())))
    );
    assert_eq!(
        info(Tool::Tmux).map(|t| t.version),
        Some(Fact::Known(Some("3.5a".into())))
    );
    assert_eq!(
        fake.runs(),
        vec![("/usr/bin/tmux".to_owned(), vec!["-V".to_owned()])],
        "grok is never run when its name carries the version"
    );
    assert!(fake.calls().contains(&Call::Canonicalize(link.clone())));

    // A grok whose name says nothing is asked.
    let fake = FakeSystem::new();
    fake.add_executable(&link, 1);
    fake.on(
        &Run::new(&link, &["--version"]),
        vec![Ok(Output::ok("grok 0.2.0\n"))],
    );
    let info = ToolResolver::new().tool(&fake, &cfg(), Tool::GrokCli);
    assert_eq!(info.version, Fact::Known(Some("0.2.0".into())));

    // tmux failing: unknown, with its path.
    let fake = FakeSystem::new();
    fake.add_executable("/usr/bin/tmux", 1);
    fake.on(
        &Run::new("/usr/bin/tmux", &["-V"]),
        vec![Err(RunError::TimedOut)],
    );
    let info = ToolResolver::new().tool(&fake, &cfg(), Tool::Tmux);
    assert_eq!(
        (info.version, info.path),
        (Fact::Unknown, Some("/usr/bin/tmux".into()))
    );
}

#[test]
fn grok_home_from_env_and_the_recorded_environment() {
    let default = grok::default_home(HOME);
    assert_eq!(default, GROK);
    for (value, home) in [
        (None, GROK.to_owned()),
        (Some("  "), GROK.to_owned()),
        (Some("~"), HOME.to_owned()),
        (Some(" ~/alt-grok "), format!("{HOME}/alt-grok")),
        (Some("/srv/grok"), "/srv/grok".to_owned()),
    ] {
        assert_eq!(grok::home_from_env(value, HOME), home, "{value:?}");
    }
    assert!(grok::environment(GROK, &default).is_empty());
    assert!(grok::environment(&format!("{GROK}/"), &default).is_empty());
    assert_eq!(
        grok::environment("/srv/grok", &default),
        [("GROK_HOME".to_owned(), "/srv/grok".to_owned())].into()
    );
    // A relative home resolves against Grok's own cwd: nothing is read.
    let mut relative = paths();
    relative.grok_home = grok::home_from_env(Some("rel/grok"), HOME);
    let fake = FakeSystem::new();
    let found = grok::observe(&fake, &relative, ID);
    assert_eq!(found.evidence, grok::unknown());
    assert!(fake.calls().is_empty());
}

#[test]
fn directory_names_must_decode_to_the_recorded_cwd() {
    assert_eq!(
        grok::decode_component("%2Fhome%2Fexample%2Fcode%2Fproject").as_deref(),
        Some(PROJECT)
    );
    assert_eq!(grok::decode_component("%2fa%20b").as_deref(), Some("/a b"));
    for bad in ["%2", "%zz", "a b", "/x", "%FF"] {
        assert_eq!(grok::decode_component(bad), None, "{bad}");
    }
    // The summary says another directory than the one it is stored under.
    let fake = FakeSystem::new();
    fake.add_file(
        &format!("{GROK}/sessions/%2Fhome%2Fexample%2Fcode%2Fother/{ID}/summary.json"),
        summary(ID, false),
        1,
    );
    assert_eq!(grok::session(&fake, &paths(), ID), Fact::Unknown);
}

#[test]
fn documentation_derived_store_fixtures() {
    // fixtures/grok/PROVENANCE.txt: shapes from the relay and relayhistory
    // readers, never captured (Grok was not installed).
    let cwd = "/Users/example/code/project";
    let mut p = paths();
    p.grok_home = "/Users/example/.grok".into();
    let dir = format!(
        "{}/sessions/%2FUsers%2Fexample%2Fcode%2Fproject/{ID}",
        p.grok_home
    );
    for (file, subagent) in [("summary.json", false), ("summary-subagent.json", true)] {
        let fake = FakeSystem::new();
        let bytes = std::fs::read(format!(
            "{}/tests/fixtures/grok/{file}",
            env!("CARGO_MANIFEST_DIR")
        ));
        fake.add_file(&format!("{dir}/summary.json"), bytes.unwrap_or_default(), 1);
        assert_eq!(
            grok::session(&fake, &p, ID),
            Fact::Known(Some(GrokSession {
                cwd: cwd.into(),
                subagent
            })),
            "{file}"
        );
    }
    let fake = FakeSystem::new();
    fake.add_file(
        &p.grok_active_sessions(),
        include_str!("fixtures/grok/active_sessions.json"),
        1,
    );
    let active = grok::active(&fake, &p);
    assert!(
        matches!(&active, Fact::Known(list) if list.len() == 2 && list.iter().all(|a| a.opened_at.is_some()))
    );
}

#[cfg(feature = "linux")]
#[test]
fn linux_defaults() {
    let paths = session_navigation_native::linux::provider_paths("/home/example/");
    assert_eq!(paths.claude_home, "/home/example/.claude");
    assert_eq!(paths.claude_desktop_support, "/home/example/.config/Claude");
    assert_eq!(
        paths.codex_state_db(),
        "/home/example/.codex/state_5.sqlite"
    );
    assert_eq!(
        paths.opencode_db(),
        "/home/example/.local/share/opencode/opencode.db"
    );
    assert_eq!(
        paths.grok_active_sessions(),
        "/home/example/.grok/active_sessions.json"
    );
    assert_eq!(
        paths.reservation_dir,
        "/home/example/.local/state/session-navigation/reservations"
    );
    assert!(paths
        .tool_search_path
        .contains(&"/home/example/.grok/bin".to_owned()));
}

#[cfg(feature = "windows")]
#[test]
fn windows_defaults() {
    let paths = session_navigation_native::windows::provider_paths(
        r"C:\Users\example",
        r"C:\Users\example\AppData\Roaming",
        r"C:\Users\example\AppData\Local",
    );
    assert_eq!(paths.claude_home, r"C:\Users\example\.claude");
    assert_eq!(
        paths.claude_desktop_support,
        r"C:\Users\example\AppData\Roaming\Claude"
    );
    assert_eq!(paths.codex_home, r"C:\Users\example\.codex");
    assert_eq!(paths.grok_home, r"C:\Users\example\.grok");
    assert_eq!(
        paths.reservation_dir,
        r"C:\Users\example\AppData\Local\session-navigation\reservations"
    );
}

#[cfg(feature = "macos")]
#[test]
fn macos_defaults_are_unchanged() {
    let cfg = session_navigation_native::MacConfig::for_user("/Users/example", 501);
    let paths = &cfg.common.paths;
    assert_eq!(
        paths.reservation_dir,
        "/Users/example/Library/Application Support/session-navigation/reservations"
    );
    assert_eq!(
        paths.claude_desktop_sessions_dir(),
        "/Users/example/Library/Application Support/Claude/claude-code-sessions"
    );
    assert_eq!(paths.grok_home, "/Users/example/.grok");
    assert!(paths
        .tool_search_path
        .contains(&"/Users/example/.grok/bin".to_owned()));
    assert_eq!(cfg.common.programs.sqlite3, "/usr/bin/sqlite3");
}
