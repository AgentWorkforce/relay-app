//! [`Observe`] for Windows, natively or for one WSL distribution. Boot, the
//! process table, the windows, the foreground, Windows Terminal's package
//! and the default terminal come from direct Win32 calls ([`fast`]) when
//! the system has them, else from one PowerShell run; a WSL session's
//! probes run beside them. Every failure becomes `Fact::Unknown`, never a
//! default.
use std::collections::BTreeMap;

use session_navigation::evidence::{AppInfo, Exe, Fact, Front, ProcessInfo, Snapshot, ToolInfo};
use session_navigation::hosts::{Ability, Surfaces, TerminalSupport, Unsupported};
use session_navigation::model::{App, Platform, SessionRef, TerminalKind};
use session_navigation::parse::is_absolute_path;
use session_navigation::ports::Observe;

use super::config::{WinRuntime, WindowsConfig};
use super::fast::{self, Facts};
use super::report::{self, Report};
use super::{powershell, scripts, under, wsl};
use crate::common::par::{self, Reuse};
use crate::common::tools::{PathStyle, ToolResolver};
use crate::common::{self, reservations, HostHistory, NoHistory};
use crate::system::{FsError, System, Win32};

/// The observe run (one PowerShell process).
pub fn observe_run(cfg: &WindowsConfig) -> crate::system::Run {
    powershell::run(cfg, scripts::OBSERVE, None, cfg.script_timeout)
}

/// The observe report, or `None` when PowerShell failed outright.
pub fn report<S: System>(sys: &S, cfg: &WindowsConfig) -> Option<Report> {
    powershell::json(sys, &observe_run(cfg))
}

/// What this adapter can do with each terminal that exists on Windows.
/// UNVERIFIED (each ability's mechanism has never run): raising a window
/// with `SetForegroundWindow`, launching through `wt.exe` and through a new
/// console.
pub fn terminals() -> Vec<TerminalSupport> {
    let not_yet = Ability::Unsupported(Unsupported::NotImplemented);
    let no_api = Ability::Unsupported(Unsupported::NoPaneApi);
    vec![
        TerminalSupport {
            terminal: TerminalKind::WindowsTerminal,
            // Its session GUID selects nothing.
            focus_pane: no_api,
            raise_window: Ability::Available,
            activate_app: not_yet,
            launch: Ability::Available,
            tabs_share_window: true,
        },
        TerminalSupport {
            terminal: TerminalKind::ConsoleHost,
            focus_pane: no_api,
            raise_window: Ability::Available,
            activate_app: not_yet,
            launch: Ability::Available,
            tabs_share_window: false,
        },
        // Raised as windows like any other; their pane APIs and launch
        // forms are not built for Windows.
        TerminalSupport {
            terminal: TerminalKind::WezTerm,
            focus_pane: not_yet,
            raise_window: Ability::Available,
            activate_app: not_yet,
            launch: not_yet,
            tabs_share_window: true,
        },
        TerminalSupport {
            terminal: TerminalKind::Alacritty,
            focus_pane: no_api,
            raise_window: Ability::Available,
            activate_app: not_yet,
            launch: not_yet,
            tabs_share_window: false,
        },
    ]
}

pub struct WindowsObserver<S, H = NoHistory> {
    sys: S,
    cfg: WindowsConfig,
    history: H,
    tools: ToolResolver,
    /// Facts reused for at most two seconds (see [`fast::Facts`]).
    facts: Facts,
    wsl_tools: Reuse<Vec<ToolInfo>>,
}

impl<S: System> WindowsObserver<S, NoHistory> {
    pub fn new(sys: S, cfg: WindowsConfig) -> Self {
        Self::with_history(sys, cfg, NoHistory)
    }
}

fn running(table: Option<&[ProcessInfo]>, exe: Exe) -> Option<bool> {
    table.map(|t| t.iter().any(|p| p.exe == exe))
}

impl<S: System, H: HostHistory> WindowsObserver<S, H> {
    pub fn with_history(sys: S, cfg: WindowsConfig, history: H) -> Self {
        Self {
            sys,
            cfg,
            history,
            tools: ToolResolver::new(),
            facts: Facts::default(),
            wsl_tools: Reuse::default(),
        }
    }

    pub fn config(&self) -> &WindowsConfig {
        &self.cfg
    }

    /// Whether a file exists: `Some(false)` only for a positive not-found.
    fn exists(&self, path: &str) -> Option<bool> {
        match self.sys.metadata(path) {
            Ok(meta) => Some(meta.is_file),
            Err(FsError::NotFound) => Some(false),
            Err(_) => None,
        }
    }

    /// One row per app that exists on Windows: Windows Terminal from its
    /// package, the console host and WezTerm/Alacritty from their default
    /// files; running from the native table.
    fn apps(&self, report: Option<&Report>, table: Option<&[ProcessInfo]>) -> Fact<Vec<AppInfo>> {
        let Some(wt) = report.and_then(|r| r.terminal.as_ref()) else {
            return Fact::Unknown;
        };
        let pf = &self.cfg.program_files;
        let files = [
            (App::ConsoleHost, Exe::ConsoleHost, self.cfg.conhost()),
            (
                App::WezTerm,
                Exe::WezTerm,
                under(pf, "WezTerm\\wezterm-gui.exe"),
            ),
            (
                App::Alacritty,
                Exe::Alacritty,
                under(pf, "Alacritty\\alacritty.exe"),
            ),
        ];
        let Some(wt_running) = running(table, Exe::WindowsTerminal) else {
            return Fact::Unknown;
        };
        let mut out = vec![AppInfo {
            app: App::WindowsTerminal,
            installed: wt.installed,
            version: wt.version.clone().filter(|_| wt.installed),
            running: wt_running,
        }];
        for (app, exe, path) in files {
            let (Some(found), Some(running)) = (self.exists(&path), running(table, exe)) else {
                return Fact::Unknown;
            };
            out.push(AppInfo {
                app,
                installed: found || running,
                version: None,
                running,
            });
        }
        Fact::Known(out)
    }

    fn front(&self, report: Option<&Report>, table: Option<&[ProcessInfo]>) -> Front {
        let (app, window) = report::front(table, report.and_then(|r| r.foreground.as_ref()));
        Front {
            app,
            // No tty exists on Windows.
            pane_tty: Fact::Known(None),
            // No pane API is driven on Windows.
            pane: Fact::Known(None),
            window,
            // No provider desktop app is modelled on Windows.
            view: Fact::Known(None),
        }
    }

    /// Whether the recorded cwd exists in this runtime (WSL: through the
    /// distribution's share).
    fn cwd_exists(&self, cwd: Option<&str>) -> Fact<bool> {
        match &self.cfg.runtime {
            WinRuntime::Native => common::cwd_exists(&self.sys, cwd),
            WinRuntime::Wsl { distro, .. } => match cwd {
                Some(cwd) if is_absolute_path(cwd) => {
                    match self.sys.metadata(&self.cfg.unc(distro, cwd)) {
                        Ok(meta) => Fact::Known(meta.is_dir),
                        Err(FsError::NotFound) => Fact::Known(false),
                        Err(_) => Fact::Unknown,
                    }
                }
                _ => Fact::Unknown,
            },
        }
    }
}

/// The native report: the Win32 fast path, else the PowerShell script.
fn native_report<S: System>(sys: &S, cfg: &WindowsConfig, facts: &mut Facts) -> Option<Report> {
    sys.win32()
        .and_then(|win: &dyn Win32| fast::report(win, facts))
        .or_else(|| report(sys, cfg))
}

/// A WSL distribution's table, unless it is positively not installed
/// (`wsl.exe -d` would only fail); an unreadable list does not stop it.
fn wsl_table<S: System>(sys: &S, cfg: &WindowsConfig, distro: &str) -> Option<wsl::WslTable> {
    let installed = sys
        .run(&wsl::list_run(cfg))
        .ok()
        .filter(crate::system::Output::success)
        .and_then(|o| wsl::parse_list(&o.stdout))
        .is_none_or(|l| l.iter().any(|d| d.eq_ignore_ascii_case(distro)));
    installed.then(|| wsl::table(sys, cfg, distro)).flatten()
}

/// The distribution's tools; a probe that answered is reused (2 s).
fn wsl_tools<S: System>(
    sys: &S,
    cfg: &WindowsConfig,
    distro: &str,
    reuse: &mut Reuse<Vec<ToolInfo>>,
) -> Vec<ToolInfo> {
    reuse.get_or_keep(
        || wsl::tools(sys, cfg, distro),
        |tools| *tools != wsl::unknown_tools(),
    )
}

impl<S: System + Sync, H: HostHistory> Observe for WindowsObserver<S, H> {
    fn observe(&mut self, session: &SessionRef) -> Snapshot {
        let observed_at = self.sys.now();
        let (sys, cfg) = (&self.sys, &self.cfg);
        // A WSL session's probes run beside the native reads.
        let (report, wsl) = match &cfg.runtime {
            WinRuntime::Native => (native_report(sys, cfg, &mut self.facts), None),
            WinRuntime::Wsl { distro, .. } => {
                let (facts, tools) = (&mut self.facts, &mut self.wsl_tools);
                let (report, table, tools) = par::join3(
                    sys,
                    || native_report(sys, cfg, facts),
                    || wsl_table(sys, cfg, distro),
                    || wsl_tools(sys, cfg, distro, tools),
                );
                (report, Some((table, tools)))
            }
        };
        let native = report.as_ref().and_then(Report::table);
        let native_table = native.as_deref();
        let windows = match (
            report.as_ref().and_then(|r| r.windows.clone()),
            native_table,
        ) {
            (Some(raw), Some(table)) => Fact::Known(report::windows(table, raw.into_vec())),
            _ => Fact::Unknown,
        };
        // The session's own runtime: its table, boot and tools.
        let (boot, boot_time, processes, tools, tmux) = match &self.cfg.runtime {
            WinRuntime::Native => (
                report.as_ref().and_then(Report::boot),
                report.as_ref().and_then(Report::boot_time),
                native.clone(),
                self.tools
                    .all_in(&self.sys, &self.cfg.common, PathStyle::Windows),
                // No tmux runs natively on Windows.
                Fact::Known(Vec::new()),
            ),
            WinRuntime::Wsl { .. } => {
                let (table, tools) = wsl.unwrap_or_else(|| (None, wsl::unknown_tools()));
                (
                    table.as_ref().map(|t| t.boot),
                    table.as_ref().map(|t| t.boot_time),
                    table.map(|t| t.processes),
                    tools,
                    // tmux inside the distribution is not read yet.
                    Fact::Unknown,
                )
            }
        };
        let processes = processes.map_or(Fact::Unknown, Fact::Known);
        let common = &self.cfg.common;
        let found = common::observe_provider(&self.sys, common, session, boot_time, &processes);
        Snapshot {
            session: session.clone(),
            observed_at,
            platform: Platform::Windows,
            runtime: Fact::Known(self.cfg.core_runtime()),
            boot: boot.map_or(Fact::Unknown, Fact::Known),
            processes,
            front: self.front(report.as_ref(), native_table),
            apps: self.apps(report.as_ref(), native_table),
            tools,
            // No provider desktop app is modelled on Windows, so no handler
            // could ever be used (as on Linux).
            schemes: Fact::Known(Vec::new()),
            surfaces: Surfaces {
                terminals: Fact::Known(terminals()),
                panes: Fact::Known(Vec::new()),
                tmux,
                windows,
            },
            last_seen_host: self.history.last_seen(session),
            original_host: self.history.original(session),
            terminal_profile: self.history.terminal_profile(session),
            default_terminal: report::default_terminal(
                report.as_ref().and_then(|r| r.delegation.as_ref()),
            ),
            reservation: reservations::read(&self.sys, &common.paths.reservation_dir, session),
            cwd_exists: self.cwd_exists(found.cwd.as_deref()),
            cwd: found.cwd.clone().map_or(Fact::Unknown, Fact::Known),
            // Only the configured default homes are searched.
            environment: if found.found {
                Fact::Known(BTreeMap::new())
            } else {
                Fact::Unknown
            },
            capabilities: Vec::new(),
            provider: found.evidence,
        }
    }
}
