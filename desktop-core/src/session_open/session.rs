//! The session a request names, as the engine names it. Claude's desktop
//! app shows `local_…` ids; the engine works on the CLI conversation id the
//! desktop record names, and only that record may join them (never a title
//! or a directory).
use session_navigation::model::{Provider, SessionRef, TerminalKind};
use session_navigation::parse::claude::parse_desktop_record;
use session_navigation::parse::ids;
use session_navigation_native::system::{join, System};

use super::wording::{self, Local};

/// Largest desktop record read (the core parser's own bound).
const RECORD_LIMIT: usize = 1024 * 1024;

pub fn provider(source: &str) -> Option<Provider> {
    match source {
        "claude" => Some(Provider::Claude),
        "codex" => Some(Provider::Codex),
        "grok" => Some(Provider::Grok),
        "opencode" => Some(Provider::OpenCode),
        _ => None,
    }
}

pub fn source(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Grok => "grok",
        Provider::OpenCode => "opencode",
    }
}

/// The terminal a preference id names; `Err` for an id that is none.
pub fn terminal(id: Option<&str>) -> Result<Option<TerminalKind>, Local> {
    let Some(id) = id else {
        return Ok(None);
    };
    TerminalKind::ALL
        .into_iter()
        .find(|t| wording::terminal_id(*t) == id)
        .map(Some)
        .ok_or(Local::UnknownTerminal)
}

/// The engine's session for `source` and `id`. A Claude `local_…` id maps
/// through `desktop_sessions` (`claude-code-sessions`) to the CLI id its
/// one record holds; none or several records refuse it.
pub fn resolve<S: System>(
    sys: &S,
    desktop_sessions: &str,
    source: &str,
    id: &str,
) -> Result<SessionRef, Local> {
    let provider = provider(source).ok_or(Local::UnsupportedSource)?;
    if provider == Provider::Claude && id.starts_with("local_") {
        return claude_cli_id(sys, desktop_sessions, id)
            .map(|cli| SessionRef::new(provider, cli))
            .ok_or(Local::DesktopIdUnmapped);
    }
    // An invalid id still goes to the engine, which refuses it with its
    // own reason (`InvalidSessionId`) without reading anything.
    Ok(SessionRef::new(provider, id))
}

fn claude_cli_id<S: System>(sys: &S, root: &str, local: &str) -> Option<String> {
    if !ids::is_claude_local_id(local) {
        return None;
    }
    let file = format!("{local}.json");
    let mut found: Option<String> = None;
    for account in sys.list_dir(root).ok()? {
        let account = join(root, &account);
        let Ok(orgs) = sys.list_dir(&account) else {
            continue;
        };
        for org in orgs {
            let path = join(&join(&account, &org), &file);
            let Ok(bytes) = sys.read_file(&path, RECORD_LIMIT) else {
                continue;
            };
            let record = parse_desktop_record(&bytes).ok()?;
            let cli = record
                .current
                .or(record.unarchived)
                .filter(|_| record.session_id == local)?;
            if found.as_ref().is_some_and(|other| *other != cli) {
                return None;
            }
            found = Some(cli);
        }
    }
    found.map(|cli| cli.to_ascii_lowercase())
}
