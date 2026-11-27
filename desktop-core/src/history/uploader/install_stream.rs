//! Stream an install's events without letting inherited stdout hold its outcome.
use super::*;
use tokio::io::{AsyncBufReadExt, BufReader};

impl ProcessRunner {
    pub(super) fn install_stream(
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
        let stdout = child.stdout.take();
        let stderr = Capture::start(child.stderr.take(), 64 * 1024);
        let (events, receiver) = mpsc::unbounded_channel();
        let (done, finished) = tokio::sync::oneshot::channel();
        let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            // Poll the reader with the child instead of spawning another task:
            // a starved reader must not lose buffered final events when the
            // post-exit deadline expires before that task is scheduled.
            let mut reader = Box::pin(read_events(stdout, events));
            let mut reader_done = false;
            let mut cancelled = false;
            let status = loop {
                tokio::select! {
                    biased;
                    status = child.wait() => break status.ok(),
                    _ = &mut stopped => {
                        cancelled = true;
                        if let Some(pid) = child.id() {
                            signal(pid, libc::SIGTERM);
                        }
                        break match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
                            Ok(status) => status.ok(),
                            Err(_) => {
                                let _ = child.kill().await;
                                None
                            }
                        };
                    }
                    _ = &mut reader, if !reader_done => reader_done = true,
                }
            };
            drop(registration);
            // Drain buffered events directly. A descendant may keep writing
            // stdout forever, so completion still has a bounded drain window.
            if !reader_done {
                let _ = tokio::time::timeout(Duration::from_secs(1), &mut reader).await;
            }
            // Drop the sender before reporting the outcome: account setup
            // drains this channel before awaiting `finished`.
            drop(reader);
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

async fn read_events(
    stdout: Option<tokio::process::ChildStdout>,
    events: mpsc::UnboundedSender<Value>,
) {
    let Some(stdout) = stdout else { return };
    let mut lines = BufReader::new(stdout).lines();
    let mut read = 0;
    while let Ok(Some(line)) = lines.next_line().await {
        if let Ok(event @ Value::Object(_)) = serde_json::from_str::<Value>(&line) {
            let _ = events.send(event);
        }
        // Even continuously ready output must yield to child exit,
        // cancellation and the bounded drain's timer.
        read += 1;
        if read == 64 {
            read = 0;
            tokio::task::yield_now().await;
        }
    }
}
