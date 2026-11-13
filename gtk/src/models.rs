//! The view layer's words: what the GTK app says about the relay core's
//! typed answers (the registry's own types), its relative times and its
//! filters. The Mac's `RelayDesktopCore` presentation logic and
//! `shared/RelaySessionOverview` are the reference; user-visible sentences
//! match them word for word unless the platform differs. Nothing here reads
//! a file, the network or the core.
//!
//! | Module | Owns |
//! |---|---|
//! | `time` | Relative times and local clock times |
//! | `agent_relay_desktop_core::history::session_model` (re-exported) | Session rows, the search language, harness names, sharing modes, upload states, filters and stages |
//! | `agents` | Session presence and initials |
//! | `messages` | The composer: delivery notes, recipient warnings, endpoints |
//! | `openclaw_tools` | The Settings row that gives OpenClaw agents Agent Relay's tools |
//! | `overview` | The Cloud session brief and the session's original request |
//! | `capy_setup` | The Sessions page's Capy setup card |
//! | `devin_setup` | The Sessions page's offer to turn on Devin's hooks |
//! | `history_retention` | How long local session history is kept, and the warning a short choice gets |
//! | `agent_settings` | New agents joining #general; the settings agents changed |
//! | `plan` | The workspace's plan line, a cap that was reached, humans at their cap, history retention left out |
//! | `settings` | Sign-in wording, Claude Code direct delivery, the SessionStart hook, workspace labels, diagnostics |
//! | `spawn` | Spawned agents: the spawn node's status line and each harness's full-access row, from the core's descriptors |
//! | `spawn_handover` | Joining a spawned agent (its live transcript and message box) and taking it over |

mod agent_settings;
mod agents;
mod capy_setup;
mod cloud_provider_setup;
mod core_crates;
mod cursor_hooks;
mod devin_setup;
mod history_retention;
mod menu_summary;
mod messages;
mod mobile_native_continuation;
mod openclaw_tools;
mod optimistic;
mod teammate_context;
pub use optimistic::ClaudeDeliveryStatusOptimistic;
mod overview;
mod pending;
mod pending_session_resumes;
mod plan;
mod relaycast_upgrade;
mod saved_session_views;
mod session_activity;
mod session_columns;
mod session_opening;
mod session_search_cache;
mod session_titles;
mod settings;
mod sign_in_again;
mod site;
mod spawn;
mod spawn_handover;
mod startup;
mod teammate_native_input;
mod time;
mod transcript_presentation;

pub use agent_relay_desktop_core::history::session_model::*;
pub use agent_settings::*;
pub use agents::*;
pub use capy_setup::*;
pub use cloud_provider_setup::*;
pub use core_crates::*;
pub use cursor_hooks::*;
pub use devin_setup::*;
pub use history_retention::*;
pub use teammate_context::*;
pub use menu_summary::*;
pub use messages::*;
pub use mobile_native_continuation::*;
pub use openclaw_tools::*;
pub use overview::*;
pub use pending::*;
pub use pending_session_resumes::*;
pub use plan::*;
pub use relaycast_upgrade::*;
pub use saved_session_views::*;
pub use session_activity::*;
pub use session_columns::*;
pub use session_opening::*;
pub use session_search_cache::*;
pub use session_titles::*;
pub use settings::*;
pub use sign_in_again::*;
pub use site::*;
pub use spawn::*;
pub use spawn_handover::*;
pub use startup::*;
pub use teammate_native_input::*;
pub use time::*;
pub use transcript_presentation::*;
