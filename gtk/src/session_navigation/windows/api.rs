//! Small Win32 boundary: owned UTF-16 buffers and synchronous enumeration keep
//! all raw pointers within this module. The navigation policy uses safe values.
use super::NativeWindow;
use std::ptr;
use windows_sys::{
    core::BOOL,
    Win32::{
        Foundation::{HWND, LPARAM},
        UI::{
            Shell::ShellExecuteW,
            WindowsAndMessaging::{
                EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindow,
                IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOWNORMAL,
            },
        },
    },
};

struct WindowsForProcess {
    pid: i32,
    windows: Vec<NativeWindow>,
}

extern "system" fn collect(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: only visible_windows installs this callback, EnumWindows invokes
    // it synchronously with the unique live stack state, and no pointer escapes.
    let state = unsafe { &mut *(data as *mut WindowsForProcess) };
    let id = window as usize;
    if owner(id) == state.pid as u32 && visible(id) {
        state.windows.push(NativeWindow { id, pid: state.pid });
    }
    1
}

pub(super) fn visible_windows(pid: i32) -> Vec<NativeWindow> {
    let mut state = WindowsForProcess {
        pid,
        windows: vec![],
    };
    // SAFETY: state lives until synchronous EnumWindows returns; collect never
    // retains or aliases its pointer outside the callback invocation.
    unsafe {
        EnumWindows(Some(collect), &mut state as *mut _ as LPARAM);
    }
    state.windows
}

pub(super) fn owner(id: usize) -> u32 {
    let mut pid = 0;
    // SAFETY: pid is a live writable u32. Invalid or expired HWNDs fail closed.
    unsafe {
        GetWindowThreadProcessId(id as HWND, &mut pid);
    }
    pid
}

fn visible(id: usize) -> bool {
    // SAFETY: no Rust pointers; Win32 validates the opaque window handle.
    unsafe { IsWindowVisible(id as HWND) != 0 }
}

pub(super) fn is_window(id: usize) -> bool {
    // SAFETY: no Rust pointers; this call tests an opaque handle's validity.
    unsafe { IsWindow(id as HWND) != 0 }
}

pub(super) fn restore(id: usize) {
    // SAFETY: no Rust pointers; Win32 validates the opaque HWND.
    unsafe {
        ShowWindow(id as HWND, SW_RESTORE);
    }
}

pub(super) fn set_foreground(id: usize) -> bool {
    // SAFETY: no Rust pointers; Win32 validates the opaque HWND. Foreground
    // restrictions are reported to the caller rather than bypassed.
    unsafe { SetForegroundWindow(id as HWND) != 0 }
}

pub(super) fn foreground_window() -> usize {
    // SAFETY: no pointers or ownership transfer; the HWND remains OS-owned.
    unsafe { GetForegroundWindow() as usize }
}

pub(super) fn open_link(link: &str) -> bool {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let (verb, link) = (wide("open"), wide(link));
    // SAFETY: owned NUL-terminated UTF-16 strings outlive this call; optional
    // pointers are null. The caller has already validated the URI scheme.
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            link.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    result as isize > 32
}
