//! The terminal-host decision table: every terminal kind and capability on
//! each platform, tmux (attached, detached, two clients), Windows windows
//! and tabs, WSL, Grok, and the hazards between a decision and its first
//! step. Each scenario runs the engine against the simulator and the oracle.
#[path = "hosts/linux.rs"]
mod linux;
#[path = "hosts/tmux.rs"]
mod tmux;
#[path = "hosts/windows.rs"]
mod windows;
#[path = "hosts/wsl_grok_mac.rs"]
mod wsl_grok_mac;

use std::collections::BTreeSet;

use session_navigation::engine::{self, OpenReport};
use session_navigation::evidence::Fact;
use session_navigation::hosts::PaneRef;
use session_navigation::model::{
    DisplayServer, Platform, Precision, Provider, Reason, Runtime, SessionRef, TerminalKind,
};
use session_navigation::plan::{ActionKind, OpenRequest, Outcome, Plan, Step};
use session_navigation::sim::agents::StartHost;
use session_navigation::sim::hosts::Hazard;
use session_navigation::sim::starts::Mux;
use session_navigation::sim::{oracle, Event, World, WorldVars};

const X11: Platform = Platform::Linux {
    session: Fact::Known(DisplayServer::X11),
};
const WAYLAND: Platform = Platform::Linux {
    session: Fact::Known(DisplayServer::Wayland),
};

fn vars() -> WorldVars {
    WorldVars {
        grok_resume_flag_works: true,
        grok_resumed_tui_registers: true,
        pane_api_focus_raises: true,
        wayland_pane_focus_raises: true,
        x11_activation_honored: true,
        windows_foreground_allowed: true,
        ..WorldVars::default()
    }
}

fn world(platform: Platform, vars: WorldVars) -> World {
    let mut w = World::new_on(11, vars, platform);
    for (app, truth) in w.apps.iter_mut() {
        truth.installed = app.identifier(platform).is_some();
    }
    for granted in w.automation.values_mut() {
        *granted = true;
    }
    w.launch_delay = 0;
    w.codex.update_available = false;
    for on in w.hosts.api.values_mut() {
        *on = true;
    }
    w
}

fn index(w: &World, terminal: TerminalKind) -> u8 {
    w.terminal_choices()
        .iter()
        .position(|t| *t == terminal)
        .map_or(0, |i| i as u8)
}

fn start(
    w: &mut World,
    provider: Provider,
    terminal: TerminalKind,
    mux: Mux,
    wsl: bool,
) -> SessionRef {
    let host = StartHost::Terminal {
        provider,
        terminal: index(w, terminal),
        mux,
        wsl,
        with_id: false,
    };
    let dir = w.dir_at(0);
    let s = w.start_session(host, &dir).expect("session starts");
    w.apply(&Event::Discover);
    s
}

fn open(w: &mut World, s: &SessionRef, request: &OpenRequest) -> OpenReport {
    let id = w.log.len() as u64 + 1000;
    let from = w.log.len();
    w.engine_open = Some(id);
    let report = engine::open(w, s, request);
    w.engine_open = None;
    w.race = None;
    oracle::check_open(w, id, request, &report).expect("oracle");
    oracle::check_log(w, from, &BTreeSet::new()).expect("oracle log");
    report
}

fn focus_ref(report: &OpenReport) -> Option<&PaneRef> {
    report
        .decision
        .action()
        .and_then(|a| match a.steps.first() {
            Some(Step::FocusPane { pane, .. }) => Some(pane),
            _ => None,
        })
}

fn plain() -> OpenRequest {
    OpenRequest {
        // An explicit terminal preference: there is no built-in default.
        prefs: session_navigation::plan::Preferences {
            default_terminal: Some(session_navigation::model::TerminalKind::Ghostty),
            ..Default::default()
        },
        ..OpenRequest::default()
    }
}
