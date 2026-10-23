use super::failure::publish_correlated_ingress_failure;
use super::failure::publish_host_outcome;
use super::failure::publish_trusted_failure;
use super::failure::record_host_ingress_failure;
use super::publish::remove_tracked_processing;
use super::request::HostTerminalSnapshotRequest;
use super::shutdown::ScannerTaskOwnership;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;
use terminal_snapshot_renderer::canonical_timestamp;
use terminal_snapshot_renderer::TerminalSnapshotReasonCode;
use terminal_snapshot_renderer::MAX_REQUEST_BYTES;

#[allow(clippy::too_many_arguments)]
pub(super) async fn process_claimed<R: tauri::Runtime>(
    scanner_ownership: ScannerTaskOwnership,
    app: &tauri::AppHandle<R>,
    snapshot_state: Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>,
    processing: PathBuf,
    response_directory: PathBuf,
    expected_root: crate::path_identity::VerifiedPathIdentity,
    claimed: crate::path_identity::VerifiedPathIdentity,
    filename_request_id: String,
    ingress: tokio::sync::OwnedSemaphorePermit,
) {
    if !scanner_ownership.mark_owned() {
        return;
    }
    #[cfg(test)]
    snapshot_state.run_host_cancellation_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotHostCancellationStage::Processing,
    );
    let read = crate::path_identity::read_bounded_regular(&processing, MAX_REQUEST_BYTES);
    let (bytes, identity) = match read {
        Ok(value) if crate::path_identity::same_object(&claimed, &value.1) => value,
        Ok(_) => {
            record_host_ingress_failure(
                Some(filename_request_id.clone()),
                None,
                TerminalSnapshotReasonCode::ResponseUnavailable,
            );
            return;
        }
        Err(_) => {
            remove_tracked_processing(&snapshot_state, &processing, &claimed);
            record_host_ingress_failure(
                Some(filename_request_id.clone()),
                None,
                TerminalSnapshotReasonCode::InvalidRequest,
            );
            return;
        }
    };
    let request: HostTerminalSnapshotRequest =
        match terminal_snapshot_renderer::decode_bounded(&bytes, MAX_REQUEST_BYTES) {
            Ok(request) => request,
            Err(_) => {
                remove_tracked_processing(&snapshot_state, &processing, &identity);
                record_host_ingress_failure(
                    Some(filename_request_id.clone()),
                    None,
                    TerminalSnapshotReasonCode::InvalidRequest,
                );
                return;
            }
        };
    remove_tracked_processing(&snapshot_state, &processing, &identity);
    let request_id = match request.validate_correlation(&filename_request_id) {
        Ok(request_id) => request_id,
        Err(reason) => {
            record_host_ingress_failure(Some(filename_request_id), None, reason);
            return;
        }
    };
    let (_, token, expires_at) = match request.validate(chrono::Utc::now()) {
        Ok(validated) => validated,
        Err(reason) => {
            publish_correlated_ingress_failure(
                &snapshot_state,
                &response_directory,
                request_id,
                &request.confirmation_tag,
                Some(request.format),
                reason,
            );
            return;
        }
    };
    let publish_error = |reason| {
        publish_correlated_ingress_failure(
            &snapshot_state,
            &response_directory,
            request_id,
            &request.confirmation_tag,
            Some(request.format),
            reason,
        );
    };
    let Some(session_manager) =
        app.try_state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>()
    else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let Some(pty_manager) =
        app.try_state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>()
    else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let Some(settings) = app.try_state::<crate::config::settings::SettingsState>() else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let Some(restore) = app.try_state::<Arc<crate::RestoreInProgress>>() else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let Some(purge) = app.try_state::<Arc<crate::session::purge_guard::PurgeGuard>>() else {
        publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);
        return;
    };
    let remaining = match expires_at
        .signed_duration_since(chrono::Utc::now())
        .to_std()
        .ok()
        .filter(|remaining| !remaining.is_zero())
    {
        Some(remaining) => remaining,
        None => {
            publish_error(TerminalSnapshotReasonCode::SnapshotTimeout);
            return;
        }
    };
    let Some(monotonic_deadline) = std::time::Instant::now().checked_add(remaining) else {
        publish_error(TerminalSnapshotReasonCode::Internal);
        return;
    };
    let context = crate::pty::terminal_snapshot::TerminalSnapshotServiceContext {
        session_manager: session_manager.inner().clone(),
        pty_manager: pty_manager.inner().clone(),
        settings: settings.inner().clone(),
        restore: restore.inner().clone(),
        purge: purge.inner().clone(),
    };
    let service_request = crate::pty::terminal_snapshot::TerminalSnapshotServiceRequest {
        request_id,
        target: request.to.clone(),
        format: request.format,
        source_plane: crate::pty::terminal_snapshot::TerminalSnapshotSourcePlane::HostCli,
        host_authorization_deadline: Some((monotonic_deadline, expires_at)),
    };
    let audit = crate::pty::terminal_snapshot::TerminalSnapshotAuditGuard::pre_admission(
        crate::pty::terminal_snapshot::TerminalSnapshotSourcePlane::HostCli,
    );
    audit.accept_request(&service_request);
    let admission = snapshot_state
        .pre_admit_requester(
            &context,
            crate::pty::terminal_snapshot::TerminalSnapshotRequesterSelector::Host {
                token,
                expected_root,
                claimed_from: request.from.clone(),
            },
            crate::pty::terminal_snapshot::TerminalSnapshotSourcePlane::HostCli,
            service_request.host_authorization_deadline,
            audit.clone(),
        )
        .await;
    let admission = match admission {
        Ok(admission) => admission,
        Err(reason) => {
            drop(ingress);
            audit.wait_for_retained_owners(1).await;
            let published = publish_trusted_failure(
                &snapshot_state,
                &response_directory,
                request_id,
                &request.confirmation_tag,
                reason,
            );
            audit.finalize_failure(if published.is_ok() {
                reason
            } else {
                TerminalSnapshotReasonCode::ResponseUnavailable
            });
            return;
        }
    };
    drop(ingress);
    let prepared = snapshot_state
        .prepare_with_admission(admission, service_request)
        .await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(reason) => {
            audit.wait_for_retained_owners(1).await;
            let published = publish_trusted_failure(
                &snapshot_state,
                &response_directory,
                request_id,
                &request.confirmation_tag,
                reason,
            );
            audit.finalize_failure(if published.is_ok() {
                reason
            } else {
                TerminalSnapshotReasonCode::ResponseUnavailable
            });
            return;
        }
    };
    let (payload, finalization) = prepared.into_parts();
    let response_expires_at =
        canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60));
    let success_bytes = finalization
        .build_host_response(
            payload,
            request_id.to_string(),
            request.confirmation_tag.clone(),
            response_expires_at,
        )
        .await;
    audit.wait_for_retained_owners(2).await;
    #[cfg(test)]
    snapshot_state.run_host_cancellation_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotHostCancellationStage::ResponseBytesReady,
    );
    let response_directory_for_publish = response_directory.clone();
    let state_for_publish = Arc::clone(&snapshot_state);
    let confirmation_tag = request.confirmation_tag.clone();
    scanner_ownership.mark_finalizer();
    let task = match success_bytes {
        Ok(success_bytes) => tokio::task::spawn_blocking(move || {
            crate::logging::catch_payload_unwind(|| {
                finalization.finalize_host(success_bytes, |outcome| {
                    publish_host_outcome(
                        &state_for_publish,
                        &response_directory_for_publish,
                        request_id,
                        &confirmation_tag,
                        outcome,
                    )
                })
            })
        }),
        Err(reason) => tokio::task::spawn_blocking(move || {
            crate::logging::catch_payload_unwind(|| {
                finalization.fail_host(reason, |reason| {
                    publish_trusted_failure(
                        &state_for_publish,
                        &response_directory_for_publish,
                        request_id,
                        &confirmation_tag,
                        reason,
                    )
                })
            })
        }),
    };
    match crate::logging::collapse_payload_task(task.await) {
        Ok(_) => {}
        Err(_) => {
            log::error!("[terminal-snapshot] stage=host_finalizer_task code=internal");
        }
    }
}
