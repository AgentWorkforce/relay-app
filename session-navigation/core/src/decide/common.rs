//! Plan builders shared by provider policies. Each checks its own
//! preconditions and returns a reason instead of a guess.
use std::collections::BTreeMap;

use super::locate::Return;
use super::Context;
use crate::evidence::{Fact, ProcessInfo};
use crate::hosts::{Ability, Unsupported};
use crate::model::{App, Host, Platform, Precision, Reason, Runtime, TerminalKind, Tool};
use crate::parse::{is_absolute_path, is_windows_absolute_path, urls::InvalidId};
use crate::plan::{
    Action, ActionKind, Command, Decision, LaunchContext, Plan, PostCondition, SessionState, Step,
    Target,
};

/// Return to a live pane found by [`super::locate`], at most at `precision`.
pub fn return_to(
    cx: &Context<'_>,
    ret: Return,
    process: &ProcessInfo,
    precision: Precision,
) -> Result<Action, Reason> {
    let identity = process.identity;
    match ret {
        Return::Focus {
            terminal,
            pane,
            cap,
            note,
        } => Ok(Action {
            kind: ActionKind::FocusPane,
            steps: vec![Step::FocusPane {
                terminal,
                pane: pane.clone(),
                process: identity,
            }],
            precision: precision.min(cap),
            post: PostCondition::PaneFocused {
                terminal,
                pane,
                process: identity,
            },
            note,
            import: false,
            target: Some(Target::Pane(identity)),
        }),
        Return::Activate { app, note } => Ok(Action {
            kind: ActionKind::BringForward,
            steps: vec![Step::ActivateApp { app }],
            precision: Precision::AppOnly,
            post: PostCondition::AppFrontmost { app },
            note: Some(note),
            import: false,
            target: Some(Target::Pane(identity)),
        }),
        Return::Attach {
            socket,
            server,
            session,
            window,
            pane,
        } => {
            let terminal = launch_terminal(cx)?;
            let program = tool_executable(cx, Tool::Tmux)?;
            let context = launch_context(cx, terminal)?;
            let cwd = match cx.snap.cwd.known() {
                Some(cwd) if absolute(cx, cwd) => cwd.clone(),
                _ => root(cx),
            };
            let args = [
                "-S",
                &socket,
                "select-window",
                "-t",
                &window,
                ";",
                "select-pane",
                "-t",
                &pane,
                ";",
                "attach-session",
                "-t",
                &session,
            ]
            .iter()
            .map(|a| (*a).to_owned())
            .collect();
            Ok(Action {
                kind: ActionKind::Attach,
                steps: vec![Step::Launch {
                    terminal,
                    command: Command {
                        program,
                        args,
                        cwd,
                        env: BTreeMap::new(),
                    },
                    context,
                }],
                precision,
                post: PostCondition::TmuxAttached {
                    terminal,
                    socket,
                    server,
                    pane,
                    process: identity,
                },
                note: None,
                import: false,
                target: Some(Target::Pane(identity)),
            })
        }
    }
}

fn root(cx: &Context<'_>) -> String {
    match (cx.snap.platform, &cx.snap.runtime) {
        (Platform::Windows, Fact::Known(Runtime::Native)) => "C:\\".to_owned(),
        _ => "/".to_owned(),
    }
}

/// Whether `app` is installed. Unknown evidence and apps that do not exist
/// on this platform are reasons, not `false`.
pub fn installed(cx: &Context<'_>, app: App) -> Result<(), Reason> {
    if app.identifier(cx.snap.platform).is_none() {
        return Err(Reason::PlatformUnsupported);
    }
    let Fact::Known(apps) = &cx.snap.apps else {
        return Err(Reason::EvidenceIncomplete);
    };
    if apps.iter().any(|a| a.app == app && a.installed) {
        Ok(())
    } else {
        Err(Reason::AppNotInstalled)
    }
}

pub fn app_installed(cx: &Context<'_>, app: App) -> bool {
    installed(cx, app).is_ok()
}

/// Whether `app` is running; `Unknown` when the app list was unreadable.
pub fn is_running(cx: &Context<'_>, app: App) -> Fact<bool> {
    match &cx.snap.apps {
        Fact::Known(apps) => Fact::Known(apps.iter().any(|a| a.app == app && a.running)),
        Fact::Unknown => Fact::Unknown,
    }
}

/// A provider link handled by `app`. The scheme must be registered to exactly
/// that app, or the URL would go somewhere else.
pub fn link(
    cx: &Context<'_>,
    app: App,
    url: Result<String, InvalidId>,
    kind: ActionKind,
    import: bool,
) -> Result<Action, Reason> {
    let url = url.map_err(|_| Reason::InvalidSessionId)?;
    installed(cx, app)?;
    let handler = app
        .handler(cx.snap.platform)
        .ok_or(Reason::PlatformUnsupported)?;
    let Fact::Known(schemes) = &cx.snap.schemes else {
        return Err(Reason::EvidenceIncomplete);
    };
    let scheme = url.split("://").next().unwrap_or_default();
    let registered = schemes.iter().find(|s| s.scheme == scheme);
    if registered.and_then(|h| h.bundle.as_deref()) != Some(handler) {
        return Err(Reason::SchemeHandlerMismatch);
    }
    Ok(Action {
        kind,
        steps: vec![Step::OpenUrl {
            url,
            bundle: handler.to_owned(),
        }],
        precision: Precision::ExactConversation,
        post: PostCondition::ConversationShown { app },
        note: None,
        import,
        target: Some(Target::Desktop(app)),
    })
}

/// Bring an app forward. Never claims more than app-level precision.
pub fn bring_forward(cx: &Context<'_>, app: App, note: Reason) -> Result<Action, Reason> {
    installed(cx, app)?;
    Ok(Action {
        kind: ActionKind::BringForward,
        steps: vec![Step::ActivateApp { app }],
        precision: Precision::AppOnly,
        post: PostCondition::AppFrontmost { app },
        note: Some(note),
        import: false,
        target: Some(Target::Desktop(app)),
    })
}

fn host_terminal(host: &Fact<Host>) -> Option<TerminalKind> {
    match host {
        Fact::Known(Host::Terminal(terminal)) | Fact::Known(Host::Tmux(Some(terminal))) => {
            Some(*terminal)
        }
        _ => None,
    }
}

/// The default terminal: the person's explicit preference, else the
/// operating system's default as observed. There is no built-in terminal:
/// none set is `NoDefaultTerminal`, an unreadable default
/// `EvidenceIncomplete`.
pub fn default_terminal(cx: &Context<'_>) -> Result<TerminalKind, Reason> {
    if let Some(terminal) = cx.request.prefs.default_terminal {
        return Ok(terminal);
    }
    match cx.snap.default_terminal {
        Fact::Known(Some(terminal)) => Ok(terminal),
        Fact::Known(None) => Err(Reason::NoDefaultTerminal),
        Fact::Unknown => Err(Reason::EvidenceIncomplete),
    }
}

/// The terminal to resume in: an explicit default-terminal request, else the
/// terminal the session was last seen in, else the one it was first seen in,
/// else the default ([`default_terminal`]). A tmux origin resumes in the
/// terminal around it, outside tmux.
pub fn terminal_choice(cx: &Context<'_>) -> Result<TerminalKind, Reason> {
    if cx.request.use_default_terminal {
        return default_terminal(cx);
    }
    let host = host_terminal(&cx.snap.last_seen_host).or_else(|| match cx.snap.last_seen_host {
        Fact::Known(Host::Terminal(_) | Host::Tmux(_)) | Fact::Unknown => {
            host_terminal(&cx.snap.original_host)
        }
        _ => None,
    });
    match host {
        Some(terminal) => Ok(terminal),
        None => default_terminal(cx),
    }
}

fn ability_reason(ability: Ability) -> Result<(), Reason> {
    match ability {
        Ability::Available => Ok(()),
        Ability::NeedsSetting(_) => Err(Reason::HostSettingDisabled),
        Ability::NeedsPermission(_) => Err(Reason::PermissionRequired),
        Ability::Unsupported(Unsupported::NotOnThisPlatform) => Err(Reason::PlatformUnsupported),
        Ability::Unsupported(_) => Err(Reason::HostUnsupported),
    }
}

/// The terminal a launch opens a window in: installed, and launchable by
/// the adapter. Never silently another terminal.
fn launch_terminal(cx: &Context<'_>) -> Result<TerminalKind, Reason> {
    let terminal = terminal_choice(cx)?;
    installed(cx, terminal.app())?;
    match cx.snap.surfaces.support(terminal) {
        Fact::Known(support) => ability_reason(support.launch)?,
        Fact::Unknown => return Err(Reason::EvidenceIncomplete),
    }
    Ok(terminal)
}

/// An absolute path in the session's runtime.
fn absolute(cx: &Context<'_>, path: &str) -> bool {
    match (cx.snap.platform, &cx.snap.runtime) {
        (Platform::Windows, Fact::Known(Runtime::Native)) => is_windows_absolute_path(path),
        (_, Fact::Known(Runtime::Native | Runtime::Wsl { .. })) => is_absolute_path(path),
        _ => false,
    }
}

/// Where a launch runs: the session's own runtime, and the Windows Terminal
/// profile its window had.
fn launch_context(cx: &Context<'_>, terminal: TerminalKind) -> Result<LaunchContext, Reason> {
    let runtime = match &cx.snap.runtime {
        Fact::Unknown => return Err(Reason::EvidenceIncomplete),
        Fact::Known(Runtime::Native) => Runtime::Native,
        Fact::Known(Runtime::Wsl { distro }) if cx.snap.platform == Platform::Windows => {
            Runtime::Wsl {
                distro: distro.clone(),
            }
        }
        Fact::Known(_) => return Err(Reason::ForeignRuntime),
    };
    let profile = match (terminal, &cx.snap.terminal_profile) {
        (TerminalKind::WindowsTerminal, Fact::Known(profile)) => profile.clone(),
        (TerminalKind::WindowsTerminal, Fact::Unknown) => return Err(Reason::EvidenceIncomplete),
        _ => None,
    };
    Ok(LaunchContext { runtime, profile })
}

/// The resolved absolute executable for `tool`. A launch never runs a bare
/// program name: the person's PATH is not the launcher's.
fn tool_executable(cx: &Context<'_>, tool: Tool) -> Result<String, Reason> {
    let info = cx.snap.tools.iter().find(|t| t.tool == tool);
    match info.map(|t| (&t.version, &t.path)) {
        Some((Fact::Known(Some(_)), Some(path))) if absolute(cx, path) => Ok(path.clone()),
        Some((Fact::Known(_), _)) => Err(Reason::ToolNotInstalled),
        Some((Fact::Unknown, _)) | None => Err(Reason::EvidenceIncomplete),
    }
}

/// The provider-home selectors the session was recorded under, from the
/// tool's allowlist only. An unknown environment is refused: the wrong home
/// would not find the session, and OpenCode would silently create it in the
/// wrong database (OpenCode facts F).
fn launch_environment(cx: &Context<'_>, tool: Tool) -> Result<BTreeMap<String, String>, Reason> {
    let Fact::Known(env) = &cx.snap.environment else {
        return Err(Reason::EnvironmentUnknown);
    };
    Ok(env
        .iter()
        .filter(|(key, _)| tool.environment_keys().contains(&key.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect())
}

/// Start `tool args…` in the session's own directory in a new terminal window.
pub fn resume(cx: &Context<'_>, tool: Tool, args: Vec<String>) -> Result<Action, Reason> {
    match &cx.snap.runtime {
        Fact::Known(Runtime::Foreign { .. }) => return Err(Reason::ForeignRuntime),
        Fact::Unknown => return Err(Reason::EvidenceIncomplete),
        Fact::Known(_) => {}
    }
    let terminal = launch_terminal(cx)?;
    let program = tool_executable(cx, tool)?;
    let cwd = match &cx.snap.cwd {
        Fact::Known(cwd) if absolute(cx, cwd) => cwd.clone(),
        _ => return Err(Reason::CwdUnknown),
    };
    match cx.snap.cwd_exists {
        Fact::Known(true) => {}
        Fact::Known(false) => return Err(Reason::CwdMissing),
        Fact::Unknown => return Err(Reason::EvidenceIncomplete),
    }
    let env = launch_environment(cx, tool)?;
    let context = launch_context(cx, terminal)?;
    Ok(Action {
        kind: ActionKind::Resume,
        steps: vec![Step::Launch {
            terminal,
            command: Command {
                program,
                args,
                cwd,
                env,
            },
            context,
        }],
        precision: Precision::ExactConversation,
        post: PostCondition::RunningIn { terminal },
        note: None,
        import: false,
        target: None,
    })
}

/// Stop a background owner, observe its exit, then resume.
pub fn take_over(owner: &ProcessInfo, resume: Action) -> Action {
    let process = owner.identity;
    let mut steps = vec![Step::Stop { process }, Step::AwaitExit { process }];
    steps.extend(resume.steps);
    Action {
        kind: ActionKind::TakeOver,
        steps,
        ..resume
    }
}

pub fn from_result(state: SessionState, result: Result<Action, Reason>) -> Decision {
    match result {
        Ok(action) => Decision::new(state, Plan::Act(action)),
        Err(reason) => Decision::unavailable(state, reason),
    }
}

/// The decision for a session with live owners. `locatable` are actions that
/// return to an owner; `headless` are background owners; `blocked` is why
/// some owner could not be placed. A take-over is produced only on explicit
/// request.
pub fn live_decision(
    cx: &Context<'_>,
    locatable: Vec<Action>,
    headless: &[&ProcessInfo],
    blocked: Option<Reason>,
    resume: impl Fn() -> Result<Action, Reason>,
) -> Decision {
    if locatable.len() > 1 {
        return Decision::new(SessionState::LiveMultiple, Plan::Choose(locatable));
    }
    if let Some(action) = locatable.into_iter().next() {
        return Decision::new(SessionState::LiveLocatable, Plan::Act(action));
    }
    match (headless, blocked) {
        ([owner], None) if cx.request.take_over => from_result(
            SessionState::LiveHeadless,
            resume().map(|action| take_over(owner, action)),
        ),
        ([], None) => {
            Decision::unavailable(SessionState::LiveNotLocatable, Reason::LiveNotLocatable)
        }
        (_, Some(reason)) => Decision::unavailable(SessionState::LiveNotLocatable, reason),
        _ => Decision::unavailable(SessionState::LiveHeadless, Reason::RunningHeadless),
    }
}

/// Fold a newly found reason into `blocked`, keeping the first specific one.
pub fn block(blocked: &mut Option<Reason>, reason: Reason) {
    if blocked.is_none() || *blocked == Some(Reason::LiveNotLocatable) {
        *blocked = Some(reason);
    }
}
