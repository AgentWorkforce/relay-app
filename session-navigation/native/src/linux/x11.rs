//! X11 windows: the top-level list from `wmctrl -lp` (EWMH
//! `_NET_CLIENT_LIST` with each client's `_NET_WM_PID`), the active window
//! and the owner re-check from `xdotool`, and activation with
//! `xdotool windowactivate --sync`.
//!
//! UNVERIFIED output shapes (verify on an X11 session with
//! `wmctrl -lp; xdotool getactivewindow; xdotool getwindowpid $(xdotool getactivewindow)`):
//! - `wmctrl -lp` prints `0x03a00003  0 12345  host title…` per client
//!   (desktop `-1` for sticky windows, pid `0` without `_NET_WM_PID`);
//! - `xdotool getactivewindow` prints the decimal window id;
//! - `xdotool getwindowpid <id>` prints the decimal `_NET_WM_PID`, and
//!   exits non-zero when the window has none.
use session_navigation::evidence::ProcessInfo;
use session_navigation::hosts::{WindowId, WindowInfo};

use super::config::LinuxConfig;
use crate::system::{Run, System};

pub const XDOTOOL: &str = "xdotool";
pub const WMCTRL: &str = "wmctrl";
pub const HOSTNAME: &str = "/proc/sys/kernel/hostname";

/// One `wmctrl -lp` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientWindow {
    pub id: u64,
    /// `_NET_WM_PID`, `0` when the client set none.
    pub pid: u32,
    /// `WM_CLIENT_MACHINE`, `N/A` when unset.
    pub machine: String,
}

fn hex(token: &str) -> Option<u64> {
    let digits = token.strip_prefix("0x")?;
    (!digits.is_empty() && digits.len() <= 16)
        .then(|| u64::from_str_radix(digits, 16).ok())
        .flatten()
}

fn decimal<T: std::str::FromStr>(token: &str) -> Option<T> {
    let ok = !token.is_empty() && token.len() <= 20 && token.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| token.parse().ok()).flatten()
}

/// Parse `wmctrl -lp`. Any malformed row fails the list.
pub fn parse_wmctrl(bytes: &[u8]) -> Option<Vec<ClientWindow>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut rows = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut fields = line.split_ascii_whitespace();
        let id = hex(fields.next()?)?;
        let desktop = fields.next()?;
        let desktop_ok = desktop == "-1" || decimal::<u32>(desktop).is_some();
        if !desktop_ok {
            return None;
        }
        let pid = decimal(fields.next()?)?;
        let machine = fields.next().unwrap_or("N/A").to_owned();
        rows.push(ClientWindow { id, pid, machine });
    }
    Some(rows)
}

/// One decimal number on a line (`getactivewindow`, `getwindowpid`).
pub fn parse_number(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    decimal(text.trim())
}

/// Resolve each client's pid to a full identity in `table`. Windows
/// without `_NET_WM_PID`, whose pid is not in the table, or whose
/// `WM_CLIENT_MACHINE` names another host (a remote client's pid is in
/// another machine's pid space) are left out.
pub fn owned(
    clients: &[ClientWindow],
    table: &[ProcessInfo],
    host: Option<&str>,
) -> Vec<WindowInfo> {
    clients
        .iter()
        .filter(|c| c.pid != 0)
        .filter(|c| c.machine == "N/A" || host.is_none_or(|h| h == c.machine))
        .filter_map(|c| {
            let owner = table.iter().find(|p| p.identity.pid == c.pid)?;
            Some(WindowInfo {
                id: WindowId(c.id),
                owner: owner.identity,
            })
        })
        .collect()
}

pub fn run(program: &str, args: Vec<String>, cfg: &LinuxConfig) -> Run {
    let mut run = Run::with_args(program, args).timeout(cfg.common.timeouts.probe);
    run.env = cfg.x11_env();
    run
}

/// The resolved helper programs (`None` when not installed).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Helpers {
    pub xdotool: Option<String>,
    pub wmctrl: Option<String>,
}

impl Helpers {
    pub fn find<S: System>(sys: &S, cfg: &LinuxConfig) -> Self {
        Self {
            xdotool: super::apps::which(sys, &cfg.program_path, XDOTOOL),
            wmctrl: super::apps::which(sys, &cfg.program_path, WMCTRL),
        }
    }

    /// Raising needs both: `wmctrl` to list, `xdotool` to check and act.
    pub fn can_raise(&self) -> bool {
        self.xdotool.is_some() && self.wmctrl.is_some()
    }
}

/// Every client window with a resolved owner, or `None`.
pub fn windows<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    helpers: &Helpers,
    table: &[ProcessInfo],
) -> Option<Vec<WindowInfo>> {
    let wmctrl = helpers.wmctrl.as_deref()?;
    let output = crate::common::probe(sys, &run(wmctrl, vec!["-lp".into()], cfg))?;
    let clients = parse_wmctrl(&output.stdout)?;
    let host = sys
        .read_file(HOSTNAME, 256)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .map(|h| h.trim().to_owned());
    Some(owned(&clients, table, host.as_deref()))
}

/// `xdotool getactivewindow`.
pub fn active<S: System>(sys: &S, cfg: &LinuxConfig, helpers: &Helpers) -> Option<WindowId> {
    let xdotool = helpers.xdotool.as_deref()?;
    let output = crate::common::probe(sys, &run(xdotool, vec!["getactivewindow".into()], cfg))?;
    parse_number(&output.stdout).map(WindowId)
}

/// `xdotool getwindowpid <id>`: the window's `_NET_WM_PID` now.
pub fn window_pid<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    xdotool: &str,
    id: WindowId,
) -> Option<u32> {
    let args = vec!["getwindowpid".into(), id.0.to_string()];
    let output = crate::common::probe(sys, &run(xdotool, args, cfg))?;
    parse_number(&output.stdout).and_then(|p| u32::try_from(p).ok())
}

/// argv of `xdotool windowactivate --sync <id>`.
pub fn activate_args(id: WindowId) -> Vec<String> {
    vec!["windowactivate".into(), "--sync".into(), id.0.to_string()]
}
