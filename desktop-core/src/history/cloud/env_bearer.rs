//! The Cloud bearer the environment hands one command.

/// The environment variable that hands a Cloud bearer to one command.
pub const ACCESS_TOKEN_VARIABLE: &str = "CLOUD_API_ACCESS_TOKEN";

/// The environment's bearer, left in place.
pub(super) fn env_cloud_bearer() -> Option<String> {
    std::env::var(ACCESS_TOKEN_VARIABLE)
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

/// The environment's bearer, removed once read: nothing this process starts
/// afterwards, such as the detached collector setup starts, inherits a token
/// that will expire.
pub(super) fn take_env_bearer() -> Option<String> {
    let token = env_cloud_bearer();
    std::env::remove_var(ACCESS_TOKEN_VARIABLE);
    token
}
