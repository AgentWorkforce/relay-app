//! Kitty's remote-control API, OS-independent: argv builders for
//! `kitty @ --to <address> …`, the `ls` JSON parser, the error
//! classification of a failed `kitty @` run, and the "does this pane still
//! hold the process" check shared with WezTerm.
//!
//! UNVERIFIED (Kitty was not installed where this was written): every
//! output shape and error text below is taken from Kitty's documentation
//! (`kitty @ ls`, `kitty @ focus-window`, `kitten @` remote control docs),
//! not captured from a running Kitty.
//!
//! Addresses: a [`PaneRef::KittyWindow`] `socket` is the exact `--to`
//! address the pane was listed through (`unix:/path/to/sock` or, on Linux,
//! `unix:@abstract`), so the executor talks to the same instance the
//! observer saw.
use serde::Deserialize;
use session_navigation::evidence::{Exe, ProcessInfo};
use session_navigation::hosts::{PaneInfo, PaneRef};
use session_navigation::model::TerminalKind;

/// Largest `ls` output accepted (it includes every window's environment).
pub const MAX_LS: usize = 8 * 1024 * 1024;

/// One Kitty window (a pane), flattened from `ls`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KittyPane {
    pub os_window: u64,
    pub tab: u64,
    /// The window id `focus-window --match id:<id>` takes.
    pub id: u64,
    /// The window's own child process (its shell or program).
    pub pid: Option<u32>,
    pub cwd: Option<String>,
    /// The focused window of the focused tab of the focused OS window.
    pub focused: bool,
}

#[derive(Deserialize)]
struct LsWindow {
    id: u64,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    is_focused: bool,
}

#[derive(Deserialize)]
struct LsTab {
    id: u64,
    #[serde(default)]
    is_focused: bool,
    #[serde(default)]
    windows: Vec<LsWindow>,
}

#[derive(Deserialize)]
struct LsOsWindow {
    id: u64,
    #[serde(default)]
    is_focused: bool,
    #[serde(default)]
    tabs: Vec<LsTab>,
}

/// `kitty @ ls` JSON: OS windows → tabs → windows. Unknown fields are
/// ignored; a missing or non-numeric `id` fails the whole parse (`None`).
pub fn parse_ls(bytes: &[u8]) -> Option<Vec<KittyPane>> {
    if bytes.len() > MAX_LS {
        return None;
    }
    let os_windows: Vec<LsOsWindow> = serde_json::from_slice(bytes).ok()?;
    let mut panes = Vec::new();
    for os in os_windows {
        for tab in os.tabs {
            for w in tab.windows {
                panes.push(KittyPane {
                    os_window: os.id,
                    tab: tab.id,
                    id: w.id,
                    pid: w.pid.filter(|p| *p > 0),
                    cwd: w.cwd,
                    focused: os.is_focused && tab.is_focused && w.is_focused,
                });
            }
        }
    }
    Some(panes)
}

/// The panes as `PaneInfo`s for `address`. Kitty's `ls` reports no tty.
pub fn pane_infos(address: &str, panes: &[KittyPane]) -> Vec<PaneInfo> {
    panes
        .iter()
        .map(|p| PaneInfo {
            terminal: TerminalKind::Kitty,
            pane: PaneRef::KittyWindow {
                socket: address.to_owned(),
                id: p.id,
            },
            tty: None,
            pid: p.pid,
        })
        .collect()
}

/// The focused pane, when exactly one is focused.
pub fn focused(address: &str, panes: &[KittyPane]) -> Option<PaneRef> {
    let mut it = panes.iter().filter(|p| p.focused);
    match (it.next(), it.next()) {
        (Some(one), None) => Some(PaneRef::KittyWindow {
            socket: address.to_owned(),
            id: one.id,
        }),
        _ => None,
    }
}

/// `unix:/path` → `/path` (the file to stat). Abstract (`unix:@…`) and
/// TCP addresses have no file: `None`.
pub fn socket_path(address: &str) -> Option<&str> {
    address.strip_prefix("unix:").filter(|p| p.starts_with('/'))
}

/// `/path` → `unix:/path`.
pub fn address_for_path(path: &str) -> String {
    format!("unix:{path}")
}

/// An address this crate will hand to `--to`: `unix:` + an absolute path or
/// an abstract name, no control characters, bounded.
pub fn is_valid_address(address: &str) -> bool {
    let Some(rest) = address.strip_prefix("unix:") else {
        return false;
    };
    (rest.starts_with('/') || rest.starts_with('@'))
        && rest.len() > 1
        && address.len() <= 1024
        && !address.chars().any(char::is_control)
}

/// argv after the kitty program: `@ --to <address> ls`.
pub fn ls_args(address: &str) -> Vec<String> {
    vec!["@".into(), "--to".into(), address.into(), "ls".into()]
}

/// argv after the kitty program: `@ --to <address> focus-window --match id:<id>`.
pub fn focus_args(address: &str, id: u64) -> Vec<String> {
    vec![
        "@".into(),
        "--to".into(),
        address.into(),
        "focus-window".into(),
        "--match".into(),
        format!("id:{id}"),
    ]
}

/// argv after the kitty program for a new OS window running `program args`
/// in `cwd`: `--directory <cwd> [-o env=K=V]… <program> <args…>`. No shell
/// is involved; `program` must be absolute (so it is never read as an
/// option). UNVERIFIED: `-o env=K=V` sets the child's environment (Kitty's
/// `env` option, documented for kitty.conf and `--override`).
pub fn launch_args(
    cwd: &str,
    env: &[(String, String)],
    program: &str,
    args: &[String],
) -> Vec<String> {
    let mut out = vec!["--directory".to_owned(), cwd.to_owned()];
    for (k, v) in env {
        out.push("-o".into());
        out.push(format!("env={k}={v}"));
    }
    out.push(program.to_owned());
    out.extend(args.iter().cloned());
    out
}

/// Why a `kitty @` run failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteError {
    /// Nothing listens on the address (stale socket, Kitty quit).
    Gone,
    /// Kitty answered but refused remote control (`allow_remote_control`
    /// off, or a password / `ask` prompt declined).
    Refused,
    Other,
}

/// Classify a failed `kitty @` run from its stderr. UNVERIFIED: the texts
/// are Kitty's documented / source error messages, never captured here.
pub fn classify_error(stderr: &[u8]) -> RemoteError {
    let text = String::from_utf8_lossy(stderr.get(..stderr.len().min(64 * 1024)).unwrap_or(&[]))
        .to_ascii_lowercase();
    if text.contains("remote control is disabled")
        || text.contains("allow_remote_control")
        || text.contains("denied")
        || text.contains("not allowed")
        || text.contains("password")
    {
        RemoteError::Refused
    } else if text.contains("no such file")
        || text.contains("connection refused")
        || text.contains("failed to connect")
        || text.contains("could not connect")
    {
        RemoteError::Gone
    } else {
        RemoteError::Other
    }
}

/// The pid an instance socket names: Kitty appends `-<kitty pid>` to the
/// `listen_on` path given in kitty.conf (`unix:/tmp/kitty` →
/// `/tmp/kitty-1234`). UNVERIFIED (Kitty docs, `listen_on`).
pub fn socket_owner_pid(file_name: &str) -> Option<u32> {
    let (_, digits) = file_name.rsplit_once('-')?;
    let ok = !digits.is_empty() && digits.len() <= 10 && digits.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| digits.parse().ok()).flatten()
}

/// Whether the pane with `tty` / `pid` still holds `process`: the same tty,
/// or the pane's own process is `process` or one of its ancestors below the
/// terminal (`decide/locate.rs` `api_pane`, re-checked on a fresh table).
/// Shared by Kitty and WezTerm.
pub fn pane_holds(
    table: &[ProcessInfo],
    process: &ProcessInfo,
    tty: Option<&str>,
    pid: Option<u32>,
) -> bool {
    if tty.is_some() && tty == process.tty.as_deref() {
        return true;
    }
    let Some(pid) = pid else {
        return false;
    };
    let mut current = process;
    for _ in 0..64 {
        if current.identity.pid == pid {
            return true;
        }
        if is_terminal(current.exe) || current.ppid == 0 || current.ppid == current.identity.pid {
            return false;
        }
        match table.iter().find(|p| p.identity.pid == current.ppid) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
    false
}

fn is_terminal(exe: Exe) -> bool {
    matches!(
        exe,
        Exe::Ghostty
            | Exe::AppleTerminal
            | Exe::ITerm2
            | Exe::Kitty
            | Exe::WezTerm
            | Exe::Alacritty
            | Exe::GnomeTerminal
            | Exe::Konsole
            | Exe::Xterm
            | Exe::WindowsTerminal
            | Exe::ConsoleHost
    )
}
