//! `LinuxObserver` snapshot fields on fake Linux machines.
#![cfg(feature = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod linux_common;

use linux_common::scenes::*;
use linux_common::*;
use session_navigation::evidence::{Exe, Fact, ProcessInfo, Snapshot};
use session_navigation::hosts::{
    Ability, PaneRef, Setting, TerminalSupport, Unsupported, WindowId,
};
use session_navigation::model::{App, DisplayServer, Platform, TerminalKind, Tool};
use session_navigation::ports::Observe;
use session_navigation_native::common::wezterm;
use session_navigation_native::linux::{BootClock, LinuxObserver};
use session_navigation_native::system::FakeSystem;

fn observe(fake: &FakeSystem, display: Option<&str>) -> Snapshot {
    LinuxObserver::new(fake, cfg(display)).observe(&grok())
}

fn row(snap: &Snapshot, pid: u32) -> ProcessInfo {
    snap.processes
        .known()
        .unwrap()
        .iter()
        .find(|p| p.identity.pid == pid)
        .cloned()
        .unwrap_or_else(|| panic!("no pid {pid}"))
}

fn support(snap: &Snapshot, terminal: TerminalKind) -> TerminalSupport {
    snap.surfaces.support(terminal).known().copied().unwrap()
}

fn kitty_x11() -> FakeSystem {
    let fake = kitty_machine(Some("x11")).build();
    grok_files(&fake, &[(2601, OPENED_AT)]);
    script_x11(&fake, Some("x11"), &[KITTY_WINDOW]);
    fake.on(&kitty_ls_run(), vec![ok(fixture("kitty-ls.json"))]);
    fake
}

#[test]
fn x11_snapshot_with_a_kitty_pane() {
    let fake = kitty_x11();
    let snap = observe(&fake, Some("x11"));
    assert_eq!(
        snap.platform,
        Platform::Linux {
            session: Fact::Known(DisplayServer::X11)
        }
    );
    assert_eq!(snap.boot, Fact::Known(boot()));
    let agent = row(&snap, 2601);
    assert_eq!(agent.identity, identity(2601, 2601));
    assert_eq!((agent.ppid, agent.ruid, agent.euid), (2481, UID, UID));
    assert_eq!(agent.exe, Exe::GrokCli);
    assert_eq!(agent.tty.as_deref(), Some("/dev/pts/1"));
    assert_eq!(agent.window, Some(WindowId(KITTY_WINDOW)));
    assert_eq!(agent.cwd, Fact::Known(PROJECT.into()));
    assert_eq!(agent.argv, vec!["grok"]);
    // Only agent CLIs get a cwd; root's processes are classified by comm.
    assert_eq!(row(&snap, 2481).cwd, Fact::Unknown);
    assert_eq!(row(&snap, 2481).exe, Exe::Shell);
    assert_eq!(row(&snap, 2470).exe, Exe::Kitty);
    assert_eq!((row(&snap, 1).exe, row(&snap, 1).ruid), (Exe::Other, 0));
    assert_eq!(row(&snap, 2).tty, None);
    // The rest of the environment never reaches the snapshot.
    assert!(!format!("{snap:?}").contains("do-not-read"));

    let apps = snap.apps.known().unwrap();
    let app = |a: App| apps.iter().find(|r| r.app == a).cloned().unwrap();
    assert!(app(App::Kitty).installed && app(App::Kitty).running);
    assert!(app(App::Konsole).installed && !app(App::Konsole).running);
    assert!(!app(App::Ghostty).installed);
    assert_eq!(apps.len(), 7);
    assert_eq!(snap.schemes, Fact::Known(vec![]));
    let grok_tool = snap.tools.iter().find(|t| t.tool == Tool::GrokCli).unwrap();
    assert_eq!(grok_tool.version, Fact::Known(Some("0.1.23".into())));
    assert_eq!(grok_tool.path.as_deref(), Some(GROK_BIN));
    assert_eq!(snap.cwd, Fact::Known(PROJECT.into()));
    assert_eq!(snap.cwd_exists, Fact::Known(true));
    assert_eq!(snap.terminal_profile, Fact::Known(None));
    assert_eq!(snap.reservation, Fact::Known(None));

    let kitty = support(&snap, TerminalKind::Kitty);
    assert_eq!(kitty.focus_pane, Ability::Available);
    assert_eq!(kitty.raise_window, Ability::Available);
    assert_eq!(
        kitty.activate_app,
        Ability::Unsupported(Unsupported::NotImplemented)
    );
    assert_eq!(kitty.launch, Ability::Available);
    assert_eq!(
        support(&snap, TerminalKind::WezTerm).focus_pane,
        Ability::NeedsSetting(Setting::WezTermMuxSocket)
    );
    let xterm = support(&snap, TerminalKind::Xterm);
    assert_eq!(
        xterm.focus_pane,
        Ability::Unsupported(Unsupported::NoPaneApi)
    );
    assert!(!xterm.tabs_share_window);
    assert_eq!(
        support(&snap, TerminalKind::Ghostty).launch,
        Ability::Unsupported(Unsupported::NotImplemented)
    );
    assert_eq!(snap.surfaces.terminals.known().unwrap().len(), 7);

    let panes = snap.surfaces.panes.known().unwrap();
    assert_eq!(panes.len(), 2);
    assert_eq!(panes[0].pid, Some(2481));
    let windows = snap.surfaces.windows.known().unwrap();
    // Firefox's pid and the xterm's are not in this table.
    let ids: Vec<u64> = windows.iter().map(|w| w.id.0).collect();
    assert_eq!(ids, vec![0x01e0_0003, KITTY_WINDOW]);
    assert_eq!(windows[1].owner, identity(2470, 2470));
    assert_eq!(snap.surfaces.tmux, Fact::Known(vec![]));

    assert_eq!(snap.front.app, Fact::Known(Some(App::Kitty)));
    assert_eq!(snap.front.window, Fact::Known(Some(WindowId(KITTY_WINDOW))));
    let focused = PaneRef::KittyWindow {
        socket: KITTY_ADDRESS.into(),
        id: 2,
    };
    assert_eq!(snap.front.pane, Fact::Known(Some(focused)));
    assert_eq!(snap.front.pane_tty, Fact::Known(Some("/dev/pts/2".into())));
}

#[test]
fn wayland_reports_no_windows_and_unknown_front() {
    let fake = kitty_machine(Some("wayland")).build();
    grok_files(&fake, &[(2601, OPENED_AT)]);
    fake.on(&kitty_ls_run(), vec![ok(fixture("kitty-ls-focused.json"))]);
    let snap = observe(&fake, Some("wayland"));
    assert_eq!(
        snap.platform,
        Platform::Linux {
            session: Fact::Known(DisplayServer::Wayland)
        }
    );
    assert_eq!(snap.surfaces.windows, Fact::Known(vec![]));
    assert_eq!(snap.front.window, Fact::Unknown);
    assert_eq!(snap.front.app, Fact::Unknown);
    let xterm = support(&snap, TerminalKind::Xterm);
    assert_eq!(
        xterm.raise_window,
        Ability::Unsupported(Unsupported::Wayland)
    );
    assert_eq!(
        xterm.activate_app,
        Ability::Unsupported(Unsupported::Wayland)
    );
    assert_eq!(xterm.launch, Ability::Available);
    // Kitty still says which of its windows has focus.
    assert_eq!(
        snap.front.pane,
        Fact::Known(Some(PaneRef::KittyWindow {
            socket: KITTY_ADDRESS.into(),
            id: 1
        }))
    );
    assert!(fake.runs().iter().all(|(p, _)| p != XDOTOOL && p != WMCTRL));
}

#[test]
fn no_display_is_unknown_and_nothing_launches() {
    let fake = xterm_machine(None).build();
    grok_files(&fake, &[]);
    let snap = observe(&fake, None);
    assert_eq!(
        snap.platform,
        Platform::Linux {
            session: Fact::Unknown
        }
    );
    assert_eq!(snap.surfaces.windows, Fact::Unknown);
    let xterm = support(&snap, TerminalKind::Xterm);
    assert_eq!(
        xterm.launch,
        Ability::Unsupported(Unsupported::NotImplemented)
    );
    assert_eq!(
        xterm.raise_window,
        Ability::Unsupported(Unsupported::NotImplemented)
    );
}

#[test]
fn an_unreadable_process_makes_the_table_unknown_but_an_exited_one_is_skipped() {
    let m = xterm_machine(Some("x11"));
    let fake = m.build();
    grok_files(&fake, &[]);
    script_x11(&fake, Some("x11"), &[XTERM_WINDOW]);
    // A pid whose files are already gone: it exited.
    fake.add_dir("/proc/5555");
    let snap = observe(&fake, Some("x11"));
    assert!(snap
        .processes
        .known()
        .unwrap()
        .iter()
        .all(|p| p.identity.pid != 5555));

    let fake = m.build();
    grok_files(&fake, &[]);
    fake.make_unreadable("/proc/3110/stat");
    let snap = observe(&fake, Some("x11"));
    assert_eq!(snap.processes, Fact::Unknown);
    assert_eq!(snap.apps, Fact::Unknown);
    assert_eq!(snap.boot, Fact::Known(boot()));
}

#[test]
fn a_deleted_executable_still_classifies() {
    let mut m = xterm_machine(Some("x11"));
    let mut agent = grok_proc(3120, 3110, 3);
    agent.exe = Some(format!("{GROK_REAL} (deleted)"));
    m.push(agent);
    let fake = m.build();
    grok_files(&fake, &[]);
    let snap = observe(&fake, Some("x11"));
    assert_eq!(row(&snap, 3120).exe, Exe::GrokCli);
}

#[test]
fn boot_time_is_read_once_per_boot() {
    let fake = xterm_machine(None).build();
    grok_files(&fake, &[]);
    let clock = BootClock::new();
    let mut observer = LinuxObserver::with_history(
        &fake,
        cfg(None),
        session_navigation_native::NoHistory,
        clock.clone(),
    );
    let first = row(&observer.observe(&grok()), 3120).identity;
    // The wall clock was stepped: btime now reads one second later.
    fake.add_file("/proc/stat", "cpu 1 2 3\nbtime 1762900001\n", 0);
    let second = row(&observer.observe(&grok()), 3120).identity;
    assert_eq!(first, second);
    // A fresh clock would see the moved value.
    let fresh = row(&observe(&fake, None), 3120).identity;
    assert_eq!(fresh.start, first.start + 1);
    // A new boot id reads btime again.
    fake.add_file(
        "/proc/sys/kernel/random/boot_id",
        "11111111-2222-4333-8444-555555555555\n",
        0,
    );
    let rebooted = row(&observer.observe(&grok()), 3120).identity;
    assert_ne!(rebooted.boot, first.boot);
    assert_eq!(rebooted.start, first.start + 1);
}

#[test]
fn kitty_refusing_remote_control_needs_the_setting() {
    let fake = kitty_machine(Some("wayland")).build();
    grok_files(&fake, &[]);
    fake.on(
        &kitty_ls_run(),
        vec![fail(
            "Error: Remote control is disabled. Add allow_remote_control to kitty.conf\n",
        )],
    );
    let snap = observe(&fake, Some("wayland"));
    assert_eq!(
        support(&snap, TerminalKind::Kitty).focus_pane,
        Ability::NeedsSetting(Setting::KittyRemoteControl)
    );
    assert_eq!(snap.surfaces.panes, Fact::Known(vec![]));
    // Any other failure: the pane list is not complete.
    let fake = kitty_machine(Some("wayland")).build();
    grok_files(&fake, &[]);
    fake.on(&kitty_ls_run(), vec![fail("unexpected\n")]);
    assert_eq!(
        observe(&fake, Some("wayland")).surfaces.panes,
        Fact::Unknown
    );
}

#[test]
fn wezterm_panes_and_front_pane() {
    let display = Some("x11");
    let mut m = Machine::new(display);
    m.push(Proc::new(
        4100,
        1600,
        "/usr/bin/wezterm-gui",
        &["wezterm-gui"],
    ));
    m.push(
        Proc::new(4110, 4100, "/usr/bin/bash", &["bash"])
            .tty(4)
            .env("WEZTERM_UNIX_SOCKET", WEZ_SOCKET),
    );
    m.push(grok_proc(4120, 4110, 4).env("WEZTERM_UNIX_SOCKET", WEZ_SOCKET));
    let fake = m.build();
    grok_files(&fake, &[(4120, OPENED_AT)]);
    let window = 0x0500_0002_u64;
    fake.on(
        &wmctrl_run(display),
        vec![ok(format!("{window:#010x}  0 4100 devbox wez\n"))],
    );
    fake.on(&active_run(display), vec![ok(format!("{window}\n"))]);
    let list = |args: Vec<String>, out: &str| fake.on_run_owned(WEZTERM, args, ok(fixture(out)));
    list(wezterm::list_args(), "wezterm-list.json");
    list(wezterm::clients_args(), "wezterm-clients.json");
    let snap = observe(&fake, display);
    let panes = snap.surfaces.panes.known().unwrap();
    assert_eq!(panes.len(), 2);
    assert_eq!(panes[1].tty.as_deref(), Some("/dev/pts/4"));
    assert_eq!(
        support(&snap, TerminalKind::WezTerm).focus_pane,
        Ability::Available
    );
    assert_eq!(snap.front.app, Fact::Known(Some(App::WezTerm)));
    assert_eq!(
        snap.front.pane,
        Fact::Known(Some(PaneRef::WezTermPane {
            socket: WEZ_SOCKET.into(),
            id: 1
        }))
    );
    assert_eq!(snap.front.pane_tty, Fact::Known(Some("/dev/pts/4".into())));
    // Every cli call names the socket in its environment.
    let calls = fake.calls();
    let wez_envs: Vec<_> = calls
        .iter()
        .filter_map(|c| match c {
            session_navigation_native::system::Call::Run { program, env, .. }
                if program == WEZTERM =>
            {
                Some(env.clone())
            }
            _ => None,
        })
        .collect();
    assert!(!wez_envs.is_empty());
    assert!(wez_envs
        .iter()
        .all(|e| e == &vec![("WEZTERM_UNIX_SOCKET".to_owned(), WEZ_SOCKET.to_owned())]));
}

#[test]
fn tmux_servers_and_their_failures() {
    let fake = tmux_machine(true).build();
    grok_files(&fake, &[]);
    script_tmux(&fake);
    script_x11(&fake, Some("x11"), &[XTERM_WINDOW]);
    let snap = observe(&fake, Some("x11"));
    let servers = snap.surfaces.tmux.known().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].server, identity(3200, 3200));
    assert_eq!(servers[0].panes.len(), 3);
    assert_eq!(servers[0].clients[0].active_pane, "%1");
    assert_eq!(row(&snap, 3300).exe, Exe::Tmux);

    // A stale socket (no server) is skipped.
    let fake = tmux_machine(true).build();
    grok_files(&fake, &[]);
    script_tmux(&fake);
    fake.set_run(
        TMUX,
        session_navigation_native::common::tmux::server_pid_args(TMUX_SOCKET),
        fail("no server running on /tmp/tmux-1000/default\n"),
    );
    assert_eq!(
        observe(&fake, Some("x11")).surfaces.tmux,
        Fact::Known(vec![])
    );

    // An unreadable socket directory: unknown.
    let fake = tmux_machine(true).build();
    grok_files(&fake, &[]);
    script_tmux(&fake);
    fake.make_unreadable("/tmp/tmux-1000");
    assert_eq!(observe(&fake, Some("x11")).surfaces.tmux, Fact::Unknown);
}
