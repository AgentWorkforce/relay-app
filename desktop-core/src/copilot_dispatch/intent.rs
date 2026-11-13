//! Bounded, local, exact-call capabilities. These are never Cloud proofs.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const LEASE: Duration = Duration::from_secs(900);
const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 8 << 20;
const PREFIX: &str = "copilot-intent-v1.";

#[derive(Clone, PartialEq)]
pub(crate) struct Call {
    pub sid: String,
    pub tool: String,
    pub args: Value,
    pub owner: Owner,
    pub peer: Peer,
    pub journal: Anchor,
    pub authority: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Owner {
    pub site: String,
    pub user: String,
    pub workspace: String,
    pub generation: u64,
    pub source: String,
    pub registration: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Peer {
    pub runtime: agent_relay_node::relay::process::ProcessInfo,
    pub hold: FileIdentity,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub birth_ns: u128,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Anchor {
    pub file: FileIdentity,
    pub offset: u64,
}

enum State {
    Issued,
    Dispatching,
    Completed(Value),
    CompletedWithoutBody,
    InDoubt,
    Rejected,
}

struct Entry {
    call: Call,
    issued: Instant,
    state: State,
    bytes: usize,
}

pub(crate) struct Registry {
    entries: Mutex<BTreeMap<String, Entry>>,
    pub(super) verifier: Arc<dyn super::peer::Verifier>,
    http: Arc<AtomicUsize>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            entries: Default::default(),
            verifier: Arc::new(super::peer::Kernel),
            http: Default::default(),
        }
    }
}

pub(super) struct Permit(Arc<AtomicUsize>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) enum Begin {
    Send,
    Cached(Value),
}

fn ticket() -> Result<String, &'static str> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| "intent_unavailable")?;
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

impl Registry {
    #[cfg(test)]
    pub(crate) fn with_verifier(verifier: Arc<dyn super::peer::Verifier>) -> Self {
        Self {
            entries: Default::default(),
            verifier,
            http: Default::default(),
        }
    }
    pub(super) fn http_permit(&self) -> Result<Permit, &'static str> {
        self.http
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < 8).then_some(n + 1)
            })
            .map_err(|_| "dispatch_capacity")?;
        Ok(Permit(self.http.clone()))
    }
    pub(super) fn issue(
        &self,
        mut call: Call,
        now: Instant,
    ) -> Result<(String, Call), &'static str> {
        let ticket = ticket()?;
        if call.tool == "send_relay_message" && call.args.get("idempotency_key").is_none() {
            call.args["idempotency_key"] = json!(format!("copilot_{}", &ticket[PREFIX.len()..]));
        }
        let bytes = serde_json::to_vec(&call.args)
            .map_err(|_| "invalid_call")?
            .len();
        if bytes > 32 * 1024 {
            return Err("invalid_call");
        }
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, e| now.saturating_duration_since(e.issued) < LEASE);
        for entry in entries
            .values_mut()
            .filter(|e| matches!(e.state, State::Issued))
        {
            if entry.call.owner.site != call.owner.site
                || entry.call.owner.user != call.owner.user
                || entry.call.owner.workspace != call.owner.workspace
                || entry.call.owner.generation != call.owner.generation
                || entry.call.owner.source != call.owner.source
                || entry.call.authority != call.authority
                || (entry.call.sid == call.sid
                    && (entry.call.owner != call.owner || entry.call.peer != call.peer))
            {
                entry.state = State::Rejected;
            }
        }
        // Cached outcomes are an optimization, never a permanent quota on new
        // calls. Eviction cannot authorize a retry: a missing ticket is refused.
        while entries.len() >= MAX_ENTRIES
            || entries.values().map(|e| e.bytes).sum::<usize>() + bytes > MAX_BYTES
        {
            let oldest = entries
                .iter()
                .filter(|(_, e)| {
                    matches!(
                        e.state,
                        State::Completed(_) | State::CompletedWithoutBody | State::Rejected
                    )
                })
                .min_by_key(|(_, e)| e.issued)
                .map(|(ticket, _)| ticket.clone());
            let Some(oldest) = oldest else { break };
            entries.remove(&oldest);
        }
        let pending = entries
            .values()
            .filter(|e| matches!(e.state, State::Issued));
        let chat = pending.clone().filter(|e| e.call.sid == call.sid).count();
        let runtime = pending
            .filter(|e| e.call.peer.runtime == call.peer.runtime)
            .count();
        if entries.len() >= MAX_ENTRIES
            || chat >= 8
            || runtime >= 32
            || entries.values().map(|e| e.bytes).sum::<usize>() + bytes > MAX_BYTES
        {
            return Err("intent_capacity");
        }
        entries.insert(
            ticket.clone(),
            Entry {
                call: call.clone(),
                issued: now,
                state: State::Issued,
                bytes,
            },
        );
        Ok((ticket, call))
    }

    pub(crate) fn call(&self, ticket: &str, now: Instant) -> Result<Call, &'static str> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.get(ticket).ok_or("unknown_or_expired_intent")?;
        if now.saturating_duration_since(entry.issued) >= LEASE
            || matches!(entry.state, State::Rejected)
        {
            return Err("unknown_or_expired_intent");
        }
        Ok(entry.call.clone())
    }

    pub(super) fn cached(
        &self,
        ticket: &str,
        call: &Call,
        now: Instant,
    ) -> Result<Option<Value>, &'static str> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.get(ticket).ok_or("unknown_or_expired_intent")?;
        if entry.call != *call || now.saturating_duration_since(entry.issued) >= LEASE {
            return Err("intent_changed_or_expired");
        }
        match &entry.state {
            State::Issued => Ok(None),
            State::Completed(value) => Ok(Some(value.clone())),
            State::CompletedWithoutBody => Err("already_dispatched_result_unavailable"),
            State::Dispatching | State::InDoubt => Err("dispatch_in_doubt_do_not_repeat"),
            State::Rejected => Err("intent_rejected"),
        }
    }

    /// The caller revalidated native ownership and exact arguments immediately
    /// before this atomic transition. Once begun, no path can send it again.
    pub(super) fn begin(
        &self,
        ticket: &str,
        call: &Call,
        now: Instant,
    ) -> Result<Begin, &'static str> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.get_mut(ticket).ok_or("unknown_or_expired_intent")?;
        if entry.call != *call || now.saturating_duration_since(entry.issued) >= LEASE {
            return Err("intent_changed_or_expired");
        }
        match &entry.state {
            State::Issued => {
                entry.state = State::Dispatching;
                Ok(Begin::Send)
            }
            State::Completed(value) => Ok(Begin::Cached(value.clone())),
            State::CompletedWithoutBody => Err("already_dispatched_result_unavailable"),
            State::Dispatching | State::InDoubt => Err("dispatch_in_doubt_do_not_repeat"),
            State::Rejected => Err("intent_rejected"),
        }
    }

    pub(super) fn finish(&self, ticket: &str, response: Option<Value>) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let available = MAX_BYTES.saturating_sub(entries.values().map(|e| e.bytes).sum());
        if let Some(entry) = entries
            .get_mut(ticket)
            .filter(|e| matches!(e.state, State::Dispatching))
        {
            entry.state = match response {
                Some(response) => {
                    let bytes = serde_json::to_vec(&response).map_or(usize::MAX, |b| b.len());
                    if bytes <= available.min(256 * 1024) {
                        entry.bytes += bytes;
                        State::Completed(response)
                    } else {
                        State::CompletedWithoutBody
                    }
                }
                None => State::InDoubt,
            };
        }
    }

    /// Only work that has not crossed the dispatch transition can be retired.
    /// Dispatching and unknown outcomes retain their no-repeat semantics.
    pub(super) fn reject(&self, ticket: &str) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries
            .get_mut(ticket)
            .filter(|e| matches!(e.state, State::Issued))
        {
            entry.state = State::Rejected;
        }
    }

    pub(super) fn retire_invalid(&self, incoming: &Call, invalid: impl Fn(&str, &Call) -> bool) {
        // Do filesystem inspection outside the registry mutex. Pending calls in
        // other chats are not inspected or rejected merely because they wait.
        let candidates: Vec<_> = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, e)| matches!(e.state, State::Issued) && e.call.sid == incoming.sid)
            .map(|(ticket, e)| (ticket.clone(), e.call.clone()))
            .collect();
        for (ticket, call) in candidates {
            if invalid(&ticket, &call) {
                self.reject(&ticket);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_node::relay::process::ProcessInfo;
    fn call(sid: &str) -> Call {
        let file = FileIdentity {
            device: 1,
            inode: 2,
            birth_ns: 3,
        };
        Call {
            sid: sid.into(),
            tool: "send_relay_message".into(),
            args: json!({"to":"peer","text":"hello"}),
            owner: Owner {
                site: "https://example.test".into(),
                user: "u".into(),
                workspace: "w".into(),
                generation: 1,
                source: "s".into(),
                registration: "r".into(),
            },
            peer: Peer {
                runtime: ProcessInfo {
                    pid: 1,
                    parent: 0,
                    uid: 501,
                    start: 99,
                },
                hold: file.clone(),
            },
            journal: Anchor { file, offset: 100 },
            authority: 1,
        }
    }
    #[test]
    fn delayed_approval_redeems_once_and_preserves_the_send_key() {
        let r = Registry::default();
        let now = Instant::now();
        let (ticket, call) = r.issue(call("one"), now).unwrap();
        assert!(call.args["idempotency_key"]
            .as_str()
            .unwrap()
            .starts_with("copilot_"));
        let later = now + Duration::from_secs(185);
        assert!(matches!(r.begin(&ticket, &call, later), Ok(Begin::Send)));
        assert!(r.begin(&ticket, &call, later).is_err());
        r.finish(&ticket, Some(json!({"accepted":true})));
        assert!(matches!(
            r.begin(&ticket, &call, later),
            Ok(Begin::Cached(_))
        ));
    }
    #[test]
    fn unknown_outcome_and_owner_changes_never_resend() {
        let r = Registry::default();
        let now = Instant::now();
        let (ticket, call) = r.issue(call("one"), now).unwrap();
        let mut changed = call.clone();
        changed.owner.generation += 1;
        assert!(r.begin(&ticket, &changed, now).is_err());
        assert!(matches!(r.begin(&ticket, &call, now), Ok(Begin::Send)));
        r.finish(&ticket, None);
        assert!(r.begin(&ticket, &call, now).is_err());
        assert!(r.call(&ticket, now + LEASE).is_err());
    }
    #[test]
    fn shared_runtime_keeps_chats_separate_and_caps_pending_work() {
        let r = Registry::default();
        let now = Instant::now();
        let (ticket, call) = r.issue(call("one"), now).unwrap();
        let mut borrowed = call.clone();
        borrowed.sid = "two".into();
        assert!(r.begin(&ticket, &borrowed, now).is_err());
        for _ in 1..8 {
            r.issue(call.clone(), now).unwrap();
        }
        assert!(r.issue(call, now).is_err());
    }
    #[test]
    fn caller_send_keys_are_preserved_and_large_results_do_not_grow_the_cache() {
        let r = Registry::default();
        let now = Instant::now();
        let mut call = call("one");
        call.args["idempotency_key"] = json!("owner_chosen_key");
        let (ticket, call) = r.issue(call, now).unwrap();
        assert_eq!(call.args["idempotency_key"], "owner_chosen_key");
        r.begin(&ticket, &call, now).unwrap();
        r.finish(&ticket, Some(json!({"text":"x".repeat(300*1024)})));
        assert!(r.begin(&ticket, &call, now).is_err());
    }
    #[test]
    fn cancelled_or_denied_intents_do_not_block_the_next_native_turn() {
        let r = Registry::default();
        let now = Instant::now();
        let mut old = Vec::new();
        for _ in 0..8 {
            old.push(r.issue(call("one"), now).unwrap().0);
        }
        assert!(r.issue(call("one"), now).is_err());
        r.retire_invalid(&call("one"), |ticket, _| old.contains(&ticket.to_owned()));
        assert!(r.issue(call("one"), now).is_ok());
        for ticket in old {
            assert!(r.call(&ticket, now).is_err());
        }
    }
    #[test]
    fn account_generation_and_runtime_replacement_retire_only_unstarted_intents() {
        let r = Registry::default();
        let now = Instant::now();
        let (unknown, active) = r.issue(call("one"), now).unwrap();
        r.begin(&unknown, &active, now).unwrap();
        r.finish(&unknown, None);
        let mut pending = Vec::new();
        for _ in 0..8 {
            pending.push(r.issue(call("one"), now).unwrap().0);
        }
        let mut changed = call("one");
        changed.owner.generation += 1;
        assert!(r.issue(changed.clone(), now).is_ok());
        for ticket in pending {
            assert!(r.call(&ticket, now).is_err());
        }
        assert!(r.begin(&unknown, &active, now).is_err());
        changed.peer.runtime.start += 1;
        assert!(r.issue(changed, now).is_ok());

        // A source reset belongs to this Core, not just the current chat. Old
        // chats must not consume the shared runtime quota after that reset.
        let r = Registry::default();
        let mut old = Vec::new();
        for chat in ["one", "two", "three", "four"] {
            for _ in 0..8 {
                old.push(r.issue(call(chat), now).unwrap().0);
            }
        }
        let mut reset = call("new-chat");
        reset.owner.source = "new-source".into();
        assert!(r.issue(reset, now).is_ok());
        for ticket in old {
            assert!(r.call(&ticket, now).is_err());
        }
    }
    #[test]
    fn completed_cache_capacity_does_not_limit_long_running_chats_or_reauthorize_old_tickets() {
        let r = Registry::default();
        let now = Instant::now();
        let mut first = None;
        for i in 0..(MAX_ENTRIES + 20) {
            let at = now + Duration::from_millis(i as u64);
            let (ticket, call) = r.issue(call("one"), at).unwrap();
            if first.is_none() {
                first = Some((ticket.clone(), call.clone()));
            }
            r.begin(&ticket, &call, now).unwrap();
            r.finish(&ticket, Some(json!({"result":"ok"})));
        }
        let (ticket, call) = first.unwrap();
        assert!(r.begin(&ticket, &call, now).is_err());
        assert!(r.issue(call, now).is_ok());
    }
    #[test]
    fn waiting_parallel_calls_and_unknown_outcomes_are_not_evicted_as_terminal_cache() {
        let r = Registry::default();
        let now = Instant::now();
        let (ticket, call) = r.issue(call("one"), now).unwrap();
        r.retire_invalid(&call, |_, _| false);
        assert!(r.call(&ticket, now).is_ok());
        r.begin(&ticket, &call, now).unwrap();
        r.reject(&ticket);
        r.finish(&ticket, None);
        r.reject(&ticket);
        assert!(r.begin(&ticket, &call, now).is_err());
    }
}
