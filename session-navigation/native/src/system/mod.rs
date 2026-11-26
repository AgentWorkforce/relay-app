//! The only impure seam. Every byte the adapters read from the machine and
//! every program they run goes through [`System`]; [`RealSystem`] does it for
//! real and [`FakeSystem`] replays a scripted machine and records each call.
mod fake;
mod kernel;
#[allow(unsafe_code)]
mod kernel_macos;
pub mod kernel_parse;
mod real;
pub mod win32;
mod win32_fake;
#[allow(unsafe_code)]
mod win32_real;

use std::fmt;
use std::time::Duration;

pub use fake::{Call, FakeSystem};
pub use kernel::{Kernel, KernelCall, RawProcess, ScriptedKernel};
pub use real::RealSystem;
pub use win32::Win32;
pub use win32_fake::{ScriptedWin32, Win32Call, WinWorld};

/// A value that must never be logged: its `Debug` prints `<redacted>`, it has
/// no `Display` and no `Serialize`, and reading it needs an explicit call.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Vec<u8>);

impl Secret {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// The raw bytes, for the one place that hands them to a program's stdin.
    pub fn expose_secret(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// One program run: an absolute path and an argv vector, never a shell line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub program: String,
    pub args: Vec<String>,
    /// Extra environment. The child gets only these plus a fixed minimal PATH.
    pub env: Vec<(String, String)>,
    /// Bytes for stdin. Always secret-typed: the only stdin the adapters
    /// send is curl's config, which carries the OpenCode password.
    pub stdin: Option<Secret>,
    pub timeout: Duration,
    /// Combined cap for stdout and for stderr, each.
    pub max_output: usize,
}

impl Run {
    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            env: Vec::new(),
            stdin: None,
            timeout: Duration::from_secs(5),
            max_output: 1024 * 1024,
        }
    }

    pub fn with_args(program: &str, args: Vec<String>) -> Self {
        Self {
            args,
            ..Self::new(program, &[])
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn max_output(mut self, max: usize) -> Self {
        self.max_output = max;
        self
    }

    pub fn stdin(mut self, stdin: Secret) -> Self {
        self.stdin = Some(stdin);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The exit code; `None` when the child was ended by a signal.
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn ok(stdout: impl Into<Vec<u8>>) -> Self {
        Self::exit(0, stdout, Vec::new())
    }

    pub fn exit(status: i32, stdout: impl Into<Vec<u8>>, stderr: impl Into<Vec<u8>>) -> Self {
        Self {
            status: Some(status),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunError {
    /// The program does not exist (or the path was not absolute).
    NotFound,
    TimedOut,
    /// stdout or stderr went over `max_output`; the child was killed.
    TooLarge,
    Io,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    PermissionDenied,
    TooLarge,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub is_dir: bool,
    pub is_file: bool,
    pub len: u64,
    /// Modification time in whole seconds since the epoch.
    pub mtime: u64,
    /// Any execute bit is set.
    pub executable: bool,
}

/// Small and dumb on purpose: no policy lives behind it.
pub trait System {
    fn run(&self, run: &Run) -> Result<Output, RunError>;
    /// The whole file, or `TooLarge` when it has more than `cap` bytes.
    fn read_file(&self, path: &str, cap: usize) -> Result<Vec<u8>, FsError>;
    /// At most the first `len` bytes.
    fn read_prefix(&self, path: &str, len: usize) -> Result<Vec<u8>, FsError>;
    /// Entry names (UTF-8 only), sorted.
    fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError>;
    /// Follows symlinks.
    fn metadata(&self, path: &str) -> Result<Metadata, FsError>;
    /// The absolute path with every symlink resolved (UTF-8 only).
    fn canonicalize(&self, path: &str) -> Result<String, FsError>;
    /// Create `path` and parents with mode 0700; an existing directory is
    /// fine, anything else there is an error.
    fn create_private_dir(&self, path: &str) -> Result<(), FsError>;
    /// Write a temporary file next to `path` (mode 0600) and rename it over.
    fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<(), FsError>;
    fn remove_file(&self, path: &str) -> Result<(), FsError>;
    /// Seconds since the epoch.
    fn now(&self) -> u64;
    /// The target a symbolic link names, not resolved further (UTF-8 only):
    /// Linux `/proc/<pid>/exe` reads `/usr/bin/x (deleted)` for a deleted
    /// executable, which no canonicalize can return. Defaults to
    /// [`System::canonicalize`] for systems without one.
    fn read_link(&self, path: &str) -> Result<String, FsError> {
        self.canonicalize(path)
    }
    /// Start a program and do not wait for it: a terminal window that lives
    /// on after the call (stdin, stdout and stderr are null). Same rules as
    /// [`System::run`]: an absolute program, an argv vector, only `env` plus
    /// the minimal PATH. Defaults to `NotFound` for systems without one.
    fn spawn_detached(&self, _run: &Run) -> Result<(), RunError> {
        Err(RunError::NotFound)
    }
    /// Direct Win32 calls, when this system has them (`RealSystem` on
    /// Windows; a `FakeSystem` given a [`ScriptedWin32`]). `None` keeps the
    /// Windows adapter on its PowerShell path.
    fn win32(&self) -> Option<&dyn Win32> {
        None
    }
    /// Direct kernel reads of the process table, when this system has them
    /// (`RealSystem` on macOS; a `FakeSystem` given a [`ScriptedKernel`]).
    /// `None` keeps the macOS adapter on its `ps`/`sysctl`/`lsof` path.
    fn kernel(&self) -> Option<&dyn Kernel> {
        None
    }
    /// Whether independent reads may run on several threads at once.
    /// `RealSystem` says yes; `FakeSystem` says no unless asked, so its
    /// calls are recorded in one stable order.
    fn concurrent(&self) -> bool {
        false
    }
}

impl<T: System + ?Sized> System for &T {
    fn run(&self, run: &Run) -> Result<Output, RunError> {
        (**self).run(run)
    }
    fn read_file(&self, path: &str, cap: usize) -> Result<Vec<u8>, FsError> {
        (**self).read_file(path, cap)
    }
    fn read_prefix(&self, path: &str, len: usize) -> Result<Vec<u8>, FsError> {
        (**self).read_prefix(path, len)
    }
    fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError> {
        (**self).list_dir(path)
    }
    fn metadata(&self, path: &str) -> Result<Metadata, FsError> {
        (**self).metadata(path)
    }
    fn canonicalize(&self, path: &str) -> Result<String, FsError> {
        (**self).canonicalize(path)
    }
    fn create_private_dir(&self, path: &str) -> Result<(), FsError> {
        (**self).create_private_dir(path)
    }
    fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        (**self).write_atomic(path, bytes)
    }
    fn remove_file(&self, path: &str) -> Result<(), FsError> {
        (**self).remove_file(path)
    }
    fn now(&self) -> u64 {
        (**self).now()
    }
    fn read_link(&self, path: &str) -> Result<String, FsError> {
        (**self).read_link(path)
    }
    fn spawn_detached(&self, run: &Run) -> Result<(), RunError> {
        (**self).spawn_detached(run)
    }
    fn win32(&self) -> Option<&dyn Win32> {
        (**self).win32()
    }
    fn kernel(&self) -> Option<&dyn Kernel> {
        (**self).kernel()
    }
    fn concurrent(&self) -> bool {
        (**self).concurrent()
    }
}

impl<T: System + ?Sized> System for std::sync::Arc<T> {
    fn run(&self, run: &Run) -> Result<Output, RunError> {
        (**self).run(run)
    }
    fn read_file(&self, path: &str, cap: usize) -> Result<Vec<u8>, FsError> {
        (**self).read_file(path, cap)
    }
    fn read_prefix(&self, path: &str, len: usize) -> Result<Vec<u8>, FsError> {
        (**self).read_prefix(path, len)
    }
    fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError> {
        (**self).list_dir(path)
    }
    fn metadata(&self, path: &str) -> Result<Metadata, FsError> {
        (**self).metadata(path)
    }
    fn canonicalize(&self, path: &str) -> Result<String, FsError> {
        (**self).canonicalize(path)
    }
    fn create_private_dir(&self, path: &str) -> Result<(), FsError> {
        (**self).create_private_dir(path)
    }
    fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        (**self).write_atomic(path, bytes)
    }
    fn remove_file(&self, path: &str) -> Result<(), FsError> {
        (**self).remove_file(path)
    }
    fn now(&self) -> u64 {
        (**self).now()
    }
    fn read_link(&self, path: &str) -> Result<String, FsError> {
        (**self).read_link(path)
    }
    fn spawn_detached(&self, run: &Run) -> Result<(), RunError> {
        (**self).spawn_detached(run)
    }
    fn win32(&self) -> Option<&dyn Win32> {
        (**self).win32()
    }
    fn kernel(&self) -> Option<&dyn Kernel> {
        (**self).kernel()
    }
    fn concurrent(&self) -> bool {
        (**self).concurrent()
    }
}

/// Join a directory and a single path component.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}
