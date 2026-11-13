//! Native control of an existing daemon. Never starts a daemon or chat.
use super::*;
use agent_relay_node::relay::codex::{
    app_server,
    runner::{CodexRunner, SystemRunner},
};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Runtime {
    pub enabled: bool,
    scope: Option<String>,
    generation: u64,
    desired: bool,
    cleanup: bool,
    /// An explicit removal asked running chats to drop Relay tools, whatever
    /// this core last wanted (it may not have recorded a desire since restart).
    removed: bool,
    stamp: Option<String>,
    attempted: Option<Instant>,
    detail: Option<String>,
    unhealthy: bool,
}

impl Runtime {
    fn invalidate(&mut self) {
        *self = Self {
            enabled: self.enabled,
            generation: self.generation.wrapping_add(1),
            ..Default::default()
        };
    }

    fn prepare(&mut self, scope: &str, desired: bool, now: Instant) -> Option<(u64, bool)> {
        if !self.enabled {
            return None;
        }
        let removed = std::mem::take(&mut self.removed) && !desired;
        if self.scope.as_deref() != Some(scope) {
            let cleanup = (self.desired || removed) && !desired;
            self.invalidate();
            self.scope = Some(scope.into());
            self.desired = desired;
            self.cleanup = cleanup;
        } else if removed {
            self.cleanup = true;
            self.attempted = None;
        }
        if (!desired && !self.cleanup)
            || self
                .attempted
                .is_some_and(|time| now.saturating_duration_since(time) < Duration::from_secs(30))
        {
            return None;
        }
        self.attempted = Some(now);
        Some((self.generation, self.stamp.as_deref() != Some(scope)))
    }

    fn complete(
        &mut self,
        scope: &str,
        generation: u64,
        result: Result<app_server::McpReloadReport, ()>,
    ) {
        if self.scope.as_deref() != Some(scope) || self.generation != generation {
            return;
        }
        self.stamp = None;
        self.detail = Some(match result {
            Ok(report) => {
                let verified = if self.desired {
                    &report.verified
                } else {
                    &report.absent
                };
                self.unhealthy = !report.loaded.is_empty() && &report.loaded != verified;
                if !report.loaded.is_empty() && &report.loaded == verified {
                    self.stamp = Some(scope.into());
                }
                self.cleanup = !self.desired && self.unhealthy;
                if !self.desired {
                    format!("Configuration refreshed on the reachable Codex daemon. Agent Relay tools absent from {} of {} exact loaded chats. Private app servers and standalone clients outside this daemon remain unverified.", report.absent.len(), report.loaded.len())
                } else if report.loaded.is_empty() {
                    "Configuration refreshed on the reachable Codex daemon. No loaded chats were available; no running chat has been verified. Private app servers and standalone clients remain unverified. Agent Relay sign-in and each chat’s identity are checked separately.".into()
                } else {
                    format!("Configuration refreshed on the reachable Codex daemon. Required hosted tools observed for {} of {} exact loaded chats. Private Codex app servers and standalone clients outside this daemon are not verified. Agent Relay sign-in and each chat’s identity are checked separately.", report.verified.len(), report.loaded.len())
                }
            }
            Err(()) => {
                self.unhealthy = true;
                "Automatic reload could not reach or verify an existing Codex daemon. Private Codex app servers and standalone clients expose no supported external reload channel; their running chats are not verified.".into()
            }
        });
    }
}

impl Installer {
    /// Explicit Check connection retries now, even inside maintenance's backoff.
    pub(crate) fn recheck_runtime(&self, binding: &Binding) {
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .attempted = None;
        self.reconcile_runtime(binding);
    }

    pub fn with_live_reload(self) -> Self {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .enabled = true;
        self
    }

    pub(super) fn reconcile_runtime(&self, binding: &Binding) {
        let selected = self.selection().ok().flatten().unwrap_or_default();
        let endpoint = binding.endpoint();
        let helper = self.helper.command(binding);
        let desired = selected.iter().any(|id| id == "codex")
            && self
                .proof("codex", endpoint.as_deref(), helper.as_deref())
                .1;
        let Some(scope) = self.runtime_scope(binding, desired) else {
            self.invalidate_runtime();
            return;
        };
        let attempt = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            runtime.prepare(&scope, desired, Instant::now())
        };
        let Some((generation, reload)) = attempt else {
            return;
        };
        // No runtime lock across IPC: Tools status remains a cached read.
        let result = self
            .codex
            .executable
            .as_ref()
            .ok_or(())
            .and_then(|executable| {
                SystemRunner
                    .proxy_in_home(executable, &self.codex.home, &self.codex.sqlite_home)
                    .map_err(|_| ())
            })
            .and_then(|mut stream| {
                app_server::refresh_mcp(
                    stream.as_mut(),
                    self.helper.server(),
                    reload,
                    Duration::from_secs(8),
                )
                .map_err(|_| ())
            });
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime.complete(&scope, generation, result);
    }

    pub(super) fn runtime_detail(
        &self,
        id: &str,
        binding: Option<&Binding>,
        selected: bool,
    ) -> Option<String> {
        if id != "codex" {
            return None;
        }
        let expected = self.runtime_scope(binding?, selected)?;
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (runtime.scope.as_deref() == Some(expected.as_str()))
            .then(|| runtime.detail.clone())
            .flatten()
    }

    /// Disconnect asks the reachable daemon to unload Relay tools even when
    /// this core never saw them wanted, as after a restart.
    pub(super) fn request_runtime_cleanup(&self) {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .removed = true;
    }

    pub(crate) fn invalidate_runtime(&self) {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate();
    }

    pub(super) fn runtime_problem(
        &self,
        id: &str,
        binding: Option<&Binding>,
        selected: bool,
    ) -> Option<String> {
        if id != "codex" {
            return None;
        }
        let expected = self.runtime_scope(binding?, selected)?;
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (runtime.scope.as_deref() == Some(expected.as_str()) && runtime.unhealthy)
            .then(|| runtime.detail.clone())
            .flatten()
    }

    pub(super) fn runtime_unverified(
        &self,
        id: &str,
        binding: Option<&Binding>,
        selected: bool,
    ) -> bool {
        if id != "codex" || !selected {
            return false;
        }
        let expected = binding.and_then(|binding| self.runtime_scope(binding, selected));
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime.enabled && (expected.is_none() || runtime.stamp.as_ref() != expected.as_ref())
    }

    fn runtime_scope(&self, binding: &Binding, selected: bool) -> Option<String> {
        let helper = self.helper.command(binding)?;
        let bytes = match fs::read(self.codex.home.join("config.toml")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return None,
        };
        Some(format!("{selected}:{helper}:{:x}", Sha256::digest(bytes)))
    }
}

#[cfg(test)]
mod tests;
