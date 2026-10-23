use super::publish::publish_response_bytes;
use std::path::Path;
use terminal_snapshot_renderer::canonical_timestamp;
use terminal_snapshot_renderer::encode_host_failure_payload;
use terminal_snapshot_renderer::TerminalSnapshotFormat;
use terminal_snapshot_renderer::TerminalSnapshotReasonCode;
use uuid::Uuid;

pub(super) fn record_host_ingress_failure(
    request_id: Option<String>,
    format: Option<TerminalSnapshotFormat>,
    reason: TerminalSnapshotReasonCode,
) {
    let status = match reason {
        TerminalSnapshotReasonCode::InvalidRequest
        | TerminalSnapshotReasonCode::RequesterUnavailable
        | TerminalSnapshotReasonCode::TerminalSnapshotsDisabled
        | TerminalSnapshotReasonCode::NotAuthorized => "rejected",
        _ => "failed",
    };
    crate::api::audit::record_terminal_snapshot(
        &crate::api::audit::TerminalSnapshotAuditMetadata {
            event: "terminal_snapshot".to_string(),
            request_id,
            requester_fqn: None,
            target_fqn: None,
            source_plane: "host_cli".to_string(),
            format: format.map(|format| format.to_string()),
            selected_session_id: None,
            selected_backend: None,
            rows: None,
            columns: None,
            sequence: None,
            captured_at: None,
            payload_bytes: None,
            accepted_at: None,
            completed_at: canonical_timestamp(chrono::Utc::now()),
            status: status.to_string(),
            reason_code: Some(reason.as_str().to_string()),
        },
    );
}

pub(super) fn publish_correlated_ingress_failure(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: Uuid,
    confirmation_tag: &str,
    format: Option<TerminalSnapshotFormat>,
    reason: TerminalSnapshotReasonCode,
) {
    let published = publish_trusted_failure(
        state,
        response_directory,
        request_id,
        confirmation_tag,
        reason,
    );
    record_host_ingress_failure(
        Some(request_id.to_string()),
        format,
        if published.is_ok() {
            reason
        } else {
            TerminalSnapshotReasonCode::ResponseUnavailable
        },
    );
}

pub(super) fn publish_host_outcome(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: Uuid,
    confirmation_tag: &str,
    outcome: Result<Vec<u8>, TerminalSnapshotReasonCode>,
) -> Result<(), TerminalSnapshotReasonCode> {
    #[cfg(test)]
    state.run_host_cancellation_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotHostCancellationStage::BeforePublish,
    );
    match outcome {
        Ok(bytes) => publish_response_bytes(state, response_directory, request_id, &bytes),
        Err(reason) => publish_trusted_failure(
            state,
            response_directory,
            request_id,
            confirmation_tag,
            reason,
        ),
    }
}

pub(super) fn publish_trusted_failure(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: Uuid,
    confirmation_tag: &str,
    reason: TerminalSnapshotReasonCode,
) -> Result<(), TerminalSnapshotReasonCode> {
    let expires_at = canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60));
    let bytes = encode_host_failure_payload(
        &request_id.to_string(),
        confirmation_tag,
        &expires_at,
        reason,
    )
    .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    publish_response_bytes(state, response_directory, request_id, &bytes)
}
