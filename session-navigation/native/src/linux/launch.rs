//! Launch validation and each Linux terminal's own argv form. No shell is
//! ever involved: the terminal receives the program and its arguments as
//! separate argv words, the provider environment as `K=V` words of `env`
//! (or Kitty's `-o env=K=V`), and the directory through its own flag.
//!
//! | Terminal | argv after the terminal program | UNVERIFIED, verify with |
//! |---|---|---|
//! | Kitty | `--directory <cwd> [-o env=K=V]… <program> <args…>` | `kitty --directory /tmp -o env=A=1 /usr/bin/env` |
//! | WezTerm | `start --cwd <cwd> -- [env K=V…] <program> <args…>` | `wezterm start --cwd /tmp -- /usr/bin/env A=1 /usr/bin/env` |
//! | Alacritty | `--working-directory <cwd> -e [env K=V…] <program> <args…>` | `alacritty --working-directory /tmp -e /bin/pwd` (add `--hold` by hand to read it) |
//! | GNOME Terminal | `--working-directory=<cwd> -- [env K=V…] <program> <args…>` | `gnome-terminal --working-directory=/tmp -- /bin/bash -c 'pwd; sleep 5'` |
//! | Konsole | `--workdir <cwd> -e [env K=V…] <program> <args…>` | `konsole --workdir /tmp -e /bin/bash -c 'pwd; sleep 5'` |
//! | xterm | `-e <env> -C <cwd> [K=V…] <program> <args…>` (xterm has no directory flag; `env -C` needs GNU coreutils 8.28+) | `xterm -e /usr/bin/env -C /tmp /bin/bash -c 'pwd; sleep 5'` |
//! | Ghostty | `--working-directory=<cwd> -e [env K=V…] <program> <args…>` | `ghostty --working-directory=/tmp -e /bin/bash -c 'pwd; sleep 5'` |
use session_navigation::model::{TerminalKind, Tool};
use session_navigation::parse::is_absolute_path;
use session_navigation::plan::Command;

use crate::common::{kitty, tmux, wezterm};
use crate::system::System;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// The program is not an absolute path to an executable file.
    Program,
    /// The program is neither an agent CLI nor tmux.
    UnknownProgram,
    /// A tmux launch that is not exactly the contract's attach argv.
    NotAnAttach,
    /// An environment key outside the tool's allowlist, or a bad value.
    Environment,
    /// The cwd is not an existing absolute directory.
    Cwd,
    /// A word with a control character.
    Control,
    /// `env` is needed (environment, or xterm's directory) but missing.
    NoEnvProgram,
    /// No argv form for this terminal.
    Terminal,
}

/// Re-check a launch command on this machine.
pub fn check<S: System>(sys: &S, command: &Command) -> Result<Tool, LaunchError> {
    if !is_absolute_path(&command.program) {
        return Err(LaunchError::Program);
    }
    let tool = Tool::from_executable(&command.program).ok_or(LaunchError::UnknownProgram)?;
    if tool == Tool::Tmux && (!tmux::is_attach_args(&command.args) || !command.env.is_empty()) {
        return Err(LaunchError::NotAnAttach);
    }
    let program_ok = sys
        .metadata(&command.program)
        .is_ok_and(|m| m.is_file && m.executable);
    if !program_ok {
        return Err(LaunchError::Program);
    }
    for (key, value) in &command.env {
        let allowed = tool.environment_keys().contains(&key.as_str());
        if !allowed || value.chars().any(char::is_control) {
            return Err(LaunchError::Environment);
        }
    }
    if command.args.iter().any(|a| a.chars().any(char::is_control)) {
        return Err(LaunchError::Control);
    }
    let cwd_ok =
        is_absolute_path(&command.cwd) && sys.metadata(&command.cwd).is_ok_and(|m| m.is_dir);
    if !cwd_ok {
        return Err(LaunchError::Cwd);
    }
    Ok(tool)
}

/// The argv after the terminal program. `env_program` is the resolved
/// `env`, needed whenever `command.env` is not empty (except for Kitty) and
/// always for xterm.
pub fn terminal_args(
    terminal: TerminalKind,
    command: &Command,
    env_program: Option<&str>,
) -> Result<Vec<String>, LaunchError> {
    let env: Vec<(String, String)> = command
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let cwd = command.cwd.clone();
    let needs_env = !env.is_empty() || terminal == TerminalKind::Xterm;
    let env_program = match (needs_env, env_program) {
        (true, None) if terminal != TerminalKind::Kitty => return Err(LaunchError::NoEnvProgram),
        (_, program) => program.unwrap_or(""),
    };
    let inner = || wezterm::with_env(&env, env_program, &command.program, &command.args);
    let prefixed = |head: Vec<String>| {
        let mut out = head;
        out.extend(inner());
        out
    };
    Ok(match terminal {
        TerminalKind::Kitty => kitty::launch_args(&cwd, &env, &command.program, &command.args),
        TerminalKind::WezTerm => {
            wezterm::launch_args(&cwd, &env, env_program, &command.program, &command.args)
        }
        TerminalKind::Alacritty => prefixed(vec!["--working-directory".into(), cwd, "-e".into()]),
        TerminalKind::GnomeTerminal => {
            prefixed(vec![format!("--working-directory={cwd}"), "--".into()])
        }
        TerminalKind::Konsole => prefixed(vec!["--workdir".into(), cwd, "-e".into()]),
        TerminalKind::Ghostty => prefixed(vec![format!("--working-directory={cwd}"), "-e".into()]),
        TerminalKind::Xterm => {
            let mut out = vec![
                "-e".to_owned(),
                env_program.to_owned(),
                "-C".to_owned(),
                cwd,
            ];
            out.extend(env.iter().map(|(k, v)| format!("{k}={v}")));
            out.push(command.program.clone());
            out.extend(command.args.iter().cloned());
            out
        }
        _ => return Err(LaunchError::Terminal),
    })
}
