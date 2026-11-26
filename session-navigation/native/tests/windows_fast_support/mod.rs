//! The same fake Windows machine as `windows_support`, answered through
//! `FakeSystem`'s scripted Win32 instead of PowerShell. `fast(&m)` still
//! scripts every PowerShell run (so a silent fallback would still pass a
//! scenario); the tests therefore also assert [`powershell_runs`] is zero.
#![allow(dead_code)]

use session_navigation_native::system::win32::{
    RawWinProcess, RawWindow, Win32Error, WinBoot, WinTerminalPackage,
};
use session_navigation_native::system::{FakeSystem, ScriptedWin32, WinWorld};

use super::windows_support::{Machine, State, Win, WinProc, BOOT_ID, BOOT_TIME};

/// The environment `CreateEnvironmentBlock` answers in these tests.
pub fn user_env() -> Vec<(String, String)> {
    vec![
        (
            "Path".to_owned(),
            r"C:\Windows\System32;C:\Windows".to_owned(),
        ),
        ("USERPROFILE".to_owned(), r"C:\Users\dev".to_owned()),
    ]
}

pub fn raw_process(p: &WinProc) -> RawWinProcess {
    RawWinProcess {
        pid: p.pid,
        ppid: p.ppid,
        created: Some(p.start),
        name: p.name.clone(),
        image: p.path.clone(),
        command_line: p.cmd.clone(),
        session: Some(1),
    }
}

pub fn raw_window(w: &Win) -> RawWindow {
    RawWindow {
        hwnd: u64::try_from(w.hwnd).unwrap_or(0),
        owner_pid: w.pid,
        owner_created: u64::try_from(w.created).ok(),
    }
}

/// One `State` as a scripted world: an unreadable foreground (`None`) is
/// the zero window here, so the fast path is not abandoned for it.
pub fn world(state: &State) -> WinWorld {
    WinWorld {
        boot: Ok(WinBoot {
            boot_id: Some(BOOT_ID),
            boot_time: Some(BOOT_TIME),
        }),
        processes: Ok(state.procs.iter().map(raw_process).collect()),
        windows: Ok(state.windows.iter().map(raw_window).collect()),
        foreground: Ok(state
            .foreground
            .filter(|w| w.hwnd != 0)
            .map(|w| raw_window(&w))),
    }
}

pub fn scripted(m: &Machine) -> ScriptedWin32 {
    let win = ScriptedWin32::new(m.states.iter().map(world).collect());
    let installed = m.states.first().is_none_or(|s| s.wt_installed);
    win.set_terminal(Ok(WinTerminalPackage {
        installed,
        version: installed.then(|| "1.21.2911.0".to_owned()),
    }));
    win.set_environment(Ok(user_env()));
    win
}

/// `m.build()` plus the scripted Win32 for its states.
pub fn fast(m: &Machine) -> FakeSystem {
    let mut fake = m.build();
    fake.set_win32(scripted(m));
    fake
}

pub fn win(fake: &FakeSystem) -> &ScriptedWin32 {
    fake.scripted_win32()
        .expect("the fake has a scripted Win32")
}

/// Every `powershell.exe` run recorded.
pub fn powershell_runs(fake: &FakeSystem) -> usize {
    fake.runs()
        .iter()
        .filter(|(p, _)| p.to_ascii_lowercase().ends_with("powershell.exe"))
        .count()
}

pub fn failed() -> Win32Error {
    Win32Error::new(5)
}
