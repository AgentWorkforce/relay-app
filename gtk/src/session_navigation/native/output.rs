//! Deadline-bound helper output collection. No detached reader owns the pipe:
//! returning drops our read handle even if a descendant keeps stdout open.
use std::io::{self, Read};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
const LIMIT: usize = 4_194_304;

fn terminate_helper(child: &mut Child) {
    // Terminate only the helper we spawned, never its descendants or host app.
    let _ = child.kill();
    let _ = child.wait();
}

pub(super) fn run(mut command: Command, timeout: Duration) -> Result<String, String> {
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let result = collect(&mut child, Instant::now() + timeout);
    if result.is_err() {
        terminate_helper(&mut child);
    }
    result
}

fn collect(child: &mut Child, deadline: Instant) -> Result<String, String> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or("The host output is unavailable.")?;
    prepare(&stdout).map_err(|e| e.to_string())?;
    let mut output = Vec::new();
    let mut closed = false;
    let mut status = None;
    loop {
        if status.is_none() {
            status = child.try_wait().map_err(|e| e.to_string())?;
        }
        check_status(status)?;
        if closed && status.is_some() {
            return String::from_utf8(output)
                .map_err(|_| "The terminal returned invalid output.".into());
        }
        if Instant::now() >= deadline {
            return Err(timeout_message(status).into());
        }
        let progress = if closed {
            Progress::Waiting
        } else {
            read_available(&mut stdout, &mut output).map_err(|e| e.to_string())?
        };
        if output.len() > LIMIT {
            return Err("The terminal returned an oversized response.".into());
        }
        match progress {
            Progress::Closed => closed = true,
            Progress::Waiting => std::thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            ),
            Progress::Data => (),
        }
    }
}

fn check_status(status: Option<ExitStatus>) -> Result<(), String> {
    if status.is_some_and(|s| !s.success()) {
        Err("The host rejected the request. Its focus or remote-control permission may be unavailable.".into())
    } else {
        Ok(())
    }
}

fn timeout_message(status: Option<ExitStatus>) -> &'static str {
    if status.is_some() {
        "The host did not close its output. The opening request is unverified; no replacement was started."
    } else {
        "The host did not acknowledge the request. Selection is unverified; no replacement session was started."
    }
}

enum Progress {
    Data,
    Waiting,
    Closed,
}
fn prepare(stdout: &ChildStdout) -> io::Result<()> {
    #[cfg(unix)]
    return unix::nonblocking(stdout);
    #[cfg(windows)]
    {
        let _ = stdout;
        Ok(())
    }
}

fn read_available(stdout: &mut ChildStdout, output: &mut Vec<u8>) -> io::Result<Progress> {
    let mut buffer = [0; 8192];
    #[cfg(windows)]
    let available = match windows::available(stdout)? {
        None => return Ok(Progress::Closed),
        Some(0) => return Ok(Progress::Waiting),
        Some(count) => count.min(buffer.len()),
    };
    #[cfg(unix)]
    let available = buffer.len();
    match stdout.read(&mut buffer[..available]) {
        Ok(0) => Ok(Progress::Closed),
        Ok(count) => {
            output.extend_from_slice(&buffer[..count]);
            Ok(Progress::Data)
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(Progress::Waiting)
        }
        Err(error) => Err(error),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn deadline_closes_reader_without_waiting_for_or_killing_a_descendant() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("descendant-completed");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "trap '' PIPE; (sleep 0.4; if /bin/echo still-open 2>/dev/null; then printf inherited-open > \"$1.tmp\" && mv \"$1.tmp\" \"$1\"; else printf reader-closed > \"$1.tmp\" && mv \"$1.tmp\" \"$1\"; fi) & printf helper",
            "probe",
        ]);
        command.arg(&marker);
        let start = Instant::now();
        let error = run(command, Duration::from_millis(80)).unwrap_err();
        assert!(error.contains("did not close its output"));
        assert!(start.elapsed() < Duration::from_millis(350));
        // The descendant writes its marker after our read end was dropped.
        let wait = Instant::now() + Duration::from_secs(3);
        while !marker.exists() && Instant::now() < wait {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "reader-closed");
    }

    #[test]
    fn collects_output_larger_than_a_pipe_buffer_without_blocking_the_child() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "head -c 131072 /dev/zero | tr '\\0' x"]);
        assert_eq!(
            run(command, Duration::from_secs(2)).unwrap(),
            "x".repeat(131072)
        );
    }
}
