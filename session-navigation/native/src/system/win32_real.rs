//! The real [`Win32`]: direct calls through `windows-sys`. This module
//! (this file and `win32_real/{base,read,act}.rs`, split only to stay under
//! 500 lines each) holds the Windows adapter's only `unsafe` and the
//! crate's only `windows-sys` use; elsewhere [`real`] is `None` and the
//! Windows adapter keeps its PowerShell path.
//!
//! Rules kept in every call below: each `unsafe` block has a `SAFETY:`
//! note; every handle is owned by a [`Handle`] (or [`EnvBlock`]) guard and
//! closed exactly once; every out-buffer is sized by us and its returned
//! length is checked against that size before it is read; strings are read
//! by copying UTF-16 units out of our own buffers (no pointer from the OS
//! is dereferenced except the environment block, which is bounded); no
//! transmute. UNVERIFIED: never run on Windows (type-checked only, for
//! `x86_64-pc-windows-msvc`).
use super::win32::Win32;

/// The machine's Win32, when this build runs on Windows.
#[cfg(windows)]
pub fn real() -> Option<&'static dyn Win32> {
    Some(&imp::RealWin32)
}

/// The machine's Win32, when this build runs on Windows.
#[cfg(not(windows))]
pub fn real() -> Option<&'static dyn Win32> {
    None
}

#[cfg(windows)]
mod act;
#[cfg(windows)]
mod base;
#[cfg(windows)]
mod read;

#[cfg(windows)]
mod imp {
    use super::{act, read};
    use crate::system::win32::{
        FocusRequest, FocusResult, RawWinProcess, RawWindow, SpawnRequest, Win32, Win32Error,
        WinBoot, WinDelegation, WinTerminalPackage,
    };

    pub struct RealWin32;

    impl Win32 for RealWin32 {
        fn boot(&self) -> Result<WinBoot, Win32Error> {
            read::boot()
        }

        fn processes(&self) -> Result<Vec<RawWinProcess>, Win32Error> {
            read::processes()
        }

        fn windows(&self) -> Result<Vec<RawWindow>, Win32Error> {
            read::windows()
        }

        fn foreground(&self) -> Result<Option<RawWindow>, Win32Error> {
            read::foreground()
        }

        fn focus(&self, request: &FocusRequest) -> Result<FocusResult, Win32Error> {
            act::focus(request)
        }

        fn default_terminal_delegation(&self) -> Result<WinDelegation, Win32Error> {
            read::default_terminal_delegation()
        }

        fn terminal_package(&self) -> Result<WinTerminalPackage, Win32Error> {
            read::terminal_package()
        }

        fn user_environment(&self) -> Result<Vec<(String, String)>, Win32Error> {
            act::user_environment()
        }

        fn spawn(&self, request: &SpawnRequest) -> Result<(), Win32Error> {
            act::spawn(request)
        }
    }
}
