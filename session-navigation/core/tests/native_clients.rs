//! Behaviour verified natively on the real apps (native N8-N14): Codex
//! under its daemon and OpenCode clients, as decisions and post-conditions on
//! hand-built snapshots.
#[path = "findings/builders.rs"]
mod builders;

use builders::*;
use session_navigation::decide::decide;
use session_navigation::engine::verify;
use session_navigation::evidence::*;
use session_navigation::model::*;
use session_navigation::plan::*;

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

// ------------------------------------------------------ Codex daemon mode ---

const DAEMON: u32 = 950;

fn codex_table(tuis: &[(u32, &[&str])]) -> Vec<ProcessInfo> {
    let mut table = vec![
        launchd(),
        proc_(
            DAEMON,
            1,
            Exe::CodexCli,
            None,
            &["codex", "app-server", "--listen", "unix://"],
        ),
    ];
    for (pid, argv) in tuis {
        table.extend(in_ghostty(
            &format!("/dev/ttys{pid}"),
            *pid,
            Exe::CodexCli,
            argv,
        ));
    }
    table.sort_by_key(|p| p.identity.pid);
    table.dedup_by_key(|p| p.identity.pid);
    table
}

fn codex(table: Vec<ProcessInfo>, ev: CodexEvidence) -> Decision {
    decide(
        &snap(
            Provider::Codex,
            CODEX_ID,
            table,
            ProviderEvidence::Codex(ev),
        ),
        &req(),
    )
}

/// N8-N10: a daemon-held lock whose TUI cannot be located resumes in a new
/// terminal as another client, labelled; never exact by argv or daemon.
#[test]
fn n9_codex_unlocatable_daemon_client_resumes_labelled() {
    let d = codex(
        codex_table(&[(31, &["codex"])]),
        codex_ev(CodexOrigin::Tui, vec![DAEMON]),
    );
    let a = d.action().expect("resume");
    assert_eq!(a.kind, ActionKind::Resume, "{d:?}");
    assert_eq!(a.note, Some(Reason::PossiblySecondView));
    assert_eq!(d.state, SessionState::LiveNotLocatable);
    // A desktop-origin thread is still resumed as a CLI client: a desktop
    // link would make the app-server a second writer.
    let d = codex(
        codex_table(&[(31, &["codex"])]),
        codex_ev(CodexOrigin::Desktop, vec![DAEMON]),
    );
    assert_eq!(
        d.action().map(|a| a.kind),
        Some(ActionKind::Resume),
        "{d:?}"
    );
}

/// N10 (a): a TUI whose argv names the thread is focused, at RightWindow,
/// instead of opening a needless second client.
#[test]
fn n10_codex_locatable_client_is_focused_not_duplicated() {
    let d = codex(
        codex_table(&[(31, &["codex"]), (32, &["codex", "resume", CODEX_ID])]),
        codex_ev(CodexOrigin::Tui, vec![DAEMON]),
    );
    let a = d.action().expect("focus");
    assert_eq!(a.kind, ActionKind::FocusPane, "{d:?}");
    assert_eq!(a.precision, Precision::RightWindow);
}

/// The second-view resume needs the verified daemon CLI, a live thread and
/// no other writer it cannot place.
#[test]
fn n10_codex_second_view_keeps_its_guards() {
    let mut s = snap(
        Provider::Codex,
        CODEX_ID,
        codex_table(&[(31, &["codex"])]),
        ProviderEvidence::Codex(codex_ev(CodexOrigin::Tui, vec![DAEMON])),
    );
    if let Some(t) = s.tools.iter_mut().find(|t| t.tool == Tool::CodexCli) {
        t.version = Fact::Known(Some("0.150.0".into()));
    }
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::LiveNotLocatable)
    );
    let mut archived = codex_ev(CodexOrigin::Tui, vec![DAEMON]);
    archived.thread = Fact::Known(Some(ThreadInfo {
        archived: true,
        origin: CodexOrigin::Tui,
        subagent: false,
    }));
    assert_eq!(
        codex(codex_table(&[(31, &["codex"])]), archived).plan,
        Plan::Unavailable(Reason::LiveNotLocatable)
    );
    let mut with_exec = codex_table(&[(31, &["codex"])]);
    with_exec.push(proc_(
        960,
        1,
        Exe::CodexCli,
        None,
        &["codex", "exec", "resume", CODEX_ID],
    ));
    assert_eq!(
        codex(with_exec, codex_ev(CodexOrigin::Tui, vec![DAEMON, 960])).plan,
        Plan::Unavailable(Reason::LiveNotLocatable)
    );
    // An unclassifiable holder still blocks.
    assert_eq!(
        codex(codex_table(&[]), codex_ev(CodexOrigin::Tui, vec![4242])).plan,
        Plan::Unavailable(Reason::LiveNotLocatable)
    );
}

/// N12: a resume is verified only when a writer lock appears that was not
/// there before; a TUI at the update menu holds none, and a second client
/// of the daemon adds none.
#[test]
fn n12_codex_resume_needs_a_new_lock_holder() {
    let tty = "/dev/ttys33";
    let table = codex_table(&[(33, &["codex", "resume", CODEX_ID])]);
    let at = |holders: Vec<u32>| {
        let mut s = snap(
            Provider::Codex,
            CODEX_ID,
            table.clone(),
            ProviderEvidence::Codex(codex_ev(CodexOrigin::Tui, holders)),
        );
        s.front = Front {
            app: Fact::Known(Some(App::Ghostty)),
            pane_tty: Fact::Known(Some(tty.into())),
            view: Fact::Known(None),
            pane: session_navigation::evidence::Fact::Known(None),
            window: session_navigation::evidence::Fact::Known(None),
        };
        s
    };
    let post = PostCondition::RunningIn {
        terminal: TerminalKind::Ghostty,
    };
    let check = |before, after| verify::check_against(&post, &at(before), &at(after), &req());
    assert_eq!(check(vec![], vec![DAEMON]), Outcome::Verified);
    assert_eq!(check(vec![], vec![]), Outcome::DispatchedUnverified);
    assert_eq!(
        check(vec![DAEMON], vec![DAEMON]),
        Outcome::DispatchedUnverified
    );
}

// --------------------------------------------------------------- OpenCode ---

fn oc(table: Vec<ProcessInfo>, ev: OpenCodeEvidence) -> Decision {
    decide(
        &snap(
            Provider::OpenCode,
            OC_ID,
            table,
            ProviderEvidence::OpenCode(ev),
        ),
        &req(),
    )
}

fn plain_tui() -> Vec<ProcessInfo> {
    in_ghostty("/dev/ttys044", 44, Exe::OpenCodeCli, &["opencode"])
}

/// N13-N14: another TUI never refuses a resume; every guard for the
/// verified hazards still does.
#[test]
fn n14_opencode_plain_tui_labels_but_every_hazard_guard_holds() {
    let d = oc(plain_tui(), opencode_ev());
    assert_eq!(
        d.action().and_then(|a| a.note),
        Some(Reason::PossiblySecondView),
        "{d:?}"
    );
    let refused = |ev: OpenCodeEvidence| oc(plain_tui(), ev).plan;
    let base = opencode_ev();
    assert_eq!(
        refused(OpenCodeEvidence {
            session: Fact::Unknown,
            ..base.clone()
        }),
        Plan::Unavailable(Reason::EvidenceIncomplete)
    );
    assert_eq!(
        refused(OpenCodeEvidence {
            session: Fact::Known(None),
            ..base.clone()
        }),
        Plan::Unavailable(Reason::SessionNotFound)
    );
    let session = |archived, child| {
        Fact::Known(Some(OpenCodeSession {
            archived,
            child,
            directory: DIR.into(),
        }))
    };
    assert_eq!(
        refused(OpenCodeEvidence {
            session: session(true, false),
            ..base.clone()
        }),
        Plan::Unavailable(Reason::Archived)
    );
    assert_eq!(
        refused(OpenCodeEvidence {
            session: session(false, true),
            ..base.clone()
        }),
        Plan::Unavailable(Reason::ChildSession)
    );
    assert_eq!(
        refused(OpenCodeEvidence {
            busy: Fact::Known(true),
            ..base.clone()
        }),
        Plan::Unavailable(Reason::SessionBusy)
    );
    let other_server = OpenCodeEvidence {
        server: Fact::Known(ServerState::Up {
            pid: 1111,
            version: "2.3.0".into(),
        }),
        ..base.clone()
    };
    assert_eq!(
        refused(other_server),
        Plan::Unavailable(Reason::ServerVersionMismatch)
    );
    let mut unknown_env = snap(
        Provider::OpenCode,
        OC_ID,
        plain_tui(),
        ProviderEvidence::OpenCode(base),
    );
    unknown_env.environment = Fact::Unknown;
    assert_eq!(
        decide(&unknown_env, &req()).plan,
        Plan::Unavailable(Reason::EnvironmentUnknown)
    );
}

/// N14 made a second client safe for the writer, but the running OpenCode
/// desktop is still the client to return to: a session last seen in a
/// terminal is brought forward in the running desktop, not given a TUI.
#[test]
fn n14_running_opencode_desktop_is_preferred_to_a_new_tui() {
    let mut s = snap(
        Provider::OpenCode,
        OC_ID,
        vec![launchd()],
        ProviderEvidence::OpenCode(opencode_ev()),
    );
    s.apps = Fact::Known(apps(&[App::OpenCodeDesktop], Some("2.2.6")));
    s.last_seen_host = Fact::Known(Host::Terminal(TerminalKind::AppleTerminal));
    let d = decide(&s, &req());
    assert_eq!(
        d.action().map(|a| a.kind),
        Some(ActionKind::BringForward),
        "{d:?}"
    );
}
