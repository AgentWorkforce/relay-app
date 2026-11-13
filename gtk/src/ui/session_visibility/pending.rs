use super::*;
impl App {
    pub(super) fn stop_hidden_session(
        &self,
        key: String,
        session: Option<agent_relay_node::relay::contract::agents::LocalSession>,
        generation: u64,
    ) {
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            loop {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                if app.with_state(|s| s.generation) != generation {
                    return;
                }
                let busy = app.with_state(|state| {
                    state.busy_uploads.contains(&key)
                        || session
                            .as_ref()
                            .is_some_and(|s| state.busy_targets.contains(&s.target.key()))
                });
                if busy {
                    drop(app);
                    glib::timeout_future(Duration::from_millis(20)).await;
                    continue;
                }
                let error = session.as_ref().and_then(|s| {
                    app.with_state(|state| {
                        state
                            .relay_errors
                            .get(&s.target.key())
                            .cloned()
                            .or_else(|| {
                                state
                                    .local_sessions
                                    .iter()
                                    .any(|current| {
                                        current.target == s.target && current.registration.is_some()
                                    })
                                    .then(|| "Session is still on the relay. Try again.".into())
                            })
                    })
                });
                if error.is_some() {
                    app.finish_hiding_session(&key, error);
                    return;
                }
                if key.starts_with("claude-cloud:") {
                    app.finish_hiding_session(&key, None);
                    return;
                }
                if app.with_state(|s| {
                    !s.core_connected
                        || s.account_state()
                            != agent_relay_node::relay::state::SignInState::SignedIn
                }) {
                    app.defer_hiding_session(&key);
                    return;
                }
                // Exclude preserves existing cloud copies; delete does not.
                let params = SessionKeysParams {
                    session_keys: vec![key.clone()],
                };
                let policy_revision = app.with_state(|s| s.session_query.upload_policy_revision);
                app.view_call::<agent_relay_desktop_core::contract::method::UploaderSessionsExclude>(params, move |app, result| {
                    let policy_changed = policy_revision
                        != app.with_state(|s| s.session_query.upload_policy_revision);
                    if (result.is_ok() && policy_changed)
                        || result.as_ref().err().is_some_and(exclusion_waits)
                    {
                        app.defer_hiding_session(&key);
                    } else {
                        app.finish_hiding_session(&key, result.err().map(|error| error.message));
                    }
                });
                return;
            }
        });
    }
}

fn exclusion_waits(error: &crate::bridge::CoreError) -> bool {
    error.failure == crate::bridge::Failure::Unavailable
        || matches!(
            error.kind(),
            Some(
                agent_relay_node::relay::protocol::ErrorKind::NotSignedIn
                    | agent_relay_node::relay::protocol::ErrorKind::Busy
            )
        )
}
