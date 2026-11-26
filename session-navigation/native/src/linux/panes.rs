//! Kitty and WezTerm pane APIs on Linux, over the shared parsers in
//! [`crate::common::kitty`] and [`crate::common::wezterm`].
//!
//! Instances are found from the environment their panes inherit:
//! `KITTY_LISTEN_ON` (Kitty sets it in every window when `listen_on` is
//! configured) and `WEZTERM_UNIX_SOCKET` (WezTerm sets it in every pane).
//! UNVERIFIED: verify with `tr '\0' '\n' < /proc/$$/environ | grep -e KITTY_LISTEN_ON -e WEZTERM_UNIX_SOCKET`
//! in a Kitty window and a WezTerm pane.
use std::collections::BTreeSet;

use session_navigation::evidence::ProcessInfo;
use session_navigation::hosts::{PaneInfo, PaneRef};

use super::config::LinuxConfig;
use super::table::Table;
use crate::common::kitty::{self, KittyPane, RemoteError};
use crate::common::wezterm::{self, CliError, WezClient, WezPane};
use crate::system::{Run, RunError, System};

/// One Kitty instance that answered `ls`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KittyInstance {
    pub address: String,
    pub panes: Vec<KittyPane>,
}

/// One WezTerm GUI that answered `list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WezInstance {
    pub socket: String,
    pub panes: Vec<WezPane>,
    /// `list-clients`, when it answered.
    pub clients: Option<Vec<WezClient>>,
}

/// What the pane APIs showed in one observation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Apis {
    pub kitty: Vec<KittyInstance>,
    pub wezterm: Vec<WezInstance>,
    /// An instance failed for a reason other than "gone" or "refused":
    /// the pane list is not complete.
    pub incomplete: bool,
}

impl Apis {
    pub fn pane_infos(&self) -> Vec<PaneInfo> {
        let mut out = Vec::new();
        for k in &self.kitty {
            out.extend(kitty::pane_infos(&k.address, &k.panes));
        }
        for w in &self.wezterm {
            out.extend(wezterm::pane_infos(&w.socket, &w.panes));
        }
        out
    }

    /// The focused Kitty window, when exactly one instance has one focused
    /// (Kitty's own `is_focused` on the OS window).
    pub fn kitty_focused(&self) -> Option<PaneRef> {
        let mut found = self
            .kitty
            .iter()
            .filter_map(|k| kitty::focused(&k.address, &k.panes));
        match (found.next(), found.next()) {
            (Some(one), None) => Some(one),
            _ => None,
        }
    }

    /// The focused pane of the WezTerm GUI whose pid is `gui_pid`.
    pub fn wezterm_focused(&self, gui_pid: u32) -> Option<PaneRef> {
        let mut found = self
            .wezterm
            .iter()
            .filter_map(|w| wezterm::focused(&w.socket, gui_pid, w.clients.as_deref()?, &w.panes));
        match (found.next(), found.next()) {
            (Some(one), None) => Some(one),
            _ => None,
        }
    }
}

fn kitty_run(cfg: &LinuxConfig, kitty: &str, args: Vec<String>) -> Run {
    Run::with_args(kitty, args)
        .timeout(cfg.common.timeouts.probe)
        .max_output(kitty::MAX_LS)
}

fn wez_run(cfg: &LinuxConfig, wezterm: &str, socket: &str, args: Vec<String>) -> Run {
    let mut run = Run::with_args(wezterm, args)
        .timeout(cfg.common.timeouts.probe)
        .max_output(wezterm::MAX_LIST);
    run.env = wezterm::socket_env(socket);
    run
}

fn kitty_failure(result: Result<crate::system::Output, RunError>) -> RemoteError {
    match result {
        Ok(output) => kitty::classify_error(&output.stderr),
        Err(RunError::NotFound) => RemoteError::Gone,
        Err(_) => RemoteError::Other,
    }
}

/// `kitty @ --to <address> ls`.
pub fn kitty_ls<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    kitty: &str,
    address: &str,
) -> Result<Vec<KittyPane>, RemoteError> {
    if !kitty::is_valid_address(address) {
        return Err(RemoteError::Other);
    }
    let result = sys.run(&kitty_run(cfg, kitty, kitty::ls_args(address)));
    match &result {
        Ok(output) if output.success() => kitty::parse_ls(&output.stdout).ok_or(RemoteError::Other),
        _ => Err(kitty_failure(result)),
    }
}

/// `kitty @ --to <address> focus-window --match id:<id>`.
pub fn kitty_focus<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    kitty: &str,
    address: &str,
    id: u64,
) -> Result<(), RemoteError> {
    let mut run = kitty_run(cfg, kitty, kitty::focus_args(address, id));
    run.timeout = cfg.common.timeouts.dispatch;
    let result = sys.run(&run);
    match &result {
        Ok(output) if output.success() => Ok(()),
        _ => Err(kitty_failure(result)),
    }
}

fn wez_call<S: System>(sys: &S, run: &Run) -> Result<Vec<u8>, CliError> {
    match sys.run(run) {
        Ok(output) if output.success() => Ok(output.stdout),
        Ok(output) => Err(wezterm::classify_error(&output.stderr)),
        Err(RunError::NotFound) => Err(CliError::Gone),
        Err(_) => Err(CliError::Other),
    }
}

/// `wezterm cli --no-auto-start list --format json` on `socket`.
pub fn wez_list<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    wezterm_program: &str,
    socket: &str,
) -> Result<Vec<WezPane>, CliError> {
    if !wezterm::is_valid_socket(socket) {
        return Err(CliError::Other);
    }
    let out = wez_call(
        sys,
        &wez_run(cfg, wezterm_program, socket, wezterm::list_args()),
    )?;
    wezterm::parse_list(&out).ok_or(CliError::Other)
}

/// `wezterm cli --no-auto-start activate-pane --pane-id <id>` on `socket`.
pub fn wez_activate<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    wezterm_program: &str,
    socket: &str,
    id: u64,
) -> Result<(), CliError> {
    let mut run = wez_run(cfg, wezterm_program, socket, wezterm::activate_args(id));
    run.timeout = cfg.common.timeouts.dispatch;
    wez_call(sys, &run).map(|_| ())
}

fn wez_clients<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    wezterm_program: &str,
    socket: &str,
) -> Option<Vec<WezClient>> {
    let out = wez_call(
        sys,
        &wez_run(cfg, wezterm_program, socket, wezterm::clients_args()),
    )
    .ok()?;
    wezterm::parse_clients(&out)
}

/// Every distinct `KITTY_LISTEN_ON` / `WEZTERM_UNIX_SOCKET` in the table.
pub fn addresses(table: &Table) -> (BTreeSet<String>, BTreeSet<String>) {
    let kitty = table
        .identities
        .values()
        .filter_map(|i| i.kitty_listen_on.clone())
        .collect();
    let wez = table
        .identities
        .values()
        .filter_map(|i| i.wezterm_socket.clone())
        .collect();
    (kitty, wez)
}

/// Whether a Kitty / WezTerm process runs at all.
pub fn running(table: &[ProcessInfo], exe: session_navigation::evidence::Exe) -> bool {
    table.iter().any(|p| p.exe == exe)
}

/// Ask every instance found in the table. A refused or stale instance is
/// left out (its terminal's focus ability says so); any other failure
/// marks the listing incomplete.
pub fn discover<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    table: &Table,
    kitty_program: Option<&str>,
    wezterm_program: Option<&str>,
) -> Apis {
    let (kitty_addresses, wez_sockets) = addresses(table);
    let mut apis = Apis::default();
    if let Some(program) = kitty_program {
        for address in kitty_addresses {
            match kitty_ls(sys, cfg, program, &address) {
                Ok(panes) => apis.kitty.push(KittyInstance { address, panes }),
                Err(RemoteError::Gone | RemoteError::Refused) => {}
                Err(RemoteError::Other) => apis.incomplete = true,
            }
        }
    }
    if let Some(program) = wezterm_program {
        for socket in wez_sockets {
            match wez_list(sys, cfg, program, &socket) {
                Ok(panes) => {
                    let clients = wez_clients(sys, cfg, program, &socket);
                    apis.wezterm.push(WezInstance {
                        socket,
                        panes,
                        clients,
                    });
                }
                Err(CliError::Gone) => {}
                Err(_) => apis.incomplete = true,
            }
        }
    }
    apis
}
