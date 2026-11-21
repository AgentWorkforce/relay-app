//! The urgent receipt journal on disk: load (blocking on unusable storage),
//! prune and save.

use super::*;

/// The journal's schema version; a file with a higher one blocks in place.
const VERSION: u8 = 1;

/// Why a journal could not be loaded.
enum Fault {
    /// The file is there but could not be opened or read.
    Io,
    /// Read in full, but too large, not JSON or invalid.
    Damaged,
    /// Written by a newer Agent Relay, with a higher schema version.
    Newer,
}

/// What the person sees while `fault` blocks urgent delivery: the file and
/// how to repair it.
fn fault_text(fault: Fault, path: &Path) -> String {
    let path = path.display();
    match fault {
        Fault::Io => format!(
            "Urgent delivery unavailable: receipt storage exists but cannot be read ({path}). \
             Check the file's permissions and the disk; Agent Relay tries it again at its next check."
        ),
        Fault::Damaged => format!(
            "Urgent delivery unavailable: receipt storage is damaged ({path}). \
             Update Agent Relay if a newer version wrote it; otherwise move it aside to start \
             fresh (a note it recorded may then reach an agent twice)."
        ),
        Fault::Newer => format!(
            "Urgent delivery unavailable: receipt storage was written by a newer version of \
             Agent Relay ({path}). Update Agent Relay and leave the file in place."
        ),
    }
}

/// The journal at `path`. A missing file is a fresh start; `Damaged` only for
/// a file read in full whose content is bad; an I/O error on the file is
/// `Io` (its receipts may be fine).
fn read(path: &Path) -> Result<Saved, Fault> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Saved {
                version: VERSION,
                ..Saved::default()
            })
        }
        Err(_) => return Err(Fault::Io),
    };
    if file.metadata().map_err(|_| Fault::Io)?.len() > MAX_BYTES {
        return Err(Fault::Damaged);
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Fault::Io)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Fault::Damaged);
    }
    parse(&bytes)
}

/// The version is read first, leniently, so a newer file is told apart from
/// a damaged one whatever fields it has.
fn parse(bytes: &[u8]) -> Result<Saved, Fault> {
    #[derive(Deserialize)]
    struct Versioned {
        version: u64,
    }
    let version = serde_json::from_slice::<Versioned>(bytes)
        .map_err(|_| Fault::Damaged)?
        .version;
    if version > u64::from(VERSION) {
        return Err(Fault::Newer);
    }
    let saved: Saved = serde_json::from_slice(bytes).map_err(|_| Fault::Damaged)?;
    let invalid = saved.version != VERSION
        || records(&saved) > MAX_RECORDS
        || saved
            .agents
            .iter()
            .any(|r| !r.target.is_valid() || !plain_id(&r.id))
        || saved
            .humans
            .iter()
            .any(|b| b.ids.is_empty() || b.ids.iter().any(|id| !plain_id(id)));
    if invalid {
        return Err(Fault::Damaged);
    }
    Ok(saved)
}

/// Every receipt and notified id the journal holds.
fn records(saved: &Saved) -> usize {
    saved.agents.len() + saved.humans.iter().map(|b| b.ids.len()).sum::<usize>()
}

/// Reserved, accepted or in doubt: the provider may still hold the note.
fn unsettled(attempt: &Attempt) -> bool {
    matches!(
        attempt,
        Attempt::Reserved | Attempt::Accepted | Attempt::Uncertain
    )
}

impl Journal {
    /// A MISSING file is a fresh start (nothing was ever handed over from
    /// this app data). A file that is there but unusable blocks urgent agent
    /// delivery and stays in place, never set aside or overwritten: its
    /// receipts may show notes a provider still holds, and starting fresh
    /// could submit them again. The urgent pass loads it again while blocked
    /// ([`Advisories::retry_blocked_journal`]), so a fault that clears, or a
    /// file the person repaired, unblocks without a restart.
    pub(in crate::coordination_advisories) fn load(directory: &Path) -> Self {
        let path = directory.join(FILE);
        let (saved, fault) = match read(&path) {
            Ok(saved) => (saved, None),
            Err(fault) => (Saved::default(), Some(fault_text(fault, &path))),
        };
        Self {
            saved,
            status: "idle",
            fault,
            observed: HashMap::new(),
            unreached: HashSet::new(),
        }
    }

    /// Whether urgent agent delivery is blocked by unusable receipt storage.
    pub(in crate::coordination_advisories) fn blocked(&self) -> bool {
        self.fault.is_some()
    }

    /// Unsettled receipts (reserved, accepted, in doubt) are not dropped by
    /// age: a listing is one capped, filtered page, so an id missing from it
    /// is no proof the provider let the note go; the urgent pass releases one
    /// held past [`ACCEPTED_OWNED_FOR`] to the turn end instead. Settled ones
    /// (delivered, refused, retry, released) go past RETENTION unless the id
    /// is still among this process's cached rows for that account and session
    /// (`cached`); an id that comes back after that is a new event. At
    /// capacity, see [`Journal::evict_at_capacity`].
    pub(super) fn prune(&mut self, now: u64, cached: &HashSet<(receipts::Scope, Target, String)>) {
        self.saved.agents.retain(|r| {
            unsettled(&r.attempt)
                || now.saturating_sub(r.at) < RETENTION
                || cached.contains(&(r.scope.clone(), r.target.clone(), r.id.clone()))
        });
        self.observed.retain(|_, at| now.saturating_sub(*at) < HOUR);
        self.saved
            .humans
            .retain(|r| now.saturating_sub(r.at) < RETENTION);
        self.evict_at_capacity(now, cached);
    }

    /// A backstop for a journal near MAX_RECORDS: unsettled receipts held past
    /// [`ACCEPTED_OWNED_FOR`] whose row is no longer cached go too, oldest
    /// first, until one more batch of MAX_ROWS fits. One whose row is still
    /// cached stays: the urgent pass releases it to the turn end, while
    /// dropping it would let the urgent path inject it again. Still full after
    /// that, save() keeps failing closed and nothing is injected.
    fn evict_at_capacity(&mut self, now: u64, cached: &HashSet<(receipts::Scope, Target, String)>) {
        let over = records(&self.saved).saturating_sub(MAX_RECORDS - MAX_ROWS);
        if over == 0 {
            return;
        }
        let mut stale: Vec<(u64, usize)> = self
            .saved
            .agents
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                unsettled(&r.attempt)
                    && now.saturating_sub(r.at) >= ACCEPTED_OWNED_FOR
                    && !cached.contains(&(r.scope.clone(), r.target.clone(), r.id.clone()))
            })
            .map(|(index, r)| (r.at, index))
            .collect();
        stale.sort_unstable();
        let mut evicted: Vec<usize> = stale.into_iter().take(over).map(|(_, i)| i).collect();
        // Highest index first, so each removal leaves the rest in place.
        evicted.sort_unstable_by(|a, b| b.cmp(a));
        for index in evicted {
            self.saved.agents.remove(index);
        }
    }

    pub(super) fn save(&mut self, directory: &Path) -> bool {
        if self.blocked() || records(&self.saved) > MAX_RECORDS {
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
    pub(super) fn agent_receipt(
        &self,
        principal: &Principal,
        target: &Target,
        id: &str,
    ) -> Option<(Attempt, u64)> {
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
