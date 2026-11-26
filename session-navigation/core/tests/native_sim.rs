//! Behaviour verified natively on the real apps (native N1-N19), as the
//! simulator models it, end to end through the engine and the oracle.
use std::collections::BTreeSet;

use session_navigation::engine::{self, OpenReport};
use session_navigation::evidence::{Exe, Fact, ProviderEvidence};
use session_navigation::model::{App, Precision, Provider, Reason, SessionRef, TerminalKind};
use session_navigation::plan::{
    ActionKind, Command, Dispatch, OpenRequest, Outcome, PostCondition, Step,
};
use session_navigation::ports::{Execute, Observe};
use session_navigation::sim::agents::StartHost;
use session_navigation::sim::events::dir;
use session_navigation::sim::oracle;
use session_navigation::sim::world::{LogEntry, Role};
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
    w.codex.update_available = false;
    w
}

fn req() -> OpenRequest {
    OpenRequest {
        // An explicit terminal preference: there is no built-in default.
        prefs: session_navigation::plan::Preferences {
            default_terminal: Some(session_navigation::model::TerminalKind::Ghostty),
            ..Default::default()
        },
        ..OpenRequest::default()
    }
}

/// Open through the engine with attribution; the oracle must accept it.
fn open(w: &mut World, s: &SessionRef) -> OpenReport {
    let id = 7_000 + w.log.len() as u64;
    let from = w.log.len();
    w.engine_open = Some(id);
    let report = engine::open(w, s, &req());
    w.engine_open = None;
    oracle::check_open(w, id, &req(), &report).expect("oracle");
    oracle::check_log(w, from, &BTreeSet::new()).expect("oracle log");
    report
}

fn command(program: &str, args: &[&str], cwd: &str) -> Command {
    Command {
        program: program.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: cwd.into(),
        env: Default::default(),
    }
}

fn kind(r: &OpenReport) -> Option<ActionKind> {
    r.decision.action().map(|a| a.kind)
}

fn launches(w: &World) -> usize {
    w.log
        .iter()
        .filter(|e| matches!(e, LogEntry::LaunchDispatched { open: Some(_), .. }))
        .count()
}

fn every_world() -> impl Iterator<Item = WorldVars> {
    (0..1u32 << WorldVars::LEGACY)
        .step_by(37)
        .map(WorldVars::from_bits)
}

// ------------------------------------------------------------ Claude.app ---

fn claude_record(w: &World, s: &SessionRef) -> session_navigation::sim::world::DesktopSession {
    w.claude
        .desktop
        .values()
        .find(|d| d.holds_live(&s.id))
        .cloned()
        .expect("record")
}

/// N2: archiving stops the record's process and deletes its registry file.
#[test]
fn n2_archive_stops_the_process_and_deletes_its_registry_file() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::ClaudeDesktop, dir(0))
        .expect("start");
    let child = w
        .procs
        .values()
        .find(|p| matches!(p.role, Role::DesktopChild(_)))
        .map(|p| p.pid)
        .expect("child");
    assert!(w.claude.registry.contains_key(&child));
    w.apply(&Event::Archive { pick: 0 });
    assert!(claude_record(&w, &s).archived);
    assert!(!w.procs.contains_key(&child), "process exited");
    assert!(
        !w.claude.registry.contains_key(&child),
        "registry file deleted"
    );
}

/// N3: `continue` on an archived record changes nothing on the record.
#[test]
fn n3_continue_on_an_archived_record_leaves_it_untouched() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::ClaudeDesktop, dir(0))
        .expect("start");
    w.apply(&Event::Archive { pick: 0 });
    let before = claude_record(&w, &s);
    let count = w.claude.desktop.len();
    let step = Step::OpenUrl {
        url: format!("claude://code/continue?session={}", before.local),
        bundle: App::ClaudeDesktop.bundle_id().into(),
    };
    assert_eq!(w.execute(&step), Dispatch::Dispatched);
    assert_eq!(claude_record(&w, &s), before);
    assert_eq!(w.claude.desktop.len(), count);
}

/// N5: on the verified app, `resume` revives the same record in every
/// world: same record and `cliSessionId`, no new record, no
/// `unarchivedCliSessionId`, `lastFocusedAt` moved, and the engine verifies
/// it from that signal.
#[test]
fn n5_resume_link_revives_the_same_record_in_every_world() {
    for vars in every_world() {
        let mut w = clean(vars);
        let s = w
            .start_session(StartHost::ClaudeDesktop, dir(0))
            .expect("start");
        w.apply(&Event::Archive { pick: 0 });
        let before = claude_record(&w, &s);
        let count = w.claude.desktop.len();
        let r = open(&mut w, &s);
        assert_eq!(kind(&r), Some(ActionKind::Reopen), "{vars:?}");
        assert_eq!(r.outcome, Outcome::Verified, "{vars:?}");
        let after = claude_record(&w, &s);
        assert_eq!(after.local, before.local);
        assert_eq!(after.current, before.current);
        assert_eq!(after.unarchived, None);
        assert!(!after.archived);
        assert!(after.last_focused > before.last_focused);
        assert_eq!(w.claude.desktop.len(), count, "no new record");
    }
}

/// N1: a `continue` link on a live record moves `lastFocusedAt`, and that
/// alone verifies it, in every world (with or without a process start).
#[test]
fn n1_continue_link_is_verified_in_every_world() {
    for vars in every_world() {
        let mut w = clean(vars);
        let s = w
            .start_session(StartHost::ClaudeDesktop, dir(0))
            .expect("start");
        w.apply(&Event::QuitApp {
            app: App::ClaudeDesktop,
        });
        let r = open(&mut w, &s);
        assert_eq!(kind(&r), Some(ActionKind::OpenExisting), "{vars:?}");
        assert_eq!(r.outcome, Outcome::Verified, "{vars:?}");
    }
}

/// The record's `lastFocusedAt` reaches `decide` and `verify` through the
/// real parser.
#[test]
fn n1_sim_reports_last_focused_through_the_parser() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::ClaudeDesktop, dir(0))
        .expect("start");
    let snap = w.observe(&s);
    let ProviderEvidence::Claude(ev) = snap.provider else {
        panic!("claude evidence");
    };
    let claims = ev.desktop.known().cloned().expect("desktop");
    let expected = claude_record(&w, &s).last_focused;
    assert!(expected.is_some());
    assert_eq!(
        claims.first().map(|c| c.last_focused),
        Some(Fact::Known(expected))
    );
}

// ---------------------------------------------------------- Codex daemon ---

/// N8-N10: two TUIs on one thread are two clients of one writer, the
/// daemon, which alone holds the lock.
#[test]
fn n10_second_codex_client_is_not_a_second_writer() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::CodexGhostty, dir(0))
        .expect("start");
    w.apply(&Event::PersonResume { pick: 0 });
    w.apply(&Event::PersonResume { pick: 0 });
    let clients = w
        .procs
        .values()
        .filter(|p| p.role == Role::Interactive && p.hosts.as_ref() == Some(&s))
        .count();
    assert_eq!(clients, 3);
    assert_eq!(w.writers(&s), 1);
    let ProviderEvidence::Codex(ev) = w.observe(&s).provider else {
        panic!("codex evidence");
    };
    assert_eq!(
        ev.lock_holders,
        Fact::Known(w.daemon_pid().into_iter().collect())
    );
}

/// N9-N10: the engine resumes next to an unlocatable plain TUI, labelled;
/// the oracle sees one writer; the next open focuses the new window.
#[test]
fn n10_engine_second_view_then_focuses_its_own_window() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::CodexGhostty, dir(0))
        .expect("start");
    let r = open(&mut w, &s);
    let note = r.decision.action().and_then(|a| a.note);
    assert_eq!(
        (kind(&r), note),
        (Some(ActionKind::Resume), Some(Reason::PossiblySecondView))
    );
    assert_eq!(
        r.outcome,
        Outcome::DispatchedUnverified,
        "same lock holder as before"
    );
    assert_eq!(w.writers(&s), 1);
    let again = open(&mut w, &s);
    assert_eq!(kind(&again), Some(ActionKind::FocusPane));
    assert_eq!(again.outcome, Outcome::Verified);
    assert_eq!(launches(&w), 1);
}

/// The oracle catches a launch next to a window that shows the thread and
/// names it in argv: a needless second client.
#[test]
fn n10_oracle_flags_a_needless_second_client() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::CodexGhostty, dir(0))
        .expect("start");
    w.apply(&Event::PersonResume { pick: 0 });
    let id = 9_000;
    let from = w.log.len();
    w.engine_open = Some(id);
    let program = "/opt/homebrew/bin/codex";
    let step = Step::Launch {
        terminal: TerminalKind::Ghostty,
        command: command(program, &["resume", &s.id], dir(0)),
        context: session_navigation::plan::LaunchContext::native(),
    };
    w.execute(&step);
    w.engine_open = None;
    let verdict = oracle::check_log(&w, from, &BTreeSet::new());
    assert!(
        matches!(&verdict, Err(v) if v.cause.contains("needless second client")),
        "{verdict:?}"
    );
}

/// N12: a launched TUI stalls at the update menu: dispatched, not verified,
/// not launched twice; once dismissed it presents the thread.
#[test]
fn n12_update_menu_stalls_a_launch_without_a_double_launch() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::CodexGhostty, dir(0))
        .expect("start");
    w.quit_app(App::Ghostty);
    w.codex.update_available = true;
    let r = open(&mut w, &s);
    assert_eq!(kind(&r), Some(ActionKind::Resume));
    assert_eq!(r.outcome, Outcome::DispatchedUnverified);
    assert_eq!(w.writers(&s), 0, "nothing presents the thread yet");
    let again = open(&mut w, &s);
    assert_eq!(
        kind(&again),
        Some(ActionKind::FocusPane),
        "{:?}",
        again.decision
    );
    assert_eq!(launches(&w), 1, "no second launch");
    w.apply(&Event::DismissUpdate { pick: 0 });
    assert_eq!(w.writers(&s), 1);
    let third = open(&mut w, &s);
    assert_eq!(kind(&third), Some(ActionKind::FocusPane));
    assert_eq!(third.outcome, Outcome::Verified);
}

// --------------------------------------------------------------- OpenCode ---

/// N13-N14: a resume next to a plain TUI adds a client, not a writer; a
/// launch next to a running turn is still a duplicate to the oracle.
#[test]
fn n14_opencode_clients_are_not_writers_but_turns_are() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::OpenCodeDesktop, dir(0))
        .expect("start");
    w.quit_app(App::OpenCodeDesktop);
    w.start_session(StartHost::OpenCodeTerminal { with_id: false }, dir(0));
    let mut request = req();
    request.use_default_terminal = true;
    let id = 7_500;
    w.engine_open = Some(id);
    let r = engine::open(&mut w, &s, &request);
    w.engine_open = None;
    oracle::check_open(&w, id, &request, &r).expect("oracle");
    oracle::check_log(&w, 0, &BTreeSet::new()).expect("oracle log");
    assert_eq!(
        r.decision.action().and_then(|a| a.note),
        Some(Reason::PossiblySecondView)
    );
    assert_eq!(w.writers(&s), 0);
    w.apply(&Event::Start {
        host: StartHost::Headless {
            provider: Provider::OpenCode,
            supervised: false,
        },
        dir: 0,
        tag: 1,
    });
    let busy = w.catalog.last().cloned().expect("busy session");
    assert_eq!(w.writers(&busy), 1, "a running turn is a writer");
}

// ---------------------------------------------------------------- Ghostty ---

/// N15, N18: the observed chain. A person's pane is ghostty → login (euid
/// 0, ruid 501) → shell → agent; a launched command runs under login
/// directly, whose argv carries the command; the launch is still verified
/// without the terminal's pid or working directory.
#[test]
fn n18_ghostty_launch_runs_under_login_and_is_verified() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::ClaudeGhostty, dir(0))
        .expect("start");
    let login = w
        .procs
        .values()
        .find(|p| p.exe == Exe::Login)
        .cloned()
        .expect("login");
    assert_eq!((login.euid, login.ruid), (0, 501));
    w.quit_app(App::Ghostty);
    let r = open(&mut w, &s);
    assert_eq!(kind(&r), Some(ActionKind::Resume));
    assert_eq!(r.outcome, Outcome::Verified);
    let agent = w
        .procs
        .values()
        .find(|p| p.hosts.as_ref() == Some(&s))
        .cloned()
        .expect("agent");
    let parent = w.procs.get(&agent.ppid).cloned().expect("parent");
    assert_eq!(parent.exe, Exe::Login);
    assert!(parent
        .argv
        .iter()
        .any(|a| a.starts_with("exec -l ") && a.contains(&s.id)));
    let precision = r.decision.action().map(|a| a.precision);
    assert_eq!(precision, Some(Precision::ExactConversation));
}

/// N16-N17: focus finds the pane by tty only and reads it back.
#[test]
fn n17_focus_by_tty_is_read_back() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::ClaudeGhostty, dir(0))
        .expect("start");
    w.open_pane(TerminalKind::Ghostty, dir(1));
    let r = open(&mut w, &s);
    assert!(matches!(
        r.decision.action().map(|a| &a.post),
        Some(PostCondition::PaneFocused { .. })
    ));
    assert_eq!(r.outcome, Outcome::Verified);
    let unknown = Step::FocusPane {
        terminal: TerminalKind::Ghostty,
        pane: session_navigation::hosts::PaneRef::Tty {
            tty: "/dev/ttys999".into(),
        },
        process: r
            .before
            .processes
            .known()
            .and_then(|t| t.first())
            .map(|p| p.identity)
            .expect("p"),
    };
    assert_eq!(w.execute(&unknown), Dispatch::Failed(Reason::TargetGone));
}

/// Claude.app applies a link asynchronously: an observer that sees the
/// record one observation late still gets the `lastFocusedAt` signal within
/// the engine's bounded re-observation.
struct Lagging {
    w: World,
    stale: Option<session_navigation::evidence::Snapshot>,
    late: usize,
}

impl Observe for Lagging {
    fn observe(&mut self, s: &SessionRef) -> session_navigation::evidence::Snapshot {
        match (self.late, &self.stale) {
            (n, Some(stale)) if n > 0 => {
                self.late -= 1;
                stale.clone()
            }
            _ => {
                let fresh = self.w.observe(s);
                self.stale = Some(fresh.clone());
                fresh
            }
        }
    }
}

impl Execute for Lagging {
    fn execute(&mut self, step: &Step) -> Dispatch {
        self.late = self.late.max(1);
        self.w.execute(step)
    }
}

impl session_navigation::ports::Reservations for Lagging {
    fn reserve(
        &mut self,
        r: session_navigation::evidence::Reservation,
    ) -> Result<(), session_navigation::ports::StoreError> {
        self.w.reserve(r)
    }
    fn release(&mut self, s: &SessionRef) -> Result<(), session_navigation::ports::StoreError> {
        self.w.release(s)
    }
}

#[test]
fn n1_link_signal_is_awaited_a_bounded_number_of_times() {
    let mut w = clean(WorldVars::default());
    let s = w
        .start_session(StartHost::ClaudeDesktop, dir(0))
        .expect("start");
    w.apply(&Event::QuitApp {
        app: App::ClaudeDesktop,
    });
    let mut lagging = Lagging {
        w,
        stale: None,
        late: 0,
    };
    let r = engine::open(&mut lagging, &s, &req());
    assert_eq!(kind(&r), Some(ActionKind::OpenExisting));
    assert_eq!(r.outcome, Outcome::Verified);
    // An app that never focuses the record stays unverified, not failed.
    let mut never = Lagging {
        late: usize::MAX,
        ..lagging
    };
    never.stale = Some(never.w.observe(&s));
    let r = engine::open(&mut never, &s, &req());
    assert_ne!(r.outcome, Outcome::Verified);
    assert_eq!(engine::MAX_LINK_OBSERVATIONS, 3);
}
