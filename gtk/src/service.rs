//! The relay core as a systemd user service, the Linux counterpart of the
//! Mac's LaunchAgent: it keeps agents connected and uploads running while
//! the app is closed, and starts at login.
//!
//! `agent-relay.service` ships with the package (and the per-user install
//! writes it to `~/.config/systemd/user`). Nothing here needs root: every
//! command is `systemctl --user`.
//!
//! On Windows the same [`UserService`] is `LogonTask`: a per-user
//! Task Scheduler task that runs `agent-relay.exe --headless` when this
//! person logs in, driven through `schtasks.exe` (no administrator needed).

use super::updates::{self, CommandOutput};
use std::io;
use std::time::Duration;

pub const UNIT: &str = "agent-relay.service";
const TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows::{LogonTask, TASK};

/// What the Settings rows call the background service where it is missing
/// or a change to it fails: the systemd user unit on Linux, the logon task
/// on Windows.
pub mod wording {
    /// Asked on quitting while Keep agents connected after quitting is off,
    /// in the Mac's words.
    pub const QUIT_OFFLINE_TITLE: &str = "Quit and take your agents offline?";
    pub const QUIT_OFFLINE_DETAIL: &str = "Keep agents connected after quitting is off, so your agents stop receiving messages and uploads stop until you open Agent Relay again.";
    pub const QUIT_ANYWAY: &str = "Quit Anyway";
    pub const KEEP_AGENTS_CONNECTED: &str = "Keep Agents Connected";

    #[cfg(not(windows))]
    pub const NOT_INSTALLED_KEEPS_CONNECTED: &str = "The agent-relay systemd user service is not installed, so your agents connect only once Agent Relay opens after you log in. Install the package or the per-user release to add it.";
    #[cfg(not(windows))]
    pub const NOT_INSTALLED: &str = "The agent-relay systemd user service is not installed. Install the package or the per-user release to add it.";
    #[cfg(not(windows))]
    pub const ENABLE_FAILED: &str = "systemctl could not enable the service.";
    #[cfg(not(windows))]
    pub const DISABLE_FAILED: &str = "systemctl could not turn off the service.";

    #[cfg(windows)]
    pub const NOT_INSTALLED_KEEPS_CONNECTED: &str = "The Agent Relay logon task is not installed, so your agents connect only once Agent Relay opens after you log in. Install the per-user release to add it.";
    #[cfg(windows)]
    pub const NOT_INSTALLED: &str =
        "The Agent Relay logon task is not installed. Install the per-user release to add it.";
    #[cfg(windows)]
    pub const ENABLE_FAILED: &str = "Task Scheduler could not enable the service.";
    #[cfg(windows)]
    pub const DISABLE_FAILED: &str = "Task Scheduler could not turn off the service.";
}

/// Where the unit stands. `installed` is false where systemd or the unit is
/// missing (running from a checkout, a host without systemd).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServiceState {
    pub installed: bool,
    pub enabled: bool,
    pub active: bool,
}

impl ServiceState {
    pub fn label(&self) -> &'static str {
        match (self.installed, self.active, self.enabled) {
            (false, _, _) => "Not installed",
            (true, true, true) => "Running · starts at login",
            (true, true, false) => "Running · does not start at login",
            (true, false, true) => "Stopped · starts at login",
            (true, false, false) => "Off",
        }
    }
}

/// What asking systemd to start the unit came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceStart {
    /// The unit is missing or the person has not enabled it: the app hosts
    /// the core itself.
    NotEnabled,
    /// systemd started the enabled unit.
    Started,
    /// The enabled unit's start failed or did not answer in time. systemd
    /// may still bring it up (a queued job, `Restart=always`), so the socket
    /// stays the service's and no core is hosted beside it.
    Unconfirmed,
}

/// The background service, as the app controls it. Calls block on
/// `systemctl`; run them off the UI thread.
pub trait UserService: Send + Sync {
    fn state(&self) -> ServiceState;

    /// The current login registration, or None when this host has no service
    /// to schedule. An unreadable registration is an error, never "off".
    fn startup_registration(&self) -> Result<Option<bool>, String> {
        let state = self.state();
        Ok(state.installed.then_some(state.enabled))
    }

    /// Starts the unit when the person has enabled it.
    ///
    /// The unit runs `serve --headless`, which applies the agent-led
    /// defaults (direct delivery, automatic activation, sharing only the
    /// sessions chosen). Those are choices made by
    /// enabling the service, never by opening the app.
    fn start(&self) -> ServiceStart;

    /// Changes only the next-login registration, leaving the running core alone.
    /// Opening the app and quitting it are independent of starting at login.
    fn set_start_at_login(&self, enabled: bool) -> Result<(), String>;

    /// Enables and starts the unit; the failure as a sentence.
    fn enable(&self) -> Result<(), String>;

    /// Stops the unit, when the person asks to; the failure as a sentence.
    fn stop(&self) -> Result<(), String>;

    /// Stops the unit and removes its login registration for explicit
    /// service management; the failure as a sentence.
    fn disable(&self) -> Result<(), String>;
}

/// Runs one `systemctl` command.
pub type Runner = dyn Fn(&[&str]) -> io::Result<CommandOutput> + Send + Sync;

/// `agent-relay.service` through `systemctl --user`.
pub struct Systemd {
    runner: Box<Runner>,
    unit: String,
}

impl Default for Systemd {
    fn default() -> Systemd {
        Systemd::new(Box::new(|argv| updates::run(argv, TIMEOUT)), UNIT)
    }
}

impl Systemd {
    pub fn new(runner: Box<Runner>, unit: &str) -> Systemd {
        Systemd {
            runner,
            unit: unit.to_string(),
        }
    }

    fn run(&self, arguments: &[&str]) -> io::Result<CommandOutput> {
        let mut argv = vec!["systemctl", "--user"];
        argv.extend_from_slice(arguments);
        (self.runner)(&argv)
    }

    /// `None` when `systemctl` could not run at all.
    fn systemctl(&self, arguments: &[&str]) -> Option<CommandOutput> {
        self.run(arguments).ok()
    }

    /// The unit's state; an error only when `systemctl` did not answer.
    fn read_state(&self) -> io::Result<ServiceState> {
        let shown = self.run(&[
            "show",
            &self.unit,
            "--property=LoadState,UnitFileState,ActiveState",
        ])?;
        if shown.code != Some(0) {
            return Ok(ServiceState::default());
        }
        let value = |key: &str| {
            shown
                .stdout
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .unwrap_or_default()
        };
        if value("LoadState") != "loaded" {
            return Ok(ServiceState::default());
        }
        Ok(ServiceState {
            installed: true,
            enabled: matches!(
                value("UnitFileState"),
                "enabled" | "enabled-runtime" | "linked"
            ),
            active: matches!(value("ActiveState"), "active" | "activating" | "reloading"),
        })
    }

    fn report(&self, done: Option<CommandOutput>, fallback: &str) -> Result<(), String> {
        let done = done.ok_or_else(|| "systemd is not available on this computer.".to_string())?;
        if done.code == Some(0) {
            return Ok(());
        }
        let detail = if done.stderr.trim().is_empty() {
            done.stdout.trim()
        } else {
            done.stderr.trim()
        };
        Err(if detail.is_empty() {
            fallback.to_string()
        } else {
            detail.to_string()
        })
    }
}

impl UserService for Systemd {
    fn state(&self) -> ServiceState {
        self.read_state().unwrap_or_default()
    }

    fn startup_registration(&self) -> Result<Option<bool>, String> {
        match self.read_state() {
            Ok(state) => Ok(state.installed.then_some(state.enabled)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("systemd could not read the login setting.".into()),
        }
    }

    fn start(&self) -> ServiceStart {
        let state = match self.read_state() {
            Ok(state) => state,
            // A systemd too busy to answer may still run an enabled unit.
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return ServiceStart::Unconfirmed
            }
            Err(_) => return ServiceStart::NotEnabled,
        };
        if !state.installed || !state.enabled {
            return ServiceStart::NotEnabled;
        }
        // `systemctl start` waits for the job; one that failed or was cut
        // off by the timeout is no proof that no job was queued.
        match self.run(&["start", &self.unit]) {
            Ok(started) if started.code == Some(0) => ServiceStart::Started,
            _ => ServiceStart::Unconfirmed,
        }
    }

    fn set_start_at_login(&self, enabled: bool) -> Result<(), String> {
        let state = self
            .read_state()
            .map_err(|_| "systemd is not available on this computer.")?;
        if !state.installed {
            return if enabled {
                Err(wording::NOT_INSTALLED.into())
            } else {
                Ok(())
            };
        }
        let action = if enabled { "enable" } else { "disable" };
        self.report(
            self.systemctl(&[action, &self.unit]),
            if enabled {
                wording::ENABLE_FAILED
            } else {
                wording::DISABLE_FAILED
            },
        )
    }

    fn enable(&self) -> Result<(), String> {
        let done = self.systemctl(&["enable", "--now", &self.unit]);
        self.report(done, "systemctl could not enable the service.")
    }

    fn stop(&self) -> Result<(), String> {
        let done = self.systemctl(&["stop", &self.unit]);
        self.report(done, "systemctl could not stop the service.")
    }

    fn disable(&self) -> Result<(), String> {
        let done = self.systemctl(&["disable", "--now", &self.unit]);
        self.report(done, "systemctl could not turn off the service.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A `systemctl` answering each subcommand from `answers` (exit code,
    /// stdout), and every command it ran.
    fn systemd(answers: &[(&str, i32, &str)]) -> (Systemd, Arc<Mutex<Vec<Vec<String>>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let answers: Vec<(String, i32, String)> = answers
            .iter()
            .map(|(command, code, out)| (command.to_string(), *code, out.to_string()))
            .collect();
        let seen = calls.clone();
        let runner = move |argv: &[&str]| {
            seen.lock()
                .unwrap()
                .push(argv.iter().map(|a| a.to_string()).collect());
            let (code, stdout) = answers
                .iter()
                .find(|(command, _, _)| command == argv[2])
                .map(|(_, code, out)| (*code, out.clone()))
                .unwrap_or((0, String::new()));
            Ok(CommandOutput {
                code: Some(code),
                stdout,
                stderr: String::new(),
            })
        };
        (Systemd::new(Box::new(runner), UNIT), calls)
    }

    #[test]
    fn state_reads_systemctl_show() {
        let (service, _) = systemd(&[(
            "show",
            0,
            "LoadState=loaded\nUnitFileState=enabled\nActiveState=active\n",
        )]);
        let state = service.state();
        assert!(state.installed && state.enabled && state.active);
        assert_eq!(state.label(), "Running · starts at login");
        assert_eq!(service.start(), ServiceStart::Started);

        // A unit nobody enabled is never started by opening the app: its
        // headless mode applies the agent-led defaults.
        let (disabled, calls) = systemd(&[(
            "show",
            0,
            "LoadState=loaded\nUnitFileState=disabled\nActiveState=inactive\n",
        )]);
        assert_eq!(disabled.start(), ServiceStart::NotEnabled);
        assert!(!calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call == &["systemctl", "--user", "start", UNIT]));

        let (missing, _) = systemd(&[(
            "show",
            0,
            "LoadState=not-found\nUnitFileState=\nActiveState=inactive\n",
        )]);
        assert!(!missing.state().installed);
        assert_eq!(missing.start(), ServiceStart::NotEnabled);
    }

    #[test]
    fn an_enabled_unit_whose_start_is_unconfirmed_keeps_the_socket() {
        const ENABLED: &str = "LoadState=loaded\nUnitFileState=enabled\nActiveState=inactive\n";
        let (failed, _) = systemd(&[("show", 0, ENABLED), ("start", 1, "")]);
        assert_eq!(failed.start(), ServiceStart::Unconfirmed);

        // `systemctl` cut off by the timeout, while showing or starting.
        let answering = |timed_out: &'static str| {
            Systemd::new(
                Box::new(move |argv: &[&str]| {
                    if argv[2] == timed_out {
                        return Err(io::Error::from(io::ErrorKind::TimedOut));
                    }
                    Ok(CommandOutput {
                        code: Some(0),
                        stdout: ENABLED.to_string(),
                        stderr: String::new(),
                    })
                }),
                UNIT,
            )
        };
        assert_eq!(answering("start").start(), ServiceStart::Unconfirmed);
        assert_eq!(answering("show").start(), ServiceStart::Unconfirmed);
    }

    #[test]
    fn login_registration_does_not_start_or_stop_the_running_core() {
        let (service, calls) = systemd(&[(
            "show",
            0,
            "LoadState=loaded\nUnitFileState=enabled\nActiveState=active\n",
        )]);
        service.set_start_at_login(false).unwrap();
        service.set_start_at_login(true).unwrap();
        let calls = calls.lock().unwrap();
        assert!(calls
            .iter()
            .any(|call| call == &["systemctl", "--user", "disable", UNIT]));
        assert!(calls
            .iter()
            .any(|call| call == &["systemctl", "--user", "enable", UNIT]));
        assert!(!calls.iter().any(|call| call
            .iter()
            .any(|arg| matches!(arg.as_str(), "--now" | "start" | "stop" | "restart"))));
    }

    #[test]
    fn missing_unit_can_be_off_but_cannot_be_enabled() {
        let (service, _) = systemd(&[("show", 0, "LoadState=not-found\n")]);
        assert_eq!(service.set_start_at_login(false), Ok(()));
        assert_eq!(
            service.set_start_at_login(true),
            Err(wording::NOT_INSTALLED.into())
        );
    }

    #[test]
    fn enable_and_stop_report_failures() {
        let (service, calls) = systemd(&[("enable", 1, ""), ("stop", 0, ""), ("disable", 1, "")]);
        assert!(service.enable().unwrap_err().contains("could not enable"));
        assert_eq!(service.stop(), Ok(()));
        assert!(service
            .disable()
            .unwrap_err()
            .contains("could not turn off"));
        assert!(calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call == &["systemctl", "--user", "disable", "--now", UNIT]));
        assert!(calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call == &["systemctl", "--user", "enable", "--now", UNIT]));
    }

    #[test]
    fn without_systemctl_nothing_is_installed() {
        let service = Systemd::new(
            Box::new(|_: &[&str]| Err(io::Error::from(io::ErrorKind::NotFound))),
            UNIT,
        );
        assert_eq!(service.state(), ServiceState::default());
        assert_eq!(service.state().label(), "Not installed");
        assert_eq!(service.start(), ServiceStart::NotEnabled);
        assert_eq!(
            service.enable(),
            Err("systemd is not available on this computer.".into())
        );
    }

    #[test]
    fn startup_registration_distinguishes_missing_systemd_from_failed_reads() {
        let missing = Systemd::new(
            Box::new(|_| Err(io::Error::from(io::ErrorKind::NotFound))),
            UNIT,
        );
        assert_eq!(missing.startup_registration(), Ok(None));
        let timed_out = Systemd::new(
            Box::new(|_| Err(io::Error::from(io::ErrorKind::TimedOut))),
            UNIT,
        );
        assert!(timed_out.startup_registration().is_err());
        let (not_running, _) = systemd(&[("show", 1, "")]);
        assert_eq!(not_running.startup_registration(), Ok(None));
        let (no_unit, _) = systemd(&[("show", 0, "LoadState=not-found\n")]);
        assert_eq!(no_unit.startup_registration(), Ok(None));
        let (enabled, _) = systemd(&[("show", 0, "LoadState=loaded\nUnitFileState=enabled\n")]);
        assert_eq!(enabled.startup_registration(), Ok(Some(true)));
    }

    #[test]
    fn labels_name_every_state() {
        let state = |enabled, active| ServiceState {
            installed: true,
            enabled,
            active,
        };
        assert_eq!(
            state(false, true).label(),
            "Running · does not start at login"
        );
        assert_eq!(state(true, false).label(), "Stopped · starts at login");
        assert_eq!(state(false, false).label(), "Off");
    }
}
