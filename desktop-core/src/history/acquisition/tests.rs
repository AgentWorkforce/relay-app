use super::contract::{
    self, AcquisitionEntry, ClaimRequest, ClaimResponse, InstallationRequest, InstallationResponse,
    RebindRequest, RebindResponse, SessionStatus, StatusRequest, StatusResponse,
};
use super::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

const ORG: &str = "org_test";
const WS: &str = "ws_test";
const SITE: &str = "https://agentrelay.test";
const USER: &str = "user_test";

fn account() -> Account {
    Account {
        site: SITE.into(),
        user: USER.into(),
        org: ORG.into(),
        workspace: WS.into(),
    }
}
fn g(n: u32) -> String {
    format!("rhg_{}", format!("{n:08x}").repeat(8))
}
fn uuid(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}
fn grant(source: &str, session: &str, n: u32, origin: &str) -> Grant {
    Grant {
        account: account(),
        source: source.into(),
        session_id: session.into(),
        grant_id: uuid(n),
        owner_epoch: n as u64,
        grant: Secret::new(g(n)),
        expires_at: None,
        origin_id: origin.into(),
    }
}
fn held(source: &str, session: &str, reason: &str) -> HeldSession {
    HeldSession {
        source: source.into(),
        session_id: session.into(),
        reason: reason.into(),
    }
}
fn enrolled(sources: &[&str]) -> BTreeSet<String> {
    sources.iter().map(|source| (*source).to_owned()).collect()
}
fn entry(source: &str, session: &str, n: u32) -> AcquisitionEntry {
    AcquisitionEntry {
        source: source.into(),
        session_id: session.into(),
        grant_id: uuid(n),
        owner_epoch: n.to_string(),
        grant: g(n),
    }
}

#[test]
fn the_grant_hash_matches_both_contract_vectors() {
    let grant = format!("rhg_{}", "0123456789abcdef".repeat(4));
    let id = "00000000-0000-4000-8000-000000000001";
    assert_eq!(
        grant_sha256(ORG, WS, "droid", "sess-0001", id, &grant),
        "eca3f7669845e8b736db441074fcbaa51183bfb167f7cc71003d4d564496d227"
    );
    assert_eq!(
        grant_sha256(ORG, WS, "droid", "sess-0002", id, &grant),
        "359de6c40574725c3256fb529e69c51c4cdefbe06c1cd3558c1068b8bd9019b4"
    );
}

#[test]
fn uuids_proofs_grants_and_epochs_have_their_wire_forms() {
    let first = new_uuid().unwrap();
    assert!(valid_uuid(&first), "{first}");
    assert_eq!(&first[14..15], "4");
    assert_ne!(first, new_uuid().unwrap());
    assert!(!valid_uuid("00000000-0000-4000-8000-00000000000A"));
    assert!(!valid_uuid("00000000-0000-4000-8000-00000000000"));

    let proof = new_proof().unwrap();
    assert_eq!(proof.expose().len(), contract::PROOF_LEN);
    assert!(valid_proof(proof.expose()));
    assert!(!proof.expose().contains('='));
    assert_ne!(proof, new_proof().unwrap());
    assert!(!valid_proof(&format!("{}=", proof.expose())));
    assert!(!valid_proof(&proof.expose()[1..]));
    assert!(!valid_proof(&"+".repeat(43)));

    assert!(valid_grant(&g(1)));
    assert!(!valid_grant(&g(1).to_uppercase()));
    assert!(!valid_grant(&g(1)[..67]));
    assert!(!valid_grant(&g(1).replace("rhg_", "rhx_")));

    assert!(decimal_epoch("1") && decimal_epoch("18446744073709551615"));
    for bad in ["", "0", "01", "-1", "+1", "1.0", "18446744073709551616"] {
        assert!(!decimal_epoch(bad), "{bad}");
    }
}

#[test]
fn a_batch_presents_one_sorted_entry_per_enrolled_session() {
    let grants = [
        grant("droid", "b", 2, "origin"),
        grant("droid", "a", 1, "origin"),
        grant("claude", "c", 3, "origin"),
    ];
    let records = [
        ("droid", Some("b")),
        ("droid", Some("a")),
        ("droid", Some("b")),
        ("claude", Some("c")),
        ("codex", None),
        ("codex", Some("x")),
    ];
    let presented = present(
        records,
        "origin",
        &enrolled(&["droid", "claude"]),
        |source, session| {
            grants
                .iter()
                .find(|grant| grant.source == source && grant.session_id == session)
        },
    );
    let Presentation::Send(entries) = presented else {
        panic!("held: {presented:?}");
    };
    assert_eq!(
        entries,
        vec![
            entry("claude", "c", 3),
            entry("droid", "a", 1),
            entry("droid", "b", 2)
        ]
    );
    assert!(validate_entries(&entries, records));

    // Nothing enrolled: no entries, nothing held, the grants are not read.
    assert_eq!(
        present(records, "origin", &enrolled(&[]), |_, _| panic!("read")),
        Presentation::Send(vec![])
    );
}

#[test]
fn a_protected_session_without_a_usable_grant_for_this_origin_is_held() {
    let mut broken = grant("droid", "broken", 4, "origin");
    broken.grant = Secret::new("rhg_short".into());
    let grants = [
        grant("droid", "ok", 1, "origin"),
        grant("droid", "moved", 2, "old-origin"),
        broken,
    ];
    let records = [
        ("droid", Some("ok")),
        ("droid", Some("moved")),
        ("droid", Some("broken")),
        ("droid", Some("none")),
        ("droid", None),
        ("codex", None),
    ];
    let presented = present(
        records,
        "origin",
        &enrolled(&["droid"]),
        |source, session| {
            grants
                .iter()
                .find(|grant| grant.source == source && grant.session_id == session)
        },
    );
    assert_eq!(
        presented,
        Presentation::Hold(vec![
            held("droid", "", contract::SESSION_REQUIRED),
            held("droid", "broken", hold_reason::NO_GRANT),
            held("droid", "moved", hold_reason::ORIGIN_CHANGED),
            held("droid", "none", hold_reason::NO_GRANT),
        ])
    );
}

#[test]
fn entries_are_validated_against_their_batch() {
    let sessions = [("droid", Some("a")), ("droid", Some("b")), ("codex", None)];
    let good = vec![entry("droid", "a", 1), entry("droid", "b", 2)];
    assert!(validate_entries(&good, sessions));
    assert!(validate_entries(&[], sessions));
    let unsorted = vec![entry("droid", "b", 2), entry("droid", "a", 1)];
    assert!(!validate_entries(&unsorted, sessions));
    let duplicate = vec![entry("droid", "a", 1), entry("droid", "a", 1)];
    assert!(!validate_entries(&duplicate, sessions));
    let unknown = vec![entry("droid", "c", 1)];
    assert!(!validate_entries(&unknown, sessions));
    let mut bad_grant = entry("droid", "a", 1);
    bad_grant.grant = g(1).to_uppercase();
    assert!(!validate_entries(&[bad_grant], sessions));
    let mut bad_id = entry("droid", "a", 1);
    bad_id.grant_id = "00000000-0000-4000-8000-00000000000A".into();
    assert!(!validate_entries(&[bad_id], sessions));
    let mut leading_zero = entry("droid", "a", 1);
    leading_zero.owner_epoch = "01".into();
    assert!(!validate_entries(&[leading_zero], sessions));
    // Byte order, not locale order: "B" (0x42) sorts before "a" (0x61).
    let bytewise = [("droid", Some("B")), ("droid", Some("a"))];
    assert!(validate_entries(
        &[entry("droid", "B", 1), entry("droid", "a", 2)],
        bytewise
    ));
    assert!(!validate_entries(
        &[entry("droid", "a", 2), entry("droid", "B", 1)],
        bytewise
    ));
}

#[test]
fn an_entry_serializes_to_the_wire_shape_and_never_prints_its_grant() {
    let value = serde_json::to_value(entry("droid", "a", 7)).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "source": "droid", "sessionId": "a", "grantId": uuid(7),
            "ownerEpoch": "7", "grant": g(7)
        })
    );
    assert!(!format!("{:?}", entry("droid", "a", 7)).contains(&g(7)));
    let mut state = State::default();
    state.put_grant(grant("droid", "a", 7, "origin"));
    let proof = new_proof().unwrap();
    state.installations.push(Installation {
        account: account(),
        request_uuid: uuid(1),
        proof: proof.clone(),
        installation_id: Some("inst".into()),
        origin_id: "origin".into(),
        issuer_key_id: None,
    });
    let printed = format!("{state:?}");
    assert!(!printed.contains(&g(7)) && !printed.contains(proof.expose()));
}

#[test]
fn writer_refusals_and_enrolled_sources_are_parsed_strictly() {
    let body = serde_json::json!({
        "error": {"code": "acquisition_refused"},
        "refused": [
            {"source": "droid", "sessionId": "a", "code": "not_owner"},
            {"source": "droid", "sessionId": "", "code": "session_required"},
            {"source": "droid", "sessionId": "b", "code": "grant_expired"}
        ]
    });
    let refused = contract::writer_refusal(&body).unwrap();
    assert_eq!(refused.len(), 3);
    assert_eq!(refused[1].session_id, "");
    for bad in [
        serde_json::json!({"error": {"code": "delivery_conflict"}, "refused": [{"source": "droid", "sessionId": "a", "code": "not_owner"}]}),
        serde_json::json!({"error": {"code": "acquisition_refused"}, "refused": []}),
        serde_json::json!({"error": {"code": "acquisition_refused"}}),
        serde_json::json!({"error": {"code": "acquisition_refused"}, "refused": [{"source": "droid", "sessionId": "a", "code": "made_up"}]}),
        serde_json::json!({"error": {"code": "acquisition_refused"}, "refused": [{"source": "droid", "sessionId": "", "code": "not_owner"}]}),
        serde_json::json!({"error": {"code": "acquisition_refused"}, "refused": [{"source": "droid", "sessionId": "a", "code": "session_required"}]}),
        serde_json::json!({"error": {"code": "acquisition_refused"}, "refused": [{"source": "", "sessionId": "a", "code": "not_owner"}]}),
    ] {
        assert_eq!(contract::writer_refusal(&bad), None, "{bad}");
    }
    for code in contract::WRITER_REFUSAL_CODES {
        let session = if code == contract::SESSION_REQUIRED {
            ""
        } else {
            "s"
        };
        let body = serde_json::json!({"error": {"code": "acquisition_refused"},
            "refused": [{"source": "droid", "sessionId": session, "code": code}]});
        assert!(contract::writer_refusal(&body).is_some(), "{code}");
    }

    assert_eq!(
        contract::enrolled_sources(
            &serde_json::json!({"acquisition": {"enrolledSources": ["droid", "claude"]}})
        ),
        Some(enrolled(&["claude", "droid"]))
    );
    assert_eq!(
        contract::enrolled_sources(&serde_json::json!({"acquisition": {"enrolledSources": []}})),
        Some(BTreeSet::new())
    );
    assert_eq!(contract::enrolled_sources(&serde_json::json!({})), None);
    assert_eq!(
        contract::enrolled_sources(&serde_json::json!({"acquisition": {"enrolledSources": [1]}})),
        None
    );
}

#[test]
fn permanent_revoke_is_unavailable() {
    assert_eq!(
        revoke_permanently(&account(), "droid", "a"),
        Err(IssuerError::Unavailable)
    );
}

/// A History service that keeps installations, owners and replays as the
/// contract says: a registration replays by request uuid and proof, a claim
/// or rebind replays by operation uuid (the same request again answers
/// `replay`, another request under the uuid is `operation_conflict`), and
/// every other call must carry the proof of the installation it names.
#[derive(Default)]
struct Fake {
    enrolled: Option<BTreeSet<String>>,
    /// The enrolled sources cannot be read.
    enrolled_fails: Cell<bool>,
    /// The response does not say which sources are enrolled.
    enrolled_unsaid: Cell<bool>,
    /// The state file, which must already hold the proof when it registers.
    store: Option<PathBuf>,
    server: RefCell<Server>,
    claims: RefCell<VecDeque<Result<ClaimResponse, IssuerError>>>,
    rebinds: RefCell<VecDeque<Result<RebindResponse, IssuerError>>>,
    statuses: RefCell<VecDeque<Result<StatusResponse, IssuerError>>>,
    lose_next_registration: Cell<bool>,
    /// The next claim is applied but its answer is lost.
    lose_next_claim: Cell<bool>,
    calls: RefCell<Vec<Call>>,
    /// The fingerprint of the signed-in session the calls run under.
    sign_in: RefCell<String>,
}
#[derive(Default)]
struct Server {
    registrations: BTreeMap<String, (String, String)>,
    installations: BTreeMap<String, String>,
    /// (source, session) -> the owner's installation, grant and state.
    owners: BTreeMap<(String, String), Owner>,
    claimed: BTreeMap<String, (ClaimRequest, ClaimResponse)>,
    rebound: BTreeMap<String, (RebindRequest, RebindResponse)>,
    next: u32,
}
#[derive(Debug, Clone)]
struct Owner {
    installation_id: String,
    grant_id: String,
    owner_epoch: u64,
    state: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Register { request_uuid: String },
    Claim(ClaimRequest),
    Rebind(RebindRequest),
    Status(StatusRequest),
}
impl Fake {
    fn enrolling(sources: &[&str]) -> Self {
        Self {
            enrolled: Some(enrolled(sources)),
            ..Default::default()
        }
    }
    fn proven(&self, proof: &Secret, installation_id: &str) -> Result<(), IssuerError> {
        match self.server.borrow().installations.get(installation_id) {
            Some(kept) if kept == proof.expose() => Ok(()),
            _ => Err(IssuerError::NotProven),
        }
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }
    fn registrations(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| matches!(call, Call::Register { .. }))
            .count()
    }
    /// (session, operation uuid) of every claim sent.
    fn claims(&self) -> Vec<(String, String)> {
        self.claim_requests()
            .into_iter()
            .map(|request| (request.session_id, request.operation_uuid))
            .collect()
    }
    fn claim_requests(&self) -> Vec<ClaimRequest> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Claim(request) => Some(request),
                _ => None,
            })
            .collect()
    }
    fn rebinds(&self) -> Vec<RebindRequest> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Rebind(request) => Some(request),
                _ => None,
            })
            .collect()
    }
    fn statuses(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| matches!(call, Call::Status(_)))
            .count()
    }
    /// The server's owner of `session` moves to `state`.
    fn owner_state(&self, source: &str, session: &str, state: &str) {
        self.server
            .borrow_mut()
            .owners
            .get_mut(&(source.to_owned(), session.to_owned()))
            .expect("owned")
            .state = state.to_owned();
    }
    fn own(&self, source: &str, session: &str, installation_id: &str, n: u32, state: &str) {
        self.server.borrow_mut().owners.insert(
            (source.to_owned(), session.to_owned()),
            Owner {
                installation_id: installation_id.to_owned(),
                grant_id: uuid(n),
                owner_epoch: n as u64,
                state: state.to_owned(),
            },
        );
    }
}
impl AcquisitionIssuer for Fake {
    fn enrolled_sources(&self) -> Result<Option<BTreeSet<String>>, IssuerError> {
        if self.enrolled_fails.get() {
            return Err(IssuerError::Transport("connection reset".into()));
        }
        if self.enrolled_unsaid.get() {
            return Ok(None);
        }
        Ok(self.enrolled.clone())
    }
    fn register(
        &self,
        proof: &Secret,
        request: &InstallationRequest,
    ) -> Result<InstallationResponse, IssuerError> {
        self.calls.borrow_mut().push(Call::Register {
            request_uuid: request.request_uuid.clone(),
        });
        assert!(valid_proof(proof.expose()));
        if let Some(path) = &self.store {
            let saved = std::fs::read_to_string(path).expect("state saved before registering");
            assert!(
                saved.contains(proof.expose()),
                "proof saved before registering"
            );
            assert!(saved.contains(&request.request_uuid));
        }
        let mut server = self.server.borrow_mut();
        let id = match server.registrations.get(&request.request_uuid) {
            Some((kept, _)) if kept != proof.expose() => return Err(IssuerError::ProofMismatch),
            Some((_, id)) => id.clone(),
            None => {
                server.next += 1;
                let id = uuid(1000 + server.next);
                server.registrations.insert(
                    request.request_uuid.clone(),
                    (proof.expose().to_owned(), id.clone()),
                );
                server
                    .installations
                    .insert(id.clone(), proof.expose().to_owned());
                id
            }
        };
        if self.lose_next_registration.replace(false) {
            return Err(IssuerError::Transport("connection reset".into()));
        }
        Ok(InstallationResponse {
            installation_id: id,
            origin_id: request.origin_id.clone(),
            issuer_key_id: Some("key-1".into()),
        })
    }
    fn claim(&self, proof: &Secret, request: &ClaimRequest) -> Result<ClaimResponse, IssuerError> {
        self.calls.borrow_mut().push(Call::Claim(request.clone()));
        self.proven(proof, &request.installation_id)?;
        if let Some(reply) = self.claims.borrow_mut().pop_front() {
            return reply;
        }
        let mut server = self.server.borrow_mut();
        if let Some((kept, answer)) = server.claimed.get(&request.operation_uuid) {
            if kept != request {
                return Err(IssuerError::Refused {
                    reason: "operation_conflict".into(),
                    retryable: false,
                });
            }
            let mut replay = answer.clone();
            replay.outcome = contract::OUTCOME_REPLAY.into();
            return Ok(replay);
        }
        server.next += 1;
        let n = server.next;
        server.owners.insert(
            (request.source.clone(), request.session_id.clone()),
            Owner {
                installation_id: request.installation_id.clone(),
                grant_id: uuid(n),
                owner_epoch: n as u64,
                state: contract::OWNER_ACTIVE.into(),
            },
        );
        let answer = ClaimResponse {
            outcome: contract::OUTCOME_INSTALLED.into(),
            grant_id: uuid(n),
            owner_epoch: n as u64,
            revision: None,
            expires_at: None,
            grant: g(n),
        };
        server.claimed.insert(
            request.operation_uuid.clone(),
            (request.clone(), answer.clone()),
        );
        if self.lose_next_claim.replace(false) {
            return Err(IssuerError::Transport("connection reset".into()));
        }
        Ok(answer)
    }
    fn rebind(
        &self,
        proof: &Secret,
        request: &RebindRequest,
    ) -> Result<RebindResponse, IssuerError> {
        self.calls.borrow_mut().push(Call::Rebind(request.clone()));
        self.proven(proof, &request.installation_id)?;
        if let Some(reply) = self.rebinds.borrow_mut().pop_front() {
            return reply;
        }
        let mut server = self.server.borrow_mut();
        if let Some((kept, answer)) = server.rebound.get(&request.operation_uuid) {
            if kept != request {
                return Err(IssuerError::Refused {
                    reason: "operation_conflict".into(),
                    retryable: false,
                });
            }
            let mut replay = answer.clone();
            replay.outcome = contract::OUTCOME_REPLAY.into();
            return Ok(replay);
        }
        let key = (request.source.clone(), request.session_id.clone());
        let epoch = match server.owners.get(&key) {
            Some(owner) if owner.grant_id == request.grant_id => owner.owner_epoch,
            _ => {
                return Err(IssuerError::Refused {
                    reason: "not_owner".into(),
                    retryable: false,
                })
            }
        };
        let answer = RebindResponse {
            outcome: contract::OUTCOME_REBOUND.into(),
            revision: None,
            owner_epoch: epoch,
        };
        server.rebound.insert(
            request.operation_uuid.clone(),
            (request.clone(), answer.clone()),
        );
        Ok(answer)
    }
    fn status(
        &self,
        proof: &Secret,
        request: &StatusRequest,
    ) -> Result<StatusResponse, IssuerError> {
        self.calls.borrow_mut().push(Call::Status(request.clone()));
        self.proven(proof, &request.installation_id)?;
        if let Some(reply) = self.statuses.borrow_mut().pop_front() {
            return reply;
        }
        let server = self.server.borrow();
        Ok(StatusResponse {
            sessions: request
                .session_ids
                .iter()
                .map(|session| {
                    match server
                        .owners
                        .get(&(request.source.clone(), session.clone()))
                    {
                        Some(owner) => {
                            // The grant id is named only to the installation
                            // that installed it.
                            let mine = owner.installation_id == request.installation_id;
                            SessionStatus {
                                session_id: session.clone(),
                                state: owner.state.clone(),
                                revision: None,
                                tombstone_revision: None,
                                owner_epoch: Some(owner.owner_epoch),
                                owner_kind: None,
                                grant_id: mine.then(|| owner.grant_id.clone()),
                                mine: Some(mine),
                            }
                        }
                        None => SessionStatus {
                            session_id: session.clone(),
                            state: contract::OWNER_UNKNOWN.into(),
                            revision: None,
                            tombstone_revision: None,
                            owner_epoch: None,
                            owner_kind: None,
                            grant_id: None,
                            mine: Some(false),
                        },
                    }
                })
                .collect(),
        })
    }
    fn sign_in(&self) -> String {
        self.sign_in.borrow().clone()
    }
}

fn store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().join("acquisition"));
    (directory, store)
}
fn on(store: &Store, sources: &[&str]) -> Fake {
    Fake {
        store: Some(store.path().to_owned()),
        ..Fake::enrolling(sources)
    }
}
fn busy() -> IssuerError {
    IssuerError::Refused {
        reason: "session_busy".into(),
        retryable: true,
    }
}
fn released(sessions: &[&str]) -> Vec<(String, String)> {
    sessions
        .iter()
        .map(|session| ("droid".to_owned(), (*session).to_owned()))
        .collect()
}
const LATER: i64 = reconcile::WATCH_INTERVAL_MS + 1;
const PAUSE: i64 = 60 * 60 * 1000 + 1;

#[test]
fn the_proof_is_saved_before_the_first_registration_and_outlives_a_new_origin() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let (id, proof) = ensure_installation(&issuer, &store, &account(), "origin-1").unwrap();
    assert!(valid_proof(proof.expose()));
    assert_eq!(issuer.registrations(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "state file is private: {mode:o}");
    }
    // A local reset deletes history.db and rotates the origin; the
    // installation is kept and is not registered again.
    let (again, kept) = ensure_installation(&issuer, &store, &account(), "origin-2").unwrap();
    assert_eq!((again, kept), (id.clone(), proof.clone()));
    assert_eq!(issuer.registrations(), 1);
    let state = Store::at(store.path().parent().unwrap().to_owned())
        .load()
        .unwrap();
    let installation = state.installation(&account()).unwrap();
    assert_eq!(installation.installation_id.as_deref(), Some(id.as_str()));
    assert_eq!(installation.proof, proof);
    assert_eq!(installation.issuer_key_id.as_deref(), Some("key-1"));
    // Another account is another installation with its own proof.
    let other = Account {
        workspace: "ws_other".into(),
        ..account()
    };
    let (other_id, other_proof) = ensure_installation(&issuer, &store, &other, "origin-2").unwrap();
    assert_ne!(other_id, id);
    assert_ne!(other_proof, proof);
}

#[test]
fn a_lost_registration_replays_the_same_request_and_proof_to_the_same_installation() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    issuer.lose_next_registration.set(true);
    assert!(ensure_installation(&issuer, &store, &account(), "origin").is_err());
    let saved = store
        .load()
        .unwrap()
        .installation(&account())
        .cloned()
        .unwrap();
    assert_eq!(saved.installation_id, None);
    let (id, proof) = ensure_installation(&issuer, &store, &account(), "origin").unwrap();
    assert_eq!(proof, saved.proof);
    let Call::Register {
        request_uuid: first,
    } = &issuer.calls()[0]
    else {
        panic!()
    };
    let Call::Register {
        request_uuid: second,
    } = &issuer.calls()[1]
    else {
        panic!()
    };
    assert_eq!(first, second);
    assert_eq!(first, &saved.request_uuid);
    assert_eq!(issuer.server.borrow().installations.len(), 1);
    assert!(issuer.server.borrow().installations.contains_key(&id));
}

#[test]
fn another_proof_on_the_same_request_is_a_mismatch_and_no_proof_is_minted() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    issuer.lose_next_registration.set(true);
    assert!(ensure_installation(&issuer, &store, &account(), "origin").is_err());
    // The saved proof changes under the same request uuid.
    let other = new_proof().unwrap();
    store
        .update(|state| {
            state.installations[0].proof = other.clone();
            Ok(())
        })
        .unwrap();
    let error = ensure_installation(&issuer, &store, &account(), "origin").unwrap_err();
    assert_eq!(
        error.downcast_ref::<IssuerError>(),
        Some(&IssuerError::ProofMismatch)
    );
    assert!(!error.to_string().contains(other.expose()));
    // Not retried, not replaced: the next pass meets the same typed error.
    assert_eq!(issuer.registrations(), 2);
    let kept = store
        .load()
        .unwrap()
        .installation(&account())
        .cloned()
        .unwrap();
    assert_eq!(kept.proof, other);
    assert_eq!(kept.installation_id, None);
}

#[test]
fn reconcile_claims_a_grant_verifies_it_by_status_and_releases_it() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let held = [
        held("droid", "a", hold_reason::NO_GRANT),
        held("droid", "a", contract::NOT_OWNER),
        held("droid", "", contract::SESSION_REQUIRED),
    ];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert_eq!(done.released, released(&["a"]));
    assert_eq!(done.enrolled, Some(enrolled(&["droid"])));
    let state = store.load().unwrap();
    let stored = state.grant(&account(), "droid", "a").unwrap();
    assert!(valid_grant(stored.grant.expose()) && stored.origin_id == "origin");
    assert!(state.operations.is_empty(), "{:?}", state.operations);
    assert!(state.watches.is_empty());
    // One claim for the session, whatever reasons held it, then its status;
    // nothing for the records without a session.
    assert_eq!(issuer.claims().len(), 1);
    let installation_id = state
        .installation(&account())
        .unwrap()
        .installation_id
        .clone();
    assert_eq!(
        issuer.calls().last(),
        Some(&Call::Status(StatusRequest {
            installation_id: installation_id.unwrap(),
            source: "droid".into(),
            session_ids: vec!["a".into()],
        }))
    );
    // The grant now presents.
    let presented = present(
        [("droid", Some("a"))],
        "origin",
        &enrolled(&["droid"]),
        |s, x| state.grant(&account(), s, x),
    );
    assert!(matches!(presented, Presentation::Send(entries) if entries.len() == 1));
}

#[test]
fn reconcile_never_calls_the_issuer_for_a_source_that_is_not_enrolled() {
    let (_directory, store) = store();
    let issuer = on(&store, &["claude"]);
    let held = [
        held("droid", "a", hold_reason::NO_GRANT),
        held("droid", "", contract::SESSION_REQUIRED),
    ];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert_eq!(done.released, released(&["", "a"]));
    assert!(issuer.calls().is_empty());
    let state = store.load().unwrap();
    assert!(state.installations.is_empty(), "nothing installed");
    assert_eq!(state.enrolled(&account()), Some(enrolled(&["claude"])));

    // A service that publishes no enrolment, with none known: nothing at
    // all, and nothing released either.
    let (_fresh_directory, fresh) = self::store();
    let silent = Fake {
        store: Some(fresh.path().to_owned()),
        ..Default::default()
    };
    let done = reconcile(&silent, &fresh, &account(), "origin", &held, 0).unwrap();
    assert_eq!(done, Reconciled::default());
    assert!(silent.calls().is_empty());
    assert!(!fresh.path().exists());
}

#[test]
fn an_unknown_or_unreadable_enrolment_falls_back_to_the_last_list_of_the_exact_account() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    issuer
        .claims
        .borrow_mut()
        .push_back(Err(IssuerError::Transport("reset".into())));
    let held = [
        held("droid", "a", hold_reason::NO_GRANT),
        held("claude", "c", hold_reason::NO_GRANT),
    ];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert_eq!(done.released, vec![("claude".into(), "c".into())]);
    let pending = store
        .load()
        .unwrap()
        .operation(&account(), "droid", "a")
        .cloned()
        .unwrap();
    assert!(pending.pending());

    // The list cannot be read: the last one published still applies, so
    // "droid" is not taken for unenrolled, and its operation is kept and
    // replayed as it was.
    issuer.enrolled_fails.set(true);
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert_eq!(done.enrolled, Some(enrolled(&["droid"])));
    assert_eq!(
        done.released,
        vec![("claude".into(), "c".into()), ("droid".into(), "a".into())]
    );
    let claims = issuer.claims();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0], claims[1]);
    assert_eq!(claims[0].1, pending.operation_uuid);

    // Not said at all: the same.
    issuer.enrolled_fails.set(false);
    issuer.enrolled_unsaid.set(true);
    let silent = &issuer;
    let done = reconcile(silent, &store, &account(), "origin", &held, 0).unwrap();
    assert_eq!(done.enrolled, Some(enrolled(&["droid"])));
    assert!(done.released.contains(&("claude".into(), "c".into())));

    // Another user of the workspace has no list of its own: nothing is
    // released and nothing is asked.
    let other = Account {
        user: "user_other".into(),
        ..account()
    };
    let calls = silent.calls().len();
    let done = reconcile(silent, &store, &other, "origin", &held, 0).unwrap();
    assert_eq!(done, Reconciled::default());
    assert_eq!(silent.calls().len(), calls);
    assert_eq!(
        store.load().unwrap().enrolled(&account()),
        Some(enrolled(&["droid"]))
    );
}

#[test]
fn accounts_are_exact_and_entries_without_a_site_or_user_are_dropped() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();

    // Another user of the same workspace, and the same ids on another site:
    // neither sees this account's installation or grant, and each gets its
    // own installation.
    let other_user = Account {
        user: "user_other".into(),
        ..account()
    };
    let other_site = Account {
        site: "https://other.test".into(),
        ..account()
    };
    for other in [&other_user, &other_site] {
        let state = store.load().unwrap();
        assert!(state.grant(other, "droid", "a").is_none());
        assert!(state.installation(other).is_none());
        reconcile(&issuer, &store, other, "origin", &held, 0).unwrap();
    }
    let state = store.load().unwrap();
    assert_eq!(state.installations.len(), 3);
    assert_eq!(state.grants.len(), 3);
    assert_eq!(issuer.registrations(), 3);
    let grant_ids: BTreeSet<&str> = state
        .grants
        .iter()
        .map(|grant| grant.grant_id.as_str())
        .collect();
    assert_eq!(grant_ids.len(), 3);

    // An account that is not exact is refused before any call.
    let calls = issuer.calls().len();
    let inexact = Account {
        user: String::new(),
        ..account()
    };
    assert!(reconcile(&issuer, &store, &inexact, "origin", &held, 0).is_err());
    assert_eq!(issuer.calls().len(), calls);

    // A file written before the account was exact: its entries are dropped
    // on read and the file is rewritten without them.
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap();
    for list in ["installations", "grants", "enrolled"] {
        let entries = raw[list].as_array_mut().unwrap();
        let mut old = entries[0].clone();
        let account = old["account"].as_object_mut().unwrap();
        account.remove("site");
        account.remove("user");
        entries.push(old);
    }
    std::fs::write(store.path(), serde_json::to_vec(&raw).unwrap()).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded, state);
    store.update(|_| Ok(())).unwrap();
    let rewritten: State = serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap();
    assert_eq!(rewritten, state);
}

#[test]
fn an_expired_grant_stays_held_as_renewal_unsupported_and_only_refreshes_its_status() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    ensure_installation(&issuer, &store, &account(), "origin").unwrap();
    store
        .update(|state| {
            state.put_grant(grant("droid", "a", 500, "origin"));
            Ok(())
        })
        .unwrap();
    let before = store.load().unwrap();
    let held = [held("droid", "a", contract::GRANT_EXPIRED)];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert!(done.released.is_empty());
    assert_eq!(
        done.relabeled,
        vec![self::held("droid", "a", hold_reason::RENEWAL_UNSUPPORTED)]
    );
    assert_eq!(issuer.statuses(), 1);
    assert!(issuer.claims().is_empty() && issuer.rebinds().is_empty());
    let state = store.load().unwrap();
    let watch = state.watch(&account(), "droid", "a").unwrap();
    assert_eq!(watch.reason, hold_reason::RENEWAL_UNSUPPORTED);
    assert_eq!(watch.not_before_ms, reconcile::WATCH_INTERVAL_MS);
    assert_eq!(state.grants, before.grants, "the expired grant is kept");

    // Held under the new reason: nothing until the hold cadence, then only
    // the status again. Never a claim, a renewal or a new installation.
    let relabeled = [self::held("droid", "a", hold_reason::RENEWAL_UNSUPPORTED)];
    let done = reconcile(&issuer, &store, &account(), "origin", &relabeled, 1).unwrap();
    assert_eq!(
        done,
        Reconciled {
            enrolled: Some(enrolled(&["droid"])),
            ..Reconciled::default()
        }
    );
    assert_eq!(issuer.statuses(), 1);
    let done = reconcile(&issuer, &store, &account(), "origin", &relabeled, LATER).unwrap();
    assert!(done.released.is_empty() && done.relabeled.is_empty());
    assert_eq!(issuer.statuses(), 2);
    assert!(issuer.claims().is_empty() && issuer.rebinds().is_empty());
    assert_eq!(issuer.registrations(), 1);
}

#[test]
fn the_same_valid_grant_is_released_once_status_shows_it_active() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let (id, _) = ensure_installation(&issuer, &store, &account(), "origin").unwrap();
    // The claim answers the grant the issuer already has for this
    // installation: no change, and that is not a reason to hold.
    issuer.own("droid", "a", &id, 500, contract::OWNER_ACTIVE);
    issuer.claims.borrow_mut().push_back(Ok(ClaimResponse {
        outcome: contract::OUTCOME_REPLAY.into(),
        grant_id: uuid(500),
        owner_epoch: 500,
        revision: None,
        expires_at: None,
        grant: g(500),
    }));
    let held_a = [held("droid", "a", hold_reason::NO_GRANT)];
    let done = reconcile(&issuer, &store, &account(), "origin", &held_a, 0).unwrap();
    assert_eq!(done.released, released(&["a"]));
    assert_eq!((issuer.claims().len(), issuer.statuses()), (1, 1));

    // A stored grant refused by the writer is verified, not claimed again:
    // active with its grant id and epoch, it is released.
    store
        .update(|state| {
            state.put_grant(grant("droid", "b", 600, "origin"));
            Ok(())
        })
        .unwrap();
    issuer.own("droid", "b", &id, 600, contract::OWNER_ACTIVE);
    let held_b = [held("droid", "b", "owner_draining")];
    let done = reconcile(&issuer, &store, &account(), "origin", &held_b, 0).unwrap();
    assert_eq!(done.released, released(&["b"]));
    assert_eq!((issuer.claims().len(), issuer.statuses()), (1, 2));

    // Active, but with another epoch or grant id than the stored one: held.
    store
        .update(|state| {
            state.put_grant(grant("droid", "c", 700, "origin"));
            Ok(())
        })
        .unwrap();
    issuer.own("droid", "c", &id, 701, contract::OWNER_ACTIVE);
    let held_c = [held("droid", "c", "owner_changed")];
    let done = reconcile(&issuer, &store, &account(), "origin", &held_c, 0).unwrap();
    assert!(done.released.is_empty());
    assert_eq!(issuer.claims().len(), 1);
    let watch = store
        .load()
        .unwrap()
        .watch(&account(), "droid", "c")
        .cloned()
        .unwrap();
    assert_eq!(watch.reason, "owner_active");
}

#[test]
fn a_draining_owner_holds_and_polls_status_without_a_new_claim_until_it_changes() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    issuer.own(
        "droid",
        "a",
        "other-installation",
        700,
        contract::OWNER_DRAINING,
    );
    issuer
        .claims
        .borrow_mut()
        .push_back(Err(IssuerError::Refused {
            reason: "owner_draining".into(),
            retryable: true,
        }));
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert!(done.released.is_empty());
    // An owner wait is not retried, even marked retryable.
    assert_eq!((issuer.claims().len(), issuer.statuses()), (1, 1));
    let state = store.load().unwrap();
    let watch = state.watch(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(watch.reason, "owner_draining");
    assert_eq!(
        watch
            .observed
            .as_ref()
            .map(|observed| observed.state.as_str()),
        Some(contract::OWNER_DRAINING)
    );
    let first = state.operation(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(first.refused.as_deref(), Some("owner_draining"));

    // Not due: nothing. Due and unchanged: the status only, never a claim.
    reconcile(&issuer, &store, &account(), "origin", &held, 1).unwrap();
    assert_eq!((issuer.claims().len(), issuer.statuses()), (1, 1));
    for pass in 1..=3 {
        let now = pass * LATER;
        let done = reconcile(&issuer, &store, &account(), "origin", &held, now).unwrap();
        assert!(done.released.is_empty());
        assert_eq!(issuer.claims().len(), 1);
        assert_eq!(issuer.statuses(), 1 + pass as usize);
    }

    // The owner finished draining: the status transition plans one new
    // claim, under a new uuid, which is verified and released.
    issuer.owner_state("droid", "a", contract::OWNER_RELEASED);
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 5 * LATER).unwrap();
    assert_eq!(done.released, released(&["a"]));
    let claims = issuer.claims();
    assert_eq!(claims.len(), 2);
    assert_ne!(claims[1].1, first.operation_uuid);
    let state = store.load().unwrap();
    assert!(state.watches.is_empty() && state.operations.is_empty());
}

#[test]
fn a_replayed_claim_refused_for_a_draining_owner_stays_held_until_its_status_changes() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    issuer.own(
        "droid",
        "a",
        "other-installation",
        700,
        contract::OWNER_ACTIVE,
    );
    issuer.claims.borrow_mut().extend([
        Err(IssuerError::Transport("reset".into())),
        Err(IssuerError::Refused {
            reason: "owner_draining".into(),
            retryable: false,
        }),
    ]);
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    // The historical operation is replayed exactly and refused: held.
    issuer.owner_state("droid", "a", contract::OWNER_DRAINING);
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 10).unwrap();
    assert!(done.released.is_empty());
    let claims = issuer.claim_requests();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0], claims[1]);
    for now in [11, 10 + LATER, 10 + 2 * LATER] {
        let done = reconcile(&issuer, &store, &account(), "origin", &held, now).unwrap();
        assert!(done.released.is_empty());
    }
    assert_eq!(issuer.claims().len(), 2, "no new claim while draining");

    issuer.owner_state("droid", "a", contract::OWNER_RELEASED);
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 10 + 3 * LATER).unwrap();
    assert_eq!(done.released, released(&["a"]));
    let claims = issuer.claim_requests();
    assert_eq!(claims.len(), 3);
    assert_ne!(claims[2].operation_uuid, claims[0].operation_uuid);
}

#[test]
fn a_busy_session_keeps_the_same_operation_across_passes() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    issuer
        .claims
        .borrow_mut()
        .extend((0..6).map(|_| Err(busy())));
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    // Three attempts end the pass, not the operation.
    for pass in 1..=2 {
        let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
        assert!(done.released.is_empty());
        assert_eq!(issuer.claims().len(), 3 * pass);
        let operation = store
            .load()
            .unwrap()
            .operation(&account(), "droid", "a")
            .cloned()
            .unwrap();
        assert!(operation.pending(), "{operation:?}");
    }
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert_eq!(done.released, released(&["a"]));
    let claims = issuer.claim_requests();
    assert_eq!(claims.len(), 7);
    assert!(claims.iter().all(|claim| *claim == claims[0]));
}

#[test]
fn a_transport_failure_replays_the_same_operation_next_pass() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    issuer
        .claims
        .borrow_mut()
        .push_back(Err(IssuerError::Transport("reset".into())));
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    assert!(reconcile(&issuer, &store, &account(), "origin", &held, 0)
        .unwrap()
        .released
        .is_empty());
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert_eq!(done.released.len(), 1);
    let claims = issuer.claims();
    assert_eq!(claims[0].1, claims[1].1);
}

#[test]
fn a_registration_transport_failure_holds_its_session_and_keeps_the_pass() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    issuer.lose_next_registration.set(true);
    let held = [
        held("claude", "x", hold_reason::NO_GRANT),
        held("droid", "a", hold_reason::NO_GRANT),
        held("droid", "b", hold_reason::NO_GRANT),
    ];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    // The session of a source that is not enrolled is still released; the
    // one whose registration timed out is held, and no further issuer call
    // is made this pass.
    assert_eq!(done.released, vec![("claude".to_owned(), "x".to_owned())]);
    assert!(done.relabeled.is_empty());
    assert!(!done.sign_in_required);
    assert_eq!(issuer.registrations(), 1);
    assert!(issuer.claims().is_empty());
    // The next pass replays the same registration and claims both.
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert_eq!(
        done.released,
        vec![
            ("claude".to_owned(), "x".to_owned()),
            ("droid".to_owned(), "a".to_owned()),
            ("droid".to_owned(), "b".to_owned()),
        ]
    );
    assert_eq!(issuer.registrations(), 2);
}

#[test]
fn only_a_terminal_refusal_starts_a_new_operation_and_only_after_its_pause() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    // Terminal even though marked retryable (spec 4.2).
    issuer
        .claims
        .borrow_mut()
        .push_back(Err(IssuerError::Refused {
            reason: "stale_revision".into(),
            retryable: true,
        }));
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert!(done.released.is_empty());
    assert_eq!(issuer.claims().len(), 1, "not retried");
    let refused = store
        .load()
        .unwrap()
        .operation(&account(), "droid", "a")
        .cloned()
        .unwrap();
    assert_eq!(refused.refused.as_deref(), Some("stale_revision"));
    assert!(store
        .load()
        .unwrap()
        .grant(&account(), "droid", "a")
        .is_none());
    reconcile(&issuer, &store, &account(), "origin", &held, 1).unwrap();
    assert_eq!(issuer.claims().len(), 1);
    let done = reconcile(&issuer, &store, &account(), "origin", &held, PAUSE).unwrap();
    assert_eq!(done.released, released(&["a"]));
    let claims = issuer.claims();
    assert_eq!(claims.len(), 2);
    assert_ne!(claims[1].1, refused.operation_uuid);
}

#[test]
fn a_lost_claim_before_a_reset_is_replayed_byte_for_byte_then_rebound_separately() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    // The claim lands at the issuer under origin A, but its answer is lost.
    issuer.lose_next_claim.set(true);
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    let done = reconcile(&issuer, &store, &account(), "origin-a", &held, 0).unwrap();
    assert!(done.released.is_empty());
    let stored = store
        .load()
        .unwrap()
        .operation(&account(), "droid", "a")
        .cloned()
        .unwrap();
    assert!(stored.pending());
    assert_eq!(stored.origin_id, "origin-a");

    // A local reset moves the origin to B. The stored claim goes again
    // exactly as recorded, origin A included, and its replay is then
    // rebound to B by an operation of its own; the epoch is kept.
    let done = reconcile(&issuer, &store, &account(), "origin-b", &held, 0).unwrap();
    assert_eq!(done.released, released(&["a"]));
    let claims = issuer.claim_requests();
    assert_eq!(claims.len(), 2);
    assert_eq!(
        serde_json::to_vec(&claims[0]).unwrap(),
        serde_json::to_vec(&claims[1]).unwrap()
    );
    assert_eq!(claims[1].origin_id, "origin-a");
    assert_eq!(claims[1].operation_uuid, stored.operation_uuid);
    let rebinds = issuer.rebinds();
    assert_eq!(rebinds.len(), 1);
    assert_eq!(rebinds[0].origin_id, "origin-b");
    assert_ne!(rebinds[0].operation_uuid, stored.operation_uuid);
    let claimed = issuer.server.borrow().claimed[&stored.operation_uuid]
        .1
        .clone();
    assert_eq!(rebinds[0].grant_id, claimed.grant_id);
    assert_eq!(rebinds[0].grant, claimed.grant);
    let state = store.load().unwrap();
    let grant = state.grant(&account(), "droid", "a").unwrap();
    assert_eq!(grant.origin_id, "origin-b");
    assert_eq!(grant.owner_epoch, claimed.owner_epoch);
    assert_eq!(grant.grant.expose(), claimed.grant);
    assert!(state.operations.is_empty());
    assert_eq!(issuer.registrations(), 1);
}

#[test]
fn not_authorized_waits_for_sign_in_once_and_leaves_grants_proof_and_pacing_alone() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    *issuer.sign_in.borrow_mut() = sign_in_fingerprint("rth_at_without_acquire");
    assert!(!issuer.sign_in().contains("rth_at"));
    // An earlier session was claimed: the installation and a grant exist.
    let first = [held("droid", "x", hold_reason::NO_GRANT)];
    reconcile(&issuer, &store, &account(), "origin", &first, 0).unwrap();
    let before = store.load().unwrap();
    assert_eq!(before.grants.len(), 1);
    let calls = issuer.calls().len();

    issuer
        .claims
        .borrow_mut()
        .push_back(Err(IssuerError::NotAuthorized));
    let held = [
        held("droid", "a", hold_reason::NO_GRANT),
        held("droid", "b", contract::GRANT_EXPIRED),
    ];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert!(done.released.is_empty());
    assert!(done.sign_in_required && done.newly_required);
    // One call refused; the pass stopped there and "b" was not tried.
    assert_eq!(issuer.calls().len(), calls + 1);
    assert_eq!(issuer.claims().len(), 2);
    let after = store.load().unwrap();
    assert_eq!(after.grants, before.grants);
    assert_eq!(after.installations, before.installations);
    assert_eq!(
        after.reauthorization,
        vec![Reauthorization {
            account: account(),
            sign_in: issuer.sign_in(),
        }]
    );
    // Not refused and not paced: the operation keeps the uuid it was sent
    // with and no pause.
    let operation = after.operation(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(operation.refused, None);
    assert_eq!(operation.not_before_ms, 0);
    assert_eq!(operation.operation_uuid, issuer.claims()[1].1);
    assert_eq!(after.operations.len(), 1);
    assert!(sign_in_required(&store, &account()));
    assert!(!sign_in_required(
        &store,
        &Account {
            workspace: "ws_other".into(),
            ..account()
        }
    ));
    assert!(!sign_in_required(
        &store,
        &Account {
            user: String::new(),
            ..account()
        }
    ));

    // The same sign-in: no issuer call past the enrolled sources, whenever
    // the pass runs, and state.json is left byte for byte as it was.
    let saved = std::fs::read(store.path()).unwrap();
    for now in [0, i64::MAX / 2] {
        let done = reconcile(&issuer, &store, &account(), "origin", &held, now).unwrap();
        assert!(done.sign_in_required && !done.newly_required);
        assert!(done.released.is_empty());
    }
    assert_eq!(issuer.calls().len(), calls + 1);
    assert_eq!(std::fs::read(store.path()).unwrap(), saved);
}

#[test]
fn a_new_sign_in_retries_and_a_second_refusal_waits_the_same_way() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    *issuer.sign_in.borrow_mut() = sign_in_fingerprint("rth_at_one");
    issuer.claims.borrow_mut().extend([
        Err(IssuerError::NotAuthorized),
        Err(IssuerError::NotAuthorized),
    ]);
    let held = [
        held("droid", "a", hold_reason::NO_GRANT),
        held("droid", "b", hold_reason::NO_GRANT),
    ];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert!(done.sign_in_required && done.newly_required);
    assert_eq!(issuer.claims().len(), 1);

    // A refreshed or new sign-in is tried on the next pass. Refused again:
    // recorded for that sign-in the same way, nothing escalates or pauses.
    *issuer.sign_in.borrow_mut() = sign_in_fingerprint("rth_at_two");
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert!(done.sign_in_required && done.newly_required);
    assert!(done.released.is_empty());
    let claims = issuer.claims();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0], claims[1], "same session, same operation uuid");
    let state = store.load().unwrap();
    assert_eq!(state.reauthorization.len(), 1);
    assert_eq!(state.reauthorization[0].sign_in, issuer.sign_in());
    assert!(state
        .operations
        .iter()
        .all(|operation| operation.refused.is_none() && operation.not_before_ms == 0));

    // A sign-in with session ownership: both sessions are claimed and
    // released, and acquisition no longer waits.
    *issuer.sign_in.borrow_mut() = sign_in_fingerprint("rth_at_three");
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    assert!(!done.sign_in_required && !done.newly_required);
    assert_eq!(done.released, released(&["a", "b"]));
    assert_eq!(issuer.claims()[2].1, claims[0].1);
    let state = store.load().unwrap();
    assert!(state.reauthorization.is_empty());
    assert!(state.operations.is_empty());
    assert!(!sign_in_required(&store, &account()));
}

#[test]
fn a_session_another_installation_owns_waits_on_its_owner_and_never_stops_the_pass() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    *issuer.sign_in.borrow_mut() = sign_in_fingerprint("rth_at_with_acquire");
    issuer.own(
        "droid",
        "a",
        "other-installation",
        700,
        contract::OWNER_ACTIVE,
    );
    // 403 not_owner for "a", the first session of the pass.
    issuer
        .claims
        .borrow_mut()
        .push_back(Err(IssuerError::NotOwner));
    let both = [
        held("droid", "a", hold_reason::NO_GRANT),
        held("droid", "b", hold_reason::NO_GRANT),
    ];
    let done = reconcile(&issuer, &store, &account(), "origin", &both, 0).unwrap();
    // "b" is claimed and released in the same pass; "a" stays held, waiting
    // on its owner, and nothing asks for sign-in.
    assert_eq!(done.released, released(&["b"]));
    assert_eq!(
        done.relabeled,
        vec![HeldSession {
            source: "droid".into(),
            session_id: "a".into(),
            reason: contract::NOT_OWNER.into(),
        }]
    );
    assert!(!done.sign_in_required && !done.newly_required);
    let state = store.load().unwrap();
    assert!(state.reauthorization.is_empty());
    assert!(!sign_in_required(&store, &account()));
    assert!(state.grant(&account(), "droid", "a").is_none());
    let watch = state.watch(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(watch.reason, contract::NOT_OWNER);
    let refused = state.operation(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(refused.refused.as_deref(), Some(contract::NOT_OWNER));
    assert_eq!(issuer.claims().len(), 2);

    // Later passes re-check "a" through the status route, with no new
    // claim while its owner is unchanged.
    let held = [held("droid", "a", contract::NOT_OWNER)];
    let statuses = issuer.statuses();
    for now in [1, LATER, 2 * LATER] {
        let done = reconcile(&issuer, &store, &account(), "origin", &held, now).unwrap();
        assert!(done.released.is_empty());
        assert!(!done.sign_in_required);
    }
    assert_eq!(
        issuer.claims().len(),
        2,
        "no new claim while owned elsewhere"
    );
    assert_eq!(
        issuer.statuses(),
        statuses + 2,
        "polled on the hold cadence"
    );

    // The other installation lets go: a new claim, verified and released.
    issuer.owner_state("droid", "a", contract::OWNER_RELEASED);
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 3 * LATER).unwrap();
    assert_eq!(done.released, released(&["a"]));
    let claims = issuer.claims();
    assert_eq!(claims.len(), 3);
    assert_ne!(claims[2].1, refused.operation_uuid);
    assert!(store.load().unwrap().reauthorization.is_empty());
}

/// A usable grant of "a" bound to "old-origin" (left by a reset), with this
/// installation registered, and another installation shown as the owner.
/// The first pass's rebind is answered `403 not_owner`; returns the
/// installation id and the stored grant.
fn stale_origin_refused_not_owner(issuer: &Fake, store: &Store) -> (String, Grant) {
    let (installation_id, _) =
        ensure_installation(issuer, store, &account(), "new-origin").unwrap();
    let stale = grant("droid", "a", 7, "old-origin");
    store
        .update(|state| {
            state.put_grant(stale.clone());
            Ok(())
        })
        .unwrap();
    issuer.own(
        "droid",
        "a",
        "other-installation",
        700,
        contract::OWNER_ACTIVE,
    );
    issuer
        .rebinds
        .borrow_mut()
        .push_back(Err(IssuerError::NotOwner));
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    let done = reconcile(issuer, store, &account(), "new-origin", &moved, 0).unwrap();
    assert!(done.released.is_empty());
    assert_eq!(
        done.relabeled,
        vec![held("droid", "a", contract::NOT_OWNER)]
    );
    assert!(!done.sign_in_required);
    assert_eq!(issuer.rebinds().len(), 1);
    assert_eq!(issuer.rebinds()[0].grant_id, stale.grant_id);
    assert_eq!(issuer.statuses(), 1, "the refusal reads the owner once");
    assert!(issuer.claims().is_empty());
    let state = store.load().unwrap();
    assert_eq!(state.operations.len(), 1);
    let refused = state.operation(&account(), "droid", "a").unwrap();
    assert_eq!(refused.kind, store::REBIND);
    assert_eq!(refused.refused.as_deref(), Some(contract::NOT_OWNER));
    let watch = state.watch(&account(), "droid", "a").unwrap();
    assert_eq!(watch.reason, contract::NOT_OWNER);
    assert_eq!(watch.not_before_ms, reconcile::WATCH_INTERVAL_MS);
    assert_eq!(state.grant(&account(), "droid", "a"), Some(&stale));
    (installation_id, stale)
}

#[test]
fn stale_origin_not_owner_regression() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let (installation_id, stale) = stale_origin_refused_not_owner(&issuer, &store);
    let operation = store
        .load()
        .unwrap()
        .operation(&account(), "droid", "a")
        .cloned()
        .unwrap();
    let watch = store
        .load()
        .unwrap()
        .watch(&account(), "droid", "a")
        .cloned()
        .unwrap();
    let held = [held("droid", "a", contract::NOT_OWNER)];

    // Every pass before the watch is due holds with no issuer call at all
    // and no new operation.
    let calls = issuer.calls().len();
    for minute in 1..5 {
        let now = minute * 60 * 1000;
        let done = reconcile(&issuer, &store, &account(), "new-origin", &held, now).unwrap();
        assert!(done.released.is_empty() && done.relabeled.is_empty());
        assert!(!done.sign_in_required);
        assert_eq!(issuer.calls().len(), calls, "no issuer call at {now}");
        let state = store.load().unwrap();
        assert_eq!(state.operations, vec![operation.clone()]);
        assert_eq!(state.watch(&account(), "droid", "a"), Some(&watch));
    }

    // Due, with the owner unchanged: one status call and nothing else.
    let due = watch.not_before_ms;
    let done = reconcile(&issuer, &store, &account(), "new-origin", &held, due).unwrap();
    assert!(done.released.is_empty());
    assert_eq!(issuer.calls().len(), calls + 1);
    assert!(matches!(issuer.calls().last(), Some(Call::Status(_))));
    assert_eq!(issuer.rebinds().len(), 1);
    assert!(issuer.claims().is_empty());
    let state = store.load().unwrap();
    assert_eq!(state.operations, vec![operation.clone()]);
    let rearmed = state.watch(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(rearmed.reason, watch.reason);
    assert_eq!(rearmed.observed, watch.observed);
    assert_eq!(rearmed.not_before_ms, due + reconcile::WATCH_INTERVAL_MS);
    assert_eq!(state.grant(&account(), "droid", "a"), Some(&stale));

    // Next due, the status shows the stored grant active with its epoch,
    // still bound to the old origin: rebound first, then verified.
    issuer.own("droid", "a", &installation_id, 7, contract::OWNER_ACTIVE);
    let calls = issuer.calls().len();
    let done = reconcile(
        &issuer,
        &store,
        &account(),
        "new-origin",
        &held,
        rearmed.not_before_ms,
    )
    .unwrap();
    assert_eq!(done.released, released(&["a"]));
    let tail = issuer.calls()[calls..].to_vec();
    assert_eq!(tail.len(), 3, "status, rebind, verifying status: {tail:?}");
    assert!(matches!(tail[0], Call::Status(_)));
    assert!(matches!(tail[1], Call::Rebind(_)));
    assert!(matches!(tail[2], Call::Status(_)));
    let rebinds = issuer.rebinds();
    assert_eq!(rebinds.len(), 2);
    assert_ne!(rebinds[1].operation_uuid, operation.operation_uuid);
    assert_eq!(rebinds[1].grant_id, stale.grant_id);
    assert_eq!(rebinds[1].origin_id, "new-origin");
    assert!(issuer.claims().is_empty());
    let state = store.load().unwrap();
    let after = state.grant(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(after.origin_id, "new-origin");
    assert_eq!(after.grant_id, stale.grant_id);
    assert_eq!(after.owner_epoch, stale.owner_epoch);
    assert!(state.watch(&account(), "droid", "a").is_none());
    assert!(state.operations.is_empty());
}

#[test]
fn stale_origin_due_watch_owner_changed() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let (_, stale) = stale_origin_refused_not_owner(&issuer, &store);
    let held = [held("droid", "a", contract::NOT_OWNER)];

    // Due, and another installation took the session over with a new
    // epoch: the old grant is dropped and the session is claimed, never
    // rebound again. The claim waits on that owner in turn.
    issuer.own(
        "droid",
        "a",
        "third-installation",
        701,
        contract::OWNER_ACTIVE,
    );
    issuer
        .claims
        .borrow_mut()
        .push_back(Err(IssuerError::NotOwner));
    let due = reconcile::WATCH_INTERVAL_MS;
    let done = reconcile(&issuer, &store, &account(), "new-origin", &held, due).unwrap();
    assert!(done.released.is_empty());
    assert_eq!(issuer.rebinds().len(), 1);
    assert_eq!(issuer.claims().len(), 1);
    let state = store.load().unwrap();
    assert!(state.grant(&account(), "droid", "a").is_none());
    let operation = state.operation(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(operation.kind, store::CLAIM);
    assert_eq!(operation.refused.as_deref(), Some(contract::NOT_OWNER));
    let watch = state.watch(&account(), "droid", "a").cloned().unwrap();

    // With no grant left, later passes are the ordinary owner wait of a
    // session without one: status on the hold cadence, no rebind.
    for now in [due + 60 * 1000, watch.not_before_ms] {
        let done = reconcile(&issuer, &store, &account(), "new-origin", &held, now).unwrap();
        assert!(done.released.is_empty());
    }
    assert_eq!(issuer.rebinds().len(), 1);
    assert_eq!(issuer.claims().len(), 1);

    // The owner lets go: a new claim on the current origin, released once
    // its status shows it active.
    issuer.owner_state("droid", "a", contract::OWNER_RELEASED);
    let now = watch.not_before_ms + reconcile::WATCH_INTERVAL_MS;
    let done = reconcile(&issuer, &store, &account(), "new-origin", &held, now).unwrap();
    assert_eq!(done.released, released(&["a"]));
    assert_eq!(issuer.rebinds().len(), 1, "the dead grant is never rebound");
    assert_eq!(issuer.claims().len(), 2);
    let state = store.load().unwrap();
    let after = state.grant(&account(), "droid", "a").cloned().unwrap();
    assert_ne!(after.grant_id, stale.grant_id);
    assert_eq!(after.origin_id, "new-origin");
    assert!(state.watch(&account(), "droid", "a").is_none());
}

#[test]
fn stale_origin_due_watch_released_drops_the_grant_and_claims() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let (_, stale) = stale_origin_refused_not_owner(&issuer, &store);
    let held = [held("droid", "a", contract::NOT_OWNER)];
    // Due, and the owner released the session: claimed at once on the
    // current origin and released, with no rebind of the old grant.
    issuer.owner_state("droid", "a", contract::OWNER_RELEASED);
    let due = reconcile::WATCH_INTERVAL_MS;
    let done = reconcile(&issuer, &store, &account(), "new-origin", &held, due).unwrap();
    assert_eq!(done.released, released(&["a"]));
    assert_eq!(issuer.rebinds().len(), 1);
    assert_eq!(issuer.claims().len(), 1);
    let state = store.load().unwrap();
    let after = state.grant(&account(), "droid", "a").cloned().unwrap();
    assert_ne!(after.grant_id, stale.grant_id);
    assert_eq!(after.origin_id, "new-origin");
    assert!(state.watch(&account(), "droid", "a").is_none());
    assert!(state.operations.is_empty());
}

#[test]
fn a_stale_origin_grant_after_a_409_owner_wait_polls_status_only() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    ensure_installation(&issuer, &store, &account(), "new-origin").unwrap();
    store
        .update(|state| {
            state.put_grant(grant("droid", "a", 7, "old-origin"));
            Ok(())
        })
        .unwrap();
    issuer.own(
        "droid",
        "a",
        "other-installation",
        700,
        contract::OWNER_DRAINING,
    );
    issuer
        .rebinds
        .borrow_mut()
        .push_back(Err(IssuerError::Refused {
            reason: "owner_draining".into(),
            retryable: false,
        }));
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    let mut now = 0;
    while now <= 3 * reconcile::WATCH_INTERVAL_MS {
        let done = reconcile(&issuer, &store, &account(), "new-origin", &moved, now).unwrap();
        assert!(done.released.is_empty());
        now += 60 * 1000;
    }
    // One rebind ever; the refusal's status read plus one per due watch.
    assert_eq!(issuer.rebinds().len(), 1);
    assert!(issuer.claims().is_empty());
    assert_eq!(issuer.statuses(), 1 + 3);
    assert_eq!(store.load().unwrap().operations.len(), 1);
}

#[test]
fn not_authorized_beside_another_session_still_waits_for_sign_in_account_wide() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    *issuer.sign_in.borrow_mut() = sign_in_fingerprint("rth_at_without_acquire");
    issuer
        .claims
        .borrow_mut()
        .push_back(Err(IssuerError::NotAuthorized));
    let held = [
        held("droid", "a", hold_reason::NO_GRANT),
        held("droid", "b", hold_reason::NO_GRANT),
    ];
    let done = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap();
    // The pass stops at the refusal: "b" is not tried, nothing is released
    // or relabeled, and the sign-in is recorded for the whole account.
    assert!(done.released.is_empty());
    assert!(done.relabeled.is_empty());
    assert!(done.sign_in_required && done.newly_required);
    assert_eq!(issuer.claims().len(), 1);
    let state = store.load().unwrap();
    assert_eq!(
        state.reauthorization,
        vec![Reauthorization {
            account: account(),
            sign_in: issuer.sign_in(),
        }]
    );
    assert!(state.watch(&account(), "droid", "a").is_none());
    let operation = state.operation(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(operation.refused, None);
    assert_eq!(operation.not_before_ms, 0);
    assert!(sign_in_required(&store, &account()));
}

#[test]
fn any_other_forbidden_refusal_holds_only_its_session_without_asking_for_sign_in() {
    for code in ["target_mismatch", "something_new", ""] {
        let (_directory, store) = store();
        let issuer = on(&store, &["droid"]);
        *issuer.sign_in.borrow_mut() = sign_in_fingerprint("rth_at_with_acquire");
        issuer
            .claims
            .borrow_mut()
            .push_back(Err(IssuerError::Forbidden { code: code.into() }));
        let both = [
            held("droid", "a", hold_reason::NO_GRANT),
            held("droid", "b", hold_reason::NO_GRANT),
        ];
        let done = reconcile(&issuer, &store, &account(), "origin", &both, 0).unwrap();
        assert_eq!(done.released, released(&["b"]), "{code:?}");
        assert!(done.relabeled.is_empty(), "{code:?}");
        assert!(!done.sign_in_required && !done.newly_required, "{code:?}");
        let state = store.load().unwrap();
        assert!(state.reauthorization.is_empty(), "{code:?}");
        assert!(state.grant(&account(), "droid", "a").is_none(), "{code:?}");
        let refused = state.operation(&account(), "droid", "a").cloned().unwrap();
        assert!(refused.refused.is_some(), "{code:?}");
        assert!(refused.not_before_ms > 0, "{code:?}");
        // Held and paced: no new claim before the pause ends.
        let again = [held("droid", "a", hold_reason::NO_GRANT)];
        let done = reconcile(&issuer, &store, &account(), "origin", &again, 1).unwrap();
        assert!(done.released.is_empty(), "{code:?}");
        assert_eq!(issuer.claims().len(), 2, "{code:?}");
    }
}

#[test]
fn a_new_origin_presents_nothing_until_its_grant_is_rebound_with_its_epoch() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let (id, proof) = ensure_installation(&issuer, &store, &account(), "origin-1").unwrap();
    let first = [held("droid", "a", hold_reason::NO_GRANT)];
    reconcile(&issuer, &store, &account(), "origin-1", &first, 0).unwrap();
    let before = store
        .load()
        .unwrap()
        .grant(&account(), "droid", "a")
        .cloned()
        .unwrap();

    // After a reset the origin is new: the grant is held, not presented.
    let state = store.load().unwrap();
    let presented = present(
        [("droid", Some("a"))],
        "origin-2",
        &enrolled(&["droid"]),
        |s, x| state.grant(&account(), s, x),
    );
    assert_eq!(
        presented,
        Presentation::Hold(vec![held("droid", "a", hold_reason::ORIGIN_CHANGED)])
    );

    // A terminal refusal of the rebind keeps it held and paces the next.
    issuer
        .rebinds
        .borrow_mut()
        .push_back(Err(IssuerError::Refused {
            reason: "origin_cas_lost".into(),
            retryable: false,
        }));
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    let done = reconcile(&issuer, &store, &account(), "origin-2", &moved, 0).unwrap();
    assert!(done.released.is_empty());
    assert_eq!(
        store.load().unwrap().grant(&account(), "droid", "a"),
        Some(&before)
    );
    reconcile(&issuer, &store, &account(), "origin-2", &moved, 1).unwrap();
    assert_eq!(issuer.rebinds().len(), 1);

    // The rebind carries G and the new origin, then the grant presents with
    // the epoch it had.
    let done = reconcile(&issuer, &store, &account(), "origin-2", &moved, PAUSE).unwrap();
    assert_eq!(done.released, released(&["a"]));
    let rebinds = issuer.rebinds();
    assert_eq!(rebinds.len(), 2);
    assert_ne!(rebinds[0].operation_uuid, rebinds[1].operation_uuid);
    assert_eq!(rebinds[1].grant, before.grant.expose());
    assert_eq!(rebinds[1].grant_id, before.grant_id);
    assert_eq!(rebinds[1].origin_id, "origin-2");
    assert!(issuer.claims().len() == 1, "a rebind is not a claim");
    let after = store
        .load()
        .unwrap()
        .grant(&account(), "droid", "a")
        .cloned()
        .unwrap();
    assert_eq!(after.origin_id, "origin-2");
    assert_eq!(after.grant, before.grant);
    assert_eq!(after.owner_epoch, before.owner_epoch);
    // No second installation for the new origin.
    assert_eq!(issuer.registrations(), 1);
    assert_eq!(
        ensure_installation(&issuer, &store, &account(), "origin-2").unwrap(),
        (id, proof)
    );
    let state = store.load().unwrap();
    assert!(matches!(
        present([("droid", Some("a"))], "origin-2", &enrolled(&["droid"]), |s, x| {
            state.grant(&account(), s, x)
        }),
        Presentation::Send(entries) if entries.len() == 1
    ));
}

#[test]
fn a_rebind_answer_with_another_epoch_is_refused_and_never_overwrites_it() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let first = [held("droid", "a", hold_reason::NO_GRANT)];
    reconcile(&issuer, &store, &account(), "origin-1", &first, 0).unwrap();
    let before = store
        .load()
        .unwrap()
        .grant(&account(), "droid", "a")
        .cloned()
        .unwrap();
    for answer in [
        RebindResponse {
            outcome: contract::OUTCOME_REBOUND.into(),
            revision: None,
            owner_epoch: before.owner_epoch + 1,
        },
        RebindResponse {
            outcome: "renewed".into(),
            revision: None,
            owner_epoch: before.owner_epoch,
        },
    ] {
        issuer.rebinds.borrow_mut().push_back(Ok(answer));
    }
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    for now in [0, PAUSE] {
        let done = reconcile(&issuer, &store, &account(), "origin-2", &moved, now).unwrap();
        assert!(done.released.is_empty());
        let state = store.load().unwrap();
        assert_eq!(state.grant(&account(), "droid", "a"), Some(&before));
        let operation = state.operation(&account(), "droid", "a").unwrap();
        assert_eq!(operation.refused.as_deref(), Some("rebind_mismatch"));
    }
    assert_eq!(issuer.rebinds().len(), 2);
}

#[test]
fn another_installation_of_the_same_user_is_not_proven_and_gets_nothing() {
    let (_first_directory, first) = store();
    let (_second_directory, second) = store();
    let issuer = on(&first, &["droid"]);
    let (first_id, first_proof) = ensure_installation(&issuer, &first, &account(), "o1").unwrap();
    reconcile(
        &issuer,
        &first,
        &account(),
        "o1",
        &[held("droid", "a", hold_reason::NO_GRANT)],
        0,
    )
    .unwrap();

    // The second installation names the first's id with its own proof.
    let second_proof = new_proof().unwrap();
    second
        .update(|state| {
            state.installations.push(Installation {
                account: account(),
                request_uuid: uuid(77),
                proof: second_proof.clone(),
                installation_id: Some(first_id.clone()),
                origin_id: "o2".into(),
                issuer_key_id: None,
            });
            Ok(())
        })
        .unwrap();
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    let error = reconcile(&issuer, &second, &account(), "o2", &held, 0).unwrap_err();
    assert_eq!(
        error.downcast_ref::<IssuerError>(),
        Some(&IssuerError::NotProven)
    );
    let state = second.load().unwrap();
    assert!(state.grants.is_empty(), "no grant material");
    assert_eq!(state.installations[0].proof, second_proof, "proof kept");
    // The operation is kept as it was sent, for the same request later.
    let kept = state.operation(&account(), "droid", "a").unwrap();
    assert!(kept.pending());
    // Status for another installation's id never answers without its proof.
    let request = contract::status_request(&first_id, "droid", &["a".into()]).unwrap();
    let status = issuer.status(&new_proof().unwrap(), &request).unwrap_err();
    assert_eq!(status, IssuerError::NotProven);
    let theirs = issuer.status(&first_proof, &request).unwrap();
    assert_eq!(theirs.sessions[0].mine, Some(true));
    issuer
        .server
        .borrow_mut()
        .installations
        .insert("second".into(), second_proof.expose().into());
    let request = contract::status_request("second", "droid", &["a".into()]).unwrap();
    let ours = issuer.status(&second_proof, &request).unwrap();
    assert_eq!(ours.sessions[0].mine, Some(false));
}

#[test]
fn a_wrong_proof_grants_nothing_and_keeps_every_session_held() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    ensure_installation(&issuer, &store, &account(), "origin").unwrap();
    let wrong = new_proof().unwrap();
    store
        .update(|state| {
            state.installations[0].proof = wrong.clone();
            Ok(())
        })
        .unwrap();
    let held = [
        held("droid", "a", hold_reason::NO_GRANT),
        held("droid", "b", hold_reason::NO_GRANT),
    ];
    let error = reconcile(&issuer, &store, &account(), "origin", &held, 0).unwrap_err();
    assert_eq!(
        error.downcast_ref::<IssuerError>(),
        Some(&IssuerError::NotProven)
    );
    assert!(!format!("{error:#}").contains(wrong.expose()));
    let state = store.load().unwrap();
    assert!(state.grants.is_empty());
    assert_eq!(state.installations[0].proof, wrong);
    assert!(!format!("{state:?}").contains(wrong.expose()));
    // The pass stopped at the first refusal; no new proof, no new
    // registration.
    assert_eq!(issuer.claims().len(), 1);
    assert_eq!(issuer.registrations(), 1);
}

/// One HTTP exchange per reply; each raw request (head and body) is sent back.
fn serve(replies: Vec<(u16, String)>) -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{BufRead, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (sent, requests) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for (status, body) in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream);
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut content = vec![0; length];
            reader.read_exact(&mut content).unwrap();
            sent.send(format!("{head}{}", String::from_utf8(content).unwrap()))
                .unwrap();
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            reader.get_mut().write_all(reply.as_bytes()).unwrap();
        }
    });
    (base, requests)
}
fn http(base: String) -> HttpIssuer {
    HttpIssuer::new(crate::history::cloud::StoredAuth {
        base_url: base,
        access_token: "rth_at_test".into(),
        ..Default::default()
    })
}

#[test]
fn the_http_issuer_sends_the_proof_in_the_body_to_register_and_in_the_header_elsewhere() {
    let claim = serde_json::json!({
        "outcome": "installed", "grantId": uuid(5), "ownerEpoch": "5",
        "revision": 3, "expiresAt": null, "grant": g(5)
    });
    let (base, requests) = serve(vec![
        (
            200,
            serde_json::json!({"acquisition": {"enrolledSources": ["droid"]}}).to_string(),
        ),
        (
            201,
            serde_json::json!({"installationId": uuid(9), "originId": "o", "issuerKeyId": "k"})
                .to_string(),
        ),
        (200, claim.to_string()),
        (
            200,
            serde_json::json!({"outcome": "rebound", "ownerEpoch": 6}).to_string(),
        ),
        (
            200,
            serde_json::json!({"result": {"sessions": [{"sessionId": "a", "state": "active",
                "revision": "4", "tombstoneRevision": null, "ownerEpoch": "5",
                "ownerKind": "native", "grantId": uuid(5), "mine": true}]}})
            .to_string(),
        ),
    ]);
    let issuer = http(base);
    let proof = new_proof().unwrap();

    assert_eq!(
        issuer.enrolled_sources().unwrap(),
        Some(enrolled(&["droid"]))
    );
    let limits = requests.recv().unwrap();
    assert!(
        limits.starts_with(&format!("GET {} ", contract::LIMITS_PATH)),
        "{limits}"
    );
    assert!(!limits.contains(proof.expose()));

    let registered = issuer
        .register(
            &proof,
            &InstallationRequest {
                request_uuid: uuid(1),
                origin_id: "o".into(),
                label: None,
            },
        )
        .unwrap();
    assert_eq!(registered.installation_id, uuid(9));
    let register = requests.recv().unwrap();
    assert!(register.starts_with(&format!("POST {} ", contract::INSTALLATIONS_PATH)));
    assert!(register.contains("Authorization: Bearer rth_at_test"));
    assert!(!register
        .to_ascii_lowercase()
        .contains(&contract::PROOF_HEADER.to_ascii_lowercase()));
    let body: serde_json::Value =
        serde_json::from_str(register.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"requestUuid": uuid(1), "originId": "o", "installationProof": proof.expose()})
    );

    let claimed = issuer
        .claim(
            &proof,
            &ClaimRequest {
                operation_uuid: uuid(2),
                installation_id: uuid(9),
                source: "droid".into(),
                session_id: "a".into(),
                origin_id: "o".into(),
            },
        )
        .unwrap();
    assert_eq!(
        (claimed.owner_epoch, claimed.grant.as_str()),
        (5, g(5).as_str())
    );
    let claim = requests.recv().unwrap();
    assert!(claim.starts_with(&format!("POST {} ", contract::CLAIMS_PATH)));
    assert!(claim.contains(&format!("{}: {}", contract::PROOF_HEADER, proof.expose())));
    let (_, body) = claim.split_once("\r\n\r\n").unwrap();
    assert!(!body.contains(proof.expose()));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap(),
        serde_json::json!({"operationUuid": uuid(2), "installationId": uuid(9),
            "source": "droid", "sessionId": "a", "originId": "o"})
    );

    let rebound = issuer
        .rebind(
            &proof,
            &RebindRequest {
                operation_uuid: uuid(3),
                installation_id: uuid(9),
                source: "droid".into(),
                session_id: "a".into(),
                grant_id: uuid(5),
                origin_id: "o2".into(),
                grant: g(5),
            },
        )
        .unwrap();
    assert_eq!(rebound.owner_epoch, 6);
    let rebind = requests.recv().unwrap();
    assert!(rebind.starts_with(&format!("POST {} ", contract::REBINDS_PATH)));
    assert!(rebind.contains(&format!("{}: {}", contract::PROOF_HEADER, proof.expose())));
    assert!(rebind.contains(&format!("\"grant\":\"{}\"", g(5))));

    // Status is a GET of the query target: no body, no Content-Type, the
    // proof in its header.
    let request = contract::status_request(&uuid(9), "droid", &["a".into(), "b c".into()]).unwrap();
    let status = issuer.status(&proof, &request).unwrap();
    assert_eq!(
        status.sessions,
        vec![SessionStatus {
            session_id: "a".into(),
            state: contract::OWNER_ACTIVE.into(),
            revision: Some(4),
            tombstone_revision: None,
            owner_epoch: Some(5),
            owner_kind: Some("native".into()),
            grant_id: Some(uuid(5)),
            mine: Some(true),
        }]
    );
    let status = requests.recv().unwrap();
    assert!(
        status.starts_with(&format!(
            "GET {}?installationId={}&source=droid&sessionIds=a&sessionIds=b%20c ",
            contract::STATUS_PATH,
            uuid(9)
        )),
        "{status}"
    );
    assert!(status.contains(&format!("{}: {}", contract::PROOF_HEADER, proof.expose())));
    assert!(status.contains("Authorization: Bearer rth_at_test"));
    let lower = status.to_ascii_lowercase();
    assert!(!lower.contains("content-type"), "{status}");
    assert!(!lower.contains("content-length: ") || lower.contains("content-length: 0"));
    let (_, body) = status.split_once("\r\n\r\n").unwrap();
    assert!(body.is_empty(), "{body:?}");
}

/// The decoded `(name, value)` pairs of a status target's query.
fn query_pairs(target: &str) -> Vec<(String, String)> {
    let (path, query) = target.split_once('?').unwrap();
    assert_eq!(path, contract::STATUS_PATH);
    url::form_urlencoded::parse(query.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

#[test]
fn the_status_target_repeats_session_ids_once_per_encoded_id() {
    let request = contract::status_request("i", "droid", &["a".into(), "b".into()]).unwrap();
    assert_eq!(
        request.target(),
        format!(
            "{}?installationId=i&source=droid&sessionIds=a&sessionIds=b",
            contract::STATUS_PATH
        )
    );
    let odd = "x,y&z=w v\u{e9}\u{1f600}".to_owned();
    let request = contract::status_request("i d", "dr&oid", &[odd.clone(), "a,b".into()]).unwrap();
    let target = request.target();
    // Every reserved byte is percent-encoded; a space is `%20`, never `+`,
    // and nothing outside ASCII is left raw.
    assert!(target.is_ascii());
    assert!(!target.contains('+'));
    assert!(
        target.contains("&sessionIds=x%2Cy%26z%3Dw%20v%C3%A9%F0%9F%98%80&"),
        "{target}"
    );
    assert!(target.ends_with("&sessionIds=a%2Cb"), "{target}");
    assert_eq!(
        query_pairs(&target),
        vec![
            ("installationId".into(), "i d".into()),
            ("source".into(), "dr&oid".into()),
            ("sessionIds".into(), odd),
            ("sessionIds".into(), "a,b".into()),
        ]
    );
}

#[test]
fn the_status_request_drops_duplicate_ids_in_order() {
    let request = contract::status_request(
        "i",
        "droid",
        &["b".into(), "a".into(), "b".into(), "a".into(), "c".into()],
    )
    .unwrap();
    assert_eq!(request.session_ids, vec!["b", "a", "c"]);
    let sent: Vec<_> = query_pairs(&request.target())
        .into_iter()
        .filter(|(name, _)| name == "sessionIds")
        .map(|(_, value)| value)
        .collect();
    assert_eq!(sent, vec!["b", "a", "c"]);
    // Duplicates past the cap still fit once deduplicated.
    let repeated = vec!["a".to_owned(); contract::STATUS_MAX_SESSIONS + 1];
    assert_eq!(
        contract::status_request("i", "droid", &repeated)
            .unwrap()
            .session_ids,
        vec!["a"]
    );
}

#[test]
fn the_status_target_stays_within_eight_kib() {
    let fixed = contract::status_request("i", "droid", &["".into()])
        .unwrap()
        .target()
        .len();
    // One id that brings the target to exactly the bound, and one byte over.
    let fits = "x".repeat(contract::STATUS_MAX_TARGET - fixed);
    let request = contract::status_request("i", "droid", std::slice::from_ref(&fits)).unwrap();
    assert_eq!(request.target().len(), contract::STATUS_MAX_TARGET);
    assert!(contract::status_request("i", "droid", &[format!("{fits}x")]).is_none());
    // Encoding counts: a space is three bytes on the wire.
    let spaces = " ".repeat((contract::STATUS_MAX_TARGET - fixed) / 3 + 1);
    assert!(contract::status_request("i", "droid", &[spaces]).is_none());
    // The byte bound applies below the id cap: 400 ids of 30 bytes.
    let long: Vec<String> = (0..400).map(|n| format!("{n:030}")).collect();
    assert!(long.len() <= contract::STATUS_MAX_SESSIONS);
    assert!(contract::status_request("i", "droid", &long).is_none());
    assert!(contract::status_request("i", "droid", &long[..100]).is_some());
}

#[test]
fn the_status_answer_reads_the_result_envelope_and_ignores_unknown_fields() {
    let body = serde_json::json!({"result": {"sessions": [
        {"sessionId": "a", "state": "active", "revision": "9007199254740993",
            "tombstoneRevision": "0", "ownerEpoch": "7", "ownerKind": "native",
            "grantId": null, "mine": false, "futureField": {"x": 1}},
        {"sessionId": "b", "state": "unknown", "revision": 3, "tombstoneRevision": "12",
            "ownerEpoch": null, "ownerKind": null, "grantId": null, "mine": false}
    ], "cursor": "ignored"}, "extra": true});
    let answer: contract::StatusEnvelope = serde_json::from_value(body).unwrap();
    assert_eq!(
        answer.result.sessions,
        vec![
            SessionStatus {
                session_id: "a".into(),
                state: contract::OWNER_ACTIVE.into(),
                revision: Some(9_007_199_254_740_993),
                tombstone_revision: Some(0),
                owner_epoch: Some(7),
                owner_kind: Some("native".into()),
                grant_id: None,
                mine: Some(false),
            },
            SessionStatus {
                session_id: "b".into(),
                state: contract::OWNER_UNKNOWN.into(),
                revision: Some(3),
                tombstone_revision: Some(12),
                owner_epoch: None,
                owner_kind: None,
                grant_id: None,
                mine: Some(false),
            },
        ]
    );
    // The bare list is not the wire.
    assert!(serde_json::from_value::<contract::StatusEnvelope>(
        serde_json::json!({"sessions": []})
    )
    .is_err());
    // A bigint that is not a decimal string or an integer is refused.
    for bad in [
        serde_json::json!({"sessionId": "a", "state": "active", "revision": "01"}),
        serde_json::json!({"sessionId": "a", "state": "active", "tombstoneRevision": "-1"}),
        serde_json::json!({"sessionId": "a", "state": "active", "ownerEpoch": "00"}),
        serde_json::json!({"sessionId": "a", "state": "active", "ownerEpoch": "-1"}),
        serde_json::json!({"sessionId": "a", "state": "active", "ownerEpoch": -1}),
    ] {
        assert!(
            serde_json::from_value::<SessionStatus>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}

/// `mine` decides whether an owner is another installation, which drops a
/// quarantined grant, so a row that leaves it out (or sends `null`) reads
/// as unknown, never as `false`.
#[test]
fn a_status_row_without_mine_reads_as_unknown_not_as_another_owner() {
    let row = serde_json::json!({"sessionId": "a", "state": "active",
        "ownerEpoch": "7", "grantId": null});
    let status = serde_json::from_value::<SessionStatus>(row.clone()).unwrap();
    assert_eq!(status.mine, None);
    assert_eq!(status.owner_epoch, Some(7));
    let mut null = row.clone();
    null["mine"] = serde_json::Value::Null;
    assert_eq!(
        serde_json::from_value::<SessionStatus>(null).unwrap().mine,
        None
    );
    for (mine, read) in [(true, Some(true)), (false, Some(false))] {
        let mut explicit = row.clone();
        explicit["mine"] = serde_json::json!(mine);
        assert_eq!(
            serde_json::from_value::<SessionStatus>(explicit)
                .unwrap()
                .mine,
            read
        );
    }
    let mut malformed = row;
    malformed["mine"] = serde_json::json!("false");
    assert!(serde_json::from_value::<SessionStatus>(malformed).is_err());
}

#[test]
fn a_status_owner_epoch_of_zero_is_no_owner_and_leaves_owned_sessions_intact() {
    let body = serde_json::json!({"result": {"sessions": [
        {"sessionId": "unowned", "state": "unknown", "revision": "0",
            "tombstoneRevision": "0", "ownerEpoch": "0", "ownerKind": null,
            "grantId": null, "mine": false},
        {"sessionId": "owned", "state": "active", "revision": "4",
            "tombstoneRevision": "0", "ownerEpoch": "7", "ownerKind": "native",
            "grantId": uuid(7), "mine": true},
        {"sessionId": "unowned-int", "state": "unknown", "ownerEpoch": 0}
    ]}});
    let answer: contract::StatusEnvelope = serde_json::from_value(body).unwrap();
    let sessions = answer.result.sessions;
    assert_eq!(sessions.len(), 3);
    assert_eq!(sessions[0].session_id, "unowned");
    assert_eq!(sessions[0].owner_epoch, None);
    assert_eq!(sessions[2].owner_epoch, None);
    assert_eq!(
        sessions[1],
        SessionStatus {
            session_id: "owned".into(),
            state: contract::OWNER_ACTIVE.into(),
            revision: Some(4),
            tombstone_revision: Some(0),
            owner_epoch: Some(7),
            owner_kind: Some("native".into()),
            grant_id: Some(uuid(7)),
            mine: Some(true),
        }
    );
    // A claim or rebind epoch of 0 is still refused.
    let claim = |epoch: serde_json::Value| {
        serde_json::from_value::<ClaimResponse>(serde_json::json!({
            "outcome": "installed", "grantId": uuid(7), "ownerEpoch": epoch, "grant": g(7)
        }))
    };
    assert!(claim(serde_json::json!("0")).is_err());
    assert!(claim(serde_json::json!(0)).is_err());
    assert_eq!(claim(serde_json::json!("7")).unwrap().owner_epoch, 7);
    let rebind = |epoch: serde_json::Value| {
        serde_json::from_value::<RebindResponse>(
            serde_json::json!({"outcome": "rebound", "ownerEpoch": epoch}),
        )
    };
    assert!(rebind(serde_json::json!("0")).is_err());
    assert!(rebind(serde_json::json!(0)).is_err());
}

#[test]
fn an_unowned_session_in_a_status_answer_never_blocks_or_releases_another() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let both = [
        held("droid", "a", hold_reason::NO_GRANT),
        held("droid", "b", hold_reason::NO_GRANT),
    ];
    reconcile(&issuer, &store, &account(), "origin", &both, 0).unwrap();
    let state = store.load().unwrap();
    let (a, b) = (
        state.grant(&account(), "droid", "a").cloned().unwrap(),
        state.grant(&account(), "droid", "b").cloned().unwrap(),
    );
    // "a" reads as never owned (epoch "0" is no owner): held, not released.
    // "b" shows its own grant active: released.
    let status = |session: &str, state: &str, epoch: &str, grant_id: Option<String>| {
        serde_json::json!({"sessionId": session, "state": state, "ownerEpoch": epoch,
            "grantId": grant_id, "mine": true})
    };
    let answer: contract::StatusEnvelope = serde_json::from_value(serde_json::json!({
        "result": {"sessions": [
            status("a", "active", "0", Some(a.grant_id.clone())),
            status("b", "active", &b.owner_epoch.to_string(), Some(b.grant_id.clone())),
        ]}
    }))
    .unwrap();
    // The same mixed answer to each session's status call.
    issuer
        .statuses
        .borrow_mut()
        .extend([Ok(answer.result.clone()), Ok(answer.result)]);
    let done = reconcile(&issuer, &store, &account(), "origin", &both, 1).unwrap();
    assert_eq!(done.released, released(&["b"]));
}

#[test]
fn the_http_issuer_maps_status_refusals_and_never_sends_an_oversized_target() {
    let error = |code: &str| serde_json::json!({"error": {"code": code}}).to_string();
    let cases =
        vec![
        (414, error("request_uri_too_long"), IssuerError::InvalidRequest),
        (400, error("invalid_request"), IssuerError::InvalidRequest),
        (403, error("installation_not_proven"), IssuerError::NotProven),
        (403, error("not_authorized"), IssuerError::NotAuthorized),
        (
            503,
            error("acquisition_disabled"),
            IssuerError::Transport("HTTP 503".into()),
        ),
        (
            503,
            error("introspection_unavailable"),
            IssuerError::Transport("HTTP 503".into()),
        ),
        (
            503,
            serde_json::json!({"error": {"code": "acquisition_unavailable", "retryable": true}})
                .to_string(),
            IssuerError::Transport("HTTP 503".into()),
        ),
    ];
    let (base, requests) = serve(
        cases
            .iter()
            .map(|(status, body, _)| (*status, body.clone()))
            .collect(),
    );
    let issuer = http(base);
    let proof = new_proof().unwrap();
    let request = contract::status_request(&uuid(9), "droid", &["a".into()]).unwrap();
    for (status, _, expected) in cases {
        let got = issuer.status(&proof, &request).unwrap_err();
        assert_eq!(got, expected, "HTTP {status}");
        assert!(requests.recv().unwrap().starts_with("GET "));
    }
    // A hand-built request past the bound is refused before it is sent.
    let oversized = StatusRequest {
        installation_id: uuid(9),
        source: "droid".into(),
        session_ids: vec!["x".repeat(contract::STATUS_MAX_TARGET)],
    };
    assert_eq!(
        issuer.status(&proof, &oversized).unwrap_err(),
        IssuerError::InvalidRequest
    );
    assert!(requests
        .recv_timeout(std::time::Duration::from_millis(200))
        .is_err());
}

#[test]
fn the_status_request_is_bounded_and_never_empty() {
    let many: Vec<String> = (0..=contract::STATUS_MAX_SESSIONS)
        .map(|n| n.to_string())
        .collect();
    assert!(contract::status_request("i", "droid", &many).is_none());
    assert!(
        contract::status_request("i", "droid", &many[..contract::STATUS_MAX_SESSIONS]).is_some()
    );
    assert!(contract::status_request("i", "droid", &[]).is_none());
    assert!(contract::status_request("", "droid", &many[..1]).is_none());
    assert!(contract::status_request("i", "", &many[..1]).is_none());
}

#[test]
fn the_http_issuer_maps_each_refusal_to_its_typed_error() {
    let error = |code: &str| serde_json::json!({"error": {"code": code}}).to_string();
    let cases = vec![
        (409, error("installation_proof_mismatch"), IssuerError::ProofMismatch),
        (403, error("installation_not_proven"), IssuerError::NotProven),
        (403, error("not_authorized"), IssuerError::NotAuthorized),
        (400, error("invalid_request"), IssuerError::InvalidRequest),
        (
            409,
            serde_json::json!({"error": {"code": "acquisition_refused", "reason": "owner_draining", "retryable": true}})
                .to_string(),
            IssuerError::Refused {
                reason: "owner_draining".into(),
                retryable: true,
            },
        ),
        (
            409,
            serde_json::json!({"error": {"code": "acquisition_refused", "reason": "owner_other_principal"}})
                .to_string(),
            IssuerError::Refused {
                reason: "owner_other_principal".into(),
                retryable: false,
            },
        ),
        (404, error("not_found"), IssuerError::Unavailable),
        (503, "unavailable".into(), IssuerError::Transport("HTTP 503".into())),
    ];
    let (base, _requests) = serve(
        cases
            .iter()
            .map(|(status, body, _)| (*status, body.clone()))
            .collect(),
    );
    let issuer = http(base);
    let proof = new_proof().unwrap();
    let request = ClaimRequest {
        operation_uuid: uuid(2),
        installation_id: uuid(9),
        source: "droid".into(),
        session_id: "a".into(),
        origin_id: "o".into(),
    };
    for (status, _, expected) in cases {
        let got = issuer.claim(&proof, &request).unwrap_err();
        assert_eq!(got, expected, "HTTP {status}");
        assert!(!got.to_string().contains(proof.expose()));
    }
}

#[test]
fn the_http_issuer_asks_for_sign_in_only_on_an_exact_not_authorized_403() {
    let error = |code: &str| serde_json::json!({"error": {"code": code}}).to_string();
    let forbidden = |code: &str| IssuerError::Forbidden { code: code.into() };
    let cases = vec![
        (error("not_authorized"), IssuerError::NotAuthorized),
        (error("not_owner"), IssuerError::NotOwner),
        (error("installation_not_proven"), IssuerError::NotProven),
        (error("target_mismatch"), forbidden("target_mismatch")),
        (error("not_installer"), IssuerError::NotInstaller),
        (error("something_new"), forbidden("something_new")),
        // A code that is not shaped like one is not kept.
        (error("Not Authorized <b>"), forbidden("")),
        (error(&"x".repeat(65)), forbidden("")),
        (serde_json::json!({"error": {}}).to_string(), forbidden("")),
        ("forbidden".into(), forbidden("")),
        (String::new(), forbidden("")),
    ];
    let replies =
        || -> Vec<(u16, String)> { cases.iter().map(|(body, _)| (403, body.clone())).collect() };
    let proof = new_proof().unwrap();

    // The claim route.
    let (base, requests) = serve(replies());
    let issuer = http(base);
    let claim = ClaimRequest {
        operation_uuid: uuid(2),
        installation_id: uuid(9),
        source: "droid".into(),
        session_id: "a".into(),
        origin_id: "o".into(),
    };
    for (body, expected) in &cases {
        let got = issuer.claim(&proof, &claim).unwrap_err();
        assert_eq!(&got, expected, "claim 403 {body:?}");
        assert!(requests.recv().unwrap().starts_with("POST "));
        assert!(!got.to_string().contains(proof.expose()));
    }

    // The status route.
    let (base, requests) = serve(replies());
    let issuer = http(base);
    let status = contract::status_request(&uuid(9), "droid", &["a".into()]).unwrap();
    for (body, expected) in &cases {
        let got = issuer.status(&proof, &status).unwrap_err();
        assert_eq!(&got, expected, "status 403 {body:?}");
        assert!(requests.recv().unwrap().starts_with("GET "));
    }

    // The rebind route maps exactly as the claim route: its
    // `not_authorized` is the bearer scope check too (PR 331 at 680d26b8),
    // so it asks for sign-in; ownership is `not_owner` / `not_installer`.
    let (base, requests) = serve(replies());
    let issuer = http(base);
    let rebind = RebindRequest {
        operation_uuid: uuid(3),
        installation_id: uuid(9),
        source: "droid".into(),
        session_id: "a".into(),
        grant_id: uuid(4),
        origin_id: "o".into(),
        grant: g(4),
    };
    for (body, expected) in &cases {
        let got = issuer.rebind(&proof, &rebind).unwrap_err();
        assert_eq!(&got, expected, "rebind 403 {body:?}");
        assert!(requests.recv().unwrap().starts_with("POST "));
    }

    assert_eq!(IssuerError::NotOwner.to_string(), contract::NOT_OWNER);
    assert_eq!(
        IssuerError::NotInstaller.to_string(),
        contract::NOT_INSTALLER
    );
    assert_eq!(
        forbidden("target_mismatch").to_string(),
        "forbidden: target_mismatch"
    );
    assert_eq!(forbidden("").to_string(), "forbidden");
}

/// `{"error":{"code":code}}`, as the service writes a refusal.
fn refusal_body(code: &str) -> String {
    serde_json::json!({"error": {"code": code}}).to_string()
}

/// The fake service, except that the routes switched on go over HTTP
/// through the real [`HttpIssuer`] to [`serve`]'s replies, so its refusal
/// mapping is exercised end to end by the reconcile. Calls are recorded on
/// the fake either way.
struct Routed<'a> {
    fake: &'a Fake,
    http: HttpIssuer,
    http_claims: Cell<bool>,
    http_rebinds: Cell<bool>,
    http_statuses: Cell<bool>,
}
impl<'a> Routed<'a> {
    fn new(fake: &'a Fake, base: String) -> Self {
        Self {
            fake,
            http: http(base),
            http_claims: Cell::new(false),
            http_rebinds: Cell::new(false),
            http_statuses: Cell::new(false),
        }
    }
}
impl AcquisitionIssuer for Routed<'_> {
    fn enrolled_sources(&self) -> Result<Option<BTreeSet<String>>, IssuerError> {
        self.fake.enrolled_sources()
    }
    fn register(
        &self,
        proof: &Secret,
        request: &InstallationRequest,
    ) -> Result<InstallationResponse, IssuerError> {
        self.fake.register(proof, request)
    }
    fn claim(&self, proof: &Secret, request: &ClaimRequest) -> Result<ClaimResponse, IssuerError> {
        if !self.http_claims.get() {
            return self.fake.claim(proof, request);
        }
        self.fake
            .calls
            .borrow_mut()
            .push(Call::Claim(request.clone()));
        self.http.claim(proof, request)
    }
    fn rebind(
        &self,
        proof: &Secret,
        request: &RebindRequest,
    ) -> Result<RebindResponse, IssuerError> {
        if !self.http_rebinds.get() {
            return self.fake.rebind(proof, request);
        }
        self.fake
            .calls
            .borrow_mut()
            .push(Call::Rebind(request.clone()));
        self.http.rebind(proof, request)
    }
    fn status(
        &self,
        proof: &Secret,
        request: &StatusRequest,
    ) -> Result<StatusResponse, IssuerError> {
        if !self.http_statuses.get() {
            return self.fake.status(proof, request);
        }
        self.fake
            .calls
            .borrow_mut()
            .push(Call::Status(request.clone()));
        self.http.status(proof, request)
    }
    fn sign_in(&self) -> String {
        self.fake.sign_in()
    }
}

/// PR 331 at 680d26b8: `403 not_authorized` is the bearer scope check only,
/// on rebinds too. A rebind refused so (a sign-in from before `rth:acquire`)
/// asks for exactly one normal sign-in: no fail-closed refusal, no one-hour
/// pause, no second rebind under the same sign-in; the next sign-in replays
/// the same rebind once.
#[test]
fn rebind_missing_scope_regression() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    *issuer.sign_in.borrow_mut() = sign_in_fingerprint("rth_at_without_acquire");
    // "a" has a grant bound to the origin before a reset.
    reconcile(
        &issuer,
        &store,
        &account(),
        "origin-1",
        &[held("droid", "a", hold_reason::NO_GRANT)],
        0,
    )
    .unwrap();
    let before = store
        .load()
        .unwrap()
        .grant(&account(), "droid", "a")
        .cloned()
        .unwrap();
    let (base, requests) = serve(vec![(403, refusal_body("not_authorized"))]);
    let routed = Routed::new(&issuer, base);
    routed.http_rebinds.set(true);
    let both = [
        held("droid", "a", hold_reason::ORIGIN_CHANGED),
        held("droid", "b", hold_reason::NO_GRANT),
    ];
    let done = reconcile(&routed, &store, &account(), "origin-2", &both, 0).unwrap();
    assert!(done.sign_in_required && done.newly_required);
    assert!(done.released.is_empty());
    assert!(done.relabeled.is_empty());
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("POST {} ", contract::REBINDS_PATH)));
    assert_eq!(issuer.rebinds().len(), 1);
    // The pass stopped at the refusal: "b" was not claimed.
    assert_eq!(issuer.claims().len(), 1);
    let state = store.load().unwrap();
    assert_eq!(
        state.reauthorization,
        vec![Reauthorization {
            account: account(),
            sign_in: issuer.sign_in(),
        }]
    );
    // Not refused and not paced: the rebind is kept as it was sent.
    let operation = state.operation(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(operation.kind, store::REBIND);
    assert_eq!(operation.refused, None);
    assert_eq!(operation.not_before_ms, 0);
    assert_eq!(operation.operation_uuid, issuer.rebinds()[0].operation_uuid);
    assert_eq!(state.grant(&account(), "droid", "a"), Some(&before));
    assert!(state.watch(&account(), "droid", "a").is_none());

    // The same sign-in, whenever the pass runs: no second rebind.
    for now in [0, PAUSE] {
        let done = reconcile(&routed, &store, &account(), "origin-2", &both, now).unwrap();
        assert!(done.sign_in_required && !done.newly_required);
        assert!(done.released.is_empty());
    }
    assert_eq!(issuer.rebinds().len(), 1);
    assert!(requests
        .recv_timeout(std::time::Duration::from_millis(200))
        .is_err());

    // After the one normal sign-in the same rebind is replayed once.
    *issuer.sign_in.borrow_mut() = sign_in_fingerprint("rth_at_with_acquire");
    routed.http_rebinds.set(false);
    let done = reconcile(&routed, &store, &account(), "origin-2", &both, 0).unwrap();
    assert!(!done.sign_in_required);
    assert_eq!(done.released, released(&["a", "b"]));
    let rebinds = issuer.rebinds();
    assert_eq!(rebinds.len(), 2);
    assert_eq!(rebinds[0], rebinds[1], "the same operation, replayed");
    assert!(store.load().unwrap().reauthorization.is_empty());
}

#[test]
fn a_claim_refused_not_owner_over_http_waits_on_its_owner_without_sign_in() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    issuer.own(
        "droid",
        "a",
        "other-installation",
        700,
        contract::OWNER_ACTIVE,
    );
    let (base, requests) = serve(vec![(403, refusal_body("not_owner"))]);
    let routed = Routed::new(&issuer, base);
    routed.http_claims.set(true);
    let done = reconcile(
        &routed,
        &store,
        &account(),
        "origin",
        &[held("droid", "a", hold_reason::NO_GRANT)],
        0,
    )
    .unwrap();
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("POST {} ", contract::CLAIMS_PATH)));
    assert!(done.released.is_empty());
    assert_eq!(
        done.relabeled,
        vec![held("droid", "a", contract::NOT_OWNER)]
    );
    assert!(!done.sign_in_required && !done.newly_required);
    assert_eq!(issuer.claims().len(), 1);
    assert_eq!(issuer.statuses(), 1, "the refusal reads the owner once");
    let state = store.load().unwrap();
    assert!(state.reauthorization.is_empty());
    assert!(state.grant(&account(), "droid", "a").is_none());
    let refused = state.operation(&account(), "droid", "a").unwrap();
    assert_eq!(refused.refused.as_deref(), Some(contract::NOT_OWNER));
    assert_eq!(refused.not_before_ms, 0);
    let watch = state.watch(&account(), "droid", "a").unwrap();
    assert_eq!(watch.reason, contract::NOT_OWNER);
    assert_eq!(watch.not_before_ms, reconcile::WATCH_INTERVAL_MS);
}

#[test]
fn a_rebind_refused_not_owner_over_http_waits_on_its_owner_without_sign_in() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    ensure_installation(&issuer, &store, &account(), "new-origin").unwrap();
    let stale = grant("droid", "a", 7, "old-origin");
    store
        .update(|state| {
            state.put_grant(stale.clone());
            Ok(())
        })
        .unwrap();
    issuer.own(
        "droid",
        "a",
        "other-installation",
        700,
        contract::OWNER_ACTIVE,
    );
    let (base, requests) = serve(vec![(403, refusal_body("not_owner"))]);
    let routed = Routed::new(&issuer, base);
    routed.http_rebinds.set(true);
    let done = reconcile(
        &routed,
        &store,
        &account(),
        "new-origin",
        &[held("droid", "a", hold_reason::ORIGIN_CHANGED)],
        0,
    )
    .unwrap();
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("POST {} ", contract::REBINDS_PATH)));
    assert!(done.released.is_empty());
    assert_eq!(
        done.relabeled,
        vec![held("droid", "a", contract::NOT_OWNER)]
    );
    assert!(!done.sign_in_required);
    assert_eq!(issuer.rebinds().len(), 1);
    assert!(issuer.claims().is_empty());
    let state = store.load().unwrap();
    assert!(state.reauthorization.is_empty());
    let refused = state.operation(&account(), "droid", "a").unwrap();
    assert_eq!(refused.kind, store::REBIND);
    assert_eq!(refused.refused.as_deref(), Some(contract::NOT_OWNER));
    let watch = state.watch(&account(), "droid", "a").unwrap();
    assert_eq!(watch.reason, contract::NOT_OWNER);
    assert_eq!(state.grant(&account(), "droid", "a"), Some(&stale));
}

/// A claim refused `403 not_installer` presented no grant: it is refused
/// for that session alone and paced, with no sign-in, and the next fresh
/// claim waits for the pause.
#[test]
fn a_claim_refused_not_installer_is_paced_without_sign_in_then_claimed_afresh() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let (base, requests) = serve(vec![(403, refusal_body("not_installer"))]);
    let routed = Routed::new(&issuer, base);
    routed.http_claims.set(true);
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    let done = reconcile(&routed, &store, &account(), "origin", &held, 0).unwrap();
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("POST {} ", contract::CLAIMS_PATH)));
    assert!(done.released.is_empty());
    assert!(done.relabeled.is_empty());
    assert!(!done.sign_in_required && !done.newly_required);
    let state = store.load().unwrap();
    assert!(state.reauthorization.is_empty());
    assert!(state.watch(&account(), "droid", "a").is_none());
    let refused = state.operation(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(refused.refused.as_deref(), Some(contract::NOT_INSTALLER));
    assert_eq!(refused.not_before_ms, PAUSE - 1);
    routed.http_claims.set(false);
    let done = reconcile(&routed, &store, &account(), "origin", &held, 1).unwrap();
    assert!(done.released.is_empty());
    assert_eq!(issuer.claims().len(), 1, "paced");
    let done = reconcile(&routed, &store, &account(), "origin", &held, PAUSE).unwrap();
    assert_eq!(done.released, released(&["a"]));
    let claims = issuer.claims();
    assert_eq!(claims.len(), 2);
    assert_ne!(claims[1].1, refused.operation_uuid, "a fresh claim");
}

/// The backend's SQL installer-role guard (42501) answers a rebind of a
/// valid grant `403 not_installer`, the same code as a foreign grant. The
/// scoped status still shows the session this installation's, active with
/// the stored grant id and epoch, so the grant is kept (quarantined, not
/// presented again), no fresh claim is made (it would only meet
/// `handover_not_ready` against that grant), and the rebind is refused and
/// paced. Once the role is repaired, the paced pass reads the status again
/// and rebinds the original grant, which is then released.
#[test]
fn rebind_not_installer_role_guard_regression() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    reconcile(
        &issuer,
        &store,
        &account(),
        "origin-1",
        &[held("droid", "a", hold_reason::NO_GRANT)],
        0,
    )
    .unwrap();
    let before = store
        .load()
        .unwrap()
        .grant(&account(), "droid", "a")
        .cloned()
        .unwrap();
    assert_eq!(issuer.claims().len(), 1);
    let (base, requests) = serve(vec![(403, refusal_body("not_installer"))]);
    let routed = Routed::new(&issuer, base);
    routed.http_rebinds.set(true);
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    let statuses = issuer.statuses();
    let done = reconcile(&routed, &store, &account(), "origin-2", &moved, 0).unwrap();
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("POST {} ", contract::REBINDS_PATH)));
    assert!(done.released.is_empty());
    assert!(done.relabeled.is_empty());
    assert!(!done.sign_in_required && !done.newly_required);
    assert_eq!(issuer.rebinds().len(), 1);
    assert_eq!(
        issuer.claims().len(),
        1,
        "no fresh claim against its own grant"
    );
    assert_eq!(
        issuer.statuses(),
        statuses + 1,
        "the refusal reads status once"
    );
    let state = store.load().unwrap();
    assert!(state.reauthorization.is_empty());
    assert_eq!(state.grant(&account(), "droid", "a"), Some(&before));
    let refused = state.operation(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(refused.kind, store::REBIND);
    assert_eq!(refused.refused.as_deref(), Some(contract::NOT_INSTALLER));
    assert_eq!(refused.not_before_ms, PAUSE - 1);
    let watch = state.watch(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(watch.reason, contract::NOT_INSTALLER);
    assert_eq!(
        watch.observed.as_ref().map(|seen| seen.grant_id.clone()),
        Some(Some(before.grant_id.clone()))
    );

    // Quarantined for the pause: no call at all, the grant never presented.
    let calls = issuer.calls().len();
    for now in [1, LATER, PAUSE - 2] {
        let done = reconcile(&routed, &store, &account(), "origin-2", &moved, now).unwrap();
        assert!(done.released.is_empty() && done.relabeled.is_empty());
        assert_eq!(issuer.calls().len(), calls, "no issuer call at {now}");
        assert_eq!(
            store.load().unwrap().grant(&account(), "droid", "a"),
            Some(&before)
        );
    }

    // The role is repaired: the paced pass reads the status, which still
    // shows the original grant active, and rebinds that same grant.
    routed.http_rebinds.set(false);
    let done = reconcile(&routed, &store, &account(), "origin-2", &moved, PAUSE).unwrap();
    assert_eq!(done.released, released(&["a"]));
    let tail = issuer.calls()[calls..].to_vec();
    assert_eq!(tail.len(), 3, "status, rebind, verifying status: {tail:?}");
    assert!(matches!(tail[0], Call::Status(_)));
    assert!(matches!(tail[1], Call::Rebind(_)));
    assert!(matches!(tail[2], Call::Status(_)));
    let rebinds = issuer.rebinds();
    assert_eq!(rebinds.len(), 2);
    assert_eq!(rebinds[1].grant_id, before.grant_id);
    assert_eq!(rebinds[1].origin_id, "origin-2");
    assert_ne!(rebinds[1].operation_uuid, refused.operation_uuid);
    assert_eq!(issuer.claims().len(), 1, "never claimed afresh");
    let state = store.load().unwrap();
    let after = state.grant(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(after.grant_id, before.grant_id);
    assert_eq!(after.owner_epoch, before.owner_epoch);
    assert_eq!(after.origin_id, "origin-2");
    assert!(state.watch(&account(), "droid", "a").is_none());
    assert!(state.operations.is_empty());
}

/// A rebind refused `403 not_installer` for a grant kept from an
/// installation this one replaced: the scoped status names another owner
/// (not `mine`), which disowns the grant, so it is dropped (that grant id
/// only, never presented again) and the session is claimed afresh in the
/// same pass. No sign-in and no owner wait.
#[test]
fn a_rebind_refused_not_installer_for_a_disowned_grant_drops_it_and_claims_afresh() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    ensure_installation(&issuer, &store, &account(), "new-origin").unwrap();
    let foreign = grant("droid", "a", 7, "old-origin");
    store
        .update(|state| {
            state.put_grant(foreign.clone());
            Ok(())
        })
        .unwrap();
    issuer.own(
        "droid",
        "a",
        "replaced-installation",
        7,
        contract::OWNER_ACTIVE,
    );
    let (base, requests) = serve(vec![(403, refusal_body("not_installer"))]);
    let routed = Routed::new(&issuer, base);
    routed.http_rebinds.set(true);
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    let done = reconcile(&routed, &store, &account(), "new-origin", &moved, 0).unwrap();
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("POST {} ", contract::REBINDS_PATH)));
    assert!(!done.sign_in_required && !done.newly_required);
    assert!(done.relabeled.is_empty());
    assert_eq!(done.released, released(&["a"]));
    let rebinds = issuer.rebinds();
    assert_eq!(rebinds.len(), 1);
    assert_eq!(rebinds[0].grant_id, foreign.grant_id);
    let claims = issuer.claim_requests();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].origin_id, "new-origin");
    assert_ne!(claims[0].operation_uuid, rebinds[0].operation_uuid);
    let state = store.load().unwrap();
    assert!(state.reauthorization.is_empty());
    assert!(state.watch(&account(), "droid", "a").is_none());
    assert!(state.operations.is_empty());
    let after = state.grant(&account(), "droid", "a").cloned().unwrap();
    assert_ne!(after.grant_id, foreign.grant_id);
    assert_eq!(after.origin_id, "new-origin");
    assert!(!state
        .grants
        .iter()
        .any(|kept| kept.grant_id == foreign.grant_id));
}

/// A rebind refused `403 not_installer` whose status decides nothing (the
/// session is `unknown`): the grant is kept, quarantined, and only its
/// status is read after the pause; once the status disowns it (`released`)
/// it is dropped and the session claimed afresh, never rebound again.
#[test]
fn a_rebind_refused_not_installer_keeps_its_grant_until_status_disowns_it() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let (installation_id, _) =
        ensure_installation(&issuer, &store, &account(), "new-origin").unwrap();
    let stale = grant("droid", "a", 7, "old-origin");
    store
        .update(|state| {
            state.put_grant(stale.clone());
            Ok(())
        })
        .unwrap();
    issuer
        .rebinds
        .borrow_mut()
        .push_back(Err(IssuerError::NotInstaller));
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    let done = reconcile(&issuer, &store, &account(), "new-origin", &moved, 0).unwrap();
    assert!(done.released.is_empty() && done.relabeled.is_empty());
    assert!(!done.sign_in_required);
    assert!(issuer.claims().is_empty());
    let state = store.load().unwrap();
    assert_eq!(state.grant(&account(), "droid", "a"), Some(&stale));
    assert_eq!(
        state.watch(&account(), "droid", "a").unwrap().reason,
        contract::NOT_INSTALLER
    );

    // After the pause, still undecided: one status call, nothing presented.
    let calls = issuer.calls().len();
    let done = reconcile(&issuer, &store, &account(), "new-origin", &moved, PAUSE).unwrap();
    assert!(done.released.is_empty());
    assert_eq!(issuer.calls().len(), calls + 1);
    assert!(matches!(issuer.calls().last(), Some(Call::Status(_))));
    assert_eq!(issuer.rebinds().len(), 1);
    assert!(issuer.claims().is_empty());
    let state = store.load().unwrap();
    assert_eq!(state.grant(&account(), "droid", "a"), Some(&stale));
    let watch = state.watch(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(watch.not_before_ms, PAUSE + reconcile::WATCH_INTERVAL_MS);

    // The status disowns it: dropped, claimed afresh, released.
    issuer.own("droid", "a", &installation_id, 7, contract::OWNER_RELEASED);
    let done = reconcile(
        &issuer,
        &store,
        &account(),
        "new-origin",
        &moved,
        watch.not_before_ms,
    )
    .unwrap();
    assert_eq!(done.released, released(&["a"]));
    assert_eq!(
        issuer.rebinds().len(),
        1,
        "the disowned grant is never rebound"
    );
    assert_eq!(issuer.claims().len(), 1);
    let state = store.load().unwrap();
    let after = state.grant(&account(), "droid", "a").cloned().unwrap();
    assert_ne!(after.grant_id, stale.grant_id);
    assert_eq!(after.origin_id, "new-origin");
    assert!(state.watch(&account(), "droid", "a").is_none());
    assert!(state.operations.is_empty());
}

/// A rebind refused `403 not_installer` whose 200 status row (read over
/// HTTP) is active with the grant's epoch but leaves `mine` out: that is no
/// answer about the owner, never another installation's, so the grant is
/// kept, the session held and the quarantine watch kept, on the refusal and
/// again on the paced pass. No fresh claim is made.
#[test]
fn a_quarantined_grant_survives_a_status_row_without_mine() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    ensure_installation(&issuer, &store, &account(), "new-origin").unwrap();
    let kept = grant("droid", "a", 7, "old-origin");
    store
        .update(|state| {
            state.put_grant(kept.clone());
            Ok(())
        })
        .unwrap();
    let partial = serde_json::json!({"result": {"sessions": [
        {"sessionId": "a", "state": "active", "revision": "4",
            "tombstoneRevision": "0", "ownerEpoch": "7", "ownerKind": "native",
            "grantId": null}
    ]}})
    .to_string();
    let (base, requests) = serve(vec![
        (403, refusal_body("not_installer")),
        (200, partial.clone()),
        (200, partial),
    ]);
    let routed = Routed::new(&issuer, base);
    routed.http_rebinds.set(true);
    routed.http_statuses.set(true);
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    let done = reconcile(&routed, &store, &account(), "new-origin", &moved, 0).unwrap();
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("POST {} ", contract::REBINDS_PATH)));
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("GET {}?", contract::STATUS_PATH)));
    assert!(done.released.is_empty() && done.relabeled.is_empty());
    assert!(!done.sign_in_required && !done.newly_required);
    assert!(issuer.claims().is_empty(), "never claimed afresh");
    let state = store.load().unwrap();
    assert_eq!(state.grant(&account(), "droid", "a"), Some(&kept));
    let watch = state.watch(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(watch.reason, contract::NOT_INSTALLER);
    assert_eq!(watch.observed, None, "the row is not kept as an answer");

    // The paced pass reads the same partial row: still held, still kept.
    let done = reconcile(&routed, &store, &account(), "new-origin", &moved, PAUSE).unwrap();
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("GET {}?", contract::STATUS_PATH)));
    assert!(done.released.is_empty() && done.relabeled.is_empty());
    assert!(issuer.claims().is_empty(), "never claimed afresh");
    assert_eq!(issuer.rebinds().len(), 1, "never presented again");
    let state = store.load().unwrap();
    assert_eq!(state.grant(&account(), "droid", "a"), Some(&kept));
    let watch = state.watch(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(watch.reason, contract::NOT_INSTALLER);
    assert_eq!(watch.not_before_ms, PAUSE + reconcile::WATCH_INTERVAL_MS);
}

fn owner_unknown(retryable: bool) -> IssuerError {
    IssuerError::Refused {
        reason: contract::OWNER_UNKNOWN_REFUSAL.into(),
        retryable,
    }
}

/// A rebind refused `409 acquisition_refused` `owner_unknown` (over HTTP):
/// the session can hold no live grant, so that grant id alone is dropped,
/// no watch is kept and the session is claimed afresh in the same pass. No
/// other grant of the account is touched.
#[test]
fn a_rebind_refused_owner_unknown_drops_its_grant_and_claims_afresh() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    ensure_installation(&issuer, &store, &account(), "new-origin").unwrap();
    let stale = grant("droid", "a", 7, "old-origin");
    let other = grant("droid", "b", 3, "old-origin");
    store
        .update(|state| {
            state.put_grant(stale.clone());
            state.put_grant(other.clone());
            Ok(())
        })
        .unwrap();
    let refusal = serde_json::json!({"error": {"code": "acquisition_refused",
        "reason": "owner_unknown", "retryable": false}})
    .to_string();
    let (base, requests) = serve(vec![(409, refusal)]);
    let routed = Routed::new(&issuer, base);
    routed.http_rebinds.set(true);
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    let done = reconcile(&routed, &store, &account(), "new-origin", &moved, 0).unwrap();
    assert!(requests
        .recv()
        .unwrap()
        .starts_with(&format!("POST {} ", contract::REBINDS_PATH)));
    assert!(!done.sign_in_required && !done.newly_required);
    assert!(done.relabeled.is_empty());
    assert_eq!(done.released, released(&["a"]));
    let rebinds = issuer.rebinds();
    assert_eq!(rebinds.len(), 1);
    assert_eq!(rebinds[0].grant_id, stale.grant_id);
    let claims = issuer.claim_requests();
    assert_eq!(claims.len(), 1, "claimed afresh in the same pass");
    assert_eq!(claims[0].session_id, "a");
    assert_eq!(claims[0].origin_id, "new-origin");
    assert_ne!(claims[0].operation_uuid, rebinds[0].operation_uuid);
    let state = store.load().unwrap();
    assert!(state.reauthorization.is_empty());
    assert!(state.watch(&account(), "droid", "a").is_none());
    assert!(state.operations.is_empty());
    let after = state.grant(&account(), "droid", "a").cloned().unwrap();
    assert_ne!(after.grant_id, stale.grant_id);
    assert_eq!(after.origin_id, "new-origin");
    assert!(!state
        .grants
        .iter()
        .any(|kept| kept.grant_id == stale.grant_id));
    assert_eq!(state.grant(&account(), "droid", "b"), Some(&other));
    assert_eq!(state.grants.len(), 2);
}

/// The fresh claim after a rebind refused `owner_unknown` is refused
/// `owner_unknown` too: terminal, even when marked retryable. The claim is
/// refused and paced like any terminal code, with no owner watch and no
/// status poll: no claim, rebind or status call until the pause is over,
/// so claim and refusal never loop.
#[test]
fn a_claim_refused_owner_unknown_is_terminal_not_an_owner_wait() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    ensure_installation(&issuer, &store, &account(), "new-origin").unwrap();
    let stale = grant("droid", "a", 7, "old-origin");
    store
        .update(|state| {
            state.put_grant(stale.clone());
            Ok(())
        })
        .unwrap();
    issuer
        .rebinds
        .borrow_mut()
        .push_back(Err(owner_unknown(false)));
    issuer
        .claims
        .borrow_mut()
        .push_back(Err(owner_unknown(true)));
    let moved = [held("droid", "a", hold_reason::ORIGIN_CHANGED)];
    let statuses = issuer.statuses();
    let done = reconcile(&issuer, &store, &account(), "new-origin", &moved, 0).unwrap();
    assert!(done.released.is_empty() && done.relabeled.is_empty());
    assert!(!done.sign_in_required && !done.newly_required);
    assert_eq!(issuer.rebinds().len(), 1);
    assert_eq!(issuer.claims().len(), 1, "a terminal code is not retried");
    assert_eq!(issuer.statuses(), statuses, "no status poll");
    let state = store.load().unwrap();
    assert!(state.reauthorization.is_empty());
    assert!(state.grant(&account(), "droid", "a").is_none());
    assert!(
        state.watch(&account(), "droid", "a").is_none(),
        "no owner wait"
    );
    let refused = state.operation(&account(), "droid", "a").cloned().unwrap();
    assert_eq!(refused.kind, store::CLAIM);
    assert_eq!(
        refused.refused.as_deref(),
        Some(contract::OWNER_UNKNOWN_REFUSAL)
    );
    assert_eq!(refused.not_before_ms, PAUSE - 1);

    // Paced: no call at all until the pause is over.
    let calls = issuer.calls().len();
    for now in [1, LATER, PAUSE - 2] {
        let done = reconcile(&issuer, &store, &account(), "new-origin", &moved, now).unwrap();
        assert!(done.released.is_empty() && done.relabeled.is_empty());
        assert_eq!(issuer.calls().len(), calls, "no issuer call at {now}");
    }

    // After the pause the session is claimed once more, as after any
    // terminal refusal.
    let done = reconcile(&issuer, &store, &account(), "new-origin", &moved, PAUSE).unwrap();
    assert_eq!(done.released, released(&["a"]));
    assert_eq!(issuer.claims().len(), 2);
    assert_eq!(issuer.rebinds().len(), 1);
}

/// PR 331 at 680d26b8: a status `ownerEpoch` is `null` or a positive
/// decimal string; `"0"` stays tolerated as no owner (see
/// a_status_owner_epoch_of_zero_is_no_owner_and_leaves_owned_sessions_intact).
#[test]
fn a_status_owner_epoch_is_null_or_a_positive_decimal_string() {
    let read = |epoch: Option<serde_json::Value>| {
        let mut row = serde_json::json!({"sessionId": "a", "state": "unowned",
            "revision": "0", "grantId": null, "mine": false});
        if let Some(epoch) = epoch {
            row["ownerEpoch"] = epoch;
        }
        serde_json::from_value::<SessionStatus>(row).map(|status| status.owner_epoch)
    };
    assert_eq!(read(Some(serde_json::Value::Null)).unwrap(), None);
    assert_eq!(read(None).unwrap(), None);
    assert_eq!(read(Some(serde_json::json!("1"))).unwrap(), Some(1));
    assert_eq!(read(Some(serde_json::json!("42"))).unwrap(), Some(42));
    assert_eq!(
        read(Some(serde_json::json!("18446744073709551615"))).unwrap(),
        Some(u64::MAX)
    );
    assert_eq!(read(Some(serde_json::json!("0"))).unwrap(), None);
    for bad in [
        serde_json::json!("-1"),
        serde_json::json!("01"),
        serde_json::json!("1.5"),
        serde_json::json!(""),
        serde_json::json!("x"),
        serde_json::json!("18446744073709551616"),
        serde_json::json!(-1),
        serde_json::json!(true),
    ] {
        assert!(read(Some(bad.clone())).is_err(), "{bad}");
    }
    // A never-owned session as the service answers it: unowned, null.
    let answer: contract::StatusEnvelope = serde_json::from_value(serde_json::json!({
        "result": {"sessions": [
            {"sessionId": "never", "state": "unowned", "revision": "0",
                "tombstoneRevision": null, "ownerEpoch": null, "ownerKind": null,
                "grantId": null, "mine": false},
            {"sessionId": "missing", "state": "unknown", "revision": "0",
                "tombstoneRevision": null, "ownerEpoch": null, "ownerKind": null,
                "grantId": null, "mine": false},
            {"sessionId": "owned", "state": "active", "revision": "9",
                "tombstoneRevision": null, "ownerEpoch": "8", "ownerKind": "native",
                "grantId": uuid(8), "mine": true}
        ]}
    }))
    .unwrap();
    let epochs: Vec<Option<u64>> = answer
        .result
        .sessions
        .iter()
        .map(|status| status.owner_epoch)
        .collect();
    assert_eq!(epochs, vec![None, None, Some(8)]);
}

/// PR 331 at 680d26b8: claim and rebind success epochs are positive; a
/// claim whose install has no positive epoch yet is answered `503
/// acquisition_unavailable` (retryable). Desktop holds and replays the same
/// immutable operation (same uuid, same request bytes) on the next pass; a
/// 200 that does carry a non-positive or no epoch is unreadable and is
/// replayed the same way, never kept as a grant.
#[test]
fn a_claim_answered_503_without_a_positive_epoch_replays_the_same_operation() {
    let (_directory, store) = store();
    let issuer = on(&store, &["droid"]);
    let (installation_id, _) = ensure_installation(&issuer, &store, &account(), "origin").unwrap();
    issuer.own("droid", "a", &installation_id, 5, contract::OWNER_ACTIVE);
    let unavailable = serde_json::json!({"error": {
        "code": contract::ACQUISITION_UNAVAILABLE, "retryable": true}})
    .to_string();
    let answer = |epoch: Option<serde_json::Value>| {
        let mut body = serde_json::json!({"outcome": "installed", "grantId": uuid(5),
            "revision": "3", "expiresAt": null, "grant": g(5)});
        if let Some(epoch) = epoch {
            body["ownerEpoch"] = epoch;
        }
        body.to_string()
    };
    let replies = vec![
        (503, unavailable),
        (200, answer(Some(serde_json::json!("0")))),
        (200, answer(Some(serde_json::Value::Null))),
        (200, answer(None)),
        (200, answer(Some(serde_json::json!("5")))),
    ];
    let failing = replies.len() - 1;
    let (base, requests) = serve(replies);
    let routed = Routed::new(&issuer, base);
    routed.http_claims.set(true);
    let held = [held("droid", "a", hold_reason::NO_GRANT)];
    let mut sent = Vec::new();
    for pass in 0..failing {
        let done = reconcile(&routed, &store, &account(), "origin", &held, pass as i64).unwrap();
        assert!(done.released.is_empty(), "pass {pass}");
        assert!(done.relabeled.is_empty(), "pass {pass}");
        assert!(!done.sign_in_required, "pass {pass}");
        let state = store.load().unwrap();
        assert!(
            state.grant(&account(), "droid", "a").is_none(),
            "pass {pass}"
        );
        let kept = state.operation(&account(), "droid", "a").cloned().unwrap();
        assert!(kept.pending(), "pass {pass}");
        assert_eq!(kept.refused, None, "pass {pass}");
        sent.push(requests.recv().unwrap());
    }
    let done = reconcile(&routed, &store, &account(), "origin", &held, 9).unwrap();
    assert_eq!(done.released, released(&["a"]));
    sent.push(requests.recv().unwrap());
    let bodies: Vec<&str> = sent
        .iter()
        .map(|raw| raw.split_once("\r\n\r\n").unwrap().1)
        .collect();
    assert!(bodies.iter().all(|body| *body == bodies[0]), "{bodies:?}");
    let claims = issuer.claims();
    assert_eq!(claims.len(), failing + 1);
    assert!(claims.iter().all(|claim| claim.1 == claims[0].1));
    let kept = store
        .load()
        .unwrap()
        .grant(&account(), "droid", "a")
        .cloned()
        .unwrap();
    assert_eq!((kept.grant_id, kept.owner_epoch), (uuid(5), 5));
}
