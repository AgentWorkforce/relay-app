//! Receive messages mutations, acknowledgements, and visible failures.
use super::*;
use agent_relay_node::relay::agents::Registered;

impl App {
    fn relay_change_failed(&self, key: &str, message: &str) {
        self.update_state(|state| {
            state
                .relay_errors
                .insert(key.to_string(), message.to_string());
            state.wanted.remove(key);
            state.busy_targets.remove(key);
        });
        self.relay_changed();
        self.inner.desktop.inform(
            &self.inner.window,
            "Could not change Receive messages",
            message,
        );
    }

    /// Preserve the acknowledged change even if the subsequent list fails.
    fn remember_receive_registration(&self, key: &str, registered: Option<Registered>) {
        self.update_state(|state| {
            // Retire every list started before this acknowledgement, even if
            // the next authoritative read fails instead of superseding it.
            state.local_reads += 1;
            state.local_applied = state.local_reads;
            if let Some(session) = state
                .local_sessions
                .iter_mut()
                .find(|session| session.target.key() == key)
            {
                session.registration = registered.map(|registered| {
                    let previous = registered
                        .already_registered
                        .then(|| session.registration.clone())
                        .flatten()
                        .unwrap_or_default();
                    SessionRegistration {
                        name: registered.name,
                        address: registered.address,
                        ..previous
                    }
                });
            }
        });
    }

    fn receive_refreshed(
        &self,
        key: String,
        target: Target,
        on: bool,
        read: u64,
        title_revision: u64,
        result: Result<Vec<LocalSession>, CoreError>,
    ) {
        let sessions = match result {
            Ok(sessions) => sessions,
            Err(error) => {
                let queued = self.with_state(|state| state.wanted.get(&key).copied());
                self.relay_change_failed(&key, &error.message);
                if let Some(next) = queued.filter(|next| *next != on) {
                    self.update_state(|state| {
                        state.wanted.insert(key.clone(), next);
                        state.busy_targets.insert(key.clone());
                    });
                    self.drive(key, target);
                }
                return;
            }
        };
        self.update_state(|state| {
            state.relay_errors.remove(&key);
        });
        self.apply_local_after_title_write(read, title_revision, sessions);
        self.refresh_agents();
        let (accepted, unchanged_wish) = self.with_state(|state| {
            (
                state
                    .local_sessions
                    .iter()
                    .find(|session| session.target.key() == key)
                    .is_some_and(|session| session.registration.is_some())
                    == on,
                state.wanted.get(&key) == Some(&on),
            )
        });
        if !accepted && unchanged_wish {
            self.relay_change_failed(
                &key,
                "Receive messages did not change. Refresh the session and try again.",
            );
        } else if unchanged_wish {
            self.update_state(|state| {
                state.busy_targets.remove(&key);
                state.wanted.remove(&key);
                state.settle();
            });
            self.relay_changed();
        } else {
            self.drive(key, target);
        }
    }

    /// Apply the most recent wish; a failed read never repeats the same change.
    pub(super) fn drive(&self, key: String, target: Target) {
        let (on, registration) = self.with_state(|state| {
            let registration = state
                .local_sessions
                .iter()
                .find(|session| session.target.key() == key)
                .and_then(|session| session.registration.clone());
            (state.wanted.get(&key).copied(), registration)
        });
        let Some(on) = on.filter(|on| *on != registration.is_some()) else {
            self.update_state(|state| {
                state.busy_targets.remove(&key);
                state.settle();
            });
            self.relay_changed();
            return;
        };
        let register = target.clone();
        let changed = {
            let key = key.clone();
            move |app: &App, error: Option<CoreError>| {
                if let Some(error) = error {
                    app.relay_change_failed(&key, &error.message);
                    return;
                }
                let read = app.update_state(|state| {
                    state.local_reads += 1;
                    state.local_reads
                });
                let title_revision = app.with_state(|state| state.title_write_revision);
                app.view_call::<method::SessionsList>(NoParams {}, move |app, result| {
                    app.receive_refreshed(
                        key,
                        target,
                        on,
                        read,
                        title_revision,
                        result.map(|listed| listed.sessions),
                    );
                });
            }
        };
        match registration {
            None if on => self.view_call::<method::AgentsRegister>(
                RegisterParams {
                    target: register,
                    name: None,
                    description: None,
                },
                move |app, result| match result {
                    Ok(registered) => {
                        app.remember_receive_registration(&key, Some(registered));
                        changed(app, None);
                    }
                    Err(error) => changed(app, Some(error)),
                },
            ),
            Some(registration) => self.view_call::<method::AgentsUnregister>(
                NameParams {
                    name: registration.name,
                },
                move |app, result| {
                    if result.is_ok() {
                        app.remember_receive_registration(&key, None);
                    }
                    changed(app, result.err());
                },
            ),
            None => {}
        }
    }
}
