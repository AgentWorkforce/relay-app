use super::*;
use serde_json::json;

fn session(kind: &str, id: &str, registration: Option<(&str, Option<&str>)>) -> LocalSession {
    let registration = registration.map(|(name, alias)| {
        json!({"name": name, "alias": alias, "address": format!("{name}@computer"), "added_by_session": false, "push": null})
    });
    serde_json::from_value(json!({"target": {"kind": kind, "id": id}, "title": "Local work", "live": true, "project": null, "registration": registration, "suggested_name": "session", "updated_at_ms": 200})).unwrap()
}

fn everywhere(_: &RelayTarget) -> bool {
    true
}

fn names(pairs: &[(&str, &str)]) -> Names {
    pairs
        .iter()
        .map(|(key, name)| (key.to_string(), name.to_string()))
        .collect()
}

#[test]
fn a_registered_codex_thread_is_named_under_its_native_codex_id() {
    let named = names_for(
        &[session("codex", "019a-thread", Some(("reviewer", None)))],
        everywhere,
    );
    assert_eq!(named, names(&[("codex:019a-thread", "reviewer")]));
}

#[test]
fn other_registered_kinds_are_named_under_the_sessions_upload_key() {
    let named = names_for(
        &[
            session("claude-terminal", "c1", Some(("lead", None))),
            session("grok-open", "g1", Some(("grokker", None))),
            session("opencode-open", "o1", Some(("coder", None))),
        ],
        everywhere,
    );
    assert_eq!(
        named,
        names(&[
            ("claude:c1", "lead"),
            ("grok:g1", "grokker"),
            ("opencode:o1", "coder"),
        ])
    );
}

#[test]
fn a_renamed_agent_is_recorded_by_the_alias_it_is_shown_by() {
    let named = names_for(
        &[session("codex", "t1", Some(("codex-1", Some("reviewer"))))],
        everywhere,
    );
    assert_eq!(named, names(&[("codex:t1", "reviewer")]));
}

#[test]
fn unregistered_sessions_and_kinds_history_does_not_capture_produce_no_entry() {
    let named = names_for(
        &[
            session("codex", "off-the-relay", None),
            session("openclaw", "agent", Some(("claw", None))),
            session("claude-cloud", "cse_1", Some(("cloudy", None))),
        ],
        everywhere,
    );
    assert!(named.is_empty());
}

#[test]
fn a_registration_outside_the_upload_workspace_produces_no_entry() {
    let theirs = session("codex", "theirs", Some(("elsewhere", None)));
    let ours = session("codex", "ours", Some(("here", None)));
    let named = names_for(&[theirs.clone(), ours], |target| target != &theirs.target);
    assert_eq!(named, names(&[("codex:ours", "here")]));
}

#[test]
fn a_live_window_keeps_its_name_over_a_stopped_managed_session_with_its_key() {
    let mut stopped = session("grok", "s1", Some(("managed", None)));
    stopped.live = false;
    let live = session("grok-open", "s1", Some(("window", None)));
    for sessions in [
        vec![stopped.clone(), live.clone()],
        vec![live.clone(), stopped.clone()],
    ] {
        assert_eq!(
            names_for(&sessions, everywhere),
            names(&[("grok:s1", "window")])
        );
    }
}

#[test]
fn published_keys_resolve_to_their_history_sessions_and_unknown_sources_are_left_out() {
    let resolved = resolve(&names(&[
        ("codex:t1", "reviewer"),
        ("claude:c1", "lead"),
        ("not-a-source:x", "nobody"),
        ("codex:", "empty"),
        ("no-separator", "none"),
    ]));
    assert_eq!(
        resolved,
        vec![
            (SessionKey::new(Source::Claude, "c1"), "lead".to_string()),
            (SessionKey::new(Source::Codex, "t1"), "reviewer".to_string()),
        ]
    );
}

/// Records every name it is offered, and places each as `placed` says.
struct Observer {
    calls: std::sync::Arc<Mutex<Vec<(SessionKey, String)>>>,
    placed: Placed,
}
impl NameSink for Observer {
    fn set_registered_agent_name(
        &mut self,
        source: Source,
        session_id: &str,
        name: Option<&str>,
    ) -> Result<Placed> {
        self.calls.lock().unwrap().push((
            SessionKey::new(source, session_id),
            name.expect("names are set, never cleared").to_owned(),
        ));
        Ok(self.placed)
    }
}

/// One pass over `directory` with an observer placing every name as
/// `placed`: what it applied, and whether it opened the store at all.
fn pass(
    memo: &mut Memo,
    directory: &Path,
    calls: &std::sync::Arc<Mutex<Vec<(SessionKey, String)>>>,
    placed: Placed,
) -> (Option<Applied>, bool) {
    let mut opened = false;
    let applied = before_delivery_with(memo, directory, |db| {
        opened = true;
        assert_eq!(db, directory.join("history.db"));
        Ok(Box::new(Observer {
            calls: calls.clone(),
            placed,
        }))
    });
    (applied, opened)
}

#[test]
fn an_idle_pass_with_unchanged_names_neither_opens_the_store_nor_offers_a_name() {
    let dir = tempfile::tempdir().unwrap();
    let calls = std::sync::Arc::new(Mutex::new(Vec::new()));
    let mut memo = Memo::default();
    // Nothing published: nothing to do.
    assert_eq!(
        pass(&mut memo, dir.path(), &calls, Placed::Recorded),
        (None, false)
    );
    write_published(dir.path(), &names(&[("codex:t1", "reviewer")])).unwrap();
    let (applied, opened) = pass(&mut memo, dir.path(), &calls, Placed::Recorded);
    assert!(opened);
    assert_eq!(
        applied,
        Some(Applied {
            offered: 1,
            recorded: 1,
            not_in_history_yet: 0
        })
    );
    assert_eq!(
        pass(&mut memo, dir.path(), &calls, Placed::Recorded),
        (None, false)
    );
    // A rename is a change: the new name is offered once.
    write_published(dir.path(), &names(&[("codex:t1", "renamed")])).unwrap();
    assert!(pass(&mut memo, dir.path(), &calls, Placed::Recorded).1);
    assert_eq!(
        pass(&mut memo, dir.path(), &calls, Placed::Recorded),
        (None, false)
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            (SessionKey::new(Source::Codex, "t1"), "reviewer".to_string()),
            (SessionKey::new(Source::Codex, "t1"), "renamed".to_string()),
        ]
    );
}

#[test]
fn a_name_whose_session_history_has_not_captured_yet_is_offered_again_next_pass() {
    let dir = tempfile::tempdir().unwrap();
    let calls = std::sync::Arc::new(Mutex::new(Vec::new()));
    let mut memo = Memo::default();
    write_published(dir.path(), &names(&[("codex:t1", "reviewer")])).unwrap();
    assert!(pass(&mut memo, dir.path(), &calls, Placed::NotInHistoryYet).1);
    assert!(pass(&mut memo, dir.path(), &calls, Placed::Recorded).1);
    assert!(!pass(&mut memo, dir.path(), &calls, Placed::Recorded).1);
    assert_eq!(calls.lock().unwrap().len(), 2);
}

/// A store holding the sessions in `captured`: records each name offered for
/// one of them, and leaves any other for a later pass.
struct Captured {
    captured: Vec<SessionKey>,
    held: std::sync::Arc<Mutex<HashMap<SessionKey, String>>>,
}
impl NameSink for Captured {
    fn set_registered_agent_name(
        &mut self,
        source: Source,
        session_id: &str,
        name: Option<&str>,
    ) -> Result<Placed> {
        let session = SessionKey::new(source, session_id);
        if !self.captured.contains(&session) {
            return Ok(Placed::NotInHistoryYet);
        }
        let name = name.expect("names are set, never cleared").to_owned();
        self.held.lock().unwrap().insert(session, name);
        Ok(Placed::Recorded)
    }
}

#[test]
fn names_returning_to_the_last_applied_map_after_a_partial_apply_are_applied_again() {
    let dir = tempfile::tempdir().unwrap();
    let held = std::sync::Arc::new(Mutex::new(HashMap::new()));
    let a = SessionKey::new(Source::Codex, "a");
    let mut memo = Memo::default();
    let next_pass = |memo: &mut Memo| {
        before_delivery_with(memo, dir.path(), |_| {
            Ok(Box::new(Captured {
                captured: vec![a.clone()],
                held: held.clone(),
            }))
        })
    };
    write_published(dir.path(), &names(&[("codex:a", "old")])).unwrap();
    assert_eq!(
        next_pass(&mut memo).map(|applied| applied.recorded),
        Some(1)
    );
    // `a` is renamed while `b`, not captured yet, is registered: `a` takes the
    // new name and `b` is left for a later pass.
    write_published(
        dir.path(),
        &names(&[("codex:a", "new"), ("codex:b", "temp")]),
    )
    .unwrap();
    assert_eq!(
        next_pass(&mut memo).map(|applied| applied.not_in_history_yet),
        Some(1)
    );
    assert_eq!(held.lock().unwrap()[&a], "new");
    // `a` is renamed back and `b` leaves: the first map again, applied again.
    write_published(dir.path(), &names(&[("codex:a", "old")])).unwrap();
    assert_eq!(
        next_pass(&mut memo).map(|applied| applied.recorded),
        Some(1)
    );
    assert_eq!(held.lock().unwrap()[&a], "old");
    assert_eq!(next_pass(&mut memo), None);
}

#[test]
fn the_digest_does_not_depend_on_the_order_names_arrive_in() {
    let a = (SessionKey::new(Source::Codex, "t1"), "one".to_string());
    let b = (SessionKey::new(Source::Claude, "c1"), "two".to_string());
    assert_eq!(
        digest(&[a.clone(), b.clone()]),
        digest(&[b.clone(), a.clone()])
    );
    assert_ne!(
        digest(&[a.clone()]),
        digest(&[(a.0.clone(), "renamed".to_string())])
    );
}

#[test]
fn names_are_published_only_when_they_change() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!write_published(dir.path(), &Names::new()).unwrap());
    assert!(!dir.path().join(FILE).exists());
    let named = names(&[("codex:t1", "reviewer")]);
    assert!(write_published(dir.path(), &named).unwrap());
    assert!(!write_published(dir.path(), &named).unwrap());
    assert_eq!(published(dir.path()), Some(named));
    // An agent leaving the relay is published too.
    assert!(write_published(dir.path(), &Names::new()).unwrap());
    assert_eq!(published(dir.path()), Some(Names::new()));
}

#[test]
fn a_name_is_recorded_on_a_session_the_store_holds_and_left_for_later_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("history.db");
    let mut store = SessionStore::open(ai_hist::StoreOptions::default().db_path(&db)).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    crate::history::delivery::test_evidence(&conn)
        .execute(
            "INSERT INTO sessions (session_id, source) VALUES ('t1', 'codex')",
            [],
        )
        .unwrap();
    let applied = apply(
        &mut store,
        &[
            (SessionKey::new(Source::Codex, "t1"), "reviewer".to_string()),
            (SessionKey::new(Source::Claude, "c1"), "lead".to_string()),
        ],
    )
    .unwrap();
    assert_eq!(
        applied,
        Applied {
            offered: 2,
            recorded: 1,
            not_in_history_yet: 1
        }
    );
}

#[test]
fn a_name_the_history_store_would_refuse_is_never_offered_to_it() {
    let too_long = "n".repeat(NAME_LIMIT + 1);
    let longest = "n".repeat(NAME_LIMIT);
    for refused in [
        "",
        "   ",
        too_long.as_str(),
        "line\nbreak",
        "tab\there",
        "bell\u{7}",
    ] {
        assert!(!valid_name(refused), "{refused:?}");
        assert!(names_for(&[session("codex", "t1", Some((refused, None)))], everywhere).is_empty());
        assert!(resolve(&names(&[("codex:t1", refused)])).is_empty());
    }
    // A refused alias is not replaced by the registered name: the agent is
    // shown by its alias, so history is not told another name.
    assert!(names_for(
        &[session("codex", "t1", Some(("ok", Some("bad\nalias"))))],
        everywhere
    )
    .is_empty());
    assert!(valid_name(&longest));
    assert!(valid_name("reviewer-2 (desk)"));
}
