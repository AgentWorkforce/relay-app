//! Turning raw process evidence into owners and hosts. Shared by every
//! provider policy; where a live process can be returned to is
//! [`super::locate`].
use super::Context;
use crate::evidence::{Exe, Fact, ProcessInfo};
use crate::model::{App, Host, TerminalKind};

pub use super::locate::{place, Place, Return};

const MAX_DEPTH: usize = 64;

/// Result of checking a recorded claim (pid + start) against the live table.
pub enum ClaimStatus<'a> {
    Live(&'a ProcessInfo),
    Dead,
    /// The process matches but the record may be from an earlier boot.
    Unknown,
}

/// A claim is live only when the pid is present with the same start time and
/// executable, in this boot, and the record itself was written in this boot.
pub fn check_claim<'a>(
    cx: &Context<'a>,
    pid: u32,
    start: u64,
    written_this_boot: &Fact<bool>,
    exe: Exe,
) -> ClaimStatus<'a> {
    let Some(process) = cx.table.iter().find(|p| p.identity.pid == pid) else {
        return ClaimStatus::Dead;
    };
    if process.identity.start != start || process.exe != exe || process.identity.boot != cx.boot {
        return ClaimStatus::Dead;
    }
    match written_this_boot {
        Fact::Known(true) => ClaimStatus::Live(process),
        Fact::Known(false) => ClaimStatus::Dead,
        Fact::Unknown => ClaimStatus::Unknown,
    }
}

pub fn terminal_of(exe: Exe) -> Option<TerminalKind> {
    Some(match exe {
        Exe::Ghostty => TerminalKind::Ghostty,
        Exe::AppleTerminal => TerminalKind::AppleTerminal,
        Exe::ITerm2 => TerminalKind::ITerm2,
        Exe::Kitty => TerminalKind::Kitty,
        Exe::WezTerm => TerminalKind::WezTerm,
        Exe::Alacritty => TerminalKind::Alacritty,
        Exe::GnomeTerminal => TerminalKind::GnomeTerminal,
        Exe::Konsole => TerminalKind::Konsole,
        Exe::Xterm => TerminalKind::Xterm,
        Exe::WindowsTerminal => TerminalKind::WindowsTerminal,
        Exe::ConsoleHost => TerminalKind::ConsoleHost,
        _ => return None,
    })
}

fn host_app(exe: Exe) -> Option<Host> {
    if let Some(terminal) = terminal_of(exe) {
        return Some(Host::Terminal(terminal));
    }
    match exe {
        Exe::Tmux => Some(Host::Tmux(None)),
        Exe::ClaudeDesktopApp => Some(Host::Desktop(App::ClaudeDesktop)),
        Exe::CodexDesktopApp => Some(Host::Desktop(App::CodexDesktop)),
        Exe::OpenCodeDesktopApp => Some(Host::Desktop(App::OpenCodeDesktop)),
        _ => None,
    }
}

/// The host and the process that is it (the terminal app, the tmux server,
/// the console host of a classic Windows console).
pub fn find_host<'a>(
    table: &'a [ProcessInfo],
    process: &ProcessInfo,
    ttys: bool,
) -> Fact<(Host, Option<&'a ProcessInfo>)> {
    let mut parent = process.ppid;
    for _ in 0..MAX_DEPTH {
        let Some(ancestor) = table.iter().find(|p| p.identity.pid == parent) else {
            break;
        };
        if let Some(host) = host_app(ancestor.exe) {
            return Fact::Known((host, Some(ancestor)));
        }
        // A classic Windows console: `conhost.exe` is a child of the first
        // console process it serves, not an ancestor of the agent.
        if let Some(console) = table
            .iter()
            .find(|p| p.exe == Exe::ConsoleHost && p.ppid == ancestor.identity.pid)
        {
            return Fact::Known((Host::Terminal(TerminalKind::ConsoleHost), Some(console)));
        }
        if ancestor.identity.pid <= 1 || ancestor.ppid == ancestor.identity.pid {
            // Without ttys (Windows) a process with no host cannot be told
            // apart from one in a console nobody can name.
            return if process.tty.is_none() && ttys {
                Fact::Known((Host::Headless, None))
            } else {
                Fact::Unknown
            };
        }
        parent = ancestor.ppid;
    }
    Fact::Unknown
}

/// Walk the parent chain to the hosting application. The walk goes through
/// `/usr/bin/login` between a terminal and its shell, whose effective uid is
/// 0 while its real uid is the person's (VERIFIED, Ghostty 1.4.0, native
/// N15), so it never filters by uid, effective or real.
pub fn host_of(table: &[ProcessInfo], process: &ProcessInfo) -> Fact<Host> {
    match find_host(table, process, true) {
        Fact::Known((host, _)) => Fact::Known(host),
        Fact::Unknown => Fact::Unknown,
    }
}

/// [`host_of`] on the snapshot's platform: without ttys (Windows) no
/// process is known to be headless.
pub fn host_for(cx: &Context<'_>, process: &ProcessInfo) -> Fact<Host> {
    match find_host(cx.table, process, cx.snap.platform.is_unix()) {
        Fact::Known((host, _)) => Fact::Known(host),
        Fact::Unknown => Fact::Unknown,
    }
}

/// `process` and its ancestors, nearest first, up to (excluding) `stop`.
pub fn chain<'a>(
    table: &'a [ProcessInfo],
    process: &'a ProcessInfo,
    stop: Option<u32>,
) -> Vec<&'a ProcessInfo> {
    let mut out = vec![process];
    let mut parent = process.ppid;
    for _ in 0..MAX_DEPTH {
        if Some(parent) == stop {
            break;
        }
        let Some(ancestor) = table.iter().find(|p| p.identity.pid == parent) else {
            break;
        };
        if out.iter().any(|p| p.identity.pid == ancestor.identity.pid) {
            break;
        }
        out.push(ancestor);
        parent = ancestor.ppid;
    }
    out
}

/// Arguments after the program name.
pub fn args(process: &ProcessInfo) -> &[String] {
    process.argv.get(1..).unwrap_or(&[])
}

/// Whether a process's working directory might be `cwd` (unknown counts).
pub fn cwd_may_be(process: &ProcessInfo, cwd: Option<&String>) -> bool {
    match (&process.cwd, cwd) {
        (Fact::Known(own), Some(want)) => own == want,
        _ => true,
    }
}

/// The terminal a return goes through, if it is a terminal.
pub fn return_terminal(ret: &Return) -> Option<TerminalKind> {
    match ret {
        Return::Focus { terminal, .. } => Some(*terminal),
        Return::Activate { app, .. } => app.terminal(),
        Return::Attach { .. } => None,
    }
}
