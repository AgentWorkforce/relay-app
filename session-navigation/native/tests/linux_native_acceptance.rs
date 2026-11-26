//! Opt-in, read-only check of the Linux adapter against a real Linux
//! machine. Skipped (passes trivially) unless `SESSION_NAVIGATION_NATIVE=1`
//! **and** the test runs on Linux:
//!
//! ```text
//! SESSION_NAVIGATION_NATIVE=1 cargo test -p session-navigation-native --no-default-features --features linux --test linux_native_acceptance -- --nocapture
//! ```
//!
//! It runs one observation (`/proc`, and whatever `wmctrl`, `xdotool`,
//! `kitty @ ls`, `wezterm cli list` and `tmux list-*` the machine has) and
//! checks the process table against this test's own process. It executes
//! no step: nothing is focused, launched or signalled. A skipped run is not
//! evidence of anything.
#![cfg(feature = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use session_navigation::evidence::{Exe, Fact};
use session_navigation::model::{Platform, Provider, SessionRef};
use session_navigation::ports::Observe;
use session_navigation_native::linux::{LinuxConfig, LinuxObserver};
use session_navigation_native::RealSystem;

fn enabled() -> bool {
    let asked = std::env::var("SESSION_NAVIGATION_NATIVE").as_deref() == Ok("1");
    if asked && std::env::consts::OS != "linux" {
        eprintln!("linux_native_acceptance: not on Linux, skipped");
    }
    asked && std::env::consts::OS == "linux"
}

#[test]
fn read_only_observation_finds_this_test_process() {
    if !enabled() {
        return;
    }
    let cfg = LinuxConfig::from_env().expect("HOME");
    eprintln!("display server: {:?}", cfg.display_server());
    let uid = cfg.uid;
    let mut observer = LinuxObserver::new(RealSystem, cfg);
    // A well-formed id no provider has: observing it touches nothing.
    let session = SessionRef::new(Provider::Grok, "00000000-0000-4000-8000-000000000000");
    let snap = observer.observe(&session);
    assert!(matches!(snap.platform, Platform::Linux { .. }));
    let boot = snap.boot.known().copied().expect("boot id");
    let table = snap.processes.known().expect("process table");
    let me = std::process::id();
    let row = table
        .iter()
        .find(|p| p.identity.pid == me)
        .expect("this process in the table");
    eprintln!("own row: {row:?}");
    assert_eq!(row.identity.boot, boot);
    assert_eq!(row.ruid, uid, "run the test as the owner of $HOME");
    assert_eq!(row.exe, Exe::Other, "a test binary is no agent or terminal");
    assert!(row.identity.start <= snap.observed_at);
    assert!(
        snap.observed_at - row.identity.start < 3600,
        "started within the hour"
    );
    assert!(row
        .argv
        .first()
        .is_some_and(|a| a.contains("linux_native_acceptance")));
    let parent = table.iter().find(|p| p.identity.pid == row.ppid);
    assert!(parent.is_some(), "the parent (cargo) is in the table");
    assert!(table.iter().any(|p| p.identity.pid == 1));
    // A second observation computes the same identity (stable btime).
    let again = observer.observe(&session);
    let row2 = again
        .processes
        .known()
        .unwrap()
        .iter()
        .find(|p| p.identity.pid == me)
        .cloned()
        .unwrap();
    assert_eq!(row2.identity, row.identity);
    eprintln!("terminals: {:?}", snap.surfaces.terminals);
    eprintln!("windows: {:?}", snap.surfaces.windows);
    eprintln!("panes: {:?}", snap.surfaces.panes);
    eprintln!("tmux: {:?}", snap.surfaces.tmux);
    eprintln!("front: {:?}", snap.front);
    assert_ne!(snap.surfaces.terminals, Fact::Unknown);
}
