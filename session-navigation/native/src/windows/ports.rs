//! [`WindowsPorts`]: the three adapters bundled so
//! `session_navigation::engine::open` can take them as one value.
use session_navigation::evidence::{Reservation, Snapshot};
use session_navigation::model::SessionRef;
use session_navigation::plan::{Dispatch, Step};
use session_navigation::ports::{Execute, Observe, Reservations, StoreError};

use super::config::WindowsConfig;
use super::execute::WindowsExecutor;
use super::observe::WindowsObserver;
use crate::common::reservations::FileReservations;
use crate::common::{HostHistory, NoHistory};
use crate::system::System;

pub struct WindowsPorts<S, H = NoHistory> {
    pub observer: WindowsObserver<S, H>,
    pub executor: WindowsExecutor<S>,
    /// `%LOCALAPPDATA%\session-navigation\reservations` (no explicit ACL:
    /// a new file inherits the profile directory's, which grants only the
    /// person, SYSTEM and administrators).
    pub reservations: FileReservations<S>,
}

impl<S: System + Clone> WindowsPorts<S, NoHistory> {
    /// All three adapters over clones of one system.
    pub fn new(sys: S, cfg: WindowsConfig) -> Self {
        Self::with_history(sys, cfg, NoHistory)
    }
}

impl<S: System + Clone, H: HostHistory> WindowsPorts<S, H> {
    pub fn with_history(sys: S, cfg: WindowsConfig, history: H) -> Self {
        Self {
            reservations: FileReservations::new(sys.clone(), &cfg.common.paths.reservation_dir),
            executor: WindowsExecutor::new(sys.clone(), cfg.clone()),
            observer: WindowsObserver::with_history(sys, cfg, history),
        }
    }
}

impl<S: System + Sync, H: HostHistory> Observe for WindowsPorts<S, H> {
    fn observe(&mut self, session: &SessionRef) -> Snapshot {
        self.observer.observe(session)
    }
}

impl<S: System, H: HostHistory> Execute for WindowsPorts<S, H> {
    fn execute(&mut self, step: &Step) -> Dispatch {
        self.executor.execute(step)
    }
}

impl<S: System, H: HostHistory> Reservations for WindowsPorts<S, H> {
    fn reserve(&mut self, reservation: Reservation) -> Result<(), StoreError> {
        self.reservations.reserve(reservation)
    }

    fn release(&mut self, session: &SessionRef) -> Result<(), StoreError> {
        self.reservations.release(session)
    }
}
