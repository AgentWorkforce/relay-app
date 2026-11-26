//! `session-navigation` decides how to get a person back to a coding-agent
//! session: focus it if it is still open, reopen or resume it if it was closed
//! or archived, and refuse with a specific reason whenever doing so could be
//! harmful.
//!
//! The work is split into four steps:
//!
//! 1. **observe** ([`ports::Observe`]) produces an immutable [`evidence::Snapshot`];
//! 2. **decide** ([`decide::decide`]) is a pure, total function from a snapshot
//!    to a [`plan::Decision`];
//! 3. **act** ([`ports::Execute`]) dispatches one plan step at a time;
//! 4. **verify** ([`engine::open`]) observes again and checks the plan's
//!    post-condition.
//!
//! This crate performs no real side effects. [`sim`] is a deterministic fake
//! machine that implements the ports, including the hazards documented in the
//! research notes, plus an invariant oracle that is independent of `decide`.
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

pub mod decide;
pub mod engine;
pub mod evidence;
pub mod hosts;
pub mod model;
pub mod parse;
pub mod plan;
pub mod ports;
pub mod providers;
pub mod sim;
