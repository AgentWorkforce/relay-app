//! The observe report the PowerShell script prints, as Windows PowerShell
//! 5.1's `ConvertTo-Json` writes it, and its conversion into core facts.
//!
//! Shapes handled (fixtures in `tests/fixtures/windows/`):
//! - a `DateTime` is `"\/Date(<ms>)\/"` (optionally with a `+hhmm` suffix),
//!   or `{"value": "\/Date(<ms>)\/", "DateTime": "…"}` when it carries
//!   extended-type properties; a plain integer is accepted too;
//! - an array of one element may arrive as the bare element;
//! - `null` for anything the section could not read.
use std::collections::BTreeMap;

use serde::Deserialize;

use session_navigation::evidence::{Exe, Fact, ProcessInfo};
use session_navigation::hosts::{WindowId, WindowInfo};
use session_navigation::model::{App, BootId, ProcessIdentity};

use super::classify::classify_windows;
use super::quote;

const MAX_PROCESSES: usize = 50_000;

/// An array, or the single element PowerShell 5.1 emits in its place.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    Many(Vec<T>),
    One(T),
}

impl<T> OneOrMany<T> {
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Self::Many(v) => v,
            Self::One(t) => vec![t],
        }
    }
}

/// A `DateTime` in any of the forms above.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum DateValue {
    Seconds(i64),
    Text(String),
    Object { value: String },
}

/// `/Date(1731400000123)/` or `/Date(1731400000123+0100)/` → whole Unix
/// seconds (the milliseconds are UTC; the suffix is informational).
pub fn ms_date(text: &str) -> Option<u64> {
    let inner = text.strip_prefix("/Date(")?.strip_suffix(")/")?;
    let digits = match inner.find(['+', '-']) {
        Some(0) => return None,
        Some(i) => inner.get(..i)?,
        None => inner,
    };
    if digits.is_empty() || digits.len() > 16 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok().map(|ms| ms / 1000)
}

impl DateValue {
    pub fn seconds(&self) -> Option<u64> {
        match self {
            Self::Seconds(s) => u64::try_from(*s).ok(),
            Self::Text(t) | Self::Object { value: t } => ms_date(t),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RawProcess {
    pub process_id: u32,
    pub parent_process_id: u32,
    pub creation_date: Option<DateValue>,
    pub name: Option<String>,
    pub executable_path: Option<String>,
    pub command_line: Option<String>,
    pub session_id: Option<u32>,
}

/// A window and its owner as the C# helper saw them (`Created` is Unix
/// seconds, or -1 when the owner could not be opened).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RawWindow {
    pub handle: i64,
    pub process_id: i64,
    pub created: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RawTerminal {
    pub installed: bool,
    pub version: Option<String>,
}

/// `HKCU\Console\%%Startup`: the console and terminal handoff classes
/// ("Default terminal application" in Settings). `""` when not set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RawDelegation {
    #[serde(default)]
    pub console: Option<String>,
    #[serde(default)]
    pub terminal: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub boot_id: Option<i64>,
    pub boot_time: Option<DateValue>,
    pub processes: Option<OneOrMany<RawProcess>>,
    #[serde(default)]
    pub windows: Option<OneOrMany<RawWindow>>,
    #[serde(default)]
    pub foreground: Option<RawWindow>,
    #[serde(default)]
    pub terminal: Option<RawTerminal>,
    /// `null` (or absent) when the registry could not be read.
    #[serde(default)]
    pub delegation: Option<RawDelegation>,
}

impl Report {
    /// The boot id: the boot counter, else `LastBootUpTime`.
    pub fn boot(&self) -> Option<BootId> {
        match self.boot_id {
            Some(id) => u64::try_from(id).ok().map(BootId),
            None => self.boot_time().map(BootId),
        }
    }

    /// `LastBootUpTime` in Unix seconds (what file mtimes are compared to).
    pub fn boot_time(&self) -> Option<u64> {
        self.boot_time.as_ref().and_then(DateValue::seconds)
    }

    /// The whole table (needs a boot id), or `None`.
    pub fn table(&self) -> Option<Vec<ProcessInfo>> {
        let boot = self.boot()?;
        table(self.processes.clone()?.into_vec(), boot)
    }
}

/// `Win32_Process` rows → process table. A row without a creation time
/// (the idle pseudo-process) is skipped; a duplicated pid fails the table.
/// The path and command line of another user's or an elevated process are
/// `null`: the row stays (unreadable is not exited), classified from its
/// image name, with an empty argv.
pub fn table(rows: Vec<RawProcess>, boot: BootId) -> Option<Vec<ProcessInfo>> {
    if rows.len() > MAX_PROCESSES {
        return None;
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if !seen.insert(row.process_id) {
            return None;
        }
        let Some(start) = row.creation_date.as_ref().and_then(DateValue::seconds) else {
            continue;
        };
        let argv = row
            .command_line
            .as_deref()
            .map(quote::split)
            .unwrap_or_default();
        let image = row
            .executable_path
            .as_deref()
            .or(row.name.as_deref())
            .unwrap_or("");
        // Windows has no uids: both carry the Terminal Services session.
        let session = row.session_id.unwrap_or(u32::MAX);
        out.push(ProcessInfo {
            identity: ProcessIdentity {
                pid: row.process_id,
                start,
                boot,
            },
            ppid: row.parent_process_id,
            euid: session,
            ruid: session,
            exe: classify_windows(image, &argv),
            // Windows has no ttys.
            tty: None,
            argv,
            // Another process's working directory lives in its PEB; reading
            // it needs ReadProcessMemory, which this adapter does not do.
            cwd: Fact::Unknown,
            // `WINDOWID` is X11 only.
            window: None,
        });
    }
    (!out.is_empty()).then_some(out)
}

/// The owner a window's (pid, creation time) names in `table`: only an
/// exact match. A window whose owner was created after the table was read
/// (a reused pid) or could not be opened is not attributed to anyone.
/// UNVERIFIED: `Win32_Process.CreationDate` and `GetProcessTimes` floor to
/// the same second.
pub fn owner(table: &[ProcessInfo], raw: &RawWindow) -> Option<ProcessIdentity> {
    let pid = u32::try_from(raw.process_id).ok()?;
    let created = u64::try_from(raw.created).ok()?;
    table
        .iter()
        .find(|p| p.identity.pid == pid && p.identity.start == created)
        .map(|p| p.identity)
}

/// Every visible top-level window whose owner is exactly identified.
pub fn windows(table: &[ProcessInfo], raw: Vec<RawWindow>) -> Vec<WindowInfo> {
    raw.into_iter()
        .filter_map(|w| {
            let id = WindowId(u64::try_from(w.handle).ok().filter(|h| *h != 0)?);
            Some(WindowInfo {
                id,
                owner: owner(table, &w)?,
            })
        })
        .collect()
}

/// The application a window owner stands for: the terminal itself, or a
/// classic console's first client (which owns the console window).
pub fn app_of(table: &[ProcessInfo], owner: &ProcessIdentity) -> Option<App> {
    let process = table.iter().find(|p| p.identity == *owner)?;
    match process.exe {
        Exe::WindowsTerminal => Some(App::WindowsTerminal),
        Exe::ConsoleHost => Some(App::ConsoleHost),
        Exe::WezTerm => Some(App::WezTerm),
        Exe::Alacritty => Some(App::Alacritty),
        _ if table
            .iter()
            .any(|c| c.exe == Exe::ConsoleHost && c.ppid == owner.pid) =>
        {
            Some(App::ConsoleHost)
        }
        _ => None,
    }
}

/// (front app, front window) from the foreground report: no foreground
/// window (a locked screen) is `Known(None)` for both; an owner that is not
/// exactly in the table is an `Unknown` app.
pub fn front(
    table: Option<&[ProcessInfo]>,
    foreground: Option<&RawWindow>,
) -> (Fact<Option<App>>, Fact<Option<WindowId>>) {
    let Some(fg) = foreground else {
        return (Fact::Unknown, Fact::Unknown);
    };
    let Some(handle) = u64::try_from(fg.handle).ok() else {
        return (Fact::Unknown, Fact::Unknown);
    };
    if handle == 0 {
        return (Fact::Known(None), Fact::Known(None));
    }
    let window = Fact::Known(Some(WindowId(handle)));
    let app = match table.and_then(|t| owner(t, fg).map(|o| (t, o))) {
        Some((t, o)) => Fact::Known(app_of(t, &o)),
        None => Fact::Unknown,
    };
    (app, window)
}

/// Rows by pid, for tests and diagnostics.
pub fn by_pid(table: &[ProcessInfo]) -> BTreeMap<u32, &ProcessInfo> {
    table.iter().map(|p| (p.identity.pid, p)).collect()
}

/// Windows Terminal's handoff classes (console, terminal), stable and
/// Preview builds are separate packages and not modelled here.
/// UNVERIFIED: the GUIDs come from the Windows Terminal sources
/// (`DelegationConsole` {2EACA947-…} = OpenConsole handoff,
/// `DelegationTerminal` {E12CFF52-…} = terminal handoff); verify with
/// `Get-ItemProperty 'HKCU:\Console\%%Startup'` after choosing each option
/// in Settings > System > For developers > Terminal.
const WT_CLASSES: [&str; 2] = [
    "2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69",
    "E12CFF52-A866-4C77-9A90-F570A7AA2C6B",
];
/// "Windows Console Host": both values are this class.
const CONHOST_CLASS: &str = "B23D10C0-E52E-411E-9D5B-C09FDF709C7D";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handoff {
    /// Missing, empty or the all-zero GUID: "Let Windows decide".
    Unset,
    WindowsTerminal,
    ConsoleHost,
    Other,
}

fn handoff(value: Option<&str>) -> Handoff {
    let guid = value
        .unwrap_or("")
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}');
    if guid.is_empty() || guid.chars().all(|c| c == '0' || c == '-') {
        Handoff::Unset
    } else if WT_CLASSES.iter().any(|c| c.eq_ignore_ascii_case(guid)) {
        Handoff::WindowsTerminal
    } else if CONHOST_CLASS.eq_ignore_ascii_case(guid) {
        Handoff::ConsoleHost
    } else {
        Handoff::Other
    }
}

/// The default terminal application. "Let Windows decide" is `Known(None)`:
/// Windows 11 22H2+ then picks Windows Terminal when it is installed, but no
/// documented rule says so for every build, so it is not assumed. A third
/// party's or Preview's class, or a mix, is `Known(None)` too.
pub fn default_terminal(
    delegation: Option<&RawDelegation>,
) -> Fact<Option<session_navigation::model::TerminalKind>> {
    use session_navigation::model::TerminalKind;
    let Some(d) = delegation else {
        return Fact::Unknown;
    };
    let pair = (
        handoff(d.console.as_deref()),
        handoff(d.terminal.as_deref()),
    );
    Fact::Known(match pair {
        (Handoff::Unset, Handoff::Unset) => None,
        (Handoff::WindowsTerminal | Handoff::Unset, Handoff::WindowsTerminal | Handoff::Unset) => {
            Some(TerminalKind::WindowsTerminal)
        }
        (Handoff::ConsoleHost | Handoff::Unset, Handoff::ConsoleHost | Handoff::Unset) => {
            Some(TerminalKind::ConsoleHost)
        }
        _ => None,
    })
}
