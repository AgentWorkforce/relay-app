//! [`LinuxPorts`]: the three adapters bundled so
//! `session_navigation::engine::open` can take them as one value. The
//! observer and the executor share one [`BootClock`].
use session_navigation::evidence::{Reservation, Snapshot};
use session_navigation::model::SessionRef;
use session_navigation::plan::{Dispatch, Step};
use session_navigation::ports::{Execute, Observe, Reservations, StoreError};

use super::config::LinuxConfig;
use super::execute::LinuxExecutor;
use super::observe::LinuxObserver;
use super::table::BootClock;
use crate::common::reservations::FileReservations;
use crate::common::{HostHistory, NoHistory};
use crate::system::System;

pub struct LinuxPorts<S, H = NoHistory> {
    pub observer: LinuxObserver<S, H>,
    pub executor: LinuxExecutor<S>,
    pub reservations: FileReservations<S>,
}

impl<S: System + Clone> LinuxPorts<S, NoHistory> {
    /// All three adapters over clones of one system (`&FakeSystem`,
    /// `Arc<…>` or `RealSystem`).
    pub fn new(sys: S, cfg: LinuxConfig) -> Self {
        Self::with_history(sys, cfg, NoHistory)
    }
}

impl<S: System + Clone, H: HostHistory> LinuxPorts<S, H> {
    pub fn with_history(sys: S, cfg: LinuxConfig, history: H) -> Self {
        let clock = BootClock::new();
        Self {
            reservations: FileReservations::new(sys.clone(), &cfg.common.paths.reservation_dir),
            executor: LinuxExecutor::with_clock(sys.clone(), cfg.clone(), clock.clone()),
            observer: LinuxObserver::with_history(sys, cfg, history, clock),
        }
    }
}

impl<S: System + Sync, H: HostHistory> Observe for LinuxPorts<S, H> {
    fn observe(&mut self, session: &SessionRef) -> Snapshot {
        self.observer.observe(session)
    }
}

impl<S: System, H: HostHistory> Execute for LinuxPorts<S, H> {
    fn execute(&mut self, step: &Step) -> Dispatch {
        self.executor.execute(step)
    }
}

impl<S: System, H: HostHistory> Reservations for LinuxPorts<S, H> {
    fn reserve(&mut self, reservation: Reservation) -> Result<(), StoreError> {
        self.reservations.reserve(reservation)
    }

    fn release(&mut self, session: &SessionRef) -> Result<(), StoreError> {
        self.reservations.release(session)
    }
}
