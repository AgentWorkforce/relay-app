//! Launch validation and the `Start-Process` request a launch becomes.
//!
//! | Terminal | Native | WSL |
//! |---|---|---|
//! | console host | `Start-Process <program> <args>` in `cwd` (a console program gets a new console window) | `Start-Process wsl.exe -d <distro> --cd <cwd> --exec [/usr/bin/env K=V…] <program> <args>` |
//! | Windows Terminal | `wt.exe -w 0 new-tab [--profile <id>] --startingDirectory <cwd> -- <program> <args>` | `wt.exe -w 0 new-tab [--profile <id>] -- wsl.exe -d … (as above)` |
//!
//! - `-w 0` joins the most recent Windows Terminal window instead of
//!   leaving it to the person's `windowingBehavior` setting. UNVERIFIED.
//! - `wsl.exe --exec` runs the program without the distribution's shell;
//!   the contract's `wsl.exe … -- <argv>` would hand the joined line to the
//!   default shell, which re-splits and expands it.
//! - A Windows Terminal tab does not inherit the launcher's environment, so
//!   a native launch with provider-home variables runs through the constant
//!   environment wrapper ([`super::scripts::ENV_WRAPPER`]).
//! - Every word is checked before anything runs: control characters are
//!   refused; `.cmd` programs and their arguments must be [`quote::cmd_safe`];
//!   Windows Terminal's must be [`quote::wt_safe`].
use std::collections::BTreeMap;

use session_navigation::model::{Runtime, TerminalKind, Tool};
use session_navigation::parse::{is_absolute_path, is_windows_absolute_path};
use session_navigation::plan::{Command, LaunchContext};

use super::classify::file_name;
use super::config::{WinRuntime, WindowsConfig};
use super::powershell::{self, base64};
use super::quote::{self, QuoteError};
use super::scripts;
use crate::system::System;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// The program is not an absolute path to an existing file.
    Program,
    /// The program is not one of the agent CLIs.
    UnknownProgram,
    /// An environment key outside the tool's allowlist, or a bad value.
    Environment,
    /// The cwd is not an existing absolute directory of the runtime.
    Cwd,
    /// The launch's runtime is not this configuration's.
    Runtime,
    /// A Windows Terminal profile that is not a `{GUID}`, or a profile for
    /// another terminal.
    Profile,
    /// No launch exists here for this terminal.
    Terminal,
    Quote(QuoteError),
}

impl From<QuoteError> for LaunchError {
    fn from(error: QuoteError) -> Self {
        Self::Quote(error)
    }
}

/// What the `LAUNCH` script receives on stdin.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LaunchSpec {
    /// The program `Start-Process` starts.
    pub file: String,
    /// Its whole, quoted argument string.
    pub line: String,
    /// `Start-Process -WorkingDirectory`.
    pub cwd: String,
    /// Set in the launcher's environment first (console host only).
    pub env: BTreeMap<String, String>,
}

/// The agent CLI a program path runs: `claude.exe`, `claude.cmd`, `claude`.
pub fn tool_of(program: &str) -> Option<Tool> {
    let lower = file_name(program).to_ascii_lowercase();
    let stem = lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".cmd"))
        .unwrap_or(&lower);
    Tool::from_executable(stem)
}

/// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`: the shape of `WT_PROFILE_ID`.
pub fn valid_profile(profile: &str) -> bool {
    let Some(inner) = profile.strip_prefix('{').and_then(|p| p.strip_suffix('}')) else {
        return false;
    };
    let groups: Vec<&str> = inner.split('-').collect();
    groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn plain(word: &str) -> bool {
    !word.chars().any(char::is_control)
}

fn check_env(tool: Tool, env: &BTreeMap<String, String>) -> Result<(), LaunchError> {
    for (key, value) in env {
        if !tool.environment_keys().contains(&key.as_str()) || !plain(value) {
            return Err(LaunchError::Environment);
        }
    }
    Ok(())
}

/// Native checks: an existing absolute `C:\…` program that is an agent
/// CLI, an existing absolute directory, allowlisted environment, and words
/// `cmd.exe` cannot reinterpret when the program is a `.cmd` shim.
fn check_native<S: System>(sys: &S, command: &Command) -> Result<(), LaunchError> {
    if !is_windows_absolute_path(&command.program) {
        return Err(LaunchError::Program);
    }
    let tool = tool_of(&command.program).ok_or(LaunchError::UnknownProgram)?;
    if !sys.metadata(&command.program).is_ok_and(|m| m.is_file) {
        return Err(LaunchError::Program);
    }
    check_env(tool, &command.env)?;
    let cwd_ok = is_windows_absolute_path(&command.cwd)
        && sys.metadata(&command.cwd).is_ok_and(|m| m.is_dir);
    if !cwd_ok {
        return Err(LaunchError::Cwd);
    }
    if !command.args.iter().all(|a| plain(a)) {
        return Err(QuoteError::Unquotable.into());
    }
    if quote::is_batch(&command.program)
        && !std::iter::once(&command.program)
            .chain(&command.args)
            .all(|w| quote::cmd_safe(w))
    {
        return Err(QuoteError::CmdUnsafe.into());
    }
    Ok(())
}

/// WSL checks: a POSIX program that is an agent CLI (found by the tools
/// probe inside the distribution), an existing POSIX directory (through the
/// share), allowlisted environment. Returns `wsl.exe`'s arguments.
fn wsl_args<S: System>(
    sys: &S,
    cfg: &WindowsConfig,
    distro: &str,
    command: &Command,
) -> Result<Vec<String>, LaunchError> {
    if !is_absolute_path(&command.program) {
        return Err(LaunchError::Program);
    }
    let tool = Tool::from_executable(&command.program).ok_or(LaunchError::UnknownProgram)?;
    check_env(tool, &command.env)?;
    let cwd_ok = is_absolute_path(&command.cwd)
        && sys
            .metadata(&cfg.unc(distro, &command.cwd))
            .is_ok_and(|m| m.is_dir);
    if !cwd_ok {
        return Err(LaunchError::Cwd);
    }
    if !command.args.iter().all(|a| plain(a)) {
        return Err(QuoteError::Unquotable.into());
    }
    let mut args: Vec<String> = vec![
        "-d".into(),
        distro.into(),
        "--cd".into(),
        command.cwd.clone(),
        "--exec".into(),
    ];
    if !command.env.is_empty() {
        args.push("/usr/bin/env".into());
        args.extend(command.env.iter().map(|(k, v)| format!("{k}={v}")));
    }
    args.push(command.program.clone());
    args.extend(command.args.iter().cloned());
    Ok(args)
}

/// The tab command that sets `env` and runs `program args` in `cwd`.
fn env_wrapper(cfg: &WindowsConfig, command: &Command) -> Result<Vec<String>, LaunchError> {
    let payload = serde_json::json!({
        "file": command.program,
        "line": quote::join_args(&command.args)?,
        "cwd": command.cwd,
        "env": command.env,
    });
    // Only the Base64 alphabet enters the script source.
    let script = format!(
        "$payload='{}'\n{}",
        base64(payload.to_string().as_bytes()),
        scripts::ENV_WRAPPER
    );
    let mut words = vec![cfg.powershell()];
    words.extend(powershell::args(&script));
    Ok(words)
}

/// Validate a launch and turn it into the `LAUNCH` script's input.
pub fn spec<S: System>(
    sys: &S,
    cfg: &WindowsConfig,
    terminal: TerminalKind,
    command: &Command,
    context: &LaunchContext,
) -> Result<LaunchSpec, LaunchError> {
    let wsl = match (&cfg.runtime, &context.runtime) {
        (WinRuntime::Native, Runtime::Native) => None,
        (WinRuntime::Wsl { distro, .. }, Runtime::Wsl { distro: wanted }) if distro == wanted => {
            Some(distro.as_str())
        }
        _ => return Err(LaunchError::Runtime),
    };
    // The child's argv (program first), checked for its runtime.
    let child: Vec<String> = match wsl {
        None => {
            check_native(sys, command)?;
            let mut child = vec![command.program.clone()];
            child.extend(command.args.iter().cloned());
            child
        }
        Some(distro) => {
            let mut child = vec![cfg.wsl_exe()];
            child.extend(wsl_args(sys, cfg, distro, command)?);
            child
        }
    };
    let (first, rest) = child.split_first().ok_or(LaunchError::Program)?;
    match terminal {
        TerminalKind::ConsoleHost => {
            if context.profile.is_some() {
                return Err(LaunchError::Profile);
            }
            Ok(LaunchSpec {
                file: first.clone(),
                line: quote::join_args(rest)?,
                // `wsl.exe --cd` sets the Linux directory itself.
                cwd: if wsl.is_some() {
                    cfg.user_profile.clone()
                } else {
                    command.cwd.clone()
                },
                env: if wsl.is_some() {
                    BTreeMap::new()
                } else {
                    command.env.clone()
                },
            })
        }
        TerminalKind::WindowsTerminal => {
            let mut words: Vec<String> = vec!["-w".into(), "0".into(), "new-tab".into()];
            if let Some(profile) = &context.profile {
                if !valid_profile(profile) {
                    return Err(LaunchError::Profile);
                }
                words.extend(["--profile".to_owned(), profile.clone()]);
            }
            if wsl.is_none() {
                words.extend(["--startingDirectory".to_owned(), command.cwd.clone()]);
            }
            words.push("--".into());
            if wsl.is_none() && !command.env.is_empty() {
                words.extend(env_wrapper(cfg, command)?);
            } else {
                words.extend(child.iter().cloned());
            }
            if !words.iter().all(|w| quote::wt_safe(w)) {
                return Err(QuoteError::WtUnsafe.into());
            }
            Ok(LaunchSpec {
                file: cfg.wt.clone(),
                line: quote::join_args(&words)?,
                cwd: cfg.user_profile.clone(),
                env: BTreeMap::new(),
            })
        }
        _ => Err(LaunchError::Terminal),
    }
}
