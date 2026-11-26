//! Pending resumes the apps wrote before this engine still block a
//! duplicate launch, and the recorded provider store is honoured.
use std::collections::BTreeMap;

use session_navigation::evidence::{Fact, Pending};
use session_navigation::model::{Host, TerminalKind};
use session_navigation::plan::{Decision, Plan, SessionState};
use session_navigation::ports::{Observe, Reservations};
use session_navigation_native::system::join;
use session_navigation_native::FakeSystem;

use super::support::*;
use crate::session_open::legacy::{self, GTK_FILE, MAC_FILE};
use crate::session_open::navigator::{searched, Navigator};

const APP: &str = "/Users/example/Library/Application Support/com.agentrelay.desktop";

fn keys() -> Vec<String> {
    vec![legacy::key("claude", CLAUDE_ID)]
}

fn navigator(fake: &FakeSystem) -> Navigator<Fixed, &FakeSystem> {
    let paths = mac_cfg(APP).common.paths;
    Navigator::new(
        Fixed::new(snapshot(&claude())),
        fake,
        APP,
        keys(),
        Fact::Unknown,
        searched(&paths),
    )
}

#[test]
fn a_mac_pending_entry_reads_as_an_unresolved_reservation() {
    let fake = FakeSystem::new();
    let file = format!(r#"{{"version":1,"pending":["codex:other","claude:{CLAUDE_ID}"]}}"#);
    fake.add_file(&join(APP, MAC_FILE), file, 1);
    let snapshot = navigator(&fake).observe(&claude());
    let Fact::Known(Some(reservation)) = snapshot.reservation else {
        panic!("{:?}", snapshot.reservation);
    };
    assert_eq!(reservation.session, claude());
    assert_eq!(reservation.tty, None);
    assert_eq!(reservation.boot, None);
    assert_eq!(reservation.via, Pending::Launch);
}

#[test]
fn a_gtk_pending_entry_reads_the_same_and_other_sessions_do_not() {
    let fake = FakeSystem::new();
    fake.add_file(
        &join(APP, GTK_FILE),
        format!(r#"["claude:{CLAUDE_ID}"]"#),
        1,
    );
    assert!(matches!(
        navigator(&fake).observe(&claude()).reservation,
        Fact::Known(Some(_))
    ));
    let fake = FakeSystem::new();
    fake.add_file(&join(APP, GTK_FILE), r#"["claude:someone-else"]"#, 1);
    assert_eq!(
        navigator(&fake).observe(&claude()).reservation,
        Fact::Known(None)
    );
}

#[test]
fn a_damaged_pending_file_blocks_as_unknown() {
    for (name, body) in [
        (MAC_FILE, r#"{"version":2,"pending":[]}"#),
        (GTK_FILE, "not json"),
    ] {
        let fake = FakeSystem::new();
        fake.add_file(&join(APP, name), body, 1);
        assert_eq!(
            navigator(&fake).observe(&claude()).reservation,
            Fact::Unknown
        );
    }
}

#[test]
fn releasing_clears_the_legacy_entry_and_keeps_the_others() {
    let fake = FakeSystem::new();
    let mac = format!(r#"{{"version":1,"pending":["codex:other","claude:{CLAUDE_ID}"]}}"#);
    fake.add_file(&join(APP, MAC_FILE), mac, 1);
    fake.add_file(
        &join(APP, GTK_FILE),
        format!(r#"["claude:{CLAUDE_ID}"]"#),
        1,
    );
    let mut ports = navigator(&fake);
    ports.release(&claude()).unwrap();
    let kept: serde_json::Value =
        serde_json::from_slice(&fake.file(&join(APP, MAC_FILE)).unwrap()).unwrap();
    assert_eq!(
        kept,
        serde_json::json!({"version":1,"pending":["codex:other"]})
    );
    let gtk: Vec<String> =
        serde_json::from_slice(&fake.file(&join(APP, GTK_FILE)).unwrap()).unwrap();
    assert!(gtk.is_empty());
    assert_eq!(ports.observe(&claude()).reservation, Fact::Known(None));
}

#[test]
fn only_a_live_owner_settles_a_legacy_entry_outside_the_engine() {
    let fake = FakeSystem::new();
    fake.add_file(
        &join(APP, GTK_FILE),
        format!(r#"["claude:{CLAUDE_ID}"]"#),
        1,
    );
    let ports = navigator(&fake);
    let stopped = Decision::unavailable(
        SessionState::Stopped,
        session_navigation::model::Reason::LaunchPending,
    );
    ports.settle_legacy(&stopped);
    assert_eq!(legacy::pending(&fake, APP, &keys()), Fact::Known(true));
    let live = Decision::new(SessionState::LiveLocatable, Plan::Choose(Vec::new()));
    ports.settle_legacy(&live);
    assert_eq!(legacy::pending(&fake, APP, &keys()), Fact::Known(false));
}

fn with_environment(
    found: Fact<BTreeMap<String, String>>,
    recorded: &[(&str, &str)],
) -> Fact<BTreeMap<String, String>> {
    let fake = FakeSystem::new();
    let mut snap = snapshot(&claude());
    snap.environment = found;
    snap.last_seen_host = Fact::Known(Host::Terminal(TerminalKind::Ghostty));
    let recorded = recorded
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let paths = mac_cfg(APP).common.paths;
    let mut ports = Navigator::new(
        Fixed::new(snap),
        &fake,
        APP,
        keys(),
        Fact::Known(recorded),
        searched(&paths),
    );
    ports.observe(&claude()).environment
}

#[test]
fn a_recorded_store_other_than_the_one_searched_is_unknown() {
    let found = Fact::Known(BTreeMap::new());
    // Recorded with the default home the adapter searched: kept.
    let same = with_environment(
        found.clone(),
        &[("CLAUDE_CONFIG_DIR", "/Users/example/.claude/")],
    );
    assert_eq!(same, found);
    // Recorded with another store: what was found is not that store's.
    let other = with_environment(
        found.clone(),
        &[("CLAUDE_CONFIG_DIR", "/Users/example/work-claude")],
    );
    assert_eq!(other, Fact::Unknown);
    // Another provider's key says nothing about this one.
    let unrelated = with_environment(found.clone(), &[("CODEX_HOME", "/elsewhere")]);
    assert_eq!(unrelated, found);
    // Unknown stays unknown.
    assert_eq!(with_environment(Fact::Unknown, &[]), Fact::Unknown);
}
