use super::*;

// ================================================= 3. usefulness floor ===
//
// A planner that always answers "unavailable" passes every safety invariant.
// These pin the genuinely useful outcome for each host in its clean common
// case, under every value of the UNVERIFIED variables that do not concern it.

/// Claude desktop, session open in the app (process live) → OpenExisting via
/// `claude://code/continue?session=<local>` (Claude facts B; join in facts C).
#[test]
fn floor_claude_desktop_live_is_opened_in_the_app() {
    all_combos(
        "claude desktop live",
        |_| true,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::ClaudeDesktop, dir(0))
                .ok_or("start")?;
            let local = w
                .views
                .get(&App::ClaudeDesktop)
                .cloned()
                .flatten()
                .ok_or("view")?;
            w.open_pane(TerminalKind::Ghostty, dir(1)); // something else is in front
            w.views.insert(App::ClaudeDesktop, None); // the app shows Code home
            w.apply(&Event::Discover);
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            if kind(&r.decision) != Some(ActionKind::OpenExisting) || !useful(&r.outcome) {
                return Err(format!("{:?} → {:?}", r.decision.plan, r.outcome));
            }
            let shown = w.front_app == Some(App::ClaudeDesktop)
                && w.views.get(&App::ClaudeDesktop).cloned().flatten() == Some(local);
            shown
                .then_some(())
                .ok_or_else(|| "session not shown".into())
        },
    );
}

/// Claude desktop session whose process the app paused (facts C: "A
/// non-archived desktop session may have no process") → still OpenExisting.
#[test]
fn floor_claude_desktop_idle_paused_is_opened_in_the_app() {
    all_combos(
        "claude desktop idle",
        |_| true,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::ClaudeDesktop, dir(0))
                .ok_or("start")?;
            let child = w
                .procs
                .values()
                .find(|p| matches!(p.role, Role::DesktopChild(_)))
                .map(|p| p.pid)
                .ok_or("child")?;
            w.kill(child, Exit::Normal); // idle pause; the app keeps running
            w.apply(&Event::Discover);
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            (kind(&r.decision) == Some(ActionKind::OpenExisting) && useful(&r.outcome))
                .then_some(())
                .ok_or_else(|| format!("{:?} → {:?}", r.decision.plan, r.outcome))
        },
    );
}

/// Claude desktop, archived → revive with `claude://resume?session=<cli id>`
/// (facts B: `code/continue` does not revive; `resume` unarchives). When the
/// link does unarchive, the session must come back and be shown.
#[test]
fn floor_claude_desktop_archived_is_revived() {
    all_combos(
        "claude desktop archived",
        |_| true,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::ClaudeDesktop, dir(0))
                .ok_or("start")?;
            w.apply(&Event::Archive { pick: 0 });
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            let url = format!("claude://resume?session={}", s.id);
            let ok_plan = matches!(r.decision.action(), Some(a) if a.kind == ActionKind::Reopen
            && matches!(a.steps.as_slice(), [Step::OpenUrl { url: u, .. }] if *u == url));
            if !ok_plan {
                return Err(format!("plan {:?}", r.decision.plan));
            }
            // VERIFIED for 5.1231.2 (native N5): the link revives the
            // record in every world, and `lastFocusedAt` proves it.
            let revived = w
                .claude
                .desktop
                .values()
                .any(|d| d.holds_live(&s.id) && !d.archived);
            if !revived || r.outcome != Outcome::Verified {
                return Err(format!("not revived: {:?}", r.outcome));
            }
            Ok(())
        },
    );
}

/// Claude CLI live in Ghostty → focus its pane (facts D: registry + tty).
#[test]
fn floor_claude_ghostty_live_is_focused() {
    all_combos(
        "claude ghostty live",
        |_| true,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::ClaudeGhostty, dir(0))
                .ok_or("start")?;
            let pane = pane_of(&w, &s);
            w.open_pane(TerminalKind::Ghostty, dir(1));
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            (kind(&r.decision) == Some(ActionKind::FocusPane)
                && r.outcome == Outcome::Verified
                && w.front_pane == Some(pane))
            .then_some(())
            .ok_or_else(|| format!("{:?} → {:?}", r.decision.plan, r.outcome))
        },
    );
}

/// Claude CLI in Ghostty, window closed → `claude --resume <id>` in a new
/// Ghostty window in the session's directory, verified.
#[test]
fn floor_claude_ghostty_closed_resumes_in_ghostty() {
    all_combos(
        "claude ghostty closed",
        |_| true,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::ClaudeGhostty, dir(0))
                .ok_or("start")?;
            w.apply(&Event::Discover);
            let pane = pane_of(&w, &s);
            w.close_pane(pane);
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            let ok = matches!(r.decision.action(), Some(a) if matches!(a.steps.as_slice(),
            [Step::Launch { terminal: TerminalKind::Ghostty, command, .. }]
                if command.program.ends_with("/claude") && command.cwd == dir(0)));
            (ok && r.outcome == Outcome::Verified && w.writers(&s) == 1)
                .then_some(())
                .ok_or_else(|| format!("{:?} → {:?}", r.decision.plan, r.outcome))
        },
    );
}

/// FINDING (over-caution). A second, unrelated Claude CLI in the same repo
/// blocks resuming a closed one with PossiblyOpenElsewhere — always, because
/// the planner cannot tell. Claude facts C: the registry names the session
/// per pid (join VERIFIED 3/3), the writer uses `sessionId:G()` (the current
/// session) and the CLI "rewrites its registry files in place" (facts F) on
/// every status update; only the rewrite-after-/clear is UNVERIFIED (summary
/// 5). In every world where the registry IS rewritten, this refusal protects
/// nothing, and two CLIs in one repo is the common case. What SHOULD happen:
/// resume. Fix: verify summary 5 with one native experiment and drop the rule,
/// or narrow it to CLIs whose registry `updatedAt` predates the target
/// transcript's last write (needs those fields in the snapshot and the sim).
#[test]
fn floor_claude_resume_not_blocked_by_unrelated_cli_in_same_repo() {
    all_combos(
        "claude closed + unrelated CLI same repo",
        |v| v.claude_registry_rewritten_on_clear,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::ClaudeGhostty, dir(0))
                .ok_or("start")?;
            w.apply(&Event::Discover);
            let pane = pane_of(&w, &s);
            w.close_pane(pane);
            w.start_session(StartHost::ClaudeGhostty, dir(0))
                .ok_or("other")?;
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            (kind(&r.decision) == Some(ActionKind::Resume) && r.outcome == Outcome::Verified)
                .then_some(())
                .ok_or_else(|| format!("{:?} → {:?}", r.decision.plan, r.outcome))
        },
    );
}

/// FINDING (over-caution, worse than the README states). The same rule runs
/// before the desktop branch, so a non-archived Claude DESKTOP session whose
/// process the app paused (facts C: normal, "not running does not mean
/// closed") cannot even be opened in the app with `code/continue` while any
/// terminal Claude CLI runs in that repo. Measured: ~676 such refusals in the
/// random runs with a rewritten registry. What SHOULD happen (registry
/// rewritten): OpenExisting via `claude://code/continue`.
#[test]
fn floor_claude_desktop_idle_not_blocked_by_unrelated_cli_in_same_repo() {
    all_combos(
        "claude desktop idle + unrelated CLI same repo",
        |v| v.claude_registry_rewritten_on_clear,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::ClaudeDesktop, dir(0))
                .ok_or("start")?;
            let child = w
                .procs
                .values()
                .find(|p| matches!(p.role, Role::DesktopChild(_)))
                .map(|p| p.pid)
                .ok_or("child")?;
            w.kill(child, Exit::Normal);
            w.start_session(StartHost::ClaudeGhostty, dir(0))
                .ok_or("other")?;
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            (kind(&r.decision) == Some(ActionKind::OpenExisting) && useful(&r.outcome))
                .then_some(())
                .ok_or_else(|| format!("{:?} → {:?}", r.decision.plan, r.outcome))
        },
    );
}

/// Strength check: the block is scoped by directory, and Claude desktop
/// children (registry has hostSessionId) never block a CLI resume.
#[test]
fn floor_claude_resume_unaffected_by_cli_elsewhere_or_desktop_in_same_repo() {
    all_combos(
        "claude closed + other dir CLI + desktop",
        |_| true,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::ClaudeGhostty, dir(0))
                .ok_or("start")?;
            w.apply(&Event::Discover);
            let pane = pane_of(&w, &s);
            w.close_pane(pane);
            w.start_session(StartHost::ClaudeGhostty, dir(1))
                .ok_or("other")?;
            w.start_session(StartHost::ClaudeDesktop, dir(0))
                .ok_or("desktop")?;
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            (kind(&r.decision) == Some(ActionKind::Resume) && useful(&r.outcome))
                .then_some(())
                .ok_or_else(|| format!("{:?} → {:?}", r.decision.plan, r.outcome))
        },
    );
}

/// Codex desktop: thread open in the app, and after the app quit → open
/// `codex://threads/<id>` (Codex facts 8-9).
#[test]
fn floor_codex_desktop_live_and_quit_open_the_thread() {
    all_combos(
        "codex desktop",
        |_| true,
        |vars| {
            for quit in [false, true] {
                let mut w = clean(vars);
                let s = w
                    .start_session(StartHost::CodexDesktop, dir(0))
                    .ok_or("start")?;
                w.open_pane(TerminalKind::Ghostty, dir(1));
                if quit {
                    w.apply(&Event::QuitApp {
                        app: App::CodexDesktop,
                    });
                }
                let (r, verdict) = open(&mut w, &s, &req());
                verdict.map_err(|v| format!("{v:?}"))?;
                let ok = kind(&r.decision) == Some(ActionKind::OpenExisting)
                    && useful(&r.outcome)
                    && w.views
                        .get(&App::CodexDesktop)
                        .cloned()
                        .flatten()
                        .as_deref()
                        == Some(s.id.as_str());
                if !ok {
                    return Err(format!(
                        "quit={quit}: {:?} → {:?}",
                        r.decision.plan, r.outcome
                    ));
                }
            }
            Ok(())
        },
    );
}

/// Codex TUI in Ghostty under the 0.167.0 daemon (native N8-N10): a plain
/// `codex` TUI cannot be located, so a live open resumes as another client
/// of the daemon's single writer, labelled; the window it opens is then
/// focused (argv names the thread), and after every window closes a resume
/// is verified.
#[test]
fn floor_codex_ghostty_live_focused_and_closed_resumed() {
    all_combos(
        "codex ghostty",
        |_| true,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::CodexGhostty, dir(0))
                .ok_or("start")?;
            w.apply(&Event::Discover);
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            let labelled = r.decision.action().is_some_and(|a| {
                a.kind == ActionKind::Resume && a.note == Some(Reason::PossiblySecondView)
            });
            if !labelled || !useful(&r.outcome) || w.writers(&s) != 1 {
                return Err(format!("live: {:?} → {:?}", r.decision.plan, r.outcome));
            }
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            if kind(&r.decision) != Some(ActionKind::FocusPane) || r.outcome != Outcome::Verified {
                return Err(format!("again: {:?} → {:?}", r.decision.plan, r.outcome));
            }
            w.quit_app(App::Ghostty);
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            (kind(&r.decision) == Some(ActionKind::Resume) && r.outcome == Outcome::Verified)
                .then_some(())
                .ok_or_else(|| format!("closed: {:?} → {:?}", r.decision.plan, r.outcome))
        },
    );
}

/// OpenCode desktop: there is no session deep link (OpenCode facts C,
/// VERIFIED), so the best honest outcome is bringing the app forward; after a
/// quit, relaunching restores the last tab.
#[test]
fn floor_opencode_desktop_is_brought_forward() {
    all_combos(
        "opencode desktop",
        |_| true,
        |vars| {
            for quit in [false, true] {
                let mut w = clean(vars);
                let s = w
                    .start_session(StartHost::OpenCodeDesktop, dir(0))
                    .ok_or("start")?;
                w.open_pane(TerminalKind::Ghostty, dir(1));
                w.apply(&Event::Discover);
                if quit {
                    w.apply(&Event::QuitApp {
                        app: App::OpenCodeDesktop,
                    });
                }
                let (r, verdict) = open(&mut w, &s, &req());
                verdict.map_err(|v| format!("{v:?}"))?;
                if kind(&r.decision) != Some(ActionKind::BringForward)
                    || r.outcome != Outcome::Verified
                    || w.front_app != Some(App::OpenCodeDesktop)
                {
                    return Err(format!(
                        "quit={quit}: {:?} → {:?}",
                        r.decision.plan, r.outcome
                    ));
                }
            }
            Ok(())
        },
    );
}

/// OpenCode TUI (`opencode -s <id>`) in Terminal.app: live → focus its tab;
/// closed → `opencode -s <id>` in Terminal.app, verified.
#[test]
fn floor_opencode_tui_live_focused_and_closed_resumed() {
    all_combos(
        "opencode tui",
        |_| true,
        |vars| {
            let mut w = clean(vars);
            let s = w
                .start_session(StartHost::OpenCodeTerminal { with_id: true }, dir(0))
                .ok_or("start")?;
            w.apply(&Event::Discover);
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            if kind(&r.decision) != Some(ActionKind::FocusPane) || r.outcome != Outcome::Verified {
                return Err(format!("live: {:?} → {:?}", r.decision.plan, r.outcome));
            }
            let pane = pane_of(&w, &s);
            w.close_pane(pane);
            let (r, verdict) = open(&mut w, &s, &req());
            verdict.map_err(|v| format!("{v:?}"))?;
            let ok = matches!(r.decision.action(), Some(a) if matches!(a.steps.as_slice(),
            [Step::Launch { terminal: TerminalKind::AppleTerminal, command, .. }]
                if command.program.ends_with("/opencode")));
            (ok && r.outcome == Outcome::Verified)
                .then_some(())
                .ok_or_else(|| format!("closed: {:?} → {:?}", r.decision.plan, r.outcome))
        },
    );
}
