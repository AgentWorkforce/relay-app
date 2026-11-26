//! The Win32 fast path: the same report, identity table, focus and launch
//! as the PowerShell scripts, built from [`crate::system::Win32`] calls in
//! a few milliseconds instead of a `powershell.exe` start and an `Add-Type`
//! compile. Plain logic only (no `cfg`, no `unsafe`): the data is converted
//! into the scripts' own [`Report`] shapes, so every rule downstream
//! (identity = pid + creation second + boot, owner matching, reused pids,
//! the default-terminal mapping) is the one the slow path uses.
//!
//! Every function answers `None` when a call fails before anything was
//! changed; the caller then runs the PowerShell script instead.
use session_navigation::evidence::ProcessInfo;
use session_navigation::hosts::WindowId;
use session_navigation::model::{BootId, ProcessIdentity, Reason, TerminalKind};
use session_navigation::plan::Dispatch;

use super::config::WindowsConfig;
use super::launch::LaunchSpec;
use super::quote::{self, MAX_COMMAND_LINE};
use super::report::{self, DateValue, OneOrMany, RawDelegation, RawProcess, RawTerminal, Report};
use crate::common::par::Reuse;
use crate::system::win32::{
    FocusRequest, FocusResult, RawWinProcess, RawWindow, SpawnConsole, SpawnRequest, Win32,
};

/// The native facts that cannot change within one click, reused for
/// [`crate::common::par::REUSE`] (2 s) within one observer: Windows Terminal's package and
/// the default-terminal registry values.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub terminal: Reuse<RawTerminal>,
    pub delegation: Reuse<RawDelegation>,
}

/// A Win32 row as the `Win32_Process` row the scripts print.
pub fn process_row(p: &RawWinProcess) -> RawProcess {
    RawProcess {
        process_id: p.pid,
        parent_process_id: p.ppid,
        creation_date: p
            .created
            .and_then(|c| i64::try_from(c).ok())
            .map(DateValue::Seconds),
        name: Some(p.name.clone()),
        executable_path: p.image.clone(),
        command_line: p.command_line.clone(),
        session_id: p.session,
    }
}

/// A Win32 window as the C# helper printed it (`Created` -1: unreadable).
pub fn window_row(w: &RawWindow) -> Option<report::RawWindow> {
    Some(report::RawWindow {
        handle: i64::try_from(w.hwnd).ok()?,
        process_id: i64::from(w.owner_pid),
        created: w
            .owner_created
            .and_then(|c| i64::try_from(c).ok())
            .unwrap_or(-1),
    })
}

fn boot_fields(win: &dyn Win32) -> Option<(Option<i64>, Option<DateValue>)> {
    let boot = win.boot().ok()?;
    Some((
        boot.boot_id.and_then(|id| i64::try_from(id).ok()),
        boot.boot_time
            .and_then(|t| i64::try_from(t).ok())
            .map(DateValue::Seconds),
    ))
}

fn processes(win: &dyn Win32) -> Option<OneOrMany<RawProcess>> {
    let rows = win.processes().ok()?;
    Some(OneOrMany::Many(rows.iter().map(process_row).collect()))
}

/// The observe report, read in the scripts' order (boot, table, windows,
/// foreground), or `None` when any read failed or the table is unusable.
pub fn report(win: &dyn Win32, facts: &mut Facts) -> Option<Report> {
    let (boot_id, boot_time) = boot_fields(win)?;
    let processes = processes(win)?;
    let windows: Option<Vec<report::RawWindow>> =
        win.windows().ok()?.iter().map(window_row).collect();
    let foreground = match win.foreground().ok()? {
        Some(w) => window_row(&w)?,
        // No foreground window: the helper's zero handle.
        None => report::RawWindow {
            handle: 0,
            process_id: 0,
            created: -1,
        },
    };
    let terminal = facts.terminal.get_or(|| {
        let t = win.terminal_package().ok()?;
        Some(RawTerminal {
            installed: t.installed,
            version: t.version,
        })
    })?;
    let delegation = facts.delegation.get_or(|| {
        let d = win.default_terminal_delegation().ok()?;
        Some(RawDelegation {
            console: Some(d.console),
            terminal: Some(d.terminal),
        })
    })?;
    let report = Report {
        boot_id,
        boot_time,
        processes: Some(processes),
        windows: Some(OneOrMany::Many(windows?)),
        foreground: Some(foreground),
        terminal: Some(terminal),
        delegation: Some(delegation),
    };
    report.table()?;
    Some(report)
}

/// A fresh boot and table (the executor's identity re-check).
pub fn identity(win: &dyn Win32) -> Option<(BootId, Vec<ProcessInfo>)> {
    let (boot_id, boot_time) = boot_fields(win)?;
    let report = Report {
        boot_id,
        boot_time,
        processes: Some(processes(win)?),
        windows: None,
        foreground: None,
        terminal: None,
        delegation: None,
    };
    Some((report.boot()?, report.table()?))
}

pub fn focus_request(
    hwnd: WindowId,
    owner: &ProcessIdentity,
    process: &ProcessIdentity,
) -> FocusRequest {
    FocusRequest {
        hwnd: hwnd.0,
        owner_pid: owner.pid,
        owner_created: owner.start,
        process_pid: process.pid,
        process_created: process.start,
    }
}

/// The focus script's statuses, the same mapping.
pub fn focus_dispatch(result: FocusResult) -> Dispatch {
    match result {
        FocusResult::Focused => Dispatch::Dispatched,
        FocusResult::Gone => Dispatch::Failed(Reason::TargetGone),
        FocusResult::Refused | FocusResult::Unconfirmed => Dispatch::Failed(Reason::DispatchFailed),
    }
}

/// `base` with `overrides` set on top (names compare case-insensitively,
/// as Windows does).
pub fn merge_environment(
    base: Vec<(String, String)>,
    overrides: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = base
        .into_iter()
        .filter(|(k, _)| !overrides.keys().any(|o| o.eq_ignore_ascii_case(k)))
        .collect();
    out.extend(overrides.iter().map(|(k, v)| (k.clone(), v.clone())));
    out
}

/// The `CreateProcessW` a validated launch spec becomes: what
/// `Start-Process` did, without PowerShell. A `.cmd`/`.bat` program (whose
/// words [`super::launch`] already proved `cmd.exe`-safe) runs through
/// `cmd.exe /d /s /c "<line>"`, which is what starting a batch file means;
/// everything else is started directly. `None`: the line cannot be built.
pub fn spawn_request(
    cfg: &WindowsConfig,
    terminal: TerminalKind,
    spec: &LaunchSpec,
    environment: Vec<(String, String)>,
) -> Option<SpawnRequest> {
    let program = quote::quote_program(&spec.file).ok()?;
    let line = if spec.line.is_empty() {
        program
    } else {
        format!("{program} {}", spec.line)
    };
    let (application, command_line) = if quote::is_batch(&spec.file) {
        let cmd = cfg.system32("cmd.exe");
        let quoted = quote::quote_program(&cmd).ok()?;
        (cmd, format!("{quoted} /d /s /c \"{line}\""))
    } else {
        (spec.file.clone(), line)
    };
    if command_line.encode_utf16().count() > MAX_COMMAND_LINE {
        return None;
    }
    Some(SpawnRequest {
        application,
        command_line,
        cwd: spec.cwd.clone(),
        environment: merge_environment(environment, &spec.env),
        console: match terminal {
            // `wt.exe` hands the tab to Windows Terminal; it must not flash
            // a console of its own.
            TerminalKind::WindowsTerminal => SpawnConsole::Hidden,
            _ => SpawnConsole::New,
        },
    })
}

/// Start a validated launch directly. `None`: nothing was started (the
/// environment or the process could not be created), so the caller may use
/// the script; a line that cannot be built is refused outright.
pub fn launch(
    win: &dyn Win32,
    cfg: &WindowsConfig,
    terminal: TerminalKind,
    spec: &LaunchSpec,
) -> Option<Dispatch> {
    let environment = win.user_environment().ok()?;
    let Some(request) = spawn_request(cfg, terminal, spec, environment) else {
        return Some(Dispatch::Failed(Reason::DispatchFailed));
    };
    win.spawn(&request).ok()?;
    Some(Dispatch::Dispatched)
}
