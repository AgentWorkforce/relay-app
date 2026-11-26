use super::*;

// ===================================== 2. oracle blind spots (mutations) ===

/// FINDING (false confidence). The oracle never checks a launch's working
/// directory: `check_input` checks argv only. A resume in the wrong repo is
/// harmful: Claude finds transcripts under `projects/<slug(cwd)>` (Claude
/// facts F, "changing directory changes the slug"), and a resumed agent that
/// does start acts on the wrong tree. The simulator also lets `claude
/// --resume` succeed from any directory (`claude_cli_resume` ignores cwd), so
/// a planner regression that picked the wrong cwd would verify. Fix: oracle
/// requires `command.cwd == truth cwd of the session`; make cross-directory
/// resume a world variable.
#[test]
fn oracle_flags_a_resume_launched_in_the_wrong_directory() {
    let mut w = clean(WorldVars::default());
    let s = w.start_session(StartHost::ClaudeGhostty, dir(0)).unwrap();
    let pane = pane_of(&w, &s);
    w.close_pane(pane);
    let action = launch_action(
        TerminalKind::Ghostty,
        "claude",
        &["--resume", &s.id],
        dir(1),
    );
    let verdict = rogue(&mut w, &s, action);
    assert!(
        verdict.is_err(),
        "oracle accepted `claude --resume` in {} for a session recorded in {}",
        dir(1),
        dir(0)
    );
}

/// FINDING (false confidence). A TUI launch whose CLI version differs from
/// the running service stops that service and starts its own (OpenCode facts
/// G, VERIFIED code: `mismatch:"replace"`), interrupting the desktop's
/// clients. The sim models the replacement, but the oracle has no invariant
/// for it, so only the planner's own ServerVersionMismatch guard stands
/// between a regression and this harm. Fix: log server replacement caused by
/// an engine open and fail on it.
#[test]
fn oracle_flags_an_engine_launch_that_replaces_the_shared_opencode_server() {
    let mut w = clean(WorldVars::default());
    w.start_session(StartHost::OpenCodeDesktop, dir(0)).unwrap();
    let s2 = w
        .start_session(StartHost::OpenCodeTerminal { with_id: true }, dir(1))
        .unwrap();
    let pane = pane_of(&w, &s2);
    w.close_pane(pane);
    w.tools.insert(Tool::OpenCodeCli, Some("2.2.7".into())); // staged update drift (facts G)
    let server_before = w.opencode.server;
    let action = launch_action(
        TerminalKind::AppleTerminal,
        "opencode",
        &["-s", &s2.id],
        dir(1),
    );
    let verdict = rogue(&mut w, &s2, action);
    assert_ne!(
        w.opencode.server, server_before,
        "precondition: the harm happened"
    );
    assert!(
        w.running(App::OpenCodeDesktop),
        "precondition: under a running desktop"
    );
    assert!(
        verdict.is_err(),
        "oracle accepted replacing the desktop's server"
    );
}

/// Revised by native N13-N14 (was: "engine never relaunches the desktop onto
/// a session a TUI shows"). Relaunching the OpenCode desktop for session B
/// restores its last tab A, which a TUI shows: that is a second client of
/// the server, the only writer, so it is allowed. It must still add no
/// writer, create nothing and leave the shared server alone.
#[test]
fn opencode_desktop_relaunch_next_to_a_tui_adds_a_client_not_a_writer() {
    let mut w = clean(WorldVars::default());
    let b = w.start_session(StartHost::OpenCodeDesktop, dir(1)).unwrap();
    w.apply(&Event::Discover);
    let a = w.start_session(StartHost::OpenCodeDesktop, dir(0)).unwrap();
    w.apply(&Event::QuitApp {
        app: App::OpenCodeDesktop,
    });
    // The person continues A in a TUI.
    let cmd = Command {
        program: "opencode".into(),
        args: vec!["-s".into(), a.id.clone()],
        cwd: dir(0).into(),
        env: Default::default(),
    };
    w.person_runs(TerminalKind::AppleTerminal, &cmd);
    let server = w.opencode.server;
    let (r, verdict) = open(&mut w, &b, &req());
    verdict.expect("oracle");
    assert_eq!(kind(&r.decision), Some(ActionKind::BringForward));
    assert_eq!(w.writers(&a), 0, "clients are not writers");
    assert_eq!(w.opencode.server, server, "the shared server is intact");
}

/// Strength: the classic harms are caught — a launch next to a live owner
/// (NoDuplicates) and a focus on a pane that never hosted the session
/// (NoStaleHandles).
#[test]
fn oracle_catches_duplicate_launch_and_foreign_pane_focus() {
    let mut w = clean(WorldVars::default());
    let s = w.start_session(StartHost::ClaudeGhostty, dir(0)).unwrap();
    let dup = launch_action(
        TerminalKind::Ghostty,
        "claude",
        &["--resume", &s.id],
        dir(0),
    );
    assert!(
        matches!(rogue(&mut w, &s, dup), Err(v) if v.invariant == oracle::Invariant::NoDuplicates)
    );

    let mut w = clean(WorldVars::default());
    let s = w.start_session(StartHost::ClaudeGhostty, dir(0)).unwrap();
    let other = w.start_session(StartHost::ClaudeGhostty, dir(1)).unwrap();
    let p = w
        .procs
        .values()
        .find(|p| p.hosts.as_ref() == Some(&other))
        .unwrap()
        .clone();
    let identity = session_navigation::model::ProcessIdentity {
        pid: p.pid,
        start: p.start,
        boot: session_navigation::model::BootId(w.boot),
    };
    let focus = Action {
        kind: ActionKind::FocusPane,
        steps: vec![Step::FocusPane {
            terminal: TerminalKind::Ghostty,
            pane: session_navigation::hosts::PaneRef::Tty {
                tty: p.tty.clone().unwrap(),
            },
            process: identity,
        }],
        precision: Precision::RightWindow,
        post: PostCondition::PaneFocused {
            terminal: TerminalKind::Ghostty,
            pane: session_navigation::hosts::PaneRef::Tty {
                tty: p.tty.clone().unwrap(),
            },
            process: identity,
        },
        note: None,
        import: false,
        target: None,
    };
    assert!(
        matches!(rogue(&mut w, &s, focus), Err(v) if v.invariant == oracle::Invariant::NoStaleHandles)
    );
}
