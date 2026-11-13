use super::*;
use crate::history::scrub::{ScrubContext, ScrubSurface};

/// The fenced acquire-and-apply both hydrate paths share: fetch the
/// snapshot through the connector, land it under the revision read before
/// the fetch, retry once when the fence moved mid-flight. `unchanged` is
/// intake's verdict on the applied snapshot — the caller, not the host,
/// decides nothing moved.
pub(super) fn acquire_apply(
    pass: &Pass,
    store: &SessionStore,
    key: &ObservationKey,
    observation: ai_hist::SessionObservation,
    revision: String,
    connector: &str,
) -> Result<ai_hist::HydrateSessionResult> {
    let evidence = pass.acquire(connector, observation)?;
    apply_fenced(pass, store, key, evidence, revision, connector)
}

/// [`acquire_apply`] past its first fetch: land `evidence` under
/// `revision`, fetching again once when the fence moved.
fn apply_fenced(
    pass: &Pass,
    store: &SessionStore,
    key: &ObservationKey,
    evidence: NormalizedSourceEvidence,
    revision: String,
    connector: &str,
) -> Result<ai_hist::HydrateSessionResult> {
    let acquire = |observation| pass.acquire(connector, observation);
    let submission = SourceEvidenceSubmission::new(key.clone(), revision, evidence);
    match store.apply_source_evidence(submission) {
        Ok(result) => Ok(result),
        Err(ai_hist::Error::SourceRevisionConflict(_)) => {
            let state = store.source_observation(key)?;
            let Some(observation) = state.observation else {
                anyhow::bail!("remote observation withdrawn mid-hydration");
            };
            let revision = state
                .revision
                .context("remote observation carries no revision fence")?;
            Ok(store.apply_source_evidence(SourceEvidenceSubmission::new(
                key.clone(),
                revision,
                acquire(observation)?,
            ))?)
        }
        Err(error) => Err(error.into()),
    }
}

/// Hydrate one remote catalogued session through its connector: read the
/// observation and its revision fence, acquire the snapshot, apply it under
/// that fence — `unchanged` is intake's verdict on the evidence itself,
/// never a stamp the host guesses at. `Ok(Some(false))` for an unchanged
/// apply. `Ok(None)` when no ready connector enumerated this session.
/// Selected capture downloads only through here, so its outcome is kept for
/// Settings' sync health ([`note_download`]).
pub fn hydrate(
    store: &SessionStore,
    home: &Path,
    source: &str,
    session_id: &str,
    stopped: impl Fn() -> bool,
) -> Result<Option<bool>> {
    let host = Host::system(home, store.db_path());
    let ready = ready(&host, Some(store.db_path()));
    let result = hydrate_with(
        &Pass::new(&host),
        &ready,
        store,
        source,
        session_id,
        &stopped,
    );
    if !stopped() {
        super::sweep::note_download(store.db_path(), &ready, source, session_id, &result);
    }
    result
}

pub(super) fn hydrate_with(
    pass: &Pass,
    ready: &[(&'static str, &'static str)],
    store: &SessionStore,
    source: &str,
    session_id: &str,
    stopped: impl Fn() -> bool,
) -> Result<Option<bool>> {
    for (connector, connector_source) in ready
        .iter()
        .filter(|(_, connector_source)| *connector_source == source)
    {
        let key = key(connector, connector_source, session_id);
        let state = store.source_observation(&key)?;
        let Some(observation) = state.observation else {
            continue;
        };
        if stopped() {
            return Ok(Some(false));
        }
        // The observation and checkpoint stamps live in different
        // namespaces — the provider's listing digest versus the evidence
        // hash — so they cannot be compared directly. Their `updated_ms`
        // can be: the observation's bumps only when the guarded upsert sees
        // a material change, the checkpoint's on every apply. An
        // observation no newer than the checkpoint cannot hold anything
        // the apply didn't already see — skip before the API round trip.
        // Equal milliseconds is the common case: the apply writes the
        // observation row before the checkpoint in one transaction.
        if let Some(checkpoint) = &state.checkpoint {
            if observation.updated_ms <= checkpoint.updated_ms {
                return Ok(Some(false));
            }
        }
        let revision = state
            .revision
            .context("remote observation carries no revision fence")?;
        let result = acquire_apply(pass, store, &key, observation, revision, connector)?;
        if stopped() {
            return Ok(Some(false));
        }
        return Ok(Some(result.status != "unchanged"));
    }
    Ok(None)
}

/// Hydrate a remote session into `store` when `store` is not the catalog
/// that observed it — the transcript reader's private cache, which never
/// runs remote enumeration. A missing observation row is landed as a
/// placeholder once the connector has served the session, so the evidence
/// still applies under a revision fence.
/// `Ok(None)` when no ready connector serves this source. A person opening
/// the session asked for it, so a connector held back since its last
/// listing failed is still tried.
pub fn hydrate_reader(
    store: &SessionStore,
    source: &str,
    session_id: &str,
    stopped: impl Fn() -> bool,
) -> Result<Option<bool>> {
    let home = store.roots().home.clone();
    let host = Host::system(&home, store.db_path());
    hydrate_reader_with(&host, store, source, session_id, stopped)
}

/// Whether `connector` can own `session_id` at all. Cursor Cloud Agents are
/// `bc-` ids and claude.ai lists only `session_` and `cse_` ids; a local
/// transcript's UUID must never become a remote placeholder.
fn names_session(connector: &str, session_id: &str) -> bool {
    match connector {
        CURSOR_CLOUD_CONNECTOR => session_id.starts_with("bc-"),
        CLAUDE_WEB_CONNECTOR => ["session_", "cse_"]
            .iter()
            .any(|prefix| session_id.starts_with(prefix)),
        _ => true,
    }
}

pub(super) fn hydrate_reader_with(
    host: &Host,
    store: &SessionStore,
    source: &str,
    session_id: &str,
    stopped: impl Fn() -> bool,
) -> Result<Option<bool>> {
    let Some((connector, connector_source)) =
        ready(host, None)
            .into_iter()
            .find(|(connector, connector_source)| {
                *connector_source == source && names_session(connector, session_id)
            })
    else {
        return Ok(None);
    };
    let key = key(connector, connector_source, session_id);
    let state = store.source_observation(&key)?;
    if stopped() {
        return Ok(Some(false));
    }
    let pass = Pass::asked(host);
    match (state.observation, state.revision) {
        (Some(observation), Some(revision)) => {
            acquire_apply(&pass, store, &key, observation, revision, connector)?;
        }
        _ => {
            // Fetch before landing anything: a placeholder for a session the
            // connector cannot serve would hold it remote-only in the
            // reader's cache, which then never runs local discovery for it.
            let evidence = pass.acquire(connector, placeholder(&key))?;
            store.apply_source_observations(SourceObservationBatch::new(
                connector,
                INSTANCE,
                SessionLocation::Remote,
                vec![SourceObservation {
                    session: ai_hist::ShallowSession {
                        source: connector_source.to_string(),
                        session_id: session_id.to_string(),
                        ..Default::default()
                    },
                    raw_locator: None,
                }],
            ))?;
            let revision = store
                .source_observation(&key)?
                .revision
                .context("remote observation carries no revision fence")?;
            apply_fenced(&pass, store, &key, evidence, revision, connector)?;
        }
    }
    Ok(Some(true))
}

/// What the placeholder observation will hold, for the fetch that comes
/// before it lands.
fn placeholder(key: &ObservationKey) -> ai_hist::SessionObservation {
    ai_hist::SessionObservation {
        key: key.clone(),
        raw_locator: None,
        source_stamp: None,
        discovery_state: "shallow".into(),
        access_state: "available".into(),
        updated_ms: 0,
        first_prompt: None,
        last_assistant_text: None,
    }
}

impl Pass<'_, '_> {
    /// `evidence` through the pass's scrub engine before it is stored. A
    /// refusal is the hydration's failure, recorded like any other connector
    /// failure: nothing is applied, and the next attempt fetches and scrubs
    /// afresh rather than applying the raw snapshot.
    pub(super) fn scrubbed(
        &self,
        connector: &str,
        session_id: &str,
        mut evidence: NormalizedSourceEvidence,
    ) -> Result<NormalizedSourceEvidence> {
        let context = ScrubContext::new(ScrubSurface::LocalStore)
            .source(connector)
            .session(Some(session_id));
        // The evidence envelope is the intake fence: its source stamp,
        // covered kinds and record ids must remain byte-for-byte the
        // connector's. Inside a payload, preserve only the schema's exact
        // identity paths. Everything else is content and is scrubbed as a
        // whole value, including nested objects with identity-looking keys.
        // Unknown future columns therefore fail closed into content instead
        // of silently bypassing the scrubber.
        for record in &mut evidence.records {
            for (name, value) in &mut record.payload {
                if !payload_identity(record.kind, name) && !value.is_null() {
                    *value = self.scrub.scrub_json(value, &context)?;
                }
            }
        }
        Ok(evidence)
    }
}

/// Exact top-level payload paths whose bytes establish canonical identity or
/// carry a closed classification. This is deliberately per evidence kind:
/// a nested content object named `id`, or an unknown future top-level column,
/// is still scrubbed in full.
fn payload_identity(kind: ai_hist::EvidenceKind, name: &str) -> bool {
    use ai_hist::EvidenceKind::*;
    match kind {
        History => matches!(
            name,
            "source" | "session_id" | "prompt_hash" | "timestamp_ms"
        ),
        SessionEvent => matches!(
            name,
            "source"
                | "session_id"
                | "project_key"
                | "project_key_method"
                | "message_id"
                | "parent_id"
                | "ts_ms"
                | "role"
                | "kind"
                | "provider"
                | "event_uid"
                | "tool_use_id"
                | "payload_bytes"
                | "payload_truncated"
                | "payload_hash"
                | "call_index"
                | "event_index"
                | "result_status"
                | "event_source"
                | "error_signal"
                | "subagent_session_id"
                | "agent_id"
                | "request_id"
                | "provider_message_id"
                | "stop_reason"
                | "is_sidechain"
                | "is_meta"
                | "turn_id"
                | "request_span"
                | "raw_kind"
                | "control_kind"
        ),
        ToolCall => matches!(
            name,
            "source" | "session_id" | "message_id" | "tool_use_id" | "name" | "is_error" | "ts_ms"
        ),
        FileEdit => matches!(
            name,
            "source"
                | "session_id"
                | "message_id"
                | "tool_use_id"
                | "tool_name"
                | "lines_added"
                | "lines_removed"
                | "user_modified"
                | "ts_ms"
        ),
        Relationship => matches!(
            name,
            "source"
                | "parent_session_id"
                | "relationship_uid"
                | "child_session_id"
                | "relationship"
                | "identity_status"
                | "child_agent_type"
                | "spawn_depth"
                | "evidence_kind"
                | "child_has_events"
                | "spawned_at_ms"
                | "created_ms"
                | "updated_ms"
                | "origin_session_id"
        ),
        CommitLink => matches!(
            name,
            "source"
                | "session_id"
                | "commit_sha"
                | "match_method"
                | "confidence"
                | "created_at_ms"
        ),
        SessionMarker => matches!(
            name,
            "source"
                | "session_id"
                | "marker_uid"
                | "ts_ms"
                | "message_id"
                | "parent_id"
                | "turn_id"
                | "kind"
                | "subkind"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(session_id: &str, text: &str) -> NormalizedSourceEvidence {
        let mut payload = serde_json::Map::new();
        payload.insert("source".into(), "devin".into());
        payload.insert("session_id".into(), session_id.into());
        payload.insert("ts_ms".into(), 1.into());
        payload.insert("role".into(), "user".into());
        payload.insert("kind".into(), "text".into());
        payload.insert("event_uid".into(), "event-1".into());
        payload.insert("text".into(), text.into());
        NormalizedSourceEvidence {
            source_stamp: "cloud:100:active:aa".into(),
            source_bytes: 1,
            covered_kinds: vec![ai_hist::EvidenceKind::SessionEvent],
            records: vec![ai_hist::EvidenceRecord {
                kind: ai_hist::EvidenceKind::SessionEvent,
                payload,
                record_id: None,
                revision_id: None,
            }],
        }
    }

    #[test]
    fn remote_evidence_reaches_the_store_only_as_the_engine_returned_it() {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("history.db");
        let host = Host::system(home.path(), &db);
        let mut pass = Pass::new(&host);
        pass.scrub = crate::history::scrub::testing::rewrite("oops", "clean");
        let scrubbed = pass
            .scrubbed("connector", "s1", evidence("s1", "oops"))
            .unwrap();
        let text = serde_json::to_string(&scrubbed).unwrap();
        assert!(text.contains("clean") && !text.contains("oops"), "{text}");

        (pass.scrub, _) = crate::history::scrub::testing::refuse();
        let error = pass
            .scrubbed("connector", "s1", evidence("s1", "oops"))
            .unwrap_err();
        assert!(!error.to_string().contains("oops"));
    }

    #[test]
    fn scrubbing_keeps_remote_identity_so_the_evidence_still_applies() {
        const SESSION: &str = "bc-00000000-0000-0000-0000-0000000000aa";
        const FAKE_KEY: &str = "sk-proj-ABCDEFGHIJKLMNOPQRSTUVWX1234567890abcd";

        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("history.db");
        let store = super::super::super::history::open(&db).unwrap();
        let observation = SourceObservation {
            session: ai_hist::ShallowSession {
                source: "devin".into(),
                session_id: SESSION.into(),
                source_stamp: Some("cloud:100:active:aa".into()),
                ..Default::default()
            },
            raw_locator: Some(format!("v3:sessions/{SESSION}")),
        };
        store
            .apply_source_observations(SourceObservationBatch::new(
                "connector",
                INSTANCE,
                SessionLocation::Remote,
                vec![observation],
            ))
            .unwrap();
        let key = key("connector", "devin", SESSION);
        let revision = store.source_observation(&key).unwrap().revision.unwrap();
        let host = Host::system(home.path(), &db);
        let pass = Pass::new(&host);
        let original = evidence(SESSION, &format!("OPENAI_API_KEY={FAKE_KEY}"));
        let scrubbed = pass
            .scrubbed("connector", SESSION, original.clone())
            .unwrap();

        assert_eq!(scrubbed.source_stamp, original.source_stamp);
        assert_eq!(scrubbed.covered_kinds, original.covered_kinds);
        assert_eq!(scrubbed.records[0].kind, original.records[0].kind);
        assert_eq!(scrubbed.records[0].record_id, original.records[0].record_id);
        assert_eq!(
            scrubbed.records[0].revision_id,
            original.records[0].revision_id
        );
        assert_eq!(scrubbed.records[0].payload["session_id"], SESSION);
        let text = scrubbed.records[0].payload["text"].as_str().unwrap();
        assert!(!text.contains(FAKE_KEY), "{text}");
        assert!(text.contains("[REDACTED]"), "{text}");
        store
            .apply_source_evidence(SourceEvidenceSubmission::new(key, revision, scrubbed))
            .unwrap();
    }

    #[test]
    fn scrubbing_covers_non_allowlisted_commit_and_relationship_content() {
        const SESSION: &str = "bc-00000000-0000-0000-0000-0000000000bb";
        const FAKE_TOKEN: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";

        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("history.db");
        let store = super::super::super::history::open(&db).unwrap();
        let observation = SourceObservation {
            session: ai_hist::ShallowSession {
                source: "devin".into(),
                session_id: SESSION.into(),
                source_stamp: Some("cloud:200:active:bb".into()),
                ..Default::default()
            },
            raw_locator: Some(format!("v3:sessions/{SESSION}")),
        };
        store
            .apply_source_observations(SourceObservationBatch::new(
                "connector",
                INSTANCE,
                SessionLocation::Remote,
                vec![observation],
            ))
            .unwrap();
        let key = key("connector", "devin", SESSION);
        let revision = store.source_observation(&key).unwrap().revision.unwrap();
        let record = |kind, payload: serde_json::Value, id: &str| ai_hist::EvidenceRecord {
            kind,
            payload: payload.as_object().unwrap().clone(),
            record_id: Some(id.into()),
            revision_id: Some("revision-1".into()),
        };
        let evidence = NormalizedSourceEvidence {
            source_stamp: "cloud:200:active:bb".into(),
            source_bytes: 2,
            covered_kinds: vec![
                ai_hist::EvidenceKind::CommitLink,
                ai_hist::EvidenceKind::Relationship,
            ],
            records: vec![
                record(
                    ai_hist::EvidenceKind::CommitLink,
                    serde_json::json!({
                        "source": "devin",
                        "session_id": SESSION,
                        "repo": format!("https://{FAKE_TOKEN}@github.com/x/y"),
                        "commit_sha": "abcdef",
                        "match_method": "explicit",
                        "confidence": 1.0,
                        "created_at_ms": 1,
                    }),
                    "commit-record",
                ),
                record(
                    ai_hist::EvidenceKind::Relationship,
                    serde_json::json!({
                        "source": "devin",
                        "parent_session_id": SESSION,
                        "relationship_uid": "child-1",
                        "child_session_id": "bc-00000000-0000-0000-0000-0000000000cc",
                        "relationship": "delegated",
                        "identity_status": "observed",
                        "evidence_kind": "transcript",
                        "evidence_locator": format!("https://example.test/{FAKE_TOKEN}/child"),
                        "created_ms": 1,
                        "updated_ms": 1,
                    }),
                    "relationship-record",
                ),
            ],
        };
        let host = Host::system(home.path(), &db);
        let scrubbed = Pass::new(&host)
            .scrubbed("connector", SESSION, evidence)
            .unwrap();

        let repo = scrubbed.records[0].payload["repo"].as_str().unwrap();
        let locator = scrubbed.records[1].payload["evidence_locator"]
            .as_str()
            .unwrap();
        for content in [repo, locator] {
            assert!(!content.contains(FAKE_TOKEN), "{content}");
            assert!(content.contains("[REDACTED]"), "{content}");
        }
        store
            .apply_source_evidence(SourceEvidenceSubmission::new(key, revision, scrubbed))
            .unwrap();
    }

    #[test]
    fn a_pass_scrubs_with_the_default_engine() {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("history.db");
        let host = Host::system(home.path(), &db);
        assert_eq!(
            Pass::new(&host).scrub.name(),
            crate::history::scrub::DEFAULT_ENGINE
        );
    }

    #[test]
    fn a_connector_names_only_the_ids_it_lists() {
        let uuid = "9f764b2e-62a7-4657-bdf4-9eeecd269742";
        assert!(names_session(CLAUDE_WEB_CONNECTOR, "cse_01A"));
        assert!(names_session(CLAUDE_WEB_CONNECTOR, "session_01abc"));
        assert!(!names_session(CLAUDE_WEB_CONNECTOR, uuid));
        assert!(names_session(CURSOR_CLOUD_CONNECTOR, "bc-1"));
        assert!(!names_session(CURSOR_CLOUD_CONNECTOR, uuid));
        assert!(names_session(DEVIN_CLOUD_CONNECTOR, uuid));
    }
}
