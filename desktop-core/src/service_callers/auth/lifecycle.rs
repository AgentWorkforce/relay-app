//! How bindings end: generation advances, retirement and expiry, rotation
//! rollbacks, sign-out and closed connections
//! (`docs/service-callers-auth.md` section 4).

use super::{close_iterators, State};
use crate::service_callers::registry::{self, Binding};
use crate::service_callers::{Class, Live, ServiceCallers};

impl ServiceCallers {
    /// A generation advance of `caller_id` (section 4): every binding of
    /// it goes stale, its iterators close and its leases are released.
    pub(crate) fn stale_caller(&self, caller_id: &str) {
        let mut live = self.live();
        if self.stale_in(&mut live, caller_id) {
            drop(live);
            self.changed.notify_waiters();
        }
    }

    /// [`Self::stale_caller`] under a live lock already held; whether any
    /// binding went stale (the caller wakes the waiters once it lets go).
    fn stale_in(&self, live: &mut Live, caller_id: &str) -> bool {
        let mut stale = Vec::new();
        for (id, entry) in live.connections.iter_mut() {
            if matches!(&entry.state, State::Bound(bound) if bound.caller_id == caller_id) {
                entry.state = State::Stale;
                stale.push(*id);
            }
        }
        if stale.is_empty() {
            return false;
        }
        close_iterators(live, &stale);
        true
    }

    /// The registry `binding` names, with any rotation rollback that could
    /// not be written put back first, and overlap retirement and expiry
    /// applied (section 4, generation advance 2 and 5): each retired
    /// digest is removed, a caller whose `expires_at` has passed is marked
    /// once, and either advances its generation. The advance is written and
    /// every binding of the caller torn down under one live lock. Called
    /// with the files lock held, at every check that reads a caller's
    /// authority; a write that fails is the check's failure.
    pub(crate) fn load_current(&self, binding: &Binding) -> Result<registry::RegistryFile, Class> {
        let mut file = registry::load(binding)?;
        self.reconcile_rollbacks(binding, &mut file)?;
        let now = self.host.now_ms();
        let mut moved = Vec::new();
        for caller in &mut file.callers {
            if caller.pending_provision() {
                continue;
            }
            let kept = caller.secrets.len();
            caller.secrets.retain(|digest| digest.live(now));
            let retired = caller.secrets.len() != kept;
            let expired = caller.expires_at_ms <= now && !caller.expiry_advanced;
            if expired {
                caller.expiry_advanced = true;
            }
            if retired || expired {
                caller.authority_generation += 1;
                moved.push(caller.caller_id.clone());
            }
        }
        if moved.is_empty() {
            return Ok(file);
        }
        let mut live = self.live();
        registry::save(binding, &file)?;
        let mut woke = false;
        for caller_id in &moved {
            woke |= self.stale_in(&mut live, caller_id);
        }
        drop(live);
        if woke {
            self.changed.notify_waiters();
        }
        Ok(file)
    }

    /// Settles every rotation still marked in the registry
    /// ([`registry::CallerRecord::rollback`]): one whose rollback write
    /// failed, or one the process ended in the middle of. A rotation whose
    /// new secret `caller.json` holds is kept; any other has its old record
    /// put back. Until a write that puts a record back succeeds the check
    /// fails, so no authority is read from a registry that names an
    /// undelivered secret (`Class::RotationRollbackPending`).
    fn reconcile_rollbacks(
        &self,
        binding: &Binding,
        file: &mut registry::RegistryFile,
    ) -> Result<(), Class> {
        let mut rolled_back = false;
        let mut committed = false;
        for caller in &mut file.callers {
            let Some(before) = caller.rollback.take() else {
                continue;
            };
            if crate::service_callers::handoff::holds_newest(caller) {
                if caller.handoff_path != before.handoff_path {
                    crate::service_callers::handoff::remove_kept(&before);
                }
                committed = true;
            } else {
                *caller = *before;
                caller.rollback = None;
                rolled_back = true;
            }
        }
        if !rolled_back && !committed {
            return Ok(());
        }
        match registry::save(binding, file) {
            Ok(()) => Ok(()),
            // A kept rotation reads the same with or without its mark.
            Err(_) if !rolled_back => Ok(()),
            Err(class) => Err(class),
        }
    }

    /// The periodic check: retirement and expiry for every caller, even
    /// one no call has checked since.
    pub(crate) fn sweep_lifecycle(&self) {
        let Ok(binding) = Binding::current(&*self.host) else {
            return;
        };
        let _files = self.files();
        if !binding.registry().exists() {
            return;
        }
        let _ = self.load_current(&binding);
    }

    /// Sign-out: every binding goes stale and every challenge is dropped;
    /// the connections stay known, so none becomes untouched again.
    pub(crate) fn stale_all(&self, live: &mut Live) {
        for entry in live.connections.values_mut() {
            match &mut entry.state {
                State::Bound(_) => entry.state = State::Stale,
                State::Challenged { slot, .. } => *slot = None,
                _ => {}
            }
        }
    }

    /// The core reports `connection_id` closed. An external entry becomes
    /// the `Closed` marker (removed once its calls drain); an untouched one
    /// stays untouched. Whether it was external.
    pub(crate) fn close_connection(&self, live: &mut Live, connection_id: u64) -> bool {
        let Some(entry) = live.connections.get_mut(&connection_id) else {
            return false;
        };
        if !entry.state.external() {
            return false;
        }
        entry.state = State::Closed;
        if entry.in_flight == 0 {
            live.connections.remove(&connection_id);
        }
        true
    }
}
