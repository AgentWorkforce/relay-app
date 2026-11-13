//! Native control of an existing daemon. Never starts a daemon or chat.
use super::*;
use agent_relay_node::relay::codex::{
    app_server,
    runner::{CodexRunner, SystemRunner},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// Maintenance retries a verified or empty daemon at most this often.
const COOLDOWN: Duration = Duration::from_secs(30);
/// A daemon still starting with the app, or a slow first answer, gets one
/// earlier retry; a second consecutive failure falls back to `COOLDOWN` so an
/// absent daemon is not polled every few seconds.
const FIRST_FAILURE_RETRY: Duration = Duration::from_secs(5);
/// Budget for each `refresh_mcp` call.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(8);
/// Pause before re-reading chats that had not finished loading the server
/// right after `config/mcpServer/reload`.
const SETTLE_PAUSE: Duration = Duration::from_millis(1500);
/// Whole attempt, including the settle pause and the second status read.
const ATTEMPT_BUDGET: Duration = Duration::from_secs(10);
/// A second read with less time than this left is skipped.
const SETTLE_MIN_READ: Duration = Duration::from_secs(1);

type Report = app_server::McpReloadReport;

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
    /// Check connection asked for the next attempt; it is judged strictly.
    check_requested: bool,
    /// The attempt in flight is an explicit Check connection.
    checking: bool,
    /// Consecutive automatic attempts that could not reach or verify the daemon.
    failures: u8,
    /// The first successful attempt in this scope that verified the server in
    /// some chat (or saw no chats) recorded which chats were already open
    /// without it; later chats are judged, earlier ones are grandfathered.
    baseline: bool,
    /// Chats open before Agent Relay's configuration was applied that still
    /// lack the server entirely. A running chat only loads new MCP servers
    /// when it is reloaded, so automatic checks do not report them as a problem.
    grandfathered: BTreeSet<String>,
    /// Scope in which a reload already left only grandfathered chats without
    /// the tools; maintenance reads instead of reloading every chat again.
    reloaded: Option<String>,
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
        let check = self.check_requested;
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
        let cooldown = if self.failures == 1 {
            FIRST_FAILURE_RETRY
        } else {
            COOLDOWN
        };
        if (!desired && !self.cleanup)
            || self
                .attempted
                .is_some_and(|time| now.saturating_duration_since(time) < cooldown)
        {
            self.check_requested = check;
            return None;
        }
        self.attempted = Some(now);
        self.check_requested = false;
        self.checking = check;
        if check {
            // Check connection judges every loaded chat, older ones included.
            self.grandfathered.clear();
            self.baseline = true;
            self.reloaded = None;
        }
        let proven = [&self.stamp, &self.reloaded]
            .into_iter()
            .any(|stamp| stamp.as_deref() == Some(scope));
        Some((self.generation, !proven))
    }

    fn complete(&mut self, scope: &str, generation: u64, result: Result<Report, ()>) {
        if self.scope.as_deref() != Some(scope) || self.generation != generation {
            return;
        }
        let explicit = std::mem::take(&mut self.checking);
        self.stamp = None;
        self.detail = Some(match result {
            Ok(report) if self.desired => {
                self.failures = 0;
                self.judge_installed(scope, &report);
                installed_detail(&report, self.grandfathered.len())
            }
            Ok(report) => {
                self.failures = 0;
                self.unhealthy = !report.loaded.is_empty() && report.loaded != report.absent;
                self.cleanup = self.unhealthy;
                format!("Configuration refreshed on the reachable Codex daemon. Agent Relay tools absent from {} of {} exact loaded chats. Private app servers and standalone clients outside this daemon remain unverified.", report.absent.len(), report.loaded.len())
            }
            Err(()) => self.failed(explicit),
        });
    }

    /// Judges chats against wanted tools. The first successful attempt in a
    /// scope that verifies the server in some chat grandfathers chats still
    /// without it; only the rest decide health and the verified stamp. A
    /// server verified nowhere may be broken, so no baseline is taken then.
    fn judge_installed(&mut self, scope: &str, report: &Report) {
        if !self.baseline && (report.loaded.is_empty() || !report.verified.is_empty()) {
            self.baseline = true;
            self.grandfathered = report.absent.clone();
        }
        // A chat that has the server now, even unverified, is judged.
        self.grandfathered
            .retain(|thread| report.absent.contains(thread));
        let mut judged = report
            .loaded
            .iter()
            .filter(|thread| !self.grandfathered.contains(*thread))
            .peekable();
        let any = judged.peek().is_some();
        let healthy = judged.all(|thread| report.verified.contains(thread));
        self.unhealthy = !healthy;
        if any && healthy {
            self.stamp = Some(scope.into());
        }
        self.reloaded = (healthy && !self.grandfathered.is_empty()).then(|| scope.into());
        self.cleanup = false;
    }

    /// A failed attempt proves nothing either way. `cleanup` stays as it was,
    /// so a pending unload is retried. Only Check connection reports it;
    /// maintenance keeps the last health and its detail, notes the retry, and
    /// tries again, once sooner than the usual cooldown.
    fn failed(&mut self, explicit: bool) -> String {
        const DETAIL: &str = "Automatic reload could not reach or verify an existing Codex daemon. Private Codex app servers and standalone clients expose no supported external reload channel; their running chats are not verified.";
        const RETRY: &str = " Agent Relay will try again automatically.";
        if explicit {
            self.unhealthy = true;
            self.failures = 0;
            return DETAIL.into();
        }
        self.failures = self.failures.saturating_add(1);
        let last = self.detail.take().filter(|_| self.unhealthy);
        let base = last.as_deref().map_or(DETAIL, |detail| {
            detail.strip_suffix(RETRY).unwrap_or(detail)
        });
        format!("{base}{RETRY}")
    }

    /// Whether a reload left chats that have not settled into the wanted
    /// state yet (tools connected, or removed when unwanted). Grandfathered
    /// chats need the user to reload them, so waiting for them is pointless.
    fn unsettled(&self, report: &Report) -> bool {
        let target = if self.desired {
            &report.verified
        } else {
            &report.absent
        };
        report
            .loaded
            .iter()
            .any(|thread| !target.contains(thread) && !self.grandfathered.contains(thread))
    }
}

fn installed_detail(report: &Report, grandfathered: usize) -> String {
    if report.loaded.is_empty() {
        return "Configuration refreshed on the reachable Codex daemon. No loaded chats were available; no running chat has been verified. Private app servers and standalone clients remain unverified. Agent Relay sign-in and each chat’s identity are checked separately.".into();
    }
    let older = match grandfathered {
        0 => String::new(),
        1 => " 1 chat was open before Agent Relay was set up and still needs a reload to load the tools.".into(),
        count => format!(" {count} chats were open before Agent Relay was set up and still need a reload to load the tools."),
    };
    format!("Configuration refreshed on the reachable Codex daemon. Required hosted tools observed for {} of {} exact loaded chats.{older} Private Codex app servers and standalone clients outside this daemon are not verified. Agent Relay sign-in and each chat’s identity are checked separately.", report.verified.len(), report.loaded.len())
}

/// One attempt: `refresh(reload, timeout)` opens a fresh daemon connection
/// each call. A reload restarts the server in every loaded chat, so a chat
/// can still be starting when it is first read; `unsettled` chats get one
/// more read without a reload after `SETTLE_PAUSE`, inside `ATTEMPT_BUDGET`.
/// The second read is authoritative; if it fails the attempt fails, since the
/// first read may have caught chats mid-restart.
fn settle_attempt(
    reload: bool,
    unsettled: impl Fn(&Report) -> bool,
    mut refresh: impl FnMut(bool, Duration) -> Result<Report, ()>,
    pause: impl FnOnce(Duration),
) -> Result<Report, ()> {
    let started = Instant::now();
    let first = refresh(reload, REFRESH_TIMEOUT)?;
    if !reload || !unsettled(&first) {
        return Ok(first);
    }
    let left = ATTEMPT_BUDGET.saturating_sub(started.elapsed());
    let Some(read) = left
        .checked_sub(SETTLE_PAUSE)
        .filter(|read| *read >= SETTLE_MIN_READ)
    else {
        return Ok(first);
    };
    pause(SETTLE_PAUSE);
    refresh(false, read.min(REFRESH_TIMEOUT))
}

impl Installer {
    /// Explicit Check connection retries now, even inside maintenance's backoff.
    pub(crate) fn recheck_runtime(&self, binding: &Binding) {
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            runtime.attempted = None;
            runtime.check_requested = true;
        }
        self.reconcile_runtime(binding);
        // A check that started no attempt must not make later maintenance strict.
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .check_requested = false;
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
        let (attempt, runtime) = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let attempt = runtime.prepare(&scope, desired, Instant::now());
            let snapshot = Runtime {
                desired: runtime.desired,
                grandfathered: runtime.grandfathered.clone(),
                ..Default::default()
            };
            (attempt, snapshot)
        };
        let Some((generation, reload)) = attempt else {
            return;
        };
        // No runtime lock across IPC: Tools status remains a cached read.
        let result = settle_attempt(
            reload,
            |report| runtime.unsettled(report),
            |reload, timeout| {
                let executable = self.codex.executable.as_ref().ok_or(())?;
                let mut stream = SystemRunner
                    .proxy_in_home(executable, &self.codex.home, &self.codex.sqlite_home)
                    .map_err(|_| ())?;
                app_server::refresh_mcp(stream.as_mut(), self.helper.server(), reload, timeout)
                    .map_err(|_| ())
            },
            std::thread::sleep,
        );
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
