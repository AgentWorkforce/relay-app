use super::*;
use agent_relay_node::relay::process::{ProcessTable, SystemProcesses};
use std::process::{Command, Stdio};

mod output;

pub(super) struct Native;
impl Native {
    fn command(command: &Invocation) -> Command {
        let mut child = Command::new(&command.executable);
        child
            .args(&command.args)
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        if let Some(cwd) = &command.cwd {
            child.current_dir(cwd);
        }
        if command.clear_provider_environment {
            for key in PROVIDER_ENVIRONMENT_KEYS {
                child.env_remove(key);
            }
        }
        for (key, value) in &command.environment {
            child.env(key, value);
        }
        child
    }
}
impl Host for Native {
    fn default_terminal(&self) -> Option<String> {
        let terminals: &[&str] = if cfg!(windows) {
            &["wt.exe"]
        } else {
            &[
                "xdg-terminal-exec",
                "x-terminal-emulator",
                "gnome-terminal",
                "konsole",
                "xfce4-terminal",
                "xterm",
            ]
        };
        let search = std::env::var_os("PATH")?;
        terminals.iter().find_map(|name| {
            std::env::split_paths(&search)
                .map(|path| path.join(name))
                .find(|path| path.is_file())
                .map(|path| path.to_string_lossy().into_owned())
        })
    }

    fn process_matches(&self, location: &SessionLocation) -> bool {
        let processes = SystemProcesses::default();
        let evidence = |pid| {
            processes.process(pid).map(|process| ProcessEvidence {
                pid: process.pid,
                uid: process.uid,
                start: process.start,
                zombie: processes.zombie(pid),
                executable: processes
                    .executable(pid)
                    .map(|path| path.to_string_lossy().into_owned()),
            })
        };
        let owner = evidence(location.pid);
        let host = location.host_pid.and_then(evidence);
        #[cfg(target_os = "linux")]
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|s| s.trim().to_owned());
        #[cfg(not(target_os = "linux"))]
        let boot = location.boot_id.clone();
        if cfg!(target_os = "linux") && location.boot_id.is_none() {
            return false;
        }
        process_evidence_matches(
            location,
            owner.as_ref(),
            host.as_ref(),
            agent_relay_node::relay::peer::current_uid(),
            boot.as_deref(),
        )
    }
    fn run(&self, command: &Invocation) -> Result<String, String> {
        output::run(Self::command(command), std::time::Duration::from_secs(5))
    }

    fn dispatch(&self, command: &Invocation) -> Result<(), String> {
        let mut child = Self::command(command)
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err("The operating system rejected the session link. No replacement session was started.".into())
                    }
                }
                Ok(None) if started.elapsed() < std::time::Duration::from_secs(5) => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                _ => {
                    // The opener can exec the requested application. Never kill
                    // it on timeout and never infer that a second launch is safe.
                    std::thread::spawn(move || child.wait());
                    // Dispatch is known; provider selection is unknown. Returning
                    // Dispatched suppresses the stopped-session retry UI here.
                    return Ok(());
                }
            }
        }
    }
    fn launch(&self, command: &Invocation) -> Result<(), String> {
        let mut child = Self::command(command)
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn’t start the original terminal: {e}"))?;
        std::thread::spawn(move || child.wait());
        Ok(())
    }
    fn window(&self, location: &SessionLocation) -> Result<Outcome, String> {
        #[cfg(windows)]
        return windows::activate(location, self);
        #[cfg(not(windows))]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                return Err("This Wayland compositor does not expose verified window activation. The terminal request may have been dispatched.".into());
            }
            let window = location
                .window_id
                .as_deref()
                .ok_or("No native window identity was recorded.")?;
            if !(numeric(window)
                || window
                    .strip_prefix("0x")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_hexdigit())))
            {
                return Err("Invalid native window identity.".into());
            }
            let owner = self.run(&Invocation::new(
                "xdotool",
                ["getwindowpid".into(), window.into()],
            ))?;
            if owner.trim().parse::<i32>().ok() != location.host_pid {
                return Err("The recorded window now belongs to another process.".into());
            }
            verify(self, location)?;
            self.run(&Invocation::new(
                "xdotool",
                ["windowactivate".into(), "--sync".into(), window.into()],
            ))?;
            let active = self.run(&Invocation::new("xdotool", ["getactivewindow".into()]))?;
            let parse = |s: &str| {
                s.strip_prefix("0x")
                    .map(|s| u64::from_str_radix(s, 16).ok())
                    .unwrap_or_else(|| s.parse().ok())
            };
            if parse(active.trim()) == parse(window) {
                Ok(Outcome::Verified)
            } else {
                Ok(Outcome::Dispatched)
            }
        }
    }
}
