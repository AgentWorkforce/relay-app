use super::{LocalSessionRow, SpawnHandover};
use agent_relay_node::relay::contract::agents::{LocalSession, SessionHolder};
use agent_relay_node::relay::store::SessionKind;

/// Command/link descriptions used by explicit spawned-agent handover and
/// hosted-session notes. Ordinary Open is the engine's
/// (`agent_relay_desktop_core::session_open`, see `SessionOpens`).
/// Only local CLI identities are resumed. A session Agent Relay spawned is
/// never resumed beside the process that runs it: a spawned Codex thread
/// opens only in Codex, whose app server hosts it, a spawned Claude is taken
/// over instead (`SpawnHandover`) until it has been, and any other it runs
/// (Grok, OpenCode…) is not opened at all while it does.
#[derive(Debug, PartialEq)]
pub struct SessionOpening {
    pub deep_link: Option<String>,
    pub command: Option<String>,
    /// Opened only live in its app: never resumed in Terminal, whatever the
    /// preference.
    pub live_only: bool,
    /// What held the session when this handover/notes description was read.
    /// Current ownership is observed again by the engine on every Open.
    pub holder: Option<SessionHolder>,
}

impl SessionOpening {
    /// The UI may ask the engine about any local session of a provider it
    /// opens (`OPENABLE_SOURCES`), including a desktop conversation without
    /// a terminal cwd. Only the spawned-agent handover and nonlocal scope are
    /// decided before that look.
    pub fn may_request_plan(row: &LocalSessionRow, relay: Option<&LocalSession>) -> bool {
        let handover = SpawnHandover::of(relay);
        super::OPENABLE_SOURCES.contains(&row.source())
            && !matches!(
                handover,
                Some(SpawnHandover::TakeOver | SpawnHandover::OpenLive)
            )
            && !row
                .relay
                .as_ref()
                .is_some_and(|session| session.target.kind == SessionKind::ClaudeCloud)
    }

    /// A spawned Codex thread Agent Relay's app server runs opens only live
    /// in Codex: its exact thread link, never a Terminal resume, and never
    /// through the engine, which would see the app server as its owner.
    pub fn spawned_live_link(
        row: &LocalSessionRow,
        relay: Option<&LocalSession>,
    ) -> Option<String> {
        if row.source() != "codex" || SpawnHandover::of(relay) != Some(SpawnHandover::OpenLive) {
            return None;
        }
        Self::for_session(row, relay, relay).and_then(|opening| opening.deep_link)
    }

    pub fn for_row(row: &LocalSessionRow) -> Option<Self> {
        Self::for_session(row, row.relay.as_ref(), row.relay.as_ref())
    }

    /// `for_row` with the row's session as last read (`relay`), and as the
    /// session list has it now (`listed`): `None` once the list no longer
    /// has it, which `relay` cannot say, since a row keeps the copy it was
    /// built with.
    pub fn for_session(
        row: &LocalSessionRow,
        relay: Option<&LocalSession>,
        listed: Option<&LocalSession>,
    ) -> Option<Self> {
        let handover = SpawnHandover::of(relay);
        if handover == Some(SpawnHandover::TakeOver) {
            return None;
        }
        if row
            .relay
            .as_ref()
            .is_some_and(|s| s.target.kind == SessionKind::ClaudeCloud)
        {
            return None;
        }
        let id = row
            .upload
            .as_ref()
            .map(|s| s.session_id.as_str())
            .or_else(|| row.relay.as_ref().map(|s| s.target.id.as_str()))?;
        let cwd = row
            .upload
            .as_ref()
            .and_then(|s| s.cwd.as_deref())
            .or_else(|| row.relay.as_ref().and_then(|s| s.directory.as_deref()))
            .unwrap_or_default();
        // What holds the session is read only from `listed`: a session the
        // list no longer has, or one that has ended, holds nothing.
        let holder = listed
            .filter(|session| session.running != Some(false))
            .and_then(|session| session.holder);
        let opening = Self::held(row.source(), id, cwd, holder)?;
        match handover {
            Some(SpawnHandover::OpenLive) => opening.live(),
            _ => Some(opening),
        }
    }

    /// Opened only live: its deep link, never a Terminal resume; without a
    /// deep link, not at all.
    fn live(self) -> Option<Self> {
        self.deep_link.is_some().then_some(Self {
            command: None,
            live_only: true,
            ..self
        })
    }

    pub fn new(source: &str, id: &str, cwd: &str) -> Option<Self> {
        if id.is_empty() || id.starts_with('-') || id.chars().any(char::is_control) {
            return None;
        }
        let cli = match source {
            "codex" => "codex resume",
            "claude" => "claude --resume",
            "grok" => "grok --resume",
            _ => return None,
        };
        let deep_link = (source == "codex").then(|| {
            let mut url = url::Url::parse("codex://threads/").unwrap();
            url.path_segments_mut().unwrap().push(id);
            url.to_string()
        });
        // An absolute folder: `/home/me/api` on every platform (the core
        // reports Unix sessions so), or `C:\api` on Windows.
        let absolute = cwd.starts_with('/') || std::path::Path::new(cwd).is_absolute();
        let command = (absolute && !cwd.contains('\0'))
            .then(|| format!("cd -- {} && {cli} {}", shell_quote(cwd), shell_quote(id)));
        (deep_link.is_some() || command.is_some()).then_some(Self {
            deep_link,
            command,
            live_only: false,
            holder: None,
        })
    }

    /// Builds the command description for handover/notes; it does not grant
    /// permission to launch. Ordinary Open never executes this command: the
    /// engine observes the session again and builds its own.
    pub fn held(source: &str, id: &str, cwd: &str, holder: Option<SessionHolder>) -> Option<Self> {
        let opening = Self::new(source, id, cwd)?;
        let command = match holder {
            Some(SessionHolder::Background) => None,
            _ => opening.command,
        };
        (opening.deep_link.is_some() || command.is_some()).then_some(Self {
            deep_link: opening.deep_link,
            command,
            live_only: opening.live_only,
            holder,
        })
    }

    /// What to do instead, beside the open action of a held session. None
    /// for a session Agent Relay spawned, which is taken over instead, and
    /// none for a session a person is typing in: the Mac answers that one
    /// with Claude Code's `/desktop`, which is macOS and x64 Windows only.
    pub fn held_note(session: Option<&LocalSession>) -> Option<&'static str> {
        if SpawnHandover::of(session).is_some()
            || session.is_some_and(|session| session.running == Some(false))
        {
            return None;
        }
        match session.and_then(|session| session.holder) {
            Some(SessionHolder::Background) => Some(BACKGROUND_HOLDER_NOTE),
            _ => None,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.deep_link.is_some() {
            "Open in Codex"
        } else {
            "Open in Terminal"
        }
    }
}

/// Word for word the Mac's `SessionOpening.backgroundHolderNote`.
pub const BACKGROUND_HOLDER_NOTE: &str =
    "A headless Claude Code process is running this session. Run claude agents to find it, or stop it, to continue it here.";

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_the_engine_opens_may_ask_it() {
        let row = |source: &str| LocalSessionRow {
            upload: Some(
                serde_json::from_value(serde_json::json!({"source": source, "session_id": "s1",
                    "title": "Fix", "cwd": "/w", "is_agent": false, "included": false,
                    "status": "not_shared", "last_activity_ms": 1}))
                .unwrap(),
            ),
            relay: None,
        };
        for source in ["claude", "codex", "grok", "opencode"] {
            assert!(
                SessionOpening::may_request_plan(&row(source), None),
                "{source}"
            );
        }
        for source in ["pi", "cursor", "muse"] {
            assert!(
                !SessionOpening::may_request_plan(&row(source), None),
                "{source}"
            );
        }
        // OpenCode has no handover command: only the engine opens it.
        assert_eq!(SessionOpening::for_row(&row("opencode")), None);
    }

    #[test]
    fn resumes_exact_session_in_its_directory() {
        for (source, cli) in [
            ("codex", "codex resume"),
            ("claude", "claude --resume"),
            ("grok", "grok --resume"),
        ] {
            let action = SessionOpening::new(source, "session-id", "/work/my repo").unwrap();
            assert_eq!(
                action.command.unwrap(),
                format!("cd -- '/work/my repo' && {cli} 'session-id'")
            );
        }
        assert_eq!(
            SessionOpening::new("codex", "thread/id", "")
                .unwrap()
                .deep_link
                .unwrap(),
            "codex://threads/thread%2Fid"
        );
    }
    // SessionOpeningTests.testHeldSessionIsNotOfferedTheDesktopHandoff:
    // The legacy description does not offer headless resumes. A command
    // description for an interactive holder never authorizes ordinary Open:
    // the shared planner revalidates it before any native action.
    #[test]
    fn a_session_a_headless_process_holds_is_not_offered() {
        let held = |holder| SessionOpening::held("claude", "session-id", "/repo", holder);
        assert!(held(Some(SessionHolder::Background)).is_none());
        for holder in [None, Some(SessionHolder::Interactive)] {
            assert_eq!(
                held(holder).unwrap().command.unwrap(),
                "cd -- '/repo' && claude --resume 'session-id'"
            );
        }
    }

    // The Mac answers an interactive holder with Claude Code's `/desktop`,
    // which does not exist here, so only the headless note is shown. A
    // session that has ended holds nothing, whatever it held before.
    #[test]
    fn only_a_headless_holder_has_a_note_here() {
        let session = |holder| LocalSession {
            target: agent_relay_node::relay::store::Target::new(
                agent_relay_node::relay::store::SessionKind::ClaudeTerminal,
                "session-id",
            ),
            title: "Session".into(),
            project: None,
            directory: Some("/repo".into()),
            live: true,
            running: Some(true),
            holder,
            updated_at_ms: None,
            host: None,
            registration: None,
            delivery_wait: None,
            suggested_name: "repo".into(),
        };
        let background = session(Some(SessionHolder::Background));
        let desktop_without_cwd = LocalSession {
            directory: None,
            ..background.clone()
        };
        let desktop_row = LocalSessionRow {
            upload: None,
            relay: Some(desktop_without_cwd.clone()),
        };
        assert!(
            SessionOpening::may_request_plan(&desktop_row, Some(&desktop_without_cwd)),
            "The core must resolve a provider desktop mapping even without a CLI resume directory"
        );
        assert_eq!(
            SessionOpening::held_note(Some(&background)),
            Some(BACKGROUND_HOLDER_NOTE)
        );
        for quiet in [None, Some(SessionHolder::Interactive)] {
            assert_eq!(SessionOpening::held_note(Some(&session(quiet))), None);
        }
        let ended = LocalSession {
            running: Some(false),
            ..background.clone()
        };
        assert_eq!(SessionOpening::held_note(Some(&ended)), None);
        let row = LocalSessionRow {
            upload: None,
            relay: Some(ended.clone()),
        };
        // Nor is its Terminal resume withheld once it has ended.
        assert!(
            SessionOpening::for_session(&row, Some(&ended), Some(&ended))
                .is_some_and(|opening| opening.command.is_some())
        );
        // A session the list no longer has is not held either, however the
        // row's own copy of it still reads.
        assert!(SessionOpening::for_session(&row, Some(&background), None)
            .is_some_and(|opening| opening.command.is_some()));
    }

    #[test]
    fn missing_directory_unknown_source_and_option_ids_are_not_resumed() {
        assert!(SessionOpening::new("claude", "session", "").is_none());
        assert!(SessionOpening::new("grok", "session", "relative").is_none());
        assert!(SessionOpening::new("unknown", "session", "/repo").is_none());
        for id in ["", "--help", "id\ncommand", "id\0"] {
            assert!(SessionOpening::new("codex", id, "/repo").is_none());
        }
    }
    #[test]
    fn shell_metacharacters_stay_literal() {
        let action = SessionOpening::new("grok", "id'$(touch BAD)", "/repo's `name`").unwrap();
        assert_eq!(
            action.command.unwrap(),
            "cd -- '/repo'\"'\"'s `name`' && grok --resume 'id'\"'\"'$(touch BAD)'"
        );
    }

    // The quoting is POSIX shell syntax, which the Windows app hands to Git
    // Bash or Windows Terminal; only a Unix sh can check it here.
    #[cfg(unix)]
    #[test]
    fn shell_receives_unmodified_argument() {
        let value = "repo's $(printf EXPANDED) `printf EXPANDED`; echo unexpected";
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf %s {}", shell_quote(value))])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), value);
    }
}
