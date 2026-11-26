//! Hand-built snapshots for the regression tests of the review findings.
//! Nothing here touches the real machine.
#![allow(dead_code)]
use std::collections::BTreeMap;

use session_navigation::evidence::*;
use session_navigation::model::*;
use session_navigation::plan::*;

pub const BOOT: BootId = BootId(1);
pub const DIR: &str = "/Users/example/project";
pub const ACCOUNT: &str = "acct/org";
pub const CLAUDE_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
pub const CLAUDE_OTHER: &str = "12121212-3434-4565-8787-909090909090";
pub const LOCAL_A: &str = "local_11111111-2222-4333-8444-555555555555";
pub const CODEX_ID: &str = "01900000-0000-7000-8000-000000000002";
pub const OC_ID: &str = "ses_0123456789abAAAAAAAAAAAAAA";
pub const OC_OTHER: &str = "ses_0123456789abBBBBBBBBBBBBBB";

pub fn ident(pid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        start: 1_000 + u64::from(pid),
        boot: BOOT,
    }
}

pub fn proc_(pid: u32, ppid: u32, exe: Exe, tty: Option<&str>, argv: &[&str]) -> ProcessInfo {
    ProcessInfo {
        identity: ident(pid),
        ppid,
        euid: if exe == Exe::Login { 0 } else { 501 },
        ruid: 501,
        exe,
        tty: tty.map(str::to_owned),
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        cwd: Fact::Known(DIR.to_owned()),
        window: None,
    }
}

pub fn launchd() -> ProcessInfo {
    proc_(1, 0, Exe::Other, None, &["launchd"])
}

/// Ghostty → root `login` → zsh on `tty`, plus an agent with `argv`.
pub fn in_ghostty(tty: &str, pid: u32, exe: Exe, argv: &[&str]) -> Vec<ProcessInfo> {
    vec![
        launchd(),
        proc_(811, 1, Exe::Ghostty, None, &["ghostty"]),
        proc_(995 + pid, 811, Exe::Login, Some(tty), &["/usr/bin/login"]),
        proc_(2000 + pid, 995 + pid, Exe::Shell, Some(tty), &["-/bin/zsh"]),
        proc_(pid, 2000 + pid, exe, Some(tty), argv),
    ]
}

pub fn apps(running: &[App], opencode_desktop: Option<&str>) -> Vec<AppInfo> {
    App::ALL
        .into_iter()
        .map(|app| AppInfo {
            app,
            installed: app != App::OpenCodeDesktop || opencode_desktop.is_some(),
            version: Some(
                match app {
                    App::ClaudeDesktop => "5.1231.2",
                    App::CodexDesktop => "26.1103.60933",
                    App::OpenCodeDesktop => opencode_desktop.unwrap_or("2.2.6"),
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

pub fn tools() -> Vec<ToolInfo> {
    [
        (
            Tool::ClaudeCli,
            "2.1.321",
            "/Users/example/.local/bin/claude",
        ),
        (Tool::CodexCli, "0.167.0", "/opt/homebrew/bin/codex"),
        (
            Tool::OpenCodeCli,
            "2.2.6",
            "/Users/example/.opencode/bin/opencode",
        ),
    ]
    .into_iter()
    .map(|(tool, version, path)| ToolInfo {
        tool,
        version: Fact::Known(Some(version.into())),
        path: Some(path.into()),
    })
    .collect()
}

pub fn snap(
    provider: Provider,
    id: &str,
    table: Vec<ProcessInfo>,
    ev: ProviderEvidence,
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
        apps: Fact::Known(apps(&[], Some("2.2.6"))),
        tools: tools(),
        schemes: Fact::Known(vec![
            SchemeHandler {
                scheme: "claude".into(),
                bundle: Some(App::ClaudeDesktop.bundle_id().into()),
            },
            SchemeHandler {
                scheme: "codex".into(),
                bundle: Some(App::CodexDesktop.bundle_id().into()),
            },
        ]),
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
        environment: Fact::Known(BTreeMap::new()),
        capabilities: vec![],
        provider: ev,
    }
}

pub fn opencode_ev() -> OpenCodeEvidence {
    OpenCodeEvidence {
        server: Fact::Known(ServerState::Up {
            pid: 1111,
            version: "2.2.6".into(),
        }),
        session: Fact::Known(Some(OpenCodeSession {
            archived: false,
            child: false,
            directory: DIR.into(),
        })),
        busy: Fact::Known(false),
        tui_tab_hint: false,
        desktop_tab_hint: false,
        desktop_tabs: Fact::Known(vec![]),
        reserved: Fact::Known(vec![]),
    }
}

pub fn codex_ev(origin: CodexOrigin, holders: Vec<u32>) -> CodexEvidence {
    CodexEvidence {
        thread: Fact::Known(Some(ThreadInfo {
            archived: false,
            origin,
            subagent: false,
        })),
        lock_holders: Fact::Known(holders),
        window_owner: Fact::Known(false),
    }
}

pub fn claim(local: &str, kind: ClaimKind, archived: bool, current: &str) -> DesktopClaim {
    DesktopClaim {
        local_id: local.into(),
        archived,
        kind,
        cwd: Some(DIR.into()),
        account: ACCOUNT.into(),
        current: Some(current.into()),
        last_focused: Fact::Known(None),
    }
}

pub fn registry(pid: u32, session: &str, host: Option<&str>) -> RegistryClaim {
    RegistryClaim {
        pid,
        proc_start: ident(pid).start,
        written_this_boot: Fact::Known(true),
        session_id: session.into(),
        host_session: host.map(str::to_owned),
        spare: false,
        entrypoint: if host.is_some() {
            Entrypoint::Desktop
        } else {
            Entrypoint::Cli
        },
    }
}

pub fn claude_ev(
    registry: Vec<RegistryClaim>,
    desktop: Vec<DesktopClaim>,
    transcript: Option<Entrypoint>,
) -> ClaudeEvidence {
    ClaudeEvidence {
        registry: Fact::Known(registry),
        desktop: Fact::Known(desktop),
        transcript: Fact::Known(transcript.map(|entrypoint| Transcript {
            cwd: Some(DIR.into()),
            entrypoint,
        })),
        account: Fact::Known(ACCOUNT.into()),
    }
}

pub fn launch_of(d: &Decision) -> Option<&Command> {
    d.action().and_then(|a| {
        a.steps.iter().find_map(|s| match s {
            Step::Launch { command, .. } => Some(command),
            _ => None,
        })
    })
}
