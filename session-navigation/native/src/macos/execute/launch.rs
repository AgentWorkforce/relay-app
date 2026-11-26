//! Launch validation and the argv handed to the launch scripts.
//!
//! - Ghostty runs its surface `command` as
//!   `/usr/bin/login -flp <user> /bin/bash --noprofile --norc -c exec -l <command>`
//!   (native N18), so the command is POSIX-quoted argv; the cwd and the
//!   environment travel as surface-configuration fields, not shell text.
//! - Terminal's `do script` text is read by the person's login shell, whose
//!   kind is not known. The command is POSIX-quoted, and words containing
//!   `\` or `!` are refused: fish treats `\` inside single quotes, and csh
//!   history-expands `!` even there.
//! - iTerm2's `create window … command` gets
//!   `'/bin/sh' '-c' '<constant>' 'sh' '<cwd>' ['/usr/bin/env' 'K=V'…] '<program>' '<arg>'…`:
//!   the cwd and argv are positional parameters of a constant script.
//!   UNVERIFIED: iTerm2 splits `command` into words honouring single
//!   quotes; words with `\` are refused in case it also reads escapes there.
//! - Kitty, WezTerm and Alacritty start through `/usr/bin/open -n -b
//!   <bundle> --args <argv…>` (each terminal's own argv form; `open`
//!   returns at once, so nothing waits on the window). UNVERIFIED: none of
//!   them was installed.
//! - A tmux program may only attach (`common::tmux::is_attach_args`).
use session_navigation::model::{TerminalKind, Tool};
use session_navigation::plan::Command;

use session_navigation::parse::is_absolute_path;

use super::super::scripts;
use crate::common::quote::{self, QuoteError};
use crate::common::{kitty, tmux, wezterm};
use crate::system::System;

/// `env`, as Terminal's command line already uses it.
pub const ENV: &str = "/usr/bin/env";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// The program is not an absolute path to an executable file.
    Program,
    /// The program is not one of the agent CLIs.
    UnknownProgram,
    /// An environment key outside the tool's allowlist, or a bad value.
    Environment,
    /// The cwd is not an existing absolute directory.
    Cwd,
    Quote(QuoteError),
    /// A word Terminal's unknown login shell might reinterpret.
    ShellUnsafe,
    /// No launch script exists for this terminal.
    Terminal,
    /// A tmux launch that is not exactly the contract's attach argv.
    TmuxArgs,
}

impl From<QuoteError> for LaunchError {
    fn from(error: QuoteError) -> Self {
        Self::Quote(error)
    }
}

fn check<S: System>(sys: &S, command: &Command) -> Result<Tool, LaunchError> {
    if !is_absolute_path(&command.program) {
        return Err(LaunchError::Program);
    }
    let tool = Tool::from_executable(&command.program).ok_or(LaunchError::UnknownProgram)?;
    if tool == Tool::Tmux && !tmux::is_attach_args(&command.args) {
        return Err(LaunchError::TmuxArgs);
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
    let cwd_ok =
        is_absolute_path(&command.cwd) && sys.metadata(&command.cwd).is_ok_and(|m| m.is_dir);
    if !cwd_ok {
        return Err(LaunchError::Cwd);
    }
    Ok(tool)
}

/// `cd -- '<cwd>' && exec ['/usr/bin/env' 'K=V'…] '<program>' '<arg>'…`.
pub fn terminal_command(command: &Command) -> Result<String, LaunchError> {
    let mut words = Vec::new();
    if !command.env.is_empty() {
        words.push(ENV.to_owned());
        words.extend(command.env.iter().map(|(k, v)| format!("{k}={v}")));
    }
    words.push(command.program.clone());
    words.extend(command.args.iter().cloned());
    let unsafe_word = words
        .iter()
        .chain(std::iter::once(&command.cwd))
        .any(|w| w.contains(['\\', '!']));
    if unsafe_word {
        return Err(LaunchError::ShellUnsafe);
    }
    let (program, args) = words.split_first().ok_or(LaunchError::Program)?;
    let line = quote::command_line(program, args)?;
    Ok(format!(
        "cd -- {} && exec {line}",
        quote::quote(&command.cwd)?
    ))
}

fn env_pairs(command: &Command) -> Vec<(String, String)> {
    command
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The iTerm2 `command` line (see the module notes).
pub fn iterm_command(command: &Command, env_program: &str) -> Result<String, LaunchError> {
    let mut words = vec![
        "-c".to_owned(),
        scripts::ITERM_SH.to_owned(),
        "sh".to_owned(),
        command.cwd.clone(),
    ];
    words.extend(wezterm::with_env(
        &env_pairs(command),
        env_program,
        &command.program,
        &command.args,
    ));
    if words.iter().any(|w| w.contains('\\')) {
        return Err(LaunchError::ShellUnsafe);
    }
    Ok(quote::command_line("/bin/sh", &words)?)
}

/// The full `/usr/bin/open` argv that starts `command` in a new window of
/// Kitty, WezTerm or Alacritty.
pub fn open_args<S: System>(
    sys: &S,
    terminal: TerminalKind,
    command: &Command,
    env_program: &str,
) -> Result<Vec<String>, LaunchError> {
    check(sys, command)?;
    if command
        .args
        .iter()
        .chain([&command.cwd, &command.program])
        .any(|a| a.contains('\0'))
    {
        return Err(LaunchError::Quote(QuoteError::Unquotable));
    }
    let env = env_pairs(command);
    let app = terminal.app();
    let mut args = vec![
        "-n".to_owned(),
        "-b".to_owned(),
        app.bundle_id().to_owned(),
        "--args".to_owned(),
    ];
    match terminal {
        TerminalKind::Kitty => args.extend(kitty::launch_args(
            &command.cwd,
            &env,
            &command.program,
            &command.args,
        )),
        TerminalKind::WezTerm => args.extend(wezterm::launch_args(
            &command.cwd,
            &env,
            env_program,
            &command.program,
            &command.args,
        )),
        TerminalKind::Alacritty => {
            args.extend([
                "--working-directory".to_owned(),
                command.cwd.clone(),
                "-e".to_owned(),
            ]);
            args.extend(wezterm::with_env(
                &env,
                env_program,
                &command.program,
                &command.args,
            ));
        }
        _ => return Err(LaunchError::Terminal),
    }
    Ok(args)
}

/// The `on run argv` arguments for the terminal's launch script.
pub fn launch_args<S: System>(
    sys: &S,
    terminal: TerminalKind,
    command: &Command,
) -> Result<Vec<String>, LaunchError> {
    check(sys, command)?;
    let args = match terminal {
        TerminalKind::Ghostty => {
            let mut args = vec![
                command.cwd.clone(),
                quote::command_line(&command.program, &command.args)?,
            ];
            args.extend(command.env.iter().map(|(k, v)| format!("{k}={v}")));
            args
        }
        TerminalKind::AppleTerminal => vec![terminal_command(command)?],
        TerminalKind::ITerm2 => vec![iterm_command(command, ENV)?],
        _ => return Err(LaunchError::Terminal),
    };
    // osascript must never read one of these as an option of its own.
    if args.iter().any(|a| a.starts_with('-')) {
        return Err(LaunchError::Program);
    }
    Ok(args)
}
