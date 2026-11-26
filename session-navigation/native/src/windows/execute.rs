//! [`Execute`] for Windows. Each step is re-validated here, whatever
//! `decide` already checked: a window raise by a fresh process table in
//! Rust and again at the moment of acting (by the Win32 focus, or the focus
//! script), launches by program, directory, environment and quoting. The
//! Win32 fast path ([`fast`]) is used when the system has it; a call that
//! fails before acting falls back to the PowerShell script. Data reaches
//! PowerShell only as JSON on stdin.
use serde::Deserialize;

use session_navigation::evidence::ProcessInfo;
use session_navigation::hosts::{PaneRef, WindowId};
use session_navigation::model::{BootId, ProcessIdentity, Reason, TerminalKind};
use session_navigation::plan::{Command, Dispatch, LaunchContext, Step};
use session_navigation::ports::Execute;

use super::classify::is_agent_cli;
use super::config::{WinRuntime, WindowsConfig};
use super::launch;
use super::report::Report;
use super::{fast, powershell, scripts, wsl};
use crate::system::{Output, Run, System};

pub struct WindowsExecutor<S> {
    sys: S,
    cfg: WindowsConfig,
}

#[derive(Deserialize)]
struct Status {
    status: String,
}

/// The identity re-check run (boot and table, no window helper).
pub fn identity_run(cfg: &WindowsConfig) -> Run {
    powershell::run(cfg, scripts::IDENTITY, None, cfg.script_timeout)
}

/// The focus run for `hwnd` owned by `owner`, holding `process`.
pub fn focus_run(
    cfg: &WindowsConfig,
    hwnd: WindowId,
    owner: &ProcessIdentity,
    process: &ProcessIdentity,
) -> Run {
    let input = serde_json::json!({
        "hwnd": hwnd.0,
        "owner": owner.pid,
        "ownerStart": owner.start,
        "process": process.pid,
        "processStart": process.start,
    });
    powershell::run(
        cfg,
        scripts::FOCUS,
        Some(&input),
        cfg.common.timeouts.script_act,
    )
}

/// The launch run for a validated spec.
pub fn launch_run(cfg: &WindowsConfig, spec: &launch::LaunchSpec) -> Run {
    let input = serde_json::to_value(spec).unwrap_or(serde_json::Value::Null);
    powershell::run(
        cfg,
        scripts::LAUNCH,
        Some(&input),
        cfg.common.timeouts.script_act,
    )
}

/// Terminals whose windows this adapter raises.
fn raisable(terminal: TerminalKind) -> bool {
    matches!(
        terminal,
        TerminalKind::WindowsTerminal
            | TerminalKind::ConsoleHost
            | TerminalKind::WezTerm
            | TerminalKind::Alacritty
    )
}

fn status<S: System>(sys: &S, run: &Run) -> Option<String> {
    powershell::json::<S, Status>(sys, run).map(|s| s.status)
}

impl<S: System> WindowsExecutor<S> {
    pub fn new(sys: S, cfg: WindowsConfig) -> Self {
        Self { sys, cfg }
    }

    /// A fresh native table and boot, or `None` when unreadable.
    fn native_table(&self) -> Option<(BootId, Vec<ProcessInfo>)> {
        if let Some(found) = self.sys.win32().and_then(fast::identity) {
            return Some(found);
        }
        let report: Report = powershell::json(&self.sys, &identity_run(&self.cfg))?;
        Some((report.boot()?, report.table()?))
    }

    /// Both identities must still be live in this boot, exactly.
    fn recheck(&self, owner: &ProcessIdentity, process: &ProcessIdentity) -> Result<(), Reason> {
        let (boot, table) = self.native_table().ok_or(Reason::EvidenceIncomplete)?;
        let live =
            |id: &ProcessIdentity| id.boot == boot && table.iter().any(|p| p.identity == *id);
        if live(owner) && live(process) {
            Ok(())
        } else {
            Err(Reason::TargetGone)
        }
    }

    /// Raise one top-level window: re-check, then the focus script, which
    /// re-checks the window's owner itself and reads the foreground back.
    fn focus(&self, terminal: TerminalKind, pane: &PaneRef, process: &ProcessIdentity) -> Dispatch {
        let PaneRef::WindowsWindow { hwnd, owner } = pane else {
            return Dispatch::Failed(Reason::HostUnsupported);
        };
        if !raisable(terminal) || self.cfg.runtime != WinRuntime::Native {
            return Dispatch::Failed(Reason::HostUnsupported);
        }
        if hwnd.0 == 0 || i64::try_from(hwnd.0).is_err() {
            return Dispatch::Failed(Reason::TargetGone);
        }
        if let Err(reason) = self.recheck(owner, process) {
            return Dispatch::Failed(reason);
        }
        if let Some(win) = self.sys.win32() {
            // An error means nothing was touched: the script tries instead.
            if let Ok(result) = win.focus(&fast::focus_request(*hwnd, owner, process)) {
                return fast::focus_dispatch(result);
            }
        }
        match status(&self.sys, &focus_run(&self.cfg, *hwnd, owner, process)).as_deref() {
            Some("focused") => Dispatch::Dispatched,
            Some("gone") => Dispatch::Failed(Reason::TargetGone),
            // The foreground lock refused it, or it did not stick.
            Some("refused" | "unconfirmed") => Dispatch::Failed(Reason::DispatchFailed),
            _ => Dispatch::Failed(Reason::DispatchFailed),
        }
    }

    fn launch(
        &self,
        terminal: TerminalKind,
        command: &Command,
        context: &LaunchContext,
    ) -> Dispatch {
        if !matches!(
            terminal,
            TerminalKind::WindowsTerminal | TerminalKind::ConsoleHost
        ) {
            return Dispatch::Failed(Reason::HostUnsupported);
        }
        let spec = match launch::spec(&self.sys, &self.cfg, terminal, command, context) {
            Ok(spec) => spec,
            Err(launch::LaunchError::Runtime) => {
                return Dispatch::Failed(Reason::PlatformUnsupported)
            }
            Err(_) => return Dispatch::Failed(Reason::DispatchFailed),
        };
        if let Some(dispatch) = self
            .sys
            .win32()
            .and_then(|win| fast::launch(win, &self.cfg, terminal, &spec))
        {
            return dispatch;
        }
        match status(&self.sys, &launch_run(&self.cfg, &spec)).as_deref() {
            Some("dispatched") => Dispatch::Dispatched,
            _ => Dispatch::Failed(Reason::DispatchFailed),
        }
    }

    /// Natively there is no graceful stop: Windows has no SIGTERM, a
    /// console's Ctrl+C reaches every process attached to it, and
    /// `TerminateProcess` gives the agent no chance to save. So a native
    /// stop is `HostUnsupported` and runs nothing. Inside WSL: `kill -TERM`
    /// only to an agent CLI with the exact identity, started by the
    /// distribution user the probe runs as.
    fn stop(&self, process: &ProcessIdentity) -> Dispatch {
        let WinRuntime::Wsl { distro, .. } = &self.cfg.runtime else {
            return Dispatch::Failed(Reason::HostUnsupported);
        };
        let Some(table) = wsl::table(&self.sys, &self.cfg, distro) else {
            return Dispatch::Failed(Reason::TargetGone);
        };
        let ours = table.processes.iter().any(|p| {
            p.identity == *process
                && is_agent_cli(p.exe)
                && p.ruid == table.uid
                && p.identity.pid > 1
        });
        if !ours {
            return Dispatch::Failed(Reason::TargetGone);
        }
        let run = wsl::kill_run(
            &self.cfg,
            distro,
            process.pid,
            self.cfg.common.timeouts.dispatch,
        );
        match self.sys.run(&run) {
            Ok(output) if output.success() => Dispatch::Dispatched,
            Ok(Output { .. }) | Err(_) => Dispatch::Failed(Reason::DispatchFailed),
        }
    }
}

impl<S: System> Execute for WindowsExecutor<S> {
    fn execute(&mut self, step: &Step) -> Dispatch {
        match step {
            Step::FocusPane {
                terminal,
                pane,
                process,
            } => self.focus(*terminal, pane, process),
            Step::Launch {
                terminal,
                command,
                context,
            } => self.launch(*terminal, command, context),
            Step::Stop { process } => self.stop(process),
            // No provider desktop app (and so no URL handler) is modelled
            // on Windows.
            Step::OpenUrl { .. } => Dispatch::Failed(Reason::PlatformUnsupported),
            // Activating an app without choosing a window is not built.
            Step::ActivateApp { .. } => Dispatch::Failed(Reason::HostUnsupported),
            // The engine observes the exit itself.
            Step::AwaitExit { .. } => Dispatch::Dispatched,
        }
    }
}
