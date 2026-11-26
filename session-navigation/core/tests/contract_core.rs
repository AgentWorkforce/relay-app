//! The adapter contract at the snapshot level: unknown facts refuse, paths
//! and identities are checked, Grok's files parse, Windows consoles are
//! found by their console host.
#[path = "findings/builders.rs"]
mod builders;
#[path = "contract_core/locating.rs"]
mod locating;

use builders::*;
use session_navigation::decide::decide;
use session_navigation::decide::interpret::host_of;
use session_navigation::evidence::*;
use session_navigation::hosts::{
    Ability, PaneInfo, PaneRef, Surfaces, TerminalSupport, TmuxClientInfo, TmuxPaneInfo,
    TmuxServer, Unsupported, WindowId, WindowInfo,
};
use session_navigation::model::*;
use session_navigation::parse::{grok, is_absolute_path, is_windows_absolute_path};
use session_navigation::plan::*;

const GROK_ID: &str = "abcdefab-1234-4abc-8def-0123456789ab";

fn req() -> OpenRequest {
    OpenRequest {
        // An explicit terminal preference: there is no built-in default.
        prefs: session_navigation::plan::Preferences {
            default_terminal: Some(session_navigation::model::TerminalKind::Ghostty),
            ..Default::default()
        },
        ..OpenRequest::default()
    }
}

fn claude_stopped() -> Snapshot {
    snap(
        Provider::Claude,
        CLAUDE_ID,
        vec![launchd()],
        ProviderEvidence::Claude(claude_ev(vec![], vec![], Some(Entrypoint::Cli))),
    )
}

#[test]
fn unknown_apps_schemes_or_front_never_become_absent() {
    let mut s = claude_stopped();
    assert!(launch_of(&decide(&s, &req())).is_some());
    s.apps = Fact::Unknown;
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::EvidenceIncomplete)
    );
    let desktop = claim(LOCAL_A, ClaimKind::Live, false, CLAUDE_ID);
    let mut s = snap(
        Provider::Claude,
        CLAUDE_ID,
        vec![launchd()],
        ProviderEvidence::Claude(claude_ev(vec![], vec![desktop], None)),
    );
    assert!(decide(&s, &req()).action().is_some());
    s.schemes = Fact::Unknown;
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::EvidenceIncomplete)
    );
}

#[test]
fn unknown_front_app_leaves_a_focus_unverified_not_failed() {
    use session_navigation::engine::verify::check;
    let mut s = claude_stopped();
    s.front.app = Fact::Unknown;
    let post = PostCondition::AppFrontmost {
        app: App::ClaudeDesktop,
    };
    assert_eq!(check(&post, &s, &req()), Outcome::DispatchedUnverified);
    let post = PostCondition::PaneFocused {
        terminal: TerminalKind::Ghostty,
        pane: PaneRef::Tty {
            tty: "/dev/ttys001".into(),
        },
        process: ident(5),
    };
    assert_eq!(check(&post, &s, &req()), Outcome::DispatchedUnverified);
}

#[test]
fn absolute_paths_reject_every_control_character() {
    for bad in [
        "/a\tb",
        "/a\u{1b}[31m",
        "/a\u{7f}",
        "/a\u{85}",
        "/a\nb",
        "/a\0b",
    ] {
        assert!(!is_absolute_path(bad), "{bad:?}");
        assert!(
            !is_windows_absolute_path(&bad.replacen('/', "C:\\", 1)),
            "{bad:?}"
        );
    }
    assert!(is_absolute_path("/Users/example/with space"));
    assert!(is_windows_absolute_path("C:\\Users\\example\\code"));
    for bad in ["\\\\server\\share", "C:relative", "C:\\a|b", "/posix"] {
        assert!(!is_windows_absolute_path(bad), "{bad:?}");
    }
}

#[test]
fn a_windows_session_launches_only_with_windows_paths() {
    let mut s = claude_stopped();
    s.platform = Platform::Windows;
    s.surfaces = Surfaces {
        terminals: Fact::Known(vec![session_navigation::hosts::TerminalSupport {
            terminal: TerminalKind::WindowsTerminal,
            focus_pane: session_navigation::hosts::Ability::Unsupported(
                session_navigation::hosts::Unsupported::NoPaneApi,
            ),
            raise_window: session_navigation::hosts::Ability::Available,
            activate_app: session_navigation::hosts::Ability::Unsupported(
                session_navigation::hosts::Unsupported::NotImplemented,
            ),
            launch: session_navigation::hosts::Ability::Available,
            tabs_share_window: true,
        }]),
        ..Surfaces::mac_scripted()
    };
    s.apps = Fact::Known(
        App::ALL
            .into_iter()
            .map(|app| AppInfo {
                app,
                installed: app.identifier(Platform::Windows).is_some(),
                version: Some("1.0.0".into()),
                running: false,
            })
            .collect(),
    );
    let request = OpenRequest {
        prefs: Preferences {
            default_terminal: Some(TerminalKind::WindowsTerminal),
            ..Preferences::default()
        },
        ..req()
    };
    // POSIX paths are not absolute on Windows.
    assert_eq!(
        decide(&s, &request).plan,
        Plan::Unavailable(Reason::ToolNotInstalled)
    );
    for t in s.tools.iter_mut() {
        t.path = t.path.as_ref().map(|_| "C:\\bin\\claude.exe".into());
    }
    assert_eq!(
        decide(&s, &request).plan,
        Plan::Unavailable(Reason::CwdUnknown)
    );
    s.cwd = Fact::Known("C:\\Users\\example\\project".into());
    s.terminal_profile = Fact::Unknown;
    assert_eq!(
        decide(&s, &request).plan,
        Plan::Unavailable(Reason::EvidenceIncomplete),
        "an unknown profile cannot be preserved"
    );
    s.terminal_profile = Fact::Known(Some("{p}".into()));
    let d = decide(&s, &request);
    let Some(Step::Launch { context, .. }) = d.action().and_then(|a| a.steps.first()) else {
        panic!("{d:?}");
    };
    assert_eq!(context.profile.as_deref(), Some("{p}"));
    assert_eq!(context.runtime, Runtime::Native);
    s.runtime = Fact::Known(Runtime::Foreign {
        label: "ssh".into(),
    });
    assert_eq!(
        decide(&s, &request).plan,
        Plan::Unavailable(Reason::ForeignRuntime)
    );
    s.runtime = Fact::Unknown;
    assert_eq!(
        decide(&s, &request).plan,
        Plan::Unavailable(Reason::EvidenceIncomplete)
    );
}

#[test]
fn wsl_launches_are_refused_off_windows() {
    let mut s = claude_stopped();
    s.runtime = Fact::Known(Runtime::Wsl {
        distro: "Ubuntu".into(),
    });
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::ForeignRuntime)
    );
}

#[test]
fn a_classic_console_is_found_through_its_console_host() {
    let table = vec![
        launchd(),
        proc_(300, 1, Exe::Shell, None, &["cmd.exe"]),
        proc_(301, 300, Exe::ConsoleHost, None, &["conhost.exe"]),
        proc_(302, 300, Exe::ClaudeCli, None, &["claude.exe"]),
    ];
    let agent = table.last().cloned().unwrap();
    assert_eq!(
        host_of(&table, &agent),
        Fact::Known(Host::Terminal(TerminalKind::ConsoleHost))
    );
}

#[test]
fn grok_active_sessions_and_summaries_parse_strictly() {
    let file = format!(
        r#"[{{"session_id":"{}","pid":4242,"cwd":"/w","opened_at":"2026-11-11T10:00:00.250Z"}},
            {{"session_id":"not-a-uuid","pid":1}},{{"session_id":"{GROK_ID}","pid":-3}}]"#,
        GROK_ID.to_uppercase()
    );
    let records = grok::parse_active_sessions(file.as_bytes()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, GROK_ID, "ids compare in lower case");
    assert_eq!(records[0].opened_at, Some(1_794_391_200));
    assert!(
        grok::parse_active_sessions(b"{}").is_err(),
        "not a list is not empty"
    );
    assert_eq!(
        grok::rfc3339("2026-11-11T11:00:00+01:00"),
        Some(1_794_391_200)
    );
    assert_eq!(grok::rfc3339("2026-02-30T00:00:00Z"), None);
    let summary =
        format!(r#"{{"info":{{"id":"{GROK_ID}","cwd":"/w"}},"session_kind":"subagent"}}"#);
    let parsed = grok::parse_summary(summary.as_bytes(), GROK_ID).unwrap();
    assert!(parsed.subagent);
    assert!(grok::parse_summary(summary.as_bytes(), CLAUDE_OTHER).is_err());
    assert!(grok::parse_summary(br#"{"info":{"cwd":"relative"}}"#, GROK_ID).is_err());
    assert_eq!(grok::encode_cwd("/a b/é"), "%2Fa%20b%2F%C3%A9");
}

#[test]
fn a_grok_record_is_live_only_for_a_grok_process_started_before_it() {
    let tty = "/dev/ttys003";
    let table = in_ghostty(tty, 77, Exe::GrokCli, &["grok"]);
    let record = |opened_at| GrokActive {
        session_id: GROK_ID.into(),
        pid: 77,
        cwd: Some(DIR.into()),
        opened_at,
    };
    let ev = |opened_at| {
        ProviderEvidence::Grok(GrokEvidence {
            active: Fact::Known(vec![record(opened_at)]),
            session: Fact::Known(Some(GrokSession {
                cwd: DIR.into(),
                subagent: false,
            })),
        })
    };
    let grok_tool = ToolInfo {
        tool: Tool::GrokCli,
        version: Fact::Known(Some("1.0.46".into())),
        path: Some("/Users/example/.grok/bin/grok".into()),
    };
    let snap = |table, ev| {
        let mut s = snap(Provider::Grok, GROK_ID, table, ev);
        s.tools.push(grok_tool.clone());
        s
    };
    let s = snap(table.clone(), ev(Some(5_000)));
    let d = decide(&s, &req());
    assert_eq!(
        d.action().map(|a| a.kind),
        Some(ActionKind::FocusPane),
        "{d:?}"
    );
    // A pid recycled after the record was written is not the session; it is
    // another Grok in the directory, so a resume asks first.
    let s = snap(table.clone(), ev(Some(10)));
    assert!(matches!(decide(&s, &req()).plan, Plan::Confirm(_)));
    let s = snap(table, ev(None));
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::EvidenceIncomplete)
    );
}
