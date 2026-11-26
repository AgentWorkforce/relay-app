//! Parser and fixture tests, including malformed and hostile input, and URL
//! and argv escaping.
use session_navigation::evidence::{ClaimKind, CodexOrigin, Entrypoint};
use session_navigation::parse::{claude, codex, ids, lstart, opencode, urls, ParseError};
use session_navigation::sim::rng::Rng;

macro_rules! fixture {
    ($p:literal) => {
        include_bytes!(concat!("../fixtures/", $p)).as_slice()
    };
}

// ---------------------------------------------------------------- Claude ---

#[test]
fn claude_registry_fixture() {
    let r = claude::parse_registry(fixture!("claude/cli-registry-pid.json")).unwrap();
    assert_eq!(r.pid, 43210);
    assert_eq!(r.session_id, "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee");
    assert_eq!(r.entrypoint, Entrypoint::Desktop);
    assert_eq!(
        r.host_session_id.as_deref(),
        Some("local_11111111-2222-4333-8444-555555555555")
    );
    assert_eq!(lstart::format(r.proc_start), "Wed Sep 30 10:00:00 2026");
    assert!(!r.spare && !r.background);
}

#[test]
fn claude_registry_first_value_only_and_torn() {
    let mut bytes = fixture!("claude/cli-registry-pid.json").to_vec();
    bytes.extend_from_slice(b"}garbage{\"pid\":");
    assert!(
        claude::parse_registry(&bytes).is_ok(),
        "garbage suffix after a whole value"
    );
    let torn = &fixture!("claude/cli-registry-pid.json")[..200];
    assert!(claude::parse_registry(torn).is_err());
    assert_eq!(claude::parse_registry(b"   "), Err(ParseError::Empty));
}

#[test]
fn claude_registry_hostile() {
    let base = r#"{"pid":PID,"sessionId":"SID","procStart":"PS","hostSessionId":"HOST"}"#;
    let make = |pid: &str, sid: &str, ps: &str, host: &str| {
        base.replace("PID", pid)
            .replace("SID", sid)
            .replace("PS", ps)
            .replace("HOST", host)
    };
    let good = make(
        "500",
        "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "Wed Sep 30 10:00:00 2026",
        "local_abc",
    );
    assert!(claude::parse_registry(good.as_bytes()).is_ok());
    for bad in [
        make(
            "0",
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "Wed Sep 30 10:00:00 2026",
            "local_abc",
        ),
        make(
            "-5",
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "Wed Sep 30 10:00:00 2026",
            "local_abc",
        ),
        make(
            "99999999999",
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "Wed Sep 30 10:00:00 2026",
            "local_abc",
        ),
        make(
            "500",
            "../../etc/passwd",
            "Wed Sep 30 10:00:00 2026",
            "local_abc",
        ),
        make(
            "500",
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "Thu Sep 30 10:00:00 2026",
            "local_abc",
        ),
        make(
            "500",
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "Wed Sep 31 10:00:00 2026",
            "local_abc",
        ),
        make(
            "500",
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "Wed Sep 30 10:00:00 2026",
            "local_a&prompt=x",
        ),
    ] {
        assert!(claude::parse_registry(bad.as_bytes()).is_err(), "{bad}");
    }
    let oversized = vec![b' '; claude::REGISTRY_LIMIT + 1];
    assert_eq!(
        claude::parse_registry(&oversized),
        Err(ParseError::TooLarge)
    );
    let deep = "[".repeat(50_000);
    assert!(claude::parse_registry(deep.as_bytes()).is_err());
}

#[test]
fn claude_desktop_record_claims() {
    let r = claude::parse_desktop_record(fixture!("claude/desktop-session-local.json")).unwrap();
    assert!(!r.archived);
    assert_eq!(
        r.claim_for("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
        Some(ClaimKind::Live)
    );
    assert_eq!(
        r.claim_for("99999999-8888-4777-8666-555555555555"),
        Some(ClaimKind::Prior)
    );
    assert_eq!(
        r.claim_for("12121212-3434-4565-8787-909090909090"),
        Some(ClaimKind::Prior)
    );
    assert_eq!(
        r.claim_for("11111111-2222-4333-8444-555555555555"),
        Some(ClaimKind::Stem)
    );
    assert_eq!(r.claim_for("bbbbbbbb-cccc-4ddd-8eee-ffffffffffff"), None);
    let a = claude::parse_desktop_record(fixture!("claude/desktop-session-worktree-archived.json"))
        .unwrap();
    assert!(a.archived);
    assert!(a.cwd.unwrap().contains("/.claude/worktrees/"));
}

#[test]
fn claude_desktop_record_hostile() {
    let missing_archive =
        r#"{"sessionId":"local_abc","cliSessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"}"#;
    assert_eq!(
        claude::parse_desktop_record(missing_archive.as_bytes()),
        Err(ParseError::MissingField("isArchived"))
    );
    let bad_alias = r#"{"sessionId":"local_abc","isArchived":false,"priorCliSessionIds":["x y"]}"#;
    assert!(claude::parse_desktop_record(bad_alias.as_bytes()).is_err());
    let bad_local = r#"{"sessionId":"local_../x","isArchived":false}"#;
    assert!(claude::parse_desktop_record(bad_local.as_bytes()).is_err());
    let relative_cwd = r#"{"sessionId":"local_abc","isArchived":false,"cwd":"relative/dir"}"#;
    assert_eq!(
        claude::parse_desktop_record(relative_cwd.as_bytes())
            .unwrap()
            .cwd,
        None
    );
}

#[test]
fn claude_archived_index_and_transcript() {
    let idx = claude::parse_archived_index(fixture!("claude/desktop-archived-sessions.idx.json"))
        .unwrap();
    assert_eq!(
        idx,
        vec!["local_22222222-3333-4444-8555-666666666666".to_owned()]
    );
    assert!(claude::parse_archived_index(br#"{"v":2,"archived":[]}"#).is_err());
    let line = claude::parse_transcript_line(fixture!("claude/transcript-first-message-line.json"))
        .unwrap();
    assert_eq!(line.entrypoint, Some(Entrypoint::Desktop));
    assert_eq!(line.cwd.as_deref(), Some("/Users/example/code/project"));
}

// ----------------------------------------------------------------- Codex ---

#[test]
fn codex_rollout_first_records() {
    let d =
        codex::parse_rollout_first(fixture!("codex/rollout-first-record-desktop.json")).unwrap();
    assert_eq!((d.origin, d.subagent), (CodexOrigin::Desktop, false));
    let t =
        codex::parse_rollout_first(fixture!("codex/rollout-first-record-cli-tui.json")).unwrap();
    assert_eq!((t.origin, t.subagent), (CodexOrigin::Tui, false));
    let g = codex::parse_rollout_first(fixture!(
        "codex/rollout-first-record-subagent-guardian.json"
    ))
    .unwrap();
    assert!(g.subagent);
    assert_eq!(
        g.parent_thread_id.as_deref(),
        Some("01900000-0000-7000-8000-000000000001")
    );
    assert!(codex::parse_rollout_first(br#"{"type":"response_item","payload":{}}"#).is_err());
}

#[test]
fn codex_thread_rows() {
    let r = codex::parse_thread_row(fixture!("codex/state_5-threads-row.json")).unwrap();
    assert!(!r.archived && !r.subagent);
    assert_eq!(r.origin, CodexOrigin::Desktop);
    let a = codex::parse_thread_row(fixture!("codex/state_5-threads-row-archived.json")).unwrap();
    assert!(a.archived);
    assert_eq!(a.origin, CodexOrigin::Tui);
    let weird = br#"{"id":"01900000-0000-7000-8000-000000000001","archived":2}"#;
    assert_eq!(
        codex::parse_thread_row(weird),
        Err(ParseError::InvalidValue("archived"))
    );
    let upper =
        br#"{"id":"01900000-0000-7000-8000-00000000000A","archived":0,"thread_source":"user"}"#;
    assert_eq!(
        codex::parse_thread_row(upper).unwrap().id,
        "01900000-0000-7000-8000-00000000000a"
    );
    let v4 = br#"{"id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","archived":0}"#;
    assert!(codex::parse_thread_row(v4).is_err());
}

#[test]
fn codex_rollout_names() {
    let name = "rollout-2026-01-01T19-04-05-01900000-0000-7000-8000-000000000001.jsonl";
    assert_eq!(
        codex::thread_id_from_rollout_name(name).as_deref(),
        Some("01900000-0000-7000-8000-000000000001")
    );
    assert_eq!(codex::thread_id_from_rollout_name("rollout-x.jsonl"), None);
    assert_eq!(
        codex::thread_id_from_rollout_name("rollout-01900000-0000-7000-8000-000000000001.jsonl"),
        None
    );
}

// -------------------------------------------------------------- OpenCode ---

#[test]
fn opencode_service_state_never_keeps_the_password() {
    let s = opencode::parse_service_state(fixture!("opencode/service-state.json")).unwrap();
    assert_eq!((s.pid, s.version.as_str()), (4242, "2.2.6"));
    assert!(!format!("{s:?}").contains("SYNTHETIC"));
    let remote = br#"{"version":"2.2.6","url":"http://example.com:49374","pid":5}"#;
    assert!(opencode::parse_service_state(remote).is_err());
    let port = br#"{"version":"2.2.6","url":"http://127.0.0.1:99999","pid":5}"#;
    assert!(opencode::parse_service_state(port).is_err());
    let info = opencode::parse_api_info(fixture!("opencode/api-info.json")).unwrap();
    assert_eq!(info.pid, 4242);
}

#[test]
fn opencode_session_lookups() {
    let id = "ses_0123456789abAAAAAAAAAAAAAA";
    let found = opencode::parse_session_lookup(200, fixture!("opencode/api-session-info.json"), id)
        .unwrap()
        .unwrap();
    assert!(!found.archived && !found.child);
    let wrapped = format!(
        r#"{{"data":{}}}"#,
        String::from_utf8_lossy(fixture!("opencode/api-session-info.json"))
    );
    assert!(opencode::parse_session_lookup(200, wrapped.as_bytes(), id)
        .unwrap()
        .is_some());
    let child_id = "ses_0123456789acBBBBBBBBBBBBBB";
    let child = opencode::parse_session_lookup(
        200,
        fixture!("opencode/api-session-child-archived.json"),
        child_id,
    )
    .unwrap()
    .unwrap();
    assert!(child.archived && child.child);
    let missing = "ses_000000000000000000000000ZZ";
    let not_found = fixture!("opencode/api-session-not-found.json");
    assert_eq!(
        opencode::parse_session_lookup(404, not_found, missing),
        Ok(None)
    );
    // Absence is only ever a positive, exact not-found.
    assert!(opencode::parse_session_lookup(404, not_found, id).is_err());
    assert!(opencode::parse_session_lookup(500, b"{}", id).is_err());
    assert!(opencode::parse_session_lookup(404, b"<html>", missing).is_err());
    assert!(opencode::parse_session_lookup(
        200,
        fixture!("opencode/api-session-info.json"),
        child_id
    )
    .is_err());
}

#[test]
fn opencode_db_active_and_tabs() {
    let id = "ses_0123456789abAAAAAAAAAAAAAA";
    let row = opencode::parse_db_row(fixture!("opencode/db-session_v2-row.json"), id).unwrap();
    assert_eq!(row.directory, "/Users/example/project");
    assert_eq!(
        opencode::parse_active(fixture!("opencode/api-session-active-busy.json")).unwrap(),
        vec![id.to_owned()]
    );
    assert!(
        opencode::parse_active(fixture!("opencode/api-session-active-idle.json"))
            .unwrap()
            .is_empty()
    );
    let tabs = opencode::parse_tui_tabs(fixture!("opencode/tui-tabs.json")).unwrap();
    assert_eq!(
        tabs,
        vec![("/Users/example/project".to_owned(), vec![id.to_owned()])]
    );
}

// ------------------------------------------------------- ids, urls, lstart ---

#[test]
fn id_validators() {
    assert!(ids::is_claude_cli_id(
        "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
    ));
    assert!(!ids::is_claude_cli_id(
        "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeee"
    ));
    assert!(!ids::is_claude_cli_id(
        "aaaaaaaa_bbbb-4ccc-8ddd-eeeeeeeeeeee"
    ));
    assert!(ids::is_claude_local_id(
        "local_11111111-2222-4333-8444-555555555555"
    ));
    for bad in [
        "local_",
        "local_a b",
        "local_a/../b",
        "LOCAL_abc",
        &format!("local_{}", "a".repeat(65)),
    ] {
        assert!(!ids::is_claude_local_id(bad), "{bad}");
    }
    assert!(ids::is_codex_thread_id(
        "01900000-0000-7000-8000-000000000001"
    ));
    assert!(!ids::is_codex_thread_id(
        "01900000-0000-4000-8000-000000000001"
    ));
    assert!(!ids::is_codex_thread_id(
        "01900000-0000-7000-8000-00000000000A"
    ));
    assert!(ids::is_opencode_session_id(
        "ses_0123456789abAAAAAAAAAAAAAA"
    ));
    for bad in [
        "ses_0123456789AbAAAAAAAAAAAAAA",
        "ses_0123456789abAAAAAAAAAAAAA",
        "ses_0123456789abAAAAAAAAAAAA-A",
    ] {
        assert!(!ids::is_opencode_session_id(bad), "{bad}");
    }
}

#[test]
fn url_builders_are_exact_and_refuse_hostile_ids() {
    assert_eq!(
        urls::claude_continue("local_11111111-2222-4333-8444-555555555555").unwrap(),
        "claude://code/continue?session=local_11111111-2222-4333-8444-555555555555"
    );
    assert_eq!(
        urls::claude_resume("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
        "claude://resume?session=aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
    );
    assert_eq!(
        urls::codex_thread("01900000-0000-7000-8000-000000000001").unwrap(),
        "codex://threads/01900000-0000-7000-8000-000000000001"
    );
    for hostile in [
        "local_x&prompt=hi",
        "local_x#frag",
        "../x",
        "",
        "local_\u{202e}x",
        "local_x\nprompt",
    ] {
        assert!(urls::claude_continue(hostile).is_err(), "{hostile:?}");
        assert!(urls::claude_resume(hostile).is_err());
        assert!(urls::codex_thread(hostile).is_err());
    }
    assert_eq!(urls::encode_component("a b&c/é"), "a%20b%26c%2F%C3%A9");
}

#[test]
fn lstart_round_trip_and_rejects() {
    let mut rng = Rng::new(1);
    for _ in 0..5000 {
        let t = 1_000_000_000 + rng.below(3_000_000_000);
        assert_eq!(lstart::parse(&lstart::format(t)), Ok(t));
    }
    assert_eq!(lstart::format(1_767_294_245), "Thu Jan  1 19:04:05 2026");
    assert!(lstart::parse("Thu Jan  1 19:04:05 2026").is_ok());
    for bad in [
        "Sat Feb 29 00:00:00 2025",
        "Wed Sep 30 24:00:00 2026",
        "Wed Sep 30 10:00 2026",
        "x",
        "Wed Sep 30 10:00:00 2026 extra",
    ] {
        assert!(lstart::parse(bad).is_err(), "{bad}");
    }
}

/// Random and mutated bytes never panic any parser.
#[test]
fn parsers_never_panic_on_random_or_mutated_input() {
    let seeds: [&[u8]; 6] = [
        fixture!("claude/cli-registry-pid.json"),
        fixture!("claude/desktop-session-local.json"),
        fixture!("codex/rollout-first-record-desktop.json"),
        fixture!("codex/state_5-threads-row.json"),
        fixture!("opencode/api-session-info.json"),
        fixture!("opencode/service-state.json"),
    ];
    let mut rng = Rng::new(99);
    for round in 0..20_000u64 {
        let base = seeds[(round % 6) as usize];
        let mut bytes = base.to_vec();
        for _ in 0..1 + rng.below(8) {
            let i = rng.below(bytes.len() as u64) as usize;
            match rng.below(3) {
                0 => bytes[i] = rng.below(256) as u8,
                1 => bytes.truncate(i),
                _ => bytes.insert(i, b"{}[]\":,\\0"[rng.below(9) as usize]),
            }
            if bytes.is_empty() {
                break;
            }
        }
        let _ = claude::parse_registry(&bytes);
        let _ = claude::parse_desktop_record(&bytes);
        let _ = claude::parse_transcript_line(&bytes);
        let _ = codex::parse_rollout_first(&bytes);
        let _ = codex::parse_thread_row(&bytes);
        let _ = opencode::parse_service_state(&bytes);
        let _ = opencode::parse_session_lookup(200, &bytes, "ses_0123456789abAAAAAAAAAAAAAA");
        let _ = opencode::parse_db_row(&bytes, "ses_0123456789abAAAAAAAAAAAAAA");
        let _ = opencode::parse_tui_tabs(&bytes);
        let text = String::from_utf8_lossy(&bytes);
        let _ = lstart::parse(&text);
        let _ = codex::thread_id_from_rollout_name(&text);
    }
}
