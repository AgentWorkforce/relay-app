//! Terminal hosts on macOS: what this adapter can do with each terminal
//! (`TerminalSupport`), the Kitty and WezTerm panes their APIs list, and the
//! tmux servers reachable by socket.
//!
//! UNVERIFIED natively: iTerm2, Kitty, WezTerm, Alacritty and tmux were not
//! installed on the Mac this was written on. Every command, path and output
//! shape is from each tool's documentation; the tests replay
//! documentation-derived fixtures (`tests/fixtures/macos/`).
pub mod kitty;
pub mod tmux;
pub mod wezterm;

use session_navigation::evidence::{Fact, ProcessInfo};
use session_navigation::hosts::{
    Ability, PaneRef, Permission, Setting, Surfaces, TerminalSupport, Unsupported,
};
use session_navigation::model::TerminalKind;

use super::config::MacConfig;
use super::parse::AppsReport;
use crate::system::System;

/// One pane API's view: the panes it listed, what focusing through it can
/// do now, and its focused pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiView {
    pub panes: Fact<Vec<session_navigation::hosts::PaneInfo>>,
    pub ability: Ability,
    pub front: Fact<Option<PaneRef>>,
    pub front_tty: Fact<Option<String>>,
}

impl ApiView {
    pub(crate) fn unknown(ability: Ability) -> Self {
        Self {
            panes: Fact::Unknown,
            ability,
            front: Fact::Unknown,
            front_tty: Fact::Unknown,
        }
    }

    /// No instance of the terminal runs: nothing to list.
    pub(crate) fn none(ability: Ability) -> Self {
        Self {
            panes: Fact::Known(Vec::new()),
            ability,
            front: Fact::Unknown,
            front_tty: Fact::Unknown,
        }
    }
}

/// Everything the observer reports about terminal hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostView {
    pub surfaces: Surfaces,
    pub kitty: ApiView,
    pub wezterm: ApiView,
}

/// What the machine probes found, as the support table needs it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Probed {
    /// The scripted terminal whose front-pane read was refused with -1743.
    pub automation_denied: Option<TerminalKind>,
    pub kitty: Option<Ability>,
    pub wezterm: Option<Ability>,
}

/// The macOS terminals, in [`TerminalKind::ALL`] order.
pub const MAC_TERMINALS: [TerminalKind; 6] = [
    TerminalKind::Ghostty,
    TerminalKind::AppleTerminal,
    TerminalKind::ITerm2,
    TerminalKind::Kitty,
    TerminalKind::WezTerm,
    TerminalKind::Alacritty,
];

/// What this adapter can do with each macOS terminal now.
///
/// - Ghostty, Terminal, iTerm2: scripted by tty (`focus_pane`, `launch`),
///   `NeedsPermission(MacAutomation)` once a read was refused with -1743.
/// - Kitty, WezTerm: `focus_pane` through their API when it answered, else
///   `NeedsSetting`; launch via `open -n -b <bundle> --args …`.
/// - Alacritty: no pane API; activate and launch only.
///
/// `raise_window` is `NotImplemented` everywhere: macOS has no window list
/// in this adapter. Whether an app is installed is `apps`' business, not
/// this table's.
pub fn support(probed: Probed) -> Vec<TerminalSupport> {
    let no_raise = Ability::Unsupported(Unsupported::NotImplemented);
    let api = |terminal, ability: Option<Ability>, setting| TerminalSupport {
        terminal,
        focus_pane: ability.unwrap_or(Ability::NeedsSetting(setting)),
        raise_window: no_raise,
        activate_app: Ability::Available,
        launch: Ability::Available,
        tabs_share_window: true,
    };
    MAC_TERMINALS
        .into_iter()
        .map(|terminal| match terminal {
            TerminalKind::Kitty => api(terminal, probed.kitty, Setting::KittyRemoteControl),
            TerminalKind::WezTerm => api(terminal, probed.wezterm, Setting::WezTermMuxSocket),
            TerminalKind::Alacritty => TerminalSupport {
                terminal,
                focus_pane: Ability::Unsupported(Unsupported::NoPaneApi),
                raise_window: no_raise,
                activate_app: Ability::Available,
                launch: Ability::Available,
                tabs_share_window: false,
            },
            scripted => {
                let mut s = TerminalSupport::scripted(scripted);
                if probed.automation_denied == Some(scripted) {
                    let ask = Ability::NeedsPermission(Permission::MacAutomation);
                    s.focus_pane = ask;
                    s.launch = ask;
                }
                s
            }
        })
        .collect()
}

/// Observe every host surface. `table` and `report` are the snapshot's own
/// (unknown → the dependent lists are `Unknown`); `tmux` is the resolved
/// tmux executable.
pub fn observe<S: System + Sync>(
    sys: &S,
    cfg: &MacConfig,
    table: Option<&[ProcessInfo]>,
    report: Option<&AppsReport>,
    tmux: Option<&str>,
    automation_denied: Option<TerminalKind>,
) -> HostView {
    // Independent probes, at once when the system allows it.
    let (kitty, wezterm, tmux) = crate::common::par::join3(
        sys,
        || kitty::observe(sys, cfg, table, report),
        || wezterm::observe(sys, cfg, table, report),
        || tmux::observe(sys, cfg, table, tmux),
    );
    let panes = match (&kitty.panes, &wezterm.panes) {
        (Fact::Known(a), Fact::Known(b)) => Fact::Known(a.iter().chain(b).cloned().collect()),
        _ => Fact::Unknown,
    };
    let terminals = support(Probed {
        automation_denied,
        kitty: Some(kitty.ability),
        wezterm: Some(wezterm.ability),
    });
    HostView {
        surfaces: Surfaces {
            terminals: Fact::Known(terminals),
            panes,
            tmux,
            // No window list on macOS.
            windows: Fact::Known(Vec::new()),
        },
        kitty,
        wezterm,
    }
}

/// Socket files in `dirs` whose name gives an owner pid (`owner`) that is a
/// live process of `exe` belonging to `uid`. `None` when a directory could
/// not be read (it might hold one).
pub(crate) fn sockets<S: System>(
    sys: &S,
    dirs: &[String],
    table: &[ProcessInfo],
    uid: u32,
    exe: session_navigation::evidence::Exe,
    owner: fn(&str) -> Option<u32>,
) -> Option<Vec<(String, u32)>> {
    let mut out = Vec::new();
    for dir in dirs {
        let entries = match sys.list_dir(dir) {
            Ok(entries) => entries,
            Err(crate::system::FsError::NotFound) => continue,
            Err(_) => return None,
        };
        for entry in entries {
            let Some(pid) = owner(&entry) else {
                continue;
            };
            let live = table
                .iter()
                .any(|p| p.identity.pid == pid && p.exe == exe && p.ruid == uid);
            if live {
                out.push((crate::system::join(dir, &entry), pid));
            }
        }
    }
    Some(out)
}

/// Whether any process of `exe` runs for `uid`.
pub(crate) fn running(
    table: &[ProcessInfo],
    uid: u32,
    exe: session_navigation::evidence::Exe,
) -> bool {
    table.iter().any(|p| p.exe == exe && p.ruid == uid)
}
