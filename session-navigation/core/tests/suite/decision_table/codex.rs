//! Codex desktop and Codex CLI in Ghostty.
use super::*;

// -------------------------------------------------------- Codex desktop ---

#[test]
fn codex_desktop_live_locatable() {
    let mut t = start(XD);
    let url = format!("codex://threads/{}", t.s.id);
    expect_url(&t.plan(), ActionKind::OpenExisting, &url, App::CodexDesktop);
}

#[test]
fn codex_desktop_live_not_locatable_when_scheme_hijacked() {
    let mut t = start(XD);
    t.apply(Event::HijackScheme { codex: true });
    expect_unavailable(&t.plan(), Reason::LiveNotLocatable);
}

#[test]
fn codex_desktop_stopped() {
    let mut t = start(XD);
    t.apply(Event::QuitApp {
        app: App::CodexDesktop,
    });
    let url = format!("codex://threads/{}", t.s.id);
    let d = t.plan();
    expect_url(&d, ActionKind::OpenExisting, &url, App::CodexDesktop);
    assert_eq!(d.state, SessionState::Stopped);
}

#[test]
fn codex_desktop_archived() {
    let mut t = start(XD);
    let pick = t.pick();
    t.apply(Event::Archive { pick });
    let d = t.plan();
    let url = format!("codex://threads/{}", t.s.id);
    expect_url(&d, ActionKind::OpenExisting, &url, App::CodexDesktop);
    assert_eq!(act(&d).note, Some(Reason::Archived));
    assert_eq!(d.state, SessionState::Archived);
}

#[test]
fn codex_desktop_deleted() {
    let mut t = start(XD);
    let pick = t.pick();
    t.apply(Event::Delete { pick });
    expect_unavailable(&t.plan(), Reason::SessionNotFound);
}

#[test]
fn codex_desktop_unknown_db() {
    let mut t = start(XD);
    t.apply(degrade(Channel::CodexDb));
    expect_unavailable(&t.plan(), Reason::EvidenceIncomplete);
}

#[test]
fn codex_desktop_unverified_version() {
    let mut t = start(XD);
    t.apply(upgrade_app(App::CodexDesktop));
    expect_bring(&t.plan(), App::CodexDesktop, Reason::UnverifiedVersion);
}

#[test]
fn codex_desktop_cwd_missing_still_links() {
    let mut t = start(XD);
    t.apply(Event::QuitApp {
        app: App::CodexDesktop,
    })
    .apply(Event::DeleteDir { dir: 0 });
    let url = format!("codex://threads/{}", t.s.id);
    expect_url(&t.plan(), ActionKind::OpenExisting, &url, App::CodexDesktop);
}

#[test]
fn codex_desktop_app_missing() {
    let mut t = start(XD);
    t.apply(Event::Uninstall {
        app: App::CodexDesktop,
    });
    expect_unavailable(&t.plan(), Reason::AppNotInstalled);
    // Explicitly choosing the default terminal resumes there instead.
    let d = t.plan_with(&default_terminal());
    expect_launch(
        &d,
        TerminalKind::AppleTerminal,
        "codex",
        &["resume", &t.s.id],
        &t.cwd(),
    );
}

#[test]
fn codex_desktop_multiple_live() {
    let mut t = start(XD);
    person_resume(&mut t);
    expect_choose(&t.plan(), 2);
}

// ------------------------------------------------------- Codex in Ghostty ---

/// Under the 0.167.0 daemon a TUI holds no lock (native N8): one started as
/// `codex resume <id>` is found by argv only, so it is the right window, not
/// the exact conversation.
#[test]
fn codex_ghostty_live_locatable() {
    let mut t = start(XG);
    t.apply(Event::ClosePane { pick: 0 });
    person_resume(&mut t);
    let owner = t.owner();
    expect_focus(
        &t.plan(),
        TerminalKind::Ghostty,
        &owner,
        Precision::RightWindow,
    );
}

/// A plain `codex` TUI cannot be located (native N9) and the daemon is the
/// only writer (N10): resume as another client, labelled.
#[test]
fn codex_ghostty_daemon_held_lock_resumes_as_possible_second_view() {
    for holds in [false, true] {
        let mut t = start_with(
            WorldVars {
                codex_tui_holds_lock: holds,
                ..vars()
            },
            XG,
        );
        let d = t.plan();
        expect_launch(
            &d,
            TerminalKind::Ghostty,
            "codex",
            &["resume", &t.s.id],
            &t.cwd(),
        );
        assert_eq!(act(&d).note, Some(Reason::PossiblySecondView));
        assert_eq!(d.state, SessionState::LiveNotLocatable);
    }
}

#[test]
fn codex_ghostty_stopped() {
    let mut t = start(XG);
    // Only an argv id makes the TUI identifiable to discovery (N9).
    person_resume(&mut t);
    t.apply(Event::Discover)
        .apply(Event::QuitApp { app: App::Ghostty });
    let d = t.plan_with(&terminal_prefs());
    expect_launch(
        &d,
        TerminalKind::Ghostty,
        "codex",
        &["resume", &t.s.id],
        &t.cwd(),
    );
}

#[test]
fn codex_ghostty_archived_refuses_cli_resume() {
    let mut t = start(XG);
    let pick = t.pick();
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::Archive { pick });
    let d = t.plan();
    expect_unavailable(&d, Reason::Archived);
    assert_eq!(d.state, SessionState::Archived);
}

#[test]
fn codex_ghostty_deleted() {
    let mut t = start(XG);
    let pick = t.pick();
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::Delete { pick });
    expect_unavailable(&t.plan(), Reason::SessionNotFound);
}

#[test]
fn codex_ghostty_unknown_locks() {
    let mut t = start(XG);
    t.apply(Event::ClosePane { pick: 0 })
        .apply(degrade(Channel::ProcTable));
    expect_unavailable(&t.plan(), Reason::EvidenceIncomplete);
}

#[test]
fn codex_ghostty_unverified_cli() {
    let mut t = start(XG);
    t.apply(Event::ClosePane { pick: 0 })
        .apply(upgrade_tool(Tool::CodexCli));
    expect_unavailable(&t.plan(), Reason::UnverifiedVersion);
}

#[test]
fn codex_ghostty_cwd_missing() {
    let mut t = start(XG);
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::DeleteDir { dir: 0 });
    expect_unavailable(&t.plan(), Reason::CwdMissing);
}

#[test]
fn codex_ghostty_app_missing() {
    let mut t = start(XG);
    t.apply(Event::Uninstall { app: App::Ghostty });
    expect_unavailable(&t.plan(), Reason::AppNotInstalled);
}

/// Two argv-identified TUIs: the person chooses. (A plain TUI next to an
/// identified one adds no choice: it cannot be located, N9.)
#[test]
fn codex_ghostty_multiple_live() {
    let mut t = start(XG);
    person_resume(&mut t);
    person_resume(&mut t);
    expect_choose(&t.plan(), 2);
}
