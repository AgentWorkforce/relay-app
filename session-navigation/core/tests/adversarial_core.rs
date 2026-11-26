//! Adversarial review of decide, providers, engine, parsers and plans.
//!
//! Every test states a claim about correct behaviour, the defect it exposes
//! (if it fails) and the evidence from the facts files. Nothing here touches
//! the real machine: snapshots are built by hand and the engine runs against
//! an in-memory fake of the three ports.
#[path = "adversarial_core/claude.rs"]
mod claude;
#[path = "adversarial_core/codex.rs"]
mod codex;
#[path = "adversarial_core/engine_runs.rs"]
mod engine_runs;
#[path = "adversarial_core/opencode.rs"]
mod opencode;
#[path = "adversarial_core/parsers.rs"]
mod parsers;

use std::collections::VecDeque;

use session_navigation::decide::decide;
use session_navigation::engine;
use session_navigation::evidence::*;
use session_navigation::model::*;
use session_navigation::parse;
use session_navigation::plan::*;
use session_navigation::ports::{Execute, Observe, Reservations, StoreError};

// ------------------------------------------------------------- builders ---

const BOOT: BootId = BootId(1);
const DIR: &str = "/Users/example/project";

const CLAUDE_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const CLAUDE_OTHER: &str = "12121212-3434-4565-8787-909090909090";
const LOCAL_A: &str = "local_11111111-2222-4333-8444-555555555555";
const CODEX_ID: &str = "01900000-0000-7000-8000-000000000002";
const OC_ID: &str = "ses_0123456789abAAAAAAAAAAAAAA";
// Fields added after the review (provider environment, capabilities, CLI
// paths, account, desktop tabs, reservation boot/kind) take the values of the
// world the reviewers assumed: default homes, one account, nothing verified.
const ACCOUNT: &str = "acct/org";

fn ident(pid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        start: 1_000 + u64::from(pid),
        boot: BOOT,
    }
}

fn proc_(
    pid: u32,
    ppid: u32,
    exe: Exe,
    tty: Option<&str>,
    argv: &[&str],
    cwd: Option<&str>,
) -> ProcessInfo {
    ProcessInfo {
        identity: ident(pid),
        ppid,
        euid: if exe == Exe::Login { 0 } else { 501 },
        ruid: 501,
        exe,
        tty: tty.map(str::to_owned),
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        cwd: cwd.map_or(Fact::Unknown, |c| Fact::Known(c.to_owned())),
        window: None,
    }
}

fn launchd() -> ProcessInfo {
    proc_(1, 0, Exe::Other, None, &["launchd"], None)
}

/// Ghostty → root `login` → zsh on `tty` (VERIFIED chain, Claude facts D).
fn ghostty_chain(tty: &str) -> Vec<ProcessInfo> {
    vec![
        launchd(),
        proc_(811, 1, Exe::Ghostty, None, &["ghostty"], None),
        proc_(995, 811, Exe::Login, Some(tty), &["/usr/bin/login"], None),
        proc_(998, 995, Exe::Shell, Some(tty), &["-/bin/zsh"], Some(DIR)),
    ]
}

/// Terminal.app → login → zsh on `tty` (OpenCode facts E).
fn terminal_chain(tty: &str) -> Vec<ProcessInfo> {
    vec![
        launchd(),
        proc_(700, 1, Exe::AppleTerminal, None, &["Terminal"], None),
        proc_(701, 700, Exe::Login, Some(tty), &["/usr/bin/login"], None),
        proc_(702, 701, Exe::Shell, Some(tty), &["-/bin/zsh"], Some(DIR)),
    ]
}

fn apps(running: &[App]) -> Vec<AppInfo> {
    App::ALL
        .into_iter()
        .map(|app| AppInfo {
            app,
            installed: true,
            version: Some(
                match app {
                    App::ClaudeDesktop => "5.1231.2",
                    App::CodexDesktop => "26.1103.60933",
                    App::OpenCodeDesktop => "2.2.6",
                    App::Ghostty => "1.4.0",
                    App::AppleTerminal => "2.15",
                    _ => "1.0.0",
                }
                .to_owned(),
            ),
            running: running.contains(&app),
        })
        .collect()
}

fn tools() -> Vec<ToolInfo> {
    vec![
        ToolInfo {
            tool: Tool::ClaudeCli,
            version: Fact::Known(Some("2.1.321".into())),
            path: Some("/Users/example/.local/bin/claude".into()),
        },
        ToolInfo {
            tool: Tool::CodexCli,
            version: Fact::Known(Some("0.167.0".into())),
            path: Some("/opt/homebrew/bin/codex".into()),
        },
        ToolInfo {
            tool: Tool::OpenCodeCli,
            version: Fact::Known(Some("2.2.6".into())),
            path: Some("/Users/example/.opencode/bin/opencode".into()),
        },
    ]
}

fn schemes() -> Vec<SchemeHandler> {
    vec![
        SchemeHandler {
            scheme: "claude".into(),
            bundle: Some("com.anthropic.claudefordesktop".into()),
        },
        SchemeHandler {
            scheme: "codex".into(),
            bundle: Some("com.openai.codex".into()),
        },
    ]
}

fn snap(
    provider: Provider,
    id: &str,
    table: Vec<ProcessInfo>,
    running: &[App],
    evidence: ProviderEvidence,
) -> Snapshot {
    Snapshot {
        session: SessionRef::new(provider, id),
        observed_at: 100,
        boot: Fact::Known(BOOT),
        processes: Fact::Known(table),
        front: Front {
            app: Fact::Known(None),
            pane_tty: Fact::Known(None),
            pane: session_navigation::evidence::Fact::Known(None),
            window: session_navigation::evidence::Fact::Known(None),
            view: Fact::Unknown,
        },
        apps: Fact::Known(apps(running)),
        tools: tools(),
        schemes: Fact::Known(schemes()),
        platform: session_navigation::model::Platform::MacOs,
        runtime: session_navigation::evidence::Fact::Known(
            session_navigation::model::Runtime::Native,
        ),
        surfaces: session_navigation::hosts::Surfaces::mac_scripted(),
        original_host: session_navigation::evidence::Fact::Unknown,
        terminal_profile: session_navigation::evidence::Fact::Known(None),
        // None set: a test that needs a default passes a preference.
        default_terminal: Fact::Known(None),
        last_seen_host: Fact::Unknown,
        reservation: Fact::Known(None),
        cwd: Fact::Known(DIR.to_owned()),
        cwd_exists: Fact::Known(true),
        environment: Fact::Known(Default::default()),
        capabilities: vec![],
        provider: evidence,
    }
}

fn opencode_ev(busy: Fact<bool>) -> ProviderEvidence {
    ProviderEvidence::OpenCode(OpenCodeEvidence {
        server: Fact::Known(ServerState::Up {
            pid: 1111,
            version: "2.2.6".into(),
        }),
        session: Fact::Known(Some(OpenCodeSession {
            archived: false,
            child: false,
            directory: DIR.into(),
        })),
        busy,
        tui_tab_hint: false,
        desktop_tab_hint: false,
        desktop_tabs: Fact::Known(vec![]),
        reserved: Fact::Known(vec![]),
    })
}

fn codex_ev(origin: CodexOrigin, holders: Vec<u32>) -> ProviderEvidence {
    ProviderEvidence::Codex(CodexEvidence {
        thread: Fact::Known(Some(ThreadInfo {
            archived: false,
            origin,
            subagent: false,
        })),
        lock_holders: Fact::Known(holders),
        window_owner: Fact::Known(false),
    })
}

fn claude_ev(
    registry: Vec<RegistryClaim>,
    desktop: Vec<DesktopClaim>,
    transcript: Option<Entrypoint>,
) -> ProviderEvidence {
    ProviderEvidence::Claude(ClaudeEvidence {
        registry: Fact::Known(registry),
        desktop: Fact::Known(desktop),
        transcript: Fact::Known(transcript.map(|entrypoint| Transcript {
            cwd: Some(DIR.into()),
            entrypoint,
        })),
        account: Fact::Known(ACCOUNT.into()),
    })
}

fn launches(d: &Decision) -> bool {
    let actions: Vec<&Action> = match &d.plan {
        Plan::Act(a) => vec![a],
        Plan::Choose(list) => list.iter().collect(),
        // Nothing runs until the person confirms (added after the review).
        Plan::Confirm(_) => vec![],
        Plan::Unavailable(_) => vec![],
    };
    actions
        .iter()
        .any(|a| a.steps.iter().any(|s| matches!(s, Step::Launch { .. })))
}

fn req() -> OpenRequest {
    OpenRequest {
        // An explicit terminal preference: there is no built-in default.
        prefs: session_navigation::plan::Preferences {
            default_terminal: Some(session_navigation::model::TerminalKind::Ghostty),
            ..Default::default()
        },
        ..OpenRequest::default()
    }
}
