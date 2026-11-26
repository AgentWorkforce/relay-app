//! WezTerm's `wezterm cli`, OS-independent: argv builders, the
//! `list --format json` and `list-clients --format json` parsers, error
//! classification, and the launch argv.
//!
//! UNVERIFIED (WezTerm was not installed where this was written): the JSON
//! shapes and error texts follow WezTerm's documentation (`wezterm cli
//! list`, `list-clients`, `activate-pane`, `start`), not a captured run.
//!
//! Every `cli` call passes `--no-auto-start`, so observing never starts a
//! mux server, and talks to exactly one GUI through `WEZTERM_UNIX_SOCKET`
//! (the [`PaneRef::WezTermPane`] `socket`, an absolute path).
use serde::Deserialize;
use session_navigation::hosts::{PaneInfo, PaneRef};
use session_navigation::model::TerminalKind;

pub use super::kitty::pane_holds;

/// The environment variable that selects the GUI's socket.
pub const SOCKET_ENV: &str = "WEZTERM_UNIX_SOCKET";
pub const MAX_LIST: usize = 4 * 1024 * 1024;

/// One pane from `list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WezPane {
    pub window: u64,
    pub tab: u64,
    pub id: u64,
    /// `tty_name` (Unix only; absent on Windows and for remote domains).
    pub tty: Option<String>,
    /// `cwd` as WezTerm reports it (a `file://host/path` URL).
    pub cwd: Option<String>,
    /// The active pane of its tab (not of the GUI).
    pub active_in_tab: bool,
}

#[derive(Deserialize)]
struct ListRow {
    window_id: u64,
    tab_id: u64,
    pane_id: u64,
    #[serde(default)]
    tty_name: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    is_active: bool,
}

/// `wezterm cli list --format json`.
pub fn parse_list(bytes: &[u8]) -> Option<Vec<WezPane>> {
    if bytes.len() > MAX_LIST {
        return None;
    }
    let rows: Vec<ListRow> = serde_json::from_slice(bytes).ok()?;
    Some(
        rows.into_iter()
            .map(|r| WezPane {
                window: r.window_id,
                tab: r.tab_id,
                id: r.pane_id,
                tty: r
                    .tty_name
                    .filter(|t| t.starts_with("/dev/") && !t.chars().any(char::is_control)),
                cwd: r.cwd,
                active_in_tab: r.is_active,
            })
            .collect(),
    )
}

/// One client from `list-clients`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WezClient {
    pub pid: u32,
    pub focused_pane: Option<u64>,
    /// Idle time in whole seconds, when reported.
    pub idle_secs: Option<u64>,
}

#[derive(Deserialize)]
struct Duration {
    secs: u64,
}

#[derive(Deserialize)]
struct ClientRow {
    pid: u32,
    #[serde(default)]
    focused_pane_id: Option<u64>,
    #[serde(default)]
    idle_time: Option<Duration>,
}

/// `wezterm cli list-clients --format json`.
pub fn parse_clients(bytes: &[u8]) -> Option<Vec<WezClient>> {
    if bytes.len() > MAX_LIST {
        return None;
    }
    let rows: Vec<ClientRow> = serde_json::from_slice(bytes).ok()?;
    Some(
        rows.into_iter()
            .map(|r| WezClient {
                pid: r.pid,
                focused_pane: r.focused_pane_id,
                idle_secs: r.idle_time.map(|d| d.secs),
            })
            .collect(),
    )
}

/// The focused pane of the GUI whose pid is `gui_pid`: its own client
/// entry's `focused_pane_id`, when there is exactly one such entry and the
/// pane is listed. UNVERIFIED: the GUI appears in `list-clients` of its own
/// socket with its own pid.
pub fn focused(
    socket: &str,
    gui_pid: u32,
    clients: &[WezClient],
    panes: &[WezPane],
) -> Option<PaneRef> {
    let mut own = clients.iter().filter(|c| c.pid == gui_pid);
    let (Some(one), None) = (own.next(), own.next()) else {
        return None;
    };
    let id = one.focused_pane?;
    panes
        .iter()
        .any(|p| p.id == id)
        .then(|| PaneRef::WezTermPane {
            socket: socket.to_owned(),
            id,
        })
}

pub fn pane_infos(socket: &str, panes: &[WezPane]) -> Vec<PaneInfo> {
    panes
        .iter()
        .map(|p| PaneInfo {
            terminal: TerminalKind::WezTerm,
            pane: PaneRef::WezTermPane {
                socket: socket.to_owned(),
                id: p.id,
            },
            tty: p.tty.clone(),
            // `list` reports no process id.
            pid: None,
        })
        .collect()
}

/// A socket path this crate will put in `WEZTERM_UNIX_SOCKET`.
pub fn is_valid_socket(socket: &str) -> bool {
    socket.starts_with('/') && socket.len() <= 1024 && !socket.chars().any(char::is_control)
}

/// The GUI pid a socket file names: `gui-sock-<pid>`. UNVERIFIED (WezTerm
/// source: `wezterm-gui` creates `gui-sock-<pid>` in its runtime dir).
pub fn socket_owner_pid(file_name: &str) -> Option<u32> {
    let digits = file_name.strip_prefix("gui-sock-")?;
    let ok = !digits.is_empty() && digits.len() <= 10 && digits.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| digits.parse().ok()).flatten()
}

/// The `(key, value)` env for every `cli` run on `socket`.
pub fn socket_env(socket: &str) -> Vec<(String, String)> {
    vec![(SOCKET_ENV.to_owned(), socket.to_owned())]
}

/// argv after the wezterm program: `cli --no-auto-start list --format json`.
pub fn list_args() -> Vec<String> {
    ["cli", "--no-auto-start", "list", "--format", "json"]
        .map(String::from)
        .to_vec()
}

/// argv after the wezterm program: `cli --no-auto-start list-clients --format json`.
pub fn clients_args() -> Vec<String> {
    ["cli", "--no-auto-start", "list-clients", "--format", "json"]
        .map(String::from)
        .to_vec()
}

/// argv after the wezterm program: `cli --no-auto-start activate-pane --pane-id <id>`.
pub fn activate_args(id: u64) -> Vec<String> {
    vec![
        "cli".into(),
        "--no-auto-start".into(),
        "activate-pane".into(),
        "--pane-id".into(),
        id.to_string(),
    ]
}

/// argv after the wezterm program for a new window:
/// `start --cwd <cwd> -- [<env program> K=V…] <program> <args…>`. WezTerm's
/// `start` has no environment flag, so a non-empty `env` runs through
/// `env_program` (`/usr/bin/env`) as argv words, never a shell string.
pub fn launch_args(
    cwd: &str,
    env: &[(String, String)],
    env_program: &str,
    program: &str,
    args: &[String],
) -> Vec<String> {
    let mut out = vec![
        "start".to_owned(),
        "--cwd".to_owned(),
        cwd.to_owned(),
        "--".to_owned(),
    ];
    out.extend(with_env(env, env_program, program, args));
    out
}

/// `[<env program> K=V…] <program> <args…>`: an argv that sets `env` without
/// a shell. Also used for Alacritty.
pub fn with_env(
    env: &[(String, String)],
    env_program: &str,
    program: &str,
    args: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    if !env.is_empty() {
        out.push(env_program.to_owned());
        out.extend(env.iter().map(|(k, v)| format!("{k}={v}")));
    }
    out.push(program.to_owned());
    out.extend(args.iter().cloned());
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliError {
    /// Nothing listens on the socket (stale socket, GUI quit).
    Gone,
    /// The pane id is not known to the mux.
    NoSuchPane,
    Other,
}

/// Classify a failed `wezterm cli` run. UNVERIFIED error texts.
pub fn classify_error(stderr: &[u8]) -> CliError {
    let text = String::from_utf8_lossy(stderr.get(..stderr.len().min(64 * 1024)).unwrap_or(&[]))
        .to_ascii_lowercase();
    if text.contains("no such file")
        || text.contains("connection refused")
        || text.contains("failed to connect")
    {
        CliError::Gone
    } else if text.contains("pane") && (text.contains("not found") || text.contains("invalid")) {
        CliError::NoSuchPane
    } else {
        CliError::Other
    }
}
