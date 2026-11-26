//! Application lifecycles and the provider-side process shapes.
use super::truth::Exit;
use super::world::{Proc, Role, World, USER};
use crate::evidence::Exe;
use crate::model::{App, Provider, SessionRef};
use crate::parse::lstart;

fn app_exe(app: App) -> Exe {
    match app {
        App::ClaudeDesktop => Exe::ClaudeDesktopApp,
        App::CodexDesktop => Exe::CodexDesktopApp,
        App::OpenCodeDesktop => Exe::OpenCodeDesktopApp,
        App::Ghostty => Exe::Ghostty,
        App::AppleTerminal => Exe::AppleTerminal,
        App::ITerm2 => Exe::ITerm2,
        App::Kitty => Exe::Kitty,
        App::WezTerm => Exe::WezTerm,
        App::Alacritty => Exe::Alacritty,
        App::GnomeTerminal => Exe::GnomeTerminal,
        App::Konsole => Exe::Konsole,
        App::Xterm => Exe::Xterm,
        App::WindowsTerminal => Exe::WindowsTerminal,
        App::ConsoleHost => Exe::ConsoleHost,
    }
}

pub(crate) fn bare(ppid: u32, exe: Exe, argv: &[&str], role: Role) -> Proc {
    Proc {
        pid: 0,
        start: 0,
        ppid,
        euid: USER,
        ruid: USER,
        exe,
        tty: None,
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        cwd: Some("/".to_owned()),
        role,
        hosts: None,
        by_engine: None,
        supervised: false,
    }
}

impl World {
    /// Start `app` if it is installed and not running. Returns its pid.
    pub fn ensure_app(&mut self, app: App) -> Option<u32> {
        if let Some(pid) = self.app_pid(app) {
            return Some(pid);
        }
        if !self.installed(app) {
            return None;
        }
        let pid = self.spawn(bare(1, app_exe(app), &[app.bundle_id()], Role::App(app)));
        match app {
            App::ClaudeDesktop => self.spawn_spare(pid),
            App::CodexDesktop => {
                self.spawn(bare(
                    pid,
                    Exe::CodexCli,
                    &["codex", "app-server", "--analytics-default-enabled"],
                    Role::AppServer,
                ));
            }
            App::OpenCodeDesktop => {
                self.ensure_server(Some(pid));
                self.views.insert(app, None);
                self.restore_opencode_tab();
            }
            _ => {}
        }
        if app == App::ClaudeDesktop || app == App::CodexDesktop {
            self.views.insert(app, None);
        }
        Some(pid)
    }

    /// The OpenCode desktop restores its most recent tab on launch. That puts
    /// another client on the session, not another writer (native N14).
    fn restore_opencode_tab(&mut self) {
        let last = self
            .opencode
            .desktop_tabs
            .iter()
            .rev()
            .find(|id| self.opencode.sessions.contains_key(*id))
            .cloned();
        if let Some(id) = last {
            self.views.insert(App::OpenCodeDesktop, Some(id));
        }
    }

    pub fn quit_app(&mut self, app: App) {
        if app == App::OpenCodeDesktop && self.vars.opencode_server_survives_quit {
            if let Some(server) = self.opencode.server.and_then(|s| self.procs.get_mut(&s)) {
                server.ppid = 1;
            }
        }
        if let Some(terminal) = app.terminal() {
            let panes: Vec<u64> = self
                .panes
                .values()
                .filter(|p| p.terminal == terminal)
                .map(|p| p.id)
                .collect();
            for pane in panes {
                self.close_pane(pane);
            }
        }
        if let Some(pid) = self.app_pid(app) {
            self.kill(pid, Exit::Normal);
        }
    }

    /// Start the OpenCode background service if none is running.
    pub fn ensure_server(&mut self, parent: Option<u32>) -> u32 {
        if let Some(pid) = self.opencode.server {
            return pid;
        }
        let pid = self.spawn(bare(
            parent.unwrap_or(1),
            Exe::OpenCodeCli,
            &["opencode-cli", "serve", "--service"],
            Role::Server,
        ));
        self.opencode.server = Some(pid);
        // The desktop runs its bundled CLI; a TUI runs its own install.
        let version = match parent {
            Some(_) => self
                .apps
                .get(&App::OpenCodeDesktop)
                .map(|a| a.version.clone()),
            None => self
                .tools
                .get(&crate::model::Tool::OpenCodeCli)
                .cloned()
                .flatten(),
        };
        self.opencode.server_version = version.unwrap_or_default();
        let file = serde_json::json!({
            "id": "00000000-0000-4000-8000-000000000001",
            "version": self.opencode.server_version,
            "url": "http://127.0.0.1:49374",
            "pid": pid,
            "password": "SYNTHETIC-not-a-secret",
        });
        self.opencode.service_file = serde_json::to_vec(&file).ok();
        pid
    }

    pub fn ensure_daemon(&mut self) -> u32 {
        if let Some(p) = self.procs.values().find(|p| p.role == Role::Daemon) {
            return p.pid;
        }
        self.spawn(bare(
            1,
            Exe::CodexCli,
            &["codex", "app-server", "--listen", "unix://"],
            Role::Daemon,
        ))
    }

    pub fn daemon_pid(&self) -> Option<u32> {
        self.procs
            .values()
            .find(|p| p.role == Role::Daemon)
            .map(|p| p.pid)
    }

    /// Write `<config>/sessions/<pid>.json` in the real format.
    pub(crate) fn write_registry(
        &mut self,
        pid: u32,
        session_id: &str,
        host: Option<&str>,
        entrypoint: &str,
        spare: bool,
        background: bool,
    ) {
        let Some(proc) = self.procs.get(&pid) else {
            return;
        };
        let mut record = serde_json::json!({
            "pid": pid,
            "sessionId": session_id,
            "cwd": proc.cwd,
            "procStart": lstart::format(proc.start),
            "version": "2.1.321",
            "kind": if background { "bg" } else { "interactive" },
            "entrypoint": entrypoint,
            "spare": spare,
        });
        if let (Some(host), Some(map)) = (host, record.as_object_mut()) {
            map.insert("hostSessionId".to_owned(), serde_json::Value::from(host));
        }
        if let Ok(bytes) = serde_json::to_vec(&record) {
            let file = super::world::RegFile {
                bytes,
                mtime: self.clock,
                torn: false,
            };
            self.claude.registry.insert(pid, file);
        }
    }

    /// The desktop keeps a pre-warmed `--await-claim` spare (Claude facts C).
    fn spawn_spare(&mut self, app_pid: u32) {
        let pid = self.spawn(bare(
            app_pid,
            Exe::ClaudeCli,
            &["claude", "--await-claim"],
            Role::Spare,
        ));
        if self.vars.claude_spare_registry_before_claim {
            let named = match self.claude.last_stopped.clone() {
                Some(id) => id,
                None => self.rng.uuid_v4(),
            };
            self.write_registry(pid, &named, None, "claude-desktop", true, false);
        }
    }

    /// Start (or claim a spare for) the process of desktop session `local`.
    pub(crate) fn start_desktop_child(&mut self, local: &str) {
        let Some(app_pid) = self.ensure_app(App::ClaudeDesktop) else {
            return;
        };
        let exists = self
            .procs
            .values()
            .any(|p| p.role == Role::DesktopChild(local.to_owned()));
        let Some(desktop) = self.claude.desktop.get(local).cloned() else {
            return;
        };
        let Some(cli) = desktop.current.clone().or(desktop.unarchived.clone()) else {
            return;
        };
        if exists || desktop.archived {
            return;
        }
        let spare = self
            .procs
            .values()
            .find(|p| p.role == Role::Spare)
            .map(|p| p.pid);
        let session = SessionRef::new(Provider::Claude, cli.clone());
        let pid = match spare.and_then(|s| self.procs.get_mut(&s)) {
            Some(proc) => {
                proc.role = Role::DesktopChild(local.to_owned());
                proc.hosts = Some(session.clone());
                proc.cwd = Some(desktop.cwd.clone());
                proc.by_engine = self.engine_open;
                proc.pid
            }
            None => self.spawn(Proc {
                cwd: Some(desktop.cwd.clone()),
                hosts: Some(session.clone()),
                by_engine: self.engine_open,
                ..bare(
                    app_pid,
                    Exe::ClaudeCli,
                    &["claude", "--resume"],
                    Role::DesktopChild(local.to_owned()),
                )
            }),
        };
        self.write_registry(pid, &cli, Some(local), "claude-desktop", false, false);
        self.note_writer(&session, self.engine_open, Some(local), true);
        self.spawn_spare(app_pid);
    }
}
