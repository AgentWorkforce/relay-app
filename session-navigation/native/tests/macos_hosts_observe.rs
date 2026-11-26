//! Host surfaces on a fake Mac: Kitty and WezTerm panes, tmux servers,
//! front panes, and `TerminalSupport` as observed. Every response is a
//! documentation-derived fixture (`fixtures/macos/PROVENANCE.txt`); the real
//! tools were never run.
#![cfg(feature = "macos")]
mod common;
mod macos_world;

use common::*;
use macos_world::*;
use session_navigation::evidence::{Fact, Snapshot};
use session_navigation::hosts::{
    Ability, PaneInfo, PaneRef, Permission, Setting, TerminalSupport, TmuxClientInfo, TmuxPaneInfo,
    TmuxServer,
};
use session_navigation::model::{App, TerminalKind};
use session_navigation::ports::Observe;
use session_navigation_native::common::{kitty, tmux, wezterm};
use session_navigation_native::macos::{machine, scripts};
use session_navigation_native::system::{Call, FakeSystem, Output, RunError};
use session_navigation_native::MacObserver;

fn observe(fake: &FakeSystem) -> Snapshot {
    MacObserver::new(fake, cfg()).observe(&codex())
}

fn support(snap: &Snapshot, terminal: TerminalKind) -> TerminalSupport {
    snap.surfaces
        .support(terminal)
        .known()
        .copied()
        .unwrap_or_else(|| TerminalSupport::none(terminal))
}

fn kitty_pane(id: u64, pid: u32) -> PaneInfo {
    PaneInfo {
        terminal: TerminalKind::Kitty,
        pane: PaneRef::KittyWindow {
            socket: KITTY_ADDR.into(),
            id,
        },
        tty: None,
        pid: Some(pid),
    }
}

fn wez_pane(id: u64, tty: &str) -> PaneInfo {
    PaneInfo {
        terminal: TerminalKind::WezTerm,
        pane: PaneRef::WezTermPane {
            socket: WEZ_SOCK.into(),
            id,
        },
        tty: Some(tty.into()),
        pid: None,
    }
}

#[test]
fn lists_kitty_and_wezterm_panes_and_tmux_servers() {
    let fake = host_machine().build();
    let snap = observe(&fake);
    assert_eq!(
        snap.surfaces.panes,
        Fact::Known(vec![
            kitty_pane(1, 2001),
            kitty_pane(2, 2010),
            wez_pane(0, "/dev/ttys006"),
            wez_pane(1, "/dev/ttys009"),
        ])
    );
    let pane = |pane: &str, tty: &str, session: &str, window: &str| TmuxPaneInfo {
        pane: pane.into(),
        tty: tty.into(),
        session: session.into(),
        window: window.into(),
    };
    assert_eq!(
        snap.surfaces.tmux,
        Fact::Known(vec![TmuxServer {
            socket: TMUX_SOCK.into(),
            server: ident(4000),
            panes: vec![
                pane("%0", "/dev/ttys007", "$0", "@0"),
                pane("%1", "/dev/ttys008", "$0", "@1"),
                pane("%2", "/dev/ttys010", "$1", "@2"),
            ],
            clients: vec![TmuxClientInfo {
                tty: "/dev/ttys000".into(),
                pid: 4001,
                session: "$0".into(),
                active_pane: "%0".into(),
            }],
        }])
    );
    assert_eq!(
        support(&snap, TerminalKind::Kitty).focus_pane,
        Ability::Available
    );
    assert_eq!(
        support(&snap, TerminalKind::WezTerm).focus_pane,
        Ability::Available
    );
    assert_eq!(snap.surfaces.windows, Fact::Known(vec![]));
    // WezTerm is always asked on its own socket and never auto-starts.
    let wez_calls: Vec<_> = fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Run {
                program, args, env, ..
            } if program == WEZ => Some((args, env)),
            _ => None,
        })
        .collect();
    assert_eq!(wez_calls.len(), 2);
    for (args, env) in wez_calls {
        assert_eq!(args.get(1).map(String::as_str), Some("--no-auto-start"));
        assert_eq!(
            env,
            vec![("WEZTERM_UNIX_SOCKET".to_owned(), WEZ_SOCK.to_owned())]
        );
    }
}

#[test]
fn front_panes_come_from_the_apis() {
    let mut m = host_machine();
    m.apps_json = apps_with_terminals(Some(App::Kitty), &[App::Kitty]);
    let snap = observe(&m.build());
    assert_eq!(
        snap.front.pane,
        Fact::Known(Some(PaneRef::KittyWindow {
            socket: KITTY_ADDR.into(),
            id: 2
        }))
    );
    // Kitty's `ls` has no tty.
    assert_eq!(snap.front.pane_tty, Fact::Unknown);

    m.apps_json = apps_with_terminals(Some(App::WezTerm), &[App::WezTerm]);
    let snap = observe(&m.build());
    assert_eq!(
        snap.front.pane,
        Fact::Known(Some(PaneRef::WezTermPane {
            socket: WEZ_SOCK.into(),
            id: 0
        }))
    );
    assert_eq!(
        snap.front.pane_tty,
        Fact::Known(Some("/dev/ttys006".into()))
    );

    // A non-API terminal in front: no API pane applies.
    m.apps_json = apps_with_terminals(Some(App::Alacritty), &[App::Alacritty]);
    let snap = observe(&m.build());
    assert_eq!(
        (snap.front.pane, snap.front.pane_tty),
        (Fact::Known(None), Fact::Unknown)
    );
}

#[test]
fn kitty_without_a_reachable_api_needs_its_setting() {
    let off = Ability::NeedsSetting(Setting::KittyRemoteControl);
    // Running, no socket: remote control is not on.
    let mut m = Machine::new();
    m.procs.extend(kitty_procs());
    m.apps_json = apps_with_terminals(Some(App::Kitty), &[App::Kitty]);
    let snap = observe(&m.build());
    assert_eq!(support(&snap, TerminalKind::Kitty).focus_pane, off);
    assert_eq!(snap.surfaces.panes, Fact::Known(vec![]));
    assert_eq!(snap.front.pane, Fact::Unknown, "API off: focus unreadable");

    // A socket whose Kitty refuses remote control, or a stale one.
    m.file(KITTY_SOCK, Vec::new());
    for stderr in [KITTY_DISABLED, KITTY_GONE] {
        let mut m2 = Machine {
            runs: Vec::new(),
            ..clone_machine(&m)
        };
        m2.run(
            kitty_run(kitty::ls_args(KITTY_ADDR)),
            Ok(Output::exit(1, "", stderr)),
        );
        let snap = observe(&m2.build());
        assert_eq!(
            support(&snap, TerminalKind::Kitty).focus_pane,
            off,
            "{stderr}"
        );
        assert_eq!(snap.surfaces.panes, Fact::Known(vec![]));
    }

    // Anything else is not "no panes".
    for response in [Err(RunError::TimedOut), Ok(Output::ok("not json"))] {
        let mut m2 = clone_machine(&m);
        m2.run(kitty_run(kitty::ls_args(KITTY_ADDR)), response);
        assert_eq!(observe(&m2.build()).surfaces.panes, Fact::Unknown);
    }

    // A socket of a pid that is not Kitty is never asked.
    let mut m2 = Machine::new();
    m2.procs.extend(kitty_procs());
    m2.file("/private/tmp/kitty-998", Vec::new());
    let fake = m2.build();
    observe(&fake);
    assert!(fake.runs().iter().all(|(p, _)| p != KITTY));

    // Not running at all: nothing listed, nothing run.
    let fake = Machine::new().build();
    let snap = observe(&fake);
    assert_eq!(support(&snap, TerminalKind::Kitty).focus_pane, off);
    assert_eq!(snap.surfaces.panes, Fact::Known(vec![]));
}

fn clone_machine(m: &Machine) -> Machine {
    Machine {
        procs: m.procs.clone(),
        apps_json: m.apps_json.clone(),
        front_tty: m.front_tty.clone(),
        tools: m.tools.clone(),
        files: m.files.clone(),
        dirs: m.dirs.clone(),
        runs: m.runs.clone(),
        ps_broken: m.ps_broken,
    }
}

#[test]
fn wezterm_stale_or_failing_socket() {
    let off = Ability::NeedsSetting(Setting::WezTermMuxSocket);
    let mut m = Machine::new();
    m.procs.extend(wezterm_procs());
    m.apps_json = apps_with_terminals(None, &[App::WezTerm]);
    let snap = observe(&m.build());
    assert_eq!(support(&snap, TerminalKind::WezTerm).focus_pane, off);

    m.file(WEZ_SOCK, Vec::new());
    let mut stale = clone_machine(&m);
    stale.run(
        wez_run(wezterm::list_args()),
        Ok(Output::exit(1, "", WEZ_GONE)),
    );
    let snap = observe(&stale.build());
    assert_eq!(
        (
            support(&snap, TerminalKind::WezTerm).focus_pane,
            snap.surfaces.panes
        ),
        (off, Fact::Known(vec![]))
    );

    let mut failing = clone_machine(&m);
    failing.run(
        wez_run(wezterm::list_args()),
        Ok(Output::exit(1, "", "Error: boom")),
    );
    assert_eq!(observe(&failing.build()).surfaces.panes, Fact::Unknown);

    // No installed app to find the CLI in: the panes cannot be read.
    let mut no_app = clone_machine(&m);
    no_app.apps_json = apps_json(None, &[]);
    assert_eq!(observe(&no_app.build()).surfaces.panes, Fact::Unknown);

    // Listing works but list-clients fails: panes stand, front unknown.
    let mut no_clients = clone_machine(&m);
    no_clients.apps_json = apps_with_terminals(Some(App::WezTerm), &[App::WezTerm]);
    no_clients.run(wez_run(wezterm::list_args()), Ok(Output::ok(WEZ_LIST)));
    let snap = observe(&no_clients.build());
    assert!(matches!(&snap.surfaces.panes, Fact::Known(p) if p.len() == 2));
    assert_eq!(snap.front.pane, Fact::Unknown);
}

#[test]
fn tmux_servers_are_unknown_unless_fully_read() {
    // No socket directory: no server.
    assert_eq!(
        observe(&Machine::new().build()).surfaces.tmux,
        Fact::Known(vec![])
    );

    let base = || {
        let mut m = Machine::new();
        m.procs.extend(tmux_procs());
        m.file(TMUX_SOCK, Vec::new());
        m.tools.push((TMUX.into(), "tmux 3.5a\n".into()));
        m
    };
    // A stale socket (server gone) is skipped.
    let mut m = base();
    m.run(
        tmux_run(tmux::server_pid_args(TMUX_SOCK)),
        Ok(Output::exit(1, "", TMUX_GONE)),
    );
    assert_eq!(observe(&m.build()).surfaces.tmux, Fact::Known(vec![]));

    // A socket with no tmux to ask, a server pid not in the table, or a
    // malformed listing: unknown.
    let mut no_tool = base();
    no_tool.tools.retain(|(p, _)| p != TMUX);
    assert_eq!(observe(&no_tool.build()).surfaces.tmux, Fact::Unknown);

    let mut wrong_pid = base();
    wrong_pid.run(
        tmux_run(tmux::server_pid_args(TMUX_SOCK)),
        Ok(Output::ok("7777\n")),
    );
    assert_eq!(observe(&wrong_pid.build()).surfaces.tmux, Fact::Unknown);

    let mut bad = base();
    bad.run(
        tmux_run(tmux::server_pid_args(TMUX_SOCK)),
        Ok(Output::ok(TMUX_PID)),
    );
    bad.run(
        tmux_run(tmux::list_panes_args(TMUX_SOCK)),
        Ok(Output::ok("%0 /dev/ttys007\n")),
    );
    assert_eq!(observe(&bad.build()).surfaces.tmux, Fact::Unknown);

    let mut unreadable = base();
    let fake = unreadable.dir("/private/tmp/tmux-501").build();
    fake.make_unreadable("/private/tmp/tmux-501");
    assert_eq!(observe(&fake).surfaces.tmux, Fact::Unknown);
}

#[test]
fn iterm_front_tty_and_automation_refusals() {
    let mut m = Machine::new();
    m.apps_json = apps_with_terminals(Some(App::ITerm2), &[App::ITerm2]);
    let front = machine::script_run(
        scripts::ITERM_FRONT_TTY,
        &[],
        cfg().common.timeouts.script_read,
    );
    m.run(front.clone(), Ok(Output::ok("/dev/ttys011\n")));
    let snap = observe(&m.build());
    assert_eq!(
        snap.front.pane_tty,
        Fact::Known(Some("/dev/ttys011".into()))
    );
    assert_eq!(snap.front.pane, Fact::Known(None));
    assert_eq!(
        support(&snap, TerminalKind::ITerm2),
        TerminalSupport::scripted(TerminalKind::ITerm2)
    );

    let refused = "execution error: Not authorized to send Apple events to iTerm2. (-1743)\n";
    m.run(front, Ok(Output::exit(1, "", refused)));
    let snap = observe(&m.build());
    let ask = Ability::NeedsPermission(Permission::MacAutomation);
    let iterm = support(&snap, TerminalKind::ITerm2);
    assert_eq!(
        (iterm.focus_pane, iterm.launch, iterm.activate_app),
        (ask, ask, Ability::Available)
    );
    assert_eq!(snap.front.pane_tty, Fact::Unknown);
    // Only the refused terminal changes.
    assert_eq!(
        support(&snap, TerminalKind::Ghostty),
        TerminalSupport::scripted(TerminalKind::Ghostty)
    );

    // The same for Ghostty.
    let mut g = Machine::new();
    g.apps_json = apps_json(Some(App::Ghostty), &[App::Ghostty]);
    g.front_tty = Some(Ok(Output::exit(1, "", refused)));
    let snap = observe(&g.build());
    assert_eq!(support(&snap, TerminalKind::Ghostty).focus_pane, ask);
}

#[test]
fn unknown_registry_leaves_front_panes_unknown() {
    let mut m = host_machine();
    m.run(machine::apps_run(&cfg()), Err(RunError::TimedOut));
    let snap = observe(&m.build());
    assert_eq!(
        (snap.front.app, snap.front.pane),
        (Fact::Unknown, Fact::Unknown)
    );
    // The CLIs live in the app bundles, so their panes are unknown too;
    // tmux does not depend on the registry.
    assert_eq!(snap.surfaces.panes, Fact::Unknown);
    assert!(snap.surfaces.tmux.is_known());
}
