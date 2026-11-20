//! Priority-one delivery. The server owns ranking and the evaluation gate.
//! Durable reservations fence uncertain handoffs and dedupe human requests
//! across restarts. A notification request is not a display/read receipt.

use super::*;
use agent_relay_node::relay::inject::InjectOutcome;
use std::ops::ControlFlow;
use std::time::{SystemTime, UNIX_EPOCH};

const FILE: &str = "coordination-urgent-receipts.json";
/// Where an unreadable journal is moved, replacing any earlier one.
const BAD_FILE: &str = "coordination-urgent-receipts.json.bad";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RECORDS: usize = 4096;
const RETENTION: u64 = 8 * 24 * 60 * 60;
const HOUR: u64 = 60 * 60;
const HUMAN_HOURLY_CAP: usize = 3;
/// A note whose injection asked to be retried waits this long (seconds).
const URGENT_RETRY_AFTER: u64 = 5 * 60;
/// How long an accepted (queued) note stays the core's before the turn-end
/// hook may hand it out instead (seconds).
const ACCEPTED_OWNED_FOR: u64 = 30 * 60;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum Attempt {
    /// Persisted before the provider call. After a crash, it is uncertain.
    Reserved,
    Accepted,
    Delivered,
    Uncertain,
    Retry,
    Refused,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AgentReceipt {
    scope: receipts::Scope,
    target: Target,
    id: String,
    at: u64,
    attempt: Attempt,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HumanBatch {
    scope: receipts::Scope,
    ids: Vec<String>,
    at: u64,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    agents: Vec<AgentReceipt>,
    humans: Vec<HumanBatch>,
}

#[derive(Clone)]
pub(super) struct Journal {
    saved: Saved,
    pub(super) status: &'static str,
    blocked: bool,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Why a journal could not be loaded.
enum Fault {
    /// The file could not be opened or read: kept blocked, never set aside.
    Io,
    /// Read in full, but too large, not JSON, an unknown version or invalid.
    Unreadable,
}

impl Journal {
    pub(super) fn load(directory: &Path) -> Self {
        use std::io::Read;
        // `Unreadable` only for a file read in full whose content is bad; an I/O
        // error on the file keeps the journal blocked (its receipts may be fine).
        let read = || -> Result<Saved, Fault> {
            let file = match std::fs::File::open(directory.join(FILE)) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Saved {
                        version: 1,
                        ..Saved::default()
                    })
                }
                Err(_) => return Err(Fault::Io),
            };
            if file.metadata().map_err(|_| Fault::Io)?.len() > MAX_BYTES {
                return Err(Fault::Unreadable);
            }
            let mut bytes = Vec::new();
            file.take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| Fault::Io)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(Fault::Unreadable);
            }
            let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| Fault::Unreadable)?;
            if saved.version != 1
                || saved.agents.len() + saved.humans.iter().map(|b| b.ids.len()).sum::<usize>()
                    > MAX_RECORDS
            {
                return Err(Fault::Unreadable);
            }
            if saved
                .agents
                .iter()
                .any(|r| !r.target.is_valid() || !plain_id(&r.id))
                || saved
                    .humans
                    .iter()
                    .any(|b| b.ids.is_empty() || b.ids.iter().any(|id| !plain_id(id)))
            {
                return Err(Fault::Unreadable);
            }
            Ok(saved)
        };
        match read() {
            Ok(saved) => Self {
                saved,
                status: "idle",
                blocked: false,
            },
            // An unreadable, oversized or unknown-version journal is set
            // aside rather than blocking urgent delivery on every launch.
            // Starting fresh can repeat a note that was in doubt; that is
            // preferred to never delivering one again.
            Err(Fault::Unreadable)
                if std::fs::rename(directory.join(FILE), directory.join(BAD_FILE)).is_ok() =>
            {
                Self {
                saved: Saved {
                    version: 1,
                    ..Saved::default()
                },
                    status: "Urgent receipt storage could not be read; it was set aside and started again.",
                    blocked: false,
                }
            }
            Err(_) => Self {
                saved: Saved::default(),
                status: "Urgent delivery unavailable: receipt storage cannot be read.",
                blocked: true,
            },
        }
    }

    fn prune(&mut self, now: u64) {
        self.saved
            .agents
            .retain(|r| now.saturating_sub(r.at) < RETENTION);
        self.saved
            .humans
            .retain(|r| now.saturating_sub(r.at) < RETENTION);
    }

    fn save(&mut self, directory: &Path) -> bool {
        if self.blocked
            || self.saved.agents.len()
                + self.saved.humans.iter().map(|b| b.ids.len()).sum::<usize>()
                > MAX_RECORDS
        {
            self.status = "Urgent delivery unavailable: receipt storage is blocked or full.";
            return false;
        }
        let Some(bytes) = serde_json::to_vec(&self.saved)
            .ok()
            .filter(|b| b.len() as u64 <= MAX_BYTES)
        else {
            self.status = "Urgent delivery unavailable: receipt storage is full.";
            return false;
        };
        let ok = agent_relay_node::platform::fs::create_private_dir_all(directory).is_ok()
            && agent_relay_node::platform::fs::write_atomic(&directory.join(FILE), &bytes).is_ok();
        if !ok {
            self.status = "Urgent delivery unavailable: receipt storage could not be saved.";
        }
        ok
    }

    /// The last recorded attempt for this note, and when (epoch seconds).
    fn agent_receipt(&self, principal: &Principal, target: &Target, id: &str) -> Option<(Attempt, u64)> {
        let scope = receipts::Scope::from(principal);
        self.saved
            .agents
            .iter()
            .find(|r| r.scope == scope && &r.target == target && r.id == id)
            .map(|r| (r.attempt.clone(), r.at))
    }

    fn set_agent(
        &mut self,
        principal: &Principal,
        target: &Target,
        id: &str,
        attempt: Attempt,
        now: u64,
    ) {
        let scope = receipts::Scope::from(principal);
        if let Some(r) = self
            .saved
            .agents
            .iter_mut()
            .find(|r| r.scope == scope && &r.target == target && r.id == id)
        {
            r.attempt = attempt;
            r.at = now;
        } else {
            self.saved.agents.push(AgentReceipt {
                scope,
                target: target.clone(),
                id: id.into(),
                at: now,
                attempt,
            });
        }
    }
}

/// In-process side effects, swapped for fakes in tests. Notifications only
/// confirm that a request was emitted, never that an OS displayed it.
pub(super) trait Delivery: Send + Sync {
    fn inject<'a>(
        &'a self,
        target: &'a Target,
        id: &'a str,
        text: &'a str,
    ) -> BoxFuture<'a, InjectOutcome>;
    fn notifications_available(&self) -> bool;
    fn notify(&self, title: &str, body: &str);
    /// Whether a local notice can reach `target` at all (it is registered).
    fn reaches(&self, _target: &Target) -> bool {
        true
    }
}

pub(super) struct Local(pub Arc<Core>);

impl Delivery for Local {
    fn inject<'a>(
        &'a self,
        target: &'a Target,
        id: &'a str,
        text: &'a str,
    ) -> BoxFuture<'a, InjectOutcome> {
        Box::pin(async move {
            match registration(&self.0, target) {
                Some(registration) => self.0.inject_local_notice(&registration, id, text).await,
                None => InjectOutcome::Refused(
                    "This session is not registered for local agent notices.".into(),
                ),
            }
        })
    }
    fn notifications_available(&self) -> bool {
        cfg!(any(target_os = "macos", target_os = "linux", windows))
    }
    fn notify(&self, title: &str, body: &str) {
        self.0.hub.notify(title, body, None);
    }
    fn reaches(&self, target: &Target) -> bool {
        registration(&self.0, target).is_some()
    }
}

/// The live, unarchived registration a local notice for `target` goes to.
pub(super) fn registration(
    core: &Core,
    target: &Target,
) -> Option<agent_relay_node::relay::store::Registration> {
    core.store.read(|store| {
        store
            .registrations
            .iter()
            .find(|r| &r.target == target && r.missing_since.is_none() && r.archived_at.is_none())
            .cloned()
    })
}

impl Advisories {
    /// Snapshot under a short cache lock; serialize and write off the
    /// async runtime, without making a stop hook wait for filesystem I/O.
    async fn save_urgent(&self) -> bool {
        let mut snapshot = self
            .urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let directory = self.app_data_dir.clone();
        let result = tokio::task::spawn_blocking(move || snapshot.save(&directory))
            .await
            .unwrap_or(false);
        if !result {
            self.urgent.lock().unwrap_or_else(|p| p.into_inner()).status =
                "Urgent delivery unavailable: receipt storage could not be saved.";
        }
        result
    }

    fn mark_urgent_shown(&self, principal: &Principal, target: &Target, id: &str, now: Instant) {
        if self.tokens.principal().as_ref() != Some(principal) {
            return;
        }
        let mut state = self.state();
        if state.principal.as_ref() != Some(principal) {
            return;
        }
        state
            .taken
            .entry(target.clone())
            .or_default()
            .entry(id.into())
            .or_insert_with(|| Taken::new(now))
            .shown = true;
        if let Some(waiting) = state.waiting.get_mut(target) {
            waiting.retain(|a| a.id != id);
        }
    }

    pub fn urgent_status(&self) -> String {
        self.urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .status
            .into()
    }

    /// IDs whose provider already owns the handoff, or whose handoff is
    /// uncertain: a stop hook must not blindly submit them a second time.
    pub(super) fn provider_owned(&self, principal: &Principal, target: &Target) -> HashSet<String> {
        let scope = receipts::Scope::from(principal);
        let now = unix_now();
        self.urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .saved
            .agents
            .iter()
            .filter(|r| {
                r.scope == scope
                    && &r.target == target
                    && match r.attempt {
                        Attempt::Reserved | Attempt::Delivered | Attempt::Uncertain => true,
                        // Handed to the core's queue: owned while the core
                        // may still deliver it; after that the turn's end
                        // gets it (a lost queued notice is not lost for good).
                        Attempt::Accepted => now.saturating_sub(r.at) < ACCEPTED_OWNED_FOR,
                        Attempt::Retry | Attempt::Refused => false,
                    }
            })
            .map(|r| r.id.clone())
            .collect()
    }

    pub(super) async fn deliver_urgent(&self, delivery: &dyn Delivery, now: Instant) {
        let _one = self.urgent_run.lock().await;
        if !self.enabled() {
            return;
        }
        let Some(principal) = self.tokens.principal() else {
            return;
        };
        let Some(candidates) = self.urgent_candidates(&principal) else {
            return;
        };
        let epoch = unix_now();
        // A new pass: last pass's one-off status (sent, hourly cap, a save
        // that failed then, a journal set aside) no longer applies. Read
        // access is the poll's to clear.
        self.reset_urgent_status_if(|status| !status.starts_with("Read access unavailable"));
        self.deliver_human(delivery, principal.clone(), &candidates, epoch)
            .await;
        for (target, advisory) in &candidates {
            if !self.urgent_still_current(&principal) {
                return;
            }
            let step = self
                .deliver_urgent_agent(delivery, &principal, target, advisory, epoch, now)
                .await;
            if step.is_break() {
                return;
            }
        }
    }

    /// Clears the last pass's status, so a changed setting or a new pass
    /// never shows a stale one; a blocked journal keeps its reason.
    pub(super) fn reset_urgent_status(&self) {
        let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        if !journal.blocked {
            journal.status = "idle";
        }
    }

    /// [`Advisories::reset_urgent_status`], only when the current status
    /// matches `stale`.
    pub(super) fn reset_urgent_status_if(&self, stale: impl Fn(&str) -> bool) {
        let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        if !journal.blocked && stale(journal.status) {
            journal.status = "idle";
        }
    }

    /// The switch is still on and the same account is still signed in.
    fn urgent_still_current(&self, principal: &Principal) -> bool {
        self.enabled() && self.tokens.principal().as_ref() == Some(principal)
    }

    /// The urgent rows waiting for this account's sessions; `None` when the
    /// cache belongs to another account.
    fn urgent_candidates(&self, principal: &Principal) -> Option<Vec<(Target, Advisory)>> {
        let state = self.state();
        if state.principal.as_ref() != Some(principal) {
            return None;
        }
        Some(
            state
                .waiting
                .iter()
                .flat_map(|(target, rows)| {
                    rows.iter()
                        .filter(|a| a.urgent())
                        .map(move |a| (target.clone(), a.clone()))
                })
                .collect(),
        )
    }

    async fn deliver_human(
        &self,
        delivery: &dyn Delivery,
        principal: Principal,
        candidates: &[(Target, Advisory)],
        epoch: u64,
    ) {
        if !self.notifications_enabled() {
            self.urgent.lock().unwrap_or_else(|p| p.into_inner()).status =
                "Urgent system notifications are off.";
            return;
        }
        if !self.enabled() || self.tokens.principal().as_ref() != Some(&principal) {
            return;
        }
        if !delivery.notifications_available() || principal.user_id.is_none() {
            self.urgent.lock().unwrap_or_else(|p| p.into_inner()).status =
                "System notifications unavailable on this platform.";
            return;
        }
        let scope = receipts::Scope::from(&principal);
        let human = {
            let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
            if journal.blocked {
                return;
            }
            journal.prune(epoch);
            let mut seen: HashSet<String> = journal
                .saved
                .humans
                .iter()
                .filter(|b| b.scope == scope)
                .flat_map(|b| b.ids.clone())
                .collect();
            let rows: Vec<&Advisory> = candidates
                .iter()
                .map(|(_, a)| a)
                .filter(|a| a.notify_eligible && seen.insert(a.id.clone()))
                .take(MAX_ROWS)
                .collect();
            if rows.is_empty() {
                if candidates.iter().any(|(_, a)| !a.notify_eligible) {
                    journal.status="System notifications are withheld until alert quality is validated.";
                }
                return;
            }
            if journal
                .saved
                .humans
                .iter()
                .filter(|b| b.scope == scope && epoch.saturating_sub(b.at) < HOUR)
                .count()
                >= HUMAN_HOURLY_CAP
            {
                journal.status="System notification hourly limit reached.";
                return;
            }
            let ids = rows.iter().map(|a| a.id.clone()).collect();
            let body = one_line(
                &agent_relay_node::relay::delivery::policy::frame::body(
                    &rows
                        .iter()
                        .map(|a| a.message.as_str())
                        .collect::<Vec<_>>()
                        .join(" · "),
                ),
                MAX_MESSAGE,
            );
            let title = if rows.len() == 1 {
                "Urgent coordination alert".into()
            } else {
                format!("{} urgent coordination alerts", rows.len())
            };
            let before = journal.saved.clone();
            journal.saved.humans.push(HumanBatch {
                scope,
                ids,
                at: epoch,
            });
            (title, body, before)
        };
        if !self.save_urgent().await {
            self.urgent.lock().unwrap_or_else(|p| p.into_inner()).saved = human.2;
            return;
        }
        if self.enabled()
            && self.notifications_enabled()
            && self.tokens.principal().as_ref() == Some(&principal)
        {
            delivery.notify(&human.0, &human.1);
            self.urgent.lock().unwrap_or_else(|p| p.into_inner()).status =
                "System notification requested; OS display is not confirmed.";
        }
    }
}

mod agent;

#[cfg(test)]
mod tests;
