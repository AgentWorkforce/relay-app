//! OpenCode: one background server; the TUI and the desktop app are clients.
//!
//! The server does not record which client shows which session, so a TUI not
//! launched with this id may still show it, and the desktop may show anything
//! (facts D). There is no session deep link in 2.2.6 (facts C). `opencode -s`
//! with a missing id silently creates that session (facts F), so a launch
//! needs positive confirmation of existence.
//!
//! A TUI is a pure client of the shared server, which is the only writer;
//! starting one creates no session and does not restart the server
//! (VERIFIED, 2.2.6, native N13-N14). So another TUI being open never
//! refuses a resume: a TUI that may show the session only labels the plan
//! `PossiblySecondView`. The guards for the verified hazards stay: existence
//! positively confirmed, no child or archived id, no busy session, no
//! version mismatch that would replace the server, known environment.
use super::{mismatched, ProviderPolicy};
use crate::decide::common::{self, from_result, live_decision};
use crate::decide::interpret::{self, Place};
use crate::decide::Context;
use crate::evidence::{
    Exe, Fact, OpenCodeEvidence, OpenCodeSession, ProcessInfo, ProviderEvidence, ServerState,
};
use crate::model::{App, Component, Host, Precision, Provider, Reason, Tool};
use crate::plan::{Action, ActionKind, Decision, SessionState};

pub struct OpenCodePolicy;

/// OpenCode desktop and CLI 2.2.6 (facts header).
const VERIFIED: &[(Component, &str)] = &[
    (Component::App(App::OpenCodeDesktop), "2.2.6"),
    (Component::Tool(Tool::OpenCodeCli), "2.2.6"),
];

enum Role {
    /// Launched with `-s <id>`; it may have navigated away since.
    Shows {
        id: String,
        headless: bool,
    },
    /// A TUI that may show any session.
    MaybeShows,
    Other,
}

/// Subcommands that are not the interactive TUI (`opencode --help`, 2.2.6).
/// `mini` is a TUI and is deliberately absent.
const NOT_TUI: &[&str] = &[
    "serve",
    "run",
    "session",
    "api",
    "service",
    "debug",
    "upgrade",
    "uninstall",
    "auth",
    "models",
    "stats",
    "export",
    "import",
    "mcp",
    "acp",
    "pair",
    "web",
    "github",
    "agent",
    "completion",
    "generate",
];

fn session_flag(args: &[String]) -> Option<&str> {
    let index = args.iter().position(|a| a == "-s" || a == "--session")?;
    args.get(index + 1).map(String::as_str)
}

/// The non-TUI subcommand, if any. A word right after an option is taken as
/// that option's value, so `opencode -m run` stays a TUI: when unsure, the
/// process is treated as a TUI that may show the session.
fn subcommand(args: &[String]) -> Option<&str> {
    let mut previous_is_option = false;
    for arg in args {
        if arg.starts_with('-') {
            previous_is_option = !arg.contains('=');
            continue;
        }
        if !previous_is_option && NOT_TUI.contains(&arg.as_str()) {
            return Some(arg.as_str());
        }
        previous_is_option = false;
    }
    None
}

/// Any `opencode` process with a tty that is not a known non-TUI subcommand
/// may show any session: a session picked inside the TUI never reaches argv
/// (facts D(i)), and the positional directory may be relative (facts F).
fn role(process: &ProcessInfo) -> Role {
    let args = interpret::args(process);
    let sub = subcommand(args);
    if sub == Some("serve") {
        return Role::Other;
    }
    let shows = session_flag(args).map(str::to_owned);
    match (sub, shows) {
        (Some("run"), Some(id)) => Role::Shows { id, headless: true },
        (None, Some(id)) => Role::Shows {
            id,
            headless: false,
        },
        (None, None) if process.tty.is_some() => Role::MaybeShows,
        _ => Role::Other,
    }
}

impl ProviderPolicy for OpenCodePolicy {
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }

    fn verified_versions(&self) -> &'static [(Component, &'static str)] {
        VERIFIED
    }

    fn decide(&self, cx: &Context<'_>) -> Decision {
        let ProviderEvidence::OpenCode(ev) = &cx.snap.provider else {
            return mismatched();
        };
        let session = match &ev.session {
            Fact::Unknown => {
                return Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete)
            }
            Fact::Known(None) => {
                return Decision::unavailable(SessionState::NotFound, Reason::SessionNotFound)
            }
            Fact::Known(Some(session)) => session,
        };
        if session.child {
            return Decision::unavailable(SessionState::Stopped, Reason::ChildSession);
        }
        if session.archived {
            return Decision::unavailable(SessionState::Archived, Reason::Archived);
        }
        let id = cx.snap.session.id.as_str();
        let mut actions = Vec::new();
        let mut headless = Vec::new();
        let mut blocked: Option<Reason> = None;
        let mut maybe = false;
        for process in cx.table.iter().filter(|p| p.exe == Exe::OpenCodeCli) {
            match (role(process), interpret::place(cx, process)) {
                (Role::Shows { id: shown, .. }, _) if shown != id => {
                    maybe |= process.tty.is_some();
                }
                (
                    Role::Shows {
                        headless: false, ..
                    },
                    Place::Pane(ret),
                ) => match common::return_to(cx, ret, process, Precision::RightWindow) {
                    Ok(action) => actions.push(action),
                    Err(reason) => common::block(&mut blocked, reason),
                },
                (Role::Shows { headless: true, .. }, Place::Headless) => headless.push(process),
                (Role::Shows { .. }, Place::Unlocatable(reason)) => {
                    common::block(&mut blocked, reason)
                }
                (Role::Shows { .. }, _) => common::block(&mut blocked, Reason::LiveNotLocatable),
                (Role::MaybeShows, _) => maybe = true,
                (Role::Other, _) => {}
            }
        }
        if !actions.is_empty() || !headless.is_empty() || blocked.is_some() {
            return live_decision(cx, actions, &headless, blocked, || {
                // The running desktop is the client to return to.
                match common::is_running(cx, App::OpenCodeDesktop) {
                    Fact::Known(false) => {}
                    Fact::Known(true) => return Err(Reason::PossiblyOpenElsewhere),
                    Fact::Unknown => return Err(Reason::EvidenceIncomplete),
                }
                // Take-over: the running turn is the owner being stopped; the
                // engine re-observes after its exit before launching.
                launch(cx, ev, session, true).map(|a| second_view(a, maybe))
            });
        }
        stopped(cx, ev, session, maybe)
    }
}

/// Label a plan that may put a second client on the session (N14).
fn second_view(action: Action, maybe: bool) -> Action {
    if !maybe {
        return action;
    }
    Action {
        note: Some(Reason::PossiblySecondView),
        ..action
    }
}

fn desktop_origin(cx: &Context<'_>, ev: &OpenCodeEvidence) -> bool {
    if cx.request.use_default_terminal {
        return false;
    }
    match cx.snap.last_seen_host {
        Fact::Known(Host::Desktop(_)) => true,
        Fact::Known(_) => false,
        Fact::Unknown => ev.desktop_tab_hint && !ev.tui_tab_hint,
    }
}

/// Launching the desktop restores its saved tabs (facts D(ii)), and the
/// restored tab may be a session a TUI shows right now. That is a second
/// client, not a second writer (N14), so a TUI no longer forbids it. A turn
/// running on one of the tabs (`opencode run -s <tab>`, the busy hazard) and
/// our own unresolved launch on one still do, and unknown tabs or
/// reservations are refused.
fn desktop_launch_conflict(cx: &Context<'_>, ev: &OpenCodeEvidence) -> Result<(), Reason> {
    let (Fact::Known(tabs), Fact::Known(reserved)) = (&ev.desktop_tabs, &ev.reserved) else {
        return Err(Reason::EvidenceIncomplete);
    };
    if reserved.iter().any(|id| tabs.contains(id)) {
        return Err(Reason::LaunchPending);
    }
    // The desktop starts a server of its own bundled version; a TUI of ours
    // still starting with a CLI of another version would then replace it
    // (facts G), interrupting the desktop's turns.
    if !reserved.is_empty() && desktop_version(cx)? != cli_version(cx) {
        return Err(Reason::ServerVersionMismatch);
    }
    let turn_on_tab = cx
        .table
        .iter()
        .filter(|p| p.exe == Exe::OpenCodeCli)
        .any(|p| matches!(role(p), Role::Shows { id, headless: true } if tabs.contains(&id)));
    if turn_on_tab {
        return Err(Reason::PossiblyOpenElsewhere);
    }
    Ok(())
}

fn cli_version(cx: &Context<'_>) -> Option<String> {
    cx.snap
        .tools
        .iter()
        .find(|t| t.tool == Tool::OpenCodeCli)
        .and_then(|t| t.version.known().cloned().flatten())
}

/// The installed desktop's version (`None`: not installed or unversioned).
fn desktop_version(cx: &Context<'_>) -> Result<Option<String>, Reason> {
    match &cx.snap.apps {
        Fact::Known(apps) => Ok(apps
            .iter()
            .find(|a| a.app == App::OpenCodeDesktop && a.installed)
            .and_then(|a| a.version.clone())),
        Fact::Unknown => Err(Reason::EvidenceIncomplete),
    }
}

fn stopped(
    cx: &Context<'_>,
    ev: &OpenCodeEvidence,
    session: &OpenCodeSession,
    maybe: bool,
) -> Decision {
    // A running desktop may show this session already and cannot be asked
    // which one it shows, so a second client is never started next to it.
    let desktop_running = match common::is_running(cx, App::OpenCodeDesktop) {
        Fact::Known(running) => running,
        Fact::Unknown => {
            return Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete)
        }
    };
    // Launching the desktop restores its last tab, which may be this session:
    // not while an earlier launch of ours is unresolved.
    let launch_unresolved = match &cx.snap.reservation {
        Fact::Known(None) => false,
        Fact::Known(Some(r)) => {
            r.boot.is_none_or(|b| b == cx.boot)
                && !crate::decide::reservation_window_gone(r, cx.snap)
        }
        Fact::Unknown => true,
    };
    if !desktop_running && launch_unresolved && !cx.request.confirm_launch_gone {
        return Decision::unavailable(SessionState::LaunchPending, Reason::LaunchPending);
    }
    // The desktop app exists on macOS only (as far as this crate knows).
    let desktop_here = App::OpenCodeDesktop.identifier(cx.snap.platform).is_some();
    if desktop_running || (desktop_here && desktop_origin(cx, ev)) {
        if !desktop_running {
            if let Err(reason) = desktop_launch_conflict(cx, ev) {
                return Decision::unavailable(SessionState::Stopped, reason);
            }
        }
        return from_result(
            SessionState::Stopped,
            common::bring_forward(cx, App::OpenCodeDesktop, Reason::NoSessionDeepLink),
        );
    }
    from_result(
        SessionState::Stopped,
        launch(cx, ev, session, false).map(|a| second_view(a, maybe)),
    )
}

fn launch(
    cx: &Context<'_>,
    ev: &OpenCodeEvidence,
    session: &OpenCodeSession,
    taking_over: bool,
) -> Result<Action, Reason> {
    // A running turn means some client drives the session now (facts D(iii)).
    match ev.busy {
        Fact::Known(false) => {}
        Fact::Known(true) if taking_over => {}
        Fact::Known(true) => return Err(Reason::SessionBusy),
        Fact::Unknown => return Err(Reason::EvidenceIncomplete),
    }
    // Run in the session's own directory (facts F); refuse when other
    // evidence names another one.
    if cx.snap.cwd.known().is_some_and(|c| *c != session.directory) {
        return Err(Reason::CwdConflict);
    }
    let action = common::resume(
        cx,
        Tool::OpenCodeCli,
        vec!["-s".to_owned(), cx.snap.session.id.clone()],
    )?;
    let cli_version = cx
        .snap
        .tools
        .iter()
        .find(|t| t.tool == Tool::OpenCodeCli)
        .and_then(|t| t.version.known().cloned().flatten());
    // A TUI of another version replaces the running server, interrupting the
    // desktop's turns (facts G). With no server, the TUI starts its own; the
    // desktop later demands its bundled version and replaces that one. (A
    // CLI of an unverified version is refused by the version gate anyway.)
    let cli_verified = VERIFIED.iter().any(|(c, v)| {
        *c == Component::Tool(Tool::OpenCodeCli) && cli_version.as_deref() == Some(v)
    });
    let other_version = match &ev.server {
        Fact::Unknown => return Err(Reason::EvidenceIncomplete),
        Fact::Known(ServerState::Up { version, .. }) => Some(Some(version.clone())),
        Fact::Known(ServerState::Down) if !cli_verified => None,
        Fact::Known(ServerState::Down) => match &cx.snap.apps {
            Fact::Known(apps) => apps
                .iter()
                .find(|a| a.app == App::OpenCodeDesktop && a.installed)
                .map(|a| a.version.clone()),
            Fact::Unknown => return Err(Reason::EvidenceIncomplete),
        },
    };
    if let Some(other) = other_version {
        if other.is_none() || other != cli_version {
            return Err(Reason::ServerVersionMismatch);
        }
    }
    // The resumed TUI was launched with this id; it is the right window, but
    // the person may navigate within it.
    Ok(Action {
        kind: ActionKind::Resume,
        precision: Precision::RightWindow,
        ..action
    })
}
