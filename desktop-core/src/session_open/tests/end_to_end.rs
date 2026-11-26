//! One click through each platform adapter on a `FakeSystem` (nothing
//! answers, so every adapter must refuse without acting), the click's
//! answer for each outcome, and the one-open-at-a-time guard.
use std::path::Path;

use session_navigation::engine::OpenReport;
use session_navigation::model::Reason;
use session_navigation::plan::{Decision, Outcome as CoreOutcome, Plan, SessionState};
use session_navigation_native::system::join;
use session_navigation_native::{
    FakeSystem, LinuxConfig, LinuxPorts, MacPorts, ProviderPaths, WindowsConfig, WindowsPorts,
};

use super::support::*;
use crate::session_open::contract::{OpenResult, Outcome, Request};
use crate::session_open::legacy::GTK_FILE;
use crate::session_open::native::reservation_dir;
use crate::session_open::{open_with, plan_with, view, wording};

enum Os {
    Mac,
    Linux,
    Windows,
}

fn paths(os: &Os, app: &str) -> ProviderPaths {
    let mut paths = match os {
        Os::Mac => mac_cfg(app).common.paths,
        Os::Linux => LinuxConfig::for_user("/home/example", 1000).common.paths,
        Os::Windows => windows().common.paths,
    };
    paths.reservation_dir = reservation_dir(app);
    paths
}

fn windows() -> WindowsConfig {
    WindowsConfig::native(
        r"C:\Windows",
        r"C:\Users\example",
        r"C:\Users\example\AppData\Roaming",
        r"C:\Users\example\AppData\Local",
    )
    .unwrap()
}

fn click(os: &Os, fake: &FakeSystem, request: &Request) -> OpenResult {
    let app = request.app_data_dir.clone();
    let paths = paths(os, &app);
    match os {
        Os::Mac => open_with(fake, &paths, None, request, |h| {
            MacPorts::with_history(fake, mac_cfg(&app), h)
        }),
        Os::Linux => open_with(fake, &paths, None, request, |h| {
            let mut cfg = LinuxConfig::for_user("/home/example", 1000);
            cfg.common.paths.reservation_dir = reservation_dir(&app);
            LinuxPorts::with_history(fake, cfg, h)
        }),
        Os::Windows => open_with(fake, &paths, None, request, |h| {
            let mut cfg = windows();
            cfg.common.paths.reservation_dir = reservation_dir(&app);
            WindowsPorts::with_history(fake, cfg, h)
        }),
    }
}

fn app_dir() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    (dir, path)
}

fn refuses_without_acting(os: Os) {
    let (_dir, app) = app_dir();
    for (source, id) in [("claude", CLAUDE_ID), ("codex", CODEX_ID)] {
        let fake = FakeSystem::new();
        let result = click(&os, &fake, &request(source, id, &app));
        assert!(
            matches!(result.outcome, Outcome::Refused | Outcome::Failed),
            "{result:?}"
        );
        let code = result.reason_code.clone().unwrap();
        // Nothing answers, so nothing is known: refused before any step.
        assert_eq!(code, "evidence_incomplete", "{source}");
        assert_eq!(
            result.plan.state,
            crate::session_open::contract::State::Unknown
        );
        assert!(!result.message.is_empty());
        assert!(
            !result.plan.enabled
                || result.plan.action != crate::session_open::contract::Action::Resume
        );
        assert!(actions(&fake).is_empty(), "{:?}", actions(&fake));
        assert!(
            fake.files_under(&reservation_dir(&app)).is_empty(),
            "{code}"
        );
    }
}

#[test]
fn macos_adapter_refuses_without_acting() {
    refuses_without_acting(Os::Mac);
}

#[test]
fn linux_adapter_refuses_without_acting() {
    refuses_without_acting(Os::Linux);
}

#[test]
fn windows_adapter_refuses_without_acting() {
    refuses_without_acting(Os::Windows);
}

#[test]
fn a_legacy_pending_resume_survives_the_upgrade_on_every_adapter() {
    for os in [Os::Mac, Os::Linux, Os::Windows] {
        let (_dir, app) = app_dir();
        let fake = FakeSystem::new();
        fake.add_file(
            &join(&app, GTK_FILE),
            format!(r#"["claude:{CLAUDE_ID}"]"#),
            1,
        );
        let result = click(&os, &fake, &request("claude", CLAUDE_ID, &app));
        assert!(result.plan.pending_launch, "{result:?}");
        // Still there: nothing showed the earlier launch resolved.
        assert!(fake.file(&join(&app, GTK_FILE)).unwrap().len() > 2);
        let paths = paths(&os, &app);
        let view = plan_with(
            &fake,
            &paths,
            None,
            &request("claude", CLAUDE_ID, &app),
            |_| Fixed::new(snapshot(&claude())),
        );
        assert!(view.pending_launch);
    }
}

#[test]
fn a_second_concurrent_open_of_one_session_is_refused() {
    let (_dir, app) = app_dir();
    let locks = Path::new(&app).join("session-navigation/opening");
    let held = crate::session_open::guard::begin(&locks, &claude()).unwrap();
    assert_eq!(
        crate::session_open::guard::begin(&locks, &claude()).unwrap_err(),
        crate::session_open::guard::Refusal::Busy
    );
    let fake = FakeSystem::new();
    let result = click(&Os::Mac, &fake, &request("claude", CLAUDE_ID, &app));
    assert_eq!(result.outcome, Outcome::Refused);
    assert_eq!(result.reason_code.as_deref(), Some("already_opening"));
    assert_eq!(result.message, "This session is already being opened.");
    // Another session is not held up.
    let other = request("codex", CODEX_ID, &app);
    assert_ne!(
        click(&Os::Mac, &fake, &other).reason_code.as_deref(),
        Some("already_opening")
    );
    drop(held);
    assert!(crate::session_open::guard::begin(&locks, &claude()).is_ok());
}

fn report(decision: Decision, outcome: CoreOutcome) -> OpenReport {
    OpenReport {
        session: claude(),
        before: snapshot(&claude()),
        decision,
        executed: Vec::new(),
        after: None,
        outcome,
    }
}

#[test]
fn each_engine_outcome_becomes_one_answer() {
    let after = view::refused(wording::Local::SeveralLocations);
    let blocked = Decision::unavailable(SessionState::Stopped, Reason::ToolNotInstalled);
    let r = view::open(
        &report(
            blocked.clone(),
            CoreOutcome::Blocked(Reason::ToolNotInstalled),
        ),
        after.clone(),
    );
    assert_eq!(r.outcome, Outcome::Refused);
    assert_eq!(r.reason_code.as_deref(), Some("tool_not_installed"));
    assert_eq!(r.message, wording::reason(Reason::ToolNotInstalled).1);
    let r = view::open(
        &report(blocked.clone(), CoreOutcome::Failed(Reason::TargetGone)),
        after.clone(),
    );
    assert_eq!(
        (r.outcome, r.reason_code.as_deref()),
        (Outcome::Failed, Some("target_gone"))
    );
    let choose = Decision::new(SessionState::LiveMultiple, Plan::Choose(Vec::new()));
    let r = view::open(&report(choose, CoreOutcome::NeedsChoice), after.clone());
    assert_eq!(
        (r.outcome, r.reason_code.as_deref()),
        (Outcome::Refused, Some("several_locations"))
    );
    let r = view::open(
        &report(blocked.clone(), CoreOutcome::DispatchedUnverified),
        after.clone(),
    );
    assert_eq!(r.outcome, Outcome::DispatchedUnverified);
    assert_eq!(r.message, wording::unverified::RESUME);
    assert_eq!(r.reason_code, None);
    let r = view::open(&report(blocked, CoreOutcome::Verified), after);
    assert_eq!(r.outcome, Outcome::Verified);
}
