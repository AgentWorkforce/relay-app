//! The urgent receipt journal on disk: load (blocking on unusable storage),
//! prune and save.

use super::*;

/// Why a journal could not be loaded.
enum Fault {
    /// The file is there but could not be opened or read.
    Io,
    /// Read in full, but too large, not JSON, an unknown version or invalid.
    Unreadable,
}

impl Journal {
    pub(in crate::coordination_advisories) fn load(directory: &Path) -> Self {
        use std::io::Read;
        // `Unreadable` only for a file read in full whose content is bad; an I/O
        // error on the file keeps the journal blocked (its receipts may be fine).
        let read = || -> Result<Saved, Fault> {
            let file = match std::fs::File::open(directory.join(FILE)) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Saved {
                        version: 1,
                        ..Saved::default()
                    })
                }
                Err(_) => return Err(Fault::Io),
            };
            if file.metadata().map_err(|_| Fault::Io)?.len() > MAX_BYTES {
                return Err(Fault::Unreadable);
            }
            let mut bytes = Vec::new();
            file.take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| Fault::Io)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(Fault::Unreadable);
            }
            let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| Fault::Unreadable)?;
            if saved.version != 1
                || saved.agents.len() + saved.humans.iter().map(|b| b.ids.len()).sum::<usize>()
                    > MAX_RECORDS
            {
                return Err(Fault::Unreadable);
            }
            if saved
                .agents
                .iter()
                .any(|r| !r.target.is_valid() || !plain_id(&r.id))
                || saved
                    .humans
                    .iter()
                    .any(|b| b.ids.is_empty() || b.ids.iter().any(|id| !plain_id(id)))
            {
                return Err(Fault::Unreadable);
            }
            Ok(saved)
        };
        // A MISSING file is a fresh start (nothing was ever handed over from
        // this app data). A file that is there but unusable blocks urgent agent
        // delivery and stays in place, so every launch blocks the same way
        // until it is repaired: its receipts may show notes a provider still
        // holds, and starting fresh could submit them again.
        let (saved, status, blocked) = match read() {
            Ok(saved) => (saved, "idle", false),
            Err(Fault::Io) => (
                Saved::default(),
                "Urgent delivery unavailable: receipt storage exists but cannot be read.",
                true,
            ),
            Err(Fault::Unreadable) => (
                Saved::default(),
                "Urgent delivery unavailable: receipt storage is damaged or from another version.",
                true,
            ),
        };
        Self {
            saved,
            status,
            blocked,
            observed: HashMap::new(),
        }
    }

    /// Whether urgent agent delivery is blocked by unusable receipt storage.
    pub(in crate::coordination_advisories) fn blocked(&self) -> bool {
        self.blocked
    }

    /// Unsettled receipts (reserved, accepted, in doubt) are never dropped: a
    /// listing is one capped, filtered page, so an id missing from it is no
    /// proof the provider let the note go. Settled ones (delivered, refused,
    /// retry) go past RETENTION unless the id is still among this process's
    /// cached rows for that account and session (`cached`); an id that comes
    /// back after that is a new event. Cost: unsettled receipts of kinds with
    /// no observer stay for good and count against MAX_RECORDS; a full journal
    /// fails closed in save().
    pub(super) fn prune(&mut self, now: u64, cached: &HashSet<(receipts::Scope, Target, String)>) {
        self.saved.agents.retain(|r| {
            matches!(r.attempt, Attempt::Reserved | Attempt::Accepted | Attempt::Uncertain)
                || now.saturating_sub(r.at) < RETENTION
                || cached.contains(&(r.scope.clone(), r.target.clone(), r.id.clone()))
        });
        self.observed.retain(|_, at| now.saturating_sub(*at) < HOUR);
        self.saved
            .humans
            .retain(|r| now.saturating_sub(r.at) < RETENTION);
    }

    pub(super) fn save(&mut self, directory: &Path) -> bool {
        if self.blocked
            || self.saved.agents.len()
                + self.saved.humans.iter().map(|b| b.ids.len()).sum::<usize>()
                > MAX_RECORDS
        {
            self.status = "Urgent delivery unavailable: receipt storage is blocked or full.";
            return false;
        }
        let Some(bytes) = serde_json::to_vec(&self.saved)
            .ok()
            .filter(|b| b.len() as u64 <= MAX_BYTES)
        else {
            self.status = "Urgent delivery unavailable: receipt storage is full.";
            return false;
        };
        let ok = agent_relay_node::platform::fs::create_private_dir_all(directory).is_ok()
            && agent_relay_node::platform::fs::write_atomic(&directory.join(FILE), &bytes).is_ok();
        if !ok {
            self.status = "Urgent delivery unavailable: receipt storage could not be saved.";
        }
        ok
    }

    /// The last recorded attempt for this note, and when (epoch seconds).
    pub(super) fn agent_receipt(&self, principal: &Principal, target: &Target, id: &str) -> Option<(Attempt, u64)> {
        let scope = receipts::Scope::from(principal);
        self.saved
            .agents
            .iter()
            .find(|r| r.scope == scope && &r.target == target && r.id == id)
            .map(|r| (r.attempt.clone(), r.at))
    }

    pub(super) fn set_agent(
        &mut self,
        principal: &Principal,
        target: &Target,
        id: &str,
        attempt: Attempt,
        now: u64,
    ) {
        let scope = receipts::Scope::from(principal);
        if let Some(r) = self
            .saved
            .agents
            .iter_mut()
            .find(|r| r.scope == scope && &r.target == target && r.id == id)
        {
            r.attempt = attempt;
            r.at = now;
        } else {
            self.saved.agents.push(AgentReceipt {
                scope,
                target: target.clone(),
                id: id.into(),
                at: now,
                attempt,
            });
        }
    }
}
