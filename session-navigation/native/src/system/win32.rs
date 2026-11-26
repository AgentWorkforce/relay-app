//! The narrow Win32 seam the Windows adapter uses instead of PowerShell
//! when it is available ([`super::System::win32`]). Plain data in and out:
//! no handle, pointer or policy crosses it. [`super::RealSystem`] implements
//! it on Windows (`win32_real.rs`, the crate's only `unsafe`);
//! [`ScriptedWin32`](super::ScriptedWin32) replays a scripted machine for
//! tests on any OS.
//!
//! Every method either answers or fails with [`Win32Error`] *before*
//! changing anything, so a caller can always fall back to the slow path.

/// A failed Win32 call (the `GetLastError` / `NTSTATUS` value, for
/// diagnostics only). Nothing was changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Win32Error {
    pub code: u32,
}

impl Win32Error {
    pub const fn new(code: u32) -> Self {
        Self { code }
    }
}

/// What identifies this boot: the kernel's boot counter
/// (`PrefetchParameters\BootId`) when readable, and the boot time in whole
/// Unix seconds (UTC).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WinBoot {
    pub boot_id: Option<u64>,
    pub boot_time: Option<u64>,
}

/// One process-table row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawWinProcess {
    pub pid: u32,
    pub ppid: u32,
    /// Creation time in whole Unix seconds (UTC); `None` when the process
    /// could not be opened (the row is then not an identity).
    pub created: Option<u64>,
    /// The image file name (`szExeFile`).
    pub name: String,
    /// The full Win32 image path, when readable.
    pub image: Option<String>,
    /// The command line, when readable.
    pub command_line: Option<String>,
    /// The Terminal Services session, when readable.
    pub session: Option<u32>,
}

/// A visible, unowned top-level window and its owner process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawWindow {
    pub hwnd: u64,
    pub owner_pid: u32,
    /// The owner's creation time (whole Unix seconds), `None` when the
    /// owner could not be opened.
    pub owner_created: Option<u64>,
}

/// `HKCU\Console\%%Startup`: `""` for a missing key or value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WinDelegation {
    pub console: String,
    pub terminal: String,
}

/// Windows Terminal's package for this user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WinTerminalPackage {
    pub installed: bool,
    pub version: Option<String>,
}

/// Raise `hwnd` only if it still exists, is owned by exactly
/// (`owner_pid`, `owner_created`), and `process_pid` still started at
/// `process_created`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusRequest {
    pub hwnd: u64,
    pub owner_pid: u32,
    pub owner_created: u64,
    pub process_pid: u32,
    pub process_created: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusResult {
    /// The window, its owner or the process is not the one named.
    Gone,
    /// `SetForegroundWindow` returned false.
    Refused,
    /// It returned true but the foreground did not read back.
    Unconfirmed,
    Focused,
}

/// Which console the new process gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnConsole {
    /// `CREATE_NEW_CONSOLE`: a console program gets its own window.
    New,
    /// `CREATE_NO_WINDOW`: a launcher (`wt.exe`) that must not flash one.
    Hidden,
}

/// One `CreateProcessW`. `application` is an absolute path; `command_line`
/// is the complete, already quoted line (program word first);
/// `environment` is the whole block the child gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRequest {
    pub application: String,
    pub command_line: String,
    pub cwd: String,
    pub environment: Vec<(String, String)>,
    pub console: SpawnConsole,
}

/// The Win32 facts and acts the Windows adapter needs. `Send + Sync` so
/// observation can run it beside a WSL probe.
pub trait Win32: Send + Sync {
    fn boot(&self) -> Result<WinBoot, Win32Error>;
    /// The whole table. A row whose pid was reused between the snapshot and
    /// opening it is dropped (it was not in the snapshot).
    fn processes(&self) -> Result<Vec<RawWinProcess>, Win32Error>;
    fn windows(&self) -> Result<Vec<RawWindow>, Win32Error>;
    /// `None`: no foreground window (a locked screen).
    fn foreground(&self) -> Result<Option<RawWindow>, Win32Error>;
    /// Re-checks every identity at the moment of acting, restores a
    /// minimised window, raises it and reads the foreground back.
    fn focus(&self, request: &FocusRequest) -> Result<FocusResult, Win32Error>;
    fn default_terminal_delegation(&self) -> Result<WinDelegation, Win32Error>;
    fn terminal_package(&self) -> Result<WinTerminalPackage, Win32Error>;
    /// The person's fresh environment (machine, user and volatile
    /// variables), as `CreateEnvironmentBlock` builds it.
    fn user_environment(&self) -> Result<Vec<(String, String)>, Win32Error>;
    /// Start the process and do not wait for it.
    fn spawn(&self, request: &SpawnRequest) -> Result<(), Win32Error>;
}

/// FILETIME ticks (100 ns since 1601-01-01 UTC) → whole Unix seconds,
/// floored; `None` before 1970.
pub fn filetime_to_unix(ticks: u64) -> Option<u64> {
    (ticks / 10_000_000).checked_sub(11_644_473_600)
}

/// A UTF-16 buffer up to its first NUL.
pub fn utf16_until_nul(units: &[u16]) -> String {
    let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(units.get(..end).unwrap_or_default())
}

/// A double-NUL-terminated environment block → pairs. Entries without a
/// `=` after their first character are skipped; the drive entries
/// (`=C:=C:\…`) are kept as they are.
pub fn parse_environment_block(units: &[u16]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in units.split(|u| *u == 0) {
        if entry.is_empty() {
            break;
        }
        let text = String::from_utf16_lossy(entry);
        let split = text
            .char_indices()
            .skip(1)
            .find(|(_, c)| *c == '=')
            .map(|(i, _)| i);
        if let Some(i) = split {
            let (key, value) = text.split_at(i);
            out.push((key.to_owned(), value.get(1..).unwrap_or("").to_owned()));
        }
    }
    out
}

/// Pairs → a double-NUL-terminated block, sorted case-insensitively by
/// name. `None` when a name is empty, holds `=` after its first character,
/// or anything holds NUL.
pub fn environment_block(pairs: &[(String, String)]) -> Option<Vec<u16>> {
    let mut sorted: Vec<&(String, String)> = pairs.iter().collect();
    sorted.sort_by_key(|(k, _)| k.to_uppercase());
    let mut out = Vec::new();
    for (key, value) in sorted {
        let bad_key = key.is_empty() || key.chars().skip(1).any(|c| c == '=');
        if bad_key || key.contains('\0') || value.contains('\0') {
            return None;
        }
        out.extend(key.encode_utf16());
        out.push(u16::from(b'='));
        out.extend(value.encode_utf16());
        out.push(0);
    }
    if out.is_empty() {
        out.push(0);
    }
    out.push(0);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_floors_to_unix_seconds() {
        assert_eq!(filetime_to_unix(116_444_736_000_000_000), Some(0));
        assert_eq!(filetime_to_unix(116_444_736_019_999_999), Some(1));
        assert_eq!(filetime_to_unix(5), None);
    }

    #[test]
    fn environment_blocks_round_trip() {
        let pairs = vec![
            ("Path".to_owned(), r"C:\Windows".to_owned()),
            ("=C:".to_owned(), r"C:\".to_owned()),
            ("APPDATA".to_owned(), "x=y".to_owned()),
        ];
        let block = environment_block(&pairs).unwrap_or_default();
        let back = parse_environment_block(&block);
        assert_eq!(back.len(), 3);
        assert_eq!(back.first().map(|p| p.0.as_str()), Some("=C:"));
        assert!(back.contains(&("APPDATA".to_owned(), "x=y".to_owned())));
        assert_eq!(environment_block(&[("A=B".into(), "c".into())]), None);
        assert_eq!(environment_block(&[]), Some(vec![0, 0]));
    }
}
