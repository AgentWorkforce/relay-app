//! The kernel process table from three `ps` calls, joined on pid and start.
//!
//! - `ucomm` is the kernel's `p_comm`: the executable file's own name (at most
//!   16 bytes), set by `exec` and not by the caller. Classification starts
//!   from it.
//! - `comm` is `argv[0]` as the caller passed it (`-/bin/zsh` for a login
//!   shell, a bare `codex` when a shell found it on PATH). It is used only
//!   to confirm an app bundle path and as `argv[0]`.
//! - `args` is the argv joined with spaces by `ps`, so splitting it is lossy:
//!   an argument containing a space becomes two. Policy uses argv only to
//!   recognise subcommands and a named session id (ids never contain
//!   spaces); no safety check depends on it.
use std::collections::BTreeMap;

use session_navigation::evidence::{Exe, Fact, ProcessInfo};
use session_navigation::model::{BootId, ProcessIdentity};
use session_navigation::parse::lstart;

/// The `-o` lists of the three calls. Each ends with its free-text column.
pub const PS_IDENTITY: &str = "pid=,ppid=,uid=,ruid=,tty=,lstart=,ucomm=";
pub const PS_COMM: &str = "pid=,lstart=,comm=";
pub const PS_ARGS: &str = "pid=,lstart=,args=";

/// Bound on rows and line length: anything larger is not a process table.
const MAX_ROWS: usize = 20_000;
const MAX_LINE: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityRow {
    pub pid: u32,
    pub ppid: u32,
    pub euid: u32,
    pub ruid: u32,
    pub tty: Option<String>,
    pub start: u64,
    pub ucomm: String,
}

/// Take `n` whitespace-separated tokens, then the rest of the line trimmed.
fn split_fields(line: &str, n: usize) -> Option<(Vec<&str>, &str)> {
    let mut rest = line;
    let mut tokens = Vec::with_capacity(n);
    for _ in 0..n {
        rest = rest.trim_start_matches([' ', '\t']);
        let end = rest.find([' ', '\t']).unwrap_or(rest.len());
        let (token, tail) = rest.split_at(end);
        if token.is_empty() {
            return None;
        }
        tokens.push(token);
        rest = tail;
    }
    Some((tokens, rest.trim_matches([' ', '\t'])))
}

fn number(token: &str) -> Option<u32> {
    if token.is_empty() || token.len() > 10 || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// A uid column. `ps` prints uids as signed 32-bit, so `nobody`
/// (4294967294) is `-2` (seen natively on IPConfigurationHelper, distnoted).
fn uid(token: &str) -> Option<u32> {
    match token.strip_prefix('-') {
        Some(magnitude) => number(magnitude)
            .filter(|m| *m != 0)
            .map(|m| u32::MAX - (m - 1)),
        None => number(token),
    }
}

/// `??` → no tty; `ttys012` → `/dev/ttys012`.
pub fn tty(token: &str) -> Option<Option<String>> {
    if token == "??" {
        return Some(None);
    }
    let ok = !token.is_empty()
        && token.len() <= 32
        && token
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    ok.then(|| Some(format!("/dev/{token}")))
}

fn lines(bytes: &[u8]) -> Option<Vec<&str>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let sane =
        !lines.is_empty() && lines.len() <= MAX_ROWS && lines.iter().all(|l| l.len() <= MAX_LINE);
    sane.then_some(lines)
}

fn start_of(tokens: &[&str]) -> Option<u64> {
    lstart::parse(&tokens.join(" ")).ok()
}

/// `pid ppid uid ruid tty lstart(5) ucomm…`. Any bad row fails the table.
pub fn parse_identity(bytes: &[u8]) -> Option<Vec<IdentityRow>> {
    let mut rows = Vec::new();
    for line in lines(bytes)? {
        let (t, ucomm) = split_fields(line, 10)?;
        if ucomm.is_empty() {
            return None;
        }
        rows.push(IdentityRow {
            pid: number(t.first()?)?,
            ppid: number(t.get(1)?)?,
            euid: uid(t.get(2)?)?,
            ruid: uid(t.get(3)?)?,
            tty: tty(t.get(4)?)?,
            start: start_of(t.get(5..10)?)?,
            ucomm: ucomm.to_owned(),
        });
    }
    Some(rows)
}

/// `pid lstart(5) text…` → pid → (start, text).
pub fn parse_text(bytes: &[u8]) -> Option<BTreeMap<u32, (u64, String)>> {
    let mut out = BTreeMap::new();
    for line in lines(bytes)? {
        let (t, text) = split_fields(line, 6)?;
        let pid = number(t.first()?)?;
        let start = start_of(t.get(1..6)?)?;
        if out.insert(pid, (start, text.to_owned())).is_some() {
            return None;
        }
    }
    Some(out)
}

/// `argv[0]` from `comm` (exact, spaces kept), the rest split on whitespace.
pub fn argv(comm: &str, args: &str) -> Vec<String> {
    let mut argv = Vec::new();
    let rest = match args.strip_prefix(comm) {
        Some(rest) if !comm.is_empty() && (rest.is_empty() || rest.starts_with(' ')) => {
            argv.push(comm.to_owned());
            rest
        }
        _ => args,
    };
    argv.extend(rest.split_whitespace().map(str::to_owned));
    argv
}

const SHELLS: &[&str] = &["zsh", "bash", "sh", "dash", "fish", "ksh", "tcsh", "csh"];

/// Classify from the kernel name, confirming app bundles by `argv[0]` path
/// (apps are started by launchd with their full path).
pub fn classify(ucomm: &str, comm: &str) -> Exe {
    let path = comm.strip_prefix('-').unwrap_or(comm);
    let bundle = |suffix: &str| path.starts_with('/') && path.ends_with(suffix);
    match ucomm {
        "claude" => Exe::ClaudeCli,
        "codex" => Exe::CodexCli,
        "opencode" | "opencode-cli" => Exe::OpenCodeCli,
        // `ucomm` keeps 16 bytes, so `grok-0.1.23-darwin-arm64` is cut.
        name if name == "grok" || name.starts_with("grok-") => Exe::GrokCli,
        "tmux" => Exe::Tmux,
        "Claude" if bundle("/Claude.app/Contents/MacOS/Claude") => Exe::ClaudeDesktopApp,
        "ChatGPT" if bundle("/ChatGPT.app/Contents/MacOS/ChatGPT") => Exe::CodexDesktopApp,
        "Codex" if bundle("/Codex.app/Contents/MacOS/Codex") => Exe::CodexDesktopApp,
        "OpenCode" if bundle("/OpenCode.app/Contents/MacOS/OpenCode") => Exe::OpenCodeDesktopApp,
        "ghostty" if bundle("/Ghostty.app/Contents/MacOS/ghostty") => Exe::Ghostty,
        "Terminal" if bundle("/Terminal.app/Contents/MacOS/Terminal") => Exe::AppleTerminal,
        "iTerm2" if bundle("/iTerm.app/Contents/MacOS/iTerm2") => Exe::ITerm2,
        "kitty" if bundle("/kitty.app/Contents/MacOS/kitty") => Exe::Kitty,
        "wezterm-gui" if bundle("/WezTerm.app/Contents/MacOS/wezterm-gui") => Exe::WezTerm,
        "alacritty" if bundle("/Alacritty.app/Contents/MacOS/alacritty") => Exe::Alacritty,
        "login" => Exe::Login,
        name if SHELLS.contains(&name) => Exe::Shell,
        _ => Exe::Other,
    }
}

pub fn is_agent_cli(exe: Exe) -> bool {
    matches!(
        exe,
        Exe::ClaudeCli | Exe::CodexCli | Exe::OpenCodeCli | Exe::GrokCli
    )
}

/// Join the three outputs. A row whose pid is missing from, or started at a
/// different time in, another output changed between the calls and is
/// dropped. Any unparsable output fails the whole table. `cwd` is left
/// `Unknown`; the observer fills it for agent CLIs.
pub fn join_table(
    identity: &[u8],
    comm: &[u8],
    args: &[u8],
    boot: BootId,
) -> Option<Vec<ProcessInfo>> {
    let rows = parse_identity(identity)?;
    let comm = parse_text(comm)?;
    let args = parse_text(args)?;
    let mut seen = std::collections::BTreeSet::new();
    let mut table = Vec::new();
    for row in rows {
        if !seen.insert(row.pid) {
            return None;
        }
        let (Some((c_start, c)), Some((a_start, a))) = (comm.get(&row.pid), args.get(&row.pid))
        else {
            continue;
        };
        if *c_start != row.start || *a_start != row.start {
            continue;
        }
        table.push(ProcessInfo {
            identity: ProcessIdentity {
                pid: row.pid,
                start: row.start,
                boot,
            },
            ppid: row.ppid,
            euid: row.euid,
            ruid: row.ruid,
            exe: classify(&row.ucomm, c),
            tty: row.tty,
            argv: argv(c, a),
            cwd: Fact::Unknown,
            // X11 `WINDOWID` does not exist on macOS.
            window: None,
        });
    }
    (!table.is_empty()).then_some(table)
}
