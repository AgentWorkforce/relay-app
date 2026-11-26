//! `engine::open` over `WindowsPorts<FakeSystem>` on a fake Windows
//! machine: every scenario runs observe → decide → reserve → execute →
//! observe through the real adapter code and its scripted helper runs.
#![cfg(feature = "windows")]
mod windows_support;

use serde_json::json;
use session_navigation::engine::{self, OpenReport};
use session_navigation::evidence::Fact;
use session_navigation::hosts::{PaneRef, WindowId};
use session_navigation::model::{
    Host, Precision, ProcessIdentity, Reason, SessionRef, TerminalKind,
};
use session_navigation::plan::{ActionKind, Dispatch, OpenRequest, Outcome, Plan, Step};
use session_navigation_native::system::{Call, FakeSystem};
use session_navigation_native::windows::execute::{focus_run, identity_run, launch_run};
use session_navigation_native::windows::launch::LaunchSpec;
use session_navigation_native::windows::WindowsPorts;
use session_navigation_native::HostHistory;
use windows_support::*;

/// What background discovery recorded for the session.
struct History {
    last: Fact<Host>,
    profile: Fact<Option<String>>,
}

impl HostHistory for History {
    fn last_seen(&self, _: &SessionRef) -> Fact<Host> {
        self.last
    }
    fn terminal_profile(&self, _: &SessionRef) -> Fact<Option<String>> {
        self.profile.clone()
    }
}

fn history(host: Option<TerminalKind>, profile: Option<&str>) -> History {
    History {
        last: host.map_or(Fact::Unknown, |t| Fact::Known(Host::Terminal(t))),
        profile: Fact::Known(profile.map(str::to_owned)),
    }
}

fn open(m: &Machine, fake: &FakeSystem, h: History) -> OpenReport {
    let mut ports = WindowsPorts::with_history(fake, m.cfg.clone(), h);
    engine::open(&mut ports, &claude(), &OpenRequest::default())
}

fn any_launch() -> LaunchSpec {
    LaunchSpec {
        file: String::new(),
        line: String::new(),
        cwd: String::new(),
        env: Default::default(),
    }
}

fn focus_count(m: &Machine, fake: &FakeSystem) -> usize {
    let any = focus_run(&m.cfg, WindowId(0), &identity(0, 0), &identity(0, 0));
    count(fake, &any)
}

const CONSOLE_HWND: i64 = 0x0002_0010;
const WT_HWND: i64 = 0x0003_0020;

/// A live Claude CLI in a classic console; `focus` is the focus script's
/// answer.
fn console_machine(focus: &'static str) -> (Machine, ProcessIdentity, ProcessIdentity) {
    let mut procs = base_procs();
    procs.extend(console_chain());
    let mut before = State::new(procs);
    let cmd = before.proc(3000).clone();
    let claude = before.proc(3002).clone();
    // The console window is reported as owned by its first client.
    before.windows = vec![Win::of(CONSOLE_HWND, &cmd)];
    let mut after = before.clone();
    after.foreground = Some(Win::of(CONSOLE_HWND, &cmd));
    let mut m = Machine::new(before);
    m.states.push(after);
    let home = m.claude_home();
    m.transcript(&home, PROJECT)
        .registry(&home, claude.pid, claude.start);
    m.focus = Some(focus);
    (m, cmd.id(), claude.id())
}

#[test]
fn a_live_session_in_a_classic_console_raises_its_window_and_verifies() {
    let (m, cmd, claude) = console_machine("focused");
    let fake = m.build();
    let report = open(&m, &fake, history(None, None));
    let action = report.decision.action().expect("an action");
    assert_eq!(action.kind, ActionKind::FocusPane);
    let step = Step::FocusPane {
        terminal: TerminalKind::ConsoleHost,
        pane: PaneRef::WindowsWindow {
            hwnd: WindowId(CONSOLE_HWND as u64),
            owner: cmd,
        },
        process: claude,
    };
    assert_eq!(report.executed, vec![(step, Dispatch::Dispatched)]);
    // Claude terminal focus is never more than the right window.
    assert_eq!(action.precision, Precision::RightWindow);
    assert_eq!(report.outcome, Outcome::Verified);
    // The identities were re-read before the raise, then handed to it.
    assert_eq!(count(&fake, &identity_run(&m.cfg)), 1);
    let any = focus_run(&m.cfg, WindowId(0), &identity(0, 0), &identity(0, 0));
    assert_eq!(
        inputs(&fake, &any),
        vec![json!({
            "hwnd": CONSOLE_HWND, "owner": cmd.pid, "ownerStart": cmd.start,
            "process": claude.pid, "processStart": claude.start,
        })]
    );
}

#[test]
fn a_refused_foreground_fails_honestly_and_never_launches() {
    let (m, _, _) = console_machine("refused");
    let fake = m.build();
    let report = open(&m, &fake, history(Some(TerminalKind::ConsoleHost), None));
    assert_eq!(report.outcome, Outcome::Failed(Reason::DispatchFailed));
    assert_eq!(report.executed.len(), 1);
    assert_eq!(count(&fake, &launch_run(&m.cfg, &any_launch())), 0);
}

fn wt_machine(windows: usize) -> (Machine, ProcessIdentity) {
    let mut procs = base_procs();
    procs.extend(wt_chain(4100));
    let mut before = State::new(procs);
    let wt = before.proc(4000).clone();
    let claude = before.proc(4100).clone();
    before.windows = (0..windows)
        .map(|i| Win::of(WT_HWND + i64::try_from(i).unwrap_or(0) * 16, &wt))
        .collect();
    let mut after = before.clone();
    after.foreground = Some(Win::of(WT_HWND, &wt));
    let mut m = Machine::new(before);
    m.states.push(after);
    let home = m.claude_home();
    m.transcript(&home, PROJECT)
        .registry(&home, claude.pid, claude.start);
    m.focus = Some("focused");
    (m, wt.id())
}

#[test]
fn a_live_session_in_windows_terminal_with_one_window_is_app_level() {
    let (m, wt) = wt_machine(1);
    let fake = m.build();
    let report = open(&m, &fake, history(None, None));
    let action = report.decision.action().expect("an action");
    assert_eq!(action.precision, Precision::AppOnly);
    assert_eq!(action.note, Some(Reason::PaneSelectionUnavailable));
    assert!(matches!(
        action.steps.first(),
        Some(Step::FocusPane {
            terminal: TerminalKind::WindowsTerminal,
            pane: PaneRef::WindowsWindow { owner, .. },
            ..
        }) if owner == &wt
    ));
    assert_eq!(report.outcome, Outcome::Verified);
}

#[test]
fn two_unidentifiable_terminal_windows_are_never_picked_from() {
    let (m, _) = wt_machine(2);
    let fake = m.build();
    let report = open(&m, &fake, history(None, None));
    assert_eq!(
        report.decision.plan,
        Plan::Unavailable(Reason::WindowAmbiguous)
    );
    assert!(report.executed.is_empty());
    assert_eq!(focus_count(&m, &fake), 0);
    assert_eq!(count(&fake, &identity_run(&m.cfg)), 0);
}

#[test]
fn a_reused_pid_with_another_creation_time_is_not_focused() {
    // The registry names pid 3002 of an earlier incarnation; the claude.exe
    // now on that pid started later.
    let (mut m, _, claude) = console_machine("focused");
    let home = m.claude_home();
    m.files.retain(|(p, _, _)| !p.ends_with("3002.json"));
    m.registry(&home, claude.pid, claude.start - 50);
    let fake = m.build();
    let report = open(&m, &fake, history(None, None));
    assert!(!report
        .executed
        .iter()
        .any(|(s, _)| matches!(s, Step::FocusPane { .. })));
    assert_eq!(focus_count(&m, &fake), 0);

    // The window's owner pid was reused after the table was read: the
    // window belongs to nobody, so it is not raised either.
    let (mut m, _) = wt_machine(1);
    for state in &mut m.states {
        for w in &mut state.windows {
            w.created += 30;
        }
    }
    let fake = m.build();
    let report = open(&m, &fake, history(None, None));
    // No window is its; with no app activation either, the core names
    // the host unsupported.
    assert_eq!(
        report.decision.plan,
        Plan::Unavailable(Reason::HostUnsupported)
    );
    assert_eq!(focus_count(&m, &fake), 0);
}

#[test]
fn a_stopped_session_resumes_in_windows_terminal_with_its_profile() {
    let mut before = State::new(base_procs());
    before.foreground = None;
    // After the launch: the new tab's claude.exe in the joined window.
    let mut procs = base_procs();
    procs.extend(wt_chain(7000));
    let mut after = State::new(procs);
    let wt = after.proc(4000).clone();
    let claude = after.proc(7000).clone();
    after.windows = vec![Win::of(WT_HWND, &wt)];
    after.foreground = Some(Win::of(WT_HWND, &wt));
    let mut m = Machine::new(before);
    m.states.push(after);
    let home = m.claude_home();
    // The registry file is written by the CLI the launch starts.
    m.transcript(&home, PROJECT)
        .registry(&home, claude.pid, claude.start);
    m.launch = Some("dispatched");
    let fake = m.build();
    let report = open(
        &m,
        &fake,
        history(Some(TerminalKind::WindowsTerminal), Some(PROFILE_GUID)),
    );
    let action = report.decision.action().expect("a resume");
    assert_eq!(action.kind, ActionKind::Resume);
    let Some(Step::Launch { context, .. }) = action.steps.first() else {
        panic!("expected a launch: {:?}", report.decision);
    };
    assert_eq!(context.profile.as_deref(), Some(PROFILE_GUID));
    let line = format!(
        r"-w 0 new-tab --profile {PROFILE_GUID} --startingDirectory {PROJECT} -- {CLAUDE_EXE} --resume {CLAUDE_ID}"
    );
    assert_eq!(
        inputs(&fake, &launch_run(&m.cfg, &any_launch())),
        vec![json!({"file": m.cfg.wt, "line": line, "cwd": PROFILE, "env": {}})]
    );
    // A reservation was written before the launch (and released once the
    // resume verified).
    let calls = fake.calls();
    let reserved = calls
        .iter()
        .position(|c| matches!(c, Call::WriteAtomic(p) if p.contains("reservations")));
    let launched = calls.iter().position(
        |c| matches!(c, Call::Run { args, .. } if *args == launch_run(&m.cfg, &any_launch()).args),
    );
    assert!(reserved.is_some() && reserved < launched);
    assert_eq!(report.outcome, Outcome::Verified);
}

#[test]
fn a_stopped_session_without_a_known_profile_is_not_launched_in_windows_terminal() {
    let mut m = Machine::new(State::new(base_procs()));
    let home = m.claude_home();
    m.transcript(&home, PROJECT);
    m.launch = Some("dispatched");
    let fake = m.build();
    let h = History {
        last: Fact::Known(Host::Terminal(TerminalKind::WindowsTerminal)),
        profile: Fact::Unknown,
    };
    let report = open(&m, &fake, h);
    assert_eq!(
        report.decision.plan,
        Plan::Unavailable(Reason::EvidenceIncomplete)
    );
    assert_eq!(count(&fake, &launch_run(&m.cfg, &any_launch())), 0);
}

#[test]
fn a_stopped_wsl_session_resumes_inside_its_distribution() {
    let mut m = Machine::new(State::new(base_procs()));
    m.cfg = wsl_cfg();
    let home = m.claude_home();
    m.transcript(&home, "/home/dev/code/project");
    m.dirs
        .push(r"\\wsl.localhost\Ubuntu\home\dev\code\project".to_owned());
    m.launch = Some("dispatched");
    let fake = m.build();
    let table = wsl_table(&[
        wsl_proc(1, 0, "/init", &["/init"], 0, 5),
        wsl_proc(310, 1, "/usr/bin/bash", &["-bash"], 136 << 8, 12_000),
    ]);
    let tools = String::from_utf8(fixture("wsl-tools.txt")).unwrap_or_default();
    script_wsl(&fake, &table, &tools);
    let report = open(&m, &fake, history(Some(TerminalKind::ConsoleHost), None));
    let action = report.decision.action().expect("a resume");
    assert_eq!(action.kind, ActionKind::Resume);
    let line = format!(
        "-d {DISTRO} --cd /home/dev/code/project --exec /home/dev/.local/bin/claude --resume {CLAUDE_ID}"
    );
    assert_eq!(
        inputs(&fake, &launch_run(&m.cfg, &any_launch())),
        vec![json!({"file": m.cfg.wsl_exe(), "line": line, "cwd": PROFILE, "env": {}})]
    );
    // A process inside WSL is never in this machine's windows.
    assert_eq!(report.outcome, Outcome::DispatchedUnverified);
    let snap = report.after.expect("observed again");
    assert!(matches!(
        snap.runtime,
        Fact::Known(session_navigation::model::Runtime::Wsl { .. })
    ));
}
