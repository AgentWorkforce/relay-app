//! Shared fakes: requests, a fixed-snapshot adapter, and snapshots taken
//! by the real macOS observer over an empty `FakeSystem`.
use std::sync::{Arc, Mutex};

use session_navigation::evidence::{Reservation, Snapshot};
use session_navigation::model::{Provider, SessionRef};
use session_navigation::plan::{Dispatch, Step};
use session_navigation::ports::{Execute, Observe, Reservations, StoreError};
use session_navigation_native::{FakeSystem, MacConfig, MacObserver};

use crate::session_open::contract::Request;

pub const HOME: &str = "/Users/example";
pub const CLAUDE_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
pub const CODEX_ID: &str = "01900000-0000-7000-8000-000000000002";

pub fn request(source: &str, id: &str, app_dir: &str) -> Request {
    Request {
        source: source.into(),
        session_id: id.into(),
        app_data_dir: app_dir.into(),
        ..Request::default()
    }
}

pub fn mac_cfg(app_dir: &str) -> MacConfig {
    let mut cfg = MacConfig::for_user(HOME, 501);
    cfg.common.paths.reservation_dir = crate::session_open::native::reservation_dir(app_dir);
    cfg
}

pub fn claude() -> SessionRef {
    SessionRef::new(Provider::Claude, CLAUDE_ID)
}

/// What the real observer reports on a machine where nothing answers.
pub fn snapshot(session: &SessionRef) -> Snapshot {
    let fake = FakeSystem::new();
    MacObserver::new(&fake, MacConfig::for_user(HOME, 501)).observe(session)
}

/// An adapter that always observes the same snapshot and records the rest.
#[derive(Clone, Default)]
pub struct Fixed {
    pub snapshot: Option<Snapshot>,
    pub executed: Arc<Mutex<Vec<Step>>>,
    pub reserved: Arc<Mutex<Vec<Reservation>>>,
    pub released: Arc<Mutex<Vec<SessionRef>>>,
}

impl Fixed {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot: Some(snapshot),
            ..Self::default()
        }
    }
}

impl Observe for Fixed {
    fn observe(&mut self, session: &SessionRef) -> Snapshot {
        self.snapshot.clone().unwrap_or_else(|| snapshot(session))
    }
}

impl Execute for Fixed {
    fn execute(&mut self, step: &Step) -> Dispatch {
        self.executed.lock().unwrap().push(step.clone());
        Dispatch::Dispatched
    }
}

impl Reservations for Fixed {
    fn reserve(&mut self, reservation: Reservation) -> Result<(), StoreError> {
        self.reserved.lock().unwrap().push(reservation);
        Ok(())
    }

    fn release(&mut self, session: &SessionRef) -> Result<(), StoreError> {
        self.released.lock().unwrap().push(session.clone());
        Ok(())
    }
}

/// `osascript` runs that are AppleScript (`-e`), not the JXA registry.
pub fn applescript_runs(fake: &FakeSystem) -> usize {
    fake.runs()
        .into_iter()
        .filter(|(program, args)| {
            program.ends_with("osascript") && args.first().map(String::as_str) != Some("-l")
        })
        .count()
}

/// Every program the fake ran or spawned that could act on the machine:
/// `open`, `kill`, `osascript -e`, any spawn.
pub fn actions(fake: &FakeSystem) -> Vec<String> {
    let acting = |program: &str| {
        ["/usr/bin/open", "/bin/kill", "kill", "wt.exe", "wsl.exe"]
            .iter()
            .any(|p| program.ends_with(p))
    };
    let mut out: Vec<String> = fake
        .runs()
        .into_iter()
        .filter(|(program, _)| acting(program))
        .map(|(program, args)| format!("{program} {}", args.join(" ")))
        .collect();
    out.extend(fake.spawns().into_iter().map(|(p, _)| p));
    out
}
