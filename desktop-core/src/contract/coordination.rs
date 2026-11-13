//! Turn-end coordination advisories: the setting the apps show
//! ([`crate::coordination_advisories`]).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct CoordinationAdvisoriesParams {
    pub enabled: bool,
}

/// Whether sessions are shown teammates' overlapping work as their turn
/// ends. Off by default.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct CoordinationAdvisoriesState {
    pub enabled: bool,
}
