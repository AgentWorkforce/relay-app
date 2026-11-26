use super::*;

// =============================================================== Claude ===

/// DEFECT (wrong destination, false `Verified`): `parse/claude.rs:163` treats
/// `unarchivedCliSessionId` as a live handle even when `cliSessionId` names a
/// different conversation. The app resumes `cliSessionId` first and falls
/// back to `unarchivedCliSessionId` only when `cliSessionId` is undefined, so
/// a `continue` link for X shows conversation Y while the plan claims
/// `ExactConversation` and `verify::shows_session` accepts it.
/// Evidence: Claude facts A "The *current* conversation is
/// `cliSessionId ?? unarchivedCliSessionId`" and the resume order at
/// asar@16099366; `clearStaleResumeHandle` keeps `unarchivedCliSessionId`
/// for most reasons while the next start gets a new `cliSessionId` (VERIFIED
/// code).
#[test]
fn claude_unarchived_alias_is_not_live_when_cli_session_id_differs() {
    let record = format!(
        r#"{{"sessionId":"{LOCAL_A}","cliSessionId":"{CLAUDE_OTHER}",
            "unarchivedCliSessionId":"{CLAUDE_ID}","priorCliSessionIds":["{CLAUDE_ID}"],
            "isArchived":false,"cwd":"{DIR}"}}"#
    );
    let parsed = parse::claude::parse_desktop_record(record.as_bytes()).expect("parses");
    assert_ne!(
        parsed.claim_for(CLAUDE_ID),
        Some(ClaimKind::Live),
        "a stale unarchive snapshot is reported as the open transcript"
    );
    assert_eq!(parsed.claim_for(CLAUDE_OTHER), Some(ClaimKind::Live));
}

/// DEFECT (plan claims more precision than the host delivers):
/// `providers/claude.rs:182` builds a `code/continue` link with
/// `ExactConversation` for a `Stem` claim. The parser returns `Stem` only when
/// neither `cliSessionId` nor `unarchivedCliSessionId` is this id, so the
/// desktop session will show (and resume) a different conversation.
/// Evidence: Claude facts A, `localLiveHandles` = `[cliSessionId,
/// unarchivedCliSessionId]` only; the stem is in `cliSessionIdsClaimedBy`, not
/// in the live handles.
#[test]
fn claude_stem_only_claim_is_not_exact_conversation() {
    let ev = claude_ev(
        vec![],
        vec![DesktopClaim {
            local_id: format!("local_{CLAUDE_ID}"),
            archived: false,
            kind: ClaimKind::Stem,
            cwd: Some(DIR.into()),
            account: ACCOUNT.into(),
            current: Some(CLAUDE_OTHER.into()),
            last_focused: Fact::Known(None),
        }],
        Some(Entrypoint::Desktop),
    );
    let s = snap(Provider::Claude, CLAUDE_ID, vec![launchd()], &[], ev);
    let d = decide(&s, &req());
    if let Some(a) = d.action() {
        assert!(
            a.precision < Precision::ExactConversation,
            "stem-only claim promised the exact conversation: {a:?}"
        );
    }
}

/// DEFECT (could cause a harmful action): ids are compared byte-for-byte
/// (`providers/claude.rs:45`) although `ids::is_uuid` accepts upper-case hex.
/// With an upper-case request id, a live desktop-hosted owner whose registry
/// names the same UUID in lower case is ignored, the session looks stopped,
/// and with import allowed the plan sends `claude://resume`, which imports a
/// second desktop session for a conversation that is running.
/// Evidence: Claude facts C (registry `sessionId` is the CLI uuid; join rule
/// VERIFIED 3/3); facts B `adoptCliSession` imports a NEW `local_<cliId>`
/// when no record holds the id. Codex facts A1 shows the crate knows ids are
/// case-insensitive (it normalises Codex ids) but Claude ids are not.
#[test]
fn claude_case_variant_of_a_live_id_is_never_imported_again() {
    let upper = CLAUDE_ID.to_ascii_uppercase();
    let table = vec![
        launchd(),
        proc_(819, 1, Exe::ClaudeDesktopApp, None, &["Claude"], None),
        proc_(820, 819, Exe::Other, None, &["disclaimer"], None),
        proc_(
            821,
            820,
            Exe::ClaudeCli,
            None,
            &["claude", "--await-claim"],
            Some(DIR),
        ),
    ];
    let ev = claude_ev(
        vec![RegistryClaim {
            pid: 821,
            proc_start: ident(821).start,
            written_this_boot: Fact::Known(true),
            session_id: CLAUDE_ID.into(),
            host_session: Some(LOCAL_A.into()),
            spare: false,
            entrypoint: Entrypoint::Desktop,
        }],
        vec![],
        Some(Entrypoint::Desktop),
    );
    let s = snap(Provider::Claude, &upper, table, &[App::ClaudeDesktop], ev);
    let mut request = req();
    request.prefs.allow_import = true;
    let d = decide(&s, &request);
    let imports = d
        .action()
        .is_some_and(|a| a.import || a.kind == ActionKind::Reopen);
    assert!(!imports, "imported a duplicate of a running session: {d:?}");
}

/// DEFECT (stuck): a reservation from a `claude://resume` reopen (no tty) is
/// resolved only when the session is *live* (`decide/mod.rs:88-94`). After
/// the reopen the record is unarchived, but the desktop's idle lifecycle
/// stops its process; from then on the only plan is a `continue` link and it
/// is blocked with `LaunchPending` forever (until the person confirms). A
/// `continue` link to a non-archived record cannot create a duplicate: the
/// record is the session.
/// Evidence: Claude facts C "A non-archived desktop session may have no
/// process" (VERIFIED 1 of 4) and the `warmLifecycle` idle pause (VERIFIED
/// code); facts B `continue` only selects an existing record.
#[test]
fn claude_reopen_reservation_resolves_once_the_record_is_unarchived() {
    let ev = claude_ev(
        vec![],
        vec![DesktopClaim {
            local_id: LOCAL_A.into(),
            archived: false,
            kind: ClaimKind::Live,
            cwd: Some(DIR.into()),
            account: ACCOUNT.into(),
            current: Some(CLAUDE_ID.into()),
            last_focused: Fact::Known(None),
        }],
        Some(Entrypoint::Desktop),
    );
    let mut s = snap(
        Provider::Claude,
        CLAUDE_ID,
        vec![launchd()],
        &[App::ClaudeDesktop],
        ev,
    );
    s.reservation = Fact::Known(Some(Reservation {
        session: s.session.clone(),
        made_at: 10,
        tty: None,
        seen: true,
        boot: Some(BOOT),
        via: Pending::Link,
    }));
    let d = decide(&s, &req());
    assert_ne!(
        d.plan,
        Plan::Unavailable(Reason::LaunchPending),
        "reopened desktop session can never be returned to"
    );
}

/// STRENGTH: archived desktop sessions go through `claude://resume` with the
/// CLI uuid, never `code/continue` with the `local_` id (which silently lands
/// on Code home). Evidence: Claude facts B (VERIFIED code).
#[test]
fn claude_archived_record_uses_resume_with_cli_uuid() {
    let ev = claude_ev(
        vec![],
        vec![DesktopClaim {
            local_id: LOCAL_A.into(),
            archived: true,
            kind: ClaimKind::Live,
            cwd: Some(DIR.into()),
            account: ACCOUNT.into(),
            current: Some(CLAUDE_ID.into()),
            last_focused: Fact::Known(None),
        }],
        Some(Entrypoint::Desktop),
    );
    let s = snap(Provider::Claude, CLAUDE_ID, vec![launchd()], &[], ev);
    let d = decide(&s, &req());
    let a = d.action().expect("acts");
    assert_eq!(
        a.steps,
        vec![Step::OpenUrl {
            url: format!("claude://resume?session={CLAUDE_ID}"),
            bundle: "com.anthropic.claudefordesktop".into()
        }]
    );
}
