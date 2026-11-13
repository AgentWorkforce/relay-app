//! Starting a collector process that outlives the one starting it.
use super::{append_log, now, open_log, read_config, user_error};
use crate::history::cloud;
use anyhow::Result;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// The Mac app bundles a hard link to its core as `<core> (Sessions)`, so
/// Activity Monitor tells the collector apart from the relay core. A link,
/// not a symlink: the process takes the name it was started by.
const SESSIONS_SUFFIX: &str = " (Sessions)";

/// The executable a detached collector runs as: the bundle's Sessions link
/// beside `executable` when there is one, otherwise `executable` itself.
fn collector_executable(executable: PathBuf) -> PathBuf {
    let Some(name) = executable.file_name().and_then(|name| name.to_str()) else {
        return executable;
    };
    if name.ends_with(SESSIONS_SUFFIX) {
        return executable;
    }
    let sessions = executable.with_file_name(format!("{name}{SESSIONS_SUFFIX}"));
    if sessions.is_file() {
        sessions
    } else {
        executable
    }
}

/// A command for a process that outlives this one. It never inherits the
/// one-shot Cloud bearer setup was handed: renewal would keep presenting it
/// after it expires instead of the desktop's refreshed sign-in.
fn detached_command(executable: PathBuf) -> Command {
    let mut command = Command::new(executable);
    command.env_remove(cloud::ACCESS_TOKEN_VARIABLE);
    command
}

pub fn start_background(directory: &Path) -> Result<()> {
    let executable = collector_executable(std::env::current_exe()?);
    let startup_id = format!("{}-{}", std::process::id(), now());
    let mut log = open_log(directory)?;
    writeln!(log, "Starting Agent Relay Probe")?;
    let mut command = detached_command(executable);
    command
        .arg("run")
        .arg("--directory")
        .arg(directory)
        .arg("--startup-id")
        .arg(&startup_id)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    // Detached from this terminal: `setsid` on Unix; on Windows a console
    // process group of its own and no window.
    agent_relay_node::platform::process::new_session(&mut command);
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Err(user_error(
                "Probe stopped during startup. Run setup in --foreground to retry.",
            ));
        }
        let ready = fs::read(directory.join("runtime.json"))
            .ok()
            .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
            .is_some_and(|value| value["startup_id"] == startup_id && value["ready"] == true);
        if ready {
            humanln!("Probe is running in the background. You can close this terminal.");
            humanln!("Run setup again after restarting the computer.");
            let config = read_config(directory)?;
            humanln!(
                "Stop: agent-relay-probe stop --site-url {} --workspace {} --account {}",
                config.site_url,
                config.workspace_id,
                config.account_id
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if child.try_wait()?.is_none() {
        child.kill()?;
        child.wait()?;
    }
    Err(user_error("Probe startup timed out. Run setup again."))
}

pub(super) fn spawn_replacement_handoff(directory: &Path, startup_id: &str) -> Result<()> {
    let executable = std::env::current_exe()?;
    // The old collector still owns its streams. Rotating from this process
    // would strand them on the renamed file until it exits.
    let log = append_log(directory)?;
    let mut command = detached_command(executable);
    command
        .arg("complete-replacement")
        .arg("--directory")
        .arg(directory)
        .arg("--startup-id")
        .arg(startup_id)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    // A session of its own (Unix `setsid`; on Windows a console process
    // group and no window), so it outlives the collector it replaces.
    agent_relay_node::platform::process::new_session(&mut command);
    command.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detached_process_never_inherits_the_setup_bearer() {
        let command = detached_command(PathBuf::from("agent-relay-probe"));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == cloud::ACCESS_TOKEN_VARIABLE && value.is_none()));
    }

    #[test]
    fn the_collector_runs_as_the_bundled_sessions_link_when_there_is_one() {
        let dir = tempfile::tempdir().unwrap();
        let core = dir.path().join("Agent Relay Helper");
        fs::write(&core, b"core").unwrap();
        // ~/.local/bin/agent-relay-probe and the Linux core have no link.
        assert_eq!(collector_executable(core.clone()), core);

        let sessions = dir.path().join("Agent Relay Helper (Sessions)");
        fs::hard_link(&core, &sessions).unwrap();
        assert_eq!(collector_executable(core), sessions);
        // A collector that starts its replacement keeps its own name.
        assert_eq!(collector_executable(sessions.clone()), sessions);
    }
}
