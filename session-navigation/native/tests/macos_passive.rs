//! A passive look (`MacObserver::set_passive`) never scripts a terminal: it
//! must be safe to run just to draw a button, so it cannot raise a macOS
//! Automation prompt.
#![cfg(feature = "macos")]
mod common;

use common::*;
use session_navigation::evidence::Fact;
use session_navigation::model::App;
use session_navigation::ports::Observe;
use session_navigation_native::macos::machine::OSASCRIPT;
use session_navigation_native::MacObserver;

/// `osascript` runs that are AppleScript (`-e`), not the JXA registry
/// (`-l JavaScript`).
fn applescript_runs(fake: &session_navigation_native::FakeSystem) -> usize {
    fake.runs()
        .into_iter()
        .filter(|(program, args)| {
            program == OSASCRIPT && args.first().map(String::as_str) != Some("-l")
        })
        .count()
}

fn ghostty_in_front() -> Machine {
    let mut m = codex_machine(Some(codex_row(false, "codex-tui")));
    m.apps_json = apps_json(Some(App::Ghostty), &[App::Ghostty]);
    m
}

#[test]
fn a_passive_look_runs_no_terminal_applescript() {
    let fake = ghostty_in_front().build();
    let mut observer = MacObserver::new(&fake, cfg());
    observer.set_passive(true);
    let snapshot = observer.observe(&codex());
    assert_eq!(applescript_runs(&fake), 0, "{}", all_args(&fake));
    assert_eq!(snapshot.front.app, Fact::Known(Some(App::Ghostty)));
    assert_eq!(snapshot.front.pane_tty, Fact::Unknown);
    // The JXA registry still answered.
    assert!(snapshot.apps.is_known());
}

#[test]
fn an_active_look_reads_the_front_pane() {
    let fake = ghostty_in_front().build();
    let mut observer = MacObserver::new(&fake, cfg());
    let snapshot = observer.observe(&codex());
    assert_eq!(applescript_runs(&fake), 1, "{}", all_args(&fake));
    assert_eq!(
        snapshot.front.pane_tty,
        Fact::Known(Some("/dev/ttys000".into()))
    );
}
