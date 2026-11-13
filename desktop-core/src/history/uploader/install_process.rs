//! Setup helper lifecycle: streamed events, cancellation, and bounded pipe cleanup.
use super::*;
use tokio::io::{AsyncBufReadExt, BufReader};

impl ProcessRunner {
    pub(super) fn start_install(
        &self,
        args: Vec<String>,
        bearer: Option<Secret>,
    ) -> Result<InstallRun, UploaderError> {
        let command_name = args.first().cloned().unwrap_or_else(|| "cloud".into());
        let mut command = self.command(&args);
        if let Some(bearer) = bearer {
            command.env("CLOUD_API_ACCESS_TOKEN", bearer.expose());
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let (mut child, registration) = match self.spawn(command, generation) {
            Ok(spawned) => spawned,
            Err(error) => {
                if !error.cancelled {
                    log_helper_failure(&command_name, &error.message);
                }
                return Err(error);
            }
        };
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = Capture::start(child.stderr.take(), 64 * 1024);
        let (events, receiver) = mpsc::unbounded_channel();
        let (done, finished) = tokio::sync::oneshot::channel();
        let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let mut reader = tokio::spawn(async move {
                let Some(stdout) = stdout else { return };
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(event @ Value::Object(_)) = serde_json::from_str::<Value>(&line) {
                        let _ = events.send(event);
                    }
                }
            });
            let mut cancelled = false;
            let status = tokio::select! {
                status = child.wait() => status.ok(),
                _ = &mut stopped => {
                    cancelled = true;
                    if let Some(pid) = pid {
                        signal(pid, libc::SIGTERM);
                    }
                    match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
                        Ok(status) => status.ok(),
                        Err(_) => {
                            let _ = child.kill().await;
                            None
                        }
                    }
                }
            };
            drop(registration);
            // Drain the final events before reporting the outcome. A descendant
            // can keep stdout open after setup exits; dropping a timed-out
            // JoinHandle would detach its task and keep `events` open forever,
            // leaving Account::connect waiting instead of reading `finished`.
            if tokio::time::timeout(Duration::from_secs(1), &mut reader)
                .await
                .is_err()
            {
                reader.abort();
                // Wait for the sender to be dropped before publishing outcome.
                let _ = reader.await;
            }
            let stderr = stderr.finish().await;
            let outcome = match status {
                _ if cancelled => Err(UploaderError::cancelled()),
                Some(status) if status.success() => Ok(()),
                Some(status) if status.code().is_none() => Err(UploaderError::cancelled()),
                _ => Err(UploaderError::new(stderr_message(
                    &stderr,
                    "Agent Relay could not finish signing in. Please try again.",
                ))),
            };
            if let Err(error) = &outcome {
                if !error.cancelled {
                    log_helper_failure(&command_name, &error.message);
                }
            }
            let _ = done.send(outcome);
        });
        Ok(InstallRun {
            events: receiver,
            finished,
            cancel: Box::new(move || {
                let _ = stop.send(());
            }),
        })
    }
}
