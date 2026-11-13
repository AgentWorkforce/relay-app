//! History folders inside the computer's running WSL distributions.
//!
//! On Windows, people often run Claude Code inside WSL, so its history lives
//! in the distribution (`/home/<user>/.claude`), which Windows reaches as
//! `\\wsl.localhost\<distro>\home\<user>\.claude`. Each such folder with a
//! `projects/` folder is read as one more Claude folder, beside the ones
//! `relay::claude_dirs` records, with the same scrubbing and sharing policy.
//!
//! Only distributions that are already running are read: touching a stopped
//! one's share would boot it. The running list is asked of `wsl.exe` every
//! ten seconds and each running distribution's homes are looked for at most
//! once a minute, all on background threads, so no capture pass waits on
//! `wsl.exe` or the share. A distribution that stops drops out of [`homes`]
//! at the next list, before its share is read again (a window of at most
//! the list interval). A look that hangs (a distribution shutting down)
//! holds back only that distribution's next look; the others keep
//! publishing. The 9P share sends no file events, so these folders are
//! polled, never watched. `AGENT_RELAY_WSL_HISTORY=0` turns this off. On
//! macOS and Linux there is nothing to find.

use std::path::{Path, PathBuf};

/// Set to `0`, `false` or `off` to leave WSL distributions unread.
pub const ENV: &str = "AGENT_RELAY_WSL_HISTORY";

/// Whether `value` (of [`ENV`]) turns WSL reading off.
fn turned_off(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

/// One user's home inside a running WSL distribution. Every provider's
/// history under it is read as that provider's (`ProviderRoots::from_home`),
/// so a provider ai-hist learns is read inside WSL too; Claude's folder is
/// also an extra Claude folder, which the live capture paths follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Home {
    /// `\\wsl.localhost\<distro>\home\<user>`, or the distribution's `root`.
    pub home: PathBuf,
    /// `.claude`, when it holds a `projects/` folder.
    pub claude: Option<PathBuf>,
}

/// The homes of the distributions last listed as running, as last found;
/// none off Windows, when [`ENV`] turns it off, or before the first look has
/// finished. Never touches the file system or starts a process on the
/// caller's thread: the looks run in the background.
pub fn homes() -> Vec<Home> {
    if turned_off(std::env::var(ENV).ok().as_deref()) {
        return Vec::new();
    }
    platform::homes()
}

/// The Claude folders of [`homes`].
pub fn claude_dirs() -> Vec<PathBuf> {
    homes().into_iter().filter_map(|home| home.claude).collect()
}

/// Whether `path` is inside one of [`homes`]: its distribution was running
/// at the last list.
pub fn still_running(path: &Path) -> bool {
    homes().iter().any(|home| path.starts_with(&home.home))
}

/// Whether `path` is inside a WSL distribution's share, which sends no file
/// events and so is polled rather than watched.
pub fn is_wsl_path(path: &Path) -> bool {
    let text = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    [
        "\\\\wsl.localhost\\",
        "\\\\wsl$\\",
        "\\\\?\\unc\\wsl.localhost\\",
        "\\\\?\\unc\\wsl$\\",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}

/// The distribution names `wsl.exe -l -q` printed: UTF-16LE (with or
/// without a byte order mark) as Windows writes it, or UTF-8 when
/// `WSL_UTF8=1`. Blank lines and NULs are dropped; a name holding a path
/// separator is refused.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_distros(output: &[u8]) -> Vec<String> {
    let text = if output.len() >= 2 && (output.starts_with(&[0xff, 0xfe]) || output[1] == 0) {
        let units: Vec<u16> = output
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(output).into_owned()
    };
    text.lines()
        .map(|line| line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0'))
        .filter(|name| !name.is_empty())
        .filter(|name| !name.contains(['\\', '/', ':']) && *name != "." && *name != "..")
        .map(str::to_owned)
        .collect()
}

/// The homes under one distribution's share `root`
/// (`\\wsl.localhost\<distro>`): each `home/<user>` and `root` that can be
/// read, with its Claude folder when it holds `projects/`. An entry that
/// cannot be read is logged and skipped; a missing one is skipped quietly.
#[cfg_attr(not(windows), allow(dead_code))]
fn homes_in(root: &Path) -> Vec<Home> {
    let quiet = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
    let mut homes = Vec::new();
    match std::fs::read_dir(root.join("home")) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => homes.push(entry.path()),
                    Err(error) => skipped(&root.join("home"), &error),
                }
            }
        }
        Err(error) if quiet(&error) => {}
        Err(error) => skipped(&root.join("home"), &error),
    }
    homes.sort();
    homes.push(root.join("root"));
    homes
        .into_iter()
        .filter(|home| match std::fs::read_dir(home) {
            Ok(_) => true,
            Err(error) => {
                if !quiet(&error) {
                    skipped(home, &error);
                }
                false
            }
        })
        .map(|home| {
            let claude = home.join(".claude");
            Home {
                claude: claude.join("projects").is_dir().then_some(claude),
                home,
            }
        })
        .collect()
}

/// Log a share entry that could not be read before it is skipped.
#[cfg_attr(not(windows), allow(dead_code))]
fn skipped(path: &Path, error: &std::io::Error) {
    eprintln!("wsl history: {} skipped: {error}", path.display());
}

#[cfg(windows)]
mod platform {
    use super::{homes_in, parse_distros, Home};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::{Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    /// How often the running list is asked of `wsl.exe`.
    const LIST_EVERY: Duration = Duration::from_secs(10);
    /// How long one distribution's homes are reused, a failed look's too.
    const LOOK_EVERY: Duration = Duration::from_secs(60);
    /// How long `wsl.exe` may take to list the distributions.
    const LIST_TIMEOUT: Duration = Duration::from_secs(3);

    /// One distribution's last look, and whether one is running now.
    #[derive(Default)]
    struct Distro {
        homes: Vec<Home>,
        looked: Option<Instant>,
        looking: bool,
    }

    #[derive(Default)]
    struct State {
        running: Vec<String>,
        listed: Option<Instant>,
        listing: bool,
        distros: HashMap<String, Distro>,
    }

    static STATE: Mutex<Option<State>> = Mutex::new(None);

    fn state() -> MutexGuard<'static, Option<State>> {
        STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn homes() -> Vec<Home> {
        let mut guard = state();
        let state = guard.get_or_insert_with(State::default);
        let stale = state.listed.is_none_or(|at| at.elapsed() >= LIST_EVERY);
        if stale && !state.listing {
            state.listing = true;
            if spawn("wsl-history-list", list).is_err() {
                state.listing = false;
                state.listed = Some(Instant::now());
            }
        }
        state
            .running
            .iter()
            .filter_map(|name| state.distros.get(name))
            .flat_map(|distro| distro.homes.clone())
            .collect()
    }

    fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
        std::thread::Builder::new()
            .name(name.into())
            .spawn(work)
            .map(drop)
    }

    /// Record the running list, forget stopped distributions' homes, and
    /// start a look at each running one whose homes are stale and that has
    /// no look still running (one that hung holds back only itself).
    fn list() {
        let running = running_distros();
        let mut guard = state();
        let state = guard.get_or_insert_with(State::default);
        state.running = running.clone();
        state.listed = Some(Instant::now());
        state.listing = false;
        state
            .distros
            .retain(|name, distro| distro.looking || running.contains(name));
        for name in running {
            let distro = state.distros.entry(name.clone()).or_default();
            let stale = distro.looked.is_none_or(|at| at.elapsed() >= LOOK_EVERY);
            if stale && !distro.looking {
                distro.looking = true;
                let looked = name.clone();
                if spawn("wsl-history-look", move || look(looked)).is_err() {
                    distro.looking = false;
                    distro.looked = Some(Instant::now());
                }
            }
        }
    }

    /// Look for `name`'s homes and publish them.
    fn look(name: String) {
        let homes = homes_in(&PathBuf::from(format!(r"\\wsl.localhost\{name}")));
        let mut guard = state();
        let state = guard.get_or_insert_with(State::default);
        let distro = state.distros.entry(name).or_default();
        distro.homes = homes;
        distro.looked = Some(Instant::now());
        distro.looking = false;
    }

    /// `wsl.exe` by absolute path, never a PATH lookup.
    fn wsl_exe() -> PathBuf {
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        PathBuf::from(system_root).join("System32").join("wsl.exe")
    }

    /// The running distributions, without starting any; none when WSL is
    /// absent, fails or takes longer than [`LIST_TIMEOUT`].
    fn running_distros() -> Vec<String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let spawned = Command::new(wsl_exe())
            .args(["--list", "--running", "--quiet"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                eprintln!("wsl history: wsl.exe did not start: {error}");
                return Vec::new();
            }
        };
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(status)) => {
                    // No distribution installed exits non-zero: not an error.
                    let _ = status;
                    return Vec::new();
                }
                Err(error) => {
                    eprintln!("wsl history: wsl.exe failed: {error}");
                    return Vec::new();
                }
                Ok(None) if started.elapsed() > LIST_TIMEOUT => {
                    eprintln!("wsl history: wsl.exe took longer than {LIST_TIMEOUT:?}");
                    let _ = child.kill();
                    let _ = child.wait();
                    return Vec::new();
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            }
        }
        let mut output = Vec::new();
        if let Some(mut stdout) = child.stdout.take() {
            use std::io::Read;
            let _ = stdout.read_to_end(&mut output);
        }
        parse_distros(&output)
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Home;

    pub fn homes() -> Vec<Home> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str, bom: bool) -> Vec<u8> {
        let mut bytes = if bom { vec![0xff, 0xfe] } else { Vec::new() };
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    #[test]
    fn distros_parse_from_utf16_and_utf8() {
        for output in [
            utf16("Ubuntu\r\nDebian\r\n\r\n", false),
            utf16("Ubuntu\r\nDebian\r\n", true),
            b"Ubuntu\nDebian\n".to_vec(),
        ] {
            assert_eq!(parse_distros(&output), ["Ubuntu", "Debian"]);
        }
        assert!(parse_distros(b"").is_empty());
        assert!(parse_distros(&utf16("\r\n", false)).is_empty());
    }

    #[test]
    fn a_distro_name_with_a_separator_is_refused() {
        assert_eq!(
            parse_distros(b"..\nbad\\name\nc:x\nUbuntu-22.04\n"),
            ["Ubuntu-22.04"]
        );
    }

    #[test]
    fn every_readable_home_is_found_with_its_claude_folder() {
        let share = tempfile::tempdir().unwrap();
        let root = share.path();
        std::fs::create_dir_all(root.join("home/ana/.claude/projects")).unwrap();
        std::fs::create_dir_all(root.join("home/bo/.claude")).unwrap();
        std::fs::create_dir_all(root.join("home/cy/.codex/sessions")).unwrap();
        std::fs::create_dir_all(root.join("root/.claude/projects")).unwrap();
        let home = |user: &str, claude: bool| Home {
            home: root.join(user),
            claude: claude.then(|| root.join(user).join(".claude")),
        };
        assert_eq!(
            homes_in(root),
            [
                home("home/ana", true),
                home("home/bo", false),
                home("home/cy", false),
                home("root", true),
            ]
        );
        let empty = tempfile::tempdir().unwrap();
        assert!(homes_in(empty.path()).is_empty());
    }

    #[test]
    fn nothing_is_found_off_windows_or_when_turned_off() {
        if !cfg!(windows) {
            assert!(homes().is_empty());
        }
    }

    #[test]
    fn wsl_share_paths_are_recognised() {
        for path in [
            r"\\wsl.localhost\Ubuntu\home\ana\.claude",
            r"\\WSL$\Ubuntu\home\ana\.claude",
            r"\\?\UNC\wsl.localhost\Ubuntu\root\.claude",
            "//wsl.localhost/Ubuntu/home/ana/.claude",
        ] {
            assert!(is_wsl_path(Path::new(path)), "{path}");
        }
        for path in [
            r"C:\Users\ana\.claude",
            "/home/ana/.claude",
            r"\\server\share\.claude",
        ] {
            assert!(!is_wsl_path(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn the_switch_turns_reading_off() {
        for value in ["0", "false", "OFF", " no "] {
            assert!(turned_off(Some(value)));
        }
        for value in [None, Some("1"), Some("true"), Some("")] {
            assert!(!turned_off(value));
        }
    }
}
