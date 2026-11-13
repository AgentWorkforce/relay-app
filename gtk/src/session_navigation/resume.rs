//! Structured terminal launch commands and isolated provider environments.
use super::*;
mod powershell;
#[cfg(test)]
pub(super) use powershell::windows_argument;
pub(super) use powershell::windows_environment_program;

pub fn resume_invocation(
    command: &SessionResumeCommand,
    location: Option<&SessionLocation>,
    windows: bool,
) -> Result<Invocation, String> {
    validate_command(command)?;
    validate_runtime(location.and_then(|l| l.runtime.as_deref()), windows)?;
    validate_directory(&command.cwd, windows)?;
    if command.args.first().is_some_and(|arg| arg == "--desktop") {
        return desktop_handoff(command, location, windows);
    }
    let terminal = terminal_executable(location, windows)?;
    let name = terminal
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&terminal)
        .to_ascii_lowercase();
    let mut args = terminal_arguments(&name, command, location)?;
    args.extend(provider_program(command, windows));
    Ok(Invocation {
        executable: terminal,
        args,
        cwd: Some(command.cwd.clone()),
        environment: vec![],
        clear_provider_environment: command.environment.is_some(),
    })
}

fn validate_command(command: &SessionResumeCommand) -> Result<(), String> {
    if command.executable.is_empty()
        || command.executable.starts_with('-')
        || command.executable.contains('\0')
        || command.args.iter().any(|arg| arg.contains('\0'))
        || command.cwd.contains('\0')
    {
        return Err("The resume command is invalid.".into());
    }
    if command.environment.as_ref().is_some_and(|environment| {
        environment.iter().any(|(key, value)| {
            !PROVIDER_ENVIRONMENT_KEYS.contains(&key.as_str()) || value.contains('\0')
        })
    }) {
        return Err("The provider session environment is invalid.".into());
    }
    Ok(())
}

fn validate_runtime(runtime: Option<&str>, windows: bool) -> Result<(), String> {
    let Some(runtime) = runtime else {
        return Ok(());
    };
    if !matches!(runtime, "local" | "linux" | "windows" | "native")
        || (windows && runtime == "linux")
        || (!windows && runtime == "windows")
    {
        return Err("The original WSL, container, or remote runtime cannot be restored safely on this computer.".into());
    }
    Ok(())
}

fn validate_directory(cwd: &str, windows: bool) -> Result<(), String> {
    let absolute = if windows {
        cwd.starts_with("\\\\")
            || (cwd.as_bytes().get(1) == Some(&b':')
                && matches!(cwd.as_bytes().get(2), Some(b'\\' | b'/')))
    } else {
        cwd.starts_with('/')
    };
    if absolute {
        Ok(())
    } else {
        Err("The original working directory is unavailable in this runtime.".into())
    }
}

fn desktop_handoff(
    command: &SessionResumeCommand,
    location: Option<&SessionLocation>,
    windows: bool,
) -> Result<Invocation, String> {
    if !windows || !location.is_some_and(|host| host.application == "Claude") {
        return Err("Claude Desktop handoff is unavailable for this original application.".into());
    }
    Ok(Invocation {
        executable: command.executable.clone(),
        args: command.args.clone(),
        cwd: Some(command.cwd.clone()),
        environment: command
            .environment
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect(),
        clear_provider_environment: command.environment.is_some(),
    })
}

fn terminal_executable(
    location: Option<&SessionLocation>,
    windows: bool,
) -> Result<String, String> {
    let original = location.and_then(|l| l.host_executable.as_deref());
    if location.is_some() && original.is_none() {
        return Err("The original terminal application was not recorded. Choose Resume in default terminal to continue explicitly.".into());
    }
    let terminal = original.unwrap_or(if windows {
        "wt.exe"
    } else {
        "xdg-terminal-exec"
    });
    // Fedora and Debian put the server in libexec/lib; the public client is on PATH.
    Ok(
        if terminal.rsplit('/').next() == Some("gnome-terminal-server") {
            "gnome-terminal"
        } else {
            terminal
        }
        .into(),
    )
}

fn provider_program(command: &SessionResumeCommand, windows: bool) -> Vec<String> {
    let Some(environment) = &command.environment else {
        return std::iter::once(command.executable.clone())
            .chain(command.args.clone())
            .collect();
    };
    if windows {
        return windows_environment_program(command);
    }
    let mut program = vec!["/usr/bin/env".into()];
    for key in PROVIDER_ENVIRONMENT_KEYS {
        program.extend(["-u".into(), (*key).into()]);
    }
    program.extend(
        environment
            .iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    program.push(command.executable.clone());
    program.extend(command.args.clone());
    program
}

fn terminal_arguments(
    name: &str,
    command: &SessionResumeCommand,
    location: Option<&SessionLocation>,
) -> Result<Vec<String>, String> {
    let args = match name {
        "wt.exe" | "windowsterminal.exe" => return windows_terminal_arguments(command, location),
        "wezterm" | "wezterm-gui" | "wezterm-gui.exe" => vec!["start".into(), "--cwd".into(), command.cwd.clone(), "--".into()],
        "kitty" | "alacritty" | "alacritty.exe" | "xterm" | "konsole" | "x-terminal-emulator" => vec!["-e".into()],
        "gnome-terminal" => vec!["--working-directory".into(), command.cwd.clone(), "--".into()],
        "xdg-terminal-exec" => vec!["--".into()],
        "xfce4-terminal" => vec!["-x".into()],
        _ => return Err("The original application has no supported structured resume API. Copy the resume command to continue there.".into()),
    };
    Ok(args)
}

fn windows_terminal_arguments(
    command: &SessionResumeCommand,
    location: Option<&SessionLocation>,
) -> Result<Vec<String>, String> {
    let wrapped = command.environment.is_some();
    let profile = location.and_then(|l| l.profile_id.as_deref());
    // wt interprets semicolons before spawning its child, even without a shell.
    let unwrapped_delimiter = !wrapped
        && std::iter::once(&command.executable)
            .chain(&command.args)
            .chain(std::iter::once(&command.cwd))
            .any(|arg| arg.contains(';'));
    if unwrapped_delimiter || profile.is_some_and(|p| p.contains(';')) {
        return Err("Windows Terminal cannot preserve this session's literal semicolon argument. Use a terminal that accepts structured argv.".into());
    }
    let mut args = vec!["new-tab".into()];
    if !wrapped {
        args.extend(["--startingDirectory".into(), command.cwd.clone()]);
    }
    if let Some(profile) = profile {
        args.extend(["--profile".into(), profile.into()]);
    }
    args.push("--".into());
    Ok(args)
}
