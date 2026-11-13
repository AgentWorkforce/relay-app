//! A core still running another build than this app carries, as a manual
//! install can leave behind, is a problem the person is told about.

use super::super::Bridge;

impl Bridge {
    /// The person-facing problem when the core that answered runs another
    /// build than the one this app carries. `None` when they match or this
    /// app carries no core version.
    pub fn stale_core(&self) -> Option<String> {
        let bundled = self.inner.options.host.as_ref()?.version.as_deref()?;
        stale_core_message(self.daemon_version().as_deref(), bundled)
    }
}

/// The macOS app's wording (`AppModel.staleCoreMessage`) for a core still
/// running another build than the app carries.
pub fn stale_core_message(running: Option<&str>, bundled: &str) -> Option<String> {
    let running = running?;
    (running != bundled).then(|| {
        format!(
            "The background service is still running Agent Relay {running}, not {bundled}. Quit Agent Relay and open it again; if this stays, restart your computer."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_core_of_another_build_is_a_visible_problem() {
        assert_eq!(stale_core_message(Some("1.4.0"), "1.4.0"), None);
        assert_eq!(stale_core_message(None, "1.4.0"), None);
        let message = stale_core_message(Some("1.3.2"), "1.4.0").unwrap();
        assert!(message.contains("still running Agent Relay 1.3.2, not 1.4.0"));
    }
}
