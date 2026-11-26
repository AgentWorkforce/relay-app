use super::*;

// ============================================================ OpenCode ===

/// DEFECT (could cause a harmful action): `providers/opencode.rs:50-51`
/// classifies a process as a TUI only when its first non-flag word is absent
/// or starts with `/`. A TUI started with any valued flag
/// (`opencode -m provider/model`) is classified `Role::Other` and ignored, so
/// the planner launches a second TUI next to one that may be showing this
/// very session. The README promises "the planner never starts a TUI while a
/// TUI that might show the session is live".
/// Evidence: OpenCode facts D(i) — a session picked inside the TUI never
/// appears in argv, so any TUI in the session's directory may show it; facts
/// A "Two clients on one session" is UNVERIFIED safe.
#[test]
fn opencode_tui_started_with_a_valued_flag_still_labels_a_second_view() {
    let mut table = terminal_chain("/dev/ttys003");
    table.push(proc_(
        710,
        702,
        Exe::OpenCodeCli,
        Some("/dev/ttys003"),
        &["opencode", "-m", "anthropic/claude-sonnet"],
        Some(DIR),
    ));
    let s = snap(
        Provider::OpenCode,
        OC_ID,
        table,
        &[],
        opencode_ev(Fact::Known(false)),
    );
    // Revised by native N13-N14: the TUI is a client, not a writer, so a
    // resume is allowed, but it must still be recognised as a TUI.
    let d = decide(&s, &req());
    assert!(launches(&d), "{d:?}");
    assert_eq!(
        d.action().and_then(|a| a.note),
        Some(Reason::PossiblySecondView),
        "{d:?}"
    );
}

/// DEFECT (could cause a harmful action): same classifier, relative directory
/// positional. `opencode .` (or `opencode myproject`) is a TUI whose
/// positional `[<directory>]` is relative; it is classified `Role::Other`.
/// Evidence: OpenCode facts F "An optional positional `[<directory>]`"; the
/// TUI `process.chdir(directory)`s (facts D(i)), so its cwd is the project.
#[test]
fn opencode_tui_started_with_a_relative_directory_still_labels_a_second_view() {
    let mut table = terminal_chain("/dev/ttys003");
    table.push(proc_(
        710,
        702,
        Exe::OpenCodeCli,
        Some("/dev/ttys003"),
        &["opencode", "."],
        Some(DIR),
    ));
    let s = snap(
        Provider::OpenCode,
        OC_ID,
        table,
        &[],
        opencode_ev(Fact::Known(false)),
    );
    let d = decide(&s, &req());
    assert!(launches(&d), "{d:?}");
    assert_eq!(
        d.action().and_then(|a| a.note),
        Some(Reason::PossiblySecondView),
        "{d:?}"
    );
}

/// DEFECT (could cause a harmful action): `OpenCodeEvidence::busy` is never
/// read by any policy (grep: no use outside `sim`). A session with a running
/// foreground drain on the server is being driven by some client right now
/// (a `--server` TUI on another machine via `opencode pair`, a surviving
/// desktop turn, an API client) even when no local TUI is visible, yet the
/// planner starts a TUI on it.
/// Evidence: OpenCode facts D(iii) `/api/session/active` "Sessions absent from
/// the result are inactive" (so present = active, VERIFIED); facts A "two
/// clients prompting the same session concurrently" is UNVERIFIED safe.
#[test]
fn opencode_busy_session_is_not_resumed_into_a_second_client() {
    let s = snap(
        Provider::OpenCode,
        OC_ID,
        vec![launchd()],
        &[],
        opencode_ev(Fact::Known(true)),
    );
    let d = decide(&s, &req());
    assert!(
        !launches(&d),
        "a TUI was launched on a session the server reports as running: {d:?}"
    );
}

/// DEFECT (wrong destination): `providers/opencode.rs:134` ignores the
/// session's own `directory` (`_session`) and launches in `snap.cwd`. When the
/// two disagree the TUI starts in the wrong project.
/// Evidence: OpenCode facts F "Running in `session_v2.directory` is the safe
/// choice" and `-c` is cwd-dependent; the API returns `location.directory` as
/// the session's place (facts F, VERIFIED shape).
#[test]
fn opencode_resume_runs_in_the_sessions_own_directory() {
    let mut s = snap(
        Provider::OpenCode,
        OC_ID,
        vec![launchd()],
        &[],
        opencode_ev(Fact::Known(false)),
    );
    s.cwd = Fact::Known("/Users/example/elsewhere".into());
    let d = decide(&s, &req());
    if let Some(Step::Launch { command, .. }) = d.action().and_then(|a| a.steps.last()) {
        assert_eq!(command.cwd, DIR, "launched outside the session's directory");
    }
}

/// STRENGTH: the `-s` hazard (a missing id is created) is guarded. A
/// positive not-found, an unknown existence, a child or an archived session
/// never produce a launch. Evidence: OpenCode facts F (VERIFIED code).
#[test]
fn opencode_never_launches_without_positive_existence_of_a_top_level_session() {
    let cases: Vec<Fact<Option<OpenCodeSession>>> = vec![
        Fact::Unknown,
        Fact::Known(None),
        Fact::Known(Some(OpenCodeSession {
            archived: false,
            child: true,
            directory: DIR.into(),
        })),
        Fact::Known(Some(OpenCodeSession {
            archived: true,
            child: false,
            directory: DIR.into(),
        })),
    ];
    for session in cases {
        let mut ev = match opencode_ev(Fact::Known(false)) {
            ProviderEvidence::OpenCode(ev) => ev,
            _ => unreachable!(),
        };
        ev.session = session.clone();
        let s = snap(
            Provider::OpenCode,
            OC_ID,
            vec![launchd()],
            &[],
            ProviderEvidence::OpenCode(ev),
        );
        assert!(!launches(&decide(&s, &req())), "{session:?}");
    }
}
