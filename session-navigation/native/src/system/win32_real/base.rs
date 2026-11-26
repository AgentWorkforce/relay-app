//! Handle guards and shared helpers of the real Win32 (Windows only).
use crate::system::win32::{filetime_to_unix, utf16_until_nul, RawWindow, Win32Error};
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, FILETIME, HANDLE, HWND,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Environment::DestroyEnvironmentBlock;
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

pub(super) const MAX_PROCESSES: usize = 50_000;
pub(super) const MAX_WINDOWS: usize = 20_000;
/// 1 MiB of UTF-16 for one command line or environment block.
pub(super) const MAX_UNITS: usize = 512 * 1024;
/// `STATUS_BUFFER_OVERFLOW` and `STATUS_BUFFER_TOO_SMALL`.
pub(super) const STATUS_BUFFER_OVERFLOW: i32 = 0x8000_0005_u32 as i32;
pub(super) const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023_u32 as i32;
pub(super) const WT_FAMILY: &str = "Microsoft.WindowsTerminal_8wekyb3d8bbwe";
pub(super) const BOOT_KEY: &str =
    r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management\PrefetchParameters";
pub(super) const STARTUP_KEY: &str = r"Console\%%Startup";

/// An owned kernel handle, closed once on drop.
pub(super) struct Handle(pub(super) HANDLE);

impl Handle {
    pub(super) fn new(raw: HANDLE) -> Option<Self> {
        (!raw.is_null() && raw != INVALID_HANDLE_VALUE).then_some(Self(raw))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a valid handle this guard owns (checked in
        // `new`) and nothing else closes it.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// A `CreateEnvironmentBlock` block, destroyed once on drop.
pub(super) struct EnvBlock(pub(super) *mut c_void);

impl Drop for EnvBlock {
    fn drop(&mut self) {
        // SAFETY: the block came from a successful CreateEnvironmentBlock
        // and is destroyed only here.
        unsafe {
            DestroyEnvironmentBlock(self.0);
        }
    }
}

pub(super) fn last_error() -> Win32Error {
    // SAFETY: reads this thread's last-error value; no pointers.
    Win32Error::new(unsafe { GetLastError() })
}

pub(super) fn wide(text: &str) -> Option<Vec<u16>> {
    (!text.contains('\0')).then(|| text.encode_utf16().chain([0]).collect())
}

pub(super) fn ticks(time: &FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

pub(super) fn zero_time() -> FILETIME {
    FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    }
}

pub(super) fn system_ticks() -> u64 {
    let mut now = zero_time();
    // SAFETY: `now` is a live, writable FILETIME.
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    ticks(&now)
}

pub(super) fn open(pid: u32) -> Option<Handle> {
    // SAFETY: no pointers; a failure is a null handle, refused by `new`.
    Handle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
}

/// Creation time in FILETIME ticks.
pub(super) fn creation_ticks(process: &Handle) -> Option<u64> {
    let (mut c, mut e, mut k, mut u) = (zero_time(), zero_time(), zero_time(), zero_time());
    // SAFETY: a live process handle and four live, writable FILETIMEs.
    let ok = unsafe { GetProcessTimes(process.0, &mut c, &mut e, &mut k, &mut u) };
    (ok != 0).then(|| ticks(&c))
}

pub(super) fn created_of(pid: u32) -> Option<u64> {
    open(pid)
        .as_ref()
        .and_then(creation_ticks)
        .and_then(filetime_to_unix)
}

pub(super) fn reg_dword(root: HKEY, key: &str, name: &str) -> Result<Option<u32>, Win32Error> {
    let (k, n) = (wide(key), wide(name));
    let (Some(k), Some(n)) = (k, n) else {
        return Err(Win32Error::new(87));
    };
    let (mut value, mut size) = (0u32, size_of::<u32>() as u32);
    // SAFETY: NUL-terminated names and a `u32` out buffer of `size` bytes.
    let status = unsafe {
        RegGetValueW(
            root,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    match status {
        0 => Ok(Some(value)),
        ERROR_FILE_NOT_FOUND => Ok(None),
        code => Err(Win32Error::new(code)),
    }
}

pub(super) fn reg_string(root: HKEY, key: &str, name: &str) -> Result<Option<String>, Win32Error> {
    let (Some(k), Some(n)) = (wide(key), wide(name)) else {
        return Err(Win32Error::new(87));
    };
    let mut units = 256usize;
    for _ in 0..4 {
        let mut buffer = vec![0u16; units];
        let mut size = u32::try_from(units * 2).map_err(|_| Win32Error::new(87))?;
        // SAFETY: NUL-terminated names and `size` writable bytes.
        let status = unsafe {
            RegGetValueW(
                root,
                k.as_ptr(),
                n.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        match status {
            0 => {
                let len = (usize::try_from(size).unwrap_or(0) / 2).min(buffer.len());
                return Ok(Some(utf16_until_nul(buffer.get(..len).unwrap_or_default())));
            }
            ERROR_FILE_NOT_FOUND => return Ok(None),
            ERROR_MORE_DATA => {
                units = (usize::try_from(size).unwrap_or(0) / 2 + 1).max(units * 2);
                if units > MAX_UNITS {
                    break;
                }
            }
            code => return Err(Win32Error::new(code)),
        }
    }
    Err(Win32Error::new(ERROR_MORE_DATA))
}

pub(super) fn owner_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `pid` is a live, writable u32; a stale HWND leaves it 0.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

pub(super) fn raw_window(hwnd: usize) -> RawWindow {
    let pid = owner_pid(hwnd as HWND);
    RawWindow {
        hwnd: hwnd as u64,
        owner_pid: pid,
        owner_created: (pid != 0).then(|| created_of(pid)).flatten(),
    }
}
