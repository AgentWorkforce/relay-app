//! The boot and the kernel process table, read from `/proc` through
//! `System`, shared by the observer and the executor.
//!
//! A process that vanishes while it is read (`NotFound`) exited and is left
//! out; one whose `stat` or `status` cannot be read for any other reason
//! makes the whole table `Unknown`, because an ancestor walk over a table
//! with holes could call a hosted process headless. Another user's `exe`,
//! `cwd` and `environ` are unreadable by design (ptrace access mode): that
//! process is still listed, classified from its `comm` only as far as
//! [`super::classify::classify_comm`] allows.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use session_navigation::evidence::{Fact, ProcessInfo};
use session_navigation::hosts::WindowId;
use session_navigation::model::{BootId, ProcessIdentity};

use super::classify::{classify_comm, classify_exe, is_agent_cli};
use super::config::LinuxConfig;
use super::procfs::{self, Identity, Stat};
use crate::system::{FsError, System};

pub const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";
pub const PROC_STAT: &str = "/proc/stat";
pub const PROC: &str = "/proc";

const STAT_CAP: usize = 4096;
const STATUS_CAP: usize = 64 * 1024;
const CMDLINE_CAP: usize = 256 * 1024;
const ENVIRON_CAP: usize = 1024 * 1024;
const MAX_PROCESSES: usize = 32_768;

/// This boot: its id and its boot time (epoch seconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boot {
    pub id: BootId,
    pub btime: u64,
}

/// The first `btime` read in each boot, shared by the observer and the
/// executor. `btime` is derived from the wall clock and can move by a
/// second when the clock is stepped (UNVERIFIED how often on current
/// kernels: compare `grep btime /proc/stat` across an NTP step); start
/// times are computed from it, so a moving `btime` would make every
/// identity look recycled. One value per boot id keeps them stable.
#[derive(Clone, Debug, Default)]
pub struct BootClock(Arc<Mutex<Option<Boot>>>);

impl BootClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// The boot, reading `btime` only the first time a boot id is seen.
    pub fn read<S: System>(&self, sys: &S) -> Option<Boot> {
        let id = procfs::parse_boot_id(&sys.read_file(BOOT_ID, 256).ok()?)?;
        let mut cached = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(boot) = cached.filter(|b| b.id == id) {
            return Some(boot);
        }
        let btime = procfs::parse_btime(&sys.read_file(PROC_STAT, 1024 * 1024).ok()?)?;
        let boot = Boot { id, btime };
        *cached = Some(boot);
        Some(boot)
    }
}

/// The table plus the identity keys of each process's environment (only
/// processes of the person's own uid have one).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub processes: Vec<ProcessInfo>,
    pub identities: BTreeMap<u32, Identity>,
}

enum Row {
    Gone,
    Unreadable,
    Read(Box<ProcessInfo>, Identity),
}

fn proc_path(pid: u32, file: &str) -> String {
    format!("{PROC}/{pid}/{file}")
}

fn read_stat<S: System>(sys: &S, pid: u32) -> Result<Option<Stat>, ()> {
    match sys.read_file(&proc_path(pid, "stat"), STAT_CAP) {
        Ok(bytes) => procfs::parse_stat(&bytes)
            .filter(|s| s.pid == pid)
            .map(Some)
            .ok_or(()),
        Err(FsError::NotFound) => Ok(None),
        Err(_) => Err(()),
    }
}

/// The identity keys read from each process's environment, by process
/// instance `(boot, pid, start ticks)`, so the looks of one click do not read
/// `/proc/<pid>/environ` again for a process already read. The observer
/// keeps one for the reuse window; the executor reads afresh.
#[derive(Clone, Debug, Default)]
pub struct EnvCache {
    seen: BTreeMap<(BootId, u32, u64), Identity>,
}

impl EnvCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

fn read_row<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    boot: Boot,
    pid: u32,
    old: &EnvCache,
    new: &mut EnvCache,
) -> Row {
    let stat = match read_stat(sys, pid) {
        Ok(Some(stat)) => stat,
        Ok(None) => return Row::Gone,
        Err(()) => return Row::Unreadable,
    };
    let uids = match sys.read_file(&proc_path(pid, "status"), STATUS_CAP) {
        Ok(bytes) => procfs::parse_status_uids(&bytes),
        Err(FsError::NotFound) => return Row::Gone,
        Err(_) => None,
    };
    let Some((ruid, euid)) = uids else {
        return Row::Unreadable;
    };
    let Some(start) = procfs::start_seconds(boot.btime, stat.starttime, cfg.clock_ticks) else {
        return Row::Unreadable;
    };
    // Kernel threads have no exe (`NotFound`); another user's is denied.
    let exe = match sys.read_link(&proc_path(pid, "exe")) {
        Ok(target) => procfs::link_target(&target)
            .map_or(classify_comm(&stat.comm), |(path, _deleted)| {
                classify_exe(&path)
            }),
        Err(_) => classify_comm(&stat.comm),
    };
    let argv = sys
        .read_file(&proc_path(pid, "cmdline"), CMDLINE_CAP)
        .map(|b| procfs::parse_cmdline(&b))
        .unwrap_or_default();
    let key = (boot.id, pid, stat.starttime);
    let identity = if ruid != cfg.uid {
        Identity::default()
    } else if let Some(known) = old.seen.get(&key) {
        known.clone()
    } else {
        sys.read_file(&proc_path(pid, "environ"), ENVIRON_CAP)
            .map(|b| procfs::parse_environ(&b))
            .unwrap_or_default()
    };
    let cwd = if is_agent_cli(exe) {
        match sys.read_link(&proc_path(pid, "cwd")) {
            Ok(target) => match procfs::link_target(&target) {
                Some((path, false)) => Fact::Known(path),
                _ => Fact::Unknown,
            },
            Err(_) => Fact::Unknown,
        }
    } else {
        Fact::Unknown
    };
    // Re-read `stat`: a pid that exited and was reused while its files
    // were read starts at another time, and nothing read above is kept.
    match read_stat(sys, pid) {
        Ok(Some(again)) if again.starttime == stat.starttime => {}
        Ok(_) => return Row::Gone,
        Err(()) => return Row::Unreadable,
    }
    if ruid == cfg.uid {
        new.seen.insert(key, identity.clone());
    }
    let info = ProcessInfo {
        identity: ProcessIdentity {
            pid,
            start,
            boot: boot.id,
        },
        ppid: stat.ppid,
        euid,
        ruid,
        exe,
        tty: procfs::tty_path(stat.tty_nr),
        argv,
        cwd,
        window: identity.window.map(WindowId),
    };
    Row::Read(Box::new(info), identity)
}

/// The whole table, or `None` when it cannot be read completely.
pub fn read_table<S: System>(sys: &S, cfg: &LinuxConfig, boot: Boot) -> Option<Table> {
    read_table_with(sys, cfg, boot, &mut EnvCache::new())
}

/// [`read_table`], taking each environment's identity keys from `cache`
/// when this process instance was read before (and recording new ones).
/// Entries of processes no longer listed are dropped.
pub fn read_table_with<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    boot: Boot,
    cache: &mut EnvCache,
) -> Option<Table> {
    let names = sys.list_dir(PROC).ok()?;
    let pids: Vec<u32> = names
        .iter()
        .filter(|n| !n.is_empty() && n.len() <= 10 && n.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|n| n.parse().ok())
        .collect();
    if pids.is_empty() || pids.len() > MAX_PROCESSES {
        return None;
    }
    let mut table = Table::default();
    let mut seen = EnvCache::new();
    for pid in pids {
        match read_row(sys, cfg, boot, pid, cache, &mut seen) {
            Row::Gone => {}
            Row::Unreadable => return None,
            Row::Read(info, identity) => {
                if identity != Identity::default() {
                    table.identities.insert(pid, identity);
                }
                table.processes.push(*info);
            }
        }
    }
    *cache = seen;
    (!table.processes.is_empty()).then_some(table)
}
