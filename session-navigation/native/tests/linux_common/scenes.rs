//! Whole machines for the end-to-end and observer tests: a Grok session in
//! a Kitty pane, under xterm, in tmux (attached and detached), stopped.
use serde_json::json;
use session_navigation::evidence::Fact;
use session_navigation::model::{Host, SessionRef};
use session_navigation_native::common::{kitty, tmux, HostHistory};
use session_navigation_native::system::{FakeSystem, Run};

use super::*;

/// `opened_at` of the Grok record: after every process below started.
pub const OPENED_AT: &str = "2025-11-12T00:00:00Z";
/// The xterm window (`0x03c0000e`) and the Kitty OS window (`0x03a00007`).
pub const XTERM_WINDOW: u64 = 0x03c0_000e;
pub const KITTY_WINDOW: u64 = 0x03a0_0007;
pub const FIREFOX_WINDOW: u64 = 0x0460_0002;

pub fn grok() -> SessionRef {
    SessionRef::new(session_navigation::model::Provider::Grok, GROK_ID)
}

/// Where the integrator saw the session last.
pub struct Seen(pub Host);

impl HostHistory for Seen {
    fn last_seen(&self, _: &SessionRef) -> Fact<Host> {
        Fact::Known(self.0)
    }
}

/// Grok's own files: the session summary and the active-session records
/// (`(pid, opened_at)`).
pub fn grok_files(fake: &FakeSystem, active: &[(u32, &str)]) {
    let grok = format!("{HOME}/.grok");
    let records: Vec<_> = active
        .iter()
        .map(
            |(pid, at)| json!({"session_id": GROK_ID, "pid": pid, "cwd": PROJECT, "opened_at": at}),
        )
        .collect();
    fake.add_file(
        &format!("{grok}/active_sessions.json"),
        serde_json::to_vec(&records).unwrap(),
        1,
    );
    fake.add_file(
        &format!("{grok}/sessions/%2Fhome%2Fexample%2Fcode%2Fproject/{GROK_ID}/summary.json"),
        json!({"info": {"id": GROK_ID, "cwd": PROJECT}}).to_string(),
        1,
    );
    fake.add_executable(GROK_BIN, 1);
    fake.add_executable(GROK_REAL, 1);
    fake.add_symlink(GROK_BIN, GROK_REAL);
}

/// A Grok process in a shell.
pub fn grok_proc(pid: u32, shell: u32, tty: u64) -> Proc {
    Proc::new(pid, shell, GROK_REAL, &["grok"])
        .tty(tty)
        .cwd(PROJECT)
}

/// Kitty (2470) with two windows: bash 2481 on pts/1 running Grok 2601,
/// and bash 2490 on pts/2. Kitty's remote control listens on
/// [`KITTY_ADDRESS`].
pub fn kitty_machine(display: Option<&'static str>) -> Machine {
    let mut m = Machine::new(display);
    let kitty_env = |p: Proc| {
        p.env("KITTY_LISTEN_ON", KITTY_ADDRESS)
            .env("KITTY_WINDOW_ID", "1")
            .env("WINDOWID", &KITTY_WINDOW.to_string())
    };
    m.push(Proc::new(2470, 1600, KITTY, &["kitty"]));
    m.push(kitty_env(
        Proc::new(2481, 2470, "/usr/bin/bash", &["/bin/bash"]).tty(1),
    ));
    m.push(kitty_env(grok_proc(2601, 2481, 1)));
    m.push(kitty_env(
        Proc::new(2490, 2470, "/usr/bin/bash", &["/bin/bash"]).tty(2),
    ));
    m
}

/// xterm (3100) with bash 3110 on pts/3 running Grok 3120.
pub fn xterm_machine(display: Option<&'static str>) -> Machine {
    let mut m = Machine::new(display);
    let window = XTERM_WINDOW.to_string();
    m.push(Proc::new(3100, 1600, "/usr/bin/xterm", &["xterm"]));
    m.push(
        Proc::new(3110, 3100, "/usr/bin/bash", &["bash"])
            .tty(3)
            .env("WINDOWID", &window),
    );
    m.push(grok_proc(3120, 3110, 3).env("WINDOWID", &window));
    m
}

pub fn kitty_ls_run() -> Run {
    Run::with_args(KITTY, kitty::ls_args(KITTY_ADDRESS))
}

pub fn wmctrl_run(display: Option<&str>) -> Run {
    x11_run(WMCTRL, &["-lp"], display)
}

pub fn active_run(display: Option<&str>) -> Run {
    x11_run(XDOTOOL, &["getactivewindow"], display)
}

pub fn windowpid_run(id: u64, display: Option<&str>) -> Run {
    x11_run(XDOTOOL, &["getwindowpid", &id.to_string()], display)
}

pub fn activate_run(id: u64, display: Option<&str>) -> Run {
    x11_run(
        XDOTOOL,
        &["windowactivate", "--sync", &id.to_string()],
        display,
    )
}

pub fn wmctrl_output() -> String {
    format!(
        "0x01e00003 -1 1600   devbox Desktop\n\
         {KITTY_WINDOW:#010x}  0 2470   devbox ~/code/project\n\
         {XTERM_WINDOW:#010x}  0 3100   devbox example@devbox: ~\n\
         {FIREFOX_WINDOW:#010x}  0 3500   N/A    Firefox\n"
    )
}

/// The tmux server 3200 on [`TMUX_SOCKET`] with panes %0..%2
/// (`tmux-list-panes.txt`): Grok runs in %1 (pts/6, session $0) when
/// `attached`, else in %2 (pts/7, session $1, which no client shows). The
/// only client (3300, pts/2) shows $0 inside xterm 3100.
pub fn tmux_machine(attached: bool) -> Machine {
    let mut m = xterm_machine(Some("x11"));
    m.procs.retain(|p| p.pid != 3120 && p.pid != 3110);
    m.programs.push("tmux");
    let window = XTERM_WINDOW.to_string();
    let tmux_env = "/tmp/tmux-1000/default,3200,0";
    m.push(Proc::new(3200, 1600, TMUX, &["tmux", "new-session"]));
    let (pts, shell, agent) = if attached {
        (6, 3210, 3220)
    } else {
        (7, 3230, 3240)
    };
    m.push(
        Proc::new(shell, 3200, "/usr/bin/bash", &["-bash"])
            .tty(pts)
            .env("TMUX", tmux_env),
    );
    m.push(grok_proc(agent, shell, pts).env("TMUX", tmux_env));
    m.push(
        Proc::new(3290, 3100, "/usr/bin/bash", &["bash"])
            .tty(2)
            .env("WINDOWID", &window),
    );
    m.push(
        Proc::new(3300, 3290, TMUX, &["tmux", "attach"])
            .tty(2)
            .env("WINDOWID", &window),
    );
    m
}

pub fn script_tmux(fake: &FakeSystem) {
    fake.add_file(TMUX_SOCKET, "", 1);
    fake.on(&Run::new(TMUX, &["-V"]), vec![ok("tmux 3.4\n")]);
    let on = |args: Vec<String>, out: Vec<u8>| fake.on_run_owned(TMUX, args, ok(out));
    on(
        tmux::server_pid_args(TMUX_SOCKET),
        fixture("tmux-server-pid.txt"),
    );
    on(
        tmux::list_panes_args(TMUX_SOCKET),
        fixture("tmux-list-panes.txt"),
    );
    on(
        tmux::list_clients_args(TMUX_SOCKET),
        fixture("tmux-list-clients.txt"),
    );
}

/// Script the X11 reads: the window list, and the active window for each
/// observation in turn.
pub fn script_x11(fake: &FakeSystem, display: Option<&str>, active: &[u64]) {
    fake.on(&wmctrl_run(display), vec![ok(wmctrl_output())]);
    fake.on(
        &active_run(display),
        active.iter().map(|id| ok(format!("{id}\n"))).collect(),
    );
}
