//! Pure parsers for the `/proc` text formats (proc(5)). No I/O here; the
//! table reader in [`super::table`] feeds them bytes read through `System`.
//!
//! Every format below is the documented proc(5) one. UNVERIFIED on a real
//! Linux machine (this crate was written on macOS); the fixtures under
//! `tests/fixtures/linux/` are copies of the documented shapes. Verify with
//! `cat /proc/self/stat /proc/self/status /proc/stat /proc/sys/kernel/random/boot_id`.
use session_navigation::model::BootId;

/// The fields of `/proc/<pid>/stat` the adapter uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    pub pid: u32,
    /// `comm`: the executable's name as the kernel recorded it (at most 15
    /// bytes; `prctl(PR_SET_NAME)` can change it). Only a fallback.
    pub comm: String,
    pub ppid: u32,
    /// Field 7, the controlling terminal's device number (0 = none).
    pub tty_nr: u64,
    /// Field 22, clock ticks since boot.
    pub starttime: u64,
}

fn number<T: std::str::FromStr>(token: &str) -> Option<T> {
    if token.is_empty() || token.len() > 20 || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// Parse `/proc/<pid>/stat`. `comm` is everything between the first `(`
/// and the **last** `)`: it may itself hold spaces and parentheses
/// (`1234 (a) b (c)) S 1 …`). The numeric fields after it are counted from
/// field 3 (`state`). Any malformed field fails the parse.
pub fn parse_stat(bytes: &[u8]) -> Option<Stat> {
    let text = std::str::from_utf8(bytes).ok()?;
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let pid = number(text.get(..open)?.trim())?;
    let comm = text.get(open + 1..close)?.to_owned();
    let rest: Vec<&str> = text.get(close + 1..)?.split_ascii_whitespace().collect();
    // rest[0] is field 3 (state); field n is rest[n - 3].
    let field = |n: usize| rest.get(n - 3).copied();
    Some(Stat {
        pid,
        comm,
        ppid: number(field(4)?)?,
        // tty_nr is printed as a signed int; a negative value is not a tty.
        tty_nr: number(field(7)?).unwrap_or(0),
        starttime: number(field(22)?)?,
    })
}

/// `(real, effective)` uid from the `Uid:` line of `/proc/<pid>/status`
/// (`Uid:\treal\teffective\tsaved\tfs`).
pub fn parse_status_uids(bytes: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("Uid:"))?;
    let mut ids = line.split_ascii_whitespace();
    let real = number(ids.next()?)?;
    let effective = number(ids.next()?)?;
    Some((real, effective))
}

/// `btime` (boot time, epoch seconds) from `/proc/stat`.
pub fn parse_btime(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value = text.lines().find_map(|l| l.strip_prefix("btime "))?;
    number(value.trim())
}

/// `/proc/sys/kernel/random/boot_id` (a UUID, one per boot) hashed into a
/// [`BootId`] with 64-bit FNV-1a over its lower-case text.
pub fn parse_boot_id(bytes: &[u8]) -> Option<BootId> {
    let text = std::str::from_utf8(bytes).ok()?.trim().to_ascii_lowercase();
    let uuid_like = text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| {
            matches!(i, 8 | 13 | 18 | 23) == (b == b'-') && (b == b'-' || b.is_ascii_hexdigit())
        });
    if !uuid_like {
        return None;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(BootId(hash))
}

/// The tty path a `tty_nr` names. Unix98 pty slaves (majors 136–143) are
/// `/dev/pts/N`; the virtual consoles (major 4, minor 1–63) `/dev/ttyN`.
/// Anything else (no tty, serial lines) is `None`. The minor number is
/// split across bits 0–7 and 20–31 (`MINOR(dev)` in proc(5)).
pub fn tty_path(tty_nr: u64) -> Option<String> {
    if tty_nr == 0 {
        return None;
    }
    let major = (tty_nr >> 8) & 0xfff;
    let minor = (tty_nr & 0xff) | ((tty_nr >> 12) & 0xfff00);
    match major {
        136..=143 => Some(format!("/dev/pts/{}", (major - 136) * 256 + minor)),
        4 if (1..64).contains(&minor) => Some(format!("/dev/tty{minor}")),
        _ => None,
    }
}

/// Start time in whole seconds since the epoch: `btime + starttime / ticks`
/// (floor). `ticks` is `sysconf(_SC_CLK_TCK)`, 100 on every mainstream
/// architecture (UNVERIFIED here: `getconf CLK_TCK`).
pub fn start_seconds(btime: u64, starttime: u64, ticks: u64) -> Option<u64> {
    if ticks == 0 {
        return None;
    }
    btime.checked_add(starttime / ticks)
}

/// argv from `/proc/<pid>/cmdline`: NUL-separated, usually NUL-terminated.
/// Kernel threads have an empty one. Non-UTF-8 bytes are replaced.
pub fn parse_cmdline(bytes: &[u8]) -> Vec<String> {
    let trimmed = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split(|b| *b == 0)
        .map(|word| String::from_utf8_lossy(word).into_owned())
        .collect()
}

/// A `/proc/<pid>/exe` or `cwd` link target. The kernel appends
/// ` (deleted)` when the file was removed or replaced (an upgrade while the
/// process runs). Returns the path and whether it was deleted; anything
/// not absolute (`anon_inode:…`, a memfd) is `None`.
pub fn link_target(target: &str) -> Option<(String, bool)> {
    let (path, deleted) = match target.strip_suffix(" (deleted)") {
        Some(path) => (path, true),
        None => (target, false),
    };
    let ok = path.starts_with('/') && !path.chars().any(char::is_control);
    ok.then(|| (path.to_owned(), deleted))
}

/// The environment keys the adapter reads. Nothing else is ever kept: the
/// environment holds secrets.
pub const IDENTITY_KEYS: [&str; 4] = ["WINDOWID", "KITTY_LISTEN_ON", "WEZTERM_UNIX_SOCKET", "TMUX"];

/// The identity keys of one process's environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    /// X11 `WINDOWID` (decimal, as xterm, Kitty, Alacritty and Konsole set it).
    pub window: Option<u64>,
    /// Kitty's `--to` address for this instance.
    pub kitty_listen_on: Option<String>,
    /// WezTerm's mux socket.
    pub wezterm_socket: Option<String>,
    /// tmux's `socket,server pid,session index`.
    pub tmux: Option<String>,
}

/// Pick [`IDENTITY_KEYS`] out of `/proc/<pid>/environ` (NUL-separated
/// `KEY=VALUE`). Every other entry is skipped without being copied.
pub fn parse_environ(bytes: &[u8]) -> Identity {
    let mut found = Identity::default();
    for entry in bytes.split(|b| *b == 0) {
        let Some(eq) = entry.iter().position(|b| *b == b'=') else {
            continue;
        };
        let (key, value) = entry.split_at(eq);
        let value = value.get(1..).unwrap_or_default();
        if !IDENTITY_KEYS.iter().any(|k| k.as_bytes() == key) {
            continue;
        }
        let Ok(value) = std::str::from_utf8(value) else {
            continue;
        };
        if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
            continue;
        }
        match key {
            b"WINDOWID" => found.window = number(value).filter(|id| *id != 0),
            b"KITTY_LISTEN_ON" => found.kitty_listen_on = Some(value.to_owned()),
            b"WEZTERM_UNIX_SOCKET" => found.wezterm_socket = Some(value.to_owned()),
            b"TMUX" => found.tmux = Some(value.to_owned()),
            _ => {}
        }
    }
    found
}

/// The socket path of a `TMUX` value (`/tmp/tmux-1000/default,2481,0`).
pub fn tmux_socket(value: &str) -> Option<&str> {
    let (socket, _) = value.split_once(',')?;
    socket.starts_with('/').then_some(socket)
}
