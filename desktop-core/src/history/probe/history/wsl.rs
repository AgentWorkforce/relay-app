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
//!
//! A one-shot process (the sign-in preview) exits before those background
//! looks finish, so it asks [`homes_now`] instead: the same list and looks
//! on its own time, bounded by a budget.
//!
//! This is automatic discovery only. A WSL folder the person named
//! (`AGENT_RELAY_CLAUDE_CONFIG_DIRS`, Settings) is read whenever it can be,
//! running list and switch aside (`wsl_capture::readable`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// How long a one-shot caller (the sign-in preview) gives [`homes_now`].
pub const ONE_SHOT_BUDGET: Duration = Duration::from_secs(5);

/// The homes of the distributions running now, found on the caller's thread
/// within about `budget`: for a one-shot caller that exits before the
/// background looks of [`homes`] would finish. Only distributions `wsl.exe`
/// lists as running are read, so a stopped one is never booted; one whose
/// look outlasts the budget is left out. What is found is also published to
/// [`homes`], so the rest of the process (extra Claude folders,
/// [`still_running`]) sees the same homes. None off Windows or when [`ENV`]
/// turns it off.
pub fn homes_now(budget: Duration) -> Vec<Home> {
    if turned_off(std::env::var(ENV).ok().as_deref()) {
        return Vec::new();
    }
    platform::homes_now(budget)
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

/// `path` as one spelling of a WSL share path, with backslashes. The
/// Windows side is compared without case: `\\?\UNC\x` is read as `\\x`,
/// `\\wsl$\` as `\\wsl.localhost\`, and the server and distribution name
/// are lower-cased. The Linux path after the distribution name keeps its
/// case, since Linux tells `/home/ana` and `/home/Ana` apart. Any other path
/// is a Windows one: only lower-cased and given backslashes.
fn share_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let text = match strip_prefix_ignoring_case(&text, "\\\\?\\UNC\\") {
        Some(rest) => format!("\\\\{rest}"),
        None => text,
    };
    let Some(rest) = ["\\\\wsl.localhost\\", "\\\\wsl$\\"]
        .iter()
        .find_map(|server| strip_prefix_ignoring_case(&text, server))
    else {
        return text.to_ascii_lowercase();
    };
    match rest.split_once('\\') {
        Some((distro, linux)) => format!(
            "\\\\wsl.localhost\\{}\\{linux}",
            distro.to_ascii_lowercase()
        ),
        None => format!("\\\\wsl.localhost\\{}", rest.to_ascii_lowercase()),
    }
}

/// `text` without `prefix`, matched without ASCII case.
fn strip_prefix_ignoring_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &text[prefix.len()..])
}

/// Whether `path` is inside a WSL distribution's share, which sends no file
/// events and so is polled rather than watched.
pub fn is_wsl_path(path: &Path) -> bool {
    share_key(path).starts_with("\\\\wsl.localhost\\")
}

/// Whether WSL `path` is `dir` or inside it, however either is spelled
/// ([`share_key`]).
pub fn is_within(path: &Path, dir: &Path) -> bool {
    let (path, dir) = (share_key(path), share_key(dir));
    let dir = dir.trim_end_matches('\\');
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
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

/// Each running distribution's homes: `list` names the running ones (given
/// `budget`, which bounds its own wait) and `look` finds one's homes, each on
/// its own thread. A look not done by the deadline is left out, its thread
/// left to finish on its own. In the order listed.
#[cfg_attr(not(windows), allow(dead_code))]
fn look_within<L>(
    budget: Duration,
    list: impl FnOnce(Duration) -> Vec<String>,
    look: L,
) -> Vec<(String, Vec<Home>)>
where
    L: Fn(&str) -> Vec<Home> + Send + Sync + 'static,
{
    let deadline = Instant::now() + budget;
    let running = list(budget);
    let look = std::sync::Arc::new(look);
    let (sender, found) = std::sync::mpsc::channel();
    for (index, name) in running.iter().enumerate() {
        let (sender, look, name) = (sender.clone(), look.clone(), name.clone());
        let spawned = std::thread::Builder::new()
            .name("wsl-history-look-now".into())
            .spawn(move || drop(sender.send((index, look(&name)))));
        if let Err(error) = spawned {
            eprintln!("wsl history: look did not start: {error}");
        }
    }
    drop(sender);
    let mut homes: Vec<Option<Vec<Home>>> = vec![None; running.len()];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match found.recv_timeout(left) {
            Ok((index, found)) => homes[index] = Some(found),
            // Out of time, or every look answered.
            Err(_) => break,
        }
    }
    running
        .into_iter()
        .zip(homes)
        .filter_map(|(name, homes)| Some((name, homes?)))
        .collect()
}

/// The homes under one distribution's share `root`
/// (`\\wsl.localhost\<distro>`): each `home/<user>` and `root` that can be
/// read, with its Claude folder when it holds `projects/`. An entry that
/// cannot be read is logged and skipped; a missing one, or one this user may
/// not read, is skipped quietly ([`quiet`]).
#[cfg_attr(not(windows), allow(dead_code))]
fn homes_in(root: &Path) -> Vec<Home> {
    homes_logged(root, &mut skipped)
}

/// [`homes_in`], with each skip worth logging given to `log`.
#[cfg_attr(not(windows), allow(dead_code))]
fn homes_logged(root: &Path, log: &mut dyn FnMut(&Path, &std::io::Error)) -> Vec<Home> {
    let mut skip = |path: &Path, error: &std::io::Error| {
        if !quiet(error) {
            log(path, error);
        }
    };
    let mut homes = Vec::new();
    match std::fs::read_dir(root.join("home")) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => homes.push(entry.path()),
                    Err(error) => skip(&root.join("home"), &error),
                }
            }
        }
        Err(error) => skip(&root.join("home"), &error),
    }
    homes.sort();
    homes.push(root.join("root"));
    homes
        .into_iter()
        .filter(|home| match std::fs::read_dir(home) {
            Ok(_) => true,
            Err(error) => {
                skip(home, &error);
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

/// Whether a share entry's `error` is skipped without a word: it is missing,
/// or another user's private home (the stock image's `/home/ubuntu` and
/// `/root`, which Windows answers "Access is denied" for, every look).
#[cfg_attr(not(windows), allow(dead_code))]
fn quiet(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    )
}

/// Log a share entry that could not be read before it is skipped.
#[cfg_attr(not(windows), allow(dead_code))]
fn skipped(path: &Path, error: &std::io::Error) {
    eprintln!("wsl history: {} skipped: {error}", path.display());
}

#[cfg(windows)]
mod platform {
    use super::{homes_in, look_within, parse_distros, Home};
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

    /// The running distributions' homes, listed and looked at on this
    /// thread within `budget`, then published for [`homes`].
    pub fn homes_now(budget: Duration) -> Vec<Home> {
        let found = look_within(
            budget,
            |budget| running_distros(budget.min(LIST_TIMEOUT)),
            |name| homes_in(&share(name)),
        );
        let mut guard = state();
        let state = guard.get_or_insert_with(State::default);
        state.running = found.iter().map(|(name, _)| name.clone()).collect();
        state.listed = Some(Instant::now());
        for (name, homes) in &found {
            let distro = state.distros.entry(name.clone()).or_default();
            distro.homes = homes.clone();
            distro.looked = Some(Instant::now());
        }
        found.into_iter().flat_map(|(_, homes)| homes).collect()
    }

    /// `\\wsl.localhost\<name>`, the distribution's share.
    fn share(name: &str) -> PathBuf {
        PathBuf::from(format!(r"\\wsl.localhost\{name}"))
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
        let running = running_distros(LIST_TIMEOUT);
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
        let homes = homes_in(&share(&name));
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
    /// absent, fails or takes longer than `timeout`.
    fn running_distros(timeout: Duration) -> Vec<String> {
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
                Ok(None) if started.elapsed() > timeout => {
                    eprintln!("wsl history: wsl.exe took longer than {timeout:?}");
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
    use std::time::Duration;

    pub fn homes() -> Vec<Home> {
        Vec::new()
    }

    pub fn homes_now(_budget: Duration) -> Vec<Home> {
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
            assert!(homes_now(Duration::from_millis(50)).is_empty());
        }
    }

    /// The one-shot path lists and looks on the caller's time: only the
    /// listed (running) distributions are read, each with `homes_in`, and a
    /// look that outlasts the budget is left out instead of waited for.
    #[test]
    fn one_shot_homes_are_found_now_within_the_budget() {
        let share = tempfile::tempdir().unwrap();
        for distro in ["Ubuntu", "Debian", "Stopped"] {
            std::fs::create_dir_all(share.path().join(distro).join("home/ana/.claude/projects"))
                .unwrap();
        }
        let budget = Duration::from_millis(400);
        let given = std::sync::Mutex::new(None);
        let looked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let root = share.path().to_path_buf();
        let started = Instant::now();
        let found = look_within(
            budget,
            |budget| {
                *given.lock().unwrap() = Some(budget);
                vec!["Ubuntu".into(), "Hung".into(), "Debian".into()]
            },
            {
                let looked = looked.clone();
                move |name: &str| {
                    looked.lock().unwrap().push(name.to_owned());
                    if name == "Hung" {
                        std::thread::sleep(Duration::from_secs(3));
                    }
                    homes_in(&root.join(name))
                }
            },
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(*given.lock().unwrap(), Some(budget));
        let names: Vec<&str> = found.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["Ubuntu", "Debian"]);
        assert_eq!(
            found[0].1,
            [Home {
                home: share.path().join("Ubuntu/home/ana"),
                claude: Some(share.path().join("Ubuntu/home/ana/.claude")),
            }]
        );
        // A distribution not listed as running is never touched.
        assert!(!looked.lock().unwrap().iter().any(|name| name == "Stopped"));
        // Nothing running: nothing looked at, no wait.
        let started = Instant::now();
        assert!(look_within(budget, |_| Vec::new(), |_: &str| Vec::new()).is_empty());
        assert!(started.elapsed() < budget);
    }

    /// Another user's private home is skipped without a log line on every
    /// look; any other unreadable entry is still logged.
    #[cfg(unix)]
    #[test]
    fn a_private_home_is_skipped_quietly() {
        use std::os::unix::fs::PermissionsExt;
        let share = tempfile::tempdir().unwrap();
        let root = share.path();
        std::fs::create_dir_all(root.join("home/ana/.claude/projects")).unwrap();
        std::fs::create_dir_all(root.join("home/ubuntu")).unwrap();
        std::fs::create_dir_all(root.join("root")).unwrap();
        std::fs::write(root.join("home/notes"), "not a home").unwrap();
        for private in ["home/ubuntu", "root"] {
            std::fs::set_permissions(root.join(private), std::fs::Permissions::from_mode(0o000))
                .unwrap();
        }
        // Root reads anything; the denial cannot be staged then.
        let denied = std::fs::read_dir(root.join("root")).is_err();
        let mut logged = Vec::new();
        let homes = homes_logged(root, &mut |path, _| logged.push(path.to_path_buf()));
        for private in ["home/ubuntu", "root"] {
            std::fs::set_permissions(root.join(private), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        assert_eq!(logged, [root.join("home/notes")]);
        if denied {
            assert_eq!(
                homes,
                [Home {
                    home: root.join("home/ana"),
                    claude: Some(root.join("home/ana/.claude")),
                }]
            );
        }
    }

    #[test]
    fn only_a_missing_or_denied_entry_is_quiet() {
        use std::io::{Error, ErrorKind};
        assert!(quiet(&Error::from(ErrorKind::NotFound)));
        assert!(quiet(&Error::from(ErrorKind::PermissionDenied)));
        assert!(!quiet(&Error::from(ErrorKind::TimedOut)));
        assert!(!quiet(&Error::other("share gone")));
    }

    #[test]
    fn a_path_is_within_its_folder_however_spelled() {
        let dir = Path::new(r"\\wsl.localhost\Ubuntu\home\x\.claude");
        for path in [
            r"\\wsl.localhost\Ubuntu\home\x\.claude",
            r"\\wsl.localhost\Ubuntu\home\x\.claude\projects\p\s.jsonl",
            r"\\WSL$\ubuntu\home\x\.claude\projects",
            r"\\?\UNC\wsl.localhost\Ubuntu\home\x\.claude",
            "//wsl.localhost/Ubuntu/home/x/.claude/projects",
        ] {
            assert!(is_within(Path::new(path), dir), "{path}");
        }
        for path in [
            r"\\wsl.localhost\Ubuntu\home\x\.claude-work",
            r"\\wsl.localhost\Ubuntu\home\y\.claude",
            r"\\wsl.localhost\Debian\home\x\.claude",
        ] {
            assert!(!is_within(Path::new(path), dir), "{path}");
        }
    }

    /// Linux tells folders apart by case, so only the Windows side of a
    /// share path (the UNC prefix, the server alias and the distribution
    /// name) is compared without it.
    #[test]
    fn linux_path_case_tells_wsl_folders_apart() {
        let dir = Path::new(r"\\wsl.localhost\Ubuntu\home\ana\.claude\projects");
        for path in [
            r"\\wsl.localhost\Ubuntu\home\Ana\.claude\projects",
            r"\\wsl.localhost\Ubuntu\home\ana\.CLAUDE\projects",
            r"\\WSL$\ubuntu\HOME\ana\.claude\projects",
        ] {
            assert!(!is_within(Path::new(path), dir), "{path}");
            assert!(!is_within(dir, Path::new(path)), "{path}");
        }
        for path in [
            r"\\wsl$\Ubuntu\home\ana\.claude\projects",
            r"\\WSL.LOCALHOST\ubuntu\home\ana\.claude\projects",
            r"\\?\unc\Wsl$\UBUNTU\home\ana\.claude\projects\",
            "//wsl.localhost/Ubuntu/home/ana/.claude/projects",
        ] {
            assert!(is_within(Path::new(path), dir), "{path}");
            assert!(is_within(dir, Path::new(path)), "{path}");
        }
        // The distribution alone, however its name is cased.
        assert!(is_within(dir, Path::new(r"\\wsl$\UBUNTU")));
        // A Windows path is still compared without case.
        assert!(is_within(
            Path::new(r"C:\Users\Ana\.claude"),
            Path::new(r"c:\users\ana\.CLAUDE")
        ));
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
