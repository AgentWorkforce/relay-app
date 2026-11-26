//! Opt-in, read-only acceptance on a real Windows machine:
//!
//! ```text
//! set SESSION_NAVIGATION_NATIVE=1
//! cargo test -p session-navigation-native --no-default-features --features windows --test windows_native_acceptance -- --nocapture
//! ```
//!
//! It runs nothing that acts: PowerShell's own parser over every constant
//! script, then one real observe, checking that the table holds this test's
//! own process with a sane creation time, and that the window list and the
//! foreground were read; then the Win32 fast path against the PowerShell
//! path (boot, and pid / ppid / creation second / image of every process
//! both saw), this process in the Win32 table, and one timed observe on
//! each path (printed). Anywhere else (this crate is developed on a Mac),
//! or without the variable, every test returns at once.
#![cfg(feature = "windows")]

use session_navigation::evidence::Fact;
use session_navigation::model::{Platform, Provider, Runtime, SessionRef};
use session_navigation::ports::Observe;
use session_navigation_native::system::{
    FsError, Metadata, Output, RealSystem, Run, RunError, System,
};
use session_navigation_native::windows::{
    fast, observe, powershell, scripts, WindowsConfig, WindowsObserver,
};

fn enabled() -> bool {
    cfg!(windows) && std::env::var("SESSION_NAVIGATION_NATIVE").as_deref() == Ok("1")
}

/// Parses each script given on stdin (a JSON array of strings) with
/// PowerShell's parser and prints the number of errors in each.
const PARSE_CHECK: &str = "$ErrorActionPreference = 'Stop'\n\
$scripts = [Console]::In.ReadToEnd() | ConvertFrom-Json\n\
$counts = @(foreach ($s in $scripts) { $t = $null; $e = $null; [void][System.Management.Automation.Language.Parser]::ParseInput([string]$s, [ref]$t, [ref]$e); $e.Count })\n\
ConvertTo-Json -InputObject $counts -Compress\n";

#[test]
fn every_powershell_script_parses() {
    if !enabled() {
        return;
    }
    let cfg = WindowsConfig::from_env().expect("SystemRoot, USERPROFILE, APPDATA, LOCALAPPDATA");
    let wrapper = format!("$payload='e30='\n{}", scripts::ENV_WRAPPER);
    let all = [
        scripts::OBSERVE,
        scripts::IDENTITY,
        scripts::FOCUS,
        scripts::LAUNCH,
        wrapper.as_str(),
    ];
    let input = serde_json::json!(all);
    let run = powershell::run(&cfg, PARSE_CHECK, Some(&input), cfg.script_timeout);
    let output: Output = RealSystem.run(&run).expect("powershell runs");
    assert!(
        output.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = powershell::decode_text(&output.stdout).expect("text");
    let counts: Vec<u32> = serde_json::from_str(text.trim()).expect("counts");
    assert_eq!(counts, vec![0; all.len()], "parse errors per script");
}

#[test]
fn a_read_only_observe_sees_this_process() {
    if !enabled() {
        return;
    }
    let cfg = WindowsConfig::from_env().expect("environment");
    let now = RealSystem.now();
    let mut observer = WindowsObserver::new(RealSystem, cfg);
    let session = SessionRef::new(Provider::Claude, "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee");
    let snap = observer.observe(&session);
    println!(
        "{}",
        serde_json::to_string_pretty(&snap.surfaces).unwrap_or_default()
    );
    assert_eq!(snap.platform, Platform::Windows);
    assert_eq!(snap.runtime, Fact::Known(Runtime::Native));
    assert!(snap.boot.is_known(), "boot id");
    let table = snap.processes.known().expect("process table");
    let me = table
        .iter()
        .find(|p| p.identity.pid == std::process::id())
        .expect("this test's own process");
    assert!(
        me.identity.start <= now && me.identity.start + 3600 > now,
        "creation time {} vs now {now}",
        me.identity.start
    );
    assert!(table.iter().all(|p| p.tty.is_none()));
    assert!(snap.surfaces.windows.is_known(), "window list");
    assert!(snap.front.window.is_known(), "foreground window");
    assert!(snap.apps.is_known(), "Windows Terminal package query");
}

/// The real machine without its Win32: the PowerShell path only.
struct ScriptsOnly;

impl System for ScriptsOnly {
    fn run(&self, run: &Run) -> Result<Output, RunError> {
        RealSystem.run(run)
    }
    fn read_file(&self, path: &str, cap: usize) -> Result<Vec<u8>, FsError> {
        RealSystem.read_file(path, cap)
    }
    fn read_prefix(&self, path: &str, len: usize) -> Result<Vec<u8>, FsError> {
        RealSystem.read_prefix(path, len)
    }
    fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError> {
        RealSystem.list_dir(path)
    }
    fn metadata(&self, path: &str) -> Result<Metadata, FsError> {
        RealSystem.metadata(path)
    }
    fn canonicalize(&self, path: &str) -> Result<String, FsError> {
        RealSystem.canonicalize(path)
    }
    fn create_private_dir(&self, path: &str) -> Result<(), FsError> {
        RealSystem.create_private_dir(path)
    }
    fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        RealSystem.write_atomic(path, bytes)
    }
    fn remove_file(&self, path: &str) -> Result<(), FsError> {
        RealSystem.remove_file(path)
    }
    fn now(&self) -> u64 {
        RealSystem.now()
    }
}

#[test]
fn the_win32_table_agrees_with_powershell() {
    if !enabled() {
        return;
    }
    let cfg = WindowsConfig::from_env().expect("environment");
    let win = RealSystem.win32().expect("RealSystem has Win32 on Windows");
    let slow = observe::report(&ScriptsOnly, &cfg).expect("the PowerShell report");
    let mut facts = fast::Facts::default();
    let quick = fast::report(win, &mut facts).expect("the Win32 report");
    assert_eq!(quick.boot(), slow.boot(), "boot id");
    println!(
        "boot time: win32 {:?}, powershell {:?}",
        quick.boot_time(),
        slow.boot_time()
    );
    let slow_table = slow.table().expect("PowerShell table");
    let quick_table = quick.table().expect("Win32 table");
    let rows = |r: &session_navigation_native::windows::report::Report| -> Vec<session_navigation_native::windows::report::RawProcess> {
        r.processes.clone().map(|p| p.into_vec()).unwrap_or_default()
    };
    let (slow_rows, quick_rows) = (rows(&slow), rows(&quick));
    let (mut same, mut reused, mut bad) = (0, 0, Vec::new());
    for q in &quick_rows {
        let Some(s) = slow_rows.iter().find(|s| s.process_id == q.process_id) else {
            continue;
        };
        let (qc, sc) = (
            q.creation_date.as_ref().and_then(|d| d.seconds()),
            s.creation_date.as_ref().and_then(|d| d.seconds()),
        );
        if qc.is_none() || sc.is_none() {
            continue;
        }
        if qc != sc {
            reused += 1;
            if qc.zip(sc).is_some_and(|(a, b)| a.abs_diff(b) <= 1) {
                bad.push(format!("pid {}: creation {qc:?} vs {sc:?}", q.process_id));
            }
            continue;
        }
        same += 1;
        if q.parent_process_id != s.parent_process_id {
            bad.push(format!(
                "pid {}: ppid {} vs {}",
                q.process_id, q.parent_process_id, s.parent_process_id
            ));
        }
        let lower = |p: &Option<String>| p.as_deref().map(str::to_ascii_lowercase);
        if s.executable_path.is_some() && lower(&q.executable_path) != lower(&s.executable_path) {
            bad.push(format!(
                "pid {}: exe {:?} vs {:?}",
                q.process_id, q.executable_path, s.executable_path
            ));
        }
    }
    println!(
        "rows: win32 {} (table {}), powershell {} (table {}); agreeing {same}, pid reused between reads {reused}",
        quick_rows.len(),
        quick_table.len(),
        slow_rows.len(),
        slow_table.len()
    );
    assert!(bad.is_empty(), "disagreements:\n{}", bad.join("\n"));
    assert!(same * 10 >= slow_table.len() * 9, "too few rows agree");
    assert!(
        reused <= 3,
        "{reused} pids changed creation between two reads"
    );
}

#[test]
fn the_win32_table_holds_this_process() {
    if !enabled() {
        return;
    }
    let win = RealSystem.win32().expect("Win32");
    let rows = win.processes().expect("table");
    let me = rows
        .iter()
        .find(|p| p.pid == std::process::id())
        .expect("this process");
    let now = RealSystem.now();
    let created = me.created.expect("own creation time");
    assert!(created <= now && created + 3600 > now, "{created} vs {now}");
    let exe = std::env::current_exe().expect("current exe");
    let image = me.image.clone().expect("own image path");
    assert!(
        image.eq_ignore_ascii_case(&exe.to_string_lossy()),
        "{image} vs {exe:?}"
    );
    let line = me.command_line.clone().expect("own command line");
    assert!(
        line.to_ascii_lowercase()
            .contains("windows_native_acceptance"),
        "{line}"
    );
}

#[test]
fn one_observe_on_each_path_is_timed() {
    if !enabled() {
        return;
    }
    let cfg = WindowsConfig::from_env().expect("environment");
    let session = SessionRef::new(Provider::Claude, "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee");
    let started = std::time::Instant::now();
    let slow = WindowsObserver::new(ScriptsOnly, cfg.clone()).observe(&session);
    let slow_time = started.elapsed();
    let started = std::time::Instant::now();
    let quick = WindowsObserver::new(RealSystem, cfg).observe(&session);
    let quick_time = started.elapsed();
    println!("observe: powershell {slow_time:?}, win32 {quick_time:?}");
    assert!(slow.processes.is_known() && quick.processes.is_known());
    assert_eq!(slow.boot, quick.boot);
    assert_eq!(slow.default_terminal, quick.default_terminal);
    assert_eq!(slow.apps, quick.apps);
}
