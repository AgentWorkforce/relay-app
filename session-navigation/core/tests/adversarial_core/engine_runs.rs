use super::*;

// =============================================================== Engine ===

/// In-memory ports: a fixed snapshot template, a durable reservation slot,
/// and scripted dispatch results.
struct Fake {
    template: Snapshot,
    reservation: Option<Reservation>,
    results: VecDeque<Dispatch>,
    executed: Vec<Step>,
}

impl Observe for Fake {
    fn observe(&mut self, _: &SessionRef) -> Snapshot {
        let mut s = self.template.clone();
        s.reservation = Fact::Known(self.reservation.clone());
        s
    }
}

impl Execute for Fake {
    fn execute(&mut self, step: &Step) -> Dispatch {
        self.executed.push(step.clone());
        self.results.pop_front().unwrap_or(Dispatch::Dispatched)
    }
}

impl Reservations for Fake {
    fn reserve(&mut self, r: Reservation) -> Result<(), StoreError> {
        self.reservation = Some(r);
        Ok(())
    }
    fn release(&mut self, _: &SessionRef) -> Result<(), StoreError> {
        self.reservation = None;
        Ok(())
    }
}

fn codex_stopped_fake(results: Vec<Dispatch>) -> Fake {
    Fake {
        template: snap(
            Provider::Codex,
            CODEX_ID,
            vec![launchd()],
            &[],
            codex_ev(CodexOrigin::Tui, vec![]),
        ),
        reservation: None,
        results: results.into(),
        executed: vec![],
    }
}

fn launch_count(f: &Fake) -> usize {
    f.executed
        .iter()
        .filter(|s| matches!(s, Step::Launch { .. }))
        .count()
}

/// DEFECT (stuck): `engine/mod.rs:114-138` writes the reservation before the
/// launch and keeps it when the executor reports `Refused`. `Refused` is the
/// Automation-permission refusal (AppleScript -1743): the script never ran,
/// so nothing can start. After the person grants permission, every retry is
/// `Blocked(LaunchPending)` with no tty to resolve it.
/// Evidence: Claude facts D / Codex facts 21 (-1743 maps to "allow
/// Automation"; the script does not run).
#[test]
fn a_refused_launch_does_not_leave_a_blocking_reservation() {
    let mut f = codex_stopped_fake(vec![
        Dispatch::Refused,
        Dispatch::Opened {
            tty: "/dev/ttys009".into(),
        },
    ]);
    let s = f.template.session.clone();
    let first = engine::open(&mut f, &s, &req());
    assert_eq!(first.outcome, Outcome::Blocked(Reason::PermissionDenied));
    let second = engine::open(&mut f, &s, &req());
    assert_ne!(
        second.outcome,
        Outcome::Blocked(Reason::LaunchPending),
        "retry after granting permission is blocked forever"
    );
}

/// STRENGTH: a double click while the first launch is unobserved launches
/// once. The reservation is durable before dispatch.
#[test]
fn double_open_launches_once() {
    let mut f = codex_stopped_fake(vec![Dispatch::Dispatched, Dispatch::Dispatched]);
    let s = f.template.session.clone();
    let first = engine::open(&mut f, &s, &req());
    assert_eq!(first.outcome, Outcome::DispatchedUnverified);
    assert!(f.reservation.is_some(), "reservation written");
    let second = engine::open(&mut f, &s, &req());
    assert_eq!(second.outcome, Outcome::Blocked(Reason::LaunchPending));
    assert_eq!(launch_count(&f), 1);
}

/// STRENGTH: a failed focus is reported and never followed by a launch.
#[test]
fn failed_focus_is_not_followed_by_a_launch() {
    let mut table = ghostty_chain("/dev/ttys000");
    table.push(proc_(
        4242,
        998,
        Exe::CodexCli,
        Some("/dev/ttys000"),
        &["codex", "resume", CODEX_ID],
        Some(DIR),
    ));
    let mut f = Fake {
        template: snap(
            Provider::Codex,
            CODEX_ID,
            table,
            &[],
            codex_ev(CodexOrigin::Tui, vec![4242]),
        ),
        reservation: None,
        results: vec![Dispatch::Failed(Reason::DispatchFailed)].into(),
        executed: vec![],
    };
    let s = f.template.session.clone();
    let r = engine::open(&mut f, &s, &req());
    assert_eq!(r.outcome, Outcome::Failed(Reason::DispatchFailed));
    assert_eq!(launch_count(&f), 0);
    assert!(f.reservation.is_none());
}
