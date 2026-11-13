//! Software updates: the Linux counterpart of the Mac's `AppUpdate`.
//!
//! Both apps read the same public GitHub release, from the repository the
//! build's site profile names (`build_profile::RELEASES_REPOSITORY`), whose
//! artifacts start with its prefix (`ASSET_PREFIX`). A per-user install (the
//! checksum-verified tarball under `~/.local/lib/agent-relay/<version>`)
//! updates itself the same way the setup skill installs it: download, verify
//! the published SHA-256, unpack beside the running version, point the
//! systemd user unit at it and restart the unit if it is running. A `.deb` install needs the system
//! package manager, so the app offers the verified package instead of
//! installing it.
//!
//! On Windows (`updates/windows.rs`) a per-user install under
//! `%LOCALAPPDATA%\Programs\AgentRelay\<version>` updates itself from the
//! same release's verified `.zip`, points the logon task at the new version
//! and restarts it if it is running; its "launcher entry" is a Start menu
//! shortcut and the `agentrelay://` link handler.
//!
//! A build with a `cloud` release feed reads its site's signed feed instead
//! (`updates/cloud.rs`), with the signed-in account's headers, and checks
//! each download against the verified manifest's size and SHA-256.
//!
//! The network, the environment and the programs run are reached through a
//! [`Host`], so tests use fakes for all of them.

mod channels;
pub mod cloud;
/// The signed release feed's verifier, shared with `agent-relay-probe feed
/// verify` (`desktop-core/src/feed`).
pub use agent_relay_desktop_core::feed;
/// Whether one release version is later than another (shared with the feed's
/// verifier).
pub use agent_relay_desktop_core::feed::is_newer;
use agent_relay_desktop_core::feed::release_version as version;
mod http;
use crate::build_profile::Profile;
pub use channels::latest_for_channel;
use channels::{latest_in, parse_release_for_channel};
pub use cloud::{CloudFeed, FeedRelease};
pub use http::{fetch, request, FeedRequest, FeedResponse, Request};

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{fmt, fs};

use serde_json::Value;
use sha2::{Digest, Sha256};

#[cfg(unix)]
use super::install::{self, InstallKind};

/// How long a request may wait for the network at any one step.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// A release tarball is tens of megabytes; anything far larger is not one.
pub const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
/// The line that marks a launcher entry this app wrote.
pub const LAUNCHER_MARKER: &str = "X-AgentRelay-Managed=true";
#[cfg(unix)]
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(unix)]
const LAUNCHER_DATABASE_TIMEOUT: Duration = Duration::from_secs(10);

const ESCAPES: &str = "The release archive contains a path outside itself.";
const NOT_A_BUILD: &str = "The release archive is not an Agent Relay build.";
const TOO_LARGE: &str = "The download is larger than any Agent Relay release.";
const UNREADABLE: &str =
    "Agent Relay could not read the latest release information. Try again later.";
const NO_COMPATIBLE_UPDATE: &str = "The latest release does not include a compatible update.";
const UNVERIFIED: &str = "The downloaded update could not be verified and was discarded.";
/// A sentence safe to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateError(String);

impl UpdateError {
    pub fn new(sentence: impl Into<String>) -> UpdateError {
        UpdateError(sentence.into())
    }

    /// A local file operation that failed, naming the file.
    fn io(path: &Path) -> impl FnOnce(io::Error) -> UpdateError + '_ {
        move |error| UpdateError(format!("{error}: {}", path.display()))
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for UpdateError {}

/// Why a download produced no bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// More than [`MAX_DOWNLOAD`] bytes.
    TooLarge,
    /// No answer, or an HTTP error; the detail is for logs.
    Unreachable(String),
}

/// What a finished program reported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandOutput {
    /// The exit code; `None` when a signal ended the program.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Fetches a URL's body, at most [`MAX_DOWNLOAD`] bytes.
pub type Fetch<'a> = dyn Fn(&str) -> Result<Vec<u8>, FetchError> + 'a;
/// Runs a program (`argv[0]`) to completion within a timeout.
pub type Run<'a> = dyn Fn(&[&str], Duration) -> io::Result<CommandOutput> + 'a;
/// Reads one environment variable.
pub type Env<'a> = dyn Fn(&str) -> Option<OsString> + 'a;

/// The person's home and what the updater reaches outside the process.
pub struct Host<'a> {
    pub home: PathBuf,
    pub env: &'a Env<'a>,
    pub fetch: &'a Fetch<'a>,
    pub run: &'a Run<'a>,
    /// Requests to the site's release feed, with its headers.
    pub request: &'a Request<'a>,
    /// The signed-in site's release feed, for a build that reads one.
    pub feed: Option<&'a CloudFeed<'a>>,
}

impl Host<'static> {
    /// This process's environment, the network and real programs.
    pub fn system() -> io::Result<Host<'static>> {
        Ok(Host {
            home: agent_relay_node::relay::paths::home().map_err(io::Error::other)?,
            env: &|name| std::env::var_os(name),
            fetch: &fetch,
            run: &run,
            request: &request,
            feed: None,
        })
    }
}

impl Host<'_> {
    fn var(&self, name: &str) -> Option<OsString> {
        (self.env)(name).filter(|value| !value.is_empty())
    }
}

/// Runs `argv` with its output captured. A program still running at
/// `timeout` is killed and reported as [`io::ErrorKind::TimedOut`]; one that
/// is not installed is [`io::ErrorKind::NotFound`].
/// A program's output as text. Programs print UTF-8 on Unix; on Windows a
/// console program printing through a pipe may write UTF-16 (`schtasks
/// /Query /XML` does), which is told by its byte-order mark or, without
/// one, by its NUL bytes: UTF-8 text printed by a program never holds a NUL,
/// while UTF-16 holds one in nearly every character, so a path in any
/// script comes through intact.
pub fn console_text(data: &[u8]) -> String {
    let utf16 = data.len() >= 2
        && (data.starts_with(&[0xFF, 0xFE]) || (data.len() % 2 == 0 && data.contains(&0)));
    if utf16 {
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
            .trim_start_matches('\u{feff}')
            .to_owned()
    } else {
        String::from_utf8_lossy(data).into_owned()
    }
}

pub fn run(argv: &[&str], timeout: Duration) -> io::Result<CommandOutput> {
    let (program, arguments) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no program to run"))?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A console program run from the window opens no console of its own.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows::CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let mut data = Vec::new();
                let _ = pipe.read_to_end(&mut data);
                text = console_text(&data);
            }
            text
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{program} did not finish within {} seconds",
                    timeout.as_secs()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(CommandOutput {
        code: status.code(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub tarball_url: String,
    pub tarball_checksum_url: String,
    pub deb_url: Option<String>,
    pub deb_checksum_url: Option<String>,
    pub page_url: String,
    /// The signed feed's assets, for a release from the site's feed.
    pub feed: Option<FeedRelease>,
}

/// The release architecture name for a machine name (`uname -m`).
pub fn architecture(machine: &str) -> Result<&'static str, UpdateError> {
    match machine.to_lowercase().as_str() {
        "x86_64" | "amd64" => Ok("x64"),
        "aarch64" | "arm64" => Ok("arm64"),
        other => Err(UpdateError(format!(
            "Agent Relay has no {} build for {other}.",
            Platform::CURRENT.name()
        ))),
    }
}

/// Which platform's release assets the app installs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// `<prefix>-<version>-Linux-<arch>.tar.gz`, and the `.deb`.
    Linux,
    /// `<prefix>-<version>-Windows-<arch>.zip`.
    Windows,
}

impl Platform {
    #[cfg(not(windows))]
    pub const CURRENT: Platform = Platform::Linux;
    #[cfg(windows)]
    pub const CURRENT: Platform = Platform::Windows;

    /// The platform's name in asset names and sentences.
    pub fn name(self) -> &'static str {
        match self {
            Platform::Linux => "Linux",
            Platform::Windows => "Windows",
        }
    }

    /// The per-user archive's extension.
    pub fn archive(self) -> &'static str {
        match self {
            Platform::Linux => "tar.gz",
            Platform::Windows => "zip",
        }
    }
}

/// The release architecture this app was built for.
pub fn current_architecture() -> Result<&'static str, UpdateError> {
    architecture(std::env::consts::ARCH)
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(value) if super::truthy(value) => value.to_string(),
        _ => String::new(),
    }
}

/// Whether `url` is an `https` link to `github.com` itself.
fn on_github(url: &str) -> bool {
    url::Url::parse(url)
        .is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some("github.com"))
}

/// The published release for `arch`, from the GitHub API's JSON.
pub fn parse_release(data: &[u8], arch: &str) -> Result<Release, UpdateError> {
    parse_release_for(data, arch, Platform::CURRENT)
}

/// [`parse_release`] for `platform`'s assets. On Windows the per-user
/// archive (`tarball_url`) is the `.zip`, and there is no package.
pub fn parse_release_for(
    data: &[u8],
    arch: &str,
    platform: Platform,
) -> Result<Release, UpdateError> {
    parse_release_for_channel(&Profile::BUILT, data, arch, platform, false)
}

/// The digest from `<digest>  <filename>`, as the release publishes it.
pub fn checksum(text: &str) -> Option<String> {
    let token = text.split_whitespace().next().unwrap_or("").to_lowercase();
    (token.len() == 64
        && token
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
    .then_some(token)
}

/// The latest published release for `arch`.
pub fn latest(arch: &str, fetch: &Fetch<'_>) -> Result<Release, UpdateError> {
    latest_in(&Profile::BUILT, arch, fetch)
}

/// Whether an existing launcher entry is this app's to rewrite: one it wrote
/// (with the marker), or an unedited copy of the packaged entry, which runs a
/// bare launcher name. Anything else is the person's and is kept.
#[cfg(unix)]
fn ours(entry: &str) -> bool {
    if entry.contains(LAUNCHER_MARKER) {
        return true;
    }
    let commands: Vec<&str> = entry
        .split('\n')
        .filter_map(|line| line.strip_prefix("Exec="))
        .collect();
    commands == ["agent-relay %u"] || commands == ["agent-relay-desktop %u"]
}

/// `$XDG_DATA_HOME/applications/agent-relay.desktop`, else under
/// `~/.local/share`.
#[cfg(unix)]
fn launcher_entry_path(host: &Host<'_>) -> PathBuf {
    let data = host
        .var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|data| data.is_absolute())
        .unwrap_or_else(|| host.home.join(".local/share"));
    data.join("applications/agent-relay.desktop")
}

/// Asks the desktop to re-read its launcher entries. A slow or missing
/// `update-desktop-database` only delays that; it never fails an install.
#[cfg(unix)]
fn refresh_launcher_database(directory: &Path, host: &Host<'_>) {
    let directory = directory.to_string_lossy();
    let _ = (host.run)(
        &["update-desktop-database", &directory],
        LAUNCHER_DATABASE_TIMEOUT,
    );
}

/// The `Exec=` command for `path`, per the Desktop Entry spec: quoted when
/// it holds whitespace or a reserved character (with `"`, `` ` ``, `$` and
/// `\` escaped inside the quotes), then escaped as a string value
/// (backslashes doubled) with `%` written as `%%`.
#[cfg(unix)]
fn exec_command(path: &Path) -> String {
    const RESERVED: &str = "\"'\\><~|&;$*?#()`";
    let command = path.to_string_lossy();
    let argument = if command
        .chars()
        .any(|c| c.is_whitespace() || RESERVED.contains(c))
    {
        let mut quoted = String::from("\"");
        for c in command.chars() {
            if "\"`$\\".contains(c) {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        quoted
    } else {
        command.into_owned()
    };
    let mut value = String::with_capacity(argument.len());
    for c in argument.chars() {
        match c {
            '\\' => value.push_str("\\\\"),
            '\n' => value.push_str("\\n"),
            '\t' => value.push_str("\\t"),
            '\r' => value.push_str("\\r"),
            '%' => value.push_str("%%"),
            c => value.push(c),
        }
    }
    value
}

/// Replaces each `^{key}\S+` line start (`Exec=agent-relay %u`) with
/// `{key}{value}`, keeping the rest of the line.
#[cfg(unix)]
fn replace_line_starts(text: &str, key: &str, value: &str) -> String {
    text.split_inclusive('\n')
        .map(|line| match line.strip_prefix(key) {
            Some(rest) if rest.starts_with(|c: char| !c.is_whitespace()) => {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                format!("{key}{value}{}", &rest[end..])
            }
            _ => line.to_owned(),
        })
        .collect()
}

/// Writes `data` beside `path` and renames it into place, mode 0644.
#[cfg(unix)]
fn write_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, data)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644))?;
    fs::rename(&temporary, path)
}

/// Lists a per-user install in the desktop's app launcher: writes
/// `~/.local/share/applications/agent-relay.desktop` running `tree`'s app by
/// its full path, and the icon it names. An entry someone else wrote is left
/// alone. Returns the entry, or `None` when nothing was written.
#[cfg(unix)]
pub fn install_launcher(tree: &Path, host: &Host<'_>) -> io::Result<Option<PathBuf>> {
    let template = tree.join(install::LAUNCHER_ENTRY);
    let launcher = tree.join(install::APP);
    if !template.exists() || !launcher.exists() {
        return Ok(None);
    }
    let entry_path = launcher_entry_path(host);
    let existing = match fs::read_to_string(&entry_path) {
        Ok(existing) => Some(existing),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::InvalidData
            ) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    if existing.as_deref().is_some_and(|existing| !ours(existing)) {
        return Ok(None);
    }
    let mut entry = replace_line_starts(
        &fs::read_to_string(&template)?,
        "Exec=",
        &exec_command(&launcher),
    );
    if !entry.contains(LAUNCHER_MARKER) {
        entry = format!("{}\n{LAUNCHER_MARKER}\n", entry.trim_end_matches('\n'));
    }
    let icon = tree.join(install::ICON);
    if icon.exists() {
        let data = entry_path
            .parent()
            .and_then(Path::parent)
            .unwrap_or(Path::new("/"));
        let icon_path = data.join("icons/hicolor/scalable/apps/agent-relay.svg");
        let bytes = fs::read(&icon)?;
        if fs::read(&icon_path).ok().as_deref() != Some(&bytes[..]) {
            fs::create_dir_all(icon_path.parent().unwrap_or(data))?;
            fs::write(&icon_path, bytes)?;
        }
    }
    if existing.as_deref() == Some(entry.as_str()) {
        return Ok(Some(entry_path));
    }
    write_atomically(&entry_path, entry.as_bytes())?;
    if let Some(directory) = entry_path.parent() {
        refresh_launcher_database(directory, host);
    }
    Ok(Some(entry_path))
}

/// Removes the per-user launcher entry this app wrote, so a system package's
/// own entry, which it would shadow, is the one listed. True when removed.
#[cfg(unix)]
pub fn remove_launcher(host: &Host<'_>) -> io::Result<bool> {
    let entry_path = launcher_entry_path(host);
    let Ok(entry) = fs::read_to_string(&entry_path) else {
        return Ok(false);
    };
    if !entry.contains(LAUNCHER_MARKER) {
        return Ok(false);
    }
    fs::remove_file(&entry_path)?;
    if let Some(directory) = entry_path.parent() {
        refresh_launcher_database(directory, host);
    }
    Ok(true)
}

/// Keeps the launcher entry on the running per-user install, so it follows
/// updates and installs made without the in-app updater. Running from the
/// system package, the per-user entry a former per-user install left would
/// shadow the package's, so it is removed. Launch at login, when it is on,
/// follows an installed app (best effort). A source build changes nothing.
#[cfg(unix)]
pub fn ensure_launcher(exe: &Path, host: &Host<'_>) -> io::Result<Option<PathBuf>> {
    let _ = agent_relay_desktop_core::login_item::follow_installed_app(&host.home, host.env, exe);
    match install::install_kind(exe) {
        InstallKind::Deb => {
            remove_launcher(host)?;
            Ok(None)
        }
        InstallKind::Source => Ok(None),
        InstallKind::Tarball => match install::install_tree(exe) {
            Some(tree) => install_launcher(tree, host),
            None => Ok(None),
        },
    }
}

/// `agent-relay-desktop --install-launcher`: lists a per-user install in the app
/// launcher. Returns the exit code and the line to print (stdout on 0,
/// stderr otherwise).
#[cfg(unix)]
pub fn install_launcher_command(exe: &Path, host: &Host<'_>) -> (i32, String) {
    if install::install_kind(exe) != InstallKind::Tarball {
        return (
            2,
            "Only a per-user install (~/.local/lib/agent-relay) needs this; a package lists itself."
                .to_owned(),
        );
    }
    match ensure_launcher(exe, host) {
        Ok(Some(entry)) => (0, entry.display().to_string()),
        Ok(None) | Err(_) => (
            1,
            "Agent Relay could not write its launcher entry; one written by someone else is kept."
                .to_owned(),
        ),
    }
}

fn download_error(error: FetchError) -> UpdateError {
    match error {
        FetchError::TooLarge => UpdateError::new(TOO_LARGE),
        FetchError::Unreachable(_) => UpdateError::new(
            "Agent Relay could not download the update. Check your connection and try again.",
        ),
    }
}

fn verified_download(
    url: &str,
    checksum_url: &str,
    fetch: &Fetch<'_>,
) -> Result<Vec<u8>, UpdateError> {
    let published = fetch(checksum_url).map_err(download_error)?;
    let expected = checksum(&String::from_utf8_lossy(&published))
        .ok_or_else(|| UpdateError::new(UNVERIFIED))?;
    let data = fetch(url).map_err(download_error)?;
    let actual: String = Sha256::digest(&data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err(UpdateError::new(UNVERIFIED));
    }
    Ok(data)
}

/// The archive in `data`: gzip-compressed or plain. What it unpacks is
/// never writable by group or others, and never setuid or setgid.
#[cfg(unix)]
fn archive(data: &[u8]) -> tar::Archive<Box<dyn Read + '_>> {
    let reader: Box<dyn Read> = if data.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::read::GzDecoder::new(data))
    } else {
        Box::new(data)
    };
    let mut archive = tar::Archive::new(reader);
    archive.set_mask(0o022);
    archive
}

/// Refuses an archive with any member that would land outside it: a path
/// that is absolute or names `..`, a symlink pointing outside it (resolved
/// beside itself), a hard link to outside it (resolved from the archive's
/// root), a path or link that passes through one of the archive's own
/// symlinks (which a lexical check cannot follow), or a device file.
#[cfg(unix)]
fn validate(data: &[u8]) -> Result<(), UpdateError> {
    let escapes = || UpdateError::new(ESCAPES);
    let mut members = Vec::new();
    let mut bundle = archive(data);
    for entry in bundle
        .entries()
        .map_err(|_| UpdateError::new(NOT_A_BUILD))?
    {
        let entry = entry.map_err(|_| UpdateError::new(NOT_A_BUILD))?;
        let kind = entry.header().entry_type();
        if kind.is_character_special() || kind.is_block_special() || kind.is_fifo() {
            return Err(escapes());
        }
        let path = entry.path().map_err(|_| UpdateError::new(NOT_A_BUILD))?;
        // A member names its own place: `.` aside, plain names only.
        let mut member = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => member.push(part.to_os_string()),
                Component::CurDir => {}
                _ => return Err(escapes()),
            }
        }
        let link = if kind.is_symlink() || kind.is_hard_link() {
            let link = entry
                .link_name()
                .map_err(|_| UpdateError::new(NOT_A_BUILD))?
                .ok_or_else(|| UpdateError::new(NOT_A_BUILD))?;
            Some((kind.is_symlink(), link.into_owned()))
        } else {
            None
        };
        members.push((member, link));
    }
    let symlinks: std::collections::BTreeSet<&[OsString]> = members
        .iter()
        .filter(|(_, link)| matches!(link, Some((true, _))))
        .map(|(member, _)| member.as_slice())
        .collect();
    let through_symlink =
        |parts: &[OsString]| (1..parts.len()).any(|end| symlinks.contains(&parts[..end]));
    for (member, link) in &members {
        if through_symlink(member) {
            return Err(escapes());
        }
        let Some((symlink, link)) = link else {
            continue;
        };
        let origin: &[OsString] = if *symlink {
            &member[..member.len().saturating_sub(1)]
        } else {
            &[]
        };
        let components: Vec<Component<'_>> = link.components().collect();
        let mut parts = origin.to_vec();
        for (step, component) in components.iter().enumerate() {
            match component {
                Component::Normal(part) => parts.push(part.to_os_string()),
                Component::CurDir => {}
                Component::ParentDir => {
                    parts.pop().ok_or_else(escapes)?;
                }
                Component::RootDir | Component::Prefix(_) => return Err(escapes()),
            }
            // Every step but the last is a directory the link resolves through.
            if step + 1 < components.len() && symlinks.contains(parts.as_slice()) {
                return Err(escapes());
            }
        }
    }
    Ok(())
}

/// Unpacks a [`validate`]d archive into `into`. A member `unpack_in` will
/// not write is refused rather than skipped.
#[cfg(unix)]
fn extract(data: &[u8], into: &Path) -> Result<(), UpdateError> {
    validate(data)?;
    let mut bundle = archive(data);
    for entry in bundle
        .entries()
        .map_err(|_| UpdateError::new(NOT_A_BUILD))?
    {
        let mut entry = entry.map_err(|_| UpdateError::new(NOT_A_BUILD))?;
        if !entry.unpack_in(into).map_err(UpdateError::io(into))? {
            return Err(UpdateError::new(ESCAPES));
        }
    }
    Ok(())
}

/// `~/.config/systemd/user/agent-relay.service`, the unit a per-user install
/// points at its tree.
#[cfg(unix)]
pub fn default_unit_path(home: &Path) -> PathBuf {
    home.join(".config/systemd/user/agent-relay.service")
}

/// Installs `release` under `root` (one directory per version) beside the
/// running version and moves the systemd user unit at `unit_path` to it.
/// Returns the new `usr/bin/agent-relay-desktop`. The running version stays
/// on disk, so a failed restart can go back to it.
#[cfg(unix)]
pub fn install_tarball(
    release: &Release,
    root: &Path,
    unit_path: &Path,
    host: &Host<'_>,
) -> Result<PathBuf, UpdateError> {
    let data = cloud::archive_bytes(release, host)?;
    let destination = root.join(&release.version);
    fs::create_dir_all(root).map_err(UpdateError::io(root))?;
    // Unpacked on the install's own file system, so the tree moves into place
    // with a rename.
    let scratch = tempfile::Builder::new()
        .prefix(".agent-relay-update-")
        .tempdir_in(root)
        .map_err(UpdateError::io(root))?;
    let unpacked = scratch.path().join("unpacked");
    fs::create_dir(&unpacked).map_err(UpdateError::io(&unpacked))?;
    extract(&data, &unpacked)?;
    let trees: Vec<PathBuf> = fs::read_dir(&unpacked)
        .map_err(UpdateError::io(&unpacked))?
        .filter_map(|item| item.ok().map(|item| item.path()))
        .filter(|path| path.join(install::APP).exists())
        .collect();
    let [unpacked_tree] = &trees[..] else {
        return Err(UpdateError::new(NOT_A_BUILD));
    };
    if destination.exists() {
        fs::remove_dir_all(&destination).map_err(UpdateError::io(&destination))?;
    }
    fs::create_dir_all(&destination).map_err(UpdateError::io(&destination))?;
    let tree = destination.join(unpacked_tree.file_name().unwrap_or_default());
    fs::rename(unpacked_tree, &tree).map_err(UpdateError::io(&tree))?;
    drop(scratch);

    // The launcher entry is a convenience; the update itself stands.
    let _ = install_launcher(&tree, host);

    let template = tree.join(install::UNIT);
    if template.exists() {
        let unit = fs::read_to_string(&template).map_err(UpdateError::io(&template))?;
        let unit = unit
            .split_inclusive('\n')
            .map(|line| match line.strip_prefix("ExecStart=/usr/") {
                Some(rest) => format!("ExecStart={}/usr/{rest}", tree.display()),
                None => line.to_owned(),
            })
            .collect::<String>();
        write_atomically(unit_path, unit.as_bytes()).map_err(UpdateError::io(unit_path))?;
        for command in [
            &["systemctl", "--user", "daemon-reload"][..],
            // Only a running unit restarts: one the person never enabled or
            // stopped stays stopped.
            &["systemctl", "--user", "try-restart", "agent-relay.service"][..],
        ] {
            let detail = match (host.run)(command, SYSTEMCTL_TIMEOUT) {
                Ok(done) if done.code == Some(0) => continue,
                // Without systemd there is no unit to run the new version.
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Ok(done) => [done.stderr, done.stdout]
                    .into_iter()
                    .map(|text| text.trim().to_owned())
                    .find(|text| !text.is_empty())
                    .unwrap_or_default(),
                Err(error) => error.to_string(),
            };
            let detail = match detail.trim_end_matches('.') {
                "" => ".".to_owned(),
                detail => format!(": {detail}."),
            };
            return Err(UpdateError(format!(
                "Version {} is installed, but `{}` failed{detail} Agent Relay was not restarted.",
                release.version,
                command.join(" ")
            )));
        }
    }
    Ok(tree.join(install::APP))
}

/// The verified package, saved for the system installer in `directory`,
/// else `XDG_DOWNLOAD_DIR`, else `~/Downloads`.
pub fn download_deb(
    release: &Release,
    directory: Option<&Path>,
    host: &Host<'_>,
) -> Result<PathBuf, UpdateError> {
    let (data, name) = cloud::package_bytes(release, host)?;
    let directory = directory.map(Path::to_path_buf).unwrap_or_else(|| {
        host.var("XDG_DOWNLOAD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| host.home.join("Downloads"))
    });
    fs::create_dir_all(&directory).map_err(UpdateError::io(&directory))?;
    let path = directory.join(name);
    fs::write(&path, data).map_err(UpdateError::io(&path))?;
    Ok(path)
}

/// Installs `release` as a per-user install under `root` (one directory per
/// version) and moves the background service to it: the tarball and the
/// systemd user unit on Linux, the zip and the logon task on Windows.
/// Returns the new version's app.
#[cfg(unix)]
pub fn install_per_user(
    release: &Release,
    root: &Path,
    host: &Host<'_>,
) -> Result<PathBuf, UpdateError> {
    install_tarball(release, root, &default_unit_path(&host.home), host)
}

/// See the Unix [`install_per_user`].
#[cfg(windows)]
pub fn install_per_user(
    release: &Release,
    root: &Path,
    host: &Host<'_>,
) -> Result<PathBuf, UpdateError> {
    windows::install_zip(release, root, host)
}

/// Replaces this process with `launcher`, the new version's app. Returns
/// only when that failed.
#[cfg(unix)]
pub fn relaunch(launcher: &Path, arguments: &[String]) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    Err(Command::new(launcher).args(arguments).exec())
}

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows::{
    ensure_launcher, install_launcher, install_launcher_command, install_zip, relaunch,
};

#[cfg(test)]
mod release_channel_tests {
    use super::*;
    use serde_json::json;

    fn release(version: &str, prerelease: bool) -> Value {
        release_of(&Profile::BUILT, version, prerelease)
    }

    /// `profile`'s release: its assets carry its prefix, in its repository.
    pub(super) fn release_of(profile: &Profile, version: &str, prerelease: bool) -> Value {
        let mut assets = Vec::new();
        for platform in [Platform::Linux, Platform::Windows] {
            for arch in ["arm64", "x64"] {
                let filename = format!(
                    "{}.{}",
                    profile.asset_base(version, platform.name(), arch),
                    platform.archive()
                );
                for name in [filename.clone(), format!("{filename}.sha256")] {
                    assets.push(json!({
                        "browser_download_url": format!("https://github.com/{}/releases/download/v{version}/{name}", profile.releases_repository),
                        "name": name,
                    }));
                }
            }
        }
        json!({"tag_name": format!("v{version}"), "draft": false, "prerelease": prerelease, "assets": assets})
    }

    #[test]
    fn beta_selection_preserves_linux_and_windows_asset_formats() {
        let beta = release("2026.10.2-beta.2", true);
        let data = serde_json::to_vec(&beta).unwrap();
        for platform in [Platform::Linux, Platform::Windows] {
            for arch in ["arm64", "x64"] {
                assert!(parse_release_for(&data, arch, platform).is_err());
                let selected =
                    parse_release_for_channel(&Profile::BUILT, &data, arch, platform, true)
                        .unwrap();
                assert!(selected.tarball_url.ends_with(&format!(
                    "-{}-{arch}.{}",
                    platform.name(),
                    platform.archive()
                )));
                assert!(selected.tarball_checksum_url.ends_with(".sha256"));
                let stable = serde_json::to_vec(&release("2026.10.2", false)).unwrap();
                assert_eq!(
                    parse_release_for(&stable, arch, platform).unwrap().version,
                    "2026.10.2"
                );
            }
        }
        let selected = latest_for_channel("x64", true, &|_| {
            Ok(serde_json::to_vec(&vec![release("2026.10.1", false), beta.clone()]).unwrap())
        })
        .unwrap();
        assert!(selected.tarball_url.ends_with(&format!(
            "-{}-x64.{}",
            Platform::CURRENT.name(),
            Platform::CURRENT.archive()
        )));
        let promoted = latest_for_channel("x64", true, &|_| {
            Ok(serde_json::to_vec(&vec![beta.clone(), release("2026.10.2", false)]).unwrap())
        })
        .unwrap();
        assert_eq!(promoted.version, "2026.10.2");
    }
}

#[cfg(test)]
mod console_text_tests {
    use super::console_text;

    #[test]
    fn utf8_passes_through_and_utf16_is_decoded() {
        assert_eq!(console_text("Agent Relay\n".as_bytes()), "Agent Relay\n");
        assert_eq!(console_text(b""), "");
        let path = r"C:\Users\Jörg Müller\AppData\Local\agent-relay.exe";
        let mut with_bom = vec![0xFF, 0xFE];
        let mut bare = Vec::new();
        for unit in path.encode_utf16() {
            with_bom.extend_from_slice(&unit.to_le_bytes());
            bare.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(console_text(&with_bom), path);
        assert_eq!(console_text(&bare), path);
        // Characters past Latin-1 have a high byte of their own.
        let far = r"C:\Users\Łukasz Жуков 山田\agent-relay.exe";
        let bare: Vec<u8> = far.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(console_text(&bare), far);
        // A UTF-8 text with a non-ASCII first character is not mistaken
        // for UTF-16.
        assert_eq!(console_text("Ärger\n".as_bytes()), "Ärger\n");
        assert_eq!(console_text("Łukasz\n".as_bytes()), "Łukasz\n");
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const VERSION: &str = "2026.10.9";
    const UNIT: &str =
        "[Service]\nExecStart=/usr/lib/agent-relay/agent-relay-probe serve --headless\n";
    const ENTRY: &str = "[Desktop Entry]\nType=Application\nName=Agent Relay\nExec=agent-relay-desktop %u\nIcon=agent-relay\nMimeType=x-scheme-handler/agentrelay;\n";

    fn release_json(version: &str) -> Value {
        let base = format!(
            "https://github.com/AgentWorkforce/relay-desktop-releases/releases/download/v{version}"
        );
        let assets: Vec<Value> = ["tar.gz", "tar.gz.sha256", "deb", "deb.sha256"]
            .iter()
            .map(|suffix| {
                let name = format!(
                    "{}-{version}-Linux-x64.{suffix}",
                    crate::build_profile::ASSET_PREFIX
                );
                serde_json::json!({"name": name, "browser_download_url": format!("{base}/{name}")})
            })
            .collect();
        serde_json::json!({"tag_name": format!("v{version}"), "draft": false, "prerelease": false,
                           "html_url": "https://github.com/r", "assets": assets})
    }

    fn parse(value: &Value, arch: &str) -> Result<Release, UpdateError> {
        parse_release(value.to_string().as_bytes(), arch)
    }

    /// A header for `name`, written byte for byte so a test can name `..`.
    fn header(name: &str, kind: tar::EntryType, size: u64) -> tar::Header {
        let mut header = tar::Header::new_gnu();
        header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        header.set_entry_type(kind);
        header.set_size(size);
        header.set_mode(0o755);
        header.set_cksum();
        header
    }

    /// A gzipped archive of `(name, kind, link, data)` members.
    fn archive_of(members: &[(&str, tar::EntryType, &str, &[u8])]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, kind, link, data) in members {
            let mut header = header(name, *kind, data.len() as u64);
            if !link.is_empty() {
                header.set_link_name(link).unwrap();
                header.set_cksum();
            }
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn tarball(version: &str, unit: &str) -> Vec<u8> {
        let file = tar::EntryType::Regular;
        let init = format!("version = \"{version}\"\n");
        archive_of(&[
            (
                "AgentRelay-linux-x64/usr/bin/agent-relay-desktop",
                file,
                "",
                b"#!/bin/sh\n",
            ),
            (
                "AgentRelay-linux-x64/usr/lib/systemd/user/agent-relay.service",
                file,
                "",
                unit.as_bytes(),
            ),
            (
                "AgentRelay-linux-x64/usr/lib/agent-relay/agent-relay-probe",
                file,
                "",
                init.as_bytes(),
            ),
        ])
    }

    /// Serves `archive` and its checksum (or `digest`), records commands and
    /// answers them with `answer`.
    struct Fakes {
        home: PathBuf,
        archive: Vec<u8>,
        digest: Option<String>,
        env: Vec<(&'static str, String)>,
        commands: RefCell<Vec<String>>,
        answer: fn(&[&str]) -> io::Result<CommandOutput>,
    }

    impl Fakes {
        fn new(home: &Path, archive: Vec<u8>) -> Fakes {
            Fakes {
                home: home.to_path_buf(),
                archive,
                digest: None,
                env: Vec::new(),
                commands: RefCell::new(Vec::new()),
                answer: |_| {
                    Ok(CommandOutput {
                        code: Some(0),
                        ..CommandOutput::default()
                    })
                },
            }
        }

        fn with<T>(&self, body: impl FnOnce(&Host<'_>) -> T) -> T {
            let fetch = |url: &str| -> Result<Vec<u8>, FetchError> {
                if url.ends_with(".sha256") {
                    let digest = self.digest.clone().unwrap_or_else(|| {
                        Sha256::digest(&self.archive)
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect()
                    });
                    return Ok(format!("{digest}  AgentRelay.tar.gz\n").into_bytes());
                }
                Ok(self.archive.clone())
            };
            let run = |argv: &[&str], _: Duration| {
                self.commands.borrow_mut().push(argv.join(" "));
                (self.answer)(argv)
            };
            let env = |name: &str| {
                self.env
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.into())
            };
            body(&Host {
                home: self.home.clone(),
                env: &env,
                fetch: &fetch,
                run: &run,
                request: &|_| Err(FetchError::Unreachable("no feed".into())),
                feed: None,
            })
        }

        fn systemctl(&self) -> Vec<String> {
            self.commands
                .borrow()
                .iter()
                .filter(|command| command.starts_with("systemctl"))
                .cloned()
                .collect()
        }
    }

    #[test]
    fn beta_channel_selects_by_version_includes_stable_and_reads_every_page() {
        let beta = |version: &str| {
            let mut release = release_json(version);
            release["prerelease"] = true.into();
            release
        };
        let mut draft = beta("2026.10.99-beta.1");
        draft["draft"] = true.into();
        let first = vec![release_json("2026.10.1"); 100];
        let second = vec![
            beta("2026.10.2-beta.9"),
            draft,
            beta("2026.10.2-beta.10"),
            beta("2026.10.98-rc.1"),
            release_json("2026.10.97-beta.1"),
        ];
        let calls = std::cell::RefCell::new(Vec::new());
        let fetch = |url: &str| {
            calls.borrow_mut().push(url.to_owned());
            let page = if url.ends_with("page=1") {
                &first
            } else {
                &second
            };
            Ok(serde_json::to_vec(page).unwrap())
        };
        let selected = latest_for_channel("x64", true, &fetch).unwrap();
        assert_eq!(selected.version, "2026.10.2-beta.10");
        let releases = Profile::BUILT.releases_url().unwrap();
        assert_eq!(
            *calls.borrow(),
            [
                format!("{releases}?per_page=100&page=1"),
                format!("{releases}?per_page=100&page=2")
            ]
        );
        let promoted = latest_for_channel("x64", true, &|_| {
            Ok(
                serde_json::to_vec(&vec![beta("2026.10.2-beta.10"), release_json("2026.10.2")])
                    .unwrap(),
            )
        })
        .unwrap();
        assert_eq!(promoted.version, "2026.10.2");
        assert!(is_newer(&promoted.version, &selected.version));
        assert!(!is_newer("2026.10.1", &selected.version));
        assert!(parse_release(&serde_json::to_vec(&second[0]).unwrap(), "x64").is_err());
        assert!(parse_release(&serde_json::to_vec(&second[4]).unwrap(), "x64").is_err());
        let stable = latest_for_channel("x64", false, &|url| {
            assert_eq!(Some(url.to_owned()), Profile::BUILT.latest_release_url());
            Ok(serde_json::to_vec(&release_json("2026.10.1")).unwrap())
        })
        .unwrap();
        assert_eq!(stable.version, "2026.10.1");
    }

    #[test]
    fn beta_channel_keeps_asset_verification_and_rejects_bad_responses() {
        let mut beta = release_json("2026.10.2-beta.1");
        beta["prerelease"] = true.into();
        beta["assets"][0]["browser_download_url"] = "https://example.com/archive".into();
        assert!(latest_for_channel("x64", true, &|_| Ok(serde_json::to_vec(
            &vec![beta.clone()]
        )
        .unwrap()))
        .is_err());
        beta["assets"] = serde_json::json!([]);
        assert_eq!(
            latest_for_channel("x64", true, &|_| Ok(serde_json::to_vec(
                &vec![beta.clone()]
            )
            .unwrap()))
            .unwrap_err()
            .to_string(),
            NO_COMPATIBLE_UPDATE
        );
        for response in [b"not json".as_slice(), b"[]", b"{}"] {
            assert!(latest_for_channel("x64", true, &|_| Ok(response.to_vec())).is_err());
        }
        assert!(
            latest_for_channel("x64", true, &|_| Err(FetchError::Unreachable(
                "offline".into()
            )))
            .is_err()
        );
    }

    #[test]
    fn versions_compare_numerically_and_releases_outrank_prereleases() {
        assert!(is_newer("2026.10.10", "2026.10.9"));
        assert!(!is_newer("2026.10.9", "2026.10.9"));
        assert!(is_newer("2026.10.9", "2026.10.9-rc.1"));
        assert!(!is_newer("2026.10.9-rc.1", "2026.10.9"));
        assert!(is_newer("2026.10.9-rc.2", "2026.10.9-rc.1"));
        assert!(is_newer("2026.10.9-rc.10", "2026.10.9-rc.9"));
        assert!(!is_newer("2026.10.9-rc.9", "2026.10.9-rc.10"));
        assert!(is_newer("2026.10.9-RC.2", "2026.10.9-rc.1"));
        assert!(!is_newer("2026.10.9-RC.1", "2026.10.9-rc.1"));
        assert!(is_newer("2026.10.9-beta", "2026.10.9-alpha.3"));
        assert!(is_newer("2026.10.9-rc.1.1", "2026.10.9-rc.1"));
        assert!(!is_newer("latest", "2026.10.9"));
        assert!(!is_newer("2026.10", "2026.9.9"));
        assert!(is_newer(" 2026.10.10+build.7 ", "2026.10.9"));
    }

    #[test]
    fn the_release_names_this_architectures_verified_assets() {
        let release = parse(&release_json(VERSION), "x64").unwrap();
        assert_eq!(release.version, VERSION);
        assert!(release
            .tarball_url
            .ends_with("AgentRelay-2026.10.9-Linux-x64.tar.gz"));
        assert!(release.deb_checksum_url.unwrap().ends_with(".deb.sha256"));
        assert_eq!(release.page_url, "https://github.com/r");
        assert_eq!(
            parse(&release_json(VERSION), "arm64")
                .unwrap_err()
                .to_string(),
            NO_COMPATIBLE_UPDATE
        );
        let mut prerelease = release_json(VERSION);
        prerelease["prerelease"] = Value::Bool(true);
        assert_eq!(
            parse(&prerelease, "x64").unwrap_err().to_string(),
            UNREADABLE
        );
        for link in [
            "https://example.com/AgentRelay.tar.gz",
            "http://github.com/AgentRelay.tar.gz",
            "https://github.com.example.com/AgentRelay.tar.gz",
            "https://github.com@example.com/AgentRelay.tar.gz",
        ] {
            let mut outside = release_json(VERSION);
            outside["assets"][0]["browser_download_url"] = link.into();
            assert_eq!(
                parse(&outside, "x64").unwrap_err().to_string(),
                UNREADABLE,
                "{link}"
            );
        }
        assert_eq!(
            parse(&release_json("latest"), "x64")
                .unwrap_err()
                .to_string(),
            UNREADABLE
        );
        assert_eq!(
            parse_release(b"not json", "x64").unwrap_err().to_string(),
            UNREADABLE
        );
        let mut no_deb = release_json(VERSION);
        no_deb["assets"].as_array_mut().unwrap().truncate(2);
        assert_eq!(parse(&no_deb, "x64").unwrap().deb_url, None);
    }

    #[test]
    fn checksums_and_architectures() {
        let digest = "a".repeat(64);
        assert_eq!(
            checksum(&format!("{digest}  AgentRelay.tar.gz\n")),
            Some(digest.clone())
        );
        assert_eq!(checksum(&format!("{}  x", "A".repeat(64))), Some(digest));
        assert_eq!(checksum("not a digest"), None);
        assert_eq!(checksum(""), None);
        assert_eq!(architecture("aarch64"), Ok("arm64"));
        assert_eq!(architecture("AMD64"), Ok("x64"));
        assert_eq!(
            architecture("riscv64").unwrap_err().to_string(),
            "Agent Relay has no Linux build for riscv64."
        );
    }

    #[test]
    fn checking_reads_the_latest_release_and_names_failures() {
        let release = release_json(VERSION).to_string().into_bytes();
        let found = latest("x64", &|url| {
            assert_eq!(Some(url.to_owned()), Profile::BUILT.latest_release_url());
            Ok(release.clone())
        });
        assert_eq!(found.unwrap().version, VERSION);
        assert_eq!(
            latest("x64", &|_| Err(FetchError::Unreachable("dns".into())))
                .unwrap_err()
                .to_string(),
            UNREADABLE
        );
        assert_eq!(
            latest("x64", &|_| Err(FetchError::TooLarge))
                .unwrap_err()
                .to_string(),
            "The download is larger than any Agent Relay release."
        );
    }

    #[test]
    fn a_verified_tarball_is_installed_beside_the_running_version() {
        let directory = tempfile::tempdir().unwrap();
        let fakes = Fakes::new(&directory.path().join("home"), tarball(VERSION, UNIT));
        let root = directory.path().join("lib");
        let unit = directory.path().join("agent-relay.service");
        let release = parse(&release_json(VERSION), "x64").unwrap();
        let launcher = fakes
            .with(|host| install_tarball(&release, &root, &unit, host))
            .unwrap();
        let tree = root.join("2026.10.9/AgentRelay-linux-x64");
        assert_eq!(launcher, tree.join("usr/bin/agent-relay-desktop"));
        assert!(launcher.exists());
        assert!(fs::read_to_string(&unit).unwrap().contains(&format!(
            "ExecStart={}/usr/lib/agent-relay/agent-relay-probe serve --headless",
            tree.display()
        )));
        assert_eq!(
            fakes.systemctl(),
            [
                "systemctl --user daemon-reload",
                "systemctl --user try-restart agent-relay.service"
            ]
        );
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|item| item.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, [OsString::from("2026.10.9")]);
    }

    #[test]
    fn a_download_that_does_not_match_its_checksum_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let mut fakes = Fakes::new(directory.path(), tarball(VERSION, UNIT));
        fakes.digest = Some("b".repeat(64));
        let root = directory.path().join("lib");
        let release = parse(&release_json(VERSION), "x64").unwrap();
        let error = fakes
            .with(|host| install_tarball(&release, &root, &directory.path().join("unit"), host))
            .unwrap_err();
        assert_eq!(error.to_string(), UNVERIFIED);
        assert!(!root.exists());
        assert!(fakes.commands.borrow().is_empty());
        fakes.digest = Some("not a digest".into());
        let error = fakes
            .with(|host| install_tarball(&release, &root, &directory.path().join("unit"), host))
            .unwrap_err();
        assert_eq!(error.to_string(), UNVERIFIED);
    }

    #[test]
    fn an_archive_that_escapes_its_folder_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let fakes = Fakes::new(
            directory.path(),
            archive_of(&[("../escape", tar::EntryType::Regular, "", b"x")]),
        );
        let release = parse(&release_json(VERSION), "x64").unwrap();
        let error = fakes
            .with(|host| {
                install_tarball(
                    &release,
                    &directory.path().join("lib"),
                    &directory.path().join("unit"),
                    host,
                )
            })
            .unwrap_err();
        assert_eq!(error.to_string(), ESCAPES);
        assert!(!directory.path().join("escape").exists());
        assert!(!directory.path().join("lib/escape").exists());
        assert!(fakes.commands.borrow().is_empty());
    }

    #[test]
    fn hard_links_are_checked_from_the_archive_root_and_symlinks_beside_themselves() {
        use tar::EntryType::{Block, Char, Directory, Fifo, Link, Symlink};
        let into = tempfile::tempdir().unwrap();
        // Beside `tree/deep/link` this would stay inside; from the root it escapes.
        let hard = archive_of(&[
            ("tree/deep", Directory, "", b""),
            ("tree/deep/link", Link, "../outside", b""),
        ]);
        assert_eq!(extract(&hard, into.path()), Err(UpdateError::new(ESCAPES)));
        let absolute = archive_of(&[("tree/link", Symlink, "/etc/passwd", b"")]);
        assert_eq!(
            extract(&absolute, into.path()),
            Err(UpdateError::new(ESCAPES))
        );
        let above = archive_of(&[("tree/link", Symlink, "../../outside", b"")]);
        assert_eq!(extract(&above, into.path()), Err(UpdateError::new(ESCAPES)));
        for device in [Char, Block, Fifo] {
            let special = archive_of(&[("tree/device", device, "", b"")]);
            assert_eq!(
                extract(&special, into.path()),
                Err(UpdateError::new(ESCAPES))
            );
        }
        assert_eq!(fs::read_dir(into.path()).unwrap().count(), 0);

        let beside = archive_of(&[
            ("tree/deep", Directory, "", b""),
            ("tree/deep/link", Symlink, "../outside", b""),
        ]);
        assert_eq!(extract(&beside, into.path()), Ok(()));
        assert_eq!(
            fs::read_link(into.path().join("tree/deep/link")).unwrap(),
            PathBuf::from("../outside")
        );
    }

    #[test]
    fn a_write_through_an_archived_symlink_is_refused() {
        use tar::EntryType::{Regular, Symlink};
        let directory = tempfile::tempdir().unwrap();
        let into = directory.path().join("unpacked");
        fs::create_dir(&into).unwrap();
        // `tree/l` is the archive's root and `tree/l2` its parent, so
        // `tree/l2/escape` would land above it.
        let through = archive_of(&[
            ("tree/l", Symlink, "..", b""),
            ("tree/l2", Symlink, "l/..", b""),
            ("tree/l2/escape", Regular, "", b"x"),
        ]);
        assert_eq!(extract(&through, &into), Err(UpdateError::new(ESCAPES)));
        let direct = archive_of(&[
            ("tree/l", Symlink, "..", b""),
            ("tree/l/escape", Regular, "", b"x"),
        ]);
        assert_eq!(extract(&direct, &into), Err(UpdateError::new(ESCAPES)));
        assert!(!directory.path().join("escape").exists());
        assert!(!into.join("escape").exists());
    }

    #[test]
    fn a_path_or_link_through_an_archived_symlink_is_refused() {
        use tar::EntryType::{Directory, Regular, Symlink};
        let directory = tempfile::tempdir().unwrap();
        let into = directory.path().join("a/b/unpacked");
        fs::create_dir_all(&into).unwrap();
        // `tree/d/up` is `tree`, so `tree/d/up/x` is `tree/x` and its `../..`
        // is above the archive, though lexically it stays inside.
        let chained = archive_of(&[
            ("tree/d", Directory, "", b""),
            ("tree/d/up", Symlink, "..", b""),
            ("tree/d/up/x", Symlink, "../..", b""),
        ]);
        assert_eq!(extract(&chained, &into), Err(UpdateError::new(ESCAPES)));
        // A link resolved through `tree/d/up` lands above the archive too.
        let through = archive_of(&[
            ("tree/d", Directory, "", b""),
            ("tree/d/up", Symlink, "..", b""),
            ("tree/y", Symlink, "d/up/../..", b""),
        ]);
        assert_eq!(extract(&through, &into), Err(UpdateError::new(ESCAPES)));
        let parent = archive_of(&[("tree/a/../b", Regular, "", b"x")]);
        assert_eq!(extract(&parent, &into), Err(UpdateError::new(ESCAPES)));

        // A link to a link, each resolved beside itself, stays inside.
        let alias = archive_of(&[
            ("tree/usr/bin/agent-relay-desktop", Regular, "", b"x"),
            (
                "tree/usr/bin/agent-relay",
                Symlink,
                "agent-relay-desktop",
                b"",
            ),
            ("tree/latest", Symlink, "usr/bin/agent-relay", b""),
        ]);
        assert_eq!(extract(&alias, &into), Ok(()));
    }

    #[test]
    fn unpacked_files_are_writable_only_by_their_owner_and_never_setuid() {
        let into = tempfile::tempdir().unwrap();
        let mut header = header(
            "tree/usr/bin/agent-relay-desktop",
            tar::EntryType::Regular,
            1,
        );
        header.set_mode(0o6777);
        header.set_cksum();
        let mut builder = tar::Builder::new(Vec::new());
        builder.append(&header, &b"x"[..]).unwrap();
        let data = builder.into_inner().unwrap();
        assert_eq!(extract(&data, into.path()), Ok(()));
        let mode = fs::metadata(into.path().join("tree/usr/bin/agent-relay-desktop"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o755);
    }

    #[test]
    fn a_failed_service_restart_fails_the_update() {
        let directory = tempfile::tempdir().unwrap();
        let mut fakes = Fakes::new(directory.path(), tarball(VERSION, UNIT));
        fakes.answer = |argv| {
            Ok(CommandOutput {
                code: Some(if argv.contains(&"try-restart") { 1 } else { 0 }),
                stdout: String::new(),
                stderr: "Unit not found.\n".into(),
            })
        };
        let release = parse(&release_json(VERSION), "x64").unwrap();
        let error = fakes
            .with(|host| {
                install_tarball(
                    &release,
                    &directory.path().join("lib"),
                    &directory.path().join("unit"),
                    host,
                )
            })
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Version 2026.10.9 is installed, but `systemctl --user try-restart agent-relay.service` failed: \
             Unit not found. Agent Relay was not restarted."
        );
        fakes.answer = |_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "systemctl did not answer",
            ))
        };
        let error = fakes
            .with(|host| {
                install_tarball(
                    &release,
                    &directory.path().join("lib"),
                    &directory.path().join("unit"),
                    host,
                )
            })
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Version 2026.10.9 is installed, but `systemctl --user daemon-reload` failed: \
             systemctl did not answer. Agent Relay was not restarted."
        );
    }

    #[test]
    fn without_systemd_the_update_stands() {
        let directory = tempfile::tempdir().unwrap();
        let mut fakes = Fakes::new(directory.path(), tarball(VERSION, UNIT));
        fakes.answer = |_| Err(io::Error::from(io::ErrorKind::NotFound));
        let release = parse(&release_json(VERSION), "x64").unwrap();
        let root = directory.path().join("lib");
        let launcher = fakes
            .with(|host| install_tarball(&release, &root, &directory.path().join("unit"), host))
            .unwrap();
        assert!(launcher.exists());
        assert_eq!(fakes.systemctl(), ["systemctl --user daemon-reload"]);
    }

    #[test]
    fn an_archive_without_exactly_one_build_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let file = tar::EntryType::Regular;
        let two = archive_of(&[
            ("a/usr/bin/agent-relay-desktop", file, "", b""),
            ("b/usr/bin/agent-relay-desktop", file, "", b""),
        ]);
        let release = parse(&release_json(VERSION), "x64").unwrap();
        for archive in [
            two,
            archive_of(&[("a/README", file, "", b"")]),
            b"not an archive".to_vec(),
        ] {
            let fakes = Fakes::new(directory.path(), archive);
            let error = fakes
                .with(|host| {
                    install_tarball(
                        &release,
                        &directory.path().join("lib"),
                        &directory.path().join("unit"),
                        host,
                    )
                })
                .unwrap_err();
            assert_eq!(error.to_string(), NOT_A_BUILD);
        }
    }

    #[test]
    fn the_verified_package_is_saved_for_the_package_manager() {
        let directory = tempfile::tempdir().unwrap();
        let mut fakes = Fakes::new(directory.path(), b"package".to_vec());
        fakes.env.push((
            "XDG_DOWNLOAD_DIR",
            directory.path().join("dl").display().to_string(),
        ));
        let release = parse(&release_json(VERSION), "x64").unwrap();
        let path = fakes
            .with(|host| download_deb(&release, None, host))
            .unwrap();
        assert_eq!(
            path,
            directory
                .path()
                .join("dl/AgentRelay-2026.10.9-Linux-x64.deb")
        );
        assert_eq!(fs::read(&path).unwrap(), b"package");
        fakes.env.clear();
        let path = fakes
            .with(|host| download_deb(&release, None, host))
            .unwrap();
        assert_eq!(
            path,
            directory
                .path()
                .join("Downloads/AgentRelay-2026.10.9-Linux-x64.deb")
        );
        let without = Release {
            deb_url: None,
            ..release
        };
        assert_eq!(
            fakes
                .with(|host| download_deb(&without, None, host))
                .unwrap_err()
                .to_string(),
            "The latest release has no Debian package for this computer."
        );
    }

    #[test]
    fn programs_run_with_a_timeout() {
        let done = run(
            &["sh", "-c", "echo out; echo err >&2; exit 3"],
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(
            done,
            CommandOutput {
                code: Some(3),
                stdout: "out\n".into(),
                stderr: "err\n".into()
            }
        );
        let slow = run(&["sleep", "5"], Duration::from_millis(100)).unwrap_err();
        assert_eq!(slow.kind(), io::ErrorKind::TimedOut);
        let missing = run(&["agent-relay-no-such-program"], Duration::from_secs(1)).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }

    mod launcher {
        use super::*;

        struct Desktop {
            _directory: tempfile::TempDir,
            home: PathBuf,
            entry: PathBuf,
            fakes: Fakes,
        }

        impl Desktop {
            fn new() -> Desktop {
                let directory = tempfile::tempdir().unwrap();
                let home = directory.path().join("home");
                Desktop {
                    entry: home.join(".local/share/applications/agent-relay.desktop"),
                    fakes: Fakes::new(&home, Vec::new()),
                    home,
                    _directory: directory,
                }
            }

            fn tree(&self, version: &str, name: &str) -> PathBuf {
                let tree = self
                    .home
                    .join(".local/lib/agent-relay")
                    .join(version)
                    .join(name);
                let icon = format!("<svg>{version}</svg>");
                for (path, data) in [
                    ("usr/bin/agent-relay-desktop", "#!/bin/sh\n"),
                    ("usr/share/applications/agent-relay.desktop", ENTRY),
                    (
                        "usr/share/icons/hicolor/scalable/apps/agent-relay.svg",
                        &icon,
                    ),
                    ("usr/lib/agent-relay/agent-relay-probe", ""),
                ] {
                    fs::create_dir_all(tree.join(path).parent().unwrap()).unwrap();
                    fs::write(tree.join(path), data).unwrap();
                }
                tree
            }

            fn install(&self, tree: &Path) -> Option<PathBuf> {
                self.fakes
                    .with(|host| install_launcher(tree, host))
                    .unwrap()
            }

            fn ensure(&self, exe: &str) -> Option<PathBuf> {
                self.fakes
                    .with(|host| ensure_launcher(Path::new(exe), host))
                    .unwrap()
            }

            fn text(&self) -> String {
                fs::read_to_string(&self.entry).unwrap()
            }
        }

        const DEB: &str = "/usr/bin/agent-relay-desktop";
        const SOURCE: &str = "/src/relay-desktop/target/debug/agent-relay";

        #[test]
        fn a_per_user_install_is_listed_by_its_full_path_and_follows_updates() {
            let desktop = Desktop::new();
            let old = desktop.tree("2026.10.11", "AgentRelay-linux-x64");
            assert_eq!(desktop.install(&old), Some(desktop.entry.clone()));
            let text = desktop.text();
            assert!(text.replace('"', "").contains(&format!(
                "Exec={}/usr/bin/agent-relay-desktop %u\n",
                old.display()
            )));
            assert!(text.contains(LAUNCHER_MARKER));
            assert!(text.contains("MimeType=x-scheme-handler/agentrelay;"));
            assert_eq!(
                fs::metadata(&desktop.entry).unwrap().permissions().mode() & 0o777,
                0o644
            );
            let icon = desktop
                .home
                .join(".local/share/icons/hicolor/scalable/apps/agent-relay.svg");
            assert_eq!(fs::read_to_string(&icon).unwrap(), "<svg>2026.10.11</svg>");
            let new = desktop.tree("2026.10.12", "AgentRelay-linux-x64");
            desktop.install(&new);
            assert!(desktop.text().replace('"', "").contains(&format!(
                "Exec={}/usr/bin/agent-relay-desktop %u\n",
                new.display()
            )));
            assert_eq!(fs::read_to_string(&icon).unwrap(), "<svg>2026.10.12</svg>");
            assert_eq!(
                desktop.fakes.commands.borrow().last().unwrap(),
                &format!(
                    "update-desktop-database {}",
                    desktop.entry.parent().unwrap().display()
                )
            );
        }

        #[test]
        fn the_entry_honors_an_absolute_xdg_data_home() {
            let mut desktop = Desktop::new();
            let tree = desktop.tree("2026.10.12", "AgentRelay-linux-x64");
            let data = desktop.home.join("data");
            desktop
                .fakes
                .env
                .push(("XDG_DATA_HOME", data.display().to_string()));
            assert_eq!(
                desktop.install(&tree),
                Some(data.join("applications/agent-relay.desktop"))
            );
            assert!(data
                .join("icons/hicolor/scalable/apps/agent-relay.svg")
                .exists());
            desktop.fakes.env = vec![("XDG_DATA_HOME", "relative".into())];
            assert_eq!(desktop.install(&tree), Some(desktop.entry.clone()));
        }

        #[test]
        fn the_running_install_keeps_its_entry_and_only_per_user_installs_write_one() {
            let desktop = Desktop::new();
            let tree = desktop.tree("2026.10.12", "AgentRelay-linux-x64");
            let exe = tree.join("usr/bin/agent-relay-desktop");
            assert_eq!(install::install_tree(&exe), Some(tree.as_path()));
            assert_eq!(
                desktop.ensure(&exe.to_string_lossy()),
                Some(desktop.entry.clone())
            );
            assert_eq!(desktop.ensure(DEB), None);
            assert_eq!(desktop.ensure(SOURCE), None);
        }

        #[test]
        fn a_path_with_spaces_is_quoted_and_someone_elses_entry_is_kept() {
            let desktop = Desktop::new();
            let tree = desktop.tree("2026.10.12", "Agent Relay");
            desktop.install(&tree);
            assert!(desktop.text().contains(&format!(
                "Exec=\"{}/usr/bin/agent-relay-desktop\" %u",
                tree.display()
            )));
            let theirs = "[Desktop Entry]\nName=Something else\nExec=other\n";
            fs::write(&desktop.entry, theirs).unwrap();
            assert_eq!(desktop.install(&tree), None);
            assert_eq!(desktop.text(), theirs);
            // Quoted, then each backslash doubled for the string value.
            assert_eq!(
                exec_command(Path::new("/a/$b\"c`d\\e")),
                r#""/a/\\$b\\"c\\`d\\\\e""#
            );
            assert_eq!(exec_command(Path::new("/a/~b(1)")), r#""/a/~b(1)""#);
            assert_eq!(exec_command(Path::new("/a/50%'s")), r#""/a/50%%'s""#);
            assert_eq!(exec_command(Path::new("/a/100%")), "/a/100%%");
        }

        #[test]
        fn only_the_apps_own_or_an_unedited_packaged_entry_is_rewritten() {
            let desktop = Desktop::new();
            let tree = desktop.tree("2026.10.12", "AgentRelay-linux-x64");
            fs::create_dir_all(desktop.entry.parent().unwrap()).unwrap();
            let custom = "[Desktop Entry]\nName=Agent Relay\nIcon=agent-relay\nExec=/opt/mine/agent-relay --verbose\n";
            fs::write(&desktop.entry, custom).unwrap();
            assert_eq!(desktop.install(&tree), None);
            assert_eq!(desktop.text(), custom);
            // A copy of the packaged entry runs a bare name, and is taken over.
            fs::write(&desktop.entry, ENTRY).unwrap();
            assert_eq!(desktop.install(&tree), Some(desktop.entry.clone()));
            assert!(desktop.text().contains(LAUNCHER_MARKER));
        }

        #[test]
        fn the_system_package_removes_the_entry_a_per_user_install_left() {
            let desktop = Desktop::new();
            let tree = desktop.tree("2026.10.12", "AgentRelay-linux-x64");
            desktop.install(&tree);
            assert!(desktop.entry.exists());
            assert_eq!(desktop.ensure(DEB), None);
            assert!(!desktop.entry.exists());
            // Someone else's entry is never removed.
            fs::write(&desktop.entry, "[Desktop Entry]\nName=Mine\nExec=mine\n").unwrap();
            assert!(!desktop.fakes.with(remove_launcher).unwrap());
            assert!(desktop.entry.exists());
        }

        #[test]
        fn launch_at_login_follows_the_installed_app() {
            let desktop = Desktop::new();
            let autostart = desktop.home.join(".config/autostart/agent-relay.desktop");
            fs::create_dir_all(autostart.parent().unwrap()).unwrap();
            let earlier = "[Desktop Entry]\nType=Application\nName=Agent Relay\n\
                           Exec=/usr/bin/agent-relay --background\nIcon=agent-relay\n";
            fs::write(&autostart, earlier).unwrap();
            desktop.ensure(SOURCE);
            assert_eq!(fs::read_to_string(&autostart).unwrap(), earlier);
            desktop.ensure(DEB);
            assert!(fs::read_to_string(&autostart)
                .unwrap()
                .contains("Exec=/usr/bin/agent-relay-desktop --background\n"));
            let tree = desktop.tree("2026.10.12", "AgentRelay-linux-x64");
            let exe = tree.join("usr/bin/agent-relay-desktop");
            desktop.ensure(&exe.to_string_lossy());
            assert!(fs::read_to_string(&autostart)
                .unwrap()
                .contains(&format!("Exec={} --background\n", exe.display())));
        }

        #[test]
        fn a_slow_launcher_database_never_fails_the_install() {
            let mut desktop = Desktop::new();
            desktop.fakes.answer = |_| Err(io::Error::new(io::ErrorKind::TimedOut, "slow"));
            let tree = desktop.tree("2026.10.12", "AgentRelay-linux-x64");
            assert_eq!(desktop.install(&tree), Some(desktop.entry.clone()));
            assert!(desktop.text().contains(LAUNCHER_MARKER));
        }

        #[test]
        fn install_launcher_reports_what_it_did() {
            let desktop = Desktop::new();
            let tree = desktop.tree("2026.10.12", "AgentRelay-linux-x64");
            let exe = tree.join("usr/bin/agent-relay-desktop");
            let command = |exe: &Path| {
                desktop
                    .fakes
                    .with(|host| install_launcher_command(exe, host))
            };
            assert_eq!(
                command(Path::new(DEB)),
                (2, "Only a per-user install (~/.local/lib/agent-relay) needs this; a package lists itself.".into())
            );
            assert_eq!(command(&exe), (0, desktop.entry.display().to_string()));
            fs::write(&desktop.entry, "[Desktop Entry]\nExec=mine\n").unwrap();
            assert_eq!(
                command(&exe),
                (1, "Agent Relay could not write its launcher entry; one written by someone else is kept.".into())
            );
        }
    }
}
