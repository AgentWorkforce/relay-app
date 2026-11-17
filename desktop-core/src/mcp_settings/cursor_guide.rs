//! Agent Relay Desktop's own entries in `~/.cursor/hooks.json`: one
//! `cursor-guide-hook` ([`crate::cursor_guide`]) per event in
//! [`crate::cursor_guide::EVENTS`], beside the core's `cursor-hook` entries.
//!
//! The core's editor (`CursorSettings`) only touches entries running its own
//! subcommand, so these are this build's to add and take out. A site profile
//! build's entries name its session socket and sit beside Agent Relay's:
//! each build only ever replaces or removes its own. Every other entry, event
//! and key is kept; a file that is not a JSON object, or whose `hooks` or an
//! event has another shape, is left unchanged. Edits take the core's lock
//! over the file and write it the way the core does: through any symlink,
//! atomically, keeping its mode and a one-time backup.
//!
//! The core's lock path (`CursorSettings::lock_path`) is not public, so it is
//! derived here the same way, from the inbox directory's parent.
use super::*;
use crate::cursor_guide::{EVENTS, SUBCOMMAND};
use agent_relay_node::relay::claude_settings::{resolve_symlinks, write_keeping_file};
use agent_relay_node::relay::cursor_cli::CursorConfig;
use serde_json::Map;

/// Seconds Cursor waits for the hook, which only reads a pointer file.
const TIMEOUT_SECS: u64 = 5;

/// The hook's shell command for `executable`; `--socket` names a site
/// profile build's own session socket, and `--dev` asks Agent Relay Dev.
pub(super) fn command(
    executable: &Path,
    development: bool,
    socket: Option<&Path>,
) -> Option<String> {
    let mut command = format!("{} {SUBCOMMAND}", quote(executable.to_str()?)?);
    if let Some(socket) = socket {
        command.push_str(&format!(" --socket {}", quote(socket.to_str()?)?));
    }
    if development {
        command.push_str(" --dev");
    }
    Some(command)
}

/// This build's entries in one `hooks.json`.
pub(super) struct GuideHooks {
    file: PathBuf,
    lock: PathBuf,
    /// The command this build installs.
    command: String,
    /// The quoted session socket its command names: a site profile build's.
    socket: Option<String>,
}

impl GuideHooks {
    pub(super) fn new(
        config: &CursorConfig,
        command: String,
        socket: Option<&Path>,
    ) -> Option<Self> {
        let inbox = config.inbox();
        Some(Self {
            file: config.hooks_file(),
            lock: inbox.parent().unwrap_or(&inbox).join(".hooks.lock"),
            command,
            socket: match socket {
                Some(socket) => Some(quote(socket.to_str()?)?),
                None => None,
            },
        })
    }

    /// This build's entry in any location of the executable, with or without
    /// `--dev`: one naming exactly this build's session socket, or none for
    /// Agent Relay's. Another build's entry is never ours.
    fn ours(&self, entry: &Value) -> bool {
        entry["command"].as_str().is_some_and(|command| {
            let body = command.trim();
            let body = body.strip_suffix(" --dev").unwrap_or(body).trim_end();
            let (body, socket) = match body.rsplit_once(" --socket ") {
                Some((hook, socket)) => (hook.trim_end(), Some(socket.trim())),
                None => (body, None),
            };
            body.ends_with(&format!(" {SUBCOMMAND}")) && socket == self.socket.as_deref()
        })
    }

    /// Whether `entries` hold exactly this build's current entry.
    fn current(&self, entries: &[Value]) -> bool {
        let mut ours = entries.iter().filter(|entry| self.ours(entry));
        ours.next()
            .is_some_and(|entry| entry["command"] == self.command.as_str())
            && ours.next().is_none()
    }

    fn read(&self) -> Result<Map<String, Value>, String> {
        let bytes = match fs::read(resolve_symlinks(&self.file)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(_) => return Err("~/.cursor/hooks.json cannot be read".into()),
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Map::new());
        }
        match serde_json::from_slice(&bytes) {
            Ok(Value::Object(object)) => Ok(object),
            _ => Err("~/.cursor/hooks.json is not a JSON object, so it was left unchanged".into()),
        }
    }

    /// Whether every event has this build's current entry, once.
    pub(super) fn ready(&self) -> bool {
        self.read().is_ok_and(|object| {
            EVENTS.iter().all(|event| {
                object
                    .get("hooks")
                    .and_then(|hooks| hooks.get(*event))
                    .and_then(Value::as_array)
                    .is_some_and(|entries| self.current(entries))
            })
        })
    }

    /// `object` with this build's entries brought to `enabled`: exactly one
    /// current entry per event, or none. Others' entries keep their places.
    fn render(&self, object: &mut Map<String, Value>, enabled: bool) -> Result<(), String> {
        let shape = |what: &str| {
            format!("~/.cursor/hooks.json has a {what} value of another shape, so it was left unchanged")
        };
        if !enabled && object.get("hooks").is_none() {
            return Ok(());
        }
        if enabled {
            object.entry("version").or_insert(json!(1));
        }
        let hooks = object.entry("hooks").or_insert_with(|| json!({}));
        let hooks = hooks.as_object_mut().ok_or_else(|| shape("hooks"))?;
        for event in EVENTS {
            if !enabled && !hooks.contains_key(event) {
                continue;
            }
            let entries = hooks.entry(event).or_insert_with(|| json!([]));
            let entries = entries.as_array_mut().ok_or_else(|| shape(event))?;
            if enabled && self.current(entries) {
                continue;
            }
            entries.retain(|entry| !self.ours(entry));
            if enabled {
                entries.push(json!({"command": self.command, "timeout": TIMEOUT_SECS}));
            } else if entries.is_empty() {
                hooks.remove(event);
            }
        }
        if hooks.is_empty() {
            object.remove("hooks");
        }
        Ok(())
    }

    /// The core's lock over `hooks.json`, exclusive while the file is edited.
    fn file_lock(&self) -> Option<fs::File> {
        private_fs::create_private_dir_all(self.lock.parent()?).ok()?;
        let file = private_fs::with_mode(
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false),
            0o600,
        )
        .open(&self.lock)
        .ok()?;
        fs2::FileExt::lock_exclusive(&file).ok().map(|()| file)
    }

    /// Adds (`enabled`) or takes out this build's entries.
    pub(super) fn set(&self, enabled: bool) -> Result<(), String> {
        if !enabled
            && self
                .read()
                .is_ok_and(|object| object.get("hooks").is_none())
        {
            return Ok(());
        }
        let _held = self.file_lock().ok_or(
            "Agent Relay could not lock its Cursor hooks, so ~/.cursor/hooks.json was left unchanged",
        )?;
        let mut object = self.read()?;
        let before = object.clone();
        self.render(&mut object, enabled)?;
        if object == before {
            return Ok(());
        }
        let path = resolve_symlinks(&self.file);
        let existing = fs::read(&path).ok();
        let mut bytes =
            serde_json::to_vec_pretty(&Value::Object(object)).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        if existing.as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        write_keeping_file(
            &path,
            existing.as_deref(),
            &bytes,
            ".relay-desktop-backup",
            0o600,
        )
        .map_err(|_| "~/.cursor/hooks.json cannot be written".to_string())
    }
}

/// Unix quoting: Windows has no Cursor hooks to install.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const OURS: &str = "'/Apps/Agent Relay/agent-relay-probe' cursor-guide-hook";

    fn hooks(root: &Path, socket: Option<&Path>) -> Option<GuideHooks> {
        let config = CursorConfig::from_env(&|_| None, root);
        let probe = Path::new("/Apps/Agent Relay/agent-relay-probe");
        GuideHooks::new(&config, command(probe, false, socket)?, socket)
    }

    fn file(root: &Path) -> Option<Value> {
        let text = fs::read_to_string(root.join(".cursor/hooks.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    #[test]
    fn commands_quote_the_probe_and_name_a_profile_socket() {
        assert_eq!(
            command(
                Path::new("/Apps/Agent Relay/agent-relay-probe"),
                false,
                None
            )
            .unwrap(),
            OURS
        );
        assert_eq!(
            command(
                Path::new("/p"),
                true,
                Some(Path::new("/run/a b/relay.sock"))
            )
            .unwrap(),
            "/p cursor-guide-hook --socket '/run/a b/relay.sock' --dev"
        );
    }

    #[test]
    fn install_is_idempotent_replaces_stale_entries_and_keeps_others() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".cursor/hooks.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"version":1,"hooks":{"sessionStart":[{"command":"other-start"},
              {"command":"/old/agent-relay-probe cursor-guide-hook"},
              {"command":"/old/agent-relay-probe cursor-guide-hook --dev"}],
              "beforeMCPExecution":[{"command":"/p cursor-guide-hook --socket /other.sock"}]}}"#,
        )
        .unwrap();
        let guide = hooks(root.path(), None).unwrap();
        assert!(!guide.ready());
        guide.set(true).unwrap();
        assert!(guide.ready());
        let written = fs::read(&path).unwrap();
        guide.set(true).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            written,
            "a current file is not rewritten"
        );
        let object = file(root.path()).unwrap();
        let start = object["hooks"]["sessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2);
        assert_eq!(start[0]["command"], "other-start");
        assert_eq!(start[1]["command"], OURS);
        let mcp = object["hooks"]["beforeMCPExecution"].as_array().unwrap();
        assert_eq!(
            mcp[0]["command"], "/p cursor-guide-hook --socket /other.sock",
            "another build's stays"
        );
        assert_eq!(mcp[1]["command"], OURS);
        guide.set(false).unwrap();
        assert!(!guide.ready());
        let object = file(root.path()).unwrap();
        assert_eq!(
            object["hooks"]["sessionStart"],
            json!([{"command": "other-start"}])
        );
        assert_eq!(
            object["hooks"]["beforeMCPExecution"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_profile_build_owns_only_its_socket_s_entries() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("profile/relay.sock");
        let default = hooks(root.path(), None).unwrap();
        let profile = hooks(root.path(), Some(&socket)).unwrap();
        default.set(true).unwrap();
        profile.set(true).unwrap();
        assert!(default.ready() && profile.ready());
        assert_eq!(
            file(root.path()).unwrap()["hooks"]["sessionStart"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        profile.set(false).unwrap();
        assert!(default.ready() && !profile.ready());
        default.set(false).unwrap();
        assert!(
            file(root.path()).unwrap().get("hooks").is_none(),
            "empty events go"
        );
    }

    #[test]
    fn other_shapes_are_left_unchanged_and_nothing_is_created_to_remove() {
        let root = tempfile::tempdir().unwrap();
        let guide = hooks(root.path(), None).unwrap();
        guide.set(false).unwrap();
        assert!(!root.path().join(".cursor/hooks.json").exists());
        let path = root.path().join(".cursor/hooks.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for text in [
            r#"[1]"#,
            r#"{"hooks":[]}"#,
            r#"{"hooks":{"sessionStart":{}}}"#,
        ] {
            fs::write(&path, text).unwrap();
            assert!(guide.set(true).is_err(), "{text}");
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
    }
}
