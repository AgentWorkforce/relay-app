use super::*;

// ================================================================ Codex ===

/// DEFECT (plan claims more precision than it can deliver):
/// `providers/codex.rs:128-139` focuses a TUI with `ExactConversation` when
/// only its argv names the thread, even when the lock evidence is complete
/// and nobody holds the thread's writer lock. Argv is fixed at start; a TUI
/// that has the thread open holds it loaded (itself or via the daemon), so an
/// empty holder list means the pane now shows something else.
/// Evidence: Codex facts 12/15 (a lock = "loaded in some Codex process"),
/// facts 17 (the daemon or the TUI holds it), facts D(i) of OpenCode by
/// analogy for argv-only evidence.
#[test]
fn codex_argv_only_tui_without_any_lock_is_not_exact() {
    let mut table = ghostty_chain("/dev/ttys000");
    table.push(proc_(
        4242,
        998,
        Exe::CodexCli,
        Some("/dev/ttys000"),
        &["codex", "resume", CODEX_ID],
        Some(DIR),
    ));
    let s = snap(
        Provider::Codex,
        CODEX_ID,
        table,
        &[],
        codex_ev(CodexOrigin::Tui, vec![]),
    );
    let d = decide(&s, &req());
    if let Some(a) = d.action() {
        assert!(a.precision < Precision::ExactConversation, "{a:?}");
    }
}

/// DEFECT (useless): `providers/codex.rs:49` takes the first word that does
/// not start with `-` as the subcommand, so the value of any option
/// (`codex -m gpt-5 resume <id>`, `codex -c k=v resume <id>`) is read as the
/// subcommand and a live TUI that both names and holds the thread is
/// "not locatable". The desktop's own app-server uses `-c k=v` before its
/// subcommand, so this shape is real.
/// Evidence: Codex facts 11 / fixtures processes.json (`codex -c ... app-server`),
/// facts 16 (allowlist `codex [opts] [PROMPT]`, `codex resume …`).
#[test]
fn codex_tui_with_a_leading_option_is_still_located() {
    let mut table = ghostty_chain("/dev/ttys000");
    table.push(proc_(
        4242,
        998,
        Exe::CodexCli,
        Some("/dev/ttys000"),
        &["codex", "-m", "gpt-5", "resume", CODEX_ID],
        Some(DIR),
    ));
    let s = snap(
        Provider::Codex,
        CODEX_ID,
        table,
        &[],
        codex_ev(CodexOrigin::Tui, vec![4242]),
    );
    let d = decide(&s, &req());
    assert_eq!(
        d.action().map(|a| a.kind),
        Some(ActionKind::FocusPane),
        "{d:?}"
    );
}

/// DEFECT (a selected focus turns into a launch): `decide/mod.rs:54-57`
/// passes any `Act` whose `target` is `None` through a selection unchanged.
/// A person who picked a specific pane from an earlier choice gets a new
/// terminal window and a resumed process when that pane has gone, instead of
/// `SelectionGone`. README: "The engine never turns a failed focus or link
/// into a launch"; `select` claims to honour the pick "by full identity".
/// Evidence: README "Verify"; Codex facts 21 (relay launches always open a new
/// window).
#[test]
fn a_selected_pane_that_is_gone_never_becomes_a_launch() {
    let s = snap(
        Provider::Codex,
        CODEX_ID,
        ghostty_chain("/dev/ttys000"),
        &[],
        codex_ev(CodexOrigin::Tui, vec![]),
    );
    let request = OpenRequest {
        selected: Some(Target::Pane(ident(4242))),
        ..req()
    };
    let d = decide(&s, &request);
    assert_eq!(d.plan, Plan::Unavailable(Reason::SelectionGone), "{d:?}");
}

/// STRENGTH: the Codex link never carries `prompt` (it pre-fills the
/// composer) or any query, and a non-v7 / upper-case / hostile id never
/// reaches a URL. Evidence: Codex facts 8 (zod uuid regex; `prompt`).
#[test]
fn codex_link_is_bare_and_rejects_hostile_ids() {
    assert_eq!(
        parse::urls::codex_thread(CODEX_ID).as_deref(),
        Ok(&*format!("codex://threads/{CODEX_ID}"))
    );
    for bad in [
        "01900000-0000-4000-8000-000000000002",
        "01900000-0000-7000-8000-00000000000A",
        "01900000-0000-7000-8000-00000000000/",
        "-1900000-0000-7000-8000-000000000002",
        "01900000-0000-7000-8000-000000000002?prompt=x",
        "01900000-0000-7000-8000-000000000002#x",
    ] {
        assert!(parse::urls::codex_thread(bad).is_err(), "{bad}");
    }
}
