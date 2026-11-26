//! The boundary to the machine. Real macOS adapters come later behind these;
//! [`crate::sim::World`] implements them for tests.
use crate::evidence::{Reservation, Snapshot};
use crate::model::SessionRef;
use crate::plan::{Dispatch, Step};

/// Produce a fresh snapshot for one session.
pub trait Observe {
    fn observe(&mut self, session: &SessionRef) -> Snapshot;
}

/// Dispatch one step. `Dispatched` only means the request left; whether it
/// worked is decided by observing again.
pub trait Execute {
    fn execute(&mut self, step: &Step) -> Dispatch;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreError;

/// Durable, per-session launch reservations that survive restarts.
pub trait Reservations {
    fn reserve(&mut self, reservation: Reservation) -> Result<(), StoreError>;
    fn release(&mut self, session: &SessionRef) -> Result<(), StoreError>;
}
