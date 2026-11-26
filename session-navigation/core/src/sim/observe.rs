//! `Observe` for the fake machine. Evidence passes through the same lossy
//! channels reality offers: provider files are serialised and parsed back with
//! the real parsers, stale files survive crashes and reboots, channels can be
//! unreadable, and hints survive quit.
use serde_json::json;

use super::world::{Role, World};
use crate::evidence::{
    AppInfo, Capability, ClaudeEvidence, CodexEvidence, CodexOrigin, DesktopClaim, Exe, Fact,
    Front, OpenCodeEvidence, OpenCodeSession, ProviderEvidence, RegistryClaim, SchemeHandler,
    ServerState, Snapshot, ThreadInfo, ToolInfo, Transcript,
};
use crate::model::{App, BootId, Provider, SessionRef, Tool};
use crate::parse::{claude, codex, opencode};
use crate::ports::Observe;

impl World {
    fn front(&self) -> Front {
        let view = match self.front_app {
            // What Claude.app shows is not observable from outside (Claude
            // facts B, C), in any world; only the record's `lastFocusedAt`
            // is (native N1).
            Some(App::ClaudeDesktop) => Fact::Unknown,
            Some(app) if app.terminal().is_none() => {
                if self.vars.desktop_view_readable {
                    Fact::Known(self.views.get(&app).cloned().flatten())
                } else {
                    Fact::Unknown
                }
            }
            _ => Fact::Known(None),
        };
        let (pane_tty, pane, window) = self.front_panes();
        Front {
            app: Fact::Known(self.front_app),
            pane_tty,
            pane,
            window,
            view,
        }
    }

    fn claude_registry(&self) -> Fact<Vec<RegistryClaim>> {
        if !self.channels.claude_registry {
            return Fact::Unknown;
        }
        let mut claims = Vec::new();
        for (pid, file) in &self.claude.registry {
            let bytes = if file.torn {
                file.bytes.get(..file.bytes.len() / 2).unwrap_or_default()
            } else {
                &file.bytes
            };
            match claude::parse_registry(bytes) {
                Ok(rec) if rec.pid == *pid => claims.push(RegistryClaim {
                    pid: rec.pid,
                    proc_start: rec.proc_start,
                    // File times are unreadable when the file system channel is down.
                    written_this_boot: if self.channels.fs {
                        Fact::Known(file.mtime >= self.boot_time)
                    } else {
                        Fact::Unknown
                    },
                    session_id: rec.session_id,
                    host_session: rec.host_session_id,
                    spare: rec.spare,
                    entrypoint: rec.entrypoint,
                }),
                _ => {
                    // Unreadable file: harmless only if its pid is not a live
                    // Claude process.
                    let live = self.procs.get(pid).is_some_and(|p| p.exe == Exe::ClaudeCli);
                    if live {
                        return Fact::Unknown;
                    }
                }
            }
        }
        Fact::Known(claims)
    }

    fn claude_desktop(&self, id: &str) -> Fact<Vec<DesktopClaim>> {
        if !self.channels.claude_desktop {
            return Fact::Unknown;
        }
        let mut claims = Vec::new();
        for d in self.claude.desktop.values() {
            let bytes = json!({
                "sessionId": d.local,
                "cliSessionId": d.current,
                "unarchivedCliSessionId": d.unarchived,
                "priorCliSessionIds": d.prior,
                "isArchived": d.archived,
                "cwd": d.cwd,
                "lastFocusedAt": d.last_focused,
                "title": "synthetic",
            })
            .to_string();
            let Ok(record) = claude::parse_desktop_record(bytes.as_bytes()) else {
                return Fact::Unknown;
            };
            if let Some(kind) = record.claim_for(id) {
                claims.push(DesktopClaim {
                    local_id: record.session_id,
                    archived: record.archived,
                    kind,
                    cwd: record.cwd,
                    account: ACCOUNT.to_owned(),
                    current: record.current.clone().or(record.unarchived.clone()),
                    last_focused: record.last_focused,
                });
            }
        }
        Fact::Known(claims)
    }

    fn claude_evidence(&self, id: &str) -> (ProviderEvidence, Option<String>) {
        let desktop = self.claude_desktop(id);
        let transcript = match self.claude.transcripts.get(id) {
            Some(t) => {
                let entry = match t.entrypoint {
                    crate::evidence::Entrypoint::Cli => "cli",
                    crate::evidence::Entrypoint::Desktop => "claude-desktop",
                    crate::evidence::Entrypoint::Sdk => "sdk-cli",
                    crate::evidence::Entrypoint::Other => "other",
                };
                let line =
                    json!({"type": "user", "sessionId": id, "cwd": t.cwd, "entrypoint": entry});
                match claude::parse_transcript_line(line.to_string().as_bytes()) {
                    Ok(parsed) => Fact::Known(Some(Transcript {
                        cwd: parsed.cwd,
                        entrypoint: parsed
                            .entrypoint
                            .unwrap_or(crate::evidence::Entrypoint::Other),
                    })),
                    Err(_) => Fact::Unknown,
                }
            }
            None => Fact::Known(None),
        };
        let desktop_cwd = desktop.known().and_then(|claims| {
            claims
                .iter()
                .find(|c| c.kind != crate::evidence::ClaimKind::Prior)
                .and_then(|c| c.cwd.clone())
        });
        let transcript_cwd = transcript.known().cloned().flatten().and_then(|t| t.cwd);
        let evidence = ClaudeEvidence {
            registry: self.claude_registry(),
            desktop,
            transcript,
            account: Fact::Known(ACCOUNT.to_owned()),
        };
        (
            ProviderEvidence::Claude(evidence),
            desktop_cwd.or(transcript_cwd),
        )
    }

    fn lock_holders(&self, id: &str) -> Vec<u32> {
        let mut holders = Vec::new();
        for p in self.procs.values() {
            let hosts = p
                .hosts
                .as_ref()
                .is_some_and(|h| h.provider == Provider::Codex && h.id == id);
            match p.role {
                Role::Interactive if hosts => {
                    if self.codex.own_lock.contains(&p.pid) {
                        holders.push(p.pid);
                    } else if let Some(daemon) = self.daemon_pid() {
                        holders.push(daemon);
                    }
                }
                Role::Headless if hosts => holders.push(p.pid),
                Role::AppServer if self.codex.app_loaded.contains(id) => holders.push(p.pid),
                _ => {}
            }
        }
        holders.sort_unstable();
        holders.dedup();
        holders
    }

    fn codex_evidence(&self, id: &str) -> (ProviderEvidence, Option<String>) {
        let thread = self.codex.threads.get(id);
        let thread_fact = match (self.channels.codex_db, thread) {
            (false, _) => Fact::Unknown,
            (true, None) => Fact::Known(None),
            (true, Some(t)) => {
                let originator = match t.origin {
                    CodexOrigin::Desktop => "Codex Desktop",
                    CodexOrigin::Tui => "codex-tui",
                    CodexOrigin::Exec => "codex_exec",
                    CodexOrigin::Other => "other",
                };
                let row = json!({
                    "id": id, "cwd": t.cwd, "archived": i64::from(t.archived),
                    "originator": originator, "source": "vscode",
                    "thread_source": if t.subagent { "guardian_review" } else { "user" },
                });
                match codex::parse_thread_row(row.to_string().as_bytes()) {
                    Ok(r) => Fact::Known(Some(ThreadInfo {
                        archived: r.archived,
                        origin: r.origin,
                        subagent: r.subagent,
                    })),
                    Err(_) => Fact::Unknown,
                }
            }
        };
        let window_owner =
            if self.vars.codex_ipc_owner_discovery_works && self.running(App::CodexDesktop) {
                Fact::Known(
                    self.views
                        .get(&App::CodexDesktop)
                        .cloned()
                        .flatten()
                        .as_deref()
                        == Some(id),
                )
            } else {
                Fact::Unknown
            };
        let evidence = CodexEvidence {
            thread: thread_fact,
            lock_holders: if self.channels.proc_table {
                Fact::Known(self.lock_holders(id))
            } else {
                Fact::Unknown
            },
            window_owner,
        };
        (
            ProviderEvidence::Codex(evidence),
            thread.map(|t| t.cwd.clone()),
        )
    }

    fn opencode_server(&self) -> Fact<ServerState> {
        let Some(pid) = self.opencode.server else {
            return Fact::Known(ServerState::Down); // connection refused
        };
        if self.channels.opencode_hung {
            return Fact::Unknown;
        }
        let state = self
            .opencode
            .service_file
            .as_deref()
            .and_then(|b| opencode::parse_service_state(b).ok());
        let info = json!({"version": self.opencode.server_version, "pid": pid});
        let info = opencode::parse_api_info(info.to_string().as_bytes()).ok();
        match (state, info) {
            (Some(s), Some(i)) if s.pid == i.pid && s.version == i.version => {
                Fact::Known(ServerState::Up {
                    pid: i.pid,
                    version: i.version,
                })
            }
            _ => Fact::Unknown,
        }
    }

    fn opencode_session(
        &self,
        id: &str,
        server: &Fact<ServerState>,
    ) -> Fact<Option<OpenCodeSession>> {
        let session = self.opencode.sessions.get(id);
        if matches!(server, Fact::Known(ServerState::Up { .. })) {
            let (status, body) = match session {
                Some(s) => (
                    200,
                    json!({"data": {
                        "id": id, "parentID": s.parent,
                        "time": {"created": 1, "archived": if s.archived { Some(2) } else { None }},
                        "location": {"directory": s.dir},
                    }}),
                ),
                None => (
                    404,
                    json!({"_tag": "SessionNotFoundError", "sessionID": id, "message": "Session not found"}),
                ),
            };
            return match opencode::parse_session_lookup(status, body.to_string().as_bytes(), id) {
                Ok(found) => Fact::Known(found),
                Err(_) => Fact::Unknown,
            };
        }
        if !self.channels.opencode_db {
            return Fact::Unknown;
        }
        match session {
            None => Fact::Known(None),
            Some(s) => {
                let row = json!({"id": id, "parent_id": s.parent, "directory": s.dir,
                    "time_archived": if s.archived { Some(2) } else { None }});
                opencode::parse_db_row(row.to_string().as_bytes(), id)
                    .map_or(Fact::Unknown, |r| Fact::Known(Some(r)))
            }
        }
    }

    fn opencode_evidence(&self, id: &str) -> (ProviderEvidence, Option<String>) {
        let server = self.opencode_server();
        let session = self.opencode_session(id, &server);
        // `/api/session/active` lists foreground drains (facts D(iii)); with
        // no server, nothing runs a turn on it.
        let busy = match server {
            Fact::Known(ServerState::Up { .. }) => Fact::Known(self.procs.values().any(|p| {
                p.role == Role::Headless
                    && p.hosts
                        .as_ref()
                        .is_some_and(|h| h.provider == Provider::OpenCode && h.id == id)
            })),
            Fact::Known(ServerState::Down) => Fact::Known(false),
            Fact::Unknown => Fact::Unknown,
        };
        let evidence = OpenCodeEvidence {
            server,
            session,
            busy,
            tui_tab_hint: self.opencode.tui_tabs.contains(id),
            desktop_tab_hint: self.opencode.desktop_tabs.iter().any(|t| t == id),
            desktop_tabs: Fact::Known(self.opencode.desktop_tabs.clone()),
            reserved: if self.channels.reservations {
                Fact::Known(
                    self.reservations
                        .keys()
                        .filter(|s| s.provider == Provider::OpenCode && s.id != id)
                        .map(|s| s.id.clone())
                        .collect(),
                )
            } else {
                Fact::Unknown
            },
        };
        let cwd = self.opencode.sessions.get(id).map(|s| s.dir.clone());
        (ProviderEvidence::OpenCode(evidence), cwd)
    }
}

/// The one WSL distribution of the simulated Windows machine.
pub const WSL_DISTRO: &str = "Ubuntu";

/// The simulated desktop's single `<account>/<org>`.
pub const ACCOUNT: &str = "acct-0001/org-0001";

impl World {
    /// In worlds where the Claude registry follows /clear, an adapter that
    /// verified it natively reports the capability for the verified CLI.
    fn capabilities(&self) -> Vec<Capability> {
        if self.vars.claude_registry_rewritten_on_clear {
            vec![Capability::ClaudeRegistryFollowsSession {
                cli_version: super::world::verified_tool_version(Tool::ClaudeCli).to_owned(),
            }]
        } else {
            Vec::new()
        }
    }
}

impl Observe for World {
    fn observe(&mut self, session: &SessionRef) -> Snapshot {
        // Observing takes time: a process asked to stop gets closer to exiting.
        self.progress_stops(1);
        let wsl = self.wsl_sessions.contains(session);
        let (provider, cwd) = match session.provider {
            Provider::Claude => self.claude_evidence(&session.id),
            Provider::Codex => self.codex_evidence(&session.id),
            Provider::OpenCode => self.opencode_evidence(&session.id),
            Provider::Grok => self.grok_evidence(&session.id),
        };
        let cwd_exists = match (&cwd, self.channels.fs) {
            (Some(c), true) => Fact::Known(self.dirs.contains(c)),
            _ => Fact::Unknown,
        };
        Snapshot {
            session: session.clone(),
            observed_at: self.clock,
            platform: self.platform,
            runtime: Fact::Known(if wsl {
                crate::model::Runtime::Wsl {
                    distro: WSL_DISTRO.to_owned(),
                }
            } else {
                crate::model::Runtime::Native
            }),
            boot: Fact::Known(BootId(self.boot)),
            processes: if self.channels.proc_table {
                Fact::Known(self.table_for(wsl))
            } else {
                Fact::Unknown
            },
            front: self.front(),
            apps: Fact::Known(
                App::ALL
                    .into_iter()
                    .map(|app| AppInfo {
                        app,
                        installed: self.installed(app),
                        version: self
                            .apps
                            .get(&app)
                            .filter(|a| a.installed)
                            .map(|a| a.version.clone()),
                        running: self.running(app),
                    })
                    .collect(),
            ),
            tools: Tool::ALL
                .into_iter()
                .map(|tool| {
                    let version = self.tools.get(&tool).cloned().flatten();
                    ToolInfo {
                        tool,
                        path: version.as_ref().and_then(|_| {
                            if wsl {
                                Some(super::world::tool_path(tool).to_owned())
                            } else {
                                self.tool_paths.get(&tool).cloned()
                            }
                        }),
                        version: Fact::Known(version),
                    }
                })
                .collect(),
            schemes: Fact::Known(
                self.schemes
                    .iter()
                    .map(|(scheme, bundle)| SchemeHandler {
                        scheme: scheme.clone(),
                        bundle: Some(bundle.clone()),
                    })
                    .collect(),
            ),
            surfaces: self.surfaces(),
            last_seen_host: self
                .history
                .get(session)
                .map_or(Fact::Unknown, |h| Fact::Known(*h)),
            original_host: self
                .first_host
                .get(session)
                .map_or(Fact::Unknown, |h| Fact::Known(*h)),
            terminal_profile: Fact::Known(self.profiles.get(session).cloned()),
            default_terminal: self.observed_default(),
            reservation: if self.channels.reservations {
                Fact::Known(self.reservations.get(session).cloned())
            } else {
                Fact::Unknown
            },
            cwd: cwd.map_or(Fact::Unknown, Fact::Known),
            cwd_exists,
            // Every simulated session lives in its provider's default home.
            environment: if self.channels.environment {
                Fact::Known(Default::default())
            } else {
                Fact::Unknown
            },
            capabilities: self.capabilities(),
            provider,
        }
    }
}
