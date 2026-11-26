//! Behaviour verified natively on the real apps (native N1-N5, N15, N18):
//! parser, decision and post-condition tests on hand-built snapshots.
#[path = "findings/builders.rs"]
mod builders;

use builders::*;
use session_navigation::decide::decide;
use session_navigation::decide::interpret::host_of;
use session_navigation::engine::verify;
use session_navigation::evidence::*;
use session_navigation::model::*;
use session_navigation::parse::claude;
use session_navigation::plan::*;

macro_rules! fixture {
    ($p:literal) => {
        include_bytes!(concat!("fixtures/", $p)).as_slice()
    };
}

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

// ------------------------------------------------- Claude lastFocusedAt ---

fn record(last_focused: &str) -> Vec<u8> {
    format!(
        r#"{{"sessionId":"{LOCAL_A}","cliSessionId":"{CLAUDE_ID}","isArchived":false{last_focused}}}"#
    )
    .into_bytes()
}

/// N1: `lastFocusedAt` is read as Known(timestamp), Known(None) for `null`,
/// and Unknown when absent or not a timestamp.
#[test]
fn n1_claude_record_last_focused_is_parsed_without_guessing() {
    let parse = |extra: &str| {
        claude::parse_desktop_record(&record(extra))
            .map(|r| r.last_focused)
            .ok()
    };
    assert_eq!(
        parse(r#","lastFocusedAt":1790000400000"#),
        Some(Fact::Known(Some(1_790_000_400_000)))
    );
    assert_eq!(parse(r#","lastFocusedAt":null"#), Some(Fact::Known(None)));
    assert_eq!(parse(""), Some(Fact::Unknown));
    for junk in [
        r#","lastFocusedAt":"soon""#,
        r#","lastFocusedAt":-1"#,
        r#","lastFocusedAt":1.5"#,
    ] {
        assert_eq!(parse(junk), Some(Fact::Unknown), "{junk}");
    }
}

#[test]
fn n1_claude_fixtures_carry_last_focused() {
    let focused = claude::parse_desktop_record(fixture!("claude/desktop-session-local.json"));
    assert_eq!(
        focused.map(|r| r.last_focused).ok(),
        Some(Fact::Known(Some(1_790_000_400_000)))
    );
    let never = claude::parse_desktop_record(fixture!("claude/desktop-session-unfocused.json"));
    assert_eq!(never.map(|r| r.last_focused).ok(), Some(Fact::Known(None)));
    let absent =
        claude::parse_desktop_record(fixture!("claude/desktop-session-worktree-archived.json"));
    assert_eq!(absent.map(|r| r.last_focused).ok(), Some(Fact::Unknown));
}

fn focused(kind: ClaimKind, archived: bool, at: Fact<Option<u64>>) -> DesktopClaim {
    DesktopClaim {
        last_focused: at,
        ..claim(LOCAL_A, kind, archived, CLAUDE_ID)
    }
}

fn claude_snap(claims: Vec<DesktopClaim>) -> Snapshot {
    let mut s = snap(
        Provider::Claude,
        CLAUDE_ID,
        vec![launchd()],
        ProviderEvidence::Claude(claude_ev(vec![], claims, Some(Entrypoint::Desktop))),
    );
    s.front.app = Fact::Known(Some(App::ClaudeDesktop));
    s
}

fn shown(before: Vec<DesktopClaim>, after: Vec<DesktopClaim>) -> Outcome {
    let post = PostCondition::ConversationShown {
        app: App::ClaudeDesktop,
    };
    verify::check_against(&post, &claude_snap(before), &claude_snap(after), &req())
}

/// N1, N3, N5: a newer `lastFocusedAt` on a live, non-archived record is
/// the success signal; an unchanged one is not proof either way; an
/// archived record is a wrong destination whatever its timestamp (N4: the
/// epitaxy view moves it without reviving).
#[test]
fn n1_claude_link_is_verified_by_a_newer_last_focused() {
    use ClaimKind::{Live, Stem};
    let at = |t| Fact::Known(Some(t));
    let none = Fact::Known(None);
    let live = |t| vec![focused(Live, false, t)];
    assert_eq!(shown(live(none), live(at(5))), Outcome::Verified);
    assert_eq!(shown(live(at(5)), live(at(6))), Outcome::Verified);
    assert_eq!(
        shown(live(at(5)), live(at(5))),
        Outcome::DispatchedUnverified
    );
    assert_eq!(
        shown(live(at(6)), live(at(5))),
        Outcome::DispatchedUnverified
    );
    assert_eq!(
        shown(live(Fact::Unknown), live(at(5))),
        Outcome::DispatchedUnverified
    );
    assert_eq!(
        shown(live(at(5)), live(Fact::Unknown)),
        Outcome::DispatchedUnverified
    );
    // A record that did not exist before (an allowed import) counts as never
    // focused before.
    assert_eq!(shown(vec![], live(at(5))), Outcome::Verified);
    assert_eq!(
        shown(
            vec![focused(Live, true, at(5))],
            vec![focused(Live, true, at(6))]
        ),
        Outcome::Failed(Reason::WrongDestination)
    );
    assert_eq!(
        shown(
            vec![focused(Stem, false, at(5))],
            vec![focused(Stem, false, at(6))]
        ),
        Outcome::DispatchedUnverified,
        "a stem-only claim never proves this conversation"
    );
    // Without the plan's snapshot nothing can be compared.
    let post = PostCondition::ConversationShown {
        app: App::ClaudeDesktop,
    };
    assert_eq!(
        verify::check(&post, &claude_snap(live(at(9))), &req()),
        Outcome::DispatchedUnverified
    );
}

/// N5: archived record → `claude://resume?session=<cli uuid>`, unchanged.
#[test]
fn n5_claude_archived_record_plans_the_resume_link() {
    let s = claude_snap(vec![focused(ClaimKind::Live, true, Fact::Known(Some(1)))]);
    let d = decide(&s, &req());
    let a = d.action().expect("a plan");
    assert_eq!(a.kind, ActionKind::Reopen);
    assert_eq!(
        a.steps,
        vec![Step::OpenUrl {
            url: format!("claude://resume?session={CLAUDE_ID}"),
            bundle: App::ClaudeDesktop.bundle_id().into(),
        }]
    );
}

// ------------------------------------------------------ process ancestry ---

fn login(pid: u32, ppid: u32, tty: &str) -> ProcessInfo {
    ProcessInfo {
        euid: 0,
        ruid: 501,
        ..proc_(
            pid,
            ppid,
            Exe::Login,
            Some(tty),
            &["/usr/bin/login", "-flp", "example"],
        )
    }
}

/// N15: ghostty (uid 501) → /usr/bin/login (euid 0, ruid 501) → shell →
/// agent. The walk must reach Ghostty.
#[test]
fn n15_ancestry_walks_through_root_effective_login() {
    let tty = "/dev/ttys007";
    let table = vec![
        launchd(),
        proc_(811, 1, Exe::Ghostty, None, &["ghostty"]),
        login(900, 811, tty),
        proc_(901, 900, Exe::Shell, Some(tty), &["-/bin/zsh"]),
        proc_(902, 901, Exe::ClaudeCli, Some(tty), &["claude"]),
    ];
    let agent = table.last().cloned().expect("agent");
    assert_eq!(table.get(2).map(|p| (p.euid, p.ruid)), Some((0, 501)));
    assert_eq!(
        host_of(&table, &agent),
        Fact::Known(Host::Terminal(TerminalKind::Ghostty))
    );
    let ev = claude_ev(
        vec![registry(902, CLAUDE_ID, None)],
        vec![],
        Some(Entrypoint::Cli),
    );
    let d = decide(
        &snap(
            Provider::Claude,
            CLAUDE_ID,
            table,
            ProviderEvidence::Claude(ev),
        ),
        &req(),
    );
    assert_eq!(
        d.action().map(|a| a.kind),
        Some(ActionKind::FocusPane),
        "{d:?}"
    );
}

/// N18: a command Ghostty launches runs as `login … bash -c exec -l <cmd>`:
/// its parent is `login` itself, and `login`'s argv names the session id.
/// The agent is still located, and `login` is never taken for an agent.
#[test]
fn n18_launched_command_under_login_is_located() {
    let tty = "/dev/ttys008";
    let mut wrapper = login(900, 811, tty);
    wrapper
        .argv
        .push(format!("exec -l /opt/homebrew/bin/codex resume {CODEX_ID}"));
    let table = vec![
        launchd(),
        proc_(811, 1, Exe::Ghostty, None, &["ghostty"]),
        wrapper,
        proc_(
            902,
            900,
            Exe::CodexCli,
            Some(tty),
            &["codex", "resume", CODEX_ID],
        ),
    ];
    let ev = codex_ev(CodexOrigin::Tui, vec![]);
    let d = decide(
        &snap(
            Provider::Codex,
            CODEX_ID,
            table,
            ProviderEvidence::Codex(ev),
        ),
        &req(),
    );
    let a = d.action().expect("focus");
    assert_eq!(a.kind, ActionKind::FocusPane);
    assert!(
        matches!(a.steps.as_slice(), [Step::FocusPane { process, .. }] if process.pid == 902),
        "{a:?}"
    );
}
