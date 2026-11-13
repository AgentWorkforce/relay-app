//! Agent Relay Desktop's private core layer: sign-in, hosted MCP, provider
//! setup and session history, added to the public core through its plugin
//! hooks.

pub mod account;
pub mod account_methods;
pub mod browser_login;
mod attestation_route;
pub mod claude_attestation;
pub mod claude_keychain;
pub mod cloud_api;
pub mod cloud_chat;
pub mod command_line;
pub mod contract;
pub mod contractor_access;
mod copilot_attestation;
pub mod copilot_dispatch;
pub mod desktop_identity;
pub mod executable_identity;
pub mod feed;
pub mod feed_headers;
pub mod feed_verify;
pub mod history;
pub mod install;
pub mod login_config;
pub mod login_item;
pub mod mcp_headers;
pub mod mcp_settings;
pub mod onboarding;
pub mod probe_copy;
pub mod storage_format;
mod provider_methods;
mod relay_build;
pub mod teammate_context;
pub mod telemetry;
pub mod session_locations;
pub mod site_build;
pub mod service_callers;
#[cfg(test)]
mod workspace_recovery_tests;
