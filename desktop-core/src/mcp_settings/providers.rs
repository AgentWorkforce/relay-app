//! Native global hosted MCP configurations. Authentication is supplied by the
//! desktop's revocable provider grant, never an app access or refresh token.
use super::{files, private_fs, recognized, COPILOT_HISTORY_MCP_SERVER, COPILOT_MCP_SERVER, NAME};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

mod copilot;
mod json_document;
mod paths;
pub(crate) mod pi;
#[cfg(test)]
mod tests;
pub(crate) mod token_file;
mod toml_document;
mod yaml_document;
pub(crate) use paths::Locations;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Provider {
    Cursor,
    CopilotHarness,
    Grok,
    OpenCodeV1,
    OpenCodeV2,
    Devin,
    Muse,
    OpenClaw,
    Hermes,
    /// Factory Droid: `~/.factory/mcp.json` under `mcpServers`.
    Droid,
    /// GitHub Copilot in VS Code: the user `mcp.json` under `servers`.
    Copilot,
}

impl Provider {
    fn server_name(self) -> &'static str {
        match self {
            Self::Copilot => COPILOT_HISTORY_MCP_SERVER,
            Self::CopilotHarness => COPILOT_MCP_SERVER,
            _ => NAME,
        }
    }
}

/// Deliberately not Debug: headers contain a revocable device credential.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct HostedConfig {
    pub endpoint: String,
    pub headers: BTreeMap<String, String>,
    pub bearer_token_file: Option<PathBuf>,
    pub copilot_stdio: Option<CopilotStdio>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CopilotStdio {
    pub command: PathBuf,
    pub args: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderFile {
    pub provider: Provider,
    pub path: PathBuf,
    copilot_helper: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Inspection {
    pub present: bool,
    pub configured: bool,
}

type Result<T> = std::result::Result<T, String>;
const OWNER_HEADER: &str = "X-Agent-Relay-Managed-By";
const OWNER: &str = "agent-relay-desktop";

impl HostedConfig {
    fn entry(&self, provider: Provider) -> Value {
        if provider == Provider::CopilotHarness {
            if let Some(stdio) = &self.copilot_stdio {
                return json!({"type":"stdio","command":stdio.command,"args":stdio.args,
                    "tools":["*"],"agentRelayManagedBy":OWNER});
            }
        }
        let mut entry = json!({"url":self.endpoint,"headers":self.headers});
        entry["headers"][OWNER_HEADER] = json!(OWNER);
        match provider {
            Provider::Cursor => {}
            Provider::CopilotHarness => {
                entry["type"] = json!("http");
                entry["tools"] = json!(["*"]);
            }
            Provider::Grok => {
                if let Some(path) = &self.bearer_token_file {
                    entry["bearer_token_file"] = json!(path);
                    if let Some(headers) = entry["headers"].as_object_mut() {
                        headers.retain(|key, _| !key.eq_ignore_ascii_case("authorization"));
                    }
                }
            }
            Provider::OpenCodeV1 | Provider::OpenCodeV2 => {
                entry["type"] = json!("remote");
                entry["oauth"] = json!(false);
                if provider == Provider::OpenCodeV1 {
                    entry["enabled"] = json!(true);
                } else {
                    entry["disabled"] = json!(false);
                }
            }
            Provider::Devin => {
                entry["transport"] = json!("http");
                entry["disabled"] = json!(false);
            }
            Provider::Muse => {
                entry["type"] = json!("streamable-http");
                entry["enabled"] = json!(true);
            }
            Provider::OpenClaw => {
                entry["transport"] = json!("streamable-http");
                entry["enabled"] = json!(true);
            }
            Provider::Hermes => entry["enabled"] = json!(true),
            Provider::Droid => entry["type"] = json!("http"),
            Provider::Copilot => entry["type"] = json!("http"),
        }
        entry
    }

    fn valid(&self) -> bool {
        url::Url::parse(&self.endpoint).is_ok_and(|url| {
            (url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
                && url.host().is_some()
                && url.path() == super::ROUTE
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        }) && !self.headers.is_empty()
            && self.headers.iter().all(|(key, value)| {
                !key.is_empty()
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && !value.is_empty()
                    && !value.contains(['\r', '\n'])
            })
    }
}

impl ProviderFile {
    pub(crate) fn new(provider: Provider, path: PathBuf) -> Self {
        Self {
            provider,
            path,
            copilot_helper: None,
        }
    }

    pub(crate) fn with_copilot_helper(mut self, helper: PathBuf) -> Self {
        self.copilot_helper = Some(helper);
        self
    }

    fn helper_for<'a>(&'a self, hosted: Option<&'a HostedConfig>) -> Option<&'a Path> {
        self.copilot_helper.as_deref().or_else(|| {
            hosted
                .and_then(|h| h.copilot_stdio.as_ref())
                .map(|s| s.command.as_path())
        })
    }

    pub(crate) fn install(&self, hosted: &HostedConfig) -> Result<()> {
        if !hosted.valid() {
            return Err("Invalid hosted MCP authentication configuration".into());
        }
        self.change(Some(hosted), Some(&hosted.endpoint))?;
        if !self.inspect(Some(hosted))?.configured {
            return Err(
                "Provider configuration changed while installing; retry installation".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn validate_install(&self, hosted: &HostedConfig) -> Result<()> {
        if !hosted.valid() {
            return Err("Invalid hosted MCP authentication configuration".into());
        }
        self.render(&self.read()?, Some(hosted), Some(&hosted.endpoint))
            .map(|_| ())
    }

    /// Remove only recognizable Agent Relay endpoints/legacy commands. A custom
    /// server named agent-relay is never considered proof of ownership.
    pub(crate) fn remove(&self, endpoint: Option<&str>) -> Result<()> {
        self.change(None, endpoint)?;
        let servers = self.servers(&self.read()?)?;
        if servers
            .values()
            .any(|entry| managed_for(self.provider, entry, endpoint, self.helper_for(None)))
        {
            return Err("Provider configuration changed while removing; retry removal".into());
        }
        Ok(())
    }

    pub(crate) fn inspect(&self, hosted: Option<&HostedConfig>) -> Result<Inspection> {
        let original = self.read()?;
        let servers = self.servers(&original)?;
        let endpoint = hosted.map(|h| h.endpoint.as_str());
        let helper = self.helper_for(hosted);
        let present = servers
            .values()
            .any(|entry| managed_for(self.provider, entry, endpoint, helper));
        let private =
            fs::metadata(&self.path).is_ok_and(|meta| private_fs::private_to_owner(&meta, 0o077));
        let schema_ready = self.provider != Provider::Muse
            || json5::from_str::<Value>(&original).is_ok_and(|root| root["schema_version"] == 1);
        let configured = private
            && schema_ready
            && (self.provider != Provider::Copilot || !servers.contains_key(COPILOT_MCP_SERVER))
            && hosted.is_some_and(|h| {
                servers.get(self.provider.server_name()) == Some(&h.entry(self.provider))
                    && servers.iter().all(|(key, entry)| {
                        key == self.provider.server_name()
                            || !managed_for(self.provider, entry, endpoint, helper)
                    })
            });
        Ok(Inspection {
            present,
            configured,
        })
    }

    fn read(&self) -> Result<String> {
        safe_parent(&self.path)?;
        match fs::symlink_metadata(&self.path) {
            Ok(meta)
                if meta.is_file()
                    && private_fs::owned_by_current_user(&meta)
                    && private_fs::private_to_owner(&meta, 0o022) =>
            {
                fs::read_to_string(&self.path)
                    .map_err(|_| "Cannot read provider configuration".into())
            }
            Ok(_) => Err(
                "Provider configuration must be an owned regular file without shared write access"
                    .into(),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(_) => Err("Cannot inspect provider configuration".into()),
        }
    }

    fn change(&self, hosted: Option<&HostedConfig>, endpoint: Option<&str>) -> Result<()> {
        safe_parent(&self.path)?;
        let original = self.read()?;
        let replacement = self.render(&original, hosted, endpoint)?;
        let private =
            fs::metadata(&self.path).is_ok_and(|meta| private_fs::private_to_owner(&meta, 0o077));
        if replacement == original && (hosted.is_none() || private) {
            return Ok(());
        }
        files::update(&self.path, |current| {
            (current == original).then_some(replacement)
        })
        .map_err(|_| "Cannot write provider configuration".to_string())
    }

    fn render(
        &self,
        original: &str,
        hosted: Option<&HostedConfig>,
        endpoint: Option<&str>,
    ) -> Result<String> {
        match self.provider {
            Provider::Grok => toml_document::render(original, hosted, endpoint),
            Provider::Hermes => yaml_document::render(original, hosted, endpoint),
            _ => json_document::render(
                self.provider,
                original,
                hosted,
                endpoint,
                self.helper_for(hosted),
            ),
        }
    }

    fn servers(&self, original: &str) -> Result<Map<String, Value>> {
        match self.provider {
            Provider::Grok => toml_document::servers(original),
            Provider::Hermes => yaml_document::servers(original),
            _ => json_document::servers(self.provider, original),
        }
    }
}

pub(super) fn safe_parent(path: &Path) -> Result<()> {
    for parent in path.ancestors().skip(1) {
        // These are platform-owned aliases on macOS; profiles beneath them are
        // still checked component by component before reaching the alias.
        #[cfg(target_os = "macos")]
        if matches!(parent.to_str(), Some("/var" | "/tmp" | "/etc")) {
            break;
        }
        match fs::symlink_metadata(parent) {
            Ok(meta) if !meta.is_dir() => {
                return Err("Provider configuration directory must not be a symlink".into())
            }
            Ok(meta)
                if private_fs::owned_by_current_user(&meta)
                    && !private_fs::private_to_owner(&meta, 0o022) =>
            {
                return Err("Provider configuration directory permits shared writes".into())
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("Cannot inspect provider configuration directory".into()),
        }
    }
    Ok(())
}

fn update_servers(
    servers: &mut Map<String, Value>,
    hosted: Option<&HostedConfig>,
    endpoint: Option<&str>,
    provider: Provider,
    helper: Option<&Path>,
) -> Result<()> {
    if provider == Provider::Copilot
        && hosted.is_some()
        && servers
            .get(COPILOT_MCP_SERVER)
            .is_some_and(|entry| !managed_for(provider, entry, endpoint, helper))
    {
        return Err("A different VS Code server uses agentrelay and would shadow native Copilot setup; rename that server before connecting".into());
    }
    if hosted.is_some()
        && servers
            .get(provider.server_name())
            .is_some_and(|entry| !managed_for(provider, entry, endpoint, helper))
    {
        return Err(format!(
            "A different server already uses the {} name",
            provider.server_name()
        ));
    }
    servers.retain(|_, entry| !managed_for(provider, entry, endpoint, helper));
    if let Some(hosted) = hosted {
        servers.insert(provider.server_name().into(), hosted.entry(provider));
    }
    Ok(())
}

fn managed(entry: &Value, endpoint: Option<&str>) -> bool {
    managed_with_helper(entry, endpoint, None)
}

fn managed_for(
    provider: Provider,
    entry: &Value,
    endpoint: Option<&str>,
    helper: Option<&Path>,
) -> bool {
    if provider == Provider::Copilot {
        copilot::managed_legacy(entry)
    } else {
        managed_with_helper(entry, endpoint, helper)
    }
}

fn managed_with_helper(entry: &Value, endpoint: Option<&str>, helper: Option<&Path>) -> bool {
    copilot::managed_stdio(entry, endpoint, helper)
        || recognized::json(entry, endpoint, None)
        || owned_http(entry)
}

fn owned_http(entry: &Value) -> bool {
    entry["headers"][OWNER_HEADER] == OWNER
        && entry["url"].as_str().is_some_and(|value| {
            url::Url::parse(value).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.path() == super::ROUTE
                    && url.host().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
            })
        })
}
