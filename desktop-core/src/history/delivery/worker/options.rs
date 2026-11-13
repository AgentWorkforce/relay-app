//! What one drain may do: its jobs, its budgets and its oops window.
#[cfg(doc)]
use super::{Receiver, MAX_IN_FLIGHT};
use crate::history::scrub::ScrubEngine;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct DrainOptions {
    /// `None` drains every job; an unknown id is an invalid argument.
    pub job_ids: Option<Vec<String>>,
    pub worker_id: String,
    pub max_batches: usize,
    pub max_prepare_steps: usize,
    /// Wall-clock budget for the drain's delivery work, measured from the end
    /// of its own maintenance. Once it passes no further prepare step or
    /// attempt starts, and an attempt already under way runs to its receiver's
    /// own timeout. The budget bounds how long a drain keeps going, not whether
    /// it goes at all: its first step, and the first attempt that step
    /// prepares, always run, so however short the budget the queue still moves.
    /// `None` bounds the drain by counts alone.
    pub max_elapsed: Option<Duration>,
    pub lease_ms: i64,
    pub request_timeout_ms: i64,
    /// An optional absolute ceiling shared by every request this drain starts.
    /// Unlike `max_elapsed`, this is measured from the caller's clock before
    /// maintenance begins and is recomputed immediately before each send.
    pub request_deadline: Option<Instant>,
    /// The most batches of one job prepared, claimed and in flight at once,
    /// at most [`MAX_IN_FLIGHT`]. Beyond one, batches are sent from that many
    /// threads while the next is prepared, to a receiver that offers
    /// [`Receiver::concurrent`]. The default, 1, delivers one batch at a
    /// time.
    pub max_in_flight: usize,
    /// An organization's oops window: each session's records wait this long
    /// after its last activity before they are prepared. `None` or zero
    /// prepares them at once. See the delivery `hold` module.
    pub oops_window_ms: Option<i64>,
    /// The engine every record and body this drain queues or sends passes
    /// through first: [`ScrubEngine::default_engine`] unless a test injects
    /// another.
    pub scrub: ScrubEngine,
}
impl DrainOptions {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            job_ids: None,
            worker_id: worker_id.into(),
            max_batches: 100,
            max_prepare_steps: 100,
            max_elapsed: None,
            lease_ms: 30_000,
            request_timeout_ms: 30_000,
            request_deadline: None,
            max_in_flight: 1,
            oops_window_ms: None,
            scrub: ScrubEngine::default_engine(),
        }
    }
}
