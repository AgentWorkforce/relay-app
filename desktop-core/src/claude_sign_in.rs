//! Starting Claude Code's own sign-in (`claude auth login`) for the person,
//! from Settings, when Claude Code cloud sessions are waiting for it.
//!
//! The command is Claude Code's, run where the person can see and answer
//! it, and nothing here reads, copies or logs what it stores. Which
//! `claude` runs:
//!
//! 1. one the person installed: on `PATH`, else where Claude Code's
//!    installers put it ([`ClaudeConfig`]'s own search);
//! 2. on macOS, else, the copy Claude's desktop app keeps for its Code tab
//!    (`~/Library/Application Support/Claude/claude-code/<version>/<hash>/claude.app`):
//!    the newest one the app marked verified, owned by this user and
//!    writable by no one else, and only when its code signature is
//!    Anthropic's (`com.anthropic.claude-code`, team `Q6L2SF6YDW`).
//!
//! On macOS it runs in Terminal through a `.command` file opened with
//! `open`, which raises no Automation (AppleScript) prompt. When Claude
//! Code's Keychain item appears, Agent Relay asks for access to it as
//! turning cloud sessions on does ([`claude_keychain::ask_access`]), and
//! the next listing uses it. On Windows the command opens in a console of
//! its own. Linux has no way here to open a terminal the person chose, so
//! it is reported unavailable and the status line says what to run.
//!
//! [`ClaudeConfig`]: agent_relay_node::relay::claude::ClaudeConfig
//! [`claude_keychain::ask_access`]: crate::claude_keychain::ask_access
use std::path::{Path, PathBuf};

/// The designated requirement the desktop app's copy must satisfy.
pub const ANTHROPIC_REQUIREMENT: &str = "anchor apple generic and identifier \"com.anthropic.claude-code\" and certificate leaf[subject.OU] = \"Q6L2SF6YDW\"";

/// Why the sign-in cannot start, as Settings shows it.
pub const NOT_FOUND: &str = "Agent Relay could not find Claude Code on this computer. Install Claude Code, then run claude auth login in Terminal";
pub const NOT_VERIFIED: &str = "The Claude Code that Claude's desktop app includes could not be verified as Anthropic's, so Agent Relay did not run it. Install Claude Code, then run claude auth login in Terminal";
pub const UNAVAILABLE: &str = "Starting Claude Code's sign-in from Agent Relay is not available on Linux. Run claude auth login in a terminal";
pub const NOT_STARTED: &str =
    "Agent Relay could not open Claude Code's sign-in. Run claude auth login in Terminal";

/// How long, after the sign-in opens, Agent Relay waits for Claude Code's
/// Keychain item before it stops looking.
#[cfg(target_os = "macos")]
const WATCH_FOR: std::time::Duration = std::time::Duration::from_secs(15 * 60);
#[cfg(target_os = "macos")]
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// Where the `claude` the sign-in runs came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// One the person installed.
    Installed,
    /// The copy in Claude's desktop app; `bundle` is its `claude.app`.
    DesktopApp { bundle: PathBuf },
}

/// The `claude` a sign-in would run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub executable: PathBuf,
    pub origin: Origin,
}

/// Whether this platform can open Claude Code's sign-in for the person.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", windows));

/// The `claude` a sign-in would run for the person whose home is `home`,
/// with `env` the environment: found, not yet verified.
pub fn locate(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<Command> {
    locate_with(home, env, None)
}

/// [`locate`] with the system install folders given (`None`: Claude Code's
/// own), so a test never finds this computer's `claude`.
fn locate_with(
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    system: Option<&[PathBuf]>,
) -> Option<Command> {
    installed(home, env, system)
        .map(|executable| Command {
            executable,
            origin: Origin::Installed,
        })
        .or_else(|| {
            cfg!(target_os = "macos")
                .then(|| desktop_app_copy(&crate::claude_desktop::app_dir(home)))
                .flatten()
        })
}

/// Whether Settings may offer the sign-in here: a supported platform with a
/// `claude` to run. Never checks a signature, so it stays cheap enough for
/// every status read; [`start`] checks it.
pub fn available(home: &Path) -> bool {
    SUPPORTED && locate(home, &|key| std::env::var(key).ok()).is_some()
}

/// A `claude` the person installed: on `PATH`, else where Claude Code's
/// installers put it.
fn installed(
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    system: Option<&[PathBuf]>,
) -> Option<PathBuf> {
    use agent_relay_node::relay::claude::ClaudeConfig;
    let name = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    env("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|path| runnable(path))
        .or_else(|| match system {
            Some(system) => ClaudeConfig::resolve(env, home, system).executable,
            None => ClaudeConfig::from_env(env, home).executable,
        })
}

fn runnable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// A version folder's name as numbers, so `2.1.331` sorts after `2.1.99`.
fn version(name: &str) -> Option<Vec<u64>> {
    name.split('.').map(|part| part.parse().ok()).collect()
}

/// The newest copy of Claude Code that Claude's desktop app under `app_dir`
/// keeps and marked verified, owned by this user and writable by no one
/// else.
pub fn desktop_app_copy(app_dir: &Path) -> Option<Command> {
    let root = app_dir.join("claude-code");
    let mut versions: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            Some((version(name.to_str()?)?, entry.path()))
        })
        .collect();
    versions.sort();
    versions.into_iter().rev().find_map(|(_, folder)| {
        let mut copies: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&folder)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|copy| copy.join(".verified").is_file())
            .filter_map(|copy| {
                let bundle = copy.join("claude.app");
                let executable = bundle.join("Contents/MacOS/claude");
                (runnable(&executable) && owned_privately(&bundle) && owned_privately(&executable))
                    .then(|| std::fs::metadata(&copy).and_then(|m| m.modified()).ok())
                    .flatten()
                    .map(|modified| (modified, bundle))
            })
            .collect();
        copies.sort();
        copies.pop().map(|(_, bundle)| Command {
            executable: bundle.join("Contents/MacOS/claude"),
            origin: Origin::DesktopApp { bundle },
        })
    })
}

/// Owned by this user or root, and writable by no one else.
fn owned_privately(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = nix::unistd::getuid().as_raw();
        std::fs::symlink_metadata(path)
            .is_ok_and(|m| (m.uid() == uid || m.uid() == 0) && m.mode() & 0o022 == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

/// Whether the bundle at `bundle` is validly signed by Anthropic as Claude
/// Code ([`ANTHROPIC_REQUIREMENT`]), its sealed resources and executable
/// included.
#[cfg(target_os = "macos")]
pub fn signed_by_anthropic(bundle: &Path) -> bool {
    use security_framework::os::macos::code_signing::{Flags, SecRequirement, SecStaticCode};
    let Some(url) = core_foundation::url::CFURL::from_path(bundle, true) else {
        return false;
    };
    let Ok(code) = SecStaticCode::from_path(&url, Flags::NONE) else {
        return false;
    };
    let Ok(requirement) = ANTHROPIC_REQUIREMENT.parse::<SecRequirement>() else {
        return false;
    };
    code.check_validity(Flags::STRICT_VALIDATE, &requirement)
        .is_ok()
}

#[cfg(not(target_os = "macos"))]
pub fn signed_by_anthropic(_bundle: &Path) -> bool {
    false
}

/// Quote `text` for a POSIX shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The `.command` script that runs `executable auth login` in Terminal,
/// says how it went and removes itself. `CLAUDE_CONFIG_DIR` is cleared, so
/// the sign-in is the default `~/.claude` login cloud sessions read.
pub fn script(executable: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         # Opened by Agent Relay: Claude Code's own sign-in, for Claude Code cloud sessions.\n\
         rm -f \"$0\"\n\
         unset CLAUDE_CONFIG_DIR\n\
         clear\n\
         echo 'Signing in to Claude Code for Agent Relay cloud sessions.'\n\
         echo\n\
         if {} auth login; then\n\
         \x20 echo\n\
         \x20 echo 'Signed in to Claude Code. If macOS asks whether Agent Relay may use Claude Code-credentials, choose Always Allow.'\n\
         \x20 echo 'You can close this window.'\n\
         else\n\
         \x20 echo\n\
         \x20 echo 'Claude Code sign-in did not finish. Try Sign in to Claude Code again from Agent Relay Settings.'\n\
         fi\n",
        shell_quote(&executable.to_string_lossy())
    )
}

/// Opens Claude Code's sign-in for the person. `Err` is the plain reason
/// it did not open.
pub fn start(home: &Path) -> Result<(), String> {
    if !SUPPORTED {
        return Err(UNAVAILABLE.to_string());
    }
    let command =
        locate(home, &|key| std::env::var(key).ok()).ok_or_else(|| NOT_FOUND.to_string())?;
    if let Origin::DesktopApp { bundle } = &command.origin {
        if !signed_by_anthropic(bundle) {
            return Err(NOT_VERIFIED.to_string());
        }
    }
    open(&command.executable)
}

#[cfg(target_os = "macos")]
fn open(executable: &Path) -> Result<(), String> {
    let path = write_script(executable).map_err(|_| NOT_STARTED.to_string())?;
    let opened = std::process::Command::new("/usr/bin/open")
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !opened {
        let _ = std::fs::remove_file(&path);
        return Err(NOT_STARTED.to_string());
    }
    watch_for_login();
    Ok(())
}

/// Writes the script, owner-only, in a private folder of this user's
/// temporary directory.
#[cfg(target_os = "macos")]
fn write_script(executable: &Path) -> std::io::Result<PathBuf> {
    use agent_relay_node::platform::fs as private_fs;
    use std::io::Write;
    let dir = std::env::temp_dir().join("agent-relay-claude-sign-in");
    private_fs::create_private_dir_all(&dir)?;
    private_fs::set_mode(&dir, 0o700)?;
    let path = dir.join(format!(
        "Claude Code sign-in {}.command",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut file = private_fs::private(std::fs::OpenOptions::new().write(true).create_new(true))
        .open(&path)?;
    file.write_all(script(executable).as_bytes())?;
    file.sync_all()?;
    drop(file);
    private_fs::set_mode(&path, 0o700)?;
    Ok(path)
}

/// Waits, on a thread of its own, for Claude Code's Keychain item, then
/// asks for access to it as turning cloud sessions on does. One watch at a
/// time.
#[cfg(target_os = "macos")]
fn watch_for_login() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WATCHING: AtomicBool = AtomicBool::new(false);
    if WATCHING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("claude-sign-in-watch".into())
        .spawn(|| {
            let deadline = std::time::Instant::now() + WATCH_FOR;
            while std::time::Instant::now() < deadline {
                if crate::claude_keychain::present() {
                    crate::claude_keychain::ask_access(
                        agent_relay_node::relay::claude_dirs::cloud_sessions_on_here,
                    );
                    break;
                }
                std::thread::sleep(WATCH_EVERY);
            }
            WATCHING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        WATCHING.store(false, Ordering::Release);
    }
}

/// A console of its own, so the person sees and answers the sign-in.
#[cfg(windows)]
fn open(executable: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    std::process::Command::new(executable)
        .args(["auth", "login"])
        .env_remove("CLAUDE_CONFIG_DIR")
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(drop)
        .map_err(|_| NOT_STARTED.to_string())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn open(_executable: &Path) -> Result<(), String> {
    Err(UNAVAILABLE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn copy(app: &Path, version: &str, hash: &str, verified: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let folder = app.join("claude-code").join(version).join(hash);
        let bundle = folder.join("claude.app");
        executable(&bundle.join("Contents/MacOS/claude"));
        std::fs::set_permissions(&bundle, std::fs::Permissions::from_mode(0o755)).unwrap();
        if verified {
            std::fs::write(folder.join(".verified"), "x").unwrap();
        }
        bundle
    }

    #[cfg(unix)]
    #[test]
    fn the_newest_verified_desktop_app_copy_is_chosen() {
        let app = tempfile::tempdir().unwrap();
        copy(app.path(), "2.1.99", "old", true);
        let newest = copy(app.path(), "2.1.331", "good", true);
        copy(app.path(), "2.1.400", "unverified", false);
        std::fs::create_dir_all(app.path().join("claude-code/not-a-version/x")).unwrap();
        let found = desktop_app_copy(app.path()).unwrap();
        assert_eq!(found.executable, newest.join("Contents/MacOS/claude"));
        assert_eq!(found.origin, Origin::DesktopApp { bundle: newest });
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_others_can_write_is_not_run() {
        use std::os::unix::fs::PermissionsExt;
        let app = tempfile::tempdir().unwrap();
        let older = copy(app.path(), "2.1.1", "a", true);
        let shared = copy(app.path(), "2.1.2", "b", true);
        std::fs::set_permissions(
            shared.join("Contents/MacOS/claude"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert_eq!(
            desktop_app_copy(app.path()).map(|found| found.origin),
            Some(Origin::DesktopApp { bundle: older })
        );
        assert_eq!(desktop_app_copy(&app.path().join("absent")), None);
    }

    #[cfg(unix)]
    #[test]
    fn an_installed_claude_comes_before_the_desktop_apps() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        executable(&bin.join("claude"));
        let path = bin.to_string_lossy().to_string();
        let env = |key: &str| (key == "PATH").then(|| path.clone());
        assert_eq!(
            locate_with(home.path(), &env, Some(&[])),
            Some(Command {
                executable: bin.join("claude"),
                origin: Origin::Installed,
            })
        );
        // Where Claude Code's installer puts it, with nothing on PATH.
        executable(&home.path().join(".local/bin/claude"));
        let none = |_: &str| None;
        assert_eq!(
            locate_with(home.path(), &none, Some(&[])).map(|found| found.executable),
            Some(home.path().join(".local/bin/claude"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_desktop_apps_copy_is_used_without_an_installed_claude() {
        let home = tempfile::tempdir().unwrap();
        let bundle = copy(
            &crate::claude_desktop::app_dir(home.path()),
            "2.1.331",
            "5cdc5c10ac5c",
            true,
        );
        let none = |_: &str| None;
        assert_eq!(
            locate_with(home.path(), &none, Some(&[])).map(|found| found.origin),
            Some(Origin::DesktopApp {
                bundle: bundle.clone()
            })
        );
        // A copy that is not Anthropic's signed bundle is never run.
        assert!(!signed_by_anthropic(&bundle));
    }

    #[test]
    fn the_script_runs_claude_code_login_and_removes_itself() {
        let script = script(Path::new(
            "/Users/me/Library/Application Support/Claude/it's/claude",
        ));
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("rm -f \"$0\"\n"));
        assert!(script.contains("unset CLAUDE_CONFIG_DIR\n"));
        assert!(script.contains(
            "if '/Users/me/Library/Application Support/Claude/it'\\''s/claude' auth login; then"
        ));
        assert!(!script.contains("security "));
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn linux_reports_the_sign_in_unavailable() {
        let home = tempfile::tempdir().unwrap();
        assert!(!available(home.path()));
        assert_eq!(start(home.path()), Err(UNAVAILABLE.to_string()));
    }
}
