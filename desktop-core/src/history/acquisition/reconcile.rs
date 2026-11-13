//! Bring held sessions back once their authority is verified: a grant
//! claimed or rebound to this installation's current origin, and shown
//! active by the issuer's status. Runs before a
//! delivery pass. Sessions of a source that is not enrolled are released
//! without any issuer call; nothing here enrols or activates anything.
use super::contract::{
    self, ClaimRequest, ClaimResponse, InstallationRequest, RebindRequest, RebindResponse,
};
use super::issuer::{AcquisitionIssuer, IssuerError};
use super::store::{
    Account, Grant, Installation, Observed, Operation, Reauthorization, Secret, Store, Watch,
    CLAIM, REBIND,
};
use super::{hold_reason, new_proof, new_uuid, usable};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

mod installation;
mod operations;
mod status;
pub use installation::ensure_installation;
use installation::{authorized, not_authorized, sign_in_required, transport};
use operations::retrying;

/// Records of one session that may not be sent yet, and why: a local
/// [`hold_reason`] or the writer's refusal code. An empty `session_id`
/// stands for the source's records that carry no session.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeldSession {
    pub source: String,
    pub session_id: String,
    pub reason: String,
}

/// What one reconcile pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// The enrolled sources the pass went by: the ones the service
    /// published, or the last ones it published for this exact account when
    /// it does not say or cannot be asked. `None` when neither is known; then
    /// nothing is released or asked for.
    pub enrolled: Option<BTreeSet<String>>,
    /// (source, session) pairs whose records may be sent again.
    pub released: Vec<(String, String)>,
    /// Held sessions whose reason changes: an expired grant is held as
    /// [`hold_reason::RENEWAL_UNSUPPORTED`], and a session the issuer
    /// answered `403 not_owner` for as [`contract::NOT_OWNER`].
    pub relabeled: Vec<HeldSession>,
    /// The issuer refused the signed-in session (exactly `403
    /// not_authorized` from the bearer check; no other 403, and not the
    /// grant routes' own `not_authorized`, sets it): held
    /// sessions wait for sign-in with session ownership
    /// ([`hold_reason::SIGN_IN_REQUIRED`]) and no call is made for them
    /// until a different sign-in.
    pub sign_in_required: bool,
    /// This pass is the one that found it out; said once a sign-in.
    pub newly_required: bool,
}

/// A busy refusal (`session_busy`) is retried at once with the same
/// operation, up to this many attempts in a pass. Running out ends the
/// pass for the session; the next pass replays the same operation.
const ATTEMPTS: usize = 3;
/// After a terminal refusal the session waits this long for the next
/// operation, which gets a new uuid.
const REFUSED_PAUSE_MS: i64 = 60 * 60 * 1000;
/// How often a session waiting on its owner's status asks for it.
pub const WATCH_INTERVAL_MS: i64 = 5 * 60 * 1000;

/// Resolve what can be resolved for `held`, for the exact `account`.
///
/// Per session, in this order: a source that is not enrolled is released
/// without any issuer call; a pending operation is replayed exactly from
/// its stored record; an expired grant stays held as renewal unsupported
/// while its status is refreshed; a grant bound to an older origin is
/// rebound (a separate operation), unless it waits on its owner, when only
/// its status is polled on the hold cadence (see [`Pass::poll_stale`]); a
/// session waiting on its owner polls status on the hold cadence; a usable
/// grant is verified by status; and only a session without one is claimed.
/// A session is released only when the status shows its own grant active
/// with the stored grant id and epoch, bound to the current origin.
///
/// A call that leaves the installation unproven stops the pass with
/// [`IssuerError::NotProven`] (or `ProofMismatch`); held sessions stay held
/// and the proof and operation are kept.
///
/// A `not_authorized` answer, on any route (a rebind's included), means the
/// signed-in session lacks `rth:acquire`: the service uses that code for
/// the bearer/scope check only. It is recorded once for the sign-in it refused, the rest
/// of the pass makes no issuer call, and nothing is paced or refused: held
/// sessions stay held, waiting for sign-in with session ownership. Passes
/// under the same sign-in make no issuer call past the enrolled sources;
/// the first pass under a different one tries again, and a refusal of that
/// one is recorded the same way.
///
/// No other 403 asks for sign-in or stops the pass, since a new sign-in
/// cannot cure it. `403 not_owner` (another installation owns the session)
/// is an owner wait of that session alone: it is held as
/// [`contract::NOT_OWNER`] and its status is polled on the hold cadence.
/// `403 not_installer` ends the operation and is paced like a terminal
/// refusal. A claim presents no grant, so nothing else changes. A rebind's
/// grant is quarantined, not dropped: the code is also the service's SQL
/// installer-role guard, so it does not prove the grant is not this
/// installation's. The grant is not presented again until the scoped
/// status says whose it is (see [`Pass::quarantined`]): dropped, and the
/// session claimed afresh, only when the status disowns it; rebound again
/// after the pause while the status shows it still this installation's
/// active grant. Any other 403 code, or
/// none, fails closed for that session alone: it stays held and its next
/// operation is paced like a terminal refusal.
///
/// A `503` (`acquisition_unavailable` included, which a claim answers
/// while its install has no positive epoch yet) is a transport failure:
/// the session is held and the same stored operation, byte for byte, is
/// replayed on the next pass.
/// A transport failure outside an operation (registering the
/// installation, say) holds that session too, and the rest of the pass
/// makes no issuer call; sessions already released or relabeled, and
/// those of a source that is not enrolled, are still returned.
pub fn reconcile(
    issuer: &dyn AcquisitionIssuer,
    store: &Store,
    account: &Account,
    origin_id: &str,
    held: &[HeldSession],
    now_ms: i64,
) -> Result<Reconciled> {
    anyhow::ensure!(account.exact(), "acquisition account is not exact");
    let published = match issuer.enrolled_sources() {
        Ok(Some(enrolled)) => {
            if store.load()?.enrolled(account).as_ref() != Some(&enrolled) {
                store.update(|state| {
                    state.put_enrolled(account, &enrolled);
                    Ok(())
                })?;
            }
            Some(enrolled)
        }
        // Not said, or not asked: unknown, never "not enrolled".
        Ok(None) | Err(_) => None,
    };
    let enrolled = match published {
        Some(enrolled) => Some(enrolled),
        None => store.load()?.enrolled(account),
    };
    let mut done = Reconciled {
        enrolled: enrolled.clone(),
        ..Reconciled::default()
    };
    let Some(enrolled) = enrolled else {
        return Ok(done);
    };
    let sign_in = issuer.sign_in();
    done.sign_in_required = store
        .load()?
        .reauthorization(account)
        .is_some_and(|marker| marker.sign_in == sign_in);
    // A session held for several reasons is acted on once a pass.
    let mut sessions: BTreeMap<(String, String), BTreeSet<&str>> = BTreeMap::new();
    for session in held {
        sessions
            .entry((session.source.clone(), session.session_id.clone()))
            .or_default()
            .insert(session.reason.as_str());
    }
    let mut pass = Pass {
        issuer,
        store,
        account,
        origin_id,
        now_ms,
        installation: None,
        claimed: BTreeSet::new(),
    };
    let mut unreachable = false;
    for ((source, session_id), reasons) in sessions {
        if !enrolled.contains(&source) {
            done.released.push((source, session_id));
            continue;
        }
        if session_id.is_empty() || done.sign_in_required || unreachable {
            continue;
        }
        match pass.session(&source, &session_id, &reasons) {
            Ok(Outcome::Release) => done.released.push((source, session_id)),
            Ok(Outcome::Hold) => {}
            Ok(Outcome::Relabel(reason)) => done.relabeled.push(HeldSession {
                source,
                session_id,
                reason: reason.to_owned(),
            }),
            Err(error) if not_authorized(&error) => {
                sign_in_required(store, account, &sign_in, &mut done)?;
            }
            // Any other issuer failure that reached this far (a registration
            // that timed out or got a 5xx, say) holds this session alone;
            // the sessions already resolved this pass are still handed back.
            // After a transport failure no further issuer call is made this
            // pass. An unproven installation and a local error stop the pass.
            Err(error) => {
                unreachable |= transport(&error);
                pass.unanswered(error)?;
            }
        }
    }
    done.released.sort();
    done.released.dedup();
    Ok(done)
}

/// What became of one held session this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Release,
    Hold,
    Relabel(&'static str),
}

/// One pass's issuer context: the installation is registered (or read)
/// once, and a session is claimed at most once.
struct Pass<'a> {
    issuer: &'a dyn AcquisitionIssuer,
    store: &'a Store,
    account: &'a Account,
    origin_id: &'a str,
    now_ms: i64,
    installation: Option<(String, Secret)>,
    claimed: BTreeSet<(String, String)>,
}

impl Pass<'_> {
    fn session(
        &mut self,
        source: &str,
        session_id: &str,
        reasons: &BTreeSet<&str>,
    ) -> Result<Outcome> {
        let state = self.store.load()?;
        // A pending operation goes first, exactly as it was stored, whatever
        // happened locally since (a reset, a new hold reason).
        if let Some(operation) = state
            .operation(self.account, source, session_id)
            .filter(|operation| operation.pending())
            .cloned()
        {
            return self.send(operation);
        }
        let grant = state
            .grant(self.account, source, session_id)
            .filter(|grant| usable(grant))
            .cloned();
        if reasons.contains(contract::GRANT_EXPIRED)
            || reasons.contains(hold_reason::RENEWAL_UNSUPPORTED)
        {
            self.renewal_unsupported(source, session_id)?;
            return Ok(if reasons.contains(hold_reason::RENEWAL_UNSUPPORTED) {
                Outcome::Hold
            } else {
                Outcome::Relabel(hold_reason::RENEWAL_UNSUPPORTED)
            });
        }
        let paused = state
            .operation(self.account, source, session_id)
            .is_some_and(|operation| operation.not_before_ms > self.now_ms);
        if let Some(grant) = grant
            .as_ref()
            .filter(|grant| grant.origin_id != self.origin_id)
        {
            if paused {
                return Ok(Outcome::Hold);
            }
            // An owner wait (a rebind refused `not_owner` or with a 409
            // owner-wait code) holds on its watch: no new operation while
            // the owner is unchanged. Only the first attempt rebinds at once.
            // A grant quarantined by `not_installer` is presented again
            // only once its status shows it still this installation's.
            return match state.watch(self.account, source, session_id).cloned() {
                Some(watch) if watch.not_before_ms > self.now_ms => Ok(Outcome::Hold),
                Some(watch) if watch.reason == contract::NOT_INSTALLER => {
                    self.quarantined(grant, watch.observed, true)
                }
                Some(watch) => self.poll_stale(watch, grant),
                None => self.begin_rebind(grant),
            };
        }
        if let Some(watch) = state.watch(self.account, source, session_id).cloned() {
            if watch.not_before_ms > self.now_ms {
                return Ok(Outcome::Hold);
            }
            return self.poll(watch, grant.as_ref(), paused);
        }
        match grant {
            Some(grant) => self.verify(&grant),
            None if paused => Ok(Outcome::Hold),
            None => self.begin_claim(source, session_id),
        }
    }
}
