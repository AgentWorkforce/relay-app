//! [`Execute`] for a real Mac. Each step is re-validated here, whatever
//! `decide` already checked: links by scheme and character set, focus and
//! stop by a fresh read of the process table, launches by program, cwd and
//! environment. Data reaches AppleScript only as `on run argv` arguments.
mod focus;
mod launch;
mod tmux;

use session_navigation::evidence::ProcessInfo;
use session_navigation::model::{
    App, BootId, Platform, ProcessIdentity, Reason, Runtime, TerminalKind,
};
use session_navigation::plan::{Dispatch, LaunchContext, Step};
use session_navigation::ports::Execute;

use super::config::MacConfig;
use super::machine::{self, is_tty};
use super::parse::{self, ps};
use super::scripts;
use crate::system::{Output, Run, RunError, System};

pub use launch::{iterm_command, launch_args, open_args, terminal_command, LaunchError, ENV};

pub const OPEN: &str = "/usr/bin/open";
pub const KILL: &str = "/bin/kill";

pub struct MacExecutor<S> {
    sys: S,
    cfg: MacConfig,
}

/// A provider URL `open` may receive: `claude://` or `codex://`, made only of
/// the characters the core's URL builders emit (unreserved, `%XX`, and
/// `:/?=&`), no longer than 2 KiB.
pub fn is_dispatchable_url(url: &str) -> bool {
    let scheme_ok = url.starts_with("claude://") || url.starts_with("codex://");
    let chars_ok = url.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'~' | b'%' | b':' | b'/' | b'?' | b'=' | b'&'
            )
    });
    scheme_ok && chars_ok && url.len() <= 2048
}

/// The app a scheme must be opened with.
fn scheme_app(url: &str) -> Option<App> {
    if url.starts_with("claude://") {
        Some(App::ClaudeDesktop)
    } else if url.starts_with("codex://") {
        Some(App::CodexDesktop)
    } else {
        None
    }
}

/// Map a finished `osascript` run to a dispatch.
fn script_outcome(result: Result<Output, RunError>) -> Result<String, Dispatch> {
    let output = result.map_err(|_| Dispatch::Failed(Reason::DispatchFailed))?;
    if output.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    Err(match parse::osascript_error(&output.stderr) {
        Some(scripts::NOT_AUTHORIZED) => Dispatch::Refused,
        Some(scripts::PANE_UNAVAILABLE) => Dispatch::Failed(Reason::TargetGone),
        Some(scripts::PANE_UNCONFIRMED) => Dispatch::Failed(Reason::WrongDestination),
        _ => Dispatch::Failed(Reason::DispatchFailed),
    })
}

impl<S: System> MacExecutor<S> {
    pub fn new(sys: S, cfg: MacConfig) -> Self {
        Self { sys, cfg }
    }

    fn dispatch(&self, run: Run) -> Dispatch {
        match self
            .sys
            .run(&run.timeout(self.cfg.common.timeouts.dispatch))
        {
            Ok(output) if output.success() => Dispatch::Dispatched,
            _ => Dispatch::Failed(Reason::DispatchFailed),
        }
    }

    /// A fresh table and the current boot, or `None` when unreadable.
    fn current(&self) -> Option<(BootId, Vec<ProcessInfo>)> {
        let boot = BootId(machine::boot(&self.sys, &self.cfg)?);
        Some((boot, machine::table(&self.sys, &self.cfg, boot)?))
    }

    /// The live process with exactly this identity (pid, start and boot).
    fn find(&self, identity: &ProcessIdentity) -> Result<ProcessInfo, Reason> {
        self.fresh(identity).map(|(_, p)| p)
    }

    /// A fresh table and the live process with exactly this identity.
    fn fresh(&self, identity: &ProcessIdentity) -> Result<(Vec<ProcessInfo>, ProcessInfo), Reason> {
        let (boot, table) = self.current().ok_or(Reason::EvidenceIncomplete)?;
        if boot != identity.boot {
            return Err(Reason::TargetGone);
        }
        let found = table
            .iter()
            .find(|p| p.identity == *identity)
            .cloned()
            .ok_or(Reason::TargetGone)?;
        Ok((table, found))
    }

    fn open_url(&self, url: &str, bundle: &str) -> Dispatch {
        let expected = scheme_app(url).map(App::bundle_id);
        if !is_dispatchable_url(url) || expected != Some(bundle) {
            return Dispatch::Failed(Reason::DispatchFailed);
        }
        self.dispatch(Run::new(OPEN, &["-b", bundle, url]))
    }

    /// Only native launches without a Windows Terminal profile: scripted in
    /// Ghostty, Terminal and iTerm2, through `open` in Kitty, WezTerm and
    /// Alacritty.
    fn launch(
        &self,
        terminal: TerminalKind,
        command: &session_navigation::plan::Command,
        context: &LaunchContext,
    ) -> Dispatch {
        if context.runtime != Runtime::Native || context.profile.is_some() {
            return Dispatch::Failed(Reason::PlatformUnsupported);
        }
        let script = match terminal {
            TerminalKind::Ghostty => scripts::GHOSTTY_LAUNCH,
            TerminalKind::AppleTerminal => scripts::TERMINAL_LAUNCH,
            TerminalKind::ITerm2 => scripts::ITERM_LAUNCH,
            TerminalKind::Kitty | TerminalKind::WezTerm | TerminalKind::Alacritty => {
                return match launch::open_args(
                    &self.sys,
                    terminal,
                    command,
                    &self.cfg.hosts.env_program,
                ) {
                    Ok(args) => self.dispatch(Run::with_args(OPEN, args)),
                    Err(_) => Dispatch::Failed(Reason::DispatchFailed),
                };
            }
            _ => return Dispatch::Failed(Reason::HostUnsupported),
        };
        let args = match launch::launch_args(&self.sys, terminal, command) {
            Ok(args) => args,
            Err(_) => return Dispatch::Failed(Reason::DispatchFailed),
        };
        let run = machine::script_run(script, &args, self.cfg.common.timeouts.script_act);
        match script_outcome(self.sys.run(&run)) {
            Ok(out) if out == "dispatched" => Dispatch::Dispatched,
            Ok(out) => match out.strip_prefix("tty:") {
                Some(tty) if is_tty(tty) => Dispatch::Opened {
                    tty: tty.to_owned(),
                },
                // A window opened but its tty is not one we recognise.
                _ => Dispatch::Dispatched,
            },
            Err(dispatch) => dispatch,
        }
    }

    /// Signal only an agent CLI the person started, by exact identity.
    fn stop(&self, process: &ProcessIdentity) -> Dispatch {
        let target = match self.find(process) {
            Ok(p) => p,
            Err(_) => return Dispatch::Failed(Reason::TargetGone),
        };
        let ours =
            ps::is_agent_cli(target.exe) && target.ruid == self.cfg.uid && target.identity.pid > 1;
        if !ours {
            return Dispatch::Failed(Reason::TargetGone);
        }
        let pid = target.identity.pid.to_string();
        self.dispatch(Run::new(KILL, &["-TERM", &pid]))
    }
}

impl<S: System> Execute for MacExecutor<S> {
    fn execute(&mut self, step: &Step) -> Dispatch {
        match step {
            Step::OpenUrl { url, bundle } => self.open_url(url, bundle),
            Step::ActivateApp { app } => match app.handler(Platform::MacOs) {
                Some(bundle) => self.dispatch(Run::new(OPEN, &["-b", bundle])),
                None => Dispatch::Failed(Reason::PlatformUnsupported),
            },
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
            // The engine observes the exit itself.
            Step::AwaitExit { .. } => Dispatch::Dispatched,
        }
    }
}
