//! What this adapter can do with each Linux terminal right now, and the
//! helper programs it found. Reported, never assumed from a name.
//!
//! | Terminal | focus_pane | raise_window | launch | tabs share a window |
//! |---|---|---|---|---|
//! | Kitty | API when an instance answered `ls`, else `NeedsSetting` | X11 with xdotool + wmctrl | `kitty --directory` | yes |
//! | WezTerm | API when a GUI answered `cli list`, else `NeedsSetting` | same | `wezterm start --cwd` | yes |
//! | Ghostty, Alacritty, GNOME Terminal, Konsole, xterm | `NoPaneApi` | same | own argv form | Ghostty, GNOME Terminal, Konsole: yes |
//!
//! `activate_app` is `Unsupported` everywhere (`Wayland` on Wayland,
//! `NotImplemented` elsewhere): Linux has no generic "bring this app
//! forward" that does not pick a window. Ghostty on Linux (GTK) has tabs,
//! so its raised window may show another tab: `tabs_share_window` is true
//! here, unlike the simulator's `tabbed` (which models Ghostty without
//! tabs).
use std::collections::BTreeMap;

use session_navigation::evidence::Fact;
use session_navigation::hosts::{Ability, Setting, TerminalSupport, Unsupported};
use session_navigation::model::{DisplayServer, TerminalKind};

use super::apps::{launcher, linux_terminals, which};
use super::config::LinuxConfig;
use super::panes::Apis;
use super::x11::Helpers;
use crate::system::System;

/// `env`, for setting a launched program's environment as argv words
/// (and, for xterm, its directory with `-C`).
pub const ENV: &str = "env";
/// `kill`, for `Stop`.
pub const KILL: &str = "kill";

/// Every helper program, resolved on the configured program path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub launchers: BTreeMap<TerminalKind, String>,
    pub x11: Helpers,
    pub env: Option<String>,
    pub kill: Option<String>,
}

impl Resolved {
    pub fn find<S: System>(sys: &S, cfg: &LinuxConfig) -> Self {
        let launchers = linux_terminals()
            .into_iter()
            .filter_map(|t| Some((t, which(sys, &cfg.program_path, launcher(t)?)?)))
            .collect();
        Self {
            launchers,
            x11: Helpers::find(sys, cfg),
            env: which(sys, &cfg.program_path, ENV),
            kill: which(sys, &cfg.program_path, KILL),
        }
    }

    pub fn launcher(&self, terminal: TerminalKind) -> Option<&str> {
        self.launchers.get(&terminal).map(String::as_str)
    }
}

fn tabs_share_window(terminal: TerminalKind) -> bool {
    !matches!(terminal, TerminalKind::Alacritty | TerminalKind::Xterm)
}

/// The `TerminalSupport` of every Linux terminal.
pub fn terminals(
    display: Fact<DisplayServer>,
    resolved: &Resolved,
    apis: &Apis,
) -> Vec<TerminalSupport> {
    let no = Ability::Unsupported;
    let raise_window = match display {
        Fact::Known(DisplayServer::X11) if resolved.x11.can_raise() => Ability::Available,
        Fact::Known(DisplayServer::Wayland) => no(Unsupported::Wayland),
        _ => no(Unsupported::NotImplemented),
    };
    let activate_app = match display {
        Fact::Known(DisplayServer::Wayland) => no(Unsupported::Wayland),
        _ => no(Unsupported::NotImplemented),
    };
    linux_terminals()
        .into_iter()
        .map(|terminal| {
            let installed = resolved.launcher(terminal).is_some();
            let focus_pane = match terminal {
                TerminalKind::Kitty if !apis.kitty.is_empty() => Ability::Available,
                TerminalKind::Kitty if installed => {
                    Ability::NeedsSetting(Setting::KittyRemoteControl)
                }
                TerminalKind::WezTerm if !apis.wezterm.is_empty() => Ability::Available,
                TerminalKind::WezTerm if installed => {
                    Ability::NeedsSetting(Setting::WezTermMuxSocket)
                }
                TerminalKind::Kitty | TerminalKind::WezTerm => no(Unsupported::NotImplemented),
                _ => no(Unsupported::NoPaneApi),
            };
            // A window needs a display to open on, and xterm a directory
            // changer (`env -C`).
            let launch = match (display, installed) {
                (Fact::Known(_), true)
                    if terminal != TerminalKind::Xterm || resolved.env.is_some() =>
                {
                    Ability::Available
                }
                _ => no(Unsupported::NotImplemented),
            };
            TerminalSupport {
                terminal,
                focus_pane,
                raise_window,
                activate_app,
                launch,
                tabs_share_window: tabs_share_window(terminal),
            }
        })
        .collect()
}
