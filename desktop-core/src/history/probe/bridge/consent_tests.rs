//! Consent is checked against the records a delivery worker can actually claim.
use super::tests::{apply, fixture};
use super::*;

fn sent(conn: &Connection, job: &str) -> HashSet<String> {
    let mut sessions = HashSet::new();
    for _ in 0..100 {
        let now = collector::now();
        let prepared = delivery::prepare_batch(conn, job, now).unwrap();
        if prepared.batch_id.is_none() {
            if prepared.bootstrap_complete && prepared.scanned_records == 0 {
                return sessions;
            }
            continue;
        }
        let Some(claim) =
            delivery::claim_batch(conn, job, "consent-test", 30_000, &|| now).unwrap()
        else {
            continue;
        };
        sessions.extend(
            claim
                .batch
                .records
                .iter()
                .filter_map(|r| r.session_id.clone()),
        );
        delivery::store_prepared_payload(
            conn,
            &claim.lease,
            &claim.batch.mapping_version,
            "application/json",
            "{}",
            &|| now,
        )
        .unwrap();
        delivery::acknowledge(
            conn,
            &claim.lease,
            &delivery::DeliveryAcknowledgment {
                batch_id: claim.batch.batch_id.clone(),
                accepted_revision_ids: claim
                    .batch
                    .records
                    .iter()
                    .map(|r| r.revision_id.clone())
                    .collect(),
                unsupported_revision_ids: vec![],
                suppressed_revision_ids: vec![],
                rejected_revision_ids: vec![],
                isolation_frontier: vec![],
                acceptance_level: delivery::AcceptanceLevel::Durable,
            },
            &|| now,
        )
        .unwrap();
    }
    panic!("bounded consent fixture did not drain");
}

#[test]
fn all_removes_automatic_baselines_but_keeps_user_denials_after_reopen() {
    let (dir, config) = fixture(SharingMode::New);
    let config = apply(dir.path(), config, None, &["claude:old"], false);
    let config = apply(dir.path(), config, Some(SharingMode::All), &[], true);
    let conn = db(dir.path()).unwrap();
    assert_eq!(
        delivery::user_exclusions(&conn).unwrap(),
        vec![parse_key("claude:old").unwrap()]
    );
    assert_eq!(
        sent(&conn, &config.job_id),
        HashSet::from(["recent".into()])
    );
    drop(conn);
    let reopened = db(dir.path()).unwrap();
    assert!(
        delivery::is_explicitly_excluded(&reopened, &parse_key("claude:old").unwrap()).unwrap()
    );
    assert!(
        !delivery::is_explicitly_excluded(&reopened, &parse_key("codex:recent").unwrap()).unwrap()
    );
    drop(reopened);
    // A later explicit include is the operation that lifts this user's deny.
    let config = apply(dir.path(), config, None, &["claude:old"], true);
    let conn = db(dir.path()).unwrap();
    assert!(delivery::user_exclusions(&conn).unwrap().is_empty());
    assert_eq!(
        sent(&conn, &config.job_id),
        HashSet::from(["old".into(), "recent".into()])
    );
}

#[test]
fn selected_uploads_only_chosen_sessions_and_new_changes_to_them() {
    let (dir, config) = fixture(SharingMode::Selected);
    let config = apply(dir.path(), config, None, &["codex:recent"], true);
    let conn = db(dir.path()).unwrap();
    assert_eq!(
        sent(&conn, &config.job_id),
        HashSet::from(["recent".into()])
    );
    delivery::test_evidence(&conn)
        .execute(
            "INSERT INTO sessions(source,session_id) VALUES('codex','unselected-new')",
            [],
        )
        .unwrap();
    delivery::test_evidence(&conn).execute("INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES('codex','recent','followup',30,'user','text','chosen synthetic followup'),('claude','old','private-followup',30,'user','text','unselected synthetic followup')", []).unwrap();
    assert_eq!(
        sent(&conn, &config.job_id),
        HashSet::from(["recent".into()])
    );
}

#[test]
fn new_uploads_future_sessions_but_not_existing_unselected_sessions() {
    let (dir, config) = fixture(SharingMode::New);
    let conn = db(dir.path()).unwrap();
    assert!(sent(&conn, &config.job_id).is_empty());
    delivery::test_evidence(&conn)
        .execute(
            "INSERT INTO sessions(source,session_id) VALUES('codex','future')",
            [],
        )
        .unwrap();
    assert_eq!(
        sent(&conn, &config.job_id),
        HashSet::from(["future".into()])
    );
    drop(conn);
    let config = apply(dir.path(), config, None, &["claude:old"], true);
    let conn = db(dir.path()).unwrap();
    let accepted = sent(&conn, &config.job_id);
    assert!(accepted.contains("old"));
    assert!(!accepted.contains("recent"));
}

#[test]
fn narrowing_to_selected_invalidates_an_already_prepared_all_batch() {
    let (dir, config) = fixture(SharingMode::All);
    let conn = db(dir.path()).unwrap();
    let now = collector::now();
    delivery::prepare_batch(&conn, &config.job_id, now).unwrap();
    let claim = delivery::claim_batch(&conn, &config.job_id, "old-worker", 30_000, &|| now)
        .unwrap()
        .unwrap();
    assert!(claim
        .batch
        .records
        .iter()
        .any(|r| r.session_id.as_deref() == Some("old")));
    drop(conn);
    let config = apply(dir.path(), config, Some(SharingMode::Selected), &[], true);
    let conn = db(dir.path()).unwrap();
    assert!(delivery::store_prepared_payload(
        &conn,
        &claim.lease,
        &claim.batch.mapping_version,
        "application/json",
        "{}",
        &|| now
    )
    .is_err());
    assert!(sent(&conn, &config.job_id).is_empty());
}

#[test]
fn interrupted_all_change_preserves_a_denial_on_every_replay() {
    let (dir, config) = fixture(SharingMode::Selected);
    let config = apply(dir.path(), config, None, &["claude:old"], false);
    let conn = db(dir.path()).unwrap();
    let plan = make_plan(&conn, dir.path(), config, Some(SharingMode::All), &[], true).unwrap();
    for _ in 0..2 {
        save_json(&dir.path().join("sharing-change.json"), &plan).unwrap();
        recover(dir.path()).unwrap();
        assert!(
            delivery::is_explicitly_excluded(&conn, &parse_key("claude:old").unwrap()).unwrap()
        );
        let config = read_config(dir.path()).unwrap();
        assert_eq!(
            sent(&conn, &config.job_id),
            HashSet::from(["recent".into()])
        );
    }
}

#[test]
fn an_old_unclassified_exclusion_is_not_reinterpreted_as_upload_consent() {
    let (dir, config) = fixture(SharingMode::All);
    let conn = db(dir.path()).unwrap();
    conn.execute_batch("ALTER TABLE delivery_exclusions DROP COLUMN explicit_deny; INSERT INTO delivery_exclusions(source,session_id) VALUES('claude','old');").unwrap();
    drop(conn);
    let conn = db(dir.path()).unwrap();
    assert_eq!(
        delivery::user_exclusions(&conn).unwrap(),
        vec![parse_key("claude:old").unwrap()]
    );
    drop(conn);
    let config = apply(dir.path(), config, Some(SharingMode::All), &[], true);
    let conn = db(dir.path()).unwrap();
    assert_eq!(
        sent(&conn, &config.job_id),
        HashSet::from(["recent".into()])
    );
}

#[test]
fn replay_of_an_older_all_intent_cannot_clear_a_persisted_deny() {
    let (dir, config) = fixture(SharingMode::All);
    let config = apply(dir.path(), config, None, &["claude:old"], false);
    let conn = db(dir.path()).unwrap();
    let mut plan = make_plan(&conn, dir.path(), config, Some(SharingMode::All), &[], true).unwrap();
    plan.user_excluded = None;
    plan.excluded.clear();
    save_json(&dir.path().join("sharing-change.json"), &plan).unwrap();
    recover(dir.path()).unwrap();
    let config = read_config(dir.path()).unwrap();
    assert_eq!(
        sent(&conn, &config.job_id),
        HashSet::from(["recent".into()])
    );
}
