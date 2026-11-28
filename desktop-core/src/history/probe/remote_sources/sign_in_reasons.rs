//! Which cloud-session listing reasons Claude Code's own sign-in resolves.

use super::claude_cloud;

/// Whether Claude Code's own sign-in would resume cloud sessions after a
/// listing gave `reason`: Claude Code is not signed in here, with or
/// without Claude's desktop app installed, or its sign-in has expired or
/// cannot be read as a Claude Code credential. Signing in again replaces
/// the login in each case.
pub(crate) fn claude_sign_in_helps(reason: &str) -> bool {
    CLAUDE_SIGN_IN_REASONS.contains(&reason)
}

/// The listing reasons [`claude_sign_in_helps`] accepts.
pub(crate) const CLAUDE_SIGN_IN_REASONS: [&str; 4] = [
    claude_cloud::NOT_SIGNED_IN,
    claude_cloud::DESKTOP_APP_NOT_ENOUGH,
    claude_cloud::EXPIRED,
    claude_cloud::UNREADABLE,
];
