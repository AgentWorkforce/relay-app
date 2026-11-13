use super::*;

pub(super) fn resolve(
    request: &SessionOpenRequest,
    report: &SessionLocationReport,
    platform: Platform,
) -> SessionOpenPlan {
    let origin = report
        .latest
        .as_ref()
        .or(report.original.as_ref())
        .or_else(|| report.previous.first());
    if origin.is_some_and(|l| l.runtime.as_deref() == Some("wsl")) {
        return unavailable(
            "Resuming the original WSL distribution and terminal profile is unavailable.",
        );
    }
    let cwd = origin
        .and_then(|l| l.cwd.as_ref())
        .or(request.directory.as_ref())
        .filter(|s| absolute_directory(s, platform));
    let environment = resume_environment(origin, platform);
    let as_terminal =
        request.prefer_terminal || request.use_default_terminal || origin.is_some_and(terminal);
    if !as_terminal {
        if let Some(url) = desktop_link(request, report) {
            let mut p = link(url, None);
            p.can_resume_default = cwd.is_some()
                && environment.is_ok()
                && ["codex", "claude", "grok"].contains(&request.source.as_str());
            return p;
        }
    }
    let environment = match environment {
        Ok(environment) => environment,
        Err(reason) => return unavailable(reason),
    };
    let Some(cwd) = cwd else {
        return unavailable("The original working directory is unknown. Open the project in a terminal to resume this session.");
    };
    launch(request, origin, platform, as_terminal, cwd, environment)
}

fn launch(
    request: &SessionOpenRequest,
    origin: Option<&SessionLocation>,
    platform: Platform,
    as_terminal: bool,
    cwd: &str,
    environment: Option<std::collections::BTreeMap<String, String>>,
) -> SessionOpenPlan {
    let Some(args) = resume_arguments(request, origin, as_terminal) else {
        return unavailable("This provider does not support resuming a local session.");
    };
    // Never substitute a shell command string for argv. A path containing spaces
    // remains one executable or directory all the way to the native launcher.
    let executable = origin
        .and_then(|l| l.executable.clone())
        .filter(|p| {
            std::path::Path::new(p)
                .file_stem()
                .is_some_and(|name| name == request.source.as_str())
        })
        .unwrap_or_else(|| request.source.clone());
    if !request.use_default_terminal
        && origin.is_some_and(|l| l.focus_capability.as_deref() == Some("tmux"))
    {
        let mut p=unavailable("The original session ran inside tmux. Restoring its socket and pane is unavailable. You can explicitly resume in your default terminal.");
        p.can_resume_default = true;
        return p;
    }
    if !request.use_default_terminal
        && args.first().is_some_and(|arg| arg == "--desktop")
        && platform == Platform::Linux
    {
        let mut p=unavailable("Claude CLI desktop handoff is unavailable on Linux. You can explicitly resume this stopped session in your default terminal.");
        p.can_resume_default = true;
        return p;
    }
    let target = if request.use_default_terminal
        || (request.prefer_terminal && origin.is_some_and(|l| !terminal(l)))
    {
        None
    } else {
        origin.cloned()
    };
    let mut p = plan(SessionOpenAction::Resume, target);
    p.can_resume_default = !request.use_default_terminal && origin.is_some();
    p.command = Some(SessionResumeCommand {
        executable,
        args,
        cwd: cwd.to_owned(),
        environment,
    });
    p
}

fn resume_arguments(
    request: &SessionOpenRequest,
    origin: Option<&SessionLocation>,
    as_terminal: bool,
) -> Option<Vec<String>> {
    Some(match request.source.as_str() {
        "codex" => vec!["resume".into(), request.session_id.clone()],
        "claude" if !as_terminal && origin.is_some_and(|l| l.application == "Claude") => vec![
            "--desktop".into(),
            "--resume".into(),
            request.session_id.clone(),
        ],
        "claude" => vec!["--resume".into(), request.session_id.clone()],
        "grok" => vec!["--resume".into(), request.session_id.clone()],
        _ => return None,
    })
}
