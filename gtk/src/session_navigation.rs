//! Native session host adapters. Plans choose policy; adapters prove the target
//! again immediately before dispatch and never replace a failed focus with resume.
use agent_relay_desktop_core::session_locations::PROVIDER_ENVIRONMENT_KEYS;
use agent_relay_desktop_core::session_locations::{
    SessionLocation, SessionOpenAction, SessionOpenPlan, SessionResumeCommand,
};

mod focus;
mod native;
mod resume;
use focus::focus;
#[cfg(test)]
use focus::pane_matches;
use native::Native;
pub use resume::resume_invocation;
#[cfg(test)]
use resume::windows_argument;
#[cfg(all(test, windows))]
use resume::windows_environment_program;
#[cfg(test)]
use std::process::Command;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The platform accepted a request; the provider hasn't acknowledged selection.
    Dispatched,
    /// The platform returned the exact selected target after activation.
    Verified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub environment: Vec<(String, String)>,
    pub clear_provider_environment: bool,
}
impl Invocation {
    fn new(executable: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            executable: executable.into(),
            args: args.into_iter().collect(),
            cwd: None,
            environment: vec![],
            clear_provider_environment: false,
        }
    }
}

/// Window discovery never uses a caption or a working directory. Captured
/// handles are checked against the current owner; otherwise only one visible
/// window in the verified process is eligible for Show application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeWindow {
    pub id: usize,
    pub pid: i32,
}
pub fn choose_window(
    location: &SessionLocation,
    windows: &[NativeWindow],
) -> Result<usize, String> {
    let pid = location
        .host_pid
        .ok_or("No application process identity was recorded.")?;
    let candidates: Vec<_> = windows.iter().filter(|window| window.pid == pid).collect();
    if let Some(id) = location.window_id.as_deref() {
        let id = id
            .strip_prefix("0x")
            .map(|s| usize::from_str_radix(s, 16))
            .unwrap_or_else(|| id.parse())
            .map_err(|_| "Invalid native window identity.")?;
        return candidates
            .iter()
            .find(|window| window.id == id)
            .map(|window| window.id)
            .ok_or_else(|| "The original window closed or changed owner.".into());
    }
    if candidates.len() == 1 {
        Ok(candidates[0].id)
    } else {
        Err("The original application has no unique visible window. Exact window selection is unavailable.".into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessEvidence {
    pub pid: i32,
    pub uid: u32,
    pub start: u64,
    pub executable: Option<String>,
    pub zombie: bool,
}
pub fn process_evidence_matches(
    location: &SessionLocation,
    owner: Option<&ProcessEvidence>,
    host: Option<&ProcessEvidence>,
    uid: u32,
    boot: Option<&str>,
) -> bool {
    let (Some(owner), Some(host)) = (owner, host) else {
        return false;
    };
    owner.pid == location.pid
        && owner.uid == uid
        && owner.start == location.process_start
        && !owner.zombie
        && location.host_pid == Some(host.pid)
        && host.uid == uid
        && location.host_process_start == Some(host.start)
        && !host.zombie
        && location
            .host_executable
            .as_deref()
            .is_some_and(|expected| host.executable.as_deref() == Some(expected))
        && location
            .executable
            .as_deref()
            .is_none_or(|expected| owner.executable.as_deref() == Some(expected))
        && location
            .boot_id
            .as_deref()
            .is_none_or(|expected| boot == Some(expected))
}

pub trait Host {
    fn default_terminal(&self) -> Option<String> {
        Some(
            if cfg!(windows) {
                "wt.exe"
            } else {
                "xdg-terminal-exec"
            }
            .into(),
        )
    }
    fn process_matches(&self, location: &SessionLocation) -> bool;
    fn run(&self, command: &Invocation) -> Result<String, String>;
    fn launch(&self, command: &Invocation) -> Result<(), String>;
    fn dispatch(&self, command: &Invocation) -> Result<(), String> {
        self.run(command).map(|_| ())
    }
    fn window(&self, location: &SessionLocation) -> Result<Outcome, String>;
}

pub fn execute(plan: &SessionOpenPlan) -> Result<Outcome, String> {
    execute_with(plan, &Native)
}

pub fn execute_with(plan: &SessionOpenPlan, host: &dyn Host) -> Result<Outcome, String> {
    match plan.action {
        SessionOpenAction::Focus => focus(
            host,
            plan.location
                .as_ref()
                .ok_or("The live target is missing.")?,
        ),
        SessionOpenAction::ShowApplication => {
            let location = plan
                .location
                .as_ref()
                .ok_or("The application target is missing.")?;
            verify(host, location)?;
            host.window(location)
        }
        SessionOpenAction::OpenLink => {
            if let Some(location) = &plan.location {
                verify(host, location)?;
            }
            let link = plan.url.as_deref().ok_or("The session link is missing.")?;
            let url = url::Url::parse(link).map_err(|_| "The session link is invalid.")?;
            if !matches!(url.scheme(), "codex" | "claude") {
                return Err("The session link is unsupported.".into());
            }
            #[cfg(windows)]
            return windows::open_link(link);
            #[cfg(not(windows))]
            {
                host.dispatch(&Invocation::new(
                    if cfg!(target_os = "macos") {
                        "open"
                    } else {
                        "xdg-open"
                    },
                    [link.into()],
                ))?;
                Ok(Outcome::Dispatched)
            }
        }
        SessionOpenAction::Resume => {
            if cfg!(all(windows, not(target_arch = "x86_64")))
                && plan.command.as_ref().is_some_and(|command| {
                    command.args.first().is_some_and(|arg| arg == "--desktop")
                })
            {
                return Err("Claude Desktop handoff requires x64 Windows. Choose an explicit terminal resume instead.".into());
            }
            let default_host;
            let location = if let Some(location) = plan.location.as_ref() {
                Some(location)
            } else {
                default_host = SessionLocation {
                    host_executable: Some(
                        host.default_terminal()
                            .ok_or("No supported default terminal is installed.")?,
                    ),
                    ..Default::default()
                };
                Some(&default_host)
            };
            let invocation = resume_invocation(
                plan.command
                    .as_ref()
                    .ok_or("The resume command is missing.")?,
                location,
                cfg!(windows),
            )?;
            host.launch(&invocation)?;
            Ok(Outcome::Dispatched)
        }
        _ => Err(plan
            .reason
            .clone()
            .unwrap_or_else(|| "Choose a session location before opening it.".into())),
    }
}

fn verify(host: &dyn Host, location: &SessionLocation) -> Result<(), String> {
    if host.process_matches(location) {
        Ok(())
    } else {
        Err("The original session process or application changed. Refresh and try again.".into())
    }
}
fn numeric(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit())
}
