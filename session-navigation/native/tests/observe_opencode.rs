//! OpenCode provider evidence over the fake Mac: each source's happy path
//! and its failures (moved out of `observe_providers.rs`).
#![cfg(feature = "macos")]
mod common;

use common::*;
use session_navigation::evidence::{
    Fact, OpenCodeEvidence, ProviderEvidence, ServerState, Snapshot,
};
use session_navigation::model::SessionRef;
use session_navigation::ports::Observe;
use session_navigation_native::common::opencode as oopencode;
use session_navigation_native::system::{FakeSystem, Output, Run, RunError};
use session_navigation_native::MacObserver;

fn observe(fake: &FakeSystem, session: &SessionRef) -> Snapshot {
    MacObserver::new(fake, cfg()).observe(session)
}

fn opencode_ev(snap: &Snapshot) -> OpenCodeEvidence {
    match &snap.provider {
        ProviderEvidence::OpenCode(ev) => ev.clone(),
        other => panic!("not opencode: {other:?}"),
    }
}

#[test]
fn opencode_server_up() {
    let fake = opencode_machine().build();
    let snap = observe(&fake, &opencode());
    let ev = opencode_ev(&snap);
    assert_eq!(
        ev.server,
        Fact::Known(ServerState::Up {
            pid: 1111,
            version: "2.2.6".into()
        })
    );
    assert_eq!(
        ev.session
            .known()
            .cloned()
            .flatten()
            .map(|s| (s.archived, s.child, s.directory)),
        Some((false, false, PROJECT.to_owned()))
    );
    assert_eq!(ev.busy, Fact::Known(false));
    assert_eq!(
        (ev.desktop_tabs, ev.reserved),
        (Fact::Known(vec![]), Fact::Known(vec![]))
    );
    assert_eq!(snap.cwd, Fact::Known(PROJECT.into()));
    // The password went only to curl's stdin, as a config line.
    let calls = fake.calls();
    let curls: Vec<_> = calls
        .iter()
        .filter_map(|c| match c {
            session_navigation_native::system::Call::Run {
                program,
                args,
                stdin,
                ..
            } if program == session_navigation_native::macos::CURL => {
                Some((args.clone(), stdin.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(curls.len(), 3);
    for (args, stdin) in &curls {
        assert!(!args.join(" ").contains(SECRET));
        assert_eq!(args[..3], ["-q", "--silent", "--show-error"]);
        let config = stdin
            .as_ref()
            .map(|s| String::from_utf8_lossy(s.expose_secret()).into_owned());
        assert_eq!(config, Some(format!("user = \"opencode:{SECRET}\"\n")));
        assert!(!format!("{stdin:?}").contains(SECRET));
    }
    assert!(!format!("{calls:?}").contains(SECRET));
    assert!(!format!("{snap:?}").contains(SECRET));
    assert!(!all_args(&fake).contains("opencode api"));
}

#[test]
fn opencode_busy_not_found_and_server_failures() {
    let mut m = opencode_machine();
    m.run(
        curl("/api/session/active"),
        Ok(curl_ok(
            200,
            &String::from_utf8_lossy(&core_fixture("opencode/api-session-active-busy.json")),
        )),
    );
    assert_eq!(
        opencode_ev(&observe(&m.build(), &opencode())).busy,
        Fact::Known(true)
    );
    let mut m = opencode_machine();
    let missing = format!(
        "{{\"_tag\":\"SessionNotFoundError\",\"sessionID\":\"{OPENCODE_ID}\",\"message\":\"x\"}}"
    );
    m.run(
        curl(&format!("/api/session/{OPENCODE_ID}")),
        Ok(curl_ok(404, &missing)),
    );
    assert_eq!(
        opencode_ev(&observe(&m.build(), &opencode())).session,
        Fact::Known(None)
    );
    // A 500 or timeout is never "not found".
    for bad in [
        Ok(curl_ok(500, "{}")),
        Err(RunError::TimedOut),
        Ok(Output::exit(28, "", "timeout")),
    ] {
        let fake = opencode_machine().build();
        fake.on(&curl(&format!("/api/session/{OPENCODE_ID}")), vec![bad]);
        assert_eq!(
            opencode_ev(&observe(&fake, &opencode())).session,
            Fact::Unknown
        );
    }
    // /api/info disagreeing with service.json: unknown server.
    let fake = opencode_machine().build();
    fake.on(
        &curl("/api/info"),
        vec![Ok(curl_ok(200, "{\"version\":\"2.2.7\",\"pid\":1111}"))],
    );
    let ev = opencode_ev(&observe(&fake, &opencode()));
    assert_eq!((ev.server, ev.busy), (Fact::Unknown, Fact::Unknown));
    // A service file pointing off-host is refused before curl runs.
    let fake = opencode_machine().build();
    fake.add_file(&cfg().common.paths.opencode_service_file(), format!("{{\"version\":\"2.2.6\",\"url\":\"http://evil.example:80\",\"pid\":1111,\"password\":\"{SECRET}\"}}"), 1);
    assert_eq!(
        opencode_ev(&observe(&fake, &opencode())).server,
        Fact::Unknown
    );
    assert!(!all_args(&fake).contains("evil"));
}

#[test]
fn opencode_server_down_falls_back_to_the_database() {
    let cfg = cfg();
    let db_run = oopencode::sqlite_run(
        &cfg.common,
        cfg.common.paths.opencode_db(),
        oopencode::db_query(OPENCODE_ID),
    );
    let row = format!("[{{\"id\":\"{OPENCODE_ID}\",\"parent_id\":null,\"directory\":\"{PROJECT}\",\"time_archived\":null}}]");
    // Refused connection = down.
    let mut m = opencode_machine();
    m.run(
        curl("/api/info"),
        Ok(Output::exit(7, "", "Failed to connect")),
    );
    m.run(db_run.clone(), Ok(Output::ok(row.clone())));
    let ev = opencode_ev(&observe(&m.build(), &opencode()));
    assert_eq!(ev.server, Fact::Known(ServerState::Down));
    assert_eq!(ev.busy, Fact::Known(false));
    assert_eq!(
        ev.session.known().cloned().flatten().map(|s| s.directory),
        Some(PROJECT.to_owned())
    );
    // No service.json at all = down; empty result = not found.
    let mut m = Machine::new();
    m.run(db_run.clone(), Ok(Output::ok("")));
    let fake = m.build();
    let ev = opencode_ev(&observe(&fake, &opencode()));
    assert_eq!(
        (ev.server, ev.session),
        (Fact::Known(ServerState::Down), Fact::Known(None))
    );
    assert!(fake
        .runs()
        .contains(&(db_run.program.clone(), db_run.args.clone())));
    let fake = Machine::new().build();
    fake.on(
        &db_run,
        vec![Ok(Output::exit(1, "", "Error: unable to open database"))],
    );
    assert_eq!(
        opencode_ev(&observe(&fake, &opencode())).session,
        Fact::Unknown
    );
}

#[test]
fn opencode_tabs_and_reservations() {
    let cfg = cfg();
    let mut m = opencode_machine();
    m.file(
        &cfg.common.paths.opencode_tui_tabs(),
        core_fixture("opencode/tui-tabs.json"),
    );
    m.file(
        &cfg.common.paths.opencode_drafts_db(),
        b"SQLite format 3\0".to_vec(),
    );
    let tabs = format!(
        "[{{\"value\":{}}}]",
        serde_json::Value::String(format!(
            "[{{\"type\":\"session\",\"server\":\"sidecar\",\"sessionId\":\"{OPENCODE_ID}\"}}]"
        ))
    );
    m.run(
        oopencode::sqlite_run(
            &cfg.common,
            cfg.common.paths.opencode_drafts_db(),
            oopencode::TABS_QUERY.into(),
        ),
        Ok(Output::ok(tabs)),
    );
    let other = SessionRef::new(
        session_navigation::model::Provider::OpenCode,
        "ses_0123456789acBBBBBBBBBBBBBB",
    );
    let name =
        session_navigation_native::common::reservations::file_name(&other).unwrap_or_default();
    m.file(
        &format!("{}/{name}", cfg.common.paths.reservation_dir),
        b"{}".to_vec(),
    );
    let ev = opencode_ev(&observe(&m.build(), &opencode()));
    assert!(ev.tui_tab_hint && ev.desktop_tab_hint);
    assert_eq!(ev.desktop_tabs, Fact::Known(vec![OPENCODE_ID.to_owned()]));
    assert_eq!(ev.reserved, Fact::Known(vec![other.id.clone()]));
    let fake = m.build();
    fake.on(
        &oopencode::sqlite_run(
            &cfg.common,
            cfg.common.paths.opencode_drafts_db(),
            oopencode::TABS_QUERY.into(),
        ),
        vec![Err(RunError::TooLarge)],
    );
    fake.add_file(
        &format!("{}/garbage", cfg.common.paths.reservation_dir),
        b"".to_vec(),
        1,
    );
    let ev = opencode_ev(&observe(&fake, &opencode()));
    assert_eq!(
        (ev.desktop_tabs, ev.reserved),
        (Fact::Unknown, Fact::Unknown)
    );
    let _ = Run::new("/x", &[]);
}
