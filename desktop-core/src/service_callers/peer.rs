//! Who is calling (`docs/service-callers.md` sections 4 and 6). The bridge
//! admits only this user's connections; these checks repeat that on the
//! calling process and keep management away from session and spawned
//! process trees.

#[cfg(not(windows))]
use super::host::Attestation;
use super::host::Host;
use super::Class;

/// How far up the process tree a management peer is walked.
const ANCESTORS: usize = 64;

/// Who asked for a management change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Initiator {
    /// A peer the operating system attests as this app's signed code: it
    /// may decide, enable, disable and revoke, and its provision or
    /// rotation proceeds at once.
    App,
    /// Every other peer, the probe included: a provision or rotation waits
    /// for the person in Settings, and app-only changes are refused.
    Probe,
}

/// Whether the calling process can be checked on this platform at all.
/// On Windows the core cannot read the calling process yet, so every
/// service-caller method, `challenge` and `authenticate` included, and every
/// bound `spawn.*` call reports `peer_check_unavailable` rather than admit an
/// unchecked peer or hand out a binding no call could use
/// (`docs/service-callers-auth.md` section 9.1). The one policy, checked
/// first wherever a service caller is answered.
pub(crate) fn available() -> Result<(), Class> {
    #[cfg(test)]
    if UNAVAILABLE.with(std::cell::Cell::get) {
        return Err(Class::PeerCheckUnavailable);
    }
    if cfg!(windows) {
        return Err(Class::PeerCheckUnavailable);
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// A failpoint: [`available`] answers as on Windows on this thread, so
    /// the Windows policy is tested on every platform.
    pub(crate) static UNAVAILABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A caller's process: this user, read from the process table.
#[cfg(not(windows))]
pub(crate) fn caller(host: &dyn Host, peer_pid: Option<i32>) -> Result<(), Class> {
    available()?;
    let pid = peer_pid.ok_or(Class::PeerUnreadable)?;
    let process = host.process(pid).ok_or(Class::PeerUnreadable)?;
    if process.uid != host.current_uid() {
        return Err(Class::PeerUidMismatch);
    }
    Ok(())
}

/// On Windows the core cannot read the calling process, so service callers
/// report themselves unavailable rather than admit an unchecked peer
/// ([`available`]).
#[cfg(windows)]
pub(crate) fn caller(_host: &dyn Host, _peer_pid: Option<i32>) -> Result<(), Class> {
    available()
}

/// A management call: no secret, this user, outside every session and
/// spawned process tree.
#[cfg(not(windows))]
pub(crate) fn management(
    host: &dyn Host,
    peer_pid: Option<i32>,
    secret_given: bool,
) -> Result<Initiator, Class> {
    available()?;
    if secret_given {
        return Err(Class::SecretOnManagement);
    }
    caller(host, peer_pid)?;
    let pid = peer_pid.ok_or(Class::PeerUnreadable)?;
    if host.in_session(pid) {
        return Err(Class::PeerInSessionTree);
    }
    let spawned: Vec<(i32, u64)> = host
        .account()
        .and(host.workspace())
        .map(|workspace| {
            host.spawned(&workspace)
                .into_iter()
                .filter_map(|spawned| spawned.process)
                .collect()
        })
        .unwrap_or_default();
    let mut current = Some(pid);
    for _ in 0..ANCESTORS {
        let Some(pid) = current.filter(|pid| *pid > 1) else {
            break;
        };
        let Some(process) = host.process(pid) else {
            break;
        };
        if spawned.contains(&(process.pid, process.start)) {
            return Err(Class::PeerInSessionTree);
        }
        current = Some(process.parent);
    }
    // Only the operating system's attestation makes the app; a file or
    // process name never does.
    Ok(match host.attest_app_peer(pid) {
        Attestation::App => Initiator::App,
        Attestation::NotApp | Attestation::Unavailable => Initiator::Probe,
    })
}

#[cfg(windows)]
pub(crate) fn management(
    host: &dyn Host,
    peer_pid: Option<i32>,
    secret_given: bool,
) -> Result<Initiator, Class> {
    available()?;
    if secret_given {
        return Err(Class::SecretOnManagement);
    }
    caller(host, peer_pid).map(|()| Initiator::Probe)
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use crate::service_callers::host::CoreHost;

    #[test]
    fn the_windows_peer_check_reports_itself_unavailable() {
        let host = CoreHost::default();
        assert_eq!(caller(&host, Some(42)), Err(Class::PeerCheckUnavailable));
        assert_eq!(caller(&host, None), Err(Class::PeerCheckUnavailable));
        assert_eq!(
            management(&host, Some(42), false),
            Err(Class::PeerCheckUnavailable)
        );
        assert_eq!(
            management(&host, Some(42), true),
            Err(Class::PeerCheckUnavailable)
        );
        assert_eq!(available(), Err(Class::PeerCheckUnavailable));
    }
}
