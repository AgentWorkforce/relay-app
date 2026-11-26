//! Read-only: observe one session on this Mac and print the snapshot and the
//! decision a default open would make. Nothing is executed or reserved; only
//! the Observe path runs (`engine::plan`).
//!
//!     cargo run -p session-navigation-native --example observe -- codex <thread-id> [--terminal <name>]
//!
//! `--terminal` sets the request's explicit terminal preference (`ghostty`,
//! `appleterminal`, `iterm2`, …: a `TerminalKind` name, any case); without
//! it there is none, and the operating system's default applies. The
//! default the decision would use is printed last.
//!
//! Process argv can carry other programs' secrets (tokens in flags), so every
//! argument that is not a known subcommand, flag or session id is printed as
//! `<redacted>`. The OpenCode password is never part of a snapshot.
use std::process::ExitCode;

use session_navigation::engine;
use session_navigation::evidence::{Fact, Snapshot};
use session_navigation::model::{Provider, SessionRef, TerminalKind};
use session_navigation::parse::ids;
use session_navigation::plan::{Decision, OpenRequest, Step};
use session_navigation_native::{MacConfig, MacObserver, RealSystem};

const SAFE_WORDS: &[&str] = &[
    "resume",
    "fork",
    "exec",
    "e",
    "app-server",
    "serve",
    "--service",
    "run",
    "-s",
    "--session",
    "-c",
    "--continue",
    "--standalone",
    "--listen",
    "--resume",
    "--await-claim",
    "--no-daemon",
];

fn safe(word: &str) -> bool {
    SAFE_WORDS.contains(&word)
        || ids::is_uuid(word)
        || ids::is_opencode_session_id(word)
        || ids::is_claude_local_id(word)
        || word.strip_prefix("--resume=").is_some_and(ids::is_uuid)
}

fn redact(mut snapshot: Snapshot) -> Snapshot {
    if let Fact::Known(table) = &mut snapshot.processes {
        for process in table.iter_mut() {
            for word in process.argv.iter_mut().skip(1) {
                if !safe(word) {
                    *word = "<redacted>".to_owned();
                }
            }
        }
    }
    snapshot
}

fn provider(name: &str) -> Option<Provider> {
    match name {
        "claude" => Some(Provider::Claude),
        "codex" => Some(Provider::Codex),
        "opencode" => Some(Provider::OpenCode),
        "grok" => Some(Provider::Grok),
        _ => None,
    }
}

fn terminal(name: &str) -> Option<TerminalKind> {
    TerminalKind::ALL
        .into_iter()
        .find(|t| format!("{t:?}").eq_ignore_ascii_case(name))
}

/// Which default applies: the preference, else the system's, else none.
fn default_used(pref: Option<TerminalKind>, system: &Fact<Option<TerminalKind>>) -> String {
    match (pref, system) {
        (Some(t), _) => format!("{t:?} (the request's preference)"),
        (None, Fact::Known(Some(t))) => format!("{t:?} (the system default)"),
        (None, Fact::Known(None)) => "none: no preference and no system default".to_owned(),
        (None, Fact::Unknown) => "unknown: no preference, system default unreadable".to_owned(),
    }
}

fn launch_terminal(decision: &Decision) -> Option<TerminalKind> {
    decision.action()?.steps.iter().find_map(|s| match s {
        Step::Launch { terminal, .. } => Some(*terminal),
        _ => None,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: observe <claude|codex|opencode|grok> <session-id> [--terminal <name>]";
    let (Some(provider), Some(id)) = (args.first().and_then(|p| provider(p)), args.get(1)) else {
        eprintln!("{usage}");
        return ExitCode::from(2);
    };
    let pref = match args.get(2..).unwrap_or_default() {
        [] => None,
        [flag, name] if flag == "--terminal" => match terminal(name) {
            Some(t) => Some(t),
            None => {
                eprintln!("unknown terminal {name:?}; one of {:?}", TerminalKind::ALL);
                return ExitCode::from(2);
            }
        },
        _ => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let Some(cfg) = MacConfig::from_env() else {
        eprintln!("HOME is not set to an absolute path");
        return ExitCode::from(2);
    };
    let mut observer = MacObserver::new(RealSystem, cfg);
    let session = SessionRef::new(provider, id.clone());
    let mut request = OpenRequest::default();
    request.prefs.default_terminal = pref;
    let (snapshot, decision) = engine::plan(&mut observer, &session, &request);
    let used = default_used(pref, &snapshot.default_terminal);
    let print = |label: &str, json: Result<String, serde_json::Error>| match json {
        Ok(text) => println!("{label}:\n{text}"),
        Err(error) => eprintln!("{label}: {error}"),
    };
    print("snapshot", serde_json::to_string_pretty(&redact(snapshot)));
    print("decision", serde_json::to_string_pretty(&decision));
    println!("default terminal: {used}");
    match launch_terminal(&decision) {
        Some(t) => println!("launch terminal: {t:?}"),
        None => println!("launch terminal: none (no launch planned)"),
    }
    ExitCode::SUCCESS
}
