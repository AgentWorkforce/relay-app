//! One batch POST at a time across this OS user's processes, including a
//! shared receiver-directed cooldown. Jobs keep their own leases and retries.
//! The scope deliberately covers a whole workspace: StoredAuth does not
//! retain the service's user ID, and parsing a bearer is not an identity API.
use super::{cloud, DeliveryFailure, ReceiverFailure};
use agent_relay_node::platform::fs as private;
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

#[derive(Debug)]
pub(super) enum Deferred {
    Busy,
    Until { at: i64, failure: DeliveryFailure },
}
impl std::fmt::Display for Deferred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("delivery POST deferred by another local job")
    }
}
impl std::error::Error for Deferred {}
impl Deferred {
    pub(super) fn verdict(&self) -> ReceiverFailure {
        match *self {
            Self::Busy => ReceiverFailure {
                deferred: true,
                local_wait_ms: Some(1_000),
                ..DeliveryFailure::Transient.into()
            },
            Self::Until { at, failure } => ReceiverFailure {
                deferred: true,
                retry_after_ms: Some(at),
                ..failure.into()
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Cooldown {
    at: i64,
    failure: DeliveryFailure,
}

/// The locked inode is never removed or replaced: its OS lock is released
/// even on process death, and the deadline survives a restart. No credential,
/// payload, job or origin identifier is stored here.
pub(super) struct PostGate(File);
impl PostGate {
    pub(super) fn acquire(auth: &cloud::StoredAuth) -> Result<Self> {
        // Collector installs and one-shot flushes use different auth homes.
        // Coordination must instead live under the shared OS profile.
        let directory = agent_relay_node::platform::home_dir()
            .context("delivery POST coordinator needs an OS profile")?
            .join(".agentworkforce/delivery-posts");
        Self::open(&directory, auth, now_ms())
    }

    fn open(directory: &Path, auth: &cloud::StoredAuth, now: i64) -> Result<Self> {
        let scope = serde_json::to_vec(&(
            cloud::normalized_stage(&auth.base_url)?,
            auth.org_id.as_deref(),
            auth.workspace_id
                .as_deref()
                .filter(|value| !value.is_empty()),
        ))?;
        private::create_private_dir_all(directory)?;
        let metadata = fs::symlink_metadata(directory)?;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && private::owned_by_current_user(&metadata)
                && private::private_to_owner(&metadata, 0o077),
            "delivery POST coordinator is not private"
        );
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        let file = private::no_follow(private::private(&mut options))
            .open(directory.join(format!("{}.lock", super::hash(&scope))))?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && private::owned_by_current_user(&metadata)
                && private::private_to_owner(&metadata, 0o077),
            "delivery POST coordinator is not private"
        );
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(error) if private::lock_held(&error) => return Err(Deferred::Busy.into()),
            Err(error) => return Err(error.into()),
        }
        let mut gate = Self(file);
        if let Some(cooldown) = gate.cooldown()? {
            if cooldown.at > now {
                return Err(Deferred::Until {
                    at: cooldown.at,
                    failure: cooldown.failure,
                }
                .into());
            }
        }
        Ok(gate)
    }

    fn cooldown(&mut self) -> Result<Option<Cooldown>> {
        let len = self.0.metadata()?.len();
        if len == 0 {
            return Ok(None);
        }
        if !matches!(len, 8 | 9) {
            return self.repair();
        }
        self.0.seek(SeekFrom::Start(0))?;
        let mut bytes = [0; 9];
        self.0.read_exact(&mut bytes[..len as usize])?;
        let mut timestamp = [0; 8];
        timestamp.copy_from_slice(&bytes[..8]);
        let at = i64::from_le_bytes(timestamp);
        let failure = match (len, bytes[8]) {
            // Older timestamps had no class. Keep their deadline and report
            // generic unavailability instead of asserting a rate limit.
            (8, _) | (9, 0) => DeliveryFailure::Transient,
            (9, 1) => DeliveryFailure::RateLimited,
            _ => return self.repair(),
        };
        if at < 0 {
            return self.repair();
        }
        Ok(Some(Cooldown { at, failure }))
    }

    /// A torn timestamp must not strand every job forever. Only repair while
    /// holding the same OS lock; ownership and file permissions were checked.
    fn repair(&mut self) -> Result<Option<Cooldown>> {
        self.0.set_len(0)?;
        self.0.sync_data()?;
        Ok(None)
    }

    /// Persist the receiver's minimum delay before releasing the POST slot.
    /// An I/O failure fails the send closed; it never becomes an ACK.
    pub(super) fn defer_until(
        &mut self,
        until: Option<i64>,
        failure: DeliveryFailure,
    ) -> Result<()> {
        let Some(until) = until else { return Ok(()) };
        let cooldown = match self.cooldown()? {
            Some(previous) if previous.at > until => previous,
            _ => Cooldown { at: until, failure },
        };
        let kind = match cooldown.failure {
            DeliveryFailure::Transient => 0,
            DeliveryFailure::RateLimited => 1,
            _ => anyhow::bail!("invalid delivery POST cooldown class"),
        };
        let mut bytes = [0; 9];
        bytes[..8].copy_from_slice(&cooldown.at.to_le_bytes());
        bytes[8] = kind;
        self.0.seek(SeekFrom::Start(0))?;
        self.0.write_all(&bytes)?;
        self.0.set_len(bytes.len() as u64)?;
        self.0.sync_data()?;
        Ok(())
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests;
