//! The real Win32 acts and their inputs: focus, the person's
//! environment, CreateProcessW.
use super::base::*;
use crate::system::win32::{
    environment_block, parse_environment_block, FocusRequest, FocusResult, SpawnConsole,
    SpawnRequest, Win32Error,
};
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::time::Duration;
use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, HANDLE, HWND};
use windows_sys::Win32::Security::{TOKEN_DUPLICATE, TOKEN_QUERY};
use windows_sys::Win32::System::Environment::CreateEnvironmentBlock;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetCurrentProcess, OpenProcessToken, CREATE_NEW_CONSOLE, CREATE_NO_WINDOW,
    CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, IsIconic, IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

pub(super) fn focus(request: &FocusRequest) -> Result<FocusResult, Win32Error> {
    let raw = usize::try_from(request.hwnd).map_err(|_| Win32Error::new(87))?;
    let hwnd = raw as HWND;
    // SAFETY: no Rust pointers; tests an opaque handle's validity.
    if unsafe { IsWindow(hwnd) } == 0 {
        return Ok(FocusResult::Gone);
    }
    let owner = owner_pid(hwnd);
    let same = owner == request.owner_pid
        && created_of(owner) == Some(request.owner_created)
        && created_of(request.process_pid) == Some(request.process_created);
    if !same {
        return Ok(FocusResult::Gone);
    }
    // SAFETY: no Rust pointers; Win32 validates the opaque HWND.
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
    }
    // SAFETY: as above; a foreground-lock refusal is reported, not
    // bypassed.
    if unsafe { SetForegroundWindow(hwnd) } == 0 {
        return Ok(FocusResult::Refused);
    }
    std::thread::sleep(Duration::from_millis(150));
    // SAFETY: no pointers.
    let front = unsafe { GetForegroundWindow() };
    Ok(if front == hwnd {
        FocusResult::Focused
    } else {
        FocusResult::Unconfirmed
    })
}

pub(super) fn user_environment() -> Result<Vec<(String, String)>, Win32Error> {
    let mut token: HANDLE = null_mut();
    // SAFETY: this process's pseudo-handle (never closed) and a live,
    // writable HANDLE.
    let ok = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &mut token,
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    let token = Handle::new(token).ok_or_else(last_error)?;
    let mut raw: *mut c_void = null_mut();
    // SAFETY: a live token with the access the call needs; `raw`
    // receives a block owned by the EnvBlock guard below.
    if unsafe { CreateEnvironmentBlock(&mut raw, token.0, 0) } == 0 || raw.is_null() {
        return Err(last_error());
    }
    let block = EnvBlock(raw);
    let base = block.0 as *const u16;
    let mut units: Vec<u16> = Vec::new();
    for i in 0..MAX_UNITS {
        // SAFETY: the block is a double-NUL-terminated UTF-16 block
        // (the API's contract); reading stops at that terminator and
        // never past MAX_UNITS.
        let unit = unsafe { base.add(i).read() };
        units.push(unit);
        if unit == 0 && (i == 0 || units.get(i - 1) == Some(&0)) {
            return Ok(parse_environment_block(&units));
        }
    }
    Err(Win32Error::new(ERROR_MORE_DATA))
}

pub(super) fn spawn(request: &SpawnRequest) -> Result<(), Win32Error> {
    let bad = || Win32Error::new(87);
    let app = wide(&request.application).ok_or_else(bad)?;
    let mut line = wide(&request.command_line).ok_or_else(bad)?;
    let cwd = wide(&request.cwd).ok_or_else(bad)?;
    let env = environment_block(&request.environment).ok_or_else(bad)?;
    let console = match request.console {
        SpawnConsole::New => CREATE_NEW_CONSOLE,
        SpawnConsole::Hidden => CREATE_NO_WINDOW,
    };
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: NUL-terminated application, directory and (mutable)
    // command line; a double-NUL-terminated UTF-16 environment
    // (CREATE_UNICODE_ENVIRONMENT); no inherited handles; live
    // startup and information structures. All outlive the call.
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            0,
            console | CREATE_UNICODE_ENVIRONMENT,
            env.as_ptr().cast(),
            cwd.as_ptr(),
            &startup,
            &mut info,
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    // Not waited for: both handles are closed at once.
    drop(Handle::new(info.hProcess));
    drop(Handle::new(info.hThread));
    Ok(())
}
