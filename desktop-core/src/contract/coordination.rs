//! Turn-end coordination advisories: the setting the apps show
//! ([`crate::coordination_advisories`]).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct CoordinationAdvisoriesParams {
    pub enabled: bool,
}

/// What the app last read of the OS's permission for its notifications;
/// it never asks for it to read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationPermission {
    Allowed,
    Denied,
    /// The app cannot read it (Linux), or the person has not been asked.
    Unknown,
}

/// The app's report that it can show a system notification, and whether
/// the OS allows it.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct CoordinationNotificationsDisplayParams {
    pub permission: NotificationPermission,
}

/// Whether sessions are shown teammates' overlapping work as their turn
/// ends. Off by default.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct CoordinationAdvisoriesState {
    pub enabled: bool,
    /// A separate opt-in; upgrades of a turn-end-only setting stay quiet.
    pub notifications_enabled: bool,
    /// Requests/availability, never a claim that the OS displayed a banner.
    pub notification_status: String,
}
