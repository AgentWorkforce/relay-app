//! One test per (host × state) cell, asserting the exact plan: URL string,
//! argv, target, precision and reason code.
mod claude;
mod codex;
mod opencode;

pub use crate::common::*;
pub use session_navigation::model::{App, Precision, Reason, TerminalKind, Tool};
pub use session_navigation::plan::{ActionKind, OpenRequest, Preferences, SessionState};
pub use session_navigation::sim::agents::StartHost;
pub use session_navigation::sim::events::Channel;
pub use session_navigation::sim::{Event, WorldVars};

pub const CD: StartHost = StartHost::ClaudeDesktop;
pub const XD: StartHost = StartHost::CodexDesktop;
pub const CG: StartHost = StartHost::ClaudeGhostty;
pub const XG: StartHost = StartHost::CodexGhostty;
pub const OD: StartHost = StartHost::OpenCodeDesktop;
pub const OT: StartHost = StartHost::OpenCodeTerminal { with_id: true };

pub fn terminal_prefs() -> OpenRequest {
    OpenRequest {
        prefs: Preferences {
            default_terminal: Some(TerminalKind::AppleTerminal),
            ..Preferences::default()
        },
        ..OpenRequest::default()
    }
}

pub fn default_terminal() -> OpenRequest {
    OpenRequest {
        use_default_terminal: true,
        ..terminal_prefs()
    }
}

pub fn degrade(channel: Channel) -> Event {
    Event::Degrade { channel, ok: false }
}

pub fn upgrade_app(app: App) -> Event {
    Event::Upgrade {
        app: Some(app),
        tool: None,
    }
}

pub fn upgrade_tool(tool: Tool) -> Event {
    Event::Upgrade {
        app: None,
        tool: Some(tool),
    }
}
