use super::*;
use crate::history::scrub::{Scrub, ScrubContext, ScrubEngine, ScrubError};

struct FixtureEngine;
impl Scrub for FixtureEngine {
    fn name(&self) -> &str {
        "fixture-v1"
    }
    fn scrub(&self, body: &[u8], context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
        if context.session_id == Some(STUB_SESSION) && body == br#""found none""# {
            panic!("synthetic scrub failure");
        }
        Ok(body.to_vec())
    }
}

#[test]
fn remote_scrub_panic_preserves_snapshot_and_retries_after_engine_change() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let api = StubApi::start();
    let (home, _env) = signed_in_home(&api.url());
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("history.db");
    let store = crate::history::probe::history::open(&db).unwrap();
    let host = Host::system(home.path(), &db);
    let mut pass = Pass::new(&host);
    pass.scrub = crate::history::scrub::testing::rewrite("found none", "previous snapshot");
    sweep_pass(&pass, &db, &store, || false).unwrap().unwrap();
    let previous = stub_texts(&db);
    assert!(previous.contains("previous snapshot"));
    let key = key(DEVIN_CLOUD_CONNECTOR, "devin", STUB_SESSION);
    let checkpoint = || {
        store
            .source_observation(&key)
            .unwrap()
            .checkpoint
            .unwrap()
            .updated_ms
    };
    let before = checkpoint();
    api.bump_updated_at();
    backdate(&db);
    enumerate_with(&host, &db, || false).unwrap().unwrap();
    let sibling = "ffffffffffffffffffffffffffffffff";
    store
        .apply_source_observations(SourceObservationBatch::new(
            DEVIN_CLOUD_CONNECTOR,
            INSTANCE,
            SessionLocation::Remote,
            vec![observation(sibling)],
        ))
        .unwrap();
    pass.scrub = ScrubEngine::new(FixtureEngine);
    let outcome = sweep_pass(&pass, &db, &store, || false).unwrap().unwrap();
    assert_eq!((outcome.failed, outcome.hydrated), (1, 1));
    assert_eq!(checkpoint(), before);
    assert_eq!(stub_texts(&db), previous);
    assert!(event_count(&db, sibling) >= 2);
    let fetches = api.fetches();
    sweep_pass(&pass, &db, &store, || false).unwrap().unwrap();
    assert_eq!(api.fetches(), fetches);
    pass.scrub = crate::history::scrub::testing::rewrite("found none", "updated snapshot");
    let outcome = sweep_pass(&pass, &db, &store, || false).unwrap().unwrap();
    assert_eq!((outcome.failed, outcome.hydrated), (0, 1));
    assert!(stub_texts(&db).contains("updated snapshot"));
}
