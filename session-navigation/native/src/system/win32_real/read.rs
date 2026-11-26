//! The real Win32 reads: boot, the process table, windows, foreground,
//! the registry delegation and Windows Terminal's package.
use super::base::*;
use crate::system::win32::{
    filetime_to_unix, utf16_until_nul, RawWinProcess, RawWindow, Win32Error, WinBoot,
    WinDelegation, WinTerminalPackage,
};
use std::mem::{offset_of, size_of};
use std::ptr::null_mut;
use windows_sys::core::{BOOL, PWSTR};
use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemTimeOfDayInformation,
};
use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_NO_MORE_FILES, HWND, LPARAM,
    STATUS_INFO_LENGTH_MISMATCH, UNICODE_STRING,
};
use windows_sys::Win32::Storage::Packaging::Appx::GetPackagesByPackageFamily;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindow, IsWindowVisible, GW_OWNER,
};

pub(super) fn image_path(process: &Handle) -> Option<String> {
    for capacity in [1024usize, 32_768] {
        let mut buffer = vec![0u16; capacity];
        let mut len = u32::try_from(capacity).ok()?;
        // SAFETY: `buffer` holds `len` writable units; on success `len` is
        // the count written, excluding the NUL.
        let ok = unsafe {
            QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len)
        };
        if ok != 0 {
            let units = buffer.get(..usize::try_from(len).ok()?)?;
            return Some(String::from_utf16_lossy(units));
        }
        if last_error().code != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
    }
    None
}

/// The `UNICODE_STRING` at the start of `buffer`, whose characters must
/// lie inside `buffer` itself.
pub(super) fn unicode_in(buffer: &[u64]) -> Option<String> {
    let bytes: Vec<u8> = buffer.iter().flat_map(|w| w.to_ne_bytes()).collect();
    let length = u16::from_ne_bytes(bytes.get(..2)?.try_into().ok()?);
    let at = offset_of!(UNICODE_STRING, Buffer);
    let raw = bytes.get(at..at + size_of::<usize>())?;
    let pointer = usize::from_ne_bytes(raw.try_into().ok()?);
    if length == 0 {
        return Some(String::new());
    }
    let start = pointer.checked_sub(buffer.as_ptr() as usize)?;
    let end = start.checked_add(usize::from(length))?;
    let units: Vec<u16> = bytes
        .get(start..end)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_ne_bytes(*pair))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

pub(super) fn command_line(process: &Handle) -> Option<String> {
    // u64 units: `UNICODE_STRING`'s alignment.
    let mut buffer = vec![0u64; 128];
    for _ in 0..3 {
        let size = u32::try_from(buffer.len() * 8).ok()?;
        let mut needed = 0u32;
        // SAFETY: `buffer` is `size` writable bytes, 8-byte aligned; the
        // call writes at most `size` bytes and reports the size needed.
        let status = unsafe {
            NtQueryInformationProcess(
                process.0,
                ProcessCommandLineInformation,
                buffer.as_mut_ptr().cast(),
                size,
                &mut needed,
            )
        };
        if matches!(
            status,
            STATUS_INFO_LENGTH_MISMATCH | STATUS_BUFFER_OVERFLOW | STATUS_BUFFER_TOO_SMALL
        ) {
            let needed = usize::try_from(needed).ok()?;
            if needed == 0 || needed > MAX_UNITS * 2 {
                return None;
            }
            buffer = vec![0u64; needed.div_ceil(8) + 1];
            continue;
        }
        return (status >= 0).then(|| unicode_in(&buffer)).flatten();
    }
    None
}

pub(super) fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0u32;
    // SAFETY: `session` is a live, writable u32.
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

/// One snapshot row; `None` when its pid now belongs to a process
/// created after the snapshot (a reused pid).
pub(super) fn row(entry: &PROCESSENTRY32W, snapshot_ticks: u64) -> Option<RawWinProcess> {
    let pid = entry.th32ProcessID;
    let handle = open(pid);
    let created_ticks = handle.as_ref().and_then(creation_ticks);
    if created_ticks.is_some_and(|t| t > snapshot_ticks) {
        return None;
    }
    Some(RawWinProcess {
        pid,
        ppid: entry.th32ParentProcessID,
        created: created_ticks.and_then(filetime_to_unix),
        name: utf16_until_nul(&entry.szExeFile),
        image: handle.as_ref().and_then(image_path),
        command_line: handle.as_ref().and_then(command_line),
        session: session_of(pid),
    })
}

/// `EnumWindows` callback: visible windows without an owner window.
unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the `&mut Vec<usize>` that `windows` passes;
    // EnumWindows calls this synchronously on that thread while the
    // vector lives, and the reference does not escape this call.
    let list = unsafe { &mut *(data as *mut Vec<usize>) };
    // SAFETY: no Rust pointers; Win32 validates the opaque handle.
    let shown = unsafe { IsWindowVisible(hwnd) != 0 && GetWindow(hwnd, GW_OWNER).is_null() };
    if shown {
        if list.len() >= MAX_WINDOWS {
            return 0;
        }
        list.push(hwnd as usize);
    }
    1
}

pub(super) fn version_key(version: &str) -> Vec<u64> {
    version.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

pub(super) fn boot() -> Result<WinBoot, Win32Error> {
    let boot_id = reg_dword(HKEY_LOCAL_MACHINE, BOOT_KEY, "BootId")
        .ok()
        .flatten()
        .map(u64::from);
    // SYSTEM_TIMEOFDAY_INFORMATION: BootTime is its first field.
    let mut info = [0u64; 6];
    let mut written = 0u32;
    // SAFETY: `info` is 48 writable, 8-aligned bytes (the structure's
    // size); the call writes at most that and reports how much.
    let status = unsafe {
        NtQuerySystemInformation(
            SystemTimeOfDayInformation,
            info.as_mut_ptr().cast(),
            size_of::<[u64; 6]>() as u32,
            &mut written,
        )
    };
    let boot_time = (status >= 0 && written >= 8)
        .then(|| info.first().copied())
        .flatten()
        .and_then(filetime_to_unix);
    Ok(WinBoot { boot_id, boot_time })
}

pub(super) fn processes() -> Result<Vec<RawWinProcess>, Win32Error> {
    let snapshot_ticks = system_ticks();
    // SAFETY: no pointers; a failure is INVALID_HANDLE_VALUE.
    let snapshot = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
        .ok_or_else(last_error)?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut rows = Vec::new();
    // SAFETY: a live snapshot handle and an entry with `dwSize` set.
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while more {
        if rows.len() >= MAX_PROCESSES {
            return Err(Win32Error::new(ERROR_MORE_DATA));
        }
        rows.extend(row(&entry, snapshot_ticks));
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    let end = last_error();
    if end.code != ERROR_NO_MORE_FILES || rows.is_empty() {
        return Err(end);
    }
    Ok(rows)
}

pub(super) fn windows() -> Result<Vec<RawWindow>, Win32Error> {
    let mut list: Vec<usize> = Vec::new();
    // SAFETY: `collect` only touches `list` through the pointer during
    // this synchronous call; `list` outlives it.
    let ok = unsafe { EnumWindows(Some(collect), &mut list as *mut Vec<usize> as LPARAM) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(list.into_iter().map(raw_window).collect())
}

pub(super) fn foreground() -> Result<Option<RawWindow>, Win32Error> {
    // SAFETY: no pointers; the HWND stays OS-owned.
    let hwnd = unsafe { GetForegroundWindow() };
    Ok((!hwnd.is_null()).then(|| raw_window(hwnd as usize)))
}

pub(super) fn default_terminal_delegation() -> Result<WinDelegation, Win32Error> {
    let read = |name| reg_string(HKEY_CURRENT_USER, STARTUP_KEY, name);
    Ok(WinDelegation {
        console: read("DelegationConsole")?.unwrap_or_default(),
        terminal: read("DelegationTerminal")?.unwrap_or_default(),
    })
}

pub(super) fn terminal_package() -> Result<WinTerminalPackage, Win32Error> {
    let family = wide(WT_FAMILY).ok_or(Win32Error::new(87))?;
    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: zero counts with null arrays ask for the sizes only.
    let status = unsafe {
        GetPackagesByPackageFamily(
            family.as_ptr(),
            &mut count,
            null_mut(),
            &mut len,
            null_mut(),
        )
    };
    if status == 0 && count == 0 {
        return Ok(WinTerminalPackage {
            installed: false,
            version: None,
        });
    }
    if status != ERROR_INSUFFICIENT_BUFFER {
        return Err(Win32Error::new(status));
    }
    let (n, l) = (usize::try_from(count), usize::try_from(len));
    let (Ok(n), Ok(l)) = (n, l) else {
        return Err(Win32Error::new(87));
    };
    if n > 64 || l > MAX_UNITS {
        return Err(Win32Error::new(ERROR_MORE_DATA));
    }
    let mut names: Vec<PWSTR> = vec![null_mut(); n];
    let mut buffer = vec![0u16; l];
    // SAFETY: `names` has `count` slots and `buffer` `len` units, the
    // sizes the first call asked for; the names point into `buffer`,
    // which is read below by value only.
    let status = unsafe {
        GetPackagesByPackageFamily(
            family.as_ptr(),
            &mut count,
            names.as_mut_ptr(),
            &mut len,
            buffer.as_mut_ptr(),
        )
    };
    if status != 0 {
        return Err(Win32Error::new(status));
    }
    let version = buffer
        .split(|u| *u == 0)
        .filter(|s| !s.is_empty())
        .take(n)
        .filter_map(|s| {
            String::from_utf16_lossy(s)
                .split('_')
                .nth(1)
                .map(str::to_owned)
        })
        .max_by_key(|v| version_key(v));
    Ok(WinTerminalPackage {
        installed: count > 0,
        version,
    })
}
