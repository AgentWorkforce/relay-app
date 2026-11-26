//! A narrow, read-only view of the kernel's process table: what `ps`,
//! `sysctl kern.boottime` and `lsof -d cwd` print, read directly. The real
//! implementation is FFI ([`super::kernel_macos`], the only `unsafe` in this
//! crate); [`ScriptedKernel`] replays a machine for tests. Adapters reach it
//! through [`super::System::kernel`]; without one they run the tools.
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

/// One process as the kernel reports it. Only what the adapters use is
/// kept; the environment block is never read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawProcess {
    pub pid: u32,
    pub ppid: u32,
    pub euid: u32,
    pub ruid: u32,
    /// Start time in whole seconds since the epoch.
    pub start: u64,
    /// The controlling terminal (`/dev/ttys004`), `None` for none.
    pub tty: Option<String>,
    /// The kernel's name for the executable (`p_comm`, at most 16 bytes).
    pub comm: String,
    /// The executable's path, `None` when it could not be read.
    pub path: Option<String>,
    /// The argument vector, `None` when it could not be read (another
    /// person's process, a zombie).
    pub argv: Option<Vec<String>>,
}

/// Direct kernel reads. Every method is read-only and answers `None` when
/// the read failed as a whole; the caller then falls back to the tools.
pub trait Kernel: Sync {
    /// The boot time in seconds since the epoch (`kern.boottime`).
    fn boot_time(&self) -> Option<u64>;
    /// Every process. One that exits mid-scan is left out; `None` when the
    /// pid list itself could not be read.
    fn processes(&self) -> Option<Vec<RawProcess>>;
    /// The current directory of `pid`, `None` when unreadable or gone.
    fn cwd(&self, pid: u32) -> Option<String>;
}

/// What a [`ScriptedKernel`] was asked, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelCall {
    BootTime,
    Processes,
    Cwd(u32),
}

/// A scripted kernel for tests: fixed answers, every call recorded.
#[derive(Debug, Default)]
pub struct ScriptedKernel {
    pub boot: Option<u64>,
    pub table: Option<Vec<RawProcess>>,
    pub cwds: BTreeMap<u32, String>,
    calls: Mutex<Vec<KernelCall>>,
}

impl ScriptedKernel {
    pub fn new(boot: Option<u64>, table: Option<Vec<RawProcess>>) -> Self {
        Self {
            boot,
            table,
            ..Self::default()
        }
    }

    pub fn with_cwd(mut self, pid: u32, cwd: &str) -> Self {
        self.cwds.insert(pid, cwd.to_owned());
        self
    }

    pub fn calls(&self) -> Vec<KernelCall> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn note(&self, call: KernelCall) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }
}

impl Kernel for ScriptedKernel {
    fn boot_time(&self) -> Option<u64> {
        self.note(KernelCall::BootTime);
        self.boot
    }

    fn processes(&self) -> Option<Vec<RawProcess>> {
        self.note(KernelCall::Processes);
        self.table.clone()
    }

    fn cwd(&self, pid: u32) -> Option<String> {
        self.note(KernelCall::Cwd(pid));
        self.cwds.get(&pid).cloned()
    }
}
