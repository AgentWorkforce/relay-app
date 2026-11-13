//! Session history: capturing this computer's coding sessions with
//! relayhistory (`ai-hist`), the upload queue and its RelayHistory transport,
//! the session catalog and its queries, history retention, the uploader the
//! account supervises, and the remote sessions (Devin Cloud, Claude Code on
//! the web) the catalogs list. The core (`agent_relay_node`) is messaging only;
//! this layer adds history to it through its plugins.

/// The RelayHistory (`ai-hist`) release session history is built on.
pub const HISTORY_VERSION: &str = env!("RELAY_HISTORY_VERSION");

/// Held by every test that changes this process's environment.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub mod acquisition;
pub mod claude_web;
pub mod cloud;
pub mod delivery;
pub mod delivery_status;
pub mod destination;
pub mod devin_cloud;
pub mod history_access;
pub mod history_retention;
pub mod migration;
pub mod operator_policy;
pub mod probe;
pub mod registered_names;
pub mod relay_uploads;
pub mod retention;
pub mod scrub;
pub mod session_model;
pub mod session_naming;
pub mod session_query;
pub mod sources;
pub mod uploader;
