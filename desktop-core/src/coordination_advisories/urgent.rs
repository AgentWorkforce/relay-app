//! Priority-one delivery. The server owns ranking and the evaluation gate.
//! Durable reservations fence uncertain handoffs and dedupe human requests
//! across restarts. A notification request is not a display/read receipt.

use super::*;
use agent_relay_node::relay::inject::InjectOutcome;
use std::time::{SystemTime, UNIX_EPOCH};

const FILE: &str = "coordination-urgent-receipts.json";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RECORDS: usize = 4096;
const RETENTION: u64 = 8 * 24 * 60 * 60;
const HOUR: u64 = 60 * 60;
const HUMAN_HOURLY_CAP: usize = 3;

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

impl Journal {
    pub(super) fn load(directory: &Path) -> Self {
        use std::io::Read;
        let read = || -> Result<Saved, ()> {
            let file = match std::fs::File::open(directory.join(FILE)) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Saved {
                        version: 1,
                        ..Saved::default()
                    })
                }
                Err(_) => return Err(()),
            };
            if file.metadata().map_err(|_| ())?.len() > MAX_BYTES {
                return Err(());
            }
            let mut bytes = Vec::new();
            file.take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| ())?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(());
            }
            let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| ())?;
            if saved.version != 1
                || saved.agents.len() + saved.humans.iter().map(|b| b.ids.len()).sum::<usize>()
                    > MAX_RECORDS
            {
                return Err(());
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
                return Err(());
            }
            Ok(saved)
        };
        match read() {
            Ok(saved) => Self {
                saved,
                status: "idle",
                blocked: false,
            },
            Err(()) => Self {
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

    fn agent(&self, principal: &Principal, target: &Target, id: &str) -> Option<Attempt> {
        let scope = receipts::Scope::from(principal);
        self.saved
            .agents
            .iter()
            .find(|r| r.scope == scope && &r.target == target && r.id == id)
            .map(|r| r.attempt.clone())
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
            let registration = self.0.store.read(|store| {
                store
                    .registrations
                    .iter()
                    .find(|r| {
                        &r.target == target && r.missing_since.is_none() && r.archived_at.is_none()
                    })
                    .cloned()
            });
            match registration {
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
        self.urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .saved
            .agents
            .iter()
            .filter(|r| {
                r.scope == scope
                    && &r.target == target
                    && matches!(
                        r.attempt,
                        Attempt::Reserved
                            | Attempt::Accepted
                            | Attempt::Delivered
                            | Attempt::Uncertain
                    )
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
        let candidates: Vec<(Target, Advisory)> = {
            let state = self.state();
            if state.principal.as_ref() != Some(&principal) {
                return;
            }
            state
                .waiting
                .iter()
                .flat_map(|(target, rows)| {
                    rows.iter()
                        .filter(|a| a.urgent())
                        .map(move |a| (target.clone(), a.clone()))
                })
                .collect()
        };
        let epoch = unix_now();
        self.deliver_human(delivery, principal.clone(), &candidates, epoch)
            .await;
        for (target, advisory) in &candidates {
            if !self.enabled() || self.tokens.principal().as_ref() != Some(&principal) {
                return;
            }
            if advisory.dm_delivered {
                continue;
            }
            let prior = self.urgent.lock().unwrap_or_else(|p| p.into_inner()).agent(
                &principal,
                target,
                &advisory.id,
            );
            if prior == Some(Attempt::Delivered) {
                self.mark_urgent_shown(&principal, target, &advisory.id, now);
                continue;
            }
            if matches!(prior, Some(Attempt::Reserved | Attempt::Uncertain)) {
                continue;
            }
            let before = {
                let state = self.state();
                // A stop hook may have taken this row after the snapshot
                // above. Claim under the same lock before persisting.
                if state.principal.as_ref() != Some(&principal)
                    || !state
                        .waiting
                        .get(target)
                        .is_some_and(|rows| rows.iter().any(|a| a.id == advisory.id))
                    || state
                        .taken
                        .get(target)
                        .is_some_and(|rows| rows.contains_key(&advisory.id))
                {
                    continue;
                }
                let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
                if journal.blocked {
                    None
                } else {
                    journal.prune(epoch);
                    let before = journal.saved.clone();
                    journal.set_agent(&principal, target, &advisory.id, Attempt::Reserved, epoch);
                    Some(before)
                }
            };
            let Some(before) = before else {
                continue;
            };
            if !self.save_urgent().await {
                self.urgent.lock().unwrap_or_else(|p| p.into_inner()).saved = before;
                continue;
            }
            if !self.enabled() || self.tokens.principal().as_ref() != Some(&principal) {
                return;
            }
            let text = format!("{}\n{FOOTER}", line(advisory));
            let outcome = delivery.inject(target, &advisory.id, &text).await;
            let attempt = match outcome {
                InjectOutcome::Delivered => Attempt::Delivered,
                InjectOutcome::Accepted(_) | InjectOutcome::WaitingForTurn(_) => Attempt::Accepted,
                InjectOutcome::InDoubt(_) => Attempt::Uncertain,
                InjectOutcome::Refused(_) => Attempt::Refused,
                InjectOutcome::Waiting(_) | InjectOutcome::Retry(_) => Attempt::Retry,
            };
            {
                let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
                journal.set_agent(&principal, target, &advisory.id, attempt.clone(), epoch);
                // Keep the observed result in memory even if this save fails;
                // the earlier durable reservation prevents a blind restart retry.
            }
            self.save_urgent().await;
            if attempt == Attempt::Delivered && self.tokens.principal().as_ref() == Some(&principal)
            {
                self.mark_urgent_shown(&principal, target, &advisory.id, now);
            }
        }
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

#[cfg(test)]
mod tests;
