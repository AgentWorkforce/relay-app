//! World-variable triggers for the variables added with the terminal hosts
//! and Grok: an executed step or a person event whose effect the variable
//! decides.
use session_navigation::engine::OpenReport;
use session_navigation::evidence::Fact;
use session_navigation::hosts::PaneRef;
use session_navigation::model::{DisplayServer, Platform, SessionRef, TerminalKind, Tool};
use session_navigation::plan::Step;

use super::Probe;

impl Probe {
    fn pane_trigger(&mut self, pane: &PaneRef) {
        let wayland = self.world.platform
            == Platform::Linux {
                session: Fact::Known(DisplayServer::Wayland),
            };
        match pane {
            PaneRef::KittyWindow { .. } | PaneRef::WezTermPane { .. } if wayland => self
                .var_trigger(
                    "wayland_pane_focus_raises",
                    self.vars.wayland_pane_focus_raises,
                ),
            PaneRef::KittyWindow { .. } | PaneRef::WezTermPane { .. } => {
                self.var_trigger("pane_api_focus_raises", self.vars.pane_api_focus_raises)
            }
            PaneRef::X11Window { .. } => {
                self.var_trigger("x11_activation_honored", self.vars.x11_activation_honored)
            }
            PaneRef::WindowsWindow { .. } => self.var_trigger(
                "windows_foreground_allowed",
                self.vars.windows_foreground_allowed,
            ),
            PaneRef::TmuxPane { outer, .. } => self.pane_trigger(outer),
            PaneRef::Tty { .. } => {}
        }
    }

    pub(super) fn host_var_triggers(&mut self, report: &OpenReport) {
        for (step, _) in &report.executed {
            match step {
                Step::FocusPane { pane, .. } => self.pane_trigger(pane),
                Step::Launch {
                    terminal, command, ..
                } => {
                    if *terminal == TerminalKind::WindowsTerminal {
                        self.var_trigger(
                            "wt_new_tab_joins_window",
                            self.vars.wt_new_tab_joins_window,
                        );
                    }
                    if Tool::from_executable(&command.program) == Some(Tool::GrokCli) {
                        self.grok_launch_triggers();
                    }
                }
                _ => {}
            }
        }
        let consoles = self
            .world
            .panes
            .values()
            .any(|p| p.terminal == TerminalKind::ConsoleHost && p.tmux.is_none());
        if self.world.platform == Platform::Windows && consoles {
            self.var_trigger(
                "console_window_owner_is_client",
                self.vars.console_window_owner_is_client,
            );
        }
    }

    fn grok_launch_triggers(&mut self) {
        self.var_trigger("grok_resume_flag_works", self.vars.grok_resume_flag_works);
        self.var_trigger(
            "grok_resumed_tui_registers",
            self.vars.grok_resumed_tui_registers,
        );
    }

    pub(super) fn grok_person_triggers(&mut self, s: &SessionRef) {
        self.grok_launch_triggers();
        let stored = self.world.grok.sessions.get(&s.id).cloned();
        match stored {
            None => self.var_trigger(
                "grok_resume_missing_creates",
                self.vars.grok_resume_missing_creates,
            ),
            Some(own) => {
                if own.subagent {
                    self.var_trigger("grok_subagent_resumable", self.vars.grok_subagent_resumable);
                }
                if self.world.grok_person_dir(s).as_deref() != Some(own.cwd.as_str()) {
                    self.var_trigger("grok_resume_needs_cwd", self.vars.grok_resume_needs_cwd);
                    self.var_trigger(
                        "grok_resume_missing_creates",
                        self.vars.grok_resume_missing_creates,
                    );
                }
            }
        }
    }
}
