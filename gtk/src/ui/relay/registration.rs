//! Renaming an agent: the name shows at once, and goes back if the core
//! refuses.
use super::*;

impl App {
    /// Shows the agent `name` renamed to `alias` at once, in the agents
    /// and in its session's registration; returns the alias it had, for
    /// `revert_rename`.
    pub(crate) fn show_rename(&self, name: &str, alias: &str) -> Option<String> {
        let previous = self.update_state(|state| {
            let mut previous = None;
            for agent in state
                .local_agents
                .iter_mut()
                .filter(|agent| agent.name == name)
            {
                previous = agent.alias.replace(alias.to_string());
            }
            for registration in state
                .local_sessions
                .iter_mut()
                .filter_map(|session| session.registration.as_mut())
                .filter(|registration| registration.name == name)
            {
                registration.alias = Some(alias.to_string());
            }
            previous
        });
        self.relay_changed();
        previous
    }

    /// The rename of `name` to `alias` was refused: wherever that alias is
    /// still shown, the one before it comes back.
    pub(crate) fn revert_rename(&self, name: &str, alias: &str, previous: Option<String>) {
        self.update_state(|state| {
            for agent in state
                .local_agents
                .iter_mut()
                .filter(|agent| agent.name == name && agent.alias.as_deref() == Some(alias))
            {
                agent.alias = previous.clone();
            }
            for registration in state
                .local_sessions
                .iter_mut()
                .filter_map(|session| session.registration.as_mut())
                .filter(|registration| {
                    registration.name == name && registration.alias.as_deref() == Some(alias)
                })
            {
                registration.alias = previous.clone();
            }
        });
        self.relay_changed();
    }
}
