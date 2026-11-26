//! The Linux observer's fast look: reads at once say what they say one
//! after another, slow-changing facts and environment identity keys are
//! reused only within the window, and everything else is read every look.
#![cfg(feature = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod linux_common;

use std::time::Duration;

use linux_common::scenes::*;
use linux_common::*;
use session_navigation::ports::Observe;
use session_navigation_native::linux::LinuxObserver;
use session_navigation_native::system::{Call, FakeSystem};

fn kitty_x11() -> FakeSystem {
    let fake = kitty_machine(Some("x11")).build();
    grok_files(&fake, &[(2601, OPENED_AT)]);
    script_x11(&fake, Some("x11"), &[KITTY_WINDOW]);
    fake.on(&kitty_ls_run(), vec![ok(fixture("kitty-ls.json"))]);
    fake
}

fn count(fake: &FakeSystem, wanted: impl Fn(&Call) -> bool) -> usize {
    fake.calls().iter().filter(|c| wanted(c)).count()
}

fn environ(c: &Call) -> bool {
    matches!(c, Call::ReadFile(p) if p.ends_with("/environ"))
}

fn wmctrl(c: &Call) -> bool {
    matches!(c, Call::Run { program, .. } if program.ends_with("wmctrl"))
}

fn tool_stat(c: &Call) -> bool {
    matches!(c, Call::Metadata(p) if p.ends_with("/grok"))
}

#[test]
fn reads_at_once_say_what_reads_in_turn_say() {
    let slow = LinuxObserver::new(&kitty_x11(), cfg(Some("x11"))).observe(&grok());
    let mut fake = kitty_x11();
    fake.set_concurrent(true);
    let quick = LinuxObserver::new(&fake, cfg(Some("x11"))).observe(&grok());
    assert_eq!(quick, slow);
}

#[test]
fn a_second_look_reuses_only_what_cannot_change_within_a_click() {
    let fake = kitty_x11();
    let mut observer = LinuxObserver::new(&fake, cfg(Some("x11")));
    let first = observer.observe(&grok());
    let environs = count(&fake, environ);
    assert!(environs > 0);
    let (windows, stats) = (count(&fake, wmctrl), count(&fake, tool_stat));
    let second = observer.observe(&grok());
    assert_eq!(first, second);
    // Environments and tool paths were not read again; the window list
    // (evidence of now) was.
    assert_eq!(count(&fake, environ), environs);
    assert_eq!(count(&fake, tool_stat), stats);
    assert_eq!(count(&fake, wmctrl), 2 * windows);

    // Without reuse, everything is read again.
    let fake = kitty_x11();
    let mut observer = LinuxObserver::new(&fake, cfg(Some("x11")));
    observer.set_reuse(Duration::ZERO);
    let _ = observer.observe(&grok());
    let environs = count(&fake, environ);
    let _ = observer.observe(&grok());
    assert_eq!(count(&fake, environ), 2 * environs);
}

#[test]
fn a_new_process_instance_gets_its_environment_read() {
    let fake = kitty_x11();
    let mut observer = LinuxObserver::new(&fake, cfg(Some("x11")));
    let _ = observer.observe(&grok());
    let before = count(&fake, environ);
    // The same pid, restarted later: another instance.
    let mut m = kitty_machine(Some("x11"));
    for p in m.procs.iter_mut().filter(|p| p.pid == 2601) {
        p.start += 5;
    }
    m.write_procs(&fake);
    let _ = observer.observe(&grok());
    assert_eq!(count(&fake, environ), before + 1);
}
