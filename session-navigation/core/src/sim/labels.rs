//! Stable identities for event targets, so that shrinking a failing run
//! (dropping events) does not retarget the events that remain.
//!
//! A pick with [`STABLE`] set is resolved by rendezvous hashing over labels
//! that are derived from the content of the event that created each target,
//! never from its position in a list. Removing a candidate that was not the
//! winner leaves the winner unchanged. A pick without the bit is an index,
//! resolved modulo the candidates (hand-written tests use that form).
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::SessionRef;

pub const STABLE: u32 = 1 << 31;

/// The label of the "a session that never existed" candidate for opens.
pub const NONEXISTENT: u64 = 0x0DEA_D5E5_510A_0000;

pub fn mix(a: u64, b: u64) -> u64 {
    let mut z = a ^ b.rotate_left(29) ^ 0x9E37_79B9_7F4A_7C15;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// FNV-1a over bytes.
pub fn digest(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01B3)
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Labels {
    hint: u64,
    /// Per kind (session, pane, process), so that how many processes an
    /// event happens to spawn (an app already running or not) does not shift
    /// the labels of the sessions it creates.
    counters: [u64; 3],
    pub sessions: BTreeMap<SessionRef, u64>,
    pub panes: BTreeMap<u64, u64>,
    pub procs: BTreeMap<u32, u64>,
}

impl Labels {
    /// Start labelling what one event creates.
    pub fn begin(&mut self, hint: u64) {
        self.hint = hint;
        self.counters = [0; 3];
    }

    fn fresh(&mut self, kind: usize) -> u64 {
        let n = self.counters.get(kind).copied().unwrap_or(0) + 1;
        if let Some(c) = self.counters.get_mut(kind) {
            *c = n;
        }
        mix(mix(self.hint, kind as u64), n)
    }

    pub fn new_session(&mut self, session: &SessionRef) {
        if !self.sessions.contains_key(session) {
            let label = self.fresh(0);
            self.sessions.insert(session.clone(), label);
        }
    }

    pub fn new_pane(&mut self, id: u64) {
        let label = self.fresh(1);
        self.panes.insert(id, label);
    }

    pub fn new_proc(&mut self, pid: u32) {
        let label = self.fresh(2);
        self.procs.insert(pid, label);
    }

    pub fn session(&self, session: &SessionRef) -> u64 {
        self.sessions
            .get(session)
            .copied()
            .unwrap_or_else(|| digest(session.id.as_bytes()))
    }

    pub fn pane(&self, id: u64) -> u64 {
        self.panes.get(&id).copied().unwrap_or(id)
    }

    pub fn proc(&self, pid: u32) -> u64 {
        self.procs.get(&pid).copied().unwrap_or(u64::from(pid))
    }
}

/// Resolve `pick` over `(item, label)` candidates.
pub fn resolve<T: Clone>(candidates: &[(T, u64)], pick: u32) -> Option<T> {
    if candidates.is_empty() {
        return None;
    }
    if pick & STABLE == 0 {
        let i = usize::try_from(pick).unwrap_or(0) % candidates.len();
        return candidates.get(i).map(|(item, _)| item.clone());
    }
    let mut best: Option<(&T, u64)> = None;
    for (item, label) in candidates {
        let score = mix(u64::from(pick), *label);
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((item, score));
        }
    }
    best.map(|(item, _)| item.clone())
}
