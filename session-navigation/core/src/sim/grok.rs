//! Grok in the fake machine, from the relay and relayhistory readers
//! (facts-grok.md): a store of session directories under `<home>/sessions/
//! <encoded cwd>/<id>/` with a `summary.json`, and `active_sessions.json`
//! records that a running TUI writes and removes on a normal exit. Every
//! behaviour those readers do not establish is a world variable.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::world::World;
use crate::evidence::{Exe, Fact, GrokEvidence, ProviderEvidence};
use crate::model::{Provider, SessionRef};
use crate::parse::grok as parse;
use crate::plan::Command;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrokSess {
    pub cwd: String,
    pub subagent: bool,
}

/// One `active_sessions.json` record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrokRecord {
    pub session_id: String,
    pub pid: u32,
    pub cwd: String,
    pub opened_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrokTruth {
    pub sessions: BTreeMap<String, GrokSess>,
    /// Keyed by pid; survives a crash (the TUI removes it only on exit).
    pub active: BTreeMap<u32, GrokRecord>,
    /// Ids stored under more than one directory (a resume from another
    /// directory that created the id there): an observer finds two.
    pub duplicated: std::collections::BTreeSet<String>,
}

fn iso(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

impl World {
    /// Where a person types `grok --resume <id>`: the session's directory,
    /// but now and then another one (a person's mistake the store's
    /// per-directory layout may not forgive, `grok_resume_needs_cwd`).
    pub fn grok_person_dir(&self, session: &SessionRef) -> Option<String> {
        let own = self.grok.sessions.get(&session.id).map(|s| s.cwd.clone())?;
        if session.id.ends_with(['0', '8']) {
            return Some(self.dir_at(1));
        }
        Some(own)
    }

    pub(crate) fn new_grok(&mut self, dir: &str, subagent: bool) -> String {
        let id = self.new_session_id(Provider::Grok);
        self.grok.sessions.insert(
            id.clone(),
            GrokSess {
                cwd: dir.to_owned(),
                subagent,
            },
        );
        id
    }

    /// A TUI process records itself (`opened_at` is now, after its start).
    pub(crate) fn grok_register(&mut self, pid: u32, session: &str, cwd: &str) {
        self.grok.active.insert(
            pid,
            GrokRecord {
                session_id: session.to_owned(),
                pid,
                cwd: cwd.to_owned(),
                opened_at: self.clock,
            },
        );
    }

    /// What `grok --resume <id>` does once its process starts in `pane`.
    pub(crate) fn grok_tui_resume(
        &mut self,
        pane: u64,
        command: &Command,
        session: &SessionRef,
        by_engine: Option<u64>,
    ) -> bool {
        // Whether the flag resumes at all (desktop-core uses `--resume`,
        // relayhistory `grok resume`): UNVERIFIED. Otherwise it errors out.
        if !self.vars.grok_resume_flag_works {
            return false;
        }
        match self.grok.sessions.get(&session.id) {
            Some(s) if s.subagent && !self.vars.grok_subagent_resumable => return false,
            // Sessions are stored per directory: from another one the
            // lookup may miss (UNVERIFIED `grok_resume_needs_cwd`).
            Some(s) if s.cwd != command.cwd && self.vars.grok_resume_needs_cwd => {
                if !self.vars.grok_resume_missing_creates {
                    return false;
                }
                self.grok_create(session, &command.cwd, by_engine);
            }
            Some(_) => {}
            None if self.vars.grok_resume_missing_creates => {
                self.grok_create(session, &command.cwd, by_engine);
            }
            None => return false,
        }
        let mut argv = vec![command.program.clone()];
        argv.extend(command.args.iter().cloned());
        let Some(pid) = self.spawn_in_pane(
            pane,
            Exe::GrokCli,
            argv,
            &command.cwd,
            Some(session.clone()),
            by_engine,
        ) else {
            return false;
        };
        if self.vars.grok_resumed_tui_registers {
            self.grok_register(pid, &session.id, &command.cwd);
        }
        true
    }

    fn grok_create(&mut self, session: &SessionRef, cwd: &str, by_engine: Option<u64>) {
        if self.grok.sessions.contains_key(&session.id) {
            // A second directory entry under the same id.
            self.grok.duplicated.insert(session.id.clone());
            if by_engine.is_some() {
                self.log.push(super::world::LogEntry::Created {
                    open: by_engine,
                    session: session.clone(),
                });
            }
            return;
        }
        self.grok.sessions.insert(
            session.id.clone(),
            GrokSess {
                cwd: cwd.to_owned(),
                subagent: false,
            },
        );
        // Logged as a creation when the engine caused it.
        self.record_session(session.clone(), by_engine);
    }

    fn grok_active(&self) -> Fact<Vec<crate::evidence::GrokActive>> {
        if !self.channels.fs {
            return Fact::Unknown;
        }
        let records: Vec<_> = self
            .grok
            .active
            .values()
            .map(|r| {
                json!({"session_id": r.session_id, "pid": r.pid, "cwd": r.cwd,
                    "opened_at": iso(r.opened_at)})
            })
            .collect();
        let bytes = serde_json::Value::Array(records).to_string();
        parse::parse_active_sessions(bytes.as_bytes()).map_or(Fact::Unknown, Fact::Known)
    }

    pub(crate) fn grok_evidence(&self, id: &str) -> (ProviderEvidence, Option<String>) {
        let session = match (self.channels.fs, self.grok.sessions.get(id)) {
            (false, _) => Fact::Unknown,
            (true, Some(_)) if self.grok.duplicated.contains(id) => Fact::Unknown,
            (true, None) => Fact::Known(None),
            (true, Some(s)) => {
                let summary = json!({
                    "info": {"id": id, "cwd": s.cwd, "model": "grok-synthetic"},
                    "session_kind": if s.subagent { "subagent" } else { "interactive" },
                    "created_at": iso(1), "updated_at": iso(2),
                });
                parse::parse_summary(summary.to_string().as_bytes(), id)
                    .map_or(Fact::Unknown, |s| Fact::Known(Some(s)))
            }
        };
        let cwd = self.grok.sessions.get(id).map(|s| s.cwd.clone());
        let evidence = GrokEvidence {
            active: self.grok_active(),
            session,
        };
        (ProviderEvidence::Grok(evidence), cwd)
    }

    /// `/new` in a Grok TUI: it moves to a fresh session; its record follows
    /// only if `grok_registry_follows_session`.
    pub(crate) fn grok_switch(&mut self, pid: u32, start: u64, old: &SessionRef) {
        // `/new` starts the session in the TUI's own directory.
        let Some(cwd) = self.procs.get(&pid).and_then(|p| p.cwd.clone()) else {
            return;
        };
        let id = self.new_grok(&cwd, false);
        let new_session = SessionRef::new(Provider::Grok, id.clone());
        self.past_hosts
            .entry((pid, start))
            .or_default()
            .push(old.clone());
        if let Some(p) = self.procs.get_mut(&pid) {
            p.hosts = Some(new_session.clone());
        }
        if self.vars.grok_registry_follows_session {
            if let Some(record) = self.grok.active.get_mut(&pid) {
                record.session_id = id;
            }
        }
        self.record_session(new_session, None);
    }
}
