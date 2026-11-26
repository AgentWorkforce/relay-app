//! [`MacPorts`]: the three adapters bundled so `session_navigation::engine::open`
//! can take them as one value.
use session_navigation::evidence::{Reservation, Snapshot};
use session_navigation::model::SessionRef;
use session_navigation::plan::{Dispatch, Step};
use session_navigation::ports::{Execute, Observe, Reservations, StoreError};

use super::config::MacConfig;
use super::execute::MacExecutor;
use super::observe::MacObserver;
use crate::common::reservations::FileReservations;
use crate::common::{HostHistory, NoHistory};
use crate::system::System;

pub struct MacPorts<S, H = NoHistory> {
    pub observer: MacObserver<S, H>,
    pub executor: MacExecutor<S>,
    pub reservations: FileReservations<S>,
}

impl<S: System + Clone> MacPorts<S, NoHistory> {
    /// All three adapters over clones of one system (`&FakeSystem`,
    /// `Arc<…>` or `RealSystem`).
    pub fn new(sys: S, cfg: MacConfig) -> Self {
        Self::with_history(sys, cfg, NoHistory)
    }
}

impl<S: System + Clone, H: HostHistory> MacPorts<S, H> {
    pub fn with_history(sys: S, cfg: MacConfig, history: H) -> Self {
        Self {
            reservations: FileReservations::new(sys.clone(), &cfg.common.paths.reservation_dir),
            executor: MacExecutor::new(sys.clone(), cfg.clone()),
            observer: MacObserver::with_history(sys, cfg, history),
        }
    }
}

impl<S: System + Sync, H: HostHistory> Observe for MacPorts<S, H> {
    fn observe(&mut self, session: &SessionRef) -> Snapshot {
        self.observer.observe(session)
    }
}

impl<S: System, H: HostHistory> Execute for MacPorts<S, H> {
    fn execute(&mut self, step: &Step) -> Dispatch {
        self.executor.execute(step)
    }
}

impl<S: System, H: HostHistory> Reservations for MacPorts<S, H> {
    fn reserve(&mut self, reservation: Reservation) -> Result<(), StoreError> {
        self.reservations.reserve(reservation)
    }

    fn release(&mut self, session: &SessionRef) -> Result<(), StoreError> {
        self.reservations.release(session)
    }
}
