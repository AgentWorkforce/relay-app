//! Simulator and oracle regression tests for the review findings.
use std::collections::BTreeSet;

use session_navigation::engine::verify;
use session_navigation::engine::OpenReport;
use session_navigation::model::{App, Precision, Provider, SessionRef, TerminalKind, Tool};
use session_navigation::plan::{
    Action, ActionKind, Command, Decision, OpenRequest, Plan, PostCondition, SessionState, Step,
};
use session_navigation::ports::{Execute, Observe};
use session_navigation::sim::agents::StartHost;
use session_navigation::sim::events::dir;
use session_navigation::sim::labels::STABLE;
use session_navigation::sim::oracle::{self, Invariant, Violation};
use session_navigation::sim::runner::{run, shrink};
use session_navigation::sim::world::LogEntry;
use session_navigation::sim::{Event, World, WorldVars};

fn clean(vars: WorldVars) -> World {
    let mut w = World::new(11, vars);
    for app in App::ALL {
        if let Some(a) = w.apps.get_mut(&app) {
            a.installed = true;
        }
        w.automation.insert(app, true);
    }
    w.launch_delay = 0;
    w
}

fn rogue(w: &mut World, s: &SessionRef, action: Action) -> Result<(), Violation> {
    // An explicit terminal preference: there is no built-in default.
    let mut request = OpenRequest::default();
    request.prefs.default_terminal = Some(session_navigation::model::TerminalKind::Ghostty);
    let id = 9_000 + w.log.len() as u64;
    let from = w.log.len();
    let before = w.observe(s);
    w.engine_open = Some(id);
    let executed: Vec<_> = action
        .steps
        .iter()
        .map(|step| (step.clone(), w.execute(step)))
        .collect();
    w.engine_open = None;
    let after = w.observe(s);
    let outcome = verify::check(&action.post, &after, &request);
    let report = OpenReport {
        session: s.clone(),
        before,
        decision: Decision::new(SessionState::Stopped, Plan::Act(action)),
        executed,
        after: Some(after),
        outcome,
    };
    oracle::check_open(w, id, &request, &report)?;
    oracle::check_log(w, from, &BTreeSet::new())
}

fn command(program: &str, args: &[&str], cwd: &str) -> Command {
    Command {
        program: program.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: cwd.into(),
        env: Default::default(),
    }
}

/// Finding 13: `claude --resume` from another directory does not find the
/// transcript (project slug, Claude facts F) and starts nothing.
#[test]
fn f13_claude_resume_from_the_wrong_directory_presents_nothing() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::ClaudeGhostty, dir(0))
        .expect("start");
    let pane = w.panes.keys().copied().next().expect("pane");
    w.close_pane(pane);
    w.person_runs(
        TerminalKind::Ghostty,
        &command("claude", &["--resume", &s.id], dir(1)),
    );
    assert_eq!(w.writers(&s), 0);
    w.person_runs(
        TerminalKind::Ghostty,
        &command("claude", &["--resume", &s.id], dir(0)),
    );
    assert_eq!(w.writers(&s), 1);
}

/// Finding 16: An absolute path that is not where the CLI is installed, or a CLI
/// that is not installed, runs nothing.
#[test]
fn f16_sim_runs_only_installed_executables() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::CodexGhostty, dir(0))
        .expect("start");
    let pane = w.panes.keys().copied().next().expect("pane");
    w.close_pane(pane);
    w.person_runs(
        TerminalKind::Ghostty,
        &command("/nowhere/codex", &["resume", &s.id], dir(0)),
    );
    assert_eq!(w.writers(&s), 0);
    w.apply(&Event::UninstallTool {
        tool: Tool::CodexCli,
    });
    w.person_runs(
        TerminalKind::Ghostty,
        &command("codex", &["resume", &s.id], dir(0)),
    );
    assert_eq!(w.writers(&s), 0);
}

/// Finding 17: A writer that a delayed launch starts next to an owner is a duplicate.
#[test]
fn f17_oracle_judges_delayed_writer_starts() {
    let mut w = clean(WorldVars::default());
    w.log.push(LogEntry::WriterStarted {
        open: Some(1),
        session: SessionRef::new(Provider::Codex, "x"),
        others: 1,
        immediate: false,
    });
    let verdict = oracle::check_log(&w, 0, &BTreeSet::new());
    assert!(matches!(verdict, Err(v) if v.invariant == Invariant::NoDuplicates));
}

/// Finding 4, revised by native N13-N14: bringing the OpenCode desktop up
/// restores a tab a TUI is showing. That is a second client of the server,
/// the only writer, so the oracle counts no writer for it (it used to count
/// one, on the UNVERIFIED assumption that two clients are two writers).
#[test]
fn f4_restored_desktop_tab_is_a_client_not_a_writer() {
    let mut w = clean(WorldVars::default());
    let a = w
        .start_session(StartHost::OpenCodeDesktop, dir(0))
        .expect("start");
    w.apply(&Event::QuitApp {
        app: App::OpenCodeDesktop,
    });
    w.person_runs(
        TerminalKind::AppleTerminal,
        &command("opencode", &["-s", &a.id], dir(0)),
    );
    let bring = Action {
        kind: ActionKind::BringForward,
        steps: vec![Step::ActivateApp {
            app: App::OpenCodeDesktop,
        }],
        precision: Precision::AppOnly,
        post: PostCondition::AppFrontmost {
            app: App::OpenCodeDesktop,
        },
        note: None,
        import: false,
        target: None,
    };
    let verdict = rogue(&mut w, &a, bring);
    assert!(verdict.is_ok(), "{verdict:?}");
    assert_eq!(
        w.views.get(&App::OpenCodeDesktop).cloned().flatten(),
        Some(a.id.clone()),
        "precondition: the tab was restored next to the TUI"
    );
    assert_eq!(w.writers(&a), 0);
}

/// Finding 18: A stable pick follows its target's identity: dropping an unrelated
/// start does not retarget it.
#[test]
fn f18_stable_picks_survive_dropping_unrelated_events() {
    let starts = |with_first: bool| {
        let mut events = Vec::new();
        if with_first {
            events.push(Event::Start {
                host: StartHost::ClaudeGhostty,
                dir: 1,
                tag: 7,
            });
        }
        events.push(Event::Start {
            host: StartHost::CodexGhostty,
            dir: 0,
            tag: 8,
        });
        events.push(Event::Start {
            host: StartHost::OpenCodeTerminal { with_id: true },
            dir: 2,
            tag: 9,
        });
        events
    };
    for pick in 0..64u32 {
        let target = |with_first: bool| {
            let mut w = clean(WorldVars::default());
            for e in starts(with_first) {
                w.apply(&e);
            }
            w.pick_session(STABLE | pick).map(|s| s.provider)
        };
        let (full, dropped) = (target(true), target(false));
        if full != Some(Provider::Claude) {
            assert_eq!(full, dropped, "pick {pick} retargeted");
        }
    }
}

/// Finding 18: Shrinking keeps a candidate only when it fails for the same cause.
#[test]
fn f18_shrink_preserves_the_cause() {
    let violation = Violation {
        invariant: Invariant::NoDuplicates,
        cause: "NoDuplicates/no such cause/None".into(),
        detail: String::new(),
    };
    let events = vec![Event::Discover, Event::Tick { seconds: 1 }];
    assert_eq!(shrink(1, WorldVars::default(), &events, &violation), events);
    assert!(run(1, WorldVars::default(), &events).is_ok());
}

/// Finding 8: In a world where a Codex TUI can switch thread, `Clear` moves one.
#[test]
fn f8_codex_tui_switches_thread_in_that_world() {
    let vars = WorldVars {
        codex_tui_switches_thread: true,
        ..WorldVars::default()
    };
    let mut w = clean(vars);
    let s = w
        .start_session(StartHost::CodexGhostty, dir(0))
        .expect("start");
    w.apply(&Event::Clear { pick: 0 });
    assert_eq!(w.writers(&s), 0, "the TUI now writes another thread");
    assert_eq!(w.catalog.len(), 2);
}
