//! Priority-one delivery. The server owns ranking and the evaluation gate.
//! Durable reservations fence uncertain handoffs and dedupe human requests
//! across restarts. A notification request is not a display/read receipt.

use super::*;
use agent_relay_node::relay::inject::InjectOutcome;
use std::ops::ControlFlow;
use std::time::{SystemTime, UNIX_EPOCH};

const FILE: &str = "coordination-urgent-receipts.json";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RECORDS: usize = 4096;
const RETENTION: u64 = 8 * 24 * 60 * 60;
const HOUR: u64 = 60 * 60;
const HUMAN_HOURLY_CAP: usize = 3;
/// A note whose injection asked to be retried waits this long (seconds).
const URGENT_RETRY_AFTER: u64 = 5 * 60;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum Attempt {
    /// Persisted before the provider call. After a crash, it is uncertain.
    Reserved,
    Accepted,
    Delivered,
    Uncertain,
    Retry,
    Refused,
    /// Held past its time and handed to the turn-end hand-out instead, once,
    /// labelled as possibly a repeat; never injected. Settled.
    Released,
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
    /// Why unusable receipt storage blocks urgent delivery, naming the file
    /// and the repair; `None` when it is usable.
    fault: Option<String>,
    /// When each held note was last observed (epoch seconds); memory only.
    observed: HashMap<(receipts::Scope, Target, String), u64>,
    /// While blocked: the sessions the last urgent pass found no local notice
    /// reaches, whose urgent notes the turn end need not keep back.
    unreached: HashSet<Target>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
    /// Reads the provider's own record of a note already handed over. It takes no text, so it cannot submit.
    fn observe<'a>(&'a self, _target: &'a Target, _id: &'a str) -> BoxFuture<'a, Observation> {
        Box::pin(async { Observation::Unsupported })
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
    /// The core's read-only look at a note it was handed: no text, no
    /// submission. No live registration: nothing is observed (fenced).
    fn observe<'a>(&'a self, target: &'a Target, id: &'a str) -> BoxFuture<'a, Observation> {
        Box::pin(async move {
            let Some(registration) = registration(&self.0, target) else {
                return Observation::Fenced;
            };
            observation_of(self.0.observe_local_notice(&registration, id).await)
        })
    }
}

/// The core's answer as this side's observation. Only the core's
/// `Delivered` settles a note; every other answer leaves it held.
pub(super) fn observation_of(
    seen: agent_relay_node::relay::local_notices::NoticeObservation,
) -> Observation {
    use agent_relay_node::relay::local_notices::NoticeObservation;
    match seen {
        NoticeObservation::Delivered => Observation::Delivered,
        NoticeObservation::Pending(_) => Observation::Pending,
        NoticeObservation::InDoubt(_) => Observation::InDoubt,
        NoticeObservation::Unsupported => Observation::Unsupported,
        NoticeObservation::Fenced(_) => Observation::Fenced,
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

    /// The fault of a blocked journal, else the last pass's status.
    pub fn urgent_status(&self) -> String {
        let journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        journal.fault.clone().unwrap_or_else(|| journal.status.into())
    }

    /// For a stop hook: first, the IDs whose provider already owns the
    /// handoff, or whose handoff is uncertain, which it must not blindly
    /// submit a second time; second, the IDs released to the turn end, whose
    /// line says it may repeat a delivery.
    pub(super) fn provider_owned(
        &self,
        principal: &Principal,
        target: &Target,
    ) -> (HashSet<String>, HashSet<String>) {
        let scope = receipts::Scope::from(principal);
        let journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        let receipts = journal
            .saved
            .agents
            .iter()
            .filter(|r| r.scope == scope && &r.target == target);
        let released = receipts
            .clone()
            .filter(|r| r.attempt == Attempt::Released)
            .map(|r| r.id.clone())
            .collect();
        let owned = receipts
            .filter(|r| match r.attempt {
                Attempt::Reserved | Attempt::Delivered | Attempt::Uncertain => true,
                // Handed to the core's queue: the provider may still deliver
                // it, so it stays the core's until an observation shows it
                // delivered, or it is released (release.rs). Never handed out
                // at a turn's end before that, which could repeat it.
                Attempt::Accepted => true,
                Attempt::Retry | Attempt::Refused | Attempt::Released => false,
            })
            .map(|r| r.id.clone())
            .collect();
        (owned, released)
    }

    pub(super) async fn deliver_urgent(&self, delivery: &dyn Delivery, now: Instant) {
        let _one = self.urgent_run.lock().await;
        if !self.enabled() {
            return;
        }
        let Some(principal) = self.tokens.principal() else {
            return;
        };
        self.retry_blocked_journal().await;
        let Some(candidates) = self.urgent_candidates(&principal) else {
            return;
        };
        self.record_reach(delivery, &candidates);
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
        if !journal.blocked() {
            journal.status = "idle";
        }
    }

    /// [`Advisories::reset_urgent_status`], only when the current status
    /// matches `stale`.
    pub(super) fn reset_urgent_status_if(&self, stale: impl Fn(&str) -> bool) {
        let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        if !journal.blocked() && stale(journal.status) {
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
        // A blocked journal's fault is the only way a person learns what to
        // repair: no later status may replace it.
        if self.urgent.lock().unwrap_or_else(|p| p.into_inner()).blocked() {
            return;
        }
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
        let cached = self.state().cached();
        let human = {
            let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
            if journal.blocked() {
                return;
            }
            journal.prune(epoch, &cached);
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
mod journal;
mod observe;
mod release;

pub(super) use observe::Observation;
pub(super) use release::ACCEPTED_OWNED_FOR;

#[cfg(test)]
mod tests;

impl State {
    /// The ids this process still has cached (waiting or handed out), for the
    /// account the cache belongs to.
    fn cached(&self) -> HashSet<(receipts::Scope, Target, String)> {
        let Some(principal) = self.principal.as_ref() else {
            return HashSet::new();
        };
        let scope = receipts::Scope::from(principal);
        let waiting = self
            .waiting
            .iter()
            .flat_map(|(t, rows)| rows.iter().map(move |a| (t, a.id.clone())));
        let taken = self
            .taken
            .iter()
            .flat_map(|(t, rows)| rows.keys().map(move |id| (t, id.clone())));
        waiting
            .chain(taken)
            .map(|(t, id)| (scope.clone(), t.clone(), id))
            .collect()
    }
}
