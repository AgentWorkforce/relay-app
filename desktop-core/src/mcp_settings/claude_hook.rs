//! Agent Relay's Claude Code PreToolUse hook, installed and removed with the
//! hosted MCP entry. Each shared Claude login's `settings.json` gets one
//! group matching [`crate::claude_attestation::MATCHER`] that runs `claude-attestation-hook`
//! ([`crate::claude_attestation`]). A site profile build's hook names its own
//! session socket and matches its own server, and sits beside Agent Relay's:
//! each build only ever replaces or removes its own. Every other hook, group
//! and setting is kept; a file that is not a JSON object, or whose `hooks` or
//! `PreToolUse` has another shape, is left unchanged.
//!
//! The same edits serve the teammate-context SessionStart hook
//! ([`crate::teammate_context`]): each [`Kind`] has its own event, subcommand
//! and group, so neither ever takes out or rewrites the other.
use super::*;
#[cfg(test)]
use crate::claude_attestation::MATCHER;
use crate::claude_attestation::SUBCOMMAND;
use agent_relay_node::relay::claude_settings::{
    claude_settings_edit_lock, resolve_symlinks, write_keeping_file,
};
use serde_json::Map;

/// Which of Agent Relay's desktop hooks: each has its own event and
/// subcommand, and is recognized only by those.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The PreToolUse chat identity hook (`claude-attestation-hook`).
    Attestation,
    /// The SessionStart teammate-context hook (`teammate-context-hook`).
    TeammateContext,
}

impl Kind {
    fn event(self) -> &'static str {
        match self {
            Kind::Attestation => "PreToolUse",
            Kind::TeammateContext => "SessionStart",
        }
    }

    pub(crate) fn subcommand(self) -> &'static str {
        match self {
            Kind::Attestation => SUBCOMMAND,
            Kind::TeammateContext => crate::teammate_context::SUBCOMMAND,
        }
    }
}

/// Asks the core, at the moment of each status read, whether Cloud verifies
/// Claude chat proofs for the signed-in account and its identity-sync
/// generation: the same check the signing route makes.
pub type Verified = std::sync::Arc<dyn Fn() -> bool + Send + Sync>;
/// Seconds Claude waits for the hook. The hook's own socket read is shorter.
const TIMEOUT_SECS: u64 = 5;

/// The hook's shell command for `executable`; `--dev` asks Agent Relay Dev,
/// and `--socket` a site profile build's own session socket.
pub(super) fn command(
    executable: &Path,
    development: bool,
    socket: Option<&Path>,
) -> Option<String> {
    command_for(Kind::Attestation, executable, development, socket)
}

/// [`command`] for `kind`'s subcommand.
pub(super) fn command_for(
    kind: Kind,
    executable: &Path,
    development: bool,
    socket: Option<&Path>,
) -> Option<String> {
    let mut command = format!("{} {}", quote(executable.to_str()?)?, kind.subcommand());
    if let Some(socket) = socket {
        command.push_str(&format!(" --socket {}", quote(socket.to_str()?)?));
    }
    if development {
        command.push_str(" --dev");
    }
    Some(command)
}

/// Whose hook: Agent Relay's names no session socket and matches
/// [`crate::claude_attestation::MATCHER`]; a site profile build's names its own session socket and
/// matches its own server.
pub(super) struct Build {
    socket: Option<String>,
    matcher: String,
}

impl Build {
    pub(super) fn new(socket: Option<&Path>, server: &str) -> Option<Self> {
        Some(Self {
            socket: match socket {
                Some(socket) => Some(quote(socket.to_str()?)?),
                None => None,
            },
            matcher: crate::claude_attestation::matcher_for(server),
        })
    }

    #[cfg(test)]
    pub(super) fn production() -> Self {
        Self {
            socket: None,
            matcher: MATCHER.into(),
        }
    }
}

/// This build's hook in any location of the executable, with or without
/// `--dev`: one naming exactly this build's session socket, or none for
/// Agent Relay's. Another build's hook is never ours.
fn ours_for(hook: &Value, build: &Build, kind: Kind) -> bool {
    hook["command"].as_str().is_some_and(|command| {
        let body = command.trim();
        let body = body.strip_suffix(" --dev").unwrap_or(body).trim_end();
        let (body, socket) = match body.rsplit_once(" --socket ") {
            Some((hook, socket)) => (hook.trim_end(), Some(socket.trim())),
            None => (body, None),
        };
        body.ends_with(kind.subcommand()) && socket == build.socket.as_deref()
    })
}

#[cfg(test)]
fn ours(hook: &Value) -> bool {
    ours_for(hook, &Build::production(), Kind::Attestation)
}

fn entry(command: &str, build: &Build, kind: Kind) -> Value {
    let hooks = json!([{"type": "command", "command": command, "timeout": TIMEOUT_SECS}]);
    match kind {
        Kind::Attestation => json!({"matcher": build.matcher, "hooks": hooks}),
        // Every session start: a new chat, a resumed one, after /clear and
        // after compaction.
        Kind::TeammateContext => json!({"hooks": hooks}),
    }
}

fn object(text: &str) -> Option<Map<String, Value>> {
    if text.trim().is_empty() {
        return Some(Map::new());
    }
    match serde_json::from_str(text).ok()? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

/// Our hooks taken out of `groups`, with any group left holding none.
fn take_out(groups: &mut Vec<Value>, build: &Build, kind: Kind) {
    for group in groups.iter_mut() {
        if let Some(hooks) = group["hooks"].as_array_mut() {
            hooks.retain(|hook| !ours_for(hook, build, kind));
        }
    }
    groups.retain(|group| group["hooks"].as_array().is_none_or(|h| !h.is_empty()));
}

#[cfg(test)]
pub(super) fn render(text: &str, command: Option<&str>) -> Option<String> {
    render_for(text, command, &Build::production())
}

/// The settings with this build's hook set to `command`, or removed when
/// it is `None`. `None` when nothing changes or the shape is unknown.
/// A current hook stays where it is, so a profile build's hook and Agent
/// Relay's never move each other's group on every refresh.
#[cfg(test)]
pub(super) fn render_for(text: &str, command: Option<&str>, build: &Build) -> Option<String> {
    render_kind(text, command, build, Kind::Attestation)
}

/// The settings with this build's `kind` hook set to `command`, or removed.
pub(super) fn render_kind(
    text: &str,
    command: Option<&str>,
    build: &Build,
    kind: Kind,
) -> Option<String> {
    let event = kind.event();
    let mut root = object(text)?;
    if command.is_some_and(|command| current_kind(&Value::Object(root.clone()), command, build, kind)) {
        return None;
    }
    let before = root.clone();
    let hooks = match root.get_mut("hooks") {
        Some(hooks) => hooks.as_object_mut()?,
        None if command.is_none() => return None,
        None => root
            .entry("hooks")
            .or_insert_with(|| json!({}))
            .as_object_mut()?,
    };
    let groups = match hooks.get_mut(event) {
        Some(groups) => groups.as_array_mut()?,
        None if command.is_none() => return None,
        None => hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()?,
    };
    take_out(groups, build, kind);
    match command {
        Some(command) => groups.push(entry(command, build, kind)),
        None if groups.is_empty() => {
            hooks.remove(event);
            if hooks.is_empty() {
                root.remove("hooks");
            }
        }
        None => {}
    }
    if root == before {
        return None;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(root)).ok()?;
    text.push('\n');
    Some(text)
}

fn groups(root: &Value, kind: Kind) -> impl Iterator<Item = &Value> {
    root["hooks"][kind.event()].as_array().into_iter().flatten()
}

#[cfg(test)]
pub(super) fn present(root: &Value) -> bool {
    present_for(root, &Build::production(), Kind::Attestation)
}

/// Any of this build's hooks in the file.
fn present_for(root: &Value, build: &Build, kind: Kind) -> bool {
    groups(root, kind)
        .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
        .any(|hook| ours_for(hook, build, kind))
}

#[cfg(test)]
pub(super) fn current(root: &Value, command: &str) -> bool {
    current_for(root, command, &Build::production())
}

/// Exactly one of this build's hooks, and it is the current one.
pub(super) fn current_for(root: &Value, command: &str, build: &Build) -> bool {
    current_kind(root, command, build, Kind::Attestation)
}

/// [`current_for`] for `kind`'s hook.
pub(super) fn current_kind(root: &Value, command: &str, build: &Build, kind: Kind) -> bool {
    let count = groups(root, kind)
        .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
        .filter(|hook| ours_for(hook, build, kind))
        .count();
    count == 1 && groups(root, kind).any(|group| *group == entry(command, build, kind))
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

impl Installer {
    /// Where status reads learn whether Cloud verifies Claude chat proofs.
    pub fn set_claude_attestation_source(&self, verified: Verified) {
        *self
            .claude_attestation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(verified);
    }

    /// Cloud verifies Claude chat proofs for the current account; false
    /// until the core has said where to ask.
    pub fn claude_attestation(&self) -> bool {
        let verified = self
            .claude_attestation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        verified.is_some_and(|verified| verified())
    }

    /// `settings.json` beside each login's `.claude.json`.
    pub(super) fn claude_settings_files(&self, shared_only: bool) -> Vec<PathBuf> {
        let mut paths: Vec<_> = self
            .claude
            .dirs
            .watched(&self.claude.home)
            .into_iter()
            .filter(|login| !shared_only || login.shared)
            .filter(|login| self.claude_login_allowed(&login.path))
            .map(|login| login.path.join("settings.json"))
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    /// Writes (`Some`) or removes (`None`) the hook in one login's settings.
    /// Whether the file then holds what was asked.
    pub(super) fn edit_claude_hook(&self, path: &Path, command: Option<&str>) -> bool {
        self.edit_claude_hook_kind(path, command, Kind::Attestation)
    }

    /// [`Self::edit_claude_hook`] for `kind`'s hook.
    pub(super) fn edit_claude_hook_kind(
        &self,
        path: &Path,
        command: Option<&str>,
        kind: Kind,
    ) -> bool {
        if !self.claude.dirs.settings_writable(path) {
            return false;
        }
        let _edit = claude_settings_edit_lock();
        let target = resolve_symlinks(path);
        let existing = match fs::read(&target) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return false,
        };
        let text = String::from_utf8_lossy(existing.as_deref().unwrap_or_default());
        let Some(build) = self.helper.hook_build() else {
            return false;
        };
        if let Some(updated) = render_kind(&text, command, &build, kind) {
            let written = write_keeping_file(
                &target,
                existing.as_deref(),
                updated.as_bytes(),
                agent_relay_node::relay::claude_settings::BACKUP_SUFFIX,
                0o600,
            );
            if written.is_err() {
                return false;
            }
        }
        // A file Claude cannot parse runs no hook, so none of ours is left.
        match (read(path), command) {
            (Some(root), Some(command)) => current_kind(&root, command, &build, kind),
            (Some(root), None) => !present_for(&root, &build, kind),
            (None, command) => command.is_none(),
        }
    }

    /// Whether any watched login holds this build's `kind` hook.
    #[cfg(test)]
    pub(super) fn claude_hook_kind_present(&self, kind: Kind) -> bool {
        let Some(build) = self.helper.hook_build() else {
            return false;
        };
        self.claude_settings_files(false)
            .iter()
            .any(|path| read(path).is_some_and(|root| present_for(&root, &build, kind)))
    }

    /// The hook command this installation writes.
    pub(super) fn claude_hook_command(&self) -> Option<String> {
        self.helper.attestation_hook()
    }

    /// Every required login has the current hook and no file has an old one.
    pub(super) fn claude_hook_configured(&self) -> bool {
        let (Some(command), Some(build)) = (self.claude_hook_command(), self.helper.hook_build())
        else {
            return false;
        };
        let required = self.claude_settings_files(true);
        !required.is_empty()
            && required
                .iter()
                .all(|path| read(path).is_some_and(|root| current_for(&root, &command, &build)))
    }

    /// Removes the hook from every watched login when `id` is Claude.
    pub(super) fn remove_claude_hooks(&self, id: &str) -> Result<(), String> {
        let files = if id == "claude" {
            self.claude_settings_files(false)
        } else {
            Vec::new()
        };
        // Every login is tried, even after one fails.
        let removed = files
            .iter()
            .fold(true, |all, path| self.edit_claude_hook(path, None) && all);
        match removed {
            true => Ok(()),
            false => Err("The Claude Code chat identity hook could not be removed.".into()),
        }
    }

    pub(super) fn claude_hook_present(&self) -> bool {
        let Some(build) = self.helper.hook_build() else {
            return false;
        };
        self.claude_settings_files(false)
            .iter()
            .any(|path| read(path).is_some_and(|root| present_for(&root, &build, Kind::Attestation)))
    }
}

#[cfg(test)]
mod tests;
