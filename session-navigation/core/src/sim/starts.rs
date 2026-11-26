//! A person starting a CLI session in any terminal of the platform: plain,
//! inside tmux (attached or detached), or inside WSL.
use super::world::World;
use crate::evidence::{CodexOrigin, Entrypoint, Exe};
use crate::model::{Platform, Provider, SessionRef, TerminalKind};

/// How the terminal session is hosted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mux {
    None,
    /// In a tmux session with a client in a window of the terminal.
    Attached,
    /// In a tmux session no client shows.
    Detached,
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

impl World {
    /// Start a `provider` CLI session in the terminal at `index` of
    /// [`World::terminal_choices`].
    pub(crate) fn start_cli(
        &mut self,
        provider: Provider,
        index: u8,
        mux: Mux,
        wsl: bool,
        with_id: bool,
        dir: &str,
    ) -> Option<SessionRef> {
        let windows = self.platform == Platform::Windows;
        if provider == Provider::Grok && windows {
            return None; // Grok offers no Windows route (facts-grok 3).
        }
        let choices = self.terminal_choices();
        let wsl = wsl && windows;
        // WSL sessions run in Windows Terminal.
        let terminal = if wsl {
            TerminalKind::WindowsTerminal
        } else {
            *choices.get(usize::from(index) % choices.len().max(1))?
        };
        // WSL sessions live at POSIX paths inside the distribution.
        let dir = if wsl {
            super::world::DIRS
                .first()
                .copied()
                .unwrap_or("/")
                .to_owned()
        } else {
            dir.to_owned()
        };
        let pane = match mux {
            Mux::None => self.open_pane_with(terminal, &dir, None, wsl)?,
            _ if windows => return None,
            Mux::Attached => self.tmux_new_session(Some(terminal), &dir)?,
            Mux::Detached => self.tmux_new_session(None, &dir)?,
        };
        let session = match provider {
            Provider::Claude => {
                let cli = self.new_session_id(Provider::Claude);
                self.claude.transcripts.insert(
                    cli.clone(),
                    super::world::ClaudeTranscript {
                        cwd: dir.clone(),
                        entrypoint: Entrypoint::Cli,
                    },
                );
                let session = SessionRef::new(Provider::Claude, cli.clone());
                let pid = self.spawn_in_pane(
                    pane,
                    Exe::ClaudeCli,
                    argv(&["claude"]),
                    &dir,
                    Some(session.clone()),
                    None,
                )?;
                self.write_registry(pid, &cli, None, "cli", false, false);
                session
            }
            Provider::Codex => {
                let id = self.new_thread_at(&dir, CodexOrigin::Tui);
                let session = SessionRef::new(Provider::Codex, id);
                let daemon_mode = self.codex_daemon_mode();
                let pid = self.spawn_in_pane(
                    pane,
                    Exe::CodexCli,
                    argv(&["codex"]),
                    &dir,
                    Some(session.clone()),
                    None,
                )?;
                if daemon_mode {
                    self.ensure_daemon();
                } else {
                    self.codex.own_lock.insert(pid);
                }
                session
            }
            Provider::OpenCode => {
                self.ensure_server(None);
                let id = self.new_opencode_at(&dir);
                let args = if with_id {
                    argv(&["opencode", "-s", &id])
                } else {
                    argv(&["opencode"])
                };
                let session = SessionRef::new(Provider::OpenCode, id.clone());
                self.spawn_in_pane(
                    pane,
                    Exe::OpenCodeCli,
                    args,
                    &dir,
                    Some(session.clone()),
                    None,
                )?;
                self.opencode.tui_tabs.insert(id);
                session
            }
            Provider::Grok => {
                let id = self.new_grok(&dir, false);
                let args = if with_id {
                    argv(&["grok", "--resume", &id])
                } else {
                    argv(&["grok"])
                };
                let session = SessionRef::new(Provider::Grok, id.clone());
                let pid = self.spawn_in_pane(
                    pane,
                    Exe::GrokCli,
                    args,
                    &dir,
                    Some(session.clone()),
                    None,
                )?;
                self.grok_register(pid, &id, &dir);
                session
            }
        };
        if wsl {
            self.wsl_sessions.insert(session.clone());
        }
        if terminal == TerminalKind::WindowsTerminal && self.hosts.rng.chance(50) {
            let profile = format!(
                "{{574e775e-4f2a-5b96-ac1e-a2962a40{:04x}}}",
                self.hosts.rng.below(0xffff)
            );
            self.profiles.insert(session.clone(), profile);
        }
        self.record_session(session.clone(), None);
        Some(session)
    }
}
