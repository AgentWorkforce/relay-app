//! The macOS process table, boot and cwds from [`Kernel`] reads: the same
//! [`ProcessInfo`] rows the `ps` path builds ([`super::parse::ps`]), without
//! running `ps`, `sysctl` or `lsof`.
//!
//! - `exe` is classified exactly as from `ps`: the kernel's `p_comm`
//!   (`ucomm`), confirmed for app bundles by `argv[0]` (the `ps` `comm`
//!   column), or by the kernel's executable path when argv is unreadable.
//! - `argv` is the real vector (`ps` joins it with spaces, so its split is
//!   lossy for arguments containing a space, and escapes control bytes as
//!   `\\012`). An unreadable one (another user's process) is the
//!   executable's path alone, as `ps` prints it; empty when that is
//!   unreadable too (a zombie).
//! - `tty` is `/dev/<name>`, `start` whole seconds, uids as the kernel has
//!   them (`nobody` = 4294967294).
use std::collections::BTreeSet;

use session_navigation::evidence::{Fact, ProcessInfo};
use session_navigation::model::{BootId, ProcessIdentity};

use super::parse::ps;
use crate::system::{Kernel, RawProcess};

/// The table, plus the executables running as each uid (`(ruid, path)`),
/// from which the running apps are told.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KernelTable {
    pub processes: Vec<ProcessInfo>,
    pub executables: BTreeSet<(u32, String)>,
    /// Read from the kernel (else from `ps`, and cwds come from `lsof`).
    pub from_kernel: bool,
}

/// One row as the `ps` path would build it.
pub fn process(raw: &RawProcess, boot: BootId) -> ProcessInfo {
    // Another user's argument area is closed to us; `ps` then shows the
    // executable's path as the whole command, and so does this.
    let argv = match (&raw.argv, &raw.path) {
        (Some(argv), _) => argv.clone(),
        (None, Some(path)) => vec![path.clone()],
        (None, None) => Vec::new(),
    };
    let named = argv
        .first()
        .map(String::as_str)
        .or(raw.path.as_deref())
        .unwrap_or("");
    ProcessInfo {
        identity: ProcessIdentity {
            pid: raw.pid,
            start: raw.start,
            boot,
        },
        ppid: raw.ppid,
        euid: raw.euid,
        ruid: raw.ruid,
        exe: ps::classify(&raw.comm, named),
        tty: raw.tty.clone(),
        argv,
        cwd: Fact::Unknown,
        // X11 `WINDOWID` does not exist on macOS.
        window: None,
    }
}

/// The whole table, `None` when the kernel could not list it (the caller
/// then runs `ps`).
pub fn table(kernel: &dyn Kernel, boot: BootId) -> Option<KernelTable> {
    let raw = kernel.processes()?;
    let mut seen = BTreeSet::new();
    let mut out = KernelTable {
        from_kernel: true,
        ..KernelTable::default()
    };
    for row in &raw {
        if !seen.insert(row.pid) {
            return None;
        }
        if let Some(path) = &row.path {
            out.executables.insert((row.ruid, path.clone()));
        }
        out.processes.push(process(row, boot));
    }
    (!out.processes.is_empty()).then_some(out)
}

/// Fill `cwd` for agent CLIs, one kernel read each (what `lsof -d cwd`
/// printed). An unreadable one stays `Unknown`.
pub fn fill_cwds(kernel: &dyn Kernel, table: &mut [ProcessInfo]) {
    for process in table.iter_mut().filter(|p| ps::is_agent_cli(p.exe)) {
        if let Some(cwd) = kernel.cwd(process.identity.pid) {
            process.cwd = Fact::Known(cwd);
        }
    }
}
