//! Pure parsers for the bytes the macOS kernel hands back
//! ([`super::kernel_macos`] does the reading). Safe code, no `cfg`: they
//! build and test on every OS. Each returns `None` for anything short,
//! malformed or for another pid; none of them panics.
use std::mem::size_of;

/// What every process row needs, from `proc_bsdinfo` or `kinfo_proc`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Basic {
    pub pid: u32,
    pub ppid: u32,
    pub euid: u32,
    pub ruid: u32,
    /// Start time, whole seconds since the epoch.
    pub start: u64,
    /// The controlling terminal's device, `u32::MAX` (`NODEV`) for none.
    pub tdev: u32,
    /// `p_comm`: the executable's name, at most 16 bytes, as `ps` prints it.
    pub comm: String,
}

/// `p_comm` holds `MAXCOMLEN` (16) bytes of the executable's name.
pub const MAXCOMLEN: usize = 16;

/// The bytes before the first NUL (all of them when there is none).
pub fn until_nul(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    bytes.get(..end).unwrap_or_default()
}

/// `p_comm` from `proc_bsdinfo`: its `pbi_comm` keeps only 15 bytes and a
/// NUL, while `pbi_name` (`p_name`, 32 bytes) starts with the same name,
/// so the first 16 bytes of `pbi_name` are `p_comm` as `ps` shows it.
pub fn comm(name: &[u8], short: &[u8]) -> String {
    let name = until_nul(name);
    let chosen = if name.is_empty() {
        until_nul(short)
    } else {
        name.get(..name.len().min(MAXCOMLEN)).unwrap_or_default()
    };
    String::from_utf8_lossy(chosen).into_owned()
}

/// `struct kinfo_proc` (LP64, both arm64 and x86_64): 648 bytes, an
/// `extern_proc` then an `eproc`. `libc` has no definition, so the fields
/// `ps` prints are read at their offsets from `<sys/sysctl.h>` and
/// `<sys/proc.h>`; the length and the pid are checked first, and a test
/// compares them with `proc_bsdinfo` for the test's own process.
pub const KINFO_SIZE: usize = 648;
const KINFO_START: usize = 0; // kp_proc.p_un.p_starttime.tv_sec (i64)
const KINFO_PID: usize = 40; // kp_proc.p_pid
const KINFO_COMM: usize = 243; // kp_proc.p_comm[MAXCOMLEN + 1]
const KINFO_RUID: usize = 392; // kp_eproc.e_pcred.p_ruid
const KINFO_EUID: usize = 420; // kp_eproc.e_ucred.cr_uid
const KINFO_PPID: usize = 560; // kp_eproc.e_ppid
const KINFO_TDEV: usize = 572; // kp_eproc.e_tdev

fn field<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

/// One `kinfo_proc`, which must describe `pid`.
pub fn parse_kinfo(bytes: &[u8], pid: i32) -> Option<Basic> {
    if bytes.len() != KINFO_SIZE {
        return None;
    }
    let u32_at = |at| field::<4>(bytes, at).map(u32::from_ne_bytes);
    if i32::from_ne_bytes(field::<4>(bytes, KINFO_PID)?) != pid {
        return None;
    }
    let start = i64::from_ne_bytes(field::<8>(bytes, KINFO_START)?);
    let comm = until_nul(bytes.get(KINFO_COMM..KINFO_COMM + MAXCOMLEN + 1)?);
    Some(Basic {
        pid: u32::try_from(pid).ok()?,
        ppid: u32_at(KINFO_PPID)?,
        euid: u32_at(KINFO_EUID)?,
        ruid: u32_at(KINFO_RUID)?,
        start: u64::try_from(start).ok()?,
        tdev: u32_at(KINFO_TDEV)?,
        comm: String::from_utf8_lossy(comm).into_owned(),
    })
}

/// `KERN_PROCARGS2`: `argc` (a native `i32`), the exec path, NUL padding,
/// then `argc` NUL-terminated arguments, then the environment, which is
/// never read.
pub fn parse_procargs(bytes: &[u8]) -> Option<Vec<String>> {
    let head = field::<4>(bytes, 0)?;
    let argc = usize::try_from(i32::from_ne_bytes(head))
        .ok()
        .filter(|n| *n <= 65_536)?;
    let mut rest = bytes.get(size_of::<i32>()..)?;
    // The exec path, then its padding.
    let end = rest.iter().position(|b| *b == 0)?;
    rest = rest.get(end..)?;
    let start = rest.iter().position(|b| *b != 0).unwrap_or(rest.len());
    rest = rest.get(start..)?;
    let mut argv = Vec::with_capacity(argc.min(1024));
    for _ in 0..argc {
        if rest.is_empty() {
            break;
        }
        let arg = until_nul(rest);
        argv.push(String::from_utf8_lossy(arg).into_owned());
        rest = rest.get(arg.len().saturating_add(1).min(rest.len())..)?;
    }
    Some(argv)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    fn area(argc: i32, body: &[u8]) -> Vec<u8> {
        let mut v = argc.to_ne_bytes().to_vec();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn procargs_keeps_argv_and_drops_the_environment() {
        let bytes = area(3, b"/bin/zsh\0\0\0\0-zsh\0\0-l\0HOME=/x\0SECRET=y\0");
        assert_eq!(
            parse_procargs(&bytes),
            Some(vec!["-zsh".to_owned(), String::new(), "-l".to_owned()])
        );
    }

    #[test]
    fn procargs_rejects_garbage() {
        assert_eq!(parse_procargs(b"\x01"), None);
        assert_eq!(parse_procargs(&area(-1, b"/x\0a\0")), None);
        assert_eq!(parse_procargs(&area(1, b"no-nul")), None);
        // argc larger than what is there: what is there.
        assert_eq!(
            parse_procargs(&area(3, b"/x\0a\0")),
            Some(vec!["a".to_owned()])
        );
        assert_eq!(parse_procargs(&area(0, b"/x\0")), Some(vec![]));
    }

    #[test]
    fn kinfo_reads_its_offsets_and_checks_length_and_pid() {
        let mut bytes = vec![0u8; KINFO_SIZE];
        bytes[KINFO_START..KINFO_START + 8].copy_from_slice(&1_794_000_000i64.to_ne_bytes());
        bytes[KINFO_PID..KINFO_PID + 4].copy_from_slice(&42i32.to_ne_bytes());
        bytes[KINFO_COMM..KINFO_COMM + 7].copy_from_slice(b"launchd");
        bytes[KINFO_RUID..KINFO_RUID + 4].copy_from_slice(&0u32.to_ne_bytes());
        bytes[KINFO_EUID..KINFO_EUID + 4].copy_from_slice(&u32::MAX.wrapping_sub(1).to_ne_bytes());
        bytes[KINFO_PPID..KINFO_PPID + 4].copy_from_slice(&1u32.to_ne_bytes());
        bytes[KINFO_TDEV..KINFO_TDEV + 4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert_eq!(
            parse_kinfo(&bytes, 42),
            Some(Basic {
                pid: 42,
                ppid: 1,
                euid: 4_294_967_294,
                ruid: 0,
                start: 1_794_000_000,
                tdev: u32::MAX,
                comm: "launchd".into(),
            })
        );
        assert_eq!(parse_kinfo(&bytes, 43), None);
        assert_eq!(parse_kinfo(&bytes[..600], 42), None);
    }

    #[test]
    fn comm_is_the_first_sixteen_bytes_of_the_name() {
        assert_eq!(
            comm(b"SkyComputerUseService\0", b"SkyComputerUseS\0"),
            "SkyComputerUseSe"
        );
        assert_eq!(comm(b"zsh\0\0\0", b"zsh\0"), "zsh");
        assert_eq!(comm(b"\0", b"short\0"), "short");
    }
}
