//! OpenCode desktop and the OpenCode TUI in Terminal.
use super::*;

// ------------------------------------------------------ OpenCode desktop ---

#[test]
fn opencode_desktop_live_is_app_only() {
    let mut t = start(OD);
    expect_bring(&t.plan(), App::OpenCodeDesktop, Reason::NoSessionDeepLink);
}

#[test]
fn opencode_desktop_with_plain_tui_in_same_dir() {
    let mut t = start(OD);
    t.w.start_session(StartHost::OpenCodeTerminal { with_id: false }, &t.cwd());
    t.apply(Event::QuitApp {
        app: App::OpenCodeDesktop,
    });
    // A TUI is a client, not a writer (native N14): no refusal.
    expect_bring(&t.plan(), App::OpenCodeDesktop, Reason::NoSessionDeepLink);
}

#[test]
fn opencode_desktop_stopped() {
    let mut t = start(OD);
    t.apply(Event::QuitApp {
        app: App::OpenCodeDesktop,
    });
    expect_bring(&t.plan(), App::OpenCodeDesktop, Reason::NoSessionDeepLink);
}

#[test]
fn opencode_desktop_archived() {
    let mut t = start(OD);
    let pick = t.pick();
    t.apply(Event::Archive { pick });
    expect_unavailable(&t.plan(), Reason::Archived);
}

#[test]
fn opencode_desktop_deleted() {
    let mut t = start(OD);
    let pick = t.pick();
    t.apply(Event::Delete { pick });
    expect_unavailable(&t.plan(), Reason::SessionNotFound);
}

#[test]
fn opencode_desktop_unknown_server_and_db() {
    let mut t = start(OD);
    t.apply(degrade(Channel::OpenCodeHung))
        .apply(degrade(Channel::OpenCodeDb));
    expect_unavailable(&t.plan(), Reason::EvidenceIncomplete);
}

#[test]
fn opencode_desktop_unverified_version_keeps_app_only_plan() {
    let mut t = start(OD);
    t.apply(upgrade_app(App::OpenCodeDesktop));
    expect_bring(&t.plan(), App::OpenCodeDesktop, Reason::NoSessionDeepLink);
}

#[test]
fn opencode_desktop_cwd_missing_still_brings_forward() {
    let mut t = start(OD);
    t.apply(Event::QuitApp {
        app: App::OpenCodeDesktop,
    })
    .apply(Event::DeleteDir { dir: 0 });
    expect_bring(&t.plan(), App::OpenCodeDesktop, Reason::NoSessionDeepLink);
}

#[test]
fn opencode_desktop_app_missing() {
    let mut t = start(OD);
    t.apply(Event::Uninstall {
        app: App::OpenCodeDesktop,
    });
    expect_unavailable(&t.plan(), Reason::AppNotInstalled);
    let d = t.plan_with(&default_terminal());
    expect_launch(
        &d,
        TerminalKind::AppleTerminal,
        "opencode",
        &["-s", &t.s.id],
        &t.cwd(),
    );
    assert_eq!(act(&d).precision, Precision::RightWindow);
}

#[test]
fn opencode_desktop_plus_tui_focuses_the_tui() {
    let mut t = start(OD);
    person_resume(&mut t);
    let owner = t.owner();
    expect_focus(
        &t.plan(),
        TerminalKind::AppleTerminal,
        &owner,
        Precision::RightWindow,
    );
}

// ----------------------------------------------------- OpenCode TUI (Terminal) ---

#[test]
fn opencode_tui_live_locatable() {
    let mut t = start(OT);
    let owner = t.owner();
    expect_focus(
        &t.plan(),
        TerminalKind::AppleTerminal,
        &owner,
        Precision::RightWindow,
    );
}

/// A plain TUI may show the session but cannot be located; it is a client
/// of the server, the only writer (native N13-N14): resume, labelled.
#[test]
fn opencode_plain_tui_is_not_locatable() {
    let mut t = start(StartHost::OpenCodeTerminal { with_id: false });
    let d = t.plan();
    // Discovery cannot attribute a plain TUI, so the default terminal runs it.
    expect_launch(
        &d,
        TerminalKind::Ghostty,
        "opencode",
        &["-s", &t.s.id],
        &t.cwd(),
    );
    assert_eq!(act(&d).note, Some(Reason::PossiblySecondView));
    assert_eq!(act(&d).precision, Precision::RightWindow);
}

#[test]
fn opencode_tui_stopped() {
    let mut t = start(OT);
    t.apply(Event::ClosePane { pick: 0 });
    let d = t.plan();
    expect_launch(
        &d,
        TerminalKind::AppleTerminal,
        "opencode",
        &["-s", &t.s.id],
        &t.cwd(),
    );
}

#[test]
fn opencode_tui_archived() {
    let mut t = start(OT);
    let pick = t.pick();
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::Archive { pick });
    expect_unavailable(&t.plan(), Reason::Archived);
}

#[test]
fn opencode_tui_deleted_never_launches() {
    let mut t = start(OT);
    let pick = t.pick();
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::Delete { pick });
    expect_unavailable(&t.plan(), Reason::SessionNotFound);
}

#[test]
fn opencode_tui_unknown_existence() {
    let mut t = start(OT);
    t.apply(Event::ClosePane { pick: 0 })
        .apply(degrade(Channel::OpenCodeHung))
        .apply(degrade(Channel::OpenCodeDb));
    expect_unavailable(&t.plan(), Reason::EvidenceIncomplete);
}

#[test]
fn opencode_tui_unverified_cli() {
    let mut t = start(OT);
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::ServerStop { crash: false })
        .apply(upgrade_tool(Tool::OpenCodeCli));
    expect_unavailable(&t.plan(), Reason::UnverifiedVersion);
}

#[test]
fn opencode_tui_server_version_mismatch() {
    let mut t = start(OT);
    t.apply(Event::ClosePane { pick: 0 })
        .apply(upgrade_tool(Tool::OpenCodeCli));
    expect_unavailable(&t.plan(), Reason::ServerVersionMismatch);
}

#[test]
fn opencode_tui_cwd_missing() {
    let mut t = start(OT);
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::DeleteDir { dir: 0 });
    expect_unavailable(&t.plan(), Reason::CwdMissing);
}

#[test]
fn opencode_tui_app_missing() {
    let mut t = start(OT);
    t.apply(Event::Uninstall {
        app: App::AppleTerminal,
    });
    expect_unavailable(&t.plan(), Reason::AppNotInstalled);
}

#[test]
fn opencode_tui_multiple_live() {
    let mut t = start(OT);
    person_resume(&mut t);
    expect_choose(&t.plan(), 2);
}
