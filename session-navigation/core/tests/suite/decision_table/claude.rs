//! Claude desktop and Claude CLI in Ghostty.
use super::*;

// ------------------------------------------------------- Claude desktop ---

#[test]
fn claude_desktop_live_locatable() {
    let mut t = start(CD);
    let d = t.plan();
    let url = format!("claude://code/continue?session={}", t.local_id());
    expect_url(&d, ActionKind::OpenExisting, &url, App::ClaudeDesktop);
    assert_eq!(d.state, SessionState::LiveLocatable);
}

#[test]
fn claude_desktop_live_not_locatable_after_unrewritten_clear() {
    let mut t = start_with(
        WorldVars {
            claude_registry_rewritten_on_clear: false,
            ..vars()
        },
        CD,
    );
    t.apply(Event::Clear { pick: 0 });
    // The registry still names the old id, but the desktop now holds it only
    // as a prior id: the window cannot show it.
    expect_unavailable(&t.plan(), Reason::LiveNotLocatable);
}

#[test]
fn claude_desktop_stopped_is_not_closed() {
    let mut t = start(CD);
    t.apply(Event::QuitApp {
        app: App::ClaudeDesktop,
    });
    let d = t.plan();
    let url = format!("claude://code/continue?session={}", t.local_id());
    expect_url(&d, ActionKind::OpenExisting, &url, App::ClaudeDesktop);
    assert_eq!(d.state, SessionState::Stopped);
}

#[test]
fn claude_desktop_archived_uses_resume_link_not_continue() {
    let mut t = start(CD);
    let pick = t.pick();
    t.apply(Event::Archive { pick });
    let d = t.plan();
    let url = format!("claude://resume?session={}", t.s.id);
    expect_url(&d, ActionKind::Reopen, &url, App::ClaudeDesktop);
    assert!(!act(&d).import);
    assert_eq!(d.state, SessionState::Archived);
}

#[test]
fn claude_desktop_deleted() {
    let mut t = start(CD);
    let pick = t.pick();
    t.apply(Event::Delete { pick });
    expect_unavailable(&t.plan(), Reason::SessionNotFound);
}

#[test]
fn claude_desktop_unknown_registry() {
    let mut t = start(CD);
    t.apply(degrade(Channel::ClaudeRegistry));
    expect_unavailable(&t.plan(), Reason::EvidenceIncomplete);
}

#[test]
fn claude_desktop_unverified_version() {
    let mut t = start(CD);
    t.apply(upgrade_app(App::ClaudeDesktop));
    expect_bring(&t.plan(), App::ClaudeDesktop, Reason::UnverifiedVersion);
}

#[test]
fn claude_desktop_cwd_missing_still_opens_in_app() {
    let mut t = start(CD);
    t.apply(Event::QuitApp {
        app: App::ClaudeDesktop,
    })
    .apply(Event::DeleteDir { dir: 0 });
    let url = format!("claude://code/continue?session={}", t.local_id());
    expect_url(
        &t.plan(),
        ActionKind::OpenExisting,
        &url,
        App::ClaudeDesktop,
    );
}

#[test]
fn claude_desktop_app_missing() {
    let mut t = start(CD);
    t.apply(Event::Uninstall {
        app: App::ClaudeDesktop,
    });
    expect_unavailable(&t.plan(), Reason::AppNotInstalled);
}

#[test]
fn claude_desktop_multiple_live() {
    let mut t = start(CD);
    person_resume(&mut t);
    let d = t.plan();
    expect_choose(&d, 2);
    assert_eq!(d.state, SessionState::LiveMultiple);
}

// ------------------------------------------------------ Claude in Ghostty ---

#[test]
fn claude_ghostty_live_locatable() {
    let mut t = start(CG);
    let owner = t.owner();
    // RightWindow: whether the registry follows /clear is UNVERIFIED.
    expect_focus(
        &t.plan(),
        TerminalKind::Ghostty,
        &owner,
        Precision::RightWindow,
    );
}

#[test]
fn claude_ghostty_live_not_locatable_outside_known_terminal() {
    let mut t = start(CG);
    let login = t.w.panes.values().next().unwrap().login;
    t.w.procs.get_mut(&login).unwrap().ppid = 1; // e.g. an ssh or detached chain
    let d = t.plan();
    expect_unavailable(&d, Reason::LiveNotLocatable);
    assert_eq!(d.state, SessionState::LiveNotLocatable);
}

#[test]
fn claude_ghostty_stopped_resumes_in_last_seen_terminal() {
    let mut t = start(CG);
    t.apply(Event::ClosePane { pick: 0 });
    let d = t.plan_with(&terminal_prefs());
    expect_launch(
        &d,
        TerminalKind::Ghostty,
        "claude",
        &["--resume", &t.s.id],
        &t.cwd(),
    );
    assert_eq!(act(&d).precision, Precision::ExactConversation);
}

#[test]
fn claude_ghostty_archive_does_not_apply_to_cli_sessions() {
    let mut t = start(CG);
    let pick = t.pick();
    t.apply(Event::Archive { pick });
    let owner = t.owner();
    expect_focus(
        &t.plan(),
        TerminalKind::Ghostty,
        &owner,
        Precision::RightWindow,
    );
}

#[test]
fn claude_ghostty_deleted() {
    let mut t = start(CG);
    let pick = t.pick();
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::Delete { pick });
    expect_unavailable(&t.plan(), Reason::SessionNotFound);
}

#[test]
fn claude_ghostty_unknown_process_table() {
    let mut t = start(CG);
    t.apply(degrade(Channel::ProcTable));
    let d = t.plan();
    expect_unavailable(&d, Reason::EvidenceIncomplete);
    assert_eq!(d.state, SessionState::Unknown);
}

#[test]
fn claude_ghostty_unverified_versions() {
    let mut t = start(CG);
    t.apply(upgrade_app(App::Ghostty));
    expect_bring(&t.plan(), App::Ghostty, Reason::UnverifiedVersion);
    let mut t = start(CG);
    t.apply(Event::ClosePane { pick: 0 })
        .apply(upgrade_tool(Tool::ClaudeCli));
    expect_unavailable(&t.plan(), Reason::UnverifiedVersion);
}

#[test]
fn claude_ghostty_cwd_missing() {
    let mut t = start(CG);
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::DeleteDir { dir: 0 });
    expect_unavailable(&t.plan(), Reason::CwdMissing);
}

#[test]
fn claude_ghostty_app_missing() {
    let mut t = start(CG);
    t.apply(Event::Uninstall { app: App::Ghostty });
    expect_unavailable(&t.plan(), Reason::AppNotInstalled);
    let d = t.plan_with(&default_terminal());
    expect_launch(
        &d,
        TerminalKind::AppleTerminal,
        "claude",
        &["--resume", &t.s.id],
        &t.cwd(),
    );
}

#[test]
fn claude_ghostty_multiple_live() {
    let mut t = start(CG);
    person_resume(&mut t);
    expect_choose(&t.plan(), 2);
}

#[test]
fn claude_ghostty_live_with_unknown_record_age() {
    // The registry file's age (this boot or an earlier one) is unreadable:
    // the owner may or may not be live, so nothing is planned.
    let mut t = start(CG);
    t.apply(degrade(Channel::Fs));
    let d = t.plan();
    expect_unavailable(&d, Reason::EvidenceIncomplete);
    assert_eq!(d.state, SessionState::Unknown);
}
