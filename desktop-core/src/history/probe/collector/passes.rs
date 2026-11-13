use super::*;

/// Historical projection seeding starts only after capture attempted its
/// watcher registration. Paused selected jobs deliberately do no member pass.
pub(super) fn wait_for_capture_driver(
    live: &crate::history::probe::live::Shared,
    cancelled: &AtomicBool,
    paused_selected: bool,
) {
    while !paused_selected
        && !stopping(cancelled)
        && !matches!(live.verdict(), Some(Some(_)))
        && live
            .report()
            .is_none_or(|report| report["driver"].is_null())
    {
        live.wait(Instant::now() + Duration::from_millis(50), || {
            stopping(cancelled)
        });
    }
}

/// One bounded delivery pass through the shared core delivery worker — the
/// same loop the SDK drains with — carrying the RelayHistory receiver. The
/// worker owns leases and their keepalive, prepared-payload persistence, the
/// eligibility recheck before dispatch, retry classification and receipt
/// compaction;
/// the receiver owns only the legacy-scheduler/account/instance guards and the
/// transport. Returns the job status the drain left behind.
pub(super) fn deliver(
    session: &mut worker::DrainSession,
    config: &Config,
    cancelled: &AtomicBool,
    oops_window_ms: Option<i64>,
) -> Result<delivery::DeliveryStatus> {
    let receiver = destination::RelayHistoryReceiver {
        base_url: Some(config.history_url.clone()),
        expected_account: Some(config.delivery_account.clone()),
        instance_id: Some(INSTANCE.into()),
        acknowledge_uninspected_schedules: config.acknowledge_uninspected_schedules,
        state: receiver_state().clone(),
        direct_upload: cloud::cloud_api_url(Some(&format!("{}/cloud", config.site_url)))
            .ok()
            .map(|api_url| destination::DirectUploadSource {
                api_url,
                workspace_id: config.workspace_id.clone(),
                admission_base_url: std::env::var("RELAYHISTORY_ADMISSION_BASE_URL").ok(),
            }),
    };
    receiver
        .state
        .set_acquisition_account(Some(crate::history::acquisition::account_for(config)));
    reconcile_acquisition(session, config);
    deliver_on(
        session,
        config,
        &receiver,
        &|| stopping(cancelled),
        oops_window_ms,
    )
}

/// The least time between two acquisition passes: the issuer paces each
/// session's own retries, this keeps a busy collector from asking for the
/// enrolled sources on every pass.
const ACQUISITION_INTERVAL: Duration = Duration::from_secs(60);

/// Settle ownership for the sessions held waiting for it, before the drain:
/// claim or rebind their grants through the issuer and verify them by
/// status, and let go of the held records of every session whose own grant
/// the issuer shows active, so this drain
/// sends them. It runs only while something is held, and does nothing for a
/// source the server has not enrolled. Best effort: any failure leaves the
/// records held for the next pass, and only its class is logged, never a
/// grant or the installation proof.
fn reconcile_acquisition(session: &mut worker::DrainSession, config: &Config) {
    use crate::history::acquisition;
    static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    let Ok(conn) = session.connection() else {
        return;
    };
    let held = match delivery::held_sessions(conn) {
        Ok(held) if !held.is_empty() => held,
        _ => return,
    };
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|last| last.elapsed() < ACQUISITION_INTERVAL) {
            return;
        }
        *last = Some(Instant::now());
    }
    let Ok(Some(auth)) = cloud::load_selected_auth(Some(&config.history_url)) else {
        return;
    };
    reconcile_signed_in(
        conn,
        config,
        auth,
        &held,
        acquisition::HttpIssuer::new,
        &acquisition::Store::default_location(),
        worker::system_clock(),
    );
}

/// [`reconcile_acquisition`] under the selected sign-in `auth`, for the
/// `held` sessions: the issuer is made from `auth` only once the signed-in
/// scope is the exact account this job delivers to, and the pass's
/// releases and new hold reasons are applied to the queue.
pub(super) fn reconcile_signed_in<I: crate::history::acquisition::AcquisitionIssuer>(
    conn: &Connection,
    config: &Config,
    auth: cloud::StoredAuth,
    held: &[crate::history::acquisition::HeldSession],
    issuer: impl FnOnce(cloud::StoredAuth) -> I,
    store: &crate::history::acquisition::Store,
    now_ms: i64,
) {
    use crate::history::acquisition;
    let Ok(Some(origin_id)) = delivery::origin_id(conn) else {
        return;
    };
    // Grants belong to the exact account this job delivers to (site, user,
    // org and workspace), and to no other: the signed-in scope must be it.
    let account = acquisition::account_for(config);
    if auth.org_id.as_deref() != Some(account.org.as_str())
        || auth.workspace_id.as_deref().unwrap_or_default() != account.workspace
        || destination::account_id(&account.org, Some(&account.workspace))
            != config.delivery_account
    {
        return;
    }
    let issuer = issuer(auth);
    match acquisition::reconcile(&issuer, store, &account, &origin_id, held, now_ms) {
        Ok(done) => {
            // Said once a sign-in; the status carries it from then on.
            if done.newly_required {
                eprintln!("acquisition waits for sign-in: the session lacks session ownership");
            }
            for held in &done.relabeled {
                if held.reason == acquisition::hold_reason::RENEWAL_UNSUPPORTED {
                    eprintln!("acquisition renewal unsupported: grant expired; sessions stay held");
                }
                if delivery::relabel_held(conn, &held.source, &held.session_id, &held.reason)
                    .is_err()
                {
                    eprintln!("acquisition hold update deferred: local storage unavailable");
                }
            }
            for (source, session_id) in done.released {
                if delivery::release_held(conn, &source, &session_id).is_err() {
                    eprintln!("acquisition release deferred: local storage unavailable");
                }
            }
        }
        Err(error) => match error.downcast_ref::<acquisition::IssuerError>() {
            Some(error) => eprintln!("acquisition pass stopped: {error}"),
            None => eprintln!("acquisition pass stopped: local state unavailable"),
        },
    }
}
#[cfg(test)]
pub(super) fn deliver_with_receiver(
    db_path: &Path,
    config: &Config,
    receiver: &dyn worker::Receiver,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<delivery::DeliveryStatus> {
    deliver_on(
        &mut worker::DrainSession::open(db_path)?,
        config,
        receiver,
        cancelled,
        None,
    )
}
/// The operator policy's oops window for this delivery pass: it holds each
/// finished session's records locally for a while. `None` when the pass
/// sends nothing: a policy that is withheld or cannot be read holds
/// everything, and the core reports it as withheld.
pub(super) fn pass_window(config: &Config) -> Option<Option<i64>> {
    pass_window_in(
        &crate::history::retention::app_data_dir(),
        &config.workspace_id,
    )
}
/// [`pass_window`] under the operator policy cached in `app_data_dir` for
/// `workspace_id`. The same read records the revision this pass's uploads
/// name (`operator_policy::pass_revision`).
fn pass_window_in(app_data_dir: &Path, workspace_id: &str) -> Option<Option<i64>> {
    use crate::history::operator_policy::{pass_policy, UploadHold};
    match pass_policy(app_data_dir, workspace_id) {
        UploadHold::None => Some(None),
        UploadHold::Window(window) => Some(Some(window)),
        UploadHold::Blocked => None,
    }
}
/// A delivery pass under the operator policy cached in `app_data_dir`, as
/// the collector makes one: `None` when the policy lets the pass send nothing.
#[cfg(test)]
pub(super) fn deliver_under_policy(
    db_path: &Path,
    config: &Config,
    receiver: &dyn worker::Receiver,
    app_data_dir: &Path,
) -> Result<Option<delivery::DeliveryStatus>> {
    let Some(oops_window_ms) = pass_window_in(app_data_dir, &config.workspace_id) else {
        return Ok(None);
    };
    let session = &mut worker::DrainSession::open(db_path)?;
    deliver_on(session, config, receiver, &|| false, oops_window_ms).map(Some)
}
/// One delivery pass. `oops_window_ms` holds each session's records that
/// long after its last activity (the operator policy's oops window).
fn deliver_on(
    session: &mut worker::DrainSession,
    config: &Config,
    receiver: &dyn worker::Receiver,
    cancelled: &(dyn Fn() -> bool + Sync),
    oops_window_ms: Option<i64>,
) -> Result<delivery::DeliveryStatus> {
    // Registered agent names reach the store before the drain fills a batch,
    // so this pass's change feed carries them through filling and scrub like
    // any other change. An idle pass whose names are unchanged does nothing.
    if let Some(directory) = session.path().parent() {
        crate::history::registered_names::before_delivery(directory, super::super::history::open);
    }
    let options = worker::DrainOptions {
        oops_window_ms,
        ..delivery_options(
            config.job_id.clone(),
            format!("probe-{}", std::process::id()),
        )
    };
    let result = session.drain(
        &worker::SingleReceiver::new(DESTINATION, INSTANCE, receiver),
        &options,
        &worker::system_clock,
        cancelled,
    )?;
    let status = result
        .statuses
        .into_iter()
        .find(|job| job.job_id == config.job_id)
        .context("delivery job missing")?;
    PassMetrics::record(&result.metrics, &status);
    // Pause can arrive while a batch is in flight. It is a successful user
    // control action, even if it invalidated that batch's lease.
    if status.state == "paused" {
        return Ok(status);
    }
    if status.state != "active" {
        if status.failure.as_deref() == Some("delivery_conflict") {
            return Err(
                destination::TransportFailure(delivery::DeliveryFailure::DeliveryConflict).into(),
            );
        }
        return Err(user_error(
            "Delivery needs attention. Reconnect with --force-login if credentials have expired.",
        ));
    }
    // A recorded transport failure is not an issue: the job stays active and
    // the worker owns its backoff. Only an unusable local state is reported,
    // and waiting for a retry deadline is normal operation, not a fault.
    // The drain's own error keeps its cause: a locked, full or damaged store
    // is reported as such, and a batch that could not be prepared as that.
    if let Some(failure) = result.failure {
        return Err(failure.into_error());
    }
    if result
        .issues
        .iter()
        .any(|issue| issue.code == worker::DrainIssueCode::PreparationFailed)
    {
        return Err(anyhow::anyhow!("an upload batch could not be prepared")
            .context(delivery::PreparationFailed));
    }
    if !result.issues.is_empty() {
        return Err(user_error(
            "Session delivery is queued or needs attention. No unacknowledged data was discarded.",
        ));
    }
    Ok(status)
}

/// A blocked job holds the verdict of the probe build that reached it, and a
/// different build can judge the same exchange differently — a receipt it now
/// parses, say. The first run under each new build retries a blocked job once;
/// later runs of that build leave its own verdicts alone.
/// The database records the build atomically with the retry, including across
/// detached collectors and restarts when the legacy file marker is unwritable.
pub(super) fn retry_verdict_of_another_build(
    directory: &Path,
    job_id: &str,
    version: &str,
) -> Result<()> {
    let marker = directory.join("verdict-retry.json");
    let retried = fs::read(&marker)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        // Unscoped files cannot prove which generation used the retry. In
        // particular, disconnect/reconnect reuses this directory for a new job.
        .filter(|value| value["job_id"].as_str() == Some(job_id))
        .and_then(|value| value["probe_version"].as_str().map(str::to_owned));
    let conn = delivery::open_for_capture(&directory.join("history.db"))?;
    if delivery::retry_job_for_build(&conn, job_id, version, retried.as_deref()).is_err() {
        // This recovery is optional. Its transaction rolls back both the retry
        // and build record on failure, so collection can continue safely.
        let _ = writeln!(
            std::io::stderr(),
            "Automatic probe recovery was deferred. Collection will continue."
        );
        return Ok(());
    }
    // Maintain the older marker for probes that do not yet read retry_build.
    if save_json(
        &marker,
        &json!({ "job_id": job_id, "probe_version": version }),
    )
    .is_err()
    {
        // Keep diagnostics free of paths, credentials and raw filesystem errors.
        let _ = writeln!(
            std::io::stderr(),
            "Probe retry file could not be saved. Retry state is stored in the database."
        );
    }
    Ok(())
}

/// What the RelayHistory receiver keeps across this process's passes: its
/// stage session, its legacy scheduler verdict and the batch limits the
/// server last published. Kept for the process rather than per pass, so
/// back-to-back passes neither reread the session nor inspect the
/// schedulers each time, and batches stay sized to the published limits.
pub(super) fn receiver_state() -> &'static destination::ReceiverState {
    static STATE: std::sync::OnceLock<destination::ReceiverState> = std::sync::OnceLock::new();
    STATE.get_or_init(destination::ReceiverState::default)
}
