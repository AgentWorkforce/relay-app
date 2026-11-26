use super::*;

// ------------------------------------------------- locating, by identity ---

const X11: Platform = Platform::Linux {
    session: Fact::Known(DisplayServer::X11),
};

fn support(terminal: TerminalKind, focus: bool, raise: bool) -> TerminalSupport {
    let no = Ability::Unsupported(Unsupported::NotImplemented);
    TerminalSupport {
        terminal,
        focus_pane: if focus { Ability::Available } else { no },
        raise_window: if raise { Ability::Available } else { no },
        activate_app: no,
        launch: Ability::Available,
        tabs_share_window: false,
    }
}

/// launchd → `terminal_exe` (811) → shell (2005) → Grok agent (5) on `tty`.
fn grok_in(terminal_exe: Exe, tty: &str) -> Snapshot {
    let table = vec![
        launchd(),
        proc_(811, 1, terminal_exe, None, &["term"]),
        proc_(2005, 811, Exe::Shell, Some(tty), &["bash"]),
        proc_(
            5,
            2005,
            Exe::GrokCli,
            Some(tty),
            &["grok", "--resume", GROK_ID],
        ),
    ];
    let mut s = snap(
        Provider::Grok,
        GROK_ID,
        table,
        ProviderEvidence::Grok(GrokEvidence {
            active: Fact::Known(vec![]),
            session: Fact::Known(Some(GrokSession {
                cwd: DIR.into(),
                subagent: false,
            })),
        }),
    );
    s.platform = X11;
    s
}

#[test]
fn two_api_panes_on_one_tty_are_not_a_pane() {
    let mut s = grok_in(Exe::Kitty, "/dev/pts/3");
    s.surfaces.terminals = Fact::Known(vec![support(TerminalKind::Kitty, true, false)]);
    let pane = |id| PaneInfo {
        terminal: TerminalKind::Kitty,
        pane: PaneRef::KittyWindow {
            socket: "unix:/tmp/kitty-811".into(),
            id,
        },
        tty: Some("/dev/pts/3".into()),
        pid: None,
    };
    s.surfaces.panes = Fact::Known(vec![pane(1)]);
    let d = decide(&s, &req());
    assert!(
        matches!(
            d.action().and_then(|a| a.steps.first()),
            Some(Step::FocusPane {
                pane: PaneRef::KittyWindow { id: 1, .. },
                ..
            })
        ),
        "{d:?}"
    );
    s.surfaces.panes = Fact::Known(vec![pane(1), pane(2)]);
    assert!(matches!(decide(&s, &req()).plan, Plan::Unavailable(_)));
}

#[test]
fn a_window_is_raised_only_when_its_own_terminal_owns_it() {
    let mut s = grok_in(Exe::GnomeTerminal, "/dev/pts/4");
    s.surfaces.terminals = Fact::Known(vec![support(TerminalKind::GnomeTerminal, false, true)]);
    let own = WindowInfo {
        id: WindowId(0x10),
        owner: ident(811),
    };
    let foreign = WindowInfo {
        id: WindowId(0x20),
        owner: ident(999),
    };
    s.surfaces.windows = Fact::Known(vec![foreign, own]);
    let d = decide(&s, &req());
    assert!(
        matches!(
            d.action().and_then(|a| a.steps.first()),
            Some(Step::FocusPane {
                pane: PaneRef::X11Window {
                    id: WindowId(0x10),
                    ..
                },
                ..
            })
        ),
        "{d:?}"
    );
    // A window that claims the right id but another owner is not it.
    s.processes
        .known_mut()
        .into_iter()
        .flatten()
        .for_each(|p| p.window = Some(WindowId(0x20)));
    let d = decide(&s, &req());
    assert!(
        matches!(
            d.action().and_then(|a| a.steps.first()),
            Some(Step::FocusPane {
                pane: PaneRef::X11Window {
                    id: WindowId(0x10),
                    ..
                },
                ..
            })
        ),
        "{d:?}"
    );
}

#[test]
fn a_tmux_pane_is_found_on_its_own_server_by_its_tty() {
    let tty = "/dev/pts/9";
    let table = vec![
        launchd(),
        proc_(50, 1, Exe::Tmux, None, &["tmux"]),
        proc_(
            5,
            50,
            Exe::GrokCli,
            Some(tty),
            &["grok", "--resume", GROK_ID],
        ),
        proc_(811, 1, Exe::Kitty, None, &["kitty"]),
        proc_(2001, 811, Exe::Shell, Some("/dev/pts/1"), &["bash"]),
        proc_(62, 2001, Exe::Tmux, Some("/dev/pts/1"), &["tmux", "attach"]),
    ];
    let mut s = grok_in(Exe::Kitty, "/dev/pts/3");
    s.processes = Fact::Known(table);
    s.surfaces.terminals = Fact::Known(vec![support(TerminalKind::Kitty, true, false)]);
    s.surfaces.panes = Fact::Known(vec![PaneInfo {
        terminal: TerminalKind::Kitty,
        pane: PaneRef::KittyWindow {
            socket: "unix:/tmp/kitty-811".into(),
            id: 7,
        },
        tty: Some("/dev/pts/1".into()),
        pid: Some(2001),
    }]);
    let pane = |pane: &str, tty: &str| TmuxPaneInfo {
        pane: pane.into(),
        tty: tty.into(),
        session: "$1".into(),
        window: "@1".into(),
    };
    let client = TmuxClientInfo {
        tty: "/dev/pts/1".into(),
        pid: 62,
        session: "$1".into(),
        active_pane: "%2".into(),
    };
    // A stale listing of another server (pane ids recycle) comes first.
    let stale = TmuxServer {
        socket: "/tmp/tmux-501/old".into(),
        server: ident(49),
        panes: vec![pane("%0", tty)],
        clients: vec![client.clone()],
    };
    let live = TmuxServer {
        socket: "/tmp/tmux-501/default".into(),
        server: ident(50),
        panes: vec![pane("%1", "/dev/pts/8"), pane("%2", tty)],
        clients: vec![client],
    };
    s.surfaces.tmux = Fact::Known(vec![stale, live]);
    let d = decide(&s, &req());
    let Some(Step::FocusPane {
        pane:
            PaneRef::TmuxPane {
                socket,
                server,
                pane,
                outer,
                ..
            },
        ..
    }) = d.action().and_then(|a| a.steps.first())
    else {
        panic!("{d:?}");
    };
    assert_eq!(socket, "/tmp/tmux-501/default");
    assert_eq!(*server, ident(50));
    assert_eq!(pane, "%2");
    assert!(matches!(**outer, PaneRef::KittyWindow { id: 7, .. }));
}

#[test]
fn an_attach_is_verified_only_while_the_pane_still_holds_the_process() {
    use session_navigation::engine::verify::check;
    let mut s = grok_in(Exe::Ghostty, "/dev/ttys009");
    s.platform = Platform::MacOs;
    s.front.app = Fact::Known(Some(App::Ghostty));
    s.front.pane_tty = Fact::Known(Some("/dev/ttys001".into()));
    let listing = |tty: &str| {
        Fact::Known(vec![TmuxServer {
            socket: "/tmp/tmux-501/default".into(),
            server: ident(50),
            panes: vec![TmuxPaneInfo {
                pane: "%2".into(),
                tty: tty.into(),
                session: "$1".into(),
                window: "@1".into(),
            }],
            clients: vec![TmuxClientInfo {
                tty: "/dev/ttys001".into(),
                pid: 62,
                session: "$1".into(),
                active_pane: "%2".into(),
            }],
        }])
    };
    let post = PostCondition::TmuxAttached {
        terminal: TerminalKind::Ghostty,
        socket: "/tmp/tmux-501/default".into(),
        server: ident(50),
        pane: "%2".into(),
        process: ident(5),
    };
    s.surfaces.tmux = listing("/dev/ttys009");
    assert_eq!(check(&post, &s, &req()), Outcome::Verified);
    // The server restarted and `%2` is another pane now.
    s.surfaces.tmux = listing("/dev/ttys010");
    assert_eq!(check(&post, &s, &req()), Outcome::DispatchedUnverified);
}

#[test]
fn a_raised_window_is_verified_only_with_its_owner() {
    use session_navigation::engine::verify::check;
    let mut s = grok_in(Exe::Alacritty, "/dev/pts/3");
    s.front.app = Fact::Known(Some(App::Alacritty));
    s.front.window = Fact::Known(Some(WindowId(0x10)));
    let post = PostCondition::PaneFocused {
        terminal: TerminalKind::Alacritty,
        pane: PaneRef::X11Window {
            id: WindowId(0x10),
            owner: ident(811),
        },
        process: ident(5),
    };
    s.surfaces.windows = Fact::Known(vec![WindowInfo {
        id: WindowId(0x10),
        owner: ident(811),
    }]);
    assert_eq!(check(&post, &s, &req()), Outcome::Verified);
    s.surfaces.windows = Fact::Known(vec![WindowInfo {
        id: WindowId(0x10),
        owner: ident(999),
    }]);
    assert_eq!(
        check(&post, &s, &req()),
        Outcome::Failed(Reason::WrongDestination)
    );
}

#[test]
fn without_ttys_an_unhosted_process_is_never_headless() {
    let table = vec![
        launchd(),
        proc_(300, 1, Exe::Other, None, &["svchost.exe"]),
        proc_(5, 300, Exe::GrokCli, None, &["grok", "--resume", GROK_ID]),
    ];
    let mut s = grok_in(Exe::Kitty, "/dev/pts/3");
    s.processes = Fact::Known(table.clone());
    assert_eq!(
        decide(&s, &req()).state,
        SessionState::LiveHeadless,
        "with ttys"
    );
    // The same shape on Windows, where no process has a tty.
    let mut s = claude_stopped();
    s.platform = Platform::Windows;
    s.processes = Fact::Known(vec![
        launchd(),
        proc_(300, 1, Exe::Other, None, &["svchost.exe"]),
        proc_(5, 300, Exe::ClaudeCli, None, &["claude"]),
    ]);
    if let ProviderEvidence::Claude(ev) = &mut s.provider {
        ev.registry = Fact::Known(vec![registry(5, CLAUDE_ID, None)]);
    }
    let take_over = OpenRequest {
        take_over: true,
        ..req()
    };
    let d = decide(&s, &take_over);
    assert_ne!(d.state, SessionState::LiveHeadless, "{d:?}");
    assert!(d
        .action()
        .is_none_or(|a| !a.steps.iter().any(|s| matches!(s, Step::Stop { .. }))));
}
