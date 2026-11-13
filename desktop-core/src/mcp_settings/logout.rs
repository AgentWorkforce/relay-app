use super::*;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn run(config: &CodexConfig, alias: &str) -> bool {
    let Some(executable) = &config.executable else {
        return false;
    };
    let mut command = Command::new(executable);
    command
        .args(["mcp", "logout", alias])
        .env("CODEX_HOME", &config.home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    agent_relay_node::platform::process::hide_window(&mut command);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}
