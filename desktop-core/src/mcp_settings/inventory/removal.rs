//! Taking a provider's Agent Relay configuration out: its hosted MCP entry,
//! the Claude Code hooks that come with it, and Codex's server entries and
//! their sign-in.
use super::*;

impl Installer {
    pub(in crate::mcp_settings) fn remove(
        &self,
        id: &str,
        binding: Option<&Binding>,
    ) -> Result<(), String> {
        if id == "cursor" {
            self.set_cursor_hooks(false)?;
        }
        if id == "copilot" {
            return self.remove_copilot(binding.and_then(Binding::endpoint).as_deref());
        }
        if Self::credential_provider(id) {
            return self.remove_native(id, binding.and_then(Binding::endpoint).as_deref());
        }
        if let Some(error) = self.config_error(id) {
            return Err(error);
        }
        let endpoint = binding.and_then(Binding::endpoint);
        let helper = self.helper_context(binding);
        if id == "codex" {
            self.remove_codex(endpoint.as_deref(), helper.as_deref());
        } else {
            for path in self.json_paths(id) {
                if id == "claude"
                    && !self.helper.development_scope()
                    && !scopes::clear(&path, endpoint.as_deref(), helper.as_deref(), true)
                {
                    return Err(
                        "Known Claude project configuration could not be cleaned up.".into(),
                    );
                }
                let _ = files::update(&path, |text| {
                    let mut root = parse_config(id, &path, text)?;
                    if !json_remove(&mut root, endpoint.as_deref(), helper.as_deref()) {
                        return None;
                    }
                    Some(format!("{}\n", serde_json::to_string_pretty(&root).ok()?))
                });
            }
            self.remove_claude_hooks(id)?;
            if id == "claude" {
                self.remove_teammate_hooks()?;
            }
        }
        if self.proof(id, endpoint.as_deref(), helper.as_deref()).0 {
            return Err("Recognized Relay configuration could not be removed.".into());
        }
        Ok(())
    }

    fn remove_codex(&self, endpoint: Option<&str>, helper: Option<&str>) {
        let _ = files::update(&self.codex.home.join("config.toml"), |text| {
            let mut doc = text.parse::<DocumentMut>().ok()?;
            let servers = doc.get_mut("mcp_servers")?.as_table_like_mut()?;
            let aliases: Vec<_> = servers
                .iter()
                .filter(|(_, s)| recognized::toml(s, endpoint, helper))
                .map(|(name, _)| name.to_string())
                .collect();
            if aliases.is_empty() {
                return None;
            }
            if !aliases
                .iter()
                .all(|alias| self.clear_codex_auth(servers.get(alias), alias, endpoint, helper))
            {
                return None;
            }
            for alias in aliases {
                servers.remove(&alias);
            }
            Some(doc.to_string())
        });
    }

    fn clear_codex_auth(
        &self,
        server: Option<&Item>,
        alias: &str,
        endpoint: Option<&str>,
        helper: Option<&str>,
    ) -> bool {
        let managed = server
            .and_then(|s| s.get("http_headers_helper"))
            .and_then(Item::as_str)
            .is_some_and(|old| helper.is_some_and(|expected| managed_helper(old, expected)));
        let hosted = server
            .and_then(|s| s.get("url"))
            .and_then(Item::as_str)
            .is_some_and(|url| recognized::endpoint(url, endpoint));
        managed || !hosted || logout::run(&self.codex, alias)
    }
}
