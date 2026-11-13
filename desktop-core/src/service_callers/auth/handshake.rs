//! The challenge exchange (`docs/service-callers-auth.md` sections 2 to
//! 4): `spawn_callers.challenge` issues a slot on the connection,
//! `spawn_callers.authenticate` takes it, does the same MAC work whatever
//! it finds, and binds the connection only once the MAC has verified.

use super::transcript::same_mac;
use super::{
    count_attempt, expire_all, hmac, is_caller_id, key_of, parse_authenticate, Bound, Slot, State,
    DUMMY_KEY, ID_MAX, PER_CALLER, SITE_URL_MAX, TOTAL, TTL_MS, VERSION,
};
use crate::contract::service_callers::{CallerBound, CallerChallenge, CallerChallengeParams};
use crate::service_callers::registry::{Binding, CallerRecord, SecretDigest};
use crate::service_callers::{Class, Live, ServiceCallers};
use agent_relay_node::relay::protocol::RpcError;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;

const AUTHENTICATE: &str = "spawn_callers.authenticate";

impl ServiceCallers {
    /// `spawn_callers.challenge` on `connection_id`.
    pub(crate) fn challenge(
        &self,
        peer_pid: Option<i32>,
        connection_id: u64,
        params: &CallerChallengeParams,
    ) -> Result<CallerChallenge, RpcError> {
        const METHOD: &str = "spawn_callers.challenge";
        let binding = Binding::current(&*self.host);
        let refuse = |class| self.refused(binding.as_ref().ok(), None, METHOD, class);
        if !is_caller_id(&params.caller_id) {
            return Err(refuse(Class::InvalidInput));
        }
        // On Windows this is `peer_check_unavailable` (`peer::available`):
        // no challenge is issued for a binding no call could use.
        crate::service_callers::peer::caller(&*self.host, peer_pid).map_err(refuse)?;
        let binding = binding.map_err(|class| self.refused(None, None, METHOD, class))?;
        let caller_id = params.caller_id.as_str();
        let refuse = |class| self.refused(Some(&binding), Some(caller_id), METHOD, class);
        let bounded = |value: &str, max: usize| (1..=max).contains(&value.len());
        if !bounded(&binding.site_url, SITE_URL_MAX)
            || !bounded(&binding.workspace_id, ID_MAX)
            || !bounded(&binding.user_id, ID_MAX)
        {
            return Err(refuse(Class::NotSignedIn));
        }
        let challenge_id = crate::service_callers::random_hex(16)
            .map(|hex| format!("ch_{hex}"))
            .ok_or_else(|| refuse(Class::StoreFailed))?;
        let mut nonce = [0u8; 32];
        getrandom::getrandom(&mut nonce).map_err(|_| refuse(Class::StoreFailed))?;
        let issued = self.host.now_ms();
        let slot = Slot {
            challenge_id: challenge_id.clone(),
            nonce,
            caller_id: params.caller_id.clone(),
            site_url: binding.site_url.clone(),
            workspace_id: binding.workspace_id.clone(),
            user_id: binding.user_id.clone(),
            key: binding.key.clone(),
            issued_ms: issued,
            expires_ms: issued + TTL_MS,
        };
        {
            let mut live = self.live();
            expire_all(&mut live, issued);
            let state = live
                .connections
                .get(&connection_id)
                .filter(|_| !live.closed.contains(connection_id))
                .map(|e| &e.state);
            let attempts = match state {
                Some(State::Bound(_) | State::Stale) => Err(Class::AlreadyBound),
                Some(State::Closed) | None => Err(Class::ConnectionClosed),
                Some(State::Locked) => Err(Class::AuthLocked),
                Some(State::Challenged { slot: Some(_), .. }) => Err(Class::ChallengeOutstanding),
                Some(State::Challenged {
                    slot: None,
                    attempts,
                }) => Ok(*attempts),
                Some(State::Untouched) => Ok(0),
            };
            let attempts = match attempts {
                Ok(attempts) => attempts,
                Err(class) => {
                    drop(live);
                    return Err(refuse(class));
                }
            };
            let (mut total, mut same_caller) = (0, 0);
            for entry in live.connections.values() {
                if let State::Challenged {
                    slot: Some(held), ..
                } = &entry.state
                {
                    total += 1;
                    if held.caller_id == slot.caller_id {
                        same_caller += 1;
                    }
                }
            }
            if total >= TOTAL || same_caller >= PER_CALLER {
                drop(live);
                return Err(refuse(Class::ChallengeCapacity));
            }
            if let Some(entry) = live.connections.get_mut(&connection_id) {
                entry.state = State::Challenged {
                    slot: Some(slot),
                    attempts,
                };
            }
        }
        self.audit(&binding, Some(caller_id), METHOD, "challenge_issued", None);
        Ok(CallerChallenge {
            version: VERSION,
            challenge_id,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            issued_at_ms: issued,
            expires_at_ms: issued + TTL_MS,
        })
    }
    /// `spawn_callers.authenticate` on `connection_id`, from its raw params
    /// so that a malformed or oversized body still consumes the challenge.
    /// `legacy` (the params carry a phase-1 credential key) is answered
    /// `legacy_credential_refused` only after the take, so the outstanding
    /// challenge is consumed and the attempt counted first.
    pub(crate) fn authenticate_connection(
        &self,
        connection_id: u64,
        raw: &Value,
        legacy: bool,
    ) -> Result<CallerBound, RpcError> {
        // A phase-1 key is never part of a well-formed body.
        let parsed = parse_authenticate(raw).filter(|_| !legacy);
        let now = self.host.now_ms();
        // 1. Take: the slot is emptied whatever follows.
        let taken = self.take_slot(connection_id, parsed.is_none(), legacy)?;
        let binding = Binding::current(&*self.host).ok();
        let taken_caller = taken.as_ref().map(|slot| slot.caller_id.as_str());
        if legacy {
            let class = Class::LegacyCredentialRefused;
            return Err(self.refused(binding.as_ref(), taken_caller, AUTHENTICATE, class));
        }
        let Some((challenge_id, authenticator)) = parsed else {
            let class = Class::InvalidInput;
            return Err(self.refused(binding.as_ref(), taken_caller, AUTHENTICATE, class));
        };
        let real = taken.is_some();
        let matched = taken.filter(|slot| slot.challenge_id == challenge_id);
        let attempt = Attempt {
            connection_id,
            real,
            caller_id: matched.as_ref().map(|slot| slot.caller_id.clone()),
            binding,
        };
        // 2. Compute, 3. compare and 4. decide.
        let verdict = self.verify(
            attempt.binding.as_ref(),
            matched.as_ref(),
            &authenticator,
            now,
        );
        if let Some(class) = verdict.failure {
            return Err(attempt.fail(self, class, Class::AuthFailed));
        }
        let (Some(record), Some(slot), Some(binding)) =
            (verdict.record, matched, attempt.binding.as_ref())
        else {
            return Err(attempt.fail(self, Class::CallerUnknown, Class::AuthFailed));
        };
        // 5. Live, only once the MAC has verified.
        if let Err(class) = record.live(now) {
            return Err(attempt.fail(self, class, class));
        }
        // 6. Bind.
        let Some(binding_id) =
            crate::service_callers::random_hex(16).map(|hex| format!("bd_{hex}"))
        else {
            return Err(attempt.fail(self, Class::StoreFailed, Class::StoreFailed));
        };
        let _files = self.files();
        if let Err(class) = self.recheck(binding, &record, &slot) {
            return Err(attempt.fail(self, class, Class::AuthFailed));
        }
        let bound = Bound {
            binding_id,
            caller_id: record.caller_id.clone(),
            authority_generation: record.authority_generation,
            identity_generation: 0,
            binding_key: slot.key,
            retires_at_ms: verdict.retires_at_ms,
        };
        if let Err(class) = self.bind(connection_id, bound) {
            let caller_id = Some(record.caller_id.as_str());
            return Err(self.refused(Some(binding), caller_id, AUTHENTICATE, class));
        }
        self.audit(
            binding,
            Some(&record.caller_id),
            AUTHENTICATE,
            "bound",
            None,
        );
        Ok(CallerBound {
            caller_id: record.caller_id,
            caller_name: record.name,
            authority_generation: record.authority_generation,
        })
    }

    /// Step 1 of `authenticate`: takes `connection_id`'s challenge, if it
    /// holds one. A body that is `invalid` counts as an attempt when a
    /// challenge was taken; a connection that cannot authenticate is
    /// refused here, as `legacy_credential_refused` for a `legacy` body.
    fn take_slot(
        &self,
        connection_id: u64,
        invalid: bool,
        legacy: bool,
    ) -> Result<Option<Slot>, RpcError> {
        let mut live = self.live();
        match take(&mut live, connection_id) {
            Ok(taken) => {
                if invalid && taken.is_some() {
                    count_attempt(&mut live, connection_id);
                }
                Ok(taken)
            }
            Err(class) => {
                drop(live);
                let class = if legacy {
                    Class::LegacyCredentialRefused
                } else {
                    class
                };
                let binding = Binding::current(&*self.host).ok();
                Err(self.refused(binding.as_ref(), None, AUTHENTICATE, class))
            }
        }
    }

    /// Steps 2 to 4 of `authenticate`, outside the live lock: the record
    /// `matched` names, the MAC over its transcript (a dummy one without a
    /// match) against both digest slots, and the decision, taken only after
    /// all MAC work.
    fn verify(
        &self,
        binding: Option<&Binding>,
        matched: Option<&Slot>,
        authenticator: &[u8; 32],
        now: i64,
    ) -> Verdict {
        let record = binding.and_then(|binding| self.record_for(binding, matched, now));
        let dummy;
        let slot = match matched {
            Some(slot) => slot,
            None => {
                dummy = Slot::dummy(binding, now);
                &dummy
            }
        };
        let digests = record.as_ref().map_or(&[][..], |r| r.secrets.as_slice());
        let proof = prove(&slot.transcript(), digests, authenticator, now);
        let failure = decide(
            matched.is_some(),
            slot.expired(now),
            record.is_some(),
            &proof,
        );
        Verdict {
            record,
            failure,
            retires_at_ms: proof.retires_at_ms,
        }
    }

    /// The record of the caller `matched` names, unless it is unknown.
    /// The registry is read (and swept) whether or not there is a match.
    fn record_for(
        &self,
        binding: &Binding,
        matched: Option<&Slot>,
        now: i64,
    ) -> Option<CallerRecord> {
        let _files = self.files();
        let file = self.load_current(binding).ok()?;
        let caller_id = matched?.caller_id.as_str();
        let record = file.callers.get(file.by_id(caller_id)?)?.clone();
        (record.live(now) != Err(Class::CallerUnknown)).then_some(record)
    }

    /// The bind's recheck, with the files lock held: the caller's
    /// generation has not moved and the scope is still the one the
    /// challenge was issued in.
    fn recheck(&self, binding: &Binding, record: &CallerRecord, slot: &Slot) -> Result<(), Class> {
        let current = self.load_current(binding).ok().and_then(|file| {
            file.by_id(&record.caller_id)
                .map(|i| file.callers[i].clone())
        });
        let key_now = Binding::current(&*self.host).ok().map(|b| b.key);
        if current.map(|c| c.authority_generation) != Some(record.authority_generation) {
            return Err(Class::GenerationMoved);
        }
        if key_now.as_deref() != Some(slot.key.as_str()) || binding.key != slot.key {
            return Err(Class::WorkspaceMismatch);
        }
        Ok(())
    }

    /// Binds `connection_id` to `bound` under the live lock, stamped with
    /// the core's identity generation there, unless the connection closed,
    /// locked or bound meanwhile.
    fn bind(&self, connection_id: u64, mut bound: Bound) -> Result<(), Class> {
        let mut live = self.live();
        let closed = live.closed.contains(connection_id);
        let Some(entry) = live.connections.get_mut(&connection_id).filter(|_| !closed) else {
            return Err(Class::ConnectionClosed);
        };
        match &entry.state {
            State::Closed => Err(Class::ConnectionClosed),
            State::Locked => Err(Class::AuthLocked),
            State::Bound(_) | State::Stale => Err(Class::AlreadyBound),
            State::Challenged { .. } | State::Untouched => {
                bound.identity_generation = self.host.generation();
                entry.state = State::Bound(bound);
                Ok(())
            }
        }
    }
}

/// Empties `connection_id`'s challenge slot, or why it cannot
/// authenticate.
fn take(live: &mut Live, connection_id: u64) -> Result<Option<Slot>, Class> {
    let closed = live.closed.contains(connection_id);
    let state = live
        .connections
        .get_mut(&connection_id)
        .filter(|_| !closed)
        .map(|e| &mut e.state);
    match state {
        Some(State::Bound(_) | State::Stale) => Err(Class::AlreadyBound),
        Some(State::Closed) | None => Err(Class::ConnectionClosed),
        Some(State::Locked) => Err(Class::AuthLocked),
        Some(State::Challenged { slot, .. }) => Ok(slot.take()),
        Some(State::Untouched) => Ok(None),
    }
}

/// One `authenticate` after the take: what a failure counts and audits.
struct Attempt {
    connection_id: u64,
    /// Whether a challenge was taken, so a failure counts as an attempt.
    real: bool,
    caller_id: Option<String>,
    binding: Option<Binding>,
}

impl Attempt {
    /// Counts and audits `class`, and answers the client `wire`.
    fn fail(&self, callers: &ServiceCallers, class: Class, wire: Class) -> RpcError {
        if self.real {
            count_attempt(&mut callers.live(), self.connection_id);
        }
        match &self.binding {
            Some(binding) => callers.audit(
                binding,
                self.caller_id.as_deref(),
                AUTHENTICATE,
                class.name(),
                None,
            ),
            None => callers
                .host
                .log(&format!("{AUTHENTICATE}: {}", class.name())),
        }
        wire.error()
    }
}

/// What steps 2 to 4 found.
struct Verdict {
    record: Option<CallerRecord>,
    failure: Option<Class>,
    /// When the digest that verified retires, if it does.
    retires_at_ms: Option<i64>,
}

/// The MAC comparison against both digest slots, folded without an early
/// exit.
struct Proof {
    /// 1 when any present digest's key verified.
    found: u8,
    /// 1 when a live digest's key verified.
    found_live: u8,
    retires_at_ms: Option<i64>,
}

/// Step 3: the MAC of `transcript` under each of the first two digests (the
/// dummy key where a slot is empty), compared with `authenticator` in
/// constant time.
fn prove(transcript: &[u8], digests: &[SecretDigest], authenticator: &[u8; 32], now: i64) -> Proof {
    let mut proof = Proof {
        found: 0,
        found_live: 0,
        retires_at_ms: None,
    };
    for index in 0..2 {
        let digest = digests.get(index);
        let key = digest.and_then(|d| key_of(&d.sha256));
        let present = u8::from(key.is_some());
        let mac = hmac(&key.unwrap_or(DUMMY_KEY), transcript);
        let hit = same_mac(&mac, authenticator) & present;
        let live_digest = u8::from(digest.is_some_and(|d| d.live(now)));
        proof.found |= hit;
        proof.found_live |= hit & live_digest;
        if hit & live_digest == 1 {
            proof.retires_at_ms = digest.and_then(|d| d.retires_at_ms);
        }
    }
    proof
}

/// Step 4: the class an `authenticate` fails with, after all MAC work.
fn decide(matched: bool, expired: bool, known: bool, proof: &Proof) -> Option<Class> {
    if !matched {
        Some(Class::ChallengeConsumed)
    } else if expired {
        Some(Class::ChallengeExpired)
    } else if !known {
        Some(Class::CallerUnknown)
    } else if proof.found == 0 {
        Some(Class::SecretMismatch)
    } else if proof.found_live == 0 {
        Some(Class::SecretRetired)
    } else {
        None
    }
}
