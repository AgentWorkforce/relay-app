//! Everything that does not depend on the operating system: provider
//! observers that only read files, run `sqlite3`/`curl`/`lsof` or parse
//! their output, POSIX quoting, reservations, tool resolution and the
//! platform-parameterised [`config`]. All of it is plain logic over
//! [`System`] and compiles and tests on any OS.
pub mod claude;
pub mod codex;
pub mod config;
pub mod grok;
pub mod kitty;
pub mod opencode;
pub mod par;
pub mod parse;
pub mod quote;
pub mod reservations;
pub mod tmux;
pub mod tools;
pub mod wezterm;

use session_navigation::evidence::{Fact, ProcessInfo, ProviderEvidence};
use session_navigation::model::{Host, Provider, SessionRef};
use session_navigation::parse::{ids, is_absolute_path, is_windows_absolute_path};

use crate::system::{FsError, Output, Run, System};
use config::CommonConfig;

/// Where background discovery saw a session. The integrator supplies it;
/// without one, it is `Unknown`.
pub trait HostHistory {
    /// The host it was last observed in.
    fn last_seen(&self, session: &SessionRef) -> Fact<Host>;

    /// The host it was first observed in.
    fn original(&self, _session: &SessionRef) -> Fact<Host> {
        Fact::Unknown
    }

    /// The Windows Terminal profile (`WT_PROFILE_ID`) the session's window
    /// had, `Known(None)` when none. Only the Windows adapter reads it.
    fn terminal_profile(&self, _session: &SessionRef) -> Fact<Option<String>> {
        Fact::Unknown
    }
}

/// No history: always `Unknown`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHistory;

impl HostHistory for NoHistory {
    fn last_seen(&self, _: &SessionRef) -> Fact<Host> {
        Fact::Unknown
    }
}

/// One provider's evidence, plus the session cwd it records and whether
/// the session was found in the configured (default) home.
pub struct Found {
    pub evidence: ProviderEvidence,
    pub cwd: Option<String>,
    pub found: bool,
}

impl Found {
    pub fn unknown(evidence: ProviderEvidence) -> Self {
        Self {
            evidence,
            cwd: None,
            found: false,
        }
    }
}

/// Run and keep the output only when it exited 0.
pub fn probe<S: System>(sys: &S, run: &Run) -> Option<Output> {
    sys.run(run).ok().filter(Output::success)
}

/// The evidence of `session`'s provider. An invalid id never reaches a path,
/// a query or a URL: its evidence is all `Unknown` (`decide` refuses it
/// first anyway).
pub fn observe_provider<S: System>(
    sys: &S,
    cfg: &CommonConfig,
    session: &SessionRef,
    boot_time: Option<u64>,
    table: &Fact<Vec<ProcessInfo>>,
) -> Found {
    if !ids::is_valid_session(session) {
        return Found::unknown(match session.provider {
            Provider::Claude => claude::unknown(),
            Provider::Codex => codex::unknown(),
            Provider::OpenCode => opencode::unknown(),
            Provider::Grok => grok::unknown(),
        });
    }
    match session.provider {
        Provider::Claude => claude::observe(sys, cfg, &session.id, boot_time, table),
        Provider::Codex => codex::observe(sys, cfg, &session.id),
        Provider::OpenCode => opencode::observe(sys, cfg, &session.id),
        Provider::Grok => grok::observe(sys, &cfg.paths, &session.id),
    }
}

/// Whether a recorded cwd (POSIX or Windows absolute) is an existing
/// directory. Anything not absolute, or unreadable, is `Unknown`.
pub fn cwd_exists<S: System>(sys: &S, cwd: Option<&str>) -> Fact<bool> {
    match cwd {
        Some(cwd) if is_absolute_path(cwd) || is_windows_absolute_path(cwd) => {
            match sys.metadata(cwd) {
                Ok(meta) => Fact::Known(meta.is_dir),
                Err(FsError::NotFound) => Fact::Known(false),
                Err(_) => Fact::Unknown,
            }
        }
        _ => Fact::Unknown,
    }
}
