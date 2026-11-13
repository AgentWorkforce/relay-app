//! In-memory rollback for the two Copilot JSON files. No credential backup is
//! written to disk, and concurrent foreign configuration is never overwritten.
use super::*;

/// Shipped HTTP entries carry this owner header. A service URL or alias alone
/// is not permission to migrate a person's hand-written VS Code server.
pub(super) fn managed_legacy(entry: &Value) -> bool {
    owned_http(entry) || (entry.get("url").is_none() && recognized::json(entry, None, None))
}

/// A legacy managed command remains removable after a normal app update.
/// A marker alone never makes an arbitrary command an owned configuration.
pub(super) fn managed_stdio(entry: &Value, endpoint: Option<&str>, helper: Option<&Path>) -> bool {
    if entry["agentRelayManagedBy"] != OWNER
        || entry["type"] != "stdio"
        || entry.get("headers").is_some()
        || !entry["command"]
            .as_str()
            .is_some_and(|p| known_helper(Path::new(p), helper))
    {
        return false;
    }
    let Some(args) = entry["args"]
        .as_array()
        .and_then(|a| a.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
    else {
        return false;
    };
    if !matches!(args.len(), 7 | 8)
        || args[0] != "copilot-mcp"
        || args[1] != "--site"
        || args[3] != "--workspace"
        || args[5] != "--account"
        || (args.len() == 8 && args[7] != "--dev")
    {
        return false;
    }
    let binding = super::super::Binding {
        site_url: args[2].into(),
        workspace_id: args[4].into(),
        account_id: args[6].into(),
    };
    binding
        .endpoint()
        .is_some_and(|url| endpoint.is_none_or(|expected| expected == url))
}

fn known_helper(path: &Path, current: Option<&Path>) -> bool {
    path.is_absolute()
        && (current == Some(path)
            || [
                ".local/bin/agent-relay-probe",
                ".local/bin/agent-relay-probe.exe",
                ".agentworkforce/desktop/bin/agent-relay-probe",
                ".agentworkforce/desktop/bin/agent-relay-probe.exe",
                "Agent Relay.app/Contents/Helpers/Agent Relay Helper",
                "Agent Relay Dev.app/Contents/Helpers/Agent Relay Helper",
            ]
            .iter()
            .any(|suffix| path.ends_with(suffix)))
}

pub(crate) struct CopilotBackup {
    file: ProviderFile,
    original: String,
}
impl ProviderFile {
    pub(crate) fn copilot_backup(&self, hosted: &HostedConfig) -> Result<CopilotBackup> {
        if !matches!(self.provider, Provider::Copilot | Provider::CopilotHarness) {
            return Err("Expected a Copilot configuration file".into());
        }
        if !hosted.valid() {
            return Err("Invalid hosted MCP authentication configuration".into());
        }
        let original = self.read()?;
        self.render(&original, Some(hosted), Some(&hosted.endpoint))?;
        Ok(CopilotBackup {
            file: self.clone(),
            original,
        })
    }
}
impl CopilotBackup {
    pub(crate) fn restore(&self, hosted: &HostedConfig) -> Result<()> {
        let current = self.file.read()?;
        let replacement = if current
            == self
                .file
                .render(&self.original, Some(hosted), Some(&hosted.endpoint))?
        {
            self.original.clone()
        } else {
            json_document::restore_copilot(
                self.file.provider,
                &current,
                &self.original,
                hosted,
                self.file.helper_for(Some(hosted)),
            )?
        };
        if replacement == current {
            return Ok(());
        }
        files::update(&self.file.path, |observed| {
            (observed == current).then_some(replacement.clone())
        })
        .map_err(|_| "Cannot roll back Copilot configuration".to_string())?;
        if self.file.read()? != replacement {
            return Err("Copilot configuration changed during rollback; repair is required".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
