//! Machine-wide probes shared by the observer and the executor: boot, the
//! process table, process cwds, the app registry, the front pane, tools.
use std::time::Duration;

use session_navigation::evidence::{Fact, ProcessInfo};
use session_navigation::model::{App, BootId, Platform, TerminalKind};

use super::config::MacConfig;
use super::kernel::{self, KernelTable};
use super::parse::{self, ps, AppsReport};
use super::scripts;
use crate::common::parse::lsof_cwds;
use crate::common::probe;
use crate::system::{Run, System};

pub const SYSCTL: &str = "/usr/sbin/sysctl";
pub const PS: &str = "/bin/ps";
pub const LSOF: &str = "/usr/sbin/lsof";
pub const OSASCRIPT: &str = "/usr/bin/osascript";
pub const SQLITE: &str = "/usr/bin/sqlite3";
pub const CURL: &str = "/usr/bin/curl";

/// `kern.boottime` seconds: the boot id. Read from the kernel when the
/// system can, else from `sysctl`.
pub fn boot<S: System>(sys: &S, cfg: &MacConfig) -> Option<u64> {
    if let Some(sec) = sys.kernel().and_then(|k| k.boot_time()) {
        return Some(sec);
    }
    let run = Run::new(SYSCTL, &["-n", "kern.boottime"]).timeout(cfg.common.timeouts.probe);
    parse::boot_time(&probe(sys, &run)?.stdout)
}

fn ps_run(columns: &str, timeout: Duration) -> Run {
    let mut run = Run::new(PS, &["-axww", "-o", columns])
        .timeout(timeout)
        .max_output(16 * 1024 * 1024);
    // `lstart` in UTC, the Claude registry's `procStart` format; C locale
    // for English day and month names.
    run.env = vec![
        ("TZ".to_owned(), "UTC".to_owned()),
        ("LC_ALL".to_owned(), "C".to_owned()),
    ];
    run
}

/// The exact `ps` runs, in order (identity, comm, args).
pub fn ps_runs(cfg: &MacConfig) -> [Run; 3] {
    let t = cfg.common.timeouts.probe;
    [
        ps_run(ps::PS_IDENTITY, t),
        ps_run(ps::PS_COMM, t),
        ps_run(ps::PS_ARGS, t),
    ]
}

/// The whole table or nothing (cwds not yet filled): from the kernel when
/// the system can read it, else from the three `ps` runs.
pub fn table<S: System>(sys: &S, cfg: &MacConfig, boot: BootId) -> Option<Vec<ProcessInfo>> {
    full_table(sys, cfg, boot).map(|t| t.processes)
}

/// [`table`], with the executables the kernel saw (empty from `ps`).
pub fn full_table<S: System>(sys: &S, cfg: &MacConfig, boot: BootId) -> Option<KernelTable> {
    if let Some(table) = sys.kernel().and_then(|k| kernel::table(k, boot)) {
        return Some(table);
    }
    ps_table(sys, cfg, boot).map(|processes| KernelTable {
        processes,
        executables: Default::default(),
        from_kernel: false,
    })
}

/// The table from the three `ps` runs.
pub fn ps_table<S: System>(sys: &S, cfg: &MacConfig, boot: BootId) -> Option<Vec<ProcessInfo>> {
    let [identity, comm, args] = ps_runs(cfg);
    let identity = probe(sys, &identity)?;
    let comm = probe(sys, &comm)?;
    let args = probe(sys, &args)?;
    ps::join_table(&identity.stdout, &comm.stdout, &args.stdout, boot)
}

/// Fill `cwd` for agent CLIs with one `lsof` call. lsof exits 1 when some
/// pid had no readable cwd (a root-owned process, one that just exited); its
/// output still counts for the others. Anything else leaves them `Unknown`.
///
/// With kernel reads, each agent CLI's cwd is read directly instead.
pub fn fill_cwds<S: System>(sys: &S, cfg: &MacConfig, table: &mut [ProcessInfo]) {
    if let Some(kernel) = sys.kernel() {
        kernel::fill_cwds(kernel, table);
        return;
    }
    lsof_cwds_into(sys, cfg, table);
}

/// [`fill_cwds`] through one `lsof` run.
pub fn lsof_cwds_into<S: System>(sys: &S, cfg: &MacConfig, table: &mut [ProcessInfo]) {
    let pids: Vec<String> = table
        .iter()
        .filter(|p| ps::is_agent_cli(p.exe))
        .map(|p| p.identity.pid.to_string())
        .collect();
    if pids.is_empty() {
        return;
    }
    let run = Run::with_args(
        &cfg.common.programs.lsof,
        vec![
            "-a".into(),
            "-d".into(),
            "cwd".into(),
            "-Fpn".into(),
            "-p".into(),
            pids.join(","),
        ],
    )
    .timeout(cfg.common.timeouts.probe);
    let Ok(output) = sys.run(&run) else {
        return;
    };
    if !matches!(output.status, Some(0 | 1)) {
        return;
    }
    let Some(cwds) = lsof_cwds(&output.stdout) else {
        return;
    };
    for process in table.iter_mut().filter(|p| ps::is_agent_cli(p.exe)) {
        if let Some(cwd) = cwds.get(&process.identity.pid) {
            process.cwd = Fact::Known(cwd.clone());
        }
    }
}

/// The apps that exist on macOS, in `App::ALL` order.
pub fn mac_apps() -> Vec<App> {
    App::ALL
        .into_iter()
        .filter(|a| a.handler(Platform::MacOs).is_some())
        .collect()
}

/// The bundles the registry query asks about, in `App::ALL` order.
pub fn app_bundles() -> Vec<&'static str> {
    mac_apps().into_iter().map(App::bundle_id).collect()
}

pub fn apps_run(cfg: &MacConfig) -> Run {
    let mut args = vec!["-l".to_owned(), "JavaScript".to_owned(), "-e".to_owned()];
    args.push(scripts::APPS_JXA.to_owned());
    args.extend(app_bundles().into_iter().map(str::to_owned));
    Run::with_args(OSASCRIPT, args).timeout(cfg.common.timeouts.apps)
}

pub fn apps<S: System>(sys: &S, cfg: &MacConfig) -> Option<AppsReport> {
    let output = probe(sys, &apps_run(cfg))?;
    parse::apps_report(&output.stdout, &app_bundles())
}

/// An AppleScript run: `osascript -e <constant> <argv…>`.
pub fn script_run(script: &str, argv: &[String], timeout: Duration) -> Run {
    let mut args = vec!["-e".to_owned(), script.to_owned()];
    args.extend(argv.iter().cloned());
    Run::with_args(OSASCRIPT, args).timeout(timeout)
}

/// A `/dev/ttysNNN` path, the only tty shape the adapters pass around.
pub fn is_tty(text: &str) -> bool {
    text.strip_prefix("/dev/ttys")
        .is_some_and(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// What reading a scripted terminal's front pane found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontRead {
    pub tty: Fact<Option<String>>,
    /// The script was refused with -1743: Automation is not allowed for
    /// this terminal.
    pub denied: bool,
}

/// The front-tty script of a tty-scripted terminal.
pub fn front_script(terminal: TerminalKind) -> Option<&'static str> {
    match terminal {
        TerminalKind::Ghostty => Some(scripts::GHOSTTY_FRONT_TTY),
        TerminalKind::AppleTerminal => Some(scripts::TERMINAL_FRONT_TTY),
        TerminalKind::ITerm2 => Some(scripts::ITERM_FRONT_TTY),
        _ => None,
    }
}

/// The focused pane of a frontmost terminal. Automation refused (-1743) or
/// any failure is `Unknown`; so is every terminal without a script here.
pub fn front_pane<S: System>(
    sys: &S,
    cfg: &MacConfig,
    terminal: TerminalKind,
) -> Fact<Option<String>> {
    front_pane_read(sys, cfg, terminal).tty
}

/// [`front_pane`], also reporting an Automation refusal.
pub fn front_pane_read<S: System>(sys: &S, cfg: &MacConfig, terminal: TerminalKind) -> FrontRead {
    let unknown = FrontRead {
        tty: Fact::Unknown,
        denied: false,
    };
    let Some(script) = front_script(terminal) else {
        return unknown;
    };
    let run = script_run(script, &[], cfg.common.timeouts.script_read);
    let Ok(output) = sys.run(&run) else {
        return unknown;
    };
    if !output.success() {
        return FrontRead {
            tty: Fact::Unknown,
            denied: parse::osascript_error(&output.stderr) == Some(scripts::NOT_AUTHORIZED),
        };
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    FrontRead {
        tty: if is_tty(&text) {
            Fact::Known(Some(text))
        } else {
            Fact::Unknown
        },
        denied: false,
    }
}

/// `<app path>/Contents/MacOS/<name>` for an installed app the registry
/// query found (`kitty`, `wezterm`, …).
pub fn app_executable(report: &AppsReport, app: App, name: &str) -> Option<String> {
    let entry = report.apps.get(app.bundle_id())?.as_ref()?;
    let ok =
        session_navigation::parse::is_absolute_path(&entry.path) && entry.path.ends_with(".app");
    ok.then(|| format!("{}/Contents/MacOS/{name}", entry.path.trim_end_matches('/')))
}
