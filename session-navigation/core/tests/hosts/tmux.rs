use super::*;

#[test]
fn tmux_attached_focuses_the_pane_through_its_client() {
    let mut w = world(Platform::MacOs, vars());
    let s = start(
        &mut w,
        Provider::Grok,
        TerminalKind::Ghostty,
        Mux::Attached,
        false,
    );
    w.front_app = None;
    let r = open(&mut w, &s, &plain());
    match focus_ref(&r) {
        Some(PaneRef::TmuxPane { outer, .. }) => assert!(matches!(**outer, PaneRef::Tty { .. })),
        other => panic!("expected a tmux pane, got {other:?} ({:?})", r.decision),
    }
    assert_eq!(r.outcome, Outcome::Verified);
    // A second client does not make it ambiguous: both show the session.
    w.apply(&Event::TmuxAttach {
        pick: 0,
        terminal: 1,
    });
    let r = open(&mut w, &s, &plain());
    assert!(
        matches!(focus_ref(&r), Some(PaneRef::TmuxPane { .. })),
        "{:?}",
        r.decision
    );
}

#[test]
fn tmux_detached_attaches_a_client_and_starts_no_agent() {
    for platform in [Platform::MacOs, X11] {
        let mut w = world(platform, vars());
        let s = start(
            &mut w,
            Provider::Claude,
            TerminalKind::Ghostty,
            Mux::Detached,
            false,
        );
        let agents_before = w
            .procs
            .values()
            .filter(|p| p.hosts.as_ref() == Some(&s))
            .count();
        let r = open(&mut w, &s, &plain());
        let a = r.decision.action().expect("attach");
        assert_eq!(a.kind, ActionKind::Attach, "{platform:?} {:?}", r.decision);
        let Some(Step::Launch { command, .. }) = a.steps.first() else {
            panic!("attach is a launch");
        };
        assert!(command.program.ends_with("tmux"));
        assert!(command.args.iter().any(|x| x == "attach-session"));
        let agents_after = w
            .procs
            .values()
            .filter(|p| p.hosts.as_ref() == Some(&s))
            .count();
        assert_eq!(agents_before, agents_after, "an attach starts no agent");
        assert!(w.reservations.is_empty(), "an attach reserves nothing");
        if platform == Platform::MacOs {
            assert_eq!(r.outcome, Outcome::Verified);
        }
        // Now a client shows it: the next open focuses it.
        let next = open(&mut w, &s, &plain());
        assert!(
            matches!(focus_ref(&next), Some(PaneRef::TmuxPane { .. })),
            "{:?}",
            next.decision
        );
    }
}

#[test]
fn hazards_between_decision_and_step_are_caught_by_identity() {
    let cases = [
        (
            Hazard::RestartTmux,
            Mux::Attached,
            TerminalKind::Ghostty,
            Platform::MacOs,
        ),
        (
            Hazard::DetachTmux,
            Mux::Attached,
            TerminalKind::Ghostty,
            Platform::MacOs,
        ),
        (Hazard::RevokeApi, Mux::None, TerminalKind::Kitty, X11),
        (Hazard::RestartTerminal, Mux::None, TerminalKind::Kitty, X11),
        (Hazard::ReuseWindow, Mux::None, TerminalKind::Alacritty, X11),
        (
            Hazard::ReuseWindow,
            Mux::None,
            TerminalKind::ConsoleHost,
            Platform::Windows,
        ),
        (
            Hazard::RevokeAutomation,
            Mux::None,
            TerminalKind::Ghostty,
            Platform::MacOs,
        ),
    ];
    for (hazard, mux, terminal, platform) in cases {
        let mut w = world(platform, vars());
        let provider = if platform == Platform::Windows {
            Provider::Claude
        } else {
            Provider::Grok
        };
        let s = start(&mut w, provider, terminal, mux, false);
        w.race = Some(hazard);
        let id = w.log.len() as u64 + 1000;
        w.engine_open = Some(id);
        let r = engine::open(&mut w, &s, &plain());
        w.engine_open = None;
        oracle::check_open(&w, id, &plain(), &r).expect("oracle");
        assert!(
            matches!(
                r.outcome,
                Outcome::Failed(Reason::TargetGone) | Outcome::Blocked(Reason::PermissionDenied)
            ),
            "{hazard:?}: {:?} {:?}",
            r.outcome,
            r.decision
        );
    }
}
