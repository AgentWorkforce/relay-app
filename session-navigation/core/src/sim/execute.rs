//! `Execute` for the fake machine: mutate truth as the real OS and apps
//! would, hazards included. Like the real AppleScript, focusing a pane looks
//! it up by tty only; identity checks are the planner's job.
use super::truth::{command_target, Exit};
use super::world::{LogEntry, PendingLaunch, World};
use crate::evidence::Reservation;
use crate::model::{App, Provider, Reason, SessionRef, TerminalKind};
use crate::plan::{Command, Dispatch, Step};
use crate::ports::{Execute, Reservations, StoreError};

impl World {
    fn automation_ok(&self, app: App) -> bool {
        self.automation.get(&app).copied().unwrap_or(false)
    }

    fn log_exec(&mut self, step: &Step, target_hosted: Vec<SessionRef>) {
        self.log.push(LogEntry::Exec {
            open: self.engine_open,
            step: step.clone(),
            target_hosted,
            acted: true,
        });
    }

    /// Everything process `pid` hosts now (first) or hosted earlier.
    pub fn hosted_by(&self, pid: u32) -> Vec<SessionRef> {
        let Some(proc) = self.procs.get(&pid) else {
            return Vec::new();
        };
        let mut out: Vec<SessionRef> = proc.hosts.iter().cloned().collect();
        if let Some(past) = self.past_hosts.get(&(pid, proc.start)) {
            out.extend(past.iter().cloned());
        }
        out
    }

    /// [`World::hosted_by`] plus the thread a TUI waiting at the Codex
    /// update menu will show once it is dismissed: its window is the
    /// session's, though it presents nothing yet.
    pub fn hosted_or_waiting(&self, pid: u32) -> Vec<SessionRef> {
        let mut out = self.hosted_by(pid);
        out.extend(self.codex.stalled.get(&pid).cloned());
        out
    }

    pub(crate) fn focus_pane(&mut self, terminal: TerminalKind, tty: &str) -> Dispatch {
        if !self.running(terminal.app()) {
            return Dispatch::Failed(Reason::TargetGone);
        }
        if !self.automation_ok(terminal.app()) {
            return Dispatch::Refused;
        }
        let Some((pane, window)) = self.pane_by_tty(terminal, tty).map(|p| (p.id, p.window)) else {
            return Dispatch::Failed(Reason::TargetGone);
        };
        self.hosts.active.insert(window, pane);
        self.front_app = Some(terminal.app());
        self.front_pane = Some(pane);
        Dispatch::Dispatched
    }

    fn open_url(&mut self, url: &str) -> Dispatch {
        let scheme = url.split("://").next().unwrap_or_default();
        let Some(bundle) = self.schemes.get(scheme).cloned() else {
            return Dispatch::Failed(Reason::DispatchFailed);
        };
        // The OS opens the URL with whichever bundle handles the scheme.
        let Some(app) = App::from_bundle_id(&bundle) else {
            return Dispatch::Dispatched;
        };
        if !self.installed(app) {
            return Dispatch::Failed(Reason::AppNotInstalled);
        }
        self.deliver_url(app, url);
        Dispatch::Dispatched
    }

    fn activate(&mut self, app: App) -> Dispatch {
        if self.ensure_app(app).is_none() {
            return Dispatch::Failed(Reason::AppNotInstalled);
        }
        self.front_app = Some(app);
        self.front_pane = app.terminal().and_then(|t| {
            self.panes
                .values()
                .filter(|p| p.terminal == t)
                .map(|p| p.id)
                .next_back()
        });
        Dispatch::Dispatched
    }

    fn launch(
        &mut self,
        terminal: TerminalKind,
        command: &Command,
        context: &crate::plan::LaunchContext,
    ) -> Dispatch {
        if !self.installed(terminal.app()) {
            return Dispatch::Failed(Reason::AppNotInstalled);
        }
        let mac = self.platform == crate::model::Platform::MacOs;
        if mac && !self.automation_ok(terminal.app()) {
            return Dispatch::Refused;
        }
        let session = command_target(command);
        let others = session.as_ref().map_or(0, |s| {
            // A new client of the Codex daemon joins its writer (N10).
            let writers = if s.provider == Provider::Codex && self.codex_daemon_mode() {
                self.codex_writers_beside_daemon(s)
            } else {
                self.writers(s)
            };
            writers.saturating_add(self.desktop_holders(s, None))
        });
        let located = session.as_ref().map_or(0, |s| self.located_clients(s));
        self.log.push(LogEntry::LaunchDispatched {
            open: self.engine_open,
            session,
            others,
            located,
        });
        let wsl = matches!(context.runtime, crate::model::Runtime::Wsl { .. });
        if wsl && self.platform != crate::model::Platform::Windows {
            return Dispatch::Failed(Reason::DispatchFailed);
        }
        let Some(pane) = self.open_launch_pane(terminal, command, wsl) else {
            return Dispatch::Failed(Reason::DispatchFailed);
        };
        // Only Ghostty and Terminal report the new window's tty (native
        // N17); `wt.exe` and Linux `-e` launches report nothing.
        let reports = self.platform == crate::model::Platform::MacOs
            && matches!(
                terminal,
                TerminalKind::Ghostty | TerminalKind::AppleTerminal
            );
        let tty = self
            .panes
            .get(&pane)
            .filter(|_| reports)
            .map(|p| p.tty.clone());
        if self.launch_delay == 0 {
            self.run_in_pane(pane, command, self.engine_open);
        } else {
            self.pending.push(PendingLaunch {
                due: self.clock.saturating_add(self.launch_delay),
                pane,
                command: command.clone(),
                by_engine: self.engine_open,
            });
        }
        match tty {
            Some(tty) => Dispatch::Opened { tty },
            None => Dispatch::Dispatched,
        }
    }

    /// Panes whose foreground agent shows `session` now and was started with
    /// its id in argv: a window a planner can find without guessing.
    pub fn located_clients(&self, session: &SessionRef) -> usize {
        self.panes
            .values()
            .filter_map(|pane| self.pane_agent(pane))
            .filter(|p| p.hosts.as_ref() == Some(session) && p.argv.contains(&session.id))
            .count()
    }

    /// A stop request: the process exits after `exit_delay` observations
    /// (it may take a while to shut down), or at once.
    fn stop(&mut self, pid: u32, start: u64) -> Dispatch {
        let matches = self.procs.get(&pid).is_some_and(|p| p.start == start);
        if !matches {
            return Dispatch::Failed(Reason::TargetGone);
        }
        if self.exit_delay == 0 {
            self.kill(pid, Exit::Normal);
        } else {
            self.stopping.insert(pid, self.exit_delay);
        }
        Dispatch::Dispatched
    }

    /// Time passes for processes that were asked to stop.
    pub(crate) fn progress_stops(&mut self, by: u64) {
        let mut due = Vec::new();
        for (pid, left) in self.stopping.iter_mut() {
            *left = left.saturating_sub(by);
            if *left == 0 {
                due.push(*pid);
            }
        }
        for pid in due {
            self.stopping.remove(&pid);
            self.kill(pid, Exit::Normal);
        }
    }

    /// Let time pass; launches whose delay has elapsed start their process.
    pub fn advance(&mut self, seconds: u64) {
        self.clock = self.clock.saturating_add(seconds);
        let (due, later): (Vec<PendingLaunch>, Vec<PendingLaunch>) =
            std::mem::take(&mut self.pending)
                .into_iter()
                .partition(|p| p.due <= self.clock);
        self.pending = later;
        for launch in due {
            self.run_in_pane(launch.pane, &launch.command, launch.by_engine);
        }
        self.progress_stops(seconds);
        self.repair_torn_registry();
        if seconds > 0 {
            self.hosts.hidden_ttys.clear();
        }
    }

    /// A live process rewrites its registry file on its next status update.
    fn repair_torn_registry(&mut self) {
        let torn: Vec<u32> = self
            .claude
            .registry
            .iter()
            .filter(|(pid, f)| f.torn && self.procs.contains_key(pid))
            .map(|(pid, _)| *pid)
            .collect();
        for pid in torn {
            if let Some(file) = self.claude.registry.get_mut(&pid) {
                file.torn = false;
            }
        }
    }
}

impl Execute for World {
    fn execute(&mut self, step: &Step) -> Dispatch {
        self.strike(step);
        let target = self.target_of(step);
        let hosted = target
            .map(|pid| self.hosted_or_waiting(pid))
            .unwrap_or_default();
        self.log_exec(step, hosted);
        let at = self.log.len().saturating_sub(1);
        let dispatch = match step {
            Step::FocusPane {
                terminal,
                pane,
                process,
            } => self.focus_ref(*terminal, pane, process),
            Step::OpenUrl { url, .. } => self.open_url(url),
            Step::ActivateApp { app } => self.activate(*app),
            Step::Launch {
                terminal,
                command,
                context,
            } => self.launch(*terminal, command, context),
            Step::Stop { process } => self.stop(process.pid, process.start),
            Step::AwaitExit { .. } => Dispatch::Dispatched,
        };
        if matches!(dispatch, Dispatch::Failed(_) | Dispatch::Refused) {
            if let Some(LogEntry::Exec { acted, .. }) = self.log.get_mut(at) {
                *acted = false;
            }
        }
        dispatch
    }
}

impl Reservations for World {
    fn reserve(&mut self, reservation: Reservation) -> Result<(), StoreError> {
        if !self.channels.reservations {
            return Err(StoreError);
        }
        self.reservations
            .insert(reservation.session.clone(), reservation);
        Ok(())
    }

    fn release(&mut self, session: &SessionRef) -> Result<(), StoreError> {
        if !self.channels.reservations {
            return Err(StoreError);
        }
        self.reservations.remove(session);
        Ok(())
    }
}
