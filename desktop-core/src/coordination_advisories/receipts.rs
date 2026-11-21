//! Minimal receipts survive a normal restart while upstream ACKs are retried.
//! Disk writes run only in the background acknowledgement path. A crash between
//! hook output and its background checkpoint can still redeliver: at least once.

use super::*;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

const FILE: &str = "coordination-advisory-receipts.json";
const MAX_RECEIPTS: usize = 4096;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
// Longer than the server's seven-day advisory retention.
const RETENTION: u64 = 8 * 24 * 60 * 60;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub(super) struct Scope {
    site: String,
    workspace_id: String,
    user_id: Option<String>,
}

impl From<&Principal> for Scope {
    fn from(principal: &Principal) -> Self {
        Self {
            site: principal.site.clone(),
            workspace_id: principal.workspace_id.clone(),
            user_id: principal.user_id.clone(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    scope: Scope,
    target: Target,
    advisory_id: String,
    shown_at: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    receipts: Vec<Receipt>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn valid(receipt: &Receipt, now: u64) -> bool {
    let scope = &receipt.scope;
    !scope.site.is_empty()
        && scope.site.len() <= 2048
        && !scope.workspace_id.is_empty()
        && scope.workspace_id.len() <= 512
        && scope
            .user_id
            .as_ref()
            .is_none_or(|user| !user.is_empty() && user.len() <= 512)
        && receipt.target.is_valid()
        && receipt.target.id.len() <= 512
        && plain_id(&receipt.advisory_id)
        && receipt.shown_at <= now
        && now - receipt.shown_at < RETENTION
}

pub(super) fn load(directory: &Path) -> Vec<Receipt> {
    let read = || -> Option<Vec<Receipt>> {
        let file = std::fs::File::open(directory.join(FILE)).ok()?;
        if file.metadata().ok()?.len() > MAX_FILE_BYTES {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return None;
        }
        let saved: Saved = serde_json::from_slice(&bytes).ok()?;
        if saved.version != 1 || saved.receipts.len() > MAX_RECEIPTS {
            return None;
        }
        let now = unix_now();
        Some(
            saved
                .receipts
                .into_iter()
                .filter(|r| valid(r, now))
                .collect(),
        )
    };
    read().unwrap_or_default()
}

pub(super) fn restore(state: &mut State, principal: &Principal) {
    let scope = Scope::from(principal);
    let now = unix_now();
    for receipt in &state.receipts {
        if receipt.scope != scope || !valid(receipt, now) {
            continue;
        }
        let mut taken = Taken::new(Instant::now());
        taken.shown = true;
        state
            .taken
            .entry(receipt.target.clone())
            .or_default()
            .insert(receipt.advisory_id.clone(), taken);
    }
}

/// IDs that already have a durable shown receipt for this account.
pub(super) fn saved(state: &State, principal: &Principal) -> HashSet<(Target, String)> {
    let scope = Scope::from(principal);
    let now = unix_now();
    state
        .receipts
        .iter()
        .filter(|r| r.scope == scope && valid(r, now))
        .map(|r| (r.target.clone(), r.advisory_id.clone()))
        .collect()
}

/// Keep the newest receipts within both bounds, rather than refusing every
/// checkpoint once old scopes fill the file. Evicted IDs remain retryable
/// in the live cache, and may be saved by a later checkpoint as ACKs drain.
fn bounded(mut rows: Vec<Receipt>) -> Option<(Saved, Vec<u8>)> {
    rows.sort_by_key(|r| r.shown_at);
    let mut size = serde_json::to_vec(&Saved {
        version: 1,
        receipts: Vec::new(),
    })
    .ok()?
    .len();
    let mut kept = Vec::new();
    for row in rows.into_iter().rev().take(MAX_RECEIPTS) {
        let extra = serde_json::to_vec(&row).ok()?.len() + usize::from(!kept.is_empty());
        if size + extra > MAX_FILE_BYTES as usize {
            break;
        }
        size += extra;
        kept.push(row);
    }
    kept.reverse();
    let saved = Saved {
        version: 1,
        receipts: kept,
    };
    let bytes = serde_json::to_vec(&saved).ok()?;
    Some((saved, bytes))
}

impl Advisories {
    /// Serialize checkpoints so an older snapshot cannot overwrite a newer ACK.
    /// Called in the background, never by pending/take/shown.
    pub(super) fn checkpoint_receipts(&self, principal: &Principal) -> bool {
        let _save = self.receipt_save.lock().unwrap_or_else(|p| p.into_inner());
        let state = self.state();
        if state.principal.as_ref() != Some(principal)
            || self.tokens.principal().as_ref() != Some(principal)
        {
            return false;
        }
        let scope = Scope::from(principal);
        let now = unix_now();
        let mut rows: Vec<Receipt> = state
            .receipts
            .iter()
            .filter(|r| r.scope != scope && valid(r, now))
            .cloned()
            .collect();
        let previous: HashMap<(&Target, &str), u64> = state
            .receipts
            .iter()
            .filter(|r| r.scope == scope)
            .map(|r| ((&r.target, r.advisory_id.as_str()), r.shown_at))
            .collect();
        for (target, taken) in &state.taken {
            for (id, taken) in taken {
                if !taken.shown || taken.acked {
                    continue;
                }
                let shown_at = previous.get(&(target, id.as_str())).copied().unwrap_or(now);
                let receipt = Receipt {
                    scope: scope.clone(),
                    target: target.clone(),
                    advisory_id: id.clone(),
                    shown_at,
                };
                if valid(&receipt, now) {
                    rows.push(receipt);
                }
            }
        }
        let Some((saved, bytes)) = bounded(rows) else {
            return false;
        };
        // Hooks only hold `state` briefly; never make them wait for filesystem I/O.
        drop(state);
        if agent_relay_node::platform::fs::create_private_dir_all(&self.app_data_dir).is_err()
            || agent_relay_node::platform::fs::write_atomic(&self.app_data_dir.join(FILE), &bytes)
                .is_err()
        {
            debug(format_args!("shown receipts could not be checkpointed"));
            return false;
        }
        self.state().receipts = saved.receipts;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> Receipt {
        Receipt {
            scope: Scope {
                site: "https://control.example".into(),
                workspace_id: "example-workspace".into(),
                user_id: Some("example-user".into()),
            },
            target: Target::new(SessionKind::Codex, "11111111-1111-4111-8111-111111111111"),
            advisory_id: "example-advisory".into(),
            shown_at: unix_now(),
        }
    }

    #[test]
    fn overflowing_receipt_count_keeps_the_newest_instead_of_refusing_to_save() {
        let now = unix_now();
        let rows = (0..MAX_RECEIPTS + 1)
            .map(|i| Receipt {
                advisory_id: format!("receipt-{i}"),
                shown_at: now - i as u64,
                ..receipt()
            })
            .collect();
        let (saved, bytes) = bounded(rows).unwrap();
        assert_eq!(saved.receipts.len(), MAX_RECEIPTS);
        assert_eq!(saved.receipts.last().unwrap().advisory_id, "receipt-0");
        assert!(!saved
            .receipts
            .iter()
            .any(|r| r.advisory_id == format!("receipt-{MAX_RECEIPTS}")));
        assert!(bytes.len() <= MAX_FILE_BYTES as usize);
    }

    #[test]
    fn overflowing_receipt_bytes_also_evicts_the_oldest() {
        let now = unix_now();
        let rows = (0..MAX_RECEIPTS)
            .map(|i| {
                let mut row = receipt();
                row.advisory_id = format!("receipt-{i}");
                row.shown_at = now - i as u64;
                row.scope.site = format!("https://{}.example", "a".repeat(2000));
                row.scope.workspace_id = "w".repeat(512);
                row.scope.user_id = Some("u".repeat(512));
                assert!(valid(&row, now));
                row
            })
            .collect();
        let (saved, bytes) = bounded(rows).unwrap();
        assert!(!saved.receipts.is_empty() && saved.receipts.len() < MAX_RECEIPTS);
        assert_eq!(saved.receipts.last().unwrap().advisory_id, "receipt-0");
        assert!(bytes.len() <= MAX_FILE_BYTES as usize);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), bytes).unwrap();
        assert_eq!(load(dir.path()).len(), saved.receipts.len());
    }

    #[test]
    fn malformed_oversized_and_expired_receipts_are_not_restored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, "not json").unwrap();
        assert!(load(dir.path()).is_empty());
        let mut row = receipt();
        row.shown_at = unix_now() - RETENTION;
        std::fs::write(
            &path,
            serde_json::to_vec(&Saved {
                version: 1,
                receipts: vec![row],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(load(dir.path()).is_empty());
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(load(dir.path()).is_empty());
    }

    #[test]
    fn restoration_requires_the_same_site_workspace_and_person_but_not_generation() {
        let row = receipt();
        let principal = Principal {
            site: row.scope.site.clone(),
            workspace_id: row.scope.workspace_id.clone(),
            user_id: row.scope.user_id.clone(),
            generation: 999,
        };
        for other in [
            Principal {
                site: "https://other.example".into(),
                ..principal.clone()
            },
            Principal {
                workspace_id: "other-workspace".into(),
                ..principal.clone()
            },
            Principal {
                user_id: Some("other-person".into()),
                ..principal.clone()
            },
        ] {
            let mut state = State {
                receipts: vec![row.clone()],
                ..State::default()
            };
            state.belong_to(Some(&other));
            assert!(state.taken.is_empty());
        }
        let mut state = State {
            receipts: vec![row.clone()],
            ..State::default()
        };
        state.belong_to(Some(&principal));
        assert!(state.taken[&row.target][&row.advisory_id].shown);
    }
}
