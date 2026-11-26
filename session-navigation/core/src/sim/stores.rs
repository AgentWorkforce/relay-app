//! Provider stores in the fake machine's truth (Claude, Codex, OpenCode).
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::evidence::{CodexOrigin, Entrypoint};
use crate::model::SessionRef;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeTranscript {
    pub cwd: String,
    pub entrypoint: Entrypoint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopSession {
    pub local: String,
    pub current: Option<String>,
    pub unarchived: Option<String>,
    pub prior: Vec<String>,
    pub archived: bool,
    pub cwd: String,
    /// `lastFocusedAt` in epoch ms; `None` until the app first focuses it.
    pub last_focused: Option<u64>,
}

impl DesktopSession {
    /// The current conversation is `cliSessionId ?? unarchivedCliSessionId`
    /// (Claude facts A); that is what the app resumes and shows.
    pub fn holds_live(&self, cli: &str) -> bool {
        self.current.as_deref().or(self.unarchived.as_deref()) == Some(cli)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegFile {
    pub bytes: Vec<u8>,
    pub mtime: u64,
    pub torn: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeTruth {
    pub transcripts: BTreeMap<String, ClaudeTranscript>,
    pub desktop: BTreeMap<String, DesktopSession>,
    pub registry: BTreeMap<u32, RegFile>,
    pub last_stopped: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub cwd: String,
    pub archived: bool,
    pub origin: CodexOrigin,
    pub subagent: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexTruth {
    pub threads: BTreeMap<String, Thread>,
    /// Threads the desktop's own app-server has loaded (a writer each).
    pub app_loaded: BTreeSet<String>,
    /// TUIs that hold their thread's writer lock themselves: only those
    /// started under an unverified CLI version in worlds where a TUI holds
    /// its own lock (`codex_tui_holds_lock`). Every other TUI is a client of
    /// the shared daemon, which is the only writer (VERIFIED for 0.167.0,
    /// native N8-N10).
    pub own_lock: BTreeSet<u32>,
    /// TUIs sitting at the blocking "Update available" menu (native N12),
    /// with the thread they will resume once it is dismissed.
    pub stalled: BTreeMap<u32, SessionRef>,
    /// An update is available, so every TUI started now opens on that menu.
    pub update_available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OcSession {
    pub dir: String,
    pub archived: bool,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OcTruth {
    pub sessions: BTreeMap<String, OcSession>,
    pub server: Option<u32>,
    pub server_version: String,
    pub service_file: Option<Vec<u8>>,
    pub tui_tabs: BTreeSet<String>,
    pub desktop_tabs: Vec<String>,
}
