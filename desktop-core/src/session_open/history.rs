//! The engine's history feed: where background discovery saw a session
//! (`session_locations`' store), as the core's [`Host`]s. Read-only, from
//! the app process or the helper: the store is opened read-only and never
//! created or migrated here. Anything absent, unreadable or not modelled is
//! `Fact::Unknown`, never a guess.
use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use session_navigation::evidence::Fact;
use session_navigation::model::{App, Host, SessionRef, TerminalKind};
use session_navigation_native::HostHistory;

use crate::session_locations::{SessionLocation, PROVIDER_ENVIRONMENT_KEYS};

/// The store's file under the core's probe directory (`session_locations`).
pub const STORE_FILE: &str = "session-locations.sqlite3";

/// What the store holds for one session: its first and latest observation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoreHistory {
    pub session: Option<SessionRef>,
    pub original: Option<SessionLocation>,
    pub latest: Option<SessionLocation>,
}

impl StoreHistory {
    /// The store's record of `session` under `root` (the core's
    /// `~/.agentworkforce/probe`), tried under the id as given and in lower
    /// case. A missing or unreadable store is an empty history.
    pub fn load(root: &Path, session: &SessionRef, ids: &[&str]) -> Self {
        let source = super::session::source(session.provider);
        let found = read(&root.join(STORE_FILE), source, ids).unwrap_or_default();
        Self {
            session: Some(session.clone()),
            original: found.0,
            latest: found.1,
        }
    }

    fn of(&self, session: &SessionRef) -> bool {
        self.session.as_ref() == Some(session)
    }
}

type Origins = (Option<SessionLocation>, Option<SessionLocation>);

fn read(path: &Path, source: &str, ids: &[&str]) -> Option<Origins> {
    if !path.is_file() {
        return None;
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.busy_timeout(std::time::Duration::from_millis(500))
        .ok()?;
    let version: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .ok()?;
    if version < 1 {
        return None;
    }
    ids.iter().find_map(|id| {
        let row: (String, String) = db
            .query_row(
                "SELECT original,latest FROM origins WHERE source=?1 AND session_id=?2",
                params![source, id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .ok()??;
        let parse = |json: &str| serde_json::from_str::<SessionLocation>(json).ok();
        Some((parse(&row.0), parse(&row.1)))
    })
}

/// The terminal or app a recorded bundle id names (the ids
/// `session_locations::hosts` records on every platform).
fn known_host(bundle: &str) -> Option<Host> {
    let terminal = |t| Some(Host::Terminal(t));
    match bundle {
        "com.mitchellh.ghostty" => terminal(TerminalKind::Ghostty),
        "com.apple.Terminal" => terminal(TerminalKind::AppleTerminal),
        "com.googlecode.iterm2" => terminal(TerminalKind::ITerm2),
        "net.kovidgoyal.kitty" => terminal(TerminalKind::Kitty),
        "com.github.wez.wezterm" => terminal(TerminalKind::WezTerm),
        "org.alacritty" => terminal(TerminalKind::Alacritty),
        "org.gnome.Terminal" => terminal(TerminalKind::GnomeTerminal),
        "org.kde.konsole" => terminal(TerminalKind::Konsole),
        "xterm" => terminal(TerminalKind::Xterm),
        "Microsoft.WindowsTerminal" => terminal(TerminalKind::WindowsTerminal),
        "com.anthropic.claudefordesktop" => Some(Host::Desktop(App::ClaudeDesktop)),
        "com.openai.codex" => Some(Host::Desktop(App::CodexDesktop)),
        // The outer terminal was not recorded: tmux's server is the
        // nearest recognised ancestor of a pane's process.
        "tmux" => Some(Host::Tmux(None)),
        _ => None,
    }
}

/// The host one observation names. A process with no recognised ancestor
/// is headless only on Unix with no tty, pane or window; on Windows every
/// process lacks a tty, so it stays unknown there.
pub fn host(location: &SessionLocation) -> Fact<Host> {
    if let Some(host) = known_host(&location.bundle_id) {
        return Fact::Known(host);
    }
    let unhosted = location.bundle_id.is_empty() && location.host_pid.is_none();
    let windows = location.runtime.as_deref() == Some("windows");
    let detached =
        location.tty.is_none() && location.pane_id.is_none() && location.window_id.is_none();
    if unhosted && detached && !windows && location.runtime.is_some() {
        Fact::Known(Host::Headless)
    } else {
        Fact::Unknown
    }
}

/// The Windows Terminal profile of the latest window: known when that host
/// is Windows Terminal and its environment was read.
pub fn profile(location: Option<&SessionLocation>) -> Fact<Option<String>> {
    let Some(location) = location else {
        return Fact::Unknown;
    };
    if host(location) != Fact::Known(Host::Terminal(TerminalKind::WindowsTerminal)) {
        return Fact::Known(None);
    }
    match (&location.profile_id, location.provider_environment_known) {
        (Some(profile), _) => Fact::Known(Some(profile.clone())),
        (None, Some(true)) => Fact::Known(None),
        (None, _) => Fact::Unknown,
    }
}

/// The provider-store paths the latest observation read from the process,
/// limited to the keys the core captures. `Unknown` when unread or absent.
pub fn environment(location: Option<&SessionLocation>) -> Fact<BTreeMap<String, String>> {
    match location {
        Some(SessionLocation {
            provider_environment_known: Some(true),
            provider_environment: Some(map),
            ..
        }) => Fact::Known(
            map.iter()
                .filter(|(key, _)| PROVIDER_ENVIRONMENT_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        _ => Fact::Unknown,
    }
}

impl HostHistory for StoreHistory {
    fn last_seen(&self, session: &SessionRef) -> Fact<Host> {
        match &self.latest {
            Some(location) if self.of(session) => host(location),
            _ => Fact::Unknown,
        }
    }

    fn original(&self, session: &SessionRef) -> Fact<Host> {
        match &self.original {
            Some(location) if self.of(session) => host(location),
            _ => Fact::Unknown,
        }
    }

    fn terminal_profile(&self, session: &SessionRef) -> Fact<Option<String>> {
        if self.of(session) {
            profile(self.latest.as_ref())
        } else {
            Fact::Unknown
        }
    }
}
