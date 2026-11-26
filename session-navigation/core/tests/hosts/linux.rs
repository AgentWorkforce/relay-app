use super::*;

#[test]
fn kitty_with_remote_control_focuses_the_exact_window() {
    for platform in [X11, WAYLAND, Platform::MacOs] {
        let mut w = world(platform, vars());
        let s = start(
            &mut w,
            Provider::Codex,
            TerminalKind::Kitty,
            Mux::None,
            false,
        );
        w.front_app = None;
        let r = open(&mut w, &s, &plain());
        // A plain `codex` TUI is not attributable (N9): the daemon serves it.
        assert!(
            matches!(r.decision.plan, Plan::Act(_)),
            "{platform:?} {:?}",
            r.decision
        );
        let s = start(
            &mut w,
            Provider::Grok,
            TerminalKind::Kitty,
            Mux::None,
            false,
        );
        w.front_app = None;
        let r = open(&mut w, &s, &plain());
        assert!(
            matches!(focus_ref(&r), Some(PaneRef::KittyWindow { .. })),
            "{:?}",
            r.decision
        );
        assert_eq!(r.outcome, Outcome::Verified, "{platform:?}");
    }
}

#[test]
fn kitty_without_remote_control_raises_its_window_at_app_precision_on_x11() {
    let mut w = world(X11, vars());
    w.hosts.api.insert(TerminalKind::Kitty, false);
    let s = start(
        &mut w,
        Provider::Grok,
        TerminalKind::Kitty,
        Mux::None,
        false,
    );
    let r = open(&mut w, &s, &plain());
    let a = r.decision.action().expect("action");
    assert!(
        matches!(focus_ref(&r), Some(PaneRef::X11Window { .. })),
        "{:?}",
        r.decision
    );
    assert_eq!(a.precision, Precision::AppOnly, "Kitty has tabs");
    assert_eq!(a.note, Some(Reason::PaneSelectionUnavailable));
    assert_eq!(r.outcome, Outcome::Verified);
}

#[test]
fn kitty_without_remote_control_on_wayland_is_refused_with_its_setting() {
    let mut w = world(WAYLAND, vars());
    w.hosts.api.insert(TerminalKind::Kitty, false);
    let s = start(
        &mut w,
        Provider::Grok,
        TerminalKind::Kitty,
        Mux::None,
        false,
    );
    let r = open(&mut w, &s, &plain());
    assert_eq!(r.decision.plan, Plan::Unavailable(Reason::WaylandNoRaise));
    assert!(r.executed.is_empty());
}

#[test]
fn alacritty_and_xterm_raise_their_declared_window_on_x11() {
    for terminal in [
        TerminalKind::Alacritty,
        TerminalKind::Xterm,
        TerminalKind::Konsole,
    ] {
        let mut w = world(X11, vars());
        let s = start(&mut w, Provider::Grok, terminal, Mux::None, false);
        let _other = start(&mut w, Provider::Grok, terminal, Mux::None, false);
        let r = open(&mut w, &s, &plain());
        assert!(
            matches!(focus_ref(&r), Some(PaneRef::X11Window { .. })),
            "{terminal:?} {:?}",
            r.decision
        );
        assert_ne!(
            r.outcome,
            Outcome::Failed(Reason::WrongDestination),
            "{terminal:?}"
        );
    }
}

#[test]
fn gnome_terminal_with_several_unidentified_windows_is_ambiguous() {
    let mut w = world(X11, vars());
    let s = start(
        &mut w,
        Provider::Grok,
        TerminalKind::GnomeTerminal,
        Mux::None,
        false,
    );
    // New panes sometimes open as tabs; open until a second window exists.
    while w
        .panes
        .values()
        .map(|p| p.window)
        .collect::<BTreeSet<_>>()
        .len()
        < 2
    {
        w.open_pane(TerminalKind::GnomeTerminal, "/");
    }
    let r = open(&mut w, &s, &plain());
    assert_eq!(r.decision.plan, Plan::Unavailable(Reason::WindowAmbiguous));
}

#[test]
fn wayland_terminal_without_a_pane_api_says_why() {
    let mut w = world(WAYLAND, vars());
    let s = start(
        &mut w,
        Provider::Grok,
        TerminalKind::GnomeTerminal,
        Mux::None,
        false,
    );
    let r = open(&mut w, &s, &plain());
    assert_eq!(r.decision.plan, Plan::Unavailable(Reason::WaylandNoRaise));
}

#[test]
fn x11_activation_refused_by_the_window_manager_is_never_verified() {
    let mut w = world(
        X11,
        WorldVars {
            x11_activation_honored: false,
            ..vars()
        },
    );
    let s = start(
        &mut w,
        Provider::Claude,
        TerminalKind::Alacritty,
        Mux::None,
        false,
    );
    w.front_app = None;
    let r = open(&mut w, &s, &plain());
    assert!(matches!(focus_ref(&r), Some(PaneRef::X11Window { .. })));
    assert_ne!(r.outcome, Outcome::Verified);
}
