use super::*;

// ======================================================== 1. fidelity ===

/// FINDING (false confidence). Claude facts B: `code/continue` "gives no
/// success signal. The library cannot confirm that the session was
/// selected", and facts C: "open in the app" is not observable. Yet
/// `desktop_view_readable` hands `verify` the Claude desktop's current
/// `local_…` view in half of all worlds, so every Verified Claude
/// OpenExisting/Reopen, and the convergence and reservation-release paths
/// that hang off Verified, run on evidence no real observer has. Fix: make
/// the Claude view always Unknown (keep the variable for Codex IPC only).
#[test]
fn sim_never_hands_decide_a_claude_desktop_view() {
    let vars = WorldVars {
        desktop_view_readable: true,
        ..WorldVars::default()
    };
    let mut w = clean(vars);
    let s = w.start_session(StartHost::ClaudeDesktop, dir(0)).unwrap();
    let snap = w.observe(&s);
    assert_eq!(snap.front.app, Fact::Known(Some(App::ClaudeDesktop)));
    assert_eq!(
        snap.front.view,
        Fact::Unknown,
        "the sim reveals which Claude conversation is shown"
    );
}

/// FINDING (false confidence). Claude facts D.3 (VERIFIED): argv is
/// unreliable because "the session can change after start". The sim models
/// that for Claude (/clear) but hard-codes that a Codex TUI never changes
/// thread, and the Codex policy trusts `codex resume <id>` argv at
/// ExactConversation (relay CODE, Codex facts 15, not VERIFIED). If the TUI
/// can start a new thread in-process (UNVERIFIED either way), the engine
/// verifies a wrong conversation at ExactConversation. This builds that world
/// and asks the oracle about the real engine's plan. Fix: a world variable
/// `codex_tui_switches_thread` driving a Codex /new event; RightWindow for
/// argv-only matches, Exact only with the TUI's own lock.
#[test]
fn codex_tui_that_switched_thread_is_not_claimed_exact() {
    let vars = WorldVars {
        codex_tui_holds_lock: true,
        ..WorldVars::default()
    };
    let mut w = clean(vars);
    let x = w.start_session(StartHost::CodexGhostty, dir(0)).unwrap();
    let pane = pane_of(&w, &x);
    w.close_pane(pane);
    let cmd = Command {
        program: "codex".into(),
        args: vec!["resume".into(), x.id.clone()],
        cwd: dir(0).into(),
        env: Default::default(),
    };
    w.person_runs(TerminalKind::Ghostty, &cmd);
    // In-TUI "new thread": the process now writes Y; argv still names X.
    let y_id = w.new_session_id(Provider::Codex);
    w.codex.threads.insert(
        y_id.clone(),
        Thread {
            cwd: dir(0).into(),
            archived: false,
            origin: CodexOrigin::Tui,
            subagent: false,
        },
    );
    let y = SessionRef::new(Provider::Codex, y_id);
    let pid = w
        .procs
        .values()
        .find(|p| p.hosts.as_ref() == Some(&x))
        .unwrap()
        .pid;
    let start = w.procs[&pid].start;
    w.past_hosts
        .entry((pid, start))
        .or_default()
        .push(x.clone());
    w.procs.get_mut(&pid).unwrap().hosts = Some(y);
    w.open_pane(TerminalKind::Ghostty, dir(1));
    let (r, verdict) = open(&mut w, &x, &req());
    assert!(
        verdict.is_ok(),
        "engine plan {:?} → {:?}; oracle: {verdict:?}",
        r.decision.plan,
        r.outcome
    );
}
