//! The private temporary copy of an enrollment key read from stdin. It holds
//! an unspent key, so it lives no longer than it must: setup removes it once
//! the core has read the key, when setup ends, and when setup is interrupted
//! (SIGINT, SIGTERM or SIGHUP on Unix; Ctrl+C, Ctrl+Break or closing the
//! console on Windows). Nothing can remove it after a forced kill (SIGKILL,
//! power loss); the docs say to delete a leftover `agent-relay-enrollment-*`
//! file by hand.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use tempfile::NamedTempFile;

/// The copies not removed yet, where an interruption can find them.
static COPIES: Mutex<Vec<NamedTempFile>> = Mutex::new(Vec::new());

/// A copy kept for the core to read; dropping it removes the file if it is
/// still there.
#[derive(Debug)]
pub(in crate::onboarding) struct StdinCopy {
    path: PathBuf,
}

impl StdinCopy {
    /// Keeps `file` until it is removed, and has interruptions remove it.
    pub(super) fn keep(file: NamedTempFile) -> Self {
        watch();
        let path = file.path().to_path_buf();
        copies().push(file);
        Self { path }
    }

    pub(in crate::onboarding) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StdinCopy {
    fn drop(&mut self) {
        remove(&self.path);
    }
}

/// Removes the copy at `path`, if `path` is one. A key file the person named
/// is never touched.
pub(super) fn remove(path: &Path) {
    let taken = {
        let mut copies = copies();
        let found = copies.iter().position(|file| file.path() == path);
        found.map(|index| copies.swap_remove(index))
    };
    if let Some(file) = taken {
        let _ = file.close();
    }
}

/// Removes every copy: the process is being interrupted.
fn remove_all() {
    let taken = std::mem::take(&mut *copies());
    for file in taken {
        let _ = file.close();
    }
}

fn copies() -> MutexGuard<'static, Vec<NamedTempFile>> {
    COPIES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Has SIGINT, SIGTERM and SIGHUP remove the copies, then end the process as
/// the signal would have. A helper thread takes the signals through
/// signal-hook's iterator, so the removal runs outside a signal handler. A
/// signal this process ignores (a background job's SIGINT, SIGHUP under
/// nohup) stays ignored. Registers once; answers whether the signals are
/// watched.
#[cfg(unix)]
pub(super) fn watch() -> bool {
    static WATCHED: OnceLock<bool> = OnceLock::new();
    *WATCHED.get_or_init(|| {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        let watched: Vec<_> = [SIGINT, SIGTERM, SIGHUP]
            .into_iter()
            .filter(|signal| !ignored(*signal))
            .collect();
        let (registered, answer) = std::sync::mpsc::sync_channel(1);
        // The thread registers the signals itself, so they are never caught
        // without a thread to act on them.
        let spawned = std::thread::Builder::new()
            .name("enrollment-key-copy".into())
            .spawn(move || {
                let signals = signal_hook::iterator::Signals::new(watched);
                let _ = registered.send(signals.is_ok());
                let Ok(mut signals) = signals else { return };
                for signal in signals.forever() {
                    remove_all();
                    let _ = signal_hook::low_level::emulate_default_handler(signal);
                }
            });
        spawned.is_ok() && answer.recv().unwrap_or(false)
    })
}

/// Whether this process ignores `signal`.
#[cfg(unix)]
fn ignored(signal: libc::c_int) -> bool {
    // SAFETY: `sigaction` is plain data, valid zeroed; a null new action only
    // reads the current one into it.
    unsafe {
        let mut current: libc::sigaction = std::mem::zeroed();
        libc::sigaction(signal, std::ptr::null(), &mut current) == 0
            && current.sa_sigaction == libc::SIG_IGN
    }
}

/// Windows' stand-in: Ctrl+C, Ctrl+Break, closing the console, logoff and
/// shutdown remove the copies. Registers once; answers whether it is watched.
#[cfg(windows)]
pub(super) fn watch() -> bool {
    static WATCHED: OnceLock<bool> = OnceLock::new();
    *WATCHED.get_or_init(|| {
        let handler: unsafe extern "system" fn(u32) -> windows_sys::core::BOOL = console_event;
        // SAFETY: registers a handler that takes no pointers; Windows runs it
        // on a thread of its own, where it may lock and delete.
        unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(handler), 1) != 0 }
    })
}

/// Removes the copies and answers false, so the next handler, finally the
/// default one, ends the process as it would have.
#[cfg(windows)]
extern "system" fn console_event(_: u32) -> windows_sys::core::BOOL {
    remove_all();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary() -> NamedTempFile {
        tempfile::Builder::new()
            .prefix("agent-relay-enrollment-")
            .tempfile()
            .unwrap()
    }

    #[test]
    fn watching_registers_once_and_again_is_harmless() {
        let first = watch();
        assert!(first, "interruptions are watched");
        assert_eq!(watch(), first);
    }

    #[test]
    fn a_copy_is_removed_by_path_once_and_its_drop_is_then_harmless() {
        let copy = StdinCopy::keep(temporary());
        let path = copy.path().to_path_buf();
        assert!(path.exists());
        remove(&path);
        assert!(!path.exists());
        remove(&path);
        drop(copy);
        assert!(!path.exists());
    }

    #[test]
    fn dropping_a_copy_removes_it() {
        let copy = StdinCopy::keep(temporary());
        let path = copy.path().to_path_buf();
        drop(copy);
        assert!(!path.exists());
    }

    #[test]
    fn a_file_that_is_not_a_copy_is_left_alone() {
        let named = temporary();
        remove(named.path());
        assert!(named.path().exists());
    }

    /// The signal path, run in a child: this test binary again, running just the
    /// one test, which acts as the child when [`CHILD`] names its directory.
    #[cfg(unix)]
    mod signals {
        use super::super::*;
        use signal_hook::consts::{SIGHUP, SIGTERM};
        use std::os::unix::process::ExitStatusExt;
        use std::process::{Command, ExitStatus};
        use std::time::Duration;

        /// Set in the child to the directory it keeps its copy in.
        const CHILD: &str = "AGENT_RELAY_TEST_STDIN_COPY_CHILD";

        /// Runs `test` (a test in this module) as a child that keeps its copy in a
        /// fresh directory; answers how it ended and the copies left there.
        fn run_child(test: &str) -> (ExitStatus, Vec<PathBuf>, String) {
            let directory = tempfile::tempdir().unwrap();
            let module = module_path!().split_once("::").unwrap().1;
            let output = Command::new(std::env::current_exe().unwrap())
                .arg(format!("{module}::{test}"))
                .args(["--exact", "--nocapture", "--test-threads=1"])
                .env(CHILD, directory.path())
                .output()
                .unwrap();
            let left = std::fs::read_dir(directory.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            let report = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            (output.status, left, report)
        }

        /// In the child: keeps a copy in `directory`. `keep` returns once the
        /// watcher thread has registered the signals.
        fn keep_in(directory: &Path) -> StdinCopy {
            let file = tempfile::Builder::new()
                .prefix("agent-relay-enrollment-")
                .tempfile_in(directory)
                .unwrap();
            let copy = StdinCopy::keep(file);
            assert!(copy.path().exists());
            copy
        }

        #[test]
        fn sigterm_removes_the_copy_and_ends_the_process_by_sigterm() {
            if let Some(directory) = std::env::var_os(CHILD) {
                let _copy = keep_in(Path::new(&directory));
                signal_hook::low_level::raise(SIGTERM).unwrap();
                // The watcher ends the process; this only stops a broken one.
                std::thread::sleep(Duration::from_secs(30));
                std::process::exit(3);
            }
            let (status, left, report) =
                run_child("sigterm_removes_the_copy_and_ends_the_process_by_sigterm");
            assert_eq!(status.signal(), Some(SIGTERM), "{status:?}\n{report}");
            assert!(left.is_empty(), "copies left: {left:?}");
        }

        #[test]
        fn an_ignored_sighup_stays_ignored_and_leaves_the_copy() {
            if let Some(directory) = std::env::var_os(CHILD) {
                // SAFETY: sets SIGHUP's disposition to ignored before any other
                // thread of this child looks at it.
                unsafe { libc::signal(SIGHUP, libc::SIG_IGN) };
                let copy = keep_in(Path::new(&directory));
                signal_hook::low_level::raise(SIGHUP).unwrap();
                std::thread::sleep(Duration::from_millis(200));
                assert!(copy.path().exists());
                // Exits without dropping the copy, so the parent can find it.
                std::process::exit(0);
            }
            let (status, left, report) =
                run_child("an_ignored_sighup_stays_ignored_and_leaves_the_copy");
            assert_eq!(status.code(), Some(0), "{status:?}\n{report}");
            assert_eq!(left.len(), 1, "the copy survives: {left:?}");
        }
    }
}
