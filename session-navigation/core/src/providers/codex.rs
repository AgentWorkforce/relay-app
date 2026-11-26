//! Codex: the desktop app (`com.openai.codex`) and the `codex` CLI.
//!
//! The desktop runs one shared app-server holding writer locks for many
//! threads: "loaded" is not "open in a window" (facts 12). CLI 0.167.0
//! defaults to a shared daemon, which holds the thread's writer lock and
//! rollout and is its only writer; a TUI is a client that holds no lock and
//! exposes no thread id unless its argv names one, and a second `codex resume`
//! client of the same thread is accepted without becoming a second writer
//! (VERIFIED, native N8-N10). So under the daemon the session is
//! multi-client-single-writer: a daemon-held lock whose TUI cannot be located
//! does not block a resume, which opens another client and says so
//! (`PossiblySecondView`). `--no-daemon` and other versions stay UNVERIFIED:
//! the version gate refuses their launches and the daemon rule is not used.
//!
//! A launched TUI may stop at a blocking "Update available" menu (N12) and
//! present nothing until the person dismisses it; the post-condition then
//! sees no new writer lock and stays unverified.
use super::{mismatched, ProviderPolicy};
use crate::decide::common::{self, from_result, live_decision};
use crate::decide::interpret::{self, Place};
use crate::decide::Context;
use crate::evidence::{
    CodexEvidence, CodexOrigin, Exe, Fact, ProcessInfo, ProviderEvidence, ThreadInfo,
};
use crate::model::{App, Component, Host, Precision, Provider, Reason, Tool};
use crate::parse::urls;
use crate::plan::{Action, ActionKind, Decision, SessionState};

pub struct CodexPolicy;

/// Codex desktop 26.1103.60933 (build 25894) and CLI 0.167.0 (facts header).
const VERIFIED: &[(Component, &str)] = &[
    (Component::App(App::CodexDesktop), "26.1103.60933"),
    (Component::Tool(Tool::CodexCli), "0.167.0"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Tui,
    Exec,
    DesktopServer,
    Daemon,
    Other,
}

/// Global options that take a separate value (`codex --help`, 0.167).
const VALUED: &[&str] = &[
    "-c",
    "--config",
    "-m",
    "--model",
    "-p",
    "--profile",
    "-i",
    "--image",
    "-s",
    "--sandbox",
    "-a",
    "--ask-for-approval",
    "-C",
    "--cd",
    "--remote",
    "--add-dir",
    "--enable",
    "--disable",
    "--local-provider",
];

/// The positional words of an argv, skipping options and their values
/// (`codex -m gpt-5 resume <id>`, `codex -c k=v app-server`).
fn positionals(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
        } else if arg.starts_with('-') {
            skip = !arg.contains('=') && VALUED.contains(&arg.as_str());
        } else {
            out.push(arg.as_str());
        }
    }
    out
}

/// Allowlist the TUI shapes (`codex [opts]`, `codex [opts] resume …`,
/// `codex [opts] fork …`) rather than denylisting others (facts 16).
fn role(cx: &Context<'_>, process: &ProcessInfo) -> Role {
    let args = interpret::args(process);
    if args.iter().any(|a| a == "app-server") {
        return match interpret::host_for(cx, process) {
            Fact::Known(Host::Desktop(App::CodexDesktop)) => Role::DesktopServer,
            _ => Role::Daemon,
        };
    }
    match positionals(args).first() {
        None => Role::Tui,
        Some(&"resume" | &"fork") => Role::Tui,
        Some(&"exec" | &"e") => Role::Exec,
        Some(_) => Role::Other,
    }
}

/// The positional after the last `resume` (relay's `resume_argument`).
fn resume_target(process: &ProcessInfo) -> Option<&str> {
    let words = positionals(interpret::args(process));
    let index = words.iter().rposition(|a| *a == "resume")?;
    words.get(index + 1).copied()
}

fn desktop_link(cx: &Context<'_>) -> Result<Action, Reason> {
    common::link(
        cx,
        App::CodexDesktop,
        urls::codex_thread(&cx.snap.session.id),
        ActionKind::OpenExisting,
        false,
    )
}

impl ProviderPolicy for CodexPolicy {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn verified_versions(&self) -> &'static [(Component, &'static str)] {
        VERIFIED
    }

    fn decide(&self, cx: &Context<'_>) -> Decision {
        let ProviderEvidence::Codex(ev) = &cx.snap.provider else {
            return mismatched();
        };
        let (Fact::Known(thread), Fact::Known(holders)) = (&ev.thread, &ev.lock_holders) else {
            return Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete);
        };
        let Some(thread) = thread else {
            return Decision::unavailable(SessionState::NotFound, Reason::SessionNotFound);
        };
        if thread.subagent {
            return Decision::unavailable(SessionState::Stopped, Reason::ChildSession);
        }
        match owners(cx, ev, thread, holders) {
            Some(decision) => decision,
            None => stopped(cx, thread, cx.request.prefs.prefer_terminal),
        }
    }
}

/// `None` when nothing holds or names the thread.
fn owners(
    cx: &Context<'_>,
    ev: &CodexEvidence,
    thread: &ThreadInfo,
    holders: &[u32],
) -> Option<Decision> {
    let id = cx.snap.session.id.as_str();
    let mut actions = Vec::new();
    let mut headless = Vec::new();
    let mut desktop = ev.window_owner.is(&true);
    let mut unattributed = false;
    let mut daemon = false;
    let mut blocked: Option<Reason> = None;
    let mut any = desktop;
    for holder in holders {
        if !cx
            .table
            .iter()
            .any(|p| p.identity.pid == *holder && p.exe == Exe::CodexCli)
        {
            // A holder we cannot classify is still a live owner.
            unattributed = true;
            any = true;
        }
    }
    for process in cx.table.iter().filter(|p| p.exe == Exe::CodexCli) {
        let names = resume_target(process).is_some_and(|t| t.eq_ignore_ascii_case(id));
        let holds = holders.contains(&process.identity.pid);
        if !names && !holds {
            continue;
        }
        any = true;
        match (role(cx, process), interpret::place(cx, process)) {
            // Only the TUI's own writer lock proves it has the thread open
            // now. Argv is fixed at start; a TUI may have moved to another
            // thread since (UNVERIFIED, `codex_tui_switches_thread`). A
            // daemon client never holds one (N8), so it is never exact.
            (Role::Tui, Place::Pane(ret)) => {
                let precision = if holds {
                    Precision::ExactConversation
                } else {
                    Precision::RightWindow
                };
                match common::return_to(cx, ret, process, precision) {
                    Ok(action) => actions.push(action),
                    Err(reason) => common::block(&mut blocked, reason),
                }
            }
            (Role::Exec, Place::Headless) => headless.push(process),
            (Role::DesktopServer, _) => desktop = true,
            (Role::Daemon, _) => daemon = true,
            (_, Place::Unlocatable(reason)) => common::block(&mut blocked, reason),
            _ => common::block(&mut blocked, Reason::LiveNotLocatable),
        }
    }
    if !any {
        return None;
    }
    if unattributed && actions.is_empty() {
        common::block(&mut blocked, Reason::LiveNotLocatable);
    }
    if desktop {
        match desktop_link(cx) {
            Ok(action) if !thread.archived => actions.push(action),
            _ => common::block(&mut blocked, Reason::LiveNotLocatable),
        }
    }
    if daemon && actions.is_empty() {
        if headless.is_empty() && blocked.is_none() {
            return Some(second_view(cx, thread));
        }
        common::block(&mut blocked, Reason::LiveNotLocatable);
    }
    // A take-over resumes where a stopped thread would, preference aside:
    // the preference never changes what a live session is offered.
    Some(live_decision(cx, actions, &headless, blocked, || {
        stopped(cx, thread, false)
            .action()
            .filter(|a| a.kind == ActionKind::Resume)
            .cloned()
            .ok_or(Reason::RunningHeadless)
    }))
}

/// The daemon serves the thread to a TUI that cannot be located (plain
/// `codex`, N9) and nothing else writes it: resume in a new terminal as
/// another client of that one writer (N10), labelled as possibly a second
/// view. Only for the verified CLI, and only as a CLI resume: a desktop
/// link would make the app-server a second writer.
fn second_view(cx: &Context<'_>, thread: &ThreadInfo) -> Decision {
    let daemon_verified = VERIFIED.iter().any(|(c, v)| {
        *c == Component::Tool(Tool::CodexCli)
            && cx
                .snap
                .tools
                .iter()
                .any(|t| t.tool == Tool::CodexCli && t.version.is(&Some((*v).to_owned())))
    });
    let resume = (daemon_verified && !thread.archived)
        .then(|| resume_in_terminal(cx))
        .and_then(Result::ok);
    match resume {
        Some(action) => Decision::new(
            SessionState::LiveNotLocatable,
            crate::plan::Plan::Act(Action {
                note: Some(Reason::PossiblySecondView),
                ..action
            }),
        ),
        None => Decision::unavailable(SessionState::LiveNotLocatable, Reason::LiveNotLocatable),
    }
}

fn resume_in_terminal(cx: &Context<'_>) -> Result<Action, Reason> {
    common::resume(
        cx,
        Tool::CodexCli,
        vec!["resume".to_owned(), cx.snap.session.id.clone()],
    )
}

/// `prefer_terminal`: the person's preference, for a thread nothing runs.
fn stopped(cx: &Context<'_>, thread: &ThreadInfo, prefer_terminal: bool) -> Decision {
    let desktop_origin = thread.origin == CodexOrigin::Desktop && !cx.request.use_default_terminal;
    if thread.archived {
        // What the desktop shows for an archived thread is UNVERIFIED (facts
        // 6, 9); a link starts nothing and the post-condition catches a wrong
        // destination. `codex resume` on an archived thread is UNVERIFIED too,
        // so the CLI path refuses.
        return if desktop_origin {
            from_result(
                SessionState::Archived,
                desktop_link(cx).map(|a| Action {
                    note: Some(Reason::Archived),
                    ..a
                }),
            )
        } else {
            Decision::unavailable(SessionState::Archived, Reason::Archived)
        };
    }
    if desktop_origin {
        // A terminal is preferred: the same CLI resume a default-terminal
        // request makes, in the terminal any resume would use. Only when
        // it is possible; otherwise the thread opens in Codex as usual.
        if prefer_terminal {
            if let Ok(action) = resume_in_terminal(cx) {
                return from_result(SessionState::Stopped, Ok(action));
            }
        }
        return from_result(SessionState::Stopped, desktop_link(cx));
    }
    from_result(SessionState::Stopped, resume_in_terminal(cx))
}
