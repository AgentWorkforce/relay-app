//! The macOS [`Kernel`]: `sysctl`, `libproc` and `KERN_PROCARGS2` through
//! FFI. This file holds every `unsafe` block of the macOS adapter (the
//! Windows one has its own, `win32_real`). Rules kept here:
//! each block has a `// SAFETY:` note, every buffer is allocated by Rust with
//! its length passed to the kernel, every length the kernel returns is
//! checked against that buffer before slicing, and nothing is transmuted.
//! The structs read (`timeval`, `proc_bsdinfo`, `proc_vnodepathinfo`) are
//! plain C data for which all-zero bytes are a valid value; `kinfo_proc`
//! and the argument area are read as bytes and parsed by the safe
//! [`super::kernel_parse`].
//!
//! Off macOS there is no implementation: [`kernel`] answers `None` and the
//! adapters run the tools.
use super::kernel::Kernel;

/// The machine's kernel, when this platform has an implementation.
pub fn kernel() -> Option<&'static dyn Kernel> {
    imp::kernel()
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Kernel;

    pub fn kernel() -> Option<&'static dyn Kernel> {
        None
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::BTreeMap;
    use std::ffi::CStr;
    use std::mem::size_of;
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::sync::{Mutex, OnceLock, PoisonError};

    use super::super::kernel::RawProcess;
    use super::super::kernel_parse::{
        comm, parse_kinfo, parse_procargs, until_nul, Basic, KINFO_SIZE,
    };
    use super::Kernel;

    /// Bound on the pid list and on one argument area.
    const MAX_PIDS: usize = 65_536;
    const MAX_ARGS: usize = 4 * 1024 * 1024;
    /// `NODEV`: no controlling terminal.
    const NODEV: u32 = u32::MAX;
    /// Threads sharing the per-pid reads of one table.
    const THREADS: usize = 4;

    /// One process as read, before its tty is named.
    struct Read {
        basic: Basic,
        argv: Option<Vec<String>>,
        path: Option<String>,
    }

    pub struct MacKernel {
        /// tty device number → `/dev/<name>`, filled by scanning `/dev`.
        ttys: Mutex<BTreeMap<u32, String>>,
        argmax: OnceLock<Option<usize>>,
    }

    static KERNEL: MacKernel = MacKernel {
        ttys: Mutex::new(BTreeMap::new()),
        argmax: OnceLock::new(),
    };

    pub fn kernel() -> Option<&'static dyn Kernel> {
        Some(&KERNEL)
    }

    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    /// `sysctlbyname` of a fixed-size value; `None` unless exactly
    /// `size_of::<T>()` bytes came back.
    fn sysctl_value<T: Copy>(name: &CStr, mut value: T) -> Option<T> {
        let mut len = size_of::<T>();
        // SAFETY: `name` is NUL-terminated; `value` is a live, writable `T`
        // of `len` bytes and the kernel writes at most `len` bytes into it
        // (updating `len`); no new value is set (null, 0).
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut value as *mut T).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && len == size_of::<T>()).then_some(value)
    }

    fn zeroed_timeval() -> libc::timeval {
        libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        }
    }

    fn pids() -> Option<Vec<i32>> {
        for _ in 0..4 {
            // SAFETY: a null buffer of size 0 only asks for the count.
            let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
            let count = usize::try_from(count).ok()?;
            let room = count.checked_add(64)?.min(MAX_PIDS);
            let mut buffer = vec![0i32; room];
            let bytes = i32::try_from(room.checked_mul(size_of::<i32>())?).ok()?;
            // SAFETY: `buffer` owns `room` i32s = `bytes` bytes, which is the
            // size passed; the kernel writes at most that many.
            let got = unsafe { libc::proc_listallpids(buffer.as_mut_ptr().cast(), bytes) };
            let got = usize::try_from(got).ok()?;
            if got < room {
                buffer.truncate(got);
                return Some(buffer);
            }
            // Filled to the brim: the table grew meanwhile; ask again.
        }
        None
    }

    enum Info<T> {
        Read(T),
        Gone,
        Denied,
    }

    /// `proc_pidinfo` of one fixed-size flavor.
    fn pidinfo<T: Copy>(pid: i32, flavor: i32, mut value: T) -> Info<T> {
        let Ok(size) = i32::try_from(size_of::<T>()) else {
            return Info::Denied;
        };
        // SAFETY: `value` is a live, writable `T` of `size` bytes, the size
        // passed; the kernel writes at most that many.
        let got =
            unsafe { libc::proc_pidinfo(pid, flavor, 0, (&mut value as *mut T).cast(), size) };
        if got == size {
            return Info::Read(value);
        }
        match errno() {
            libc::ESRCH => Info::Gone,
            _ if got > 0 => Info::Denied,
            libc::EPERM | libc::EACCES => Info::Denied,
            _ => Info::Gone,
        }
    }

    fn zeroed_bsdinfo() -> libc::proc_bsdinfo {
        // SAFETY: `proc_bsdinfo` is a C struct of integers and char arrays;
        // all-zero bytes are a valid value of it.
        unsafe { std::mem::zeroed() }
    }

    fn zeroed_vnodepathinfo() -> libc::proc_vnodepathinfo {
        // SAFETY: `proc_vnodepathinfo` is a C struct of integers and char
        // arrays; all-zero bytes are a valid value of it.
        unsafe { std::mem::zeroed() }
    }

    fn bytes(chars: &[libc::c_char]) -> Vec<u8> {
        chars.iter().map(|c| *c as u8).collect()
    }

    fn from_bsdinfo(info: &libc::proc_bsdinfo) -> Basic {
        Basic {
            pid: info.pbi_pid,
            ppid: info.pbi_ppid,
            euid: info.pbi_uid,
            ruid: info.pbi_ruid,
            start: info.pbi_start_tvsec,
            tdev: info.e_tdev,
            comm: comm(&bytes(&info.pbi_name), &bytes(&info.pbi_comm)),
        }
    }

    /// `sysctl {CTL_KERN, KERN_PROC, KERN_PROC_PID, pid}`: what `ps` reads,
    /// allowed for every process. `Ok(None)`: no such process.
    fn kinfo(pid: i32) -> Result<Option<Basic>, ()> {
        let mut buffer = vec![0u8; KINFO_SIZE];
        let mut len = KINFO_SIZE;
        let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
        let count = u32::try_from(mib.len()).map_err(|_| ())?;
        // SAFETY: `mib` is a live array of `count` ints; `buffer` owns `len`
        // bytes and the kernel writes at most `len` (updating it); no new
        // value is set.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                count,
                buffer.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        match (rc, len) {
            (0, 0) => Ok(None),
            (0, KINFO_SIZE) => parse_kinfo(&buffer, pid).map(Some).ok_or(()),
            _ => Err(()),
        }
    }

    /// The basics of `pid`: `proc_bsdinfo` when allowed (the person's own
    /// live processes), else `kinfo_proc` (another user's, zombies).
    fn basic(pid: i32) -> Info<Basic> {
        match pidinfo(pid, libc::PROC_PIDTBSDINFO, zeroed_bsdinfo()) {
            Info::Read(info) => Info::Read(from_bsdinfo(&info)),
            // A zombie has no bsdinfo either, yet `ps` lists it.
            Info::Gone | Info::Denied => match kinfo(pid) {
                Ok(Some(basic)) => Info::Read(basic),
                Ok(None) => Info::Gone,
                Err(()) => Info::Denied,
            },
        }
    }

    /// A buffer for [`path`].
    fn path_buffer() -> Vec<u8> {
        vec![0u8; usize::try_from(libc::PROC_PIDPATHINFO_MAXSIZE).unwrap_or(4096)]
    }

    fn path(pid: i32, buffer: &mut [u8]) -> Option<String> {
        let size = buffer.len();
        let cap = u32::try_from(size).ok()?;
        // SAFETY: `buffer` is a live, writable slice of `size` bytes, the
        // size passed; the kernel writes at most that many and returns the
        // length written.
        let got = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), cap) };
        let got = usize::try_from(got).ok().filter(|n| *n > 0 && *n <= size)?;
        let bytes = buffer.get(..got)?;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(got);
        std::str::from_utf8(bytes.get(..end)?)
            .ok()
            .map(str::to_owned)
    }

    impl MacKernel {
        fn argmax(&self) -> Option<usize> {
            *self.argmax.get_or_init(|| {
                let max = sysctl_value(c"kern.argmax", 0i32)?;
                usize::try_from(max).ok().map(|m| m.min(MAX_ARGS))
            })
        }

        /// A buffer for [`Self::argv`]: one argument area at most.
        fn args_buffer(&self) -> Vec<u8> {
            vec![0u8; self.argmax().unwrap_or(0)]
        }

        fn argv(&self, pid: i32, buffer: &mut [u8]) -> Option<Vec<String>> {
            let size = buffer.len();
            if size == 0 {
                return None;
            }
            let mut len = size;
            let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
            let count = u32::try_from(mib.len()).ok()?;
            // SAFETY: `mib` is a live array of `count` ints; `buffer` is a
            // live, writable slice of `len` bytes and the kernel writes at
            // most `len` (updating it); no new value is set.
            let rc = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    count,
                    buffer.as_mut_ptr().cast(),
                    &mut len,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if rc != 0 || len > size {
                return None;
            }
            parse_procargs(buffer.get(..len)?)
        }

        /// Every pid of `part`; a process that exits meanwhile (or whose
        /// pid is reused) is left out. `None` when one cannot be read at
        /// all: the table would have a hole.
        fn read_part(&self, part: &[i32]) -> Option<Vec<Read>> {
            let (mut args, mut paths) = (self.args_buffer(), path_buffer());
            let mut out = Vec::with_capacity(part.len());
            for &pid in part {
                let basic = match basic(pid) {
                    Info::Read(basic) => basic,
                    Info::Gone => continue,
                    Info::Denied => return None,
                };
                if i32::try_from(basic.pid).ok() != Some(pid) {
                    continue;
                }
                let argv = self.argv(pid, &mut args);
                let path = path(pid, &mut paths);
                match self::basic(pid) {
                    Info::Read(again) if again.start == basic.start => {}
                    _ => continue,
                }
                out.push(Read { basic, argv, path });
            }
            Some(out)
        }

        /// Every `/dev/tty*` and `/dev/console` character device by number.
        fn scan_ttys() -> BTreeMap<u32, String> {
            let mut out = BTreeMap::new();
            let Ok(entries) = std::fs::read_dir("/dev") else {
                return out;
            };
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if !(name.starts_with("tty") || name == "console") {
                    continue;
                }
                let Ok(meta) = std::fs::metadata(entry.path()) else {
                    continue;
                };
                if !meta.file_type().is_char_device() {
                    continue;
                }
                if let Ok(dev) = u32::try_from(meta.rdev()) {
                    out.insert(dev, format!("/dev/{name}"));
                }
            }
            out
        }

        /// `/dev/<name>` of every device in `devs`, scanning `/dev` again at
        /// most once when one is not known yet.
        fn tty_names(&self, devs: &[u32]) -> BTreeMap<u32, String> {
            let mut known = self.ttys.lock().unwrap_or_else(PoisonError::into_inner);
            if devs.iter().any(|d| !known.contains_key(d)) {
                *known = Self::scan_ttys();
            }
            known.clone()
        }
    }

    impl Kernel for MacKernel {
        fn boot_time(&self) -> Option<u64> {
            let tv = sysctl_value(c"kern.boottime", zeroed_timeval())?;
            u64::try_from(tv.tv_sec).ok().filter(|s| *s > 0)
        }

        fn processes(&self) -> Option<Vec<RawProcess>> {
            // `ps -ax` does not list pid 0 (kernel_task) either.
            let pids: Vec<i32> = pids()?.into_iter().filter(|p| *p > 0).collect();
            // The per-pid reads are independent: a few threads share them.
            let chunk = pids.len().div_ceil(THREADS).max(1);
            let parts: Vec<Option<Vec<Read>>> = std::thread::scope(|scope| {
                let handles: Vec<_> = pids
                    .chunks(chunk)
                    .map(|part| scope.spawn(move || self.read_part(part)))
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().ok().flatten())
                    .collect()
            });
            let mut rows = Vec::with_capacity(pids.len());
            for part in parts {
                rows.extend(part?);
            }
            let devs: Vec<u32> = rows
                .iter()
                .map(|r| r.basic.tdev)
                .filter(|d| *d != NODEV)
                .collect();
            let ttys = self.tty_names(&devs);
            let out: Vec<RawProcess> = rows
                .into_iter()
                .map(|r| RawProcess {
                    pid: r.basic.pid,
                    ppid: r.basic.ppid,
                    euid: r.basic.euid,
                    ruid: r.basic.ruid,
                    start: r.basic.start,
                    tty: ttys.get(&r.basic.tdev).cloned(),
                    comm: r.basic.comm,
                    path: r.path,
                    argv: r.argv,
                })
                .collect();
            (!out.is_empty()).then_some(out)
        }

        fn cwd(&self, pid: u32) -> Option<String> {
            let pid = i32::try_from(pid).ok()?;
            let Info::Read(info) =
                pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, zeroed_vnodepathinfo())
            else {
                return None;
            };
            let path: Vec<u8> = info
                .pvi_cdir
                .vip_path
                .iter()
                .flat_map(|chunk| bytes(chunk))
                .collect();
            std::str::from_utf8(until_nul(&path))
                .ok()
                .filter(|p| p.starts_with('/'))
                .map(str::to_owned)
        }
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::panic)]
        use super::{basic, kinfo, Info};

        #[test]
        fn kinfo_offsets_agree_with_bsdinfo_for_this_process() {
            let me = i32::try_from(std::process::id()).unwrap_or(0);
            let Info::Read(own) = basic(me) else {
                panic!("own bsdinfo");
            };
            assert_eq!(kinfo(me), Ok(Some(own.clone())));
            assert_eq!(own.ppid, std::os::unix::process::parent_id());
            // launchd is root's: denied to bsdinfo, read through kinfo.
            let Info::Read(launchd) = basic(1) else {
                panic!("launchd basics");
            };
            assert_eq!(
                (launchd.ppid, launchd.ruid, launchd.comm.as_str()),
                (0, 0, "launchd")
            );
        }
    }
}
