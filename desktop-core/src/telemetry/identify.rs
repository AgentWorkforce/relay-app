//! The end of a `$identify` request on the sender thread.

use super::{write_file, Shared, State};

/// Settles the `$identify` that merged the anonymous id into `user`. An
/// accepted one saves its merge, unless the identity moved on while it was
/// on the wire; a refused one is queued again by the next event.
pub(super) fn settle(shared: &Shared, state: &mut State, merges: (String, String), sent: bool) {
    let (user, anonymous) = merges;
    let current = state.account.as_ref().map(|a| a.user_id.as_str()) == Some(user.as_str())
        && state.identity.as_deref() == Some(anonymous.as_str())
        && state.identifying;
    if !current {
        return;
    }
    state.identifying = false;
    if !sent {
        return;
    }
    if let Err(error) = write_file(&shared.identified, format!("{user}\n").as_bytes()) {
        agent_relay_node::relay::diagnostics::write(format_args!(
            "relay: could not save the telemetry merge: {error}"
        ));
    }
    state.identified = Some(user);
}
