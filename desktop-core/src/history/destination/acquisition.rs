//! What a receiver presents and keeps for acquisition: the grants a batch
//! presents for its protected sessions, the sessions a writer refused, and
//! the sources the server enrolled.

use super::*;
use crate::history::acquisition;

/// The (source, session) of every record of a batch.
pub(super) fn batch_sessions(
    batch: &HistoryExportBatch,
) -> impl Iterator<Item = (&str, Option<&str>)> {
    batch
        .records
        .iter()
        .map(|record| (record.source.as_str(), record.session_id.as_deref()))
}
/// The entries of `acquisition` that name a session `batch` carries: a child
/// of a batch presents its parent's grants for its own sessions only.
pub(super) fn entries_for(
    batch: &HistoryExportBatch,
    acquisition: &[AcquisitionEntry],
) -> Vec<AcquisitionEntry> {
    let sessions: HashSet<(&str, &str)> = batch_sessions(batch)
        .filter_map(|(source, session)| Some((source, session?)))
        .collect();
    acquisition
        .iter()
        .filter(|entry| sessions.contains(&(entry.source.as_str(), entry.session_id.as_str())))
        .cloned()
        .collect()
}
/// The entries of `acquisition` a batch presents, checked against its
/// sessions.
pub(super) fn presented(
    batch: &HistoryExportBatch,
    acquisition: &[AcquisitionEntry],
) -> Result<Vec<AcquisitionEntry>> {
    let acquisition = entries_for(batch, acquisition);
    require(
        acquisition::validate_entries(&acquisition, batch_sessions(batch)),
        DeliveryFailure::InvalidPayload,
    )?;
    Ok(acquisition)
}

/// Check a request against the protocol, its grants included.
pub(super) fn validate_request(request: &Request) -> Result<()> {
    validate(&request.batch, MAX_PUBLISHED_BATCH_RECORDS)?;
    require(
        acquisition::validate_entries(&request.acquisition, batch_sessions(&request.batch)),
        DeliveryFailure::InvalidPayload,
    )
}

/// A writer's acquisition refusal names the sessions it refused. It is the
/// writer's verdict on those sessions, never a delivery conflict and never
/// transient: they are held, the rest of the batch goes again under a new
/// batch id. Logs the rejection either way.
pub(super) fn writer_hold(
    status: u16,
    body: Option<&serde_json::Value>,
    code: Option<&str>,
    bytes: Option<&[u8]>,
) -> Option<Vec<HeldSession>> {
    let hold = body
        .filter(|_| status == 409)
        .and_then(acquisition::contract::writer_refusal)
        .map(|refused| {
            refused
                .into_iter()
                .map(|refused| HeldSession {
                    source: refused.source,
                    session_id: refused.session_id,
                    reason: refused.code,
                })
                .collect::<Vec<_>>()
        });
    if let Some(hold) = &hold {
        eprintln!(
            "receiver refused acquisition status={status} sessions={}",
            hold.len()
        );
    } else {
        eprintln!("{}", rejection_log(status, code, bytes));
    }
    hold
}

impl ReceiverState {
    /// The sources the server last said are enrolled, if it said.
    fn enrolled(&self) -> Option<BTreeSet<String>> {
        self.0
            .enrolled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    /// Name the exact account this receiver delivers for.
    pub fn set_acquisition_account(&self, account: Option<acquisition::Account>) {
        *self
            .0
            .acquisition_account
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = account.filter(acquisition::Account::exact);
    }
    fn acquisition_account(&self) -> Option<acquisition::Account> {
        self.0
            .acquisition_account
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    /// Keep the enrolled sources a limits response or receipt published,
    /// and the last ones for the exact account in the acquisition state, so
    /// a later process that cannot ask goes by them rather than by
    /// "none enrolled". A response that does not mention acquisition leaves
    /// them as they are.
    pub(super) fn publish_enrolled(&self, enrolled: Option<BTreeSet<String>>) {
        let Some(enrolled) = enrolled else {
            return;
        };
        let changed = {
            let mut kept = self.0.enrolled.lock().unwrap_or_else(|e| e.into_inner());
            let changed = kept.as_ref() != Some(&enrolled);
            *kept = Some(enrolled.clone());
            changed
        };
        if let Some(account) = self.acquisition_account().filter(|_| changed) {
            // Best effort: the in-memory list stands for this process.
            let _ = self.acquisition_store().update(|state| {
                state.put_enrolled(&account, &enrolled);
                Ok(())
            });
        }
    }
    fn acquisition_store(&self) -> acquisition::Store {
        self.0
            .acquisition_store
            .clone()
            .map_or_else(acquisition::Store::default_location, acquisition::Store::at)
    }
    /// State whose acquisition store is `directory` and whose server has
    /// enrolled `enrolled`, for tests.
    #[cfg(test)]
    pub(crate) fn acquiring(
        directory: std::path::PathBuf,
        account: Option<acquisition::Account>,
        enrolled: Option<&[&str]>,
    ) -> Self {
        Self(Arc::new(ReceiverStateInner {
            acquisition_store: Some(directory),
            acquisition_account: Mutex::new(account),
            enrolled: Mutex::new(
                enrolled.map(|enrolled| enrolled.iter().map(|s| (*s).to_owned()).collect()),
            ),
            hosted: true,
            ..Default::default()
        }))
    }
    /// Keep the enrolled sources a limits response published, if it
    /// mentions acquisition.
    pub(super) fn enrolled_from(&self, limits: Option<&serde_json::Value>) {
        self.publish_enrolled(limits.and_then(acquisition::contract::enrolled_sources));
    }
}

impl RelayHistoryReceiver {
    /// The grants this batch presents. Nothing applies while no record is
    /// of an enrolled source; a protected session without a usable grant
    /// for this origin is held before anything is sent, and only the
    /// records of that session wait. The enrolled sources are the ones the
    /// server published to this process, or else the last ones kept for
    /// the exact account; only that account's grants are presented.
    pub(super) fn acquisition(
        &self,
        batch: &HistoryExportBatch,
    ) -> Result<Vec<AcquisitionEntry>, ReceiverFailure> {
        let account = self.state.acquisition_account();
        let unavailable = |_| {
            eprintln!("acquisition state unavailable");
            ReceiverFailure::from(DeliveryFailure::Transient)
        };
        let mut loaded = None;
        let enrolled = match (self.state.enrolled(), &account) {
            (Some(enrolled), _) => enrolled,
            (None, Some(account)) => {
                let state = self.state.acquisition_store().load().map_err(unavailable)?;
                let Some(enrolled) = state.enrolled(account) else {
                    return Ok(vec![]);
                };
                loaded = Some(state);
                enrolled
            }
            (None, None) => return Ok(vec![]),
        };
        if !batch
            .records
            .iter()
            .any(|record| enrolled.contains(&record.source))
        {
            return Ok(vec![]);
        }
        let state = match loaded {
            Some(state) => state,
            None => self.state.acquisition_store().load().map_err(unavailable)?,
        };
        let grant = |source: &str, session: &str| {
            state.grants.iter().find(|grant| {
                grant.source == source
                    && grant.session_id == session
                    && Some(&grant.account) == account.as_ref()
                    && account_id(&grant.account.org, Some(&grant.account.workspace))
                        == batch.account_id
            })
        };
        match acquisition::present(batch_sessions(batch), &batch.origin_id, &enrolled, grant) {
            acquisition::Presentation::Send(entries) => Ok(entries),
            acquisition::Presentation::Hold(hold) => Err(ReceiverFailure {
                failure: DeliveryFailure::ReceiverRejected,
                local_wait_ms: None,
                retry_after_ms: None,
                recovery: None,
                hold: Some(hold),
                upload: None,
            }),
        }
    }
}
