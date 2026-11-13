//! Whether the executable a long-running process started from is still the
//! one on disk.
//!
//! An update can replace the app without stopping what it started: a manual
//! drag-install of a new app over the old one leaves `serve` and the detached
//! session collector running the old build's image, and the collector keeps
//! uploading under the old build's rules. The version string alone cannot
//! tell (a hand-installed candidate can carry the version already
//! installed), so this compares the file itself: its device and inode where
//! the platform has them, and its size and modification time everywhere.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
    time::SystemTime,
};

/// The identity of an executable file, as captured when a process starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutableIdentity {
    len: u64,
    modified: Option<SystemTime>,
    /// Device and inode. A replaced file is a new inode even when its size
    /// and time happen to match.
    #[cfg(unix)]
    file: (u64, u64),
}

impl ExecutableIdentity {
    /// The identity of the file at `path`, or `None` when it is gone or
    /// unreadable.
    pub fn of(path: &Path) -> Option<Self> {
        Self::read(path).ok()
    }

    fn read(path: &Path) -> std::io::Result<Self> {
        let metadata = fs::metadata(path)?;
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            file,
        })
    }
}

/// The suffix Linux gives `/proc/self/exe` once the running file was
/// unlinked, as an installer replacing it does.
const UNLINKED_SUFFIX: &str = " (deleted)";

/// The executable this process runs, captured at process start: the
/// installation path to compare against, and the identity of the image
/// that is running.
#[derive(Clone, Debug)]
pub struct StartedExecutable {
    path: PathBuf,
    /// The running image. Unset when it could not be read at start for a
    /// reason other than being gone; the first later read records it.
    identity: OnceLock<ExecutableIdentity>,
    /// The running file was already unlinked when it was captured.
    unlinked: bool,
}

impl StartedExecutable {
    /// This process's own executable, as captured when it started
    /// ([`capture_at_start`]). `None` when the platform cannot name it.
    pub fn current() -> Option<Self> {
        at_start().cloned()
    }

    /// The executable the platform names `named`, with `image` the identity
    /// of the running image where the platform can read it apart from the
    /// path (Linux's `/proc/self/exe`, which still reads once unlinked).
    fn running(named: PathBuf, image: Option<std::io::Result<ExecutableIdentity>>) -> Self {
        let (path, mut unlinked) = installation_path(named);
        let identity = OnceLock::new();
        match image.unwrap_or_else(|| ExecutableIdentity::read(&path)) {
            Ok(found) => {
                let _ = identity.set(found);
            }
            // Gone before it could be read: it was replaced.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => unlinked = true,
            Err(_) => {}
        }
        Self {
            path,
            identity,
            unlinked,
        }
    }

    /// The executable at `path`, as it is now.
    pub fn at(path: PathBuf) -> Self {
        Self::running(path, None)
    }

    /// Whether the file this process started from has since been replaced
    /// or removed. One already unlinked at start counts as replaced; one
    /// whose identity could not be read at start is recorded by the first
    /// read that succeeds.
    pub fn replaced(&self) -> bool {
        if self.unlinked {
            return true;
        }
        let now = ExecutableIdentity::of(&self.path);
        match (self.identity.get(), now) {
            (Some(started), now) => now.as_ref() != Some(started),
            (None, Some(now)) => {
                let _ = self.identity.set(now);
                false
            }
            (None, None) => false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The installation path for the executable the platform names `named`,
/// and whether that name says the running file was already unlinked.
fn installation_path(named: PathBuf) -> (PathBuf, bool) {
    match named
        .to_str()
        .and_then(|name| name.strip_suffix(UNLINKED_SUFFIX))
    {
        Some(installed) => (PathBuf::from(installed), true),
        None => (named, false),
    }
}

/// The identity of the running image, read apart from its path where the
/// platform allows: Linux keeps `/proc/self/exe` readable after the file
/// was unlinked or replaced.
fn running_image() -> Option<std::io::Result<ExecutableIdentity>> {
    #[cfg(target_os = "linux")]
    {
        Some(ExecutableIdentity::read(Path::new("/proc/self/exe")))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// This process's executable as captured at start, the first time anything
/// asks ([`capture_at_start`] asks first thing in `main`).
fn at_start() -> Option<&'static StartedExecutable> {
    static STARTED: OnceLock<Option<StartedExecutable>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            let named = std::env::current_exe().ok()?;
            Some(StartedExecutable::running(named, running_image()))
        })
        .as_ref()
}

/// Records this process's executable before any startup work, so a
/// replacement during a slow start (account discovery, a store migration)
/// is still seen.
pub fn capture_at_start() {
    let _ = at_start();
    #[cfg(target_os = "macos")]
    let _ = bundle_at_start();
}

/// Whether a service manager starts this process again after it exits:
/// the core's launchd job on macOS (`KeepAlive`) and its systemd user unit on
/// Linux (`Restart=always`). Only then may a core whose executable was
/// replaced exit so the new build takes over. The Linux app can host the
/// core itself when the unit is off, and the Windows logon task does not
/// restart a core that exits, so there a replaced core keeps running until
/// the app's version check restarts it.
pub fn restarted_by_service_manager() -> bool {
    #[cfg(target_os = "macos")]
    {
        launchd_core_job(
            std::env::var("XPC_SERVICE_NAME").ok().as_deref(),
            bundle_at_start(),
        )
    }
    #[cfg(target_os = "linux")]
    {
        systemd_core_unit(&fs::read_to_string("/proc/self/cgroup").unwrap_or_default())
    }
    #[cfg(windows)]
    {
        false
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        false
    }
}

/// The core's check, each supervision pass, for its own executable having
/// been replaced: a manual install of a new app replaces it without stopping
/// it, and nothing else tells a core it is the old build. It compares with
/// the executable captured at process start, and the first call that finds
/// it replaced acts once ([`leave_for_replaced_executable`]).
pub fn leave_if_replaced() {
    static LEFT: AtomicBool = AtomicBool::new(false);
    let replaced = at_start().is_some_and(StartedExecutable::replaced);
    if replaced && !LEFT.swap(true, Ordering::SeqCst) {
        leave_for_replaced_executable();
    }
}

/// What a core does once its executable was replaced: where a service
/// manager restarts it, it asks itself to stop (SIGTERM, its normal stop)
/// so the new build takes over; elsewhere it says so and keeps serving.
fn leave_for_replaced_executable() {
    if restarted_by_service_manager() {
        eprintln!(
            "Agent Relay was updated on disk; stopping so the service manager starts the new build."
        );
        #[cfg(unix)]
        let _ = nix::sys::signal::raise(nix::sys::signal::Signal::SIGTERM);
    } else {
        eprintln!("Agent Relay was updated on disk; restart Agent Relay to run the new build.");
    }
}

/// Whether launchd's `XPC_SERVICE_NAME` names one of the core's jobs for the
/// app `bundle` (its `CFBundleIdentifier`): the KeepAlive job
/// `<bundle>.core` the app registers, or its `<bundle>.core.local` and
/// `<bundle>.core.session` fallbacks. Every site build has its own bundle
/// identifier, so the labels come from it, not from Agent Relay's own.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn launchd_core_job(service: Option<&str>, bundle: Option<&str>) -> bool {
    let (Some(label), Some(bundle)) = (service, bundle) else {
        return false;
    };
    label
        .strip_prefix(bundle)
        .is_some_and(|job| matches!(job, ".core" | ".core.local" | ".core.session"))
}

/// The identifier of the app bundle this process runs from, read once at
/// process start ([`capture_at_start`]), while the bundle it started from is
/// still in place. `None` outside an app bundle, as for the command-line
/// copy.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn bundle_at_start() -> Option<&'static str> {
    static BUNDLE: OnceLock<Option<String>> = OnceLock::new();
    BUNDLE
        .get_or_init(|| bundle_identifier(&std::env::current_exe().ok()?))
        .as_deref()
}

/// The `CFBundleIdentifier` of the `.app` bundle holding `executable`, as
/// its `Contents/Info.plist` (XML, as the build writes it) names it.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn bundle_identifier(executable: &Path) -> Option<String> {
    let bundle = executable
        .ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))?;
    let info = fs::read_to_string(bundle.join("Contents/Info.plist")).ok()?;
    let after_key = info.split("<key>CFBundleIdentifier</key>").nth(1)?;
    let value = after_key.trim_start().strip_prefix("<string>")?;
    let identifier = value.split("</string>").next()?.trim();
    (!identifier.is_empty()).then(|| identifier.to_string())
}

/// Whether `/proc/self/cgroup` puts this process in the core's systemd user
/// unit. The unit is `agent-relay.service` for every product
/// (`linux/install.sh`), so its name is fixed.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn systemd_core_unit(cgroup: &str) -> bool {
    cgroup
        .lines()
        .any(|line| line.trim_end().ends_with("/agent-relay.service"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_cores_launchd_job_is_restarted_by_launchd() {
        let ours = Some("com.agentrelay.desktop");
        assert!(launchd_core_job(Some("com.agentrelay.desktop.core"), ours));
        assert!(launchd_core_job(
            Some("com.agentrelay.desktop.core.local"),
            ours
        ));
        assert!(launchd_core_job(
            Some("com.agentrelay.desktop.core.session"),
            ours
        ));
        assert!(!launchd_core_job(
            Some("application.com.agentrelay.desktop.1146469350.1146469364"),
            ours
        ));
        // Another build's core, and jobs that only look like one.
        assert!(!launchd_core_job(
            Some("com.agentrelay.desktop.dev.core"),
            ours
        ));
        assert!(!launchd_core_job(
            Some("com.agentrelay.desktop.core.extra"),
            ours
        ));
        assert!(!launchd_core_job(
            Some("com.agentrelay.scrubber-eval.ruleset10"),
            ours
        ));
        assert!(!launchd_core_job(None, ours));
        // Outside an app bundle there is no core job.
        assert!(!launchd_core_job(Some("com.agentrelay.desktop.core"), None));
    }

    /// A site build has its own bundle identifier, and so its own labels.
    #[test]
    fn a_site_builds_core_job_is_restarted_by_launchd() {
        let site = Some("com.example.desktop");
        assert!(launchd_core_job(Some("com.example.desktop.core"), site));
        assert!(launchd_core_job(
            Some("com.example.desktop.core.local"),
            site
        ));
        assert!(launchd_core_job(
            Some("com.example.desktop.core.session"),
            site
        ));
        assert!(!launchd_core_job(Some("com.agentrelay.desktop.core"), site));
    }

    #[test]
    fn the_bundle_identifier_comes_from_the_apps_info_plist() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("Example Desktop.app/Contents");
        fs::create_dir_all(contents.join("Helpers")).unwrap();
        fs::write(
            contents.join("Info.plist"),
            "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>CFBundleName</key>\n\t<string>Example</string>\n\t<key>CFBundleIdentifier</key>\n\t<string>com.example.desktop</string>\n</dict>\n</plist>\n",
        )
        .unwrap();
        let helper = contents.join("Helpers/Agent Relay Helper");
        assert_eq!(
            bundle_identifier(&helper).as_deref(),
            Some("com.example.desktop")
        );
        assert_eq!(
            bundle_identifier(&dir.path().join("agent-relay-probe")),
            None
        );
    }

    #[test]
    fn only_the_cores_systemd_unit_is_restarted_by_systemd() {
        assert!(systemd_core_unit(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/agent-relay.service\n"
        ));
        // The GTK app hosting the core in its own scope is not restarted.
        assert!(!systemd_core_unit(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-agent-relay-4242.scope\n"
        ));
        assert!(!systemd_core_unit(""));
    }

    #[test]
    fn an_unchanged_executable_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper");
        fs::write(&path, b"build one").unwrap();
        let started = StartedExecutable::at(path);
        assert!(!started.replaced());
    }

    #[test]
    fn a_file_installed_over_the_executable_is_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper");
        fs::write(&path, b"build one").unwrap();
        let started = StartedExecutable::at(path.clone());
        // A drag-install removes the old bundle and puts the new one at the
        // same path: a new file.
        let staged = dir.path().join("staged");
        fs::write(&staged, b"build two, larger").unwrap();
        fs::rename(&staged, &path).unwrap();
        assert!(started.replaced());
    }

    #[test]
    fn a_removed_executable_is_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper");
        fs::write(&path, b"build one").unwrap();
        let started = StartedExecutable::at(path.clone());
        fs::remove_file(&path).unwrap();
        assert!(started.replaced());
    }

    /// Linux names an unlinked running file `<path> (deleted)`: an installer
    /// replaced it before the capture. That is a replacement, compared at
    /// the installation path, never detection turned off.
    #[test]
    fn an_executable_already_unlinked_at_start_is_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper");
        fs::write(&path, b"new build").unwrap();
        let named = PathBuf::from(format!("{}{UNLINKED_SUFFIX}", path.display()));
        let started = StartedExecutable::running(named, None);
        assert_eq!(started.path(), path);
        assert!(started.replaced());
    }

    /// Linux reads the running image through `/proc/self/exe` even once the
    /// file at the path was replaced: the image, not the new file, is what
    /// the path is compared against.
    #[test]
    fn the_running_image_is_compared_with_the_installed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper");
        fs::write(&path, b"old build").unwrap();
        let image = ExecutableIdentity::read(&path);
        let staged = dir.path().join("staged");
        fs::write(&staged, b"new build, installed").unwrap();
        fs::rename(&staged, &path).unwrap();
        let started = StartedExecutable::running(path.clone(), Some(image));
        assert!(started.replaced());

        let unchanged =
            StartedExecutable::running(path.clone(), Some(ExecutableIdentity::read(&path)));
        assert!(!unchanged.replaced());
    }

    /// A running file already gone when it is captured was replaced.
    #[test]
    fn a_missing_executable_at_start_is_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let started = StartedExecutable::at(dir.path().join("gone"));
        assert!(started.replaced());
    }

    /// An identity that could not be read at start for another reason does
    /// not turn detection off: the first later read records it, and a
    /// replacement after that is seen.
    #[test]
    fn a_failed_identity_read_at_start_is_recorded_later() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper");
        fs::write(&path, b"build one").unwrap();
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let started = StartedExecutable::running(path.clone(), Some(Err(denied)));
        assert!(!started.replaced(), "the first read records the identity");
        assert!(!started.replaced());
        let staged = dir.path().join("staged");
        fs::write(&staged, b"build two, larger").unwrap();
        fs::rename(&staged, &path).unwrap();
        assert!(started.replaced());
    }
}
