use super::*;

#[test]
fn windows_console_raises_its_own_window_and_terminal_tabs_are_app_level() {
    for owner_is_client in [false, true] {
        let v = WorldVars {
            console_window_owner_is_client: owner_is_client,
            ..vars()
        };
        let mut w = world(Platform::Windows, v);
        let s = start(
            &mut w,
            Provider::Claude,
            TerminalKind::ConsoleHost,
            Mux::None,
            false,
        );
        w.front_app = None;
        let r = open(&mut w, &s, &plain());
        assert!(
            matches!(focus_ref(&r), Some(PaneRef::WindowsWindow { .. })),
            "{:?}",
            r.decision
        );
        assert_eq!(
            r.decision.action().map(|a| a.precision),
            Some(Precision::RightWindow)
        );
        assert_eq!(r.outcome, Outcome::Verified);
    }
    let mut w = world(Platform::Windows, vars());
    let s = start(
        &mut w,
        Provider::Codex,
        TerminalKind::WindowsTerminal,
        Mux::None,
        false,
    );
    let r = open(&mut w, &s, &plain());
    if let Some(a) = r
        .decision
        .action()
        .filter(|a| a.kind == ActionKind::FocusPane)
    {
        assert_eq!(
            a.precision,
            Precision::AppOnly,
            "a WT session GUID selects no tab"
        );
    }
}

#[test]
fn several_windows_terminal_windows_need_a_choice_outside_relay() {
    let mut w = world(Platform::Windows, vars());
    let s = start(
        &mut w,
        Provider::Claude,
        TerminalKind::WindowsTerminal,
        Mux::None,
        false,
    );
    let dir = w.dir_at(1);
    while w
        .panes
        .values()
        .map(|p| p.window)
        .collect::<BTreeSet<_>>()
        .len()
        < 2
    {
        w.open_pane(TerminalKind::WindowsTerminal, &dir);
    }
    let r = open(&mut w, &s, &plain());
    assert_eq!(r.decision.plan, Plan::Unavailable(Reason::WindowAmbiguous));
}

#[test]
fn windows_refusing_the_foreground_fails_honestly() {
    let mut w = world(
        Platform::Windows,
        WorldVars {
            windows_foreground_allowed: false,
            ..vars()
        },
    );
    let s = start(
        &mut w,
        Provider::Claude,
        TerminalKind::ConsoleHost,
        Mux::None,
        false,
    );
    let r = open(&mut w, &s, &plain());
    assert_eq!(r.outcome, Outcome::Failed(Reason::DispatchFailed));
}

#[test]
fn grok_is_unavailable_on_windows() {
    let mut w = world(Platform::Windows, vars());
    let s = SessionRef::new(Provider::Grok, "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee");
    let r = open(&mut w, &s, &plain());
    assert_eq!(
        r.decision.plan,
        Plan::Unavailable(Reason::PlatformUnsupported)
    );
}
