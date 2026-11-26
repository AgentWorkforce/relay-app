//! Provider evidence over the fake Mac: Claude and Codex, each source's
//! happy path and its failures (OpenCode: `observe_opencode.rs`).
#![cfg(feature = "macos")]
mod common;

use common::*;
use session_navigation::evidence::{
    ClaimKind, ClaudeEvidence, CodexEvidence, CodexOrigin, Entrypoint, Fact, ProviderEvidence,
    Snapshot,
};
use session_navigation::model::SessionRef;
use session_navigation::ports::Observe;
use session_navigation_native::common::codex as ocodex;
use session_navigation_native::system::{FakeSystem, Output, RunError};
use session_navigation_native::MacObserver;

fn observe(fake: &FakeSystem, session: &SessionRef) -> Snapshot {
    MacObserver::new(fake, cfg()).observe(session)
}

fn claude_ev(snap: &Snapshot) -> ClaudeEvidence {
    match &snap.provider {
        ProviderEvidence::Claude(ev) => ev.clone(),
        other => panic!("not claude: {other:?}"),
    }
}

fn codex_ev(snap: &Snapshot) -> CodexEvidence {
    match &snap.provider {
        ProviderEvidence::Codex(ev) => ev.clone(),
        other => panic!("not codex: {other:?}"),
    }
}

#[test]
fn claude_live_desktop_session() {
    let snap = observe(&claude_machine(false, true).build(), &claude());
    let ev = claude_ev(&snap);
    let registry = ev.registry.known().cloned().unwrap_or_default();
    assert_eq!(registry.len(), 1);
    assert_eq!(registry[0].written_this_boot, Fact::Known(true));
    assert_eq!(registry[0].host_session.as_deref(), Some(LOCAL_ID));
    let desktop = ev.desktop.known().cloned().unwrap_or_default();
    assert_eq!(desktop.len(), 1);
    assert_eq!(
        (desktop[0].kind, desktop[0].archived),
        (ClaimKind::Live, false)
    );
    assert_eq!(desktop[0].account, "acct-0001/org-0001");
    assert_eq!(ev.account, Fact::Known("acct-0001/org-0001".into()));
    assert_eq!(
        ev.transcript
            .known()
            .cloned()
            .flatten()
            .map(|t| t.entrypoint),
        Some(Entrypoint::Desktop)
    );
    assert_eq!(snap.cwd, Fact::Known(PROJECT.into()));
    assert_eq!(snap.cwd_exists, Fact::Known(true));
    assert_eq!(snap.environment, Fact::Known(Default::default()));
}

#[test]
fn claude_registry_failures() {
    // An unreadable file whose pid is a live Claude process: Unknown.
    let mut m = claude_machine(false, true);
    m.files.retain(|(p, _, _)| !p.ends_with("25354.json"));
    m.file(
        &format!("{HOME}/.claude/sessions/25354.json"),
        b"{\"pid\": 253".to_vec(),
    );
    assert_eq!(
        claude_ev(&observe(&m.build(), &claude())).registry,
        Fact::Unknown
    );
    // The same garbage for a dead pid is skipped.
    let mut m = claude_machine(false, false);
    m.file(
        &format!("{HOME}/.claude/sessions/4242.json"),
        b"garbage".to_vec(),
    );
    assert_eq!(
        claude_ev(&observe(&m.build(), &claude())).registry,
        Fact::Known(vec![])
    );
    // A file older than the boot was not written this boot.
    let mut m = claude_machine(false, true);
    for f in &mut m.files {
        if f.0.ends_with("25354.json") {
            f.2 = BOOT - 5;
        }
    }
    let reg = claude_ev(&observe(&m.build(), &claude()))
        .registry
        .known()
        .cloned()
        .unwrap_or_default();
    assert_eq!(reg[0].written_this_boot, Fact::Known(false));
    // Directory unreadable.
    let fake = claude_machine(false, true).build();
    fake.make_unreadable(&format!("{HOME}/.claude/sessions"));
    assert_eq!(
        claude_ev(&observe(&fake, &claude())).registry,
        Fact::Unknown
    );
    // No directory at all is an empty registry.
    let fake = FakeSystem::new();
    assert_eq!(
        claude_ev(&observe(&fake, &claude())).registry,
        Fact::Known(vec![])
    );
}

#[test]
fn claude_desktop_and_account_failures() {
    let mut m = claude_machine(true, false);
    m.file(
        &format!(
            "{}/local_99999999-0000-4000-8000-000000000000.json",
            desktop_dir()
        ),
        b"{\"sessionId\":".to_vec(),
    );
    let ev = claude_ev(&observe(&m.build(), &claude()));
    assert_eq!((ev.desktop, ev.account), (Fact::Unknown, Fact::Unknown));
    // A second org: claims stay known, the current account does not.
    let mut m = claude_machine(true, false);
    m.dir(&format!(
        "{HOME}/Library/Application Support/Claude/claude-code-sessions/acct-0001/org-0002"
    ));
    let ev = claude_ev(&observe(&m.build(), &claude()));
    assert_eq!(ev.desktop.known().map(Vec::len), Some(1));
    assert_eq!(ev.account, Fact::Unknown);
    // An archived record is reported archived.
    let ev = claude_ev(&observe(&claude_machine(true, false).build(), &claude()));
    assert_eq!(ev.desktop.known().map(|d| d[0].archived), Some(true));
}

#[test]
fn claude_transcript_and_not_found() {
    let fake = claude_machine(false, false).build();
    fake.make_unreadable(&format!("{HOME}/.claude/projects"));
    assert_eq!(
        claude_ev(&observe(&fake, &claude())).transcript,
        Fact::Unknown
    );
    let mut m = claude_machine(false, false);
    m.files.retain(|(p, _, _)| !p.ends_with(".jsonl"));
    m.file(&transcript_path(), b"{not json\n".to_vec());
    assert_eq!(
        claude_ev(&observe(&m.build(), &claude())).transcript,
        Fact::Unknown
    );
    // Nothing anywhere: not found, environment and cwd unknown.
    let snap = observe(&Machine::new().build(), &claude());
    let ev = claude_ev(&snap);
    assert_eq!(
        (ev.transcript, ev.desktop),
        (Fact::Known(None), Fact::Known(vec![]))
    );
    assert_eq!(
        (snap.environment, snap.cwd, snap.cwd_exists),
        (Fact::Unknown, Fact::Unknown, Fact::Unknown)
    );
}

#[test]
fn codex_thread_row_and_failures() {
    let m = codex_machine(Some(codex_row(false, "codex-tui")));
    let fake = m.build();
    let snap = observe(&fake, &codex());
    let ev = codex_ev(&snap);
    let thread = ev.thread.known().cloned().flatten();
    assert_eq!(
        thread.map(|t| (t.archived, t.origin, t.subagent)),
        Some((false, CodexOrigin::Tui, false))
    );
    assert_eq!(ev.lock_holders, Fact::Known(vec![]));
    assert_eq!(ev.window_owner, Fact::Unknown);
    assert_eq!(snap.cwd, Fact::Known(PROJECT.into()));
    assert_eq!(snap.environment, Fact::Known(Default::default()));
    let run = ocodex::thread_run(&cfg().common, CODEX_ID);
    assert!(fake
        .runs()
        .contains(&(run.program.clone(), run.args.clone())));
    assert_eq!(run.args[..4], ["-readonly", "-json", "-init", "/dev/null"]);
    assert!(run.args[5].ends_with(&format!("WHERE id = '{CODEX_ID}';")));

    // Upper-case input is normalized before it reaches SQL.
    assert_eq!(
        ocodex::query_id(&CODEX_ID.to_uppercase()).as_deref(),
        Some(CODEX_ID)
    );
    assert_eq!(
        ocodex::query_id("01900000-0000-7000-8000-00000000000'"),
        None
    );

    assert_eq!(
        codex_ev(&observe(&codex_machine(None).build(), &codex())).thread,
        Fact::Known(None)
    );
    for bad in [
        Err(RunError::TimedOut),
        Ok(Output::exit(1, "", "Error: database is locked")),
        Ok(Output::ok("[{\"id\":\"x\"}]")),
        Ok(Output::ok(format!("{0}{0}", codex_row(false, "x")))),
    ] {
        let fake = codex_machine(None).build();
        fake.on(&ocodex::thread_run(&cfg().common, CODEX_ID), vec![bad]);
        assert_eq!(codex_ev(&observe(&fake, &codex())).thread, Fact::Unknown);
    }
}

#[test]
fn codex_without_database_reads_the_rollout() {
    let cfg = cfg();
    let rollout = core_fixture("codex/rollout-first-record-cli-tui.json");
    let mut m = Machine::new();
    m.dir(&cfg.common.paths.codex_locks_dir());
    m.file(
        &format!(
            "{}/sessions/2026/01/02/rollout-2026-01-01T19-04-05-{CODEX_ID}.jsonl",
            cfg.common.paths.codex_home
        ),
        rollout.clone(),
    );
    let ev = codex_ev(&observe(&m.build(), &codex()));
    assert_eq!(
        ev.thread
            .known()
            .cloned()
            .flatten()
            .map(|t| (t.archived, t.origin)),
        Some((false, CodexOrigin::Tui))
    );
    m.files.clear();
    m.file(
        &format!(
            "{}/archived_sessions/rollout-2026-01-01T19-04-05-{CODEX_ID}.jsonl",
            cfg.common.paths.codex_home
        ),
        rollout,
    );
    let ev = codex_ev(&observe(&m.build(), &codex()));
    assert_eq!(
        ev.thread.known().cloned().flatten().map(|t| t.archived),
        Some(true)
    );
    let fake = m.build();
    fake.make_unreadable(&format!(
        "{}/archived_sessions",
        cfg.common.paths.codex_home
    ));
    assert_eq!(codex_ev(&observe(&fake, &codex())).thread, Fact::Unknown);
}

#[test]
fn codex_lock_holders() {
    let cfg = cfg();
    let lock = ocodex::lock_path(&cfg.common, CODEX_ID);
    let mut m = codex_machine(Some(codex_row(false, "codex-tui")));
    m.file(&lock, Vec::new());
    m.run(
        ocodex::lock_run(&cfg.common, CODEX_ID),
        Ok(Output::ok("p79653\n")),
    );
    assert_eq!(
        codex_ev(&observe(&m.build(), &codex())).lock_holders,
        Fact::Known(vec![79653])
    );
    let fake = m.build();
    fake.on(
        &ocodex::lock_run(&cfg.common, CODEX_ID),
        vec![Ok(Output::exit(1, "", ""))],
    );
    assert_eq!(
        codex_ev(&observe(&fake, &codex())).lock_holders,
        Fact::Known(vec![])
    );
    for bad in [
        Ok(Output::exit(1, "p1\n", "")),
        Ok(Output::exit(2, "", "")),
        Err(RunError::TimedOut),
    ] {
        let fake = m.build();
        fake.on(&ocodex::lock_run(&cfg.common, CODEX_ID), vec![bad]);
        assert_eq!(
            codex_ev(&observe(&fake, &codex())).lock_holders,
            Fact::Unknown
        );
    }
    // No lock file and no readable lock directory: unknown, not "nobody".
    let mut m = codex_machine(Some(codex_row(false, "codex-tui")));
    m.dirs.clear();
    assert_eq!(
        codex_ev(&observe(&m.build(), &codex())).lock_holders,
        Fact::Unknown
    );
}
