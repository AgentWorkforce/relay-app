//! Usage observations around uploader reads.
use super::*;

pub(super) async fn sessions_with_usage(
    core: &Arc<Core>,
    account: &Account,
    limit: Option<usize>,
) -> Result<UploadSessions, RpcError> {
    let uploader = &account.uploader;
    let revision = uploader.revision();
    let sample = account
        .upload_sample
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1;
    let result = upload_sessions(core, account, uploader, limit).await;
    if revision == uploader.revision() {
        if let Ok(listing) = &result {
            let count = account
                .upload_observations
                .lock()
                .map(|mut observations| observations.observe(revision, sample, &listing.sessions))
                .unwrap_or_default();
            if count > 0 {
                telemetry::record_for(
                    account,
                    TelemetryEvent::new(
                        TelemetryOperation::UploadCompleted,
                        TelemetryOutcome::Succeeded,
                    )
                    .count(count),
                );
            }
        }
    }
    result
}

pub(super) async fn transcript_with_usage(
    account: &Account,
    request: agent_relay_node::relay::contract::transcript::LocalTranscriptParams,
) -> Result<agent_relay_node::relay::contract::transcript::LocalTranscriptPage, RpcError> {
    let uploader = &account.uploader;
    let first_page = request.cursor.is_none() && !request.following;
    let read = async {
        uploader
            .transcript(request)
            .await
            .and_then(read_answer::<agent_relay_node::relay::contract::transcript::LocalTranscriptPage>)
    };
    if first_page {
        telemetry::action(
            account,
            TelemetryEvent::new(
                TelemetryOperation::TranscriptOpen,
                TelemetryOutcome::Started,
            ),
            read,
        )
        .await
    } else {
        read.await
    }
}
