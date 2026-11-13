//! An owned, no-op hook enables the native SDK's pre-MCP event journal.
use super::super::{private_fs, providers};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

const FAILURE: &str = "Copilot MCP observer changed or is not an owned configuration";
const LIMIT: u64 = 64 * 1024;
type Bytes = Option<Vec<u8>>;
#[derive(Clone)]
struct Receipt {
    before: Bytes,
    after: Bytes,
}

pub(super) struct Settings {
    path: PathBuf,
    helper: PathBuf,
    development: bool,
    receipt: Arc<Mutex<Option<Receipt>>>,
}
pub(super) struct Backup {
    path: PathBuf,
    original: Bytes,
    receipt: Arc<Mutex<Option<Receipt>>>,
}
impl Settings {
    pub(super) fn new(path: PathBuf, helper: PathBuf, development: bool) -> Self {
        Self {
            path,
            helper,
            development,
            receipt: Arc::new(Mutex::new(None)),
        }
    }
    pub(super) fn enabled(&self) -> bool {
        read(&self.path)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|v| {
                self.helper.is_absolute() && v == value(&self.helper, self.development)
            })
    }
    fn checked(&self) -> Result<Bytes, String> {
        let bytes = read(&self.path)?;
        if bytes.as_deref().is_some_and(|b| !owned(b, &self.helper)) {
            return Err(FAILURE.into());
        }
        Ok(bytes)
    }
    pub(super) fn snapshot(&self) -> Result<Backup, String> {
        let mut receipt = self.receipt.lock().map_err(|_| FAILURE)?;
        let original = self.checked()?;
        *receipt = None;
        Ok(Backup {
            path: self.path.clone(),
            original,
            receipt: self.receipt.clone(),
        })
    }
    pub(super) fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut receipt = self.receipt.lock().map_err(|_| FAILURE)?;
        if enabled && !valid_path(&self.helper) {
            return Err(FAILURE.into());
        }
        let before = self.checked()?;
        let after = enabled
            .then(|| serde_json::to_vec_pretty(&value(&self.helper, self.development)))
            .transpose()
            .map_err(|_| FAILURE)?;
        if before == after {
            return Ok(());
        }
        replace(&self.path, &before, &after)?;
        // Record immediately after mutation, before the fallible inspection.
        *receipt = Some(Receipt {
            before,
            after: after.clone(),
        });
        if read(&self.path)? != after {
            return Err(FAILURE.into());
        }
        Ok(())
    }
}
impl Backup {
    pub(super) fn restore(&self) -> Result<(), String> {
        let mut receipt = self.receipt.lock().map_err(|_| FAILURE)?;
        let current = read(&self.path)?;
        if current == self.original {
            return Ok(());
        }
        let expected = receipt
            .as_ref()
            .filter(|r| r.before == self.original && r.after == current)
            .ok_or(FAILURE)?;
        replace(&self.path, &expected.after, &self.original)?;
        *receipt = None;
        if read(&self.path)? != self.original {
            return Err(FAILURE.into());
        }
        Ok(())
    }
}
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}
fn value(helper: &Path, development: bool) -> Value {
    let command = format!(
        "exec {} copilot-mcp-observe{}",
        quote(helper),
        if development { " --dev" } else { "" }
    );
    json!({"version":1,"agentRelayManagedBy":"agent-relay-desktop","hooks":{
        "preMcpToolCall":[{"type":"command","command":command,"timeoutSec":5}]}})
}
fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        && path
            .to_str()
            .is_some_and(|p| !p.chars().any(char::is_control))
}
fn known(helper: &Path, current: &Path) -> bool {
    valid_path(helper)
        && (helper == current
            || [
                ".local/bin/agent-relay-probe",
                ".local/bin/agent-relay-probe.exe",
                ".agentworkforce/desktop/bin/agent-relay-probe",
                ".agentworkforce/desktop/bin/agent-relay-probe.exe",
                "Agent Relay.app/Contents/Helpers/Agent Relay Helper",
                "Agent Relay Dev.app/Contents/Helpers/Agent Relay Helper",
            ]
            .iter()
            .any(|suffix| helper.ends_with(suffix)))
}
fn owned(bytes: &[u8], current: &Path) -> bool {
    let Ok(document) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    let Some(command) = document["hooks"]["preMcpToolCall"][0]["command"].as_str() else {
        return false;
    };
    for development in [false, true] {
        let suffix = if development {
            " copilot-mcp-observe --dev"
        } else {
            " copilot-mcp-observe"
        };
        let Some(encoded) = command
            .strip_prefix("exec ")
            .and_then(|v| v.strip_suffix(suffix))
        else {
            continue;
        };
        let Some(inner) = encoded
            .strip_prefix('\'')
            .and_then(|v| v.strip_suffix('\''))
        else {
            continue;
        };
        let decoded = inner.replace("'\"'\"'", "'");
        let helper = Path::new(&decoded);
        if known(helper, current) && document == value(helper, development) {
            return true;
        }
    }
    false
}
fn read(path: &Path) -> Result<Bytes, String> {
    providers::safe_parent(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(FAILURE.into()),
    };
    if !metadata.is_file()
        || !private_fs::owned_by_current_user(&metadata)
        || !private_fs::private_to_owner(&metadata, 0o022)
        || metadata.len() > LIMIT
    {
        return Err(FAILURE.into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| FAILURE)?;
    let opened = file.metadata().map_err(|_| FAILURE)?;
    if !opened.is_file()
        || !private_fs::owned_by_current_user(&opened)
        || !private_fs::private_to_owner(&opened, 0o022)
        || opened.len() > LIMIT
    {
        return Err(FAILURE.into());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FAILURE)?;
    if bytes.len() as u64 > LIMIT {
        return Err(FAILURE.into());
    }
    Ok(Some(bytes))
}
fn replace(path: &Path, before: &Bytes, after: &Bytes) -> Result<(), String> {
    providers::safe_parent(path)?;
    if read(path)? != *before {
        return Err(FAILURE.into());
    }
    if let Some(bytes) = after {
        let parent = path.parent().ok_or(FAILURE)?;
        private_fs::create_private_dir_all(parent).map_err(|_| FAILURE)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| FAILURE)?;
        private_fs::set_mode(temporary.path(), 0o600).map_err(|_| FAILURE)?;
        temporary
            .write_all(bytes)
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|_| FAILURE)?;
        if read(path)? != *before {
            return Err(FAILURE.into());
        }
        temporary.persist(path).map_err(|_| FAILURE)?;
    } else if before.is_some() {
        if read(path)? != *before {
            return Err(FAILURE.into());
        }
        fs::remove_file(path).map_err(|_| FAILURE)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests;
