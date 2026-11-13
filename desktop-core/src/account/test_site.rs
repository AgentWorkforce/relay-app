//! What a test may do to an account that a shipped core cannot: compiled
//! only into this crate's own test builds (the `test-site` feature, on
//! through its dev-dependency on itself, and `cfg(test)`).
use super::Account;
use agent_relay_node::relay::workspace::validated_base_url;

impl Account {
    /// This account signing in to `site` instead of the build's own: a
    /// test's fake Cloud on a port the build could not have known. `Err`
    /// names a `site` that is no http(s) origin, for the test to fail on.
    pub fn with_site(mut self, site: &str) -> Result<Account, String> {
        self.site_url = validated_base_url(site).ok_or_else(|| format!("{site}: not an origin"))?;
        Ok(self)
    }

    /// Whether the sign-in attempt's work has all run: its task, which
    /// ends with what follows a connection (`signed_in`, launch at login
    /// included), is gone or finished. For a test to assert on afterwards.
    pub fn attempt_finished(&self) -> bool {
        let current = self.attempt.lock().unwrap_or_else(|p| p.into_inner());
        current.task.as_ref().is_none_or(|task| task.is_finished())
    }
}
