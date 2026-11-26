//! A WSL distribution as a runtime: its own process table, its tools as
//! POSIX paths, and its file system through the distribution's share.
//!
//! - `wsl.exe -l -q` (installed distributions) prints UTF-16LE; the probes
//!   below print the Linux programs' own UTF-8 bytes.
//! - Scripts go to `sh -s` on stdin, so no script text crosses the Windows
//!   command line; only validated words do.
//! - Reading the table starts the distribution's VM when it is not running
//!   (as reading its files through the share does). UNVERIFIED: `wsl.exe`
//!   forwards stdin to `--exec` programs, and splits its own command line
//!   by the MSVC CRT rules.
//! - A process's start is `btime + starttime / CLK_TCK`, in whole seconds.
//!   Linux recomputes `btime` on each read, so a clock step (a host resume)
//!   moves every start: identities then stop matching, which refuses
//!   (never misdirects) a focus or stop.
use std::time::Duration;

use session_navigation::evidence::{Fact, ProcessInfo, ToolInfo};
use session_navigation::model::{BootId, ProcessIdentity, Tool};

use super::classify::{classify_posix, is_agent_cli};
use super::config::{plain_posix_path, WindowsConfig};
use super::powershell::decode_text;
use crate::common::parse;
use crate::system::{Output, Run, Secret, System};

const MAX_ROWS: usize = 20_000;

/// `wsl.exe -l -q`.
pub fn list_run(cfg: &WindowsConfig) -> Run {
    Run::new(&cfg.wsl_exe(), &["-l", "-q"]).timeout(cfg.wsl_timeout)
}

/// Installed distribution names from `wsl.exe -l -q` (UTF-16LE, CRLF).
pub fn parse_list(bytes: &[u8]) -> Option<Vec<String>> {
    let text = decode_text(bytes)?;
    Some(
        text.lines()
            .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// `wsl.exe -d <distro> --exec /bin/sh -s -- <words…>` with `script` on stdin.
pub fn sh_run(cfg: &WindowsConfig, distro: &str, script: &str, words: &[String]) -> Run {
    let mut args: Vec<String> = ["-d", distro, "--exec", "/bin/sh", "-s", "--"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    args.extend(words.iter().cloned());
    Run::with_args(&cfg.wsl_exe(), args)
        .stdin(Secret::new(script.as_bytes().to_vec()))
        .timeout(cfg.wsl_timeout)
        .max_output(32 * 1024 * 1024)
}

pub fn table_run(cfg: &WindowsConfig, distro: &str) -> Run {
    sh_run(cfg, distro, super::scripts::WSL_TABLE, &[])
}

/// The distribution's table as one read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslTable {
    pub boot: BootId,
    /// `btime`: what file mtimes inside the distribution compare to.
    pub boot_time: u64,
    /// The uid the probe ran as (the person's default WSL user).
    pub uid: u32,
    pub processes: Vec<ProcessInfo>,
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text == "-" {
        return Some(Vec::new());
    }
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16);
    text.as_bytes()
        .chunks(2)
        .map(|p| match p {
            [hi, lo] => Some(u8::try_from(digit(*hi)? * 16 + digit(*lo)?).ok()?),
            _ => None,
        })
        .collect()
}

fn utf8(bytes: Vec<u8>) -> Option<String> {
    String::from_utf8(bytes).ok()
}

/// `boot_id` (a UUID) → its first 64 bits.
fn boot_of(uuid: &str) -> Option<BootId> {
    let hex: String = uuid.chars().filter(|c| *c != '-').collect();
    let head = hex.get(..16)?;
    u64::from_str_radix(head, 16).ok().map(BootId)
}

/// `tty_nr` → `/dev/pts/N` or `/dev/ttyN`; anything else has none.
pub fn tty_of(tty_nr: u64) -> Option<String> {
    let major = (tty_nr >> 8) & 0xfff;
    let minor = (tty_nr & 0xff) | ((tty_nr >> 12) & 0xfff00);
    match major {
        136..=143 => Some(format!("/dev/pts/{}", (major - 136) * 256 + minor)),
        4 if minor < 64 => Some(format!("/dev/tty{minor}")),
        _ => None,
    }
}

struct Stat {
    comm: String,
    ppid: u32,
    tty_nr: u64,
    start_ticks: u64,
}

fn parse_stat(text: &str) -> Option<Stat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_owned();
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    Some(Stat {
        comm,
        ppid: rest.get(1)?.parse().ok()?,
        tty_nr: rest.get(4)?.parse().ok()?,
        start_ticks: rest.get(19)?.parse().ok()?,
    })
}

fn header<'a>(lines: &[&'a str], key: &str) -> Option<&'a str> {
    lines
        .iter()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(' ')))
        .map(str::trim)
}

/// The probe's output → the table. Any malformed `proc` line fails the
/// whole table (as the macOS `ps` join does).
pub fn parse_table(bytes: &[u8]) -> Option<WslTable> {
    let text = std::str::from_utf8(bytes).ok()?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() > MAX_ROWS {
        return None;
    }
    let boot = boot_of(header(&lines, "boot")?)?;
    let btime: u64 = header(&lines, "btime")?.parse().ok()?;
    let hz: u64 = header(&lines, "hz")?.parse().ok().filter(|h| *h > 0)?;
    let uid: u32 = header(&lines, "uid")?.parse().ok()?;
    let mut processes = Vec::new();
    for line in lines.iter().filter_map(|l| l.strip_prefix("proc ")) {
        let f: Vec<&str> = line.split(' ').collect();
        let [pid, ruid, euid, stat, exe, cwd, cmd] = f.as_slice() else {
            return None;
        };
        let stat = parse_stat(&utf8(unhex(stat)?)?)?;
        let exe = utf8(unhex(exe)?)?;
        let cwd = utf8(unhex(cwd)?)?;
        let argv: Vec<String> = String::from_utf8_lossy(&unhex(cmd)?)
            .split('\0')
            .filter(|a| !a.is_empty())
            .map(str::to_owned)
            .collect();
        let image = if exe.is_empty() { &stat.comm } else { &exe };
        let class = classify_posix(image, &argv);
        processes.push(ProcessInfo {
            identity: ProcessIdentity {
                pid: pid.parse().ok()?,
                start: btime + stat.start_ticks / hz,
                boot,
            },
            ppid: stat.ppid,
            euid: euid.parse().ok()?,
            ruid: ruid.parse().ok()?,
            exe: class,
            tty: tty_of(stat.tty_nr),
            argv,
            cwd: if is_agent_cli(class) && !cwd.is_empty() {
                Fact::Known(cwd)
            } else {
                Fact::Unknown
            },
            // `WINDOWID` belongs to X11; WSLg windows are not tracked.
            window: None,
        });
    }
    (!processes.is_empty()).then_some(WslTable {
        boot,
        boot_time: btime,
        uid,
        processes,
    })
}

/// Read the table, or `None` when `wsl.exe` or the probe failed.
pub fn table<S: System>(sys: &S, cfg: &WindowsConfig, distro: &str) -> Option<WslTable> {
    let output = sys
        .run(&table_run(cfg, distro))
        .ok()
        .filter(Output::success)?;
    parse_table(&output.stdout)
}

/// The tools probe: the configured POSIX directories, then the tool names.
pub fn tools_run(cfg: &WindowsConfig, distro: &str) -> Option<Run> {
    let dirs = &cfg.common.paths.tool_search_path;
    if !dirs.iter().all(|d| plain_posix_path(d)) {
        return None;
    }
    let mut words: Vec<String> = dirs.clone();
    words.push("--".to_owned());
    words.extend(Tool::ALL.iter().map(|t| t.program().to_owned()));
    Some(sh_run(cfg, distro, super::scripts::WSL_TOOLS, &words))
}

/// Every tool with an `Unknown` version (no probe answered).
pub fn unknown_tools() -> Vec<ToolInfo> {
    Tool::ALL
        .into_iter()
        .map(|tool| ToolInfo {
            tool,
            version: Fact::Unknown,
            path: None,
        })
        .collect()
}

/// Tools inside the distribution, in [`Tool::ALL`] order. A failed probe
/// leaves every version `Unknown`.
pub fn parse_tools(bytes: &[u8]) -> Option<Vec<ToolInfo>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut out = Vec::new();
    for tool in Tool::ALL {
        let line = text.lines().find_map(|l| {
            let rest = l.strip_prefix("tool ")?;
            let (name, rest) = rest.split_once(' ').unwrap_or((rest, ""));
            (name == tool.program()).then_some(rest)
        })?;
        let mut words = line.split(' ');
        let path = words.next().unwrap_or("-");
        if path == "-" {
            out.push(ToolInfo {
                tool,
                version: Fact::Known(None),
                path: None,
            });
            continue;
        }
        let path = utf8(unhex(path)?)?;
        if !session_navigation::parse::is_absolute_path(&path) {
            return None;
        }
        let raw = unhex(words.next().unwrap_or("-"))?;
        let version = if tool == Tool::GrokCli {
            utf8(raw).and_then(|n| parse::grok_version_from_name(&n))
        } else {
            parse::tool_version(&raw)
        };
        out.push(ToolInfo {
            tool,
            version: version.map_or(Fact::Unknown, |v| Fact::Known(Some(v))),
            path: Some(path),
        });
    }
    Some(out)
}

pub fn tools<S: System>(sys: &S, cfg: &WindowsConfig, distro: &str) -> Vec<ToolInfo> {
    let Some(run) = tools_run(cfg, distro) else {
        return unknown_tools();
    };
    sys.run(&run)
        .ok()
        .filter(Output::success)
        .and_then(|o| parse_tools(&o.stdout))
        .unwrap_or_else(unknown_tools)
}

/// `wsl.exe -d <distro> --exec /bin/kill -TERM <pid>`.
pub fn kill_run(cfg: &WindowsConfig, distro: &str, pid: u32, timeout: Duration) -> Run {
    Run::with_args(
        &cfg.wsl_exe(),
        vec![
            "-d".into(),
            distro.into(),
            "--exec".into(),
            "/bin/kill".into(),
            "-TERM".into(),
            pid.to_string(),
        ],
    )
    .timeout(timeout)
}
