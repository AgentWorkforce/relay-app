//! The bridge methods for the account, the uploader, Skip chat, contractor
//! access, telemetry and launch at login. Every type is the registry's (`relay::contract`); see
//! `docs/relay-core.md` for the contract.

use crate::account::Account;
use crate::contract::account::{JournalCompacted, SessionsUpdated, SharingChanged, UploadPreview, UploadSessions, UploaderDisconnected, UploaderPaused, UploaderStarted, UploadsRetried};
use agent_relay_node::relay::contract::{answer, Done};
use agent_relay_node::relay::daemon::Core;
use agent_relay_node::relay::protocol::{ErrorKind, RpcError};
use agent_relay_node::relay::state::Listing;
use crate::telemetry::{
    self, TelemetryEvent, TelemetryFailure, TelemetryOperation, TelemetryOutcome,
};
use crate::history::uploader::read_answer;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;

mod tools;
mod usage;
use usage::{sessions_with_usage, transcript_with_usage};

/// The telemetry failure code for an uploader command's error.
fn failure(error: &RpcError) -> TelemetryFailure {
    if error.message.contains("took too long") {
        TelemetryFailure::TimedOut
    } else if error.message == crate::history::uploader::MISSING {
        TelemetryFailure::HelperMissing
    } else {
        TelemetryFailure::HelperFailed
    }
}

/// Runs an uploader command, records it, and tells subscribers the
/// uploader changed.
async fn tracked<T: DeserializeOwned>(
    core: &Arc<Core>,
    account: &Account,
    event: TelemetryEvent,
    work: impl Future<Output = Result<Value, RpcError>>,
) -> Result<T, RpcError> {
    let result = work.await.and_then(read_answer::<T>);
    match &result {
        Ok(_) => telemetry::record_for(account, event),
        Err(error) if error.data.kind == ErrorKind::NotSignedIn => {}
        Err(error) => telemetry::record_for(
            account,
            TelemetryEvent {
                outcome: TelemetryOutcome::Failed,
                failure: Some(failure(error)),
                ..event
            },
        ),
    }
    if result.is_ok() {
        core.hub.invalidated(Listing::Uploader);
    }
    result
}

/// Records a failed read the person did not ask for by name.
fn track_read<T>(account: &Account, operation: TelemetryOperation, result: &Result<T, RpcError>) {
    if let Err(error) = result {
        if error.data.kind != ErrorKind::NotSignedIn {
            account.track(operation, TelemetryOutcome::Failed, Some(failure(error)));
        }
    }
}

/// Adds the core's filesystem-aware project identity to raw uploader rows.
/// `Some("")` deliberately distinguishes relay infrastructure from an old
/// core that did not send this field and whose clients retain their legacy
/// folder-name fallback.
async fn resolve_projects(
    names: &agent_relay_node::relay::project::Names,
    sessions: &mut [crate::contract::account::UploadSession],
) {
    let resolved = names
        .list(sessions.iter().filter_map(|session| session.cwd.as_deref()))
        .await;
    for session in sessions {
        session.project = Some(
            session
                .cwd
                .as_ref()
                .and_then(|cwd| resolved.get(cwd).cloned().flatten())
                .unwrap_or_default(),
        );
    }
}

async fn upload_sessions(
    core: &Core,
    account: &Account,
    uploader: &crate::history::uploader::Uploader,
    limit: Option<usize>,
) -> Result<UploadSessions, RpcError> {
    let mut result = uploader
        .sessions_list(limit)
        .await
        .and_then(read_answer::<UploadSessions>);
    if let Ok(result) = &mut result {
        resolve_projects(&core.project_names, &mut result.sessions).await;
    }
    track_read(account, TelemetryOperation::RefreshSessions, &result);
    result
}

async fn upload_preview(
    core: &Core,
    uploader: &crate::history::uploader::Uploader,
    limit: Option<usize>,
    refresh: bool,
) -> Result<UploadPreview, RpcError> {
    let mut result = uploader
        .sessions_preview(limit, refresh)
        .await
        .and_then(read_answer::<UploadPreview>)?;
    resolve_projects(&core.project_names, &mut result.sessions).await;
    Ok(result)
}

/// Answers an account or uploader method, for [`Account`]'s plugin.
pub(crate) async fn account_call(
    core: &Arc<Core>,
    account: &Arc<Account>,
    method: &str,
    raw: Value,
) -> Option<Result<Value, RpcError>> {
    if let Some(result) = tools::call(account, method, &raw).await {
        return Some(result);
    }
    let uploader = account.uploader.clone();
    if let Some(result) = account_actions(core, account, method, raw.clone()).await {
        return Some(result);
    }
    Some(match method {
        crate::contract::method::AccountStatus::NAME => {
            answer::<crate::contract::method::AccountStatus, _>(raw, |_| async {
                account.recheck_auth_health().await;
                Ok(account.report())
            })
            .await
        }
        crate::contract::method::AccountSignIn::NAME => {
            answer::<crate::contract::method::AccountSignIn, _>(raw, |request| account.sign_in(request)).await
        }
        crate::contract::method::AccountSelectWorkspace::NAME => {
            answer::<crate::contract::method::AccountSelectWorkspace, _>(raw, |request| async move {
                account
                    .select_workspace(&request.workspace_id)
                    .map(|_| Done {})
            })
            .await
        }
        crate::contract::method::AccountSignInCancel::NAME => {
            answer::<crate::contract::method::AccountSignInCancel, _>(raw, |_| async {
                account.cancel_sign_in();
                Ok(Done {})
            })
            .await
        }
        crate::contract::method::AccountSignOut::NAME => {
            answer::<crate::contract::method::AccountSignOut, _>(raw, |_| async {
                let result = account.sign_out().await.map(|_| Done {});
                for what in [Listing::Chat, Listing::Projects] {
                    core.hub.invalidated(what);
                }
                result
            })
            .await
        }
        crate::contract::method::AccountConnectWorkspace::NAME => {
            answer::<crate::contract::method::AccountConnectWorkspace, _>(raw, |request| async move {
                account.connect_workspace(&request.workspace_id).await
            })
            .await
        }
        crate::contract::method::UploaderStatus::NAME => {
            answer::<crate::contract::method::UploaderStatus, _>(raw, |_| async {
                let result = uploader.cached_status().await;
                track_read(account, TelemetryOperation::RefreshStatus, &result);
                result.map(|status| crate::contract::account::UploaderStatus {
                    enforced_settings: uploader.enforced_settings(),
                    uploads_withheld: uploader.uploads_withheld(),
                    ..crate::history::uploader::status_report(&status)
                })
            })
            .await
        }
        crate::contract::method::UploaderStart::NAME => {
            answer::<crate::contract::method::UploaderStart, _>(raw, |_| {
                tracked::<UploaderStarted>(
                    core,
                    account,
                    TelemetryEvent::new(
                        TelemetryOperation::StartUploader,
                        TelemetryOutcome::Succeeded,
                    ),
                    uploader.start(),
                )
            })
            .await
        }
        crate::contract::method::UploaderDisconnect::NAME => {
            answer::<crate::contract::method::UploaderDisconnect, _>(raw, |_| async {
                let result = tracked::<UploaderDisconnected>(
                    core,
                    account,
                    TelemetryEvent::new(
                        TelemetryOperation::Disconnect,
                        TelemetryOutcome::Succeeded,
                    ),
                    uploader.disconnect(),
                )
                .await;
                if result.is_ok() {
                    account.cancel();
                }
                result
            })
            .await
        }
        crate::contract::method::UploaderSharingSet::NAME => {
            answer::<crate::contract::method::UploaderSharingSet, _>(raw, |request| {
                tracked::<SharingChanged>(
                    core,
                    account,
                    TelemetryEvent::new(
                        TelemetryOperation::SharingMode,
                        TelemetryOutcome::Succeeded,
                    )
                    .mode(request.mode),
                    uploader.sharing_set(request.mode),
                )
            })
            .await
        }
        crate::contract::method::UploaderSessionsList::NAME => {
            answer::<crate::contract::method::UploaderSessionsList, _>(raw, |request| async move {
                sessions_with_usage(core, account, request.limit).await
            })
            .await
        }
        crate::contract::method::SessionsTranscript::NAME => {
            answer::<crate::contract::method::SessionsTranscript, _>(raw, |request| async move {
                transcript_with_usage(account, request).await
            })
            .await
        }
        crate::contract::method::UploaderSessionsPreview::NAME => {
            answer::<crate::contract::method::UploaderSessionsPreview, _>(raw, |request| {
                upload_preview(core, &uploader, request.limit, request.refresh)
            })
            .await
        }
        crate::contract::method::UploaderSessionsInclude::NAME => {
            answer::<crate::contract::method::UploaderSessionsInclude, _>(raw, |request| async move {
                tracked::<SessionsUpdated>(
                    core,
                    account,
                    TelemetryEvent::new(
                        TelemetryOperation::IncludeSessions,
                        TelemetryOutcome::Succeeded,
                    )
                    .count(request.session_keys.len() as i64),
                    uploader.sessions_change(true, &request.session_keys),
                )
                .await
            })
            .await
        }
        crate::contract::method::UploaderSessionsReset::NAME => {
            answer::<crate::contract::method::UploaderSessionsReset, _>(raw, |request| async move {
                let result = uploader
                    .sessions_reset(&request.confirmation)
                    .await
                    .and_then(read_answer::<crate::contract::account::SessionsReset>);
                result
            })
            .await
        }
        crate::contract::method::UploaderSessionsDelete::NAME => {
            answer::<crate::contract::method::UploaderSessionsDelete, _>(raw, |request| async move {
                tracked::<SessionsUpdated>(
                    core,
                    account,
                    TelemetryEvent::new(
                        TelemetryOperation::DeleteSessions,
                        TelemetryOutcome::Succeeded,
                    )
                    .count(request.session_keys.len() as i64),
                    uploader.sessions_delete(&request.session_keys),
                )
                .await
            })
            .await
        }
        crate::contract::method::UploaderSessionsExclude::NAME => {
            answer::<crate::contract::method::UploaderSessionsExclude, _>(raw, |request| async move {
                tracked::<SessionsUpdated>(
                    core,
                    account,
                    TelemetryEvent::new(
                        TelemetryOperation::ExcludeSessions,
                        TelemetryOutcome::Succeeded,
                    )
                    .count(request.session_keys.len() as i64),
                    uploader.sessions_change(false, &request.session_keys),
                )
                .await
            })
            .await
        }
        crate::contract::method::TelemetryRecord::NAME => {
            // Validated alike, then dropped, without telemetry.
            answer::<crate::contract::method::TelemetryRecord, _>(raw, |event| async move {
                account.record_usage(event).await;
                Ok(Done {})
            })
            .await
        }
        _ => return None,
    })
}

/// Small account credential and upload-control actions.
async fn account_actions(
    core: &Arc<Core>,
    account: &Account,
    method: &str,
    raw: Value,
) -> Option<Result<Value, RpcError>> {
    let uploader = &account.uploader;
    Some(match method {
        crate::contract::method::AccountMcpHeaders::NAME => {
            answer::<crate::contract::method::AccountMcpHeaders, _>(raw, |request| account.mcp_headers(request))
                .await
        }
        crate::contract::method::AccountLoginConfig::NAME => {
            answer::<crate::contract::method::AccountLoginConfig, _>(raw, |_| async {
                crate::login_config::for_site(account.site_url())
                    .await
                    .map_err(|_| {
                        RpcError::new(
                            ErrorKind::CloudFailed,
                            "The site's sign-in methods could not be read.",
                        )
                    })
            })
            .await
        }
        crate::contract::method::AccountFeedHeaders::NAME => {
            answer::<crate::contract::method::AccountFeedHeaders, _>(raw, |request| {
                account.feed_headers(request)
            })
            .await
        }

        crate::contract::method::UploaderSessionsFacets::NAME => {
            answer::<crate::contract::method::UploaderSessionsFacets, _>(raw, |request| {
                uploader.sessions_facets(request)
            })
            .await
        }
        crate::contract::method::UploaderSessionsQuery::NAME => {
            answer::<crate::contract::method::UploaderSessionsQuery, _>(raw, |request| async move {
                let local = agent_relay_node::relay::local_methods::sessions_list(core)
                    .await
                    .sessions;
                let archived = core.store.read(|store| store.complete_archived_keys());
                let config = core.config();
                let env = |key: &str| config.env.get(key).cloned();
                let home = config.home.clone();
                let relay_home = agent_relay_node::relay::workspace::relay_home(&env, &home);
                uploader
                    .sessions_query(
                        request,
                        local,
                        archived,
                        home,
                        relay_home,
                        &core.project_names,
                    )
                    .await
            })
            .await
        }

        crate::contract::method::UploaderPause::NAME => {
            answer::<crate::contract::method::UploaderPause, _>(raw, |_| pause(core, account, true)).await
        }
        crate::contract::method::UploaderResume::NAME => {
            answer::<crate::contract::method::UploaderResume, _>(raw, |_| pause(core, account, false)).await
        }
        crate::contract::method::UploaderRetry::NAME => {
            answer::<crate::contract::method::UploaderRetry, _>(raw, |_| {
                let work = uploader.retry();
                tracked::<UploadsRetried>(
                    core,
                    account,
                    TelemetryEvent::new(
                        TelemetryOperation::RetryUploads,
                        TelemetryOutcome::Succeeded,
                    ),
                    work,
                )
            })
            .await
        }
        crate::contract::method::UploaderCompact::NAME => {
            answer::<crate::contract::method::UploaderCompact, _>(raw, |_| {
                let work = uploader.compact();
                tracked::<JournalCompacted>(
                    core,
                    account,
                    TelemetryEvent::new(
                        TelemetryOperation::CompactJournal,
                        TelemetryOutcome::Succeeded,
                    ),
                    work,
                )
            })
            .await
        }
        _ => return None,
    })
}

async fn pause(
    core: &Arc<Core>,
    account: &Account,
    paused: bool,
) -> Result<UploaderPaused, RpcError> {
    let operation = if paused {
        TelemetryOperation::PauseUploader
    } else {
        TelemetryOperation::ResumeUploader
    };
    let result = tracked::<UploaderPaused>(
        core,
        account,
        TelemetryEvent::new(operation, TelemetryOutcome::Succeeded),
        account.uploader.pause(paused),
    )
    .await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::account::UploadSession;
    use serde_json::json;

    fn upload(cwd: &std::path::Path) -> UploadSession {
        serde_json::from_value(json!({
            "source": "codex", "session_id": "s", "title": "Work", "cwd": cwd,
            "is_agent": false, "included": false, "status": "not_shared"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn uploader_rows_carry_resolved_and_explicitly_suppressed_projects() {
        let temp = tempfile::tempdir().unwrap();
        let relay_home = temp.path().join("relay");
        let controller = relay_home.join("test-mini-node");
        let checkout = temp.path().join("relay-desktop-231");
        std::fs::create_dir_all(&controller).unwrap();
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::write(
            checkout.join(".git/config"),
            "[remote \"origin\"]\nurl = https://github.com/AgentWorkforce/relay-desktop.git\n",
        )
        .unwrap();
        let mut rows = vec![upload(&controller), upload(&checkout)];
        resolve_projects(&agent_relay_node::relay::project::Names::new(relay_home), &mut rows).await;
        assert_eq!(rows[0].project.as_deref(), Some(""));
        assert_eq!(rows[1].project.as_deref(), Some("relay-desktop"));
    }
}
