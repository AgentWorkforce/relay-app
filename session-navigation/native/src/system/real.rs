//! The real machine: `std::process::Command` (never a shell) and `std::fs`.
//! Portable std only; the few Unix-only calls (mode bits) live in [`os`]
//! with a Windows counterpart.
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{FsError, Metadata, Output, Run, RunError, System};

/// The only PATH a child sees. Programs are always given by absolute path;
/// this is for what they spawn themselves.
#[cfg(unix)]
pub const MINIMAL_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
/// The default PATH a child sees; `os::base_env` replaces it with the same
/// directories under the actual `%SystemRoot%` (UNVERIFIED on a Windows
/// machine).
#[cfg(windows)]
pub const MINIMAL_PATH: &str = r"C:\Windows\System32;C:\Windows";

/// Platform specifics of file permissions and child processes.
#[cfg(unix)]
mod os {
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    use std::process::Command;

    /// The working directory every child starts in.
    pub fn child_dir() -> String {
        "/".to_owned()
    }

    /// Private to the person: 0700.
    pub fn private_dir(builder: &mut fs::DirBuilder) {
        builder.mode(0o700);
    }

    /// Private to the person: 0600.
    pub fn private_file(options: &mut fs::OpenOptions) {
        options.mode(0o600);
    }

    /// Any execute bit is set.
    pub fn executable(meta: &fs::Metadata) -> bool {
        meta.permissions().mode() & 0o111 != 0
    }

    /// Nothing beyond `PATH` is needed.
    pub fn base_env(_: &mut Command) {}

    /// Canonical paths are already plain.
    pub fn plain_path(path: String) -> String {
        path
    }
}

/// Windows has no mode bits: a new directory or file inherits the default
/// ACL of its parent, which under the person's profile (`%LOCALAPPDATA%`)
/// already grants only them, SYSTEM and administrators. Setting an explicit
/// ACL needs the Win32 security API (unsafe FFI), which this crate forbids.
///
/// Reviewed but never compiled: no Windows target is installed where this
/// crate is developed.
#[cfg(windows)]
mod os {
    use std::fs;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    /// `CREATE_NO_WINDOW`: a console helper (PowerShell, `wsl.exe`) started
    /// from a GUI process would otherwise flash a console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Machine basics a cleared environment must still carry: without
    /// `SystemRoot` programs fail to start, without `TEMP`/`TMP`
    /// PowerShell's `Add-Type` has nowhere to compile, and without the
    /// profile variables neither `wsl.exe` nor `Get-AppxPackage` finds the
    /// person. None of them is a secret.
    const BASE: &[&str] = &[
        "SystemRoot",
        "windir",
        "SystemDrive",
        "ProgramData",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "CommonProgramFiles",
        "ComSpec",
        "PATHEXT",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "USERNAME",
        "USERDOMAIN",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "PROCESSOR_ARCHITECTURE",
        "NUMBER_OF_PROCESSORS",
    ];

    /// `%SystemRoot%` as this process sees it, else the default.
    fn system_root() -> String {
        std::env::var("SystemRoot")
            .ok()
            .filter(|r| session_navigation::parse::is_windows_absolute_path(r))
            .map(|r| r.trim_end_matches('\\').to_owned())
            .unwrap_or_else(|| r"C:\Windows".to_owned())
    }

    /// The working directory every child starts in: the system drive's
    /// root (`C:\` is wrong on a machine installed on another drive).
    pub fn child_dir() -> String {
        let root = system_root();
        format!("{}\\", root.get(..2).unwrap_or("C:"))
    }

    pub fn private_dir(_: &mut fs::DirBuilder) {}

    pub fn private_file(_: &mut fs::OpenOptions) {}

    /// No execute bit: every regular file counts. Which name is a program
    /// (`.exe`) is the caller's choice.
    pub fn executable(meta: &fs::Metadata) -> bool {
        meta.is_file()
    }

    /// The base variables, a `PATH` of the system directories only, and no
    /// console window.
    pub fn base_env(command: &mut Command) {
        for key in BASE {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let root = system_root();
        command.env("SystemRoot", &root);
        command.env(
            "PATH",
            format!(r"{root}\System32;{root};{root}\System32\WindowsPowerShell\v1.0"),
        );
        command.creation_flags(CREATE_NO_WINDOW);
    }

    /// `fs::canonicalize` returns a verbatim path (`\\?\C:\…`,
    /// `\\?\UNC\server\…`); every other path in this crate is the plain
    /// form, so the prefix is removed again.
    pub fn plain_path(path: String) -> String {
        if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else if let Some(rest) = path.strip_prefix(r"\\?\") {
            rest.to_owned()
        } else {
            path
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RealSystem;

fn fs_error(error: &std::io::Error) -> FsError {
    match error.kind() {
        ErrorKind::NotFound => FsError::NotFound,
        ErrorKind::PermissionDenied => FsError::PermissionDenied,
        _ => FsError::Other,
    }
}

/// Read at most `cap + 1` bytes so an oversized stream is detected, then
/// drain the rest so the child never blocks on a full pipe.
fn bounded_reader<R: Read + Send + 'static>(
    mut source: R,
    cap: usize,
    over: Arc<AtomicBool>,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            match source.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = cap.saturating_sub(kept.len());
                    let chunk = buffer.get(..n).unwrap_or_default();
                    if n > room {
                        kept.extend_from_slice(chunk.get(..room).unwrap_or_default());
                        over.store(true, Ordering::SeqCst);
                    } else {
                        kept.extend_from_slice(chunk);
                    }
                }
            }
        }
        kept
    })
}

impl System for RealSystem {
    fn run(&self, run: &Run) -> Result<Output, RunError> {
        if !Path::new(&run.program).is_absolute() {
            return Err(RunError::NotFound);
        }
        let mut command = Command::new(&run.program);
        command.env_clear().env("PATH", MINIMAL_PATH);
        os::base_env(&mut command);
        command
            .args(&run.args)
            .envs(run.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .current_dir(os::child_dir())
            .stdin(if run.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| match e.kind() {
            ErrorKind::NotFound => RunError::NotFound,
            _ => RunError::Io,
        })?;
        let over = Arc::new(AtomicBool::new(false));
        let stdout = child
            .stdout
            .take()
            .map(|s| bounded_reader(s, run.max_output, over.clone()));
        let stderr = child
            .stderr
            .take()
            .map(|s| bounded_reader(s, run.max_output, over.clone()));
        let writer = match (child.stdin.take(), &run.stdin) {
            (Some(mut pipe), Some(secret)) => {
                let bytes = secret.expose_secret().to_vec();
                Some(thread::spawn(move || {
                    let _ = pipe.write_all(&bytes);
                }))
            }
            _ => None,
        };
        // Watchdog: poll the child until it exits, the deadline passes or an
        // output stream goes over its cap; the last two kill it.
        let deadline = Instant::now() + run.timeout;
        let mut failure = None;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => {
                    failure = Some(RunError::Io);
                    break None;
                }
            }
            if over.load(Ordering::SeqCst) {
                failure = Some(RunError::TooLarge);
                break None;
            }
            if Instant::now() >= deadline {
                failure = Some(RunError::TimedOut);
                break None;
            }
            thread::sleep(Duration::from_millis(5));
        };
        if status.is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        let stdout = stdout.and_then(|h| h.join().ok()).unwrap_or_default();
        let stderr = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
        if let Some(error) = failure {
            return Err(error);
        }
        if over.load(Ordering::SeqCst) {
            return Err(RunError::TooLarge);
        }
        Ok(Output {
            status: status.and_then(|s| s.code()),
            stdout,
            stderr,
        })
    }

    fn read_file(&self, path: &str, cap: usize) -> Result<Vec<u8>, FsError> {
        let file = fs::File::open(path).map_err(|e| fs_error(&e))?;
        let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
        let mut bytes = Vec::new();
        file.take(limit)
            .read_to_end(&mut bytes)
            .map_err(|e| fs_error(&e))?;
        if bytes.len() > cap {
            return Err(FsError::TooLarge);
        }
        Ok(bytes)
    }

    fn read_prefix(&self, path: &str, len: usize) -> Result<Vec<u8>, FsError> {
        let file = fs::File::open(path).map_err(|e| fs_error(&e))?;
        let mut bytes = Vec::new();
        file.take(u64::try_from(len).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .map_err(|e| fs_error(&e))?;
        Ok(bytes)
    }

    fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError> {
        let entries = fs::read_dir(path).map_err(|e| fs_error(&e))?;
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| fs_error(&e))?;
            if let Ok(name) = entry.file_name().into_string() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn metadata(&self, path: &str) -> Result<Metadata, FsError> {
        let meta = fs::metadata(path).map_err(|e| fs_error(&e))?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .ok_or(FsError::Other)?;
        Ok(Metadata {
            is_dir: meta.is_dir(),
            is_file: meta.is_file(),
            len: meta.len(),
            mtime,
            executable: os::executable(&meta),
        })
    }

    fn create_private_dir(&self, path: &str) -> Result<(), FsError> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => return Ok(()),
            Ok(_) => return Err(FsError::Other),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(fs_error(&e)),
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        os::private_dir(&mut builder);
        builder.create(path).map_err(|e| fs_error(&e))
    }

    fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        let target = Path::new(path);
        let dir = target.parent().ok_or(FsError::Other)?;
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(FsError::Other)?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let temp = dir.join(format!(".{name}.tmp-{}-{nanos}", std::process::id()));
        let result = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            os::private_file(&mut options);
            let mut file = options.open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temp, path)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(fs_error(&error));
        }
        Ok(())
    }

    fn canonicalize(&self, path: &str) -> Result<String, FsError> {
        let canonical = fs::canonicalize(path).map_err(|e| fs_error(&e))?;
        canonical
            .into_os_string()
            .into_string()
            .map(os::plain_path)
            .map_err(|_| FsError::Other)
    }

    fn remove_file(&self, path: &str) -> Result<(), FsError> {
        fs::remove_file(path).map_err(|e| fs_error(&e))
    }

    fn win32(&self) -> Option<&dyn super::Win32> {
        super::win32_real::real()
    }

    fn kernel(&self) -> Option<&dyn super::Kernel> {
        super::kernel_macos::kernel()
    }

    fn concurrent(&self) -> bool {
        true
    }

    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn read_link(&self, path: &str) -> Result<String, FsError> {
        let target = fs::read_link(path).map_err(|e| fs_error(&e))?;
        target
            .into_os_string()
            .into_string()
            .map_err(|_| FsError::Other)
    }

    fn spawn_detached(&self, run: &Run) -> Result<(), RunError> {
        if !Path::new(&run.program).is_absolute() {
            return Err(RunError::NotFound);
        }
        let mut command = Command::new(&run.program);
        command.env_clear().env("PATH", MINIMAL_PATH);
        os::base_env(&mut command);
        command
            .args(&run.args)
            .envs(run.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .current_dir(os::child_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|e| match e.kind() {
            ErrorKind::NotFound => RunError::NotFound,
            _ => RunError::Io,
        })?;
        // Reap it whenever it exits, so it never lingers as a zombie.
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}
