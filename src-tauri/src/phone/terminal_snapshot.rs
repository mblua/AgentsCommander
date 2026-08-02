use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use terminal_snapshot_renderer::{
    canonical_timestamp, encode_host_failure_payload, TerminalSnapshotFormat,
    TerminalSnapshotReasonCode, MAX_REQUEST_BYTES, MAX_TRANSPORT_BYTES,
};
use uuid::Uuid;

const REQUEST_KIND: &str = "terminal-snapshot";
const REQUEST_VERSION: u32 = 1;
const REQUEST_DIRECTORY: &str = "terminal-snapshot-requests";
const RESPONSE_DIRECTORY: &str = "terminal-snapshot-responses";
const RESPONSE_TTL: Duration = Duration::from_secs(60);
const MAX_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_PER_POLL: usize = 256;
const CURSOR_CAP: usize = 4_096;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HostTerminalSnapshotRequest {
    pub kind: String,
    pub version: u32,
    pub request_id: String,
    pub token: String,
    pub from: String,
    pub to: String,
    pub format: TerminalSnapshotFormat,
    pub issued_at: String,
    pub expires_at: String,
    pub nonce: String,
    pub confirmation_tag: String,
}

impl std::fmt::Debug for HostTerminalSnapshotRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostTerminalSnapshotRequest")
            .field("version", &self.version)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl HostTerminalSnapshotRequest {
    fn validate_correlation(
        &self,
        filename_request_id: &str,
    ) -> Result<Uuid, TerminalSnapshotReasonCode> {
        let request_id = terminal_snapshot_renderer::validate_uuid(&self.request_id, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        if self.request_id != filename_request_id {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        terminal_snapshot_renderer::validate_hex(&self.nonce, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_hex(&self.confirmation_tag, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        if confirmation_tag(self) != self.confirmation_tag {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        Ok(request_id)
    }

    pub(crate) fn validate(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(Uuid, Uuid, chrono::DateTime<chrono::Utc>), TerminalSnapshotReasonCode> {
        if self.kind != REQUEST_KIND || self.version != REQUEST_VERSION {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        let request_id = terminal_snapshot_renderer::validate_uuid(&self.request_id, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let token = terminal_snapshot_renderer::validate_uuid(&self.token, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_requester_identity(&self.from, true)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_target_syntax(&self.to)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_hex(&self.nonce, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_hex(&self.confirmation_tag, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let issued = terminal_snapshot_renderer::validate_timestamp(&self.issued_at)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let expires = terminal_snapshot_renderer::validate_timestamp(&self.expires_at)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let lifetime = expires.signed_duration_since(issued);
        let lifetime_ms = lifetime.num_milliseconds();
        if !(5_000..=30_000).contains(&lifetime_ms)
            || lifetime_ms % 1_000 != 0
            || issued > now + chrono::Duration::seconds(5)
            || now >= expires
            || confirmation_tag(self) != self.confirmation_tag
        {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        Ok((request_id, token, expires))
    }
}

pub(crate) fn confirmation_tag(request: &HostTerminalSnapshotRequest) -> String {
    use sha2::{Digest, Sha256};
    let format = request.format.to_string();
    let fields: [&[u8]; 8] = [
        b"ac-terminal-snapshot-confirmation-v1",
        request.request_id.as_bytes(),
        request.nonce.as_bytes(),
        request.from.as_bytes(),
        request.to.as_bytes(),
        format.as_bytes(),
        request.issued_at.as_bytes(),
        request.expires_at.as_bytes(),
    ];
    let mut digest = Sha256::new();
    for field in fields {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    format!("{:x}", digest.finalize())
}

#[derive(Default)]
pub(crate) struct SnapshotMailboxScanner {
    cursors: HashMap<crate::path_identity::FileObjectId, String>,
    observed: std::collections::HashSet<crate::path_identity::FileObjectId>,
    startup_sweep_complete: bool,
    #[cfg(test)]
    pending_tasks: Vec<tauri::async_runtime::JoinHandle<()>>,
}

pub(crate) fn verified_requester_root_from_discovered_path(path: &Path) -> Option<PathBuf> {
    if let Ok(root) = crate::config::root_agent::verify_live_root_agent_path(path) {
        return Some(root.canonical_path);
    }
    crate::config::teams::verify_pty_input_replica_cwd(path)
        .ok()
        .map(|identity| identity.replica_root)
}

impl SnapshotMailboxScanner {
    pub(crate) fn begin_cycle(&mut self) {
        self.observed.clear();
    }

    pub(crate) fn startup_sweep_pending(&self) -> bool {
        !self.startup_sweep_complete
    }

    pub(crate) fn finish_startup_sweep(&mut self) {
        self.startup_sweep_complete = true;
    }

    pub(crate) fn startup_sweep_root(
        &mut self,
        state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
        requester_root: &Path,
    ) {
        let Ok(root_identity) = crate::path_identity::verify_directory(requester_root) else {
            return;
        };
        let local = requester_root.join(crate::config::agent_local_dir_name());
        let outbox = local.join("outbox");
        let directories = [
            (outbox.join(REQUEST_DIRECTORY), false),
            (local.join(RESPONSE_DIRECTORY), true),
        ];
        for (directory, response_directory) in directories {
            let Ok(directory_identity) = crate::path_identity::verify_directory(&directory) else {
                continue;
            };
            if !private_directory_mode(&directory)
                || !crate::path_identity::is_verified_descendant(
                    &directory_identity,
                    &root_identity,
                )
            {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for (index, entry) in entries.enumerate() {
                if index >= MAX_DIRECTORY_ENTRIES {
                    log::warn!("[terminal-snapshot] stage=cleanup code=directory_capacity");
                    break;
                }
                let Ok(entry) = entry else {
                    break;
                };
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if !protocol_cleanup_name(name, response_directory)
                    || !private_regular_file_mode(&path)
                {
                    continue;
                }
                let Some(age) = std::fs::symlink_metadata(&path)
                    .ok()
                    .filter(|metadata| metadata.is_file())
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| modified.elapsed().ok())
                else {
                    continue;
                };
                let Ok(identity) = crate::path_identity::verify_regular_file(&path) else {
                    continue;
                };
                if age >= RESPONSE_TTL {
                    safe_remove(&path, &identity);
                    continue;
                }
                let Ok(reservation) = state.reserve_existing_artifact(
                    &directory,
                    &directory_identity,
                    identity.object_id,
                ) else {
                    continue;
                };
                let ttl = RESPONSE_TTL.saturating_sub(age);
                let _ = reservation.commit_with_ttl(path, identity, ttl);
            }
        }
    }

    pub(crate) fn finish_cycle(&mut self) {
        self.cursors
            .retain(|object, _| self.observed.contains(object));
    }

    #[cfg(test)]
    pub(crate) async fn join_pending_tasks_for_test(&mut self) {
        for task in std::mem::take(&mut self.pending_tasks) {
            task.await.expect("terminal snapshot mailbox task");
        }
    }

    pub(crate) fn scan_root<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        requester_root: &Path,
    ) {
        let Ok(root_identity) = crate::path_identity::verify_directory(requester_root) else {
            return;
        };
        let local = requester_root.join(crate::config::agent_local_dir_name());
        let outbox = local.join("outbox");
        let request_directory = outbox.join(REQUEST_DIRECTORY);
        let response_directory = local.join(RESPONSE_DIRECTORY);
        if !request_directory.is_dir() || !response_directory.is_dir() {
            return;
        }
        let (
            Ok(local_identity),
            Ok(outbox_identity),
            Ok(request_directory_identity),
            Ok(response_directory_identity),
        ) = (
            crate::path_identity::verify_directory(&local),
            crate::path_identity::verify_directory(&outbox),
            crate::path_identity::verify_directory(&request_directory),
            crate::path_identity::verify_directory(&response_directory),
        )
        else {
            return;
        };
        if !private_directory_mode(&request_directory)
            || !private_directory_mode(&response_directory)
            || !crate::path_identity::is_verified_descendant(&local_identity, &root_identity)
            || !crate::path_identity::is_verified_descendant(&outbox_identity, &local_identity)
            || !crate::path_identity::is_verified_descendant(
                &request_directory_identity,
                &outbox_identity,
            )
            || !crate::path_identity::is_verified_descendant(
                &response_directory_identity,
                &local_identity,
            )
        {
            return;
        }
        if self.cursors.len() >= CURSOR_CAP
            && !self
                .cursors
                .contains_key(&request_directory_identity.object_id)
        {
            return;
        }
        self.observed.insert(request_directory_identity.object_id);
        sweep_directory(&request_directory, &request_directory_identity, false);
        sweep_directory(&response_directory, &response_directory_identity, true);
        let cursor = self
            .cursors
            .get(&request_directory_identity.object_id)
            .cloned()
            .unwrap_or_default();
        let mut next = std::collections::BTreeSet::new();
        let mut wrapped = std::collections::BTreeSet::new();
        let Ok(entries) = std::fs::read_dir(&request_directory) else {
            return;
        };
        for (index, entry) in entries.enumerate() {
            if index >= MAX_DIRECTORY_ENTRIES {
                log::warn!("[terminal-snapshot] stage=publish code=directory_capacity");
                return;
            }
            let Ok(entry) = entry else {
                return;
            };
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if protocol_request_name(&name).is_none() {
                continue;
            }
            let candidates = if name.as_str() > cursor.as_str() {
                &mut next
            } else {
                &mut wrapped
            };
            candidates.insert(name);
            if candidates.len() > MAX_PER_POLL {
                candidates.pop_last();
            }
        }
        let selected: Vec<String> = if next.is_empty() {
            wrapped.into_iter().collect()
        } else {
            next.into_iter().collect()
        };
        if let Some(last) = selected.last() {
            self.cursors
                .insert(request_directory_identity.object_id, last.clone());
        }

        let Some(snapshot_state) =
            app.try_state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
        else {
            return;
        };
        for name in selected {
            let source_key = format!(
                "host:{:016x}:{:016x}",
                request_directory_identity.object_id.volume,
                request_directory_identity.object_id.file
            );
            let Ok(ingress) = snapshot_state.try_admit_ingress(source_key) else {
                break;
            };
            let request_id = protocol_request_name(&name).unwrap_or_default().to_string();
            let request_path = request_directory.join(&name);
            let processing = request_directory.join(format!(
                ".{}.{}.terminal-snapshot-processing",
                request_id,
                Uuid::new_v4()
            ));
            if !private_regular_file_mode(&request_path) {
                continue;
            }
            let Ok(original) = crate::path_identity::verify_regular_file(&request_path) else {
                continue;
            };
            let Ok(reservation) = snapshot_state.reserve_existing_artifact(
                &request_directory,
                &request_directory_identity,
                original.object_id,
            ) else {
                break;
            };
            if crate::path_identity::publish_new_file_atomic(&request_path, &processing).is_err() {
                continue;
            }
            let Ok(claimed) = crate::path_identity::verify_regular_file(&processing) else {
                continue;
            };
            if !crate::path_identity::same_object(&original, &claimed) {
                continue;
            }
            if reservation
                .commit(processing.clone(), claimed.clone())
                .is_err()
            {
                safe_remove(&processing, &claimed);
                continue;
            }
            let app = app.clone();
            let response_directory = response_directory.clone();
            let expected_root = root_identity.clone();
            let snapshot_state = snapshot_state.inner().clone();
            let task = tauri::async_runtime::spawn(async move {
                let processed = crate::logging::catch_payload_future(process_claimed(
                    &app,
                    snapshot_state,
                    processing,
                    response_directory,
                    expected_root,
                    claimed,
                    request_id,
                    ingress,
                ))
                .await;
                if processed.is_err() {
                    log::error!("[terminal-snapshot] stage=host_task code=internal");
                }
            });
            #[cfg(test)]
            self.pending_tasks.push(task);
            #[cfg(not(test))]
            drop(task);
        }
    }
}

fn private_directory_mode(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.permissions().mode() & 0o777 == 0o700)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn private_regular_file_mode(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn protocol_request_name(name: &str) -> Option<&str> {
    let id = name.strip_suffix(".json")?;
    terminal_snapshot_renderer::validate_uuid(id, Some(4)).ok()?;
    Some(id)
}

#[allow(clippy::too_many_arguments)]
async fn process_claimed<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    snapshot_state: Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>,
    processing: PathBuf,
    response_directory: PathBuf,
    expected_root: crate::path_identity::VerifiedPathIdentity,
    claimed: crate::path_identity::VerifiedPathIdentity,
    filename_request_id: String,
    ingress: tokio::sync::OwnedSemaphorePermit,
) {
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
    #[cfg(test)]
    snapshot_state.run_host_cancellation_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotHostCancellationStage::ResponseBytesReady,
    );
    let response_directory_for_publish = response_directory.clone();
    let state_for_publish = Arc::clone(&snapshot_state);
    let confirmation_tag = request.confirmation_tag.clone();
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

fn record_host_ingress_failure(
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

fn publish_correlated_ingress_failure(
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

fn publish_host_outcome(
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

fn publish_trusted_failure(
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

fn publish_response_bytes(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: Uuid,
    bytes: &[u8],
) -> Result<(), TerminalSnapshotReasonCode> {
    if bytes.is_empty() || bytes.len() > MAX_TRANSPORT_BYTES {
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    let validated_directory = crate::path_identity::verify_directory(response_directory)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::BeforeDirectoryRetention,
        response_directory,
        response_directory,
    );
    let retained_directory = crate::path_identity::retain_directory(response_directory)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    if !crate::path_identity::same_object(&validated_directory, retained_directory.identity()) {
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    let directory_identity = retained_directory.identity().clone();
    let reservation = state
        .reserve_artifact(response_directory, &directory_identity)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    let temporary = response_directory.join(format!(
        ".{}.{}.terminal-snapshot-response-tmp",
        request_id,
        Uuid::new_v4()
    ));
    let destination = response_directory.join(format!("{request_id}.json"));
    if destination.parent() != Some(response_directory) || destination.exists() {
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::BeforeTemporaryCreate,
        &temporary,
        &destination,
    );
    let mut file = retained_directory
        .create_new_private_file(&temporary)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    let temporary_identity = retained_directory
        .verify_opened_regular_file(&temporary, &file, true)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::BeforeTemporaryCommit,
        &temporary,
        &destination,
    );
    if reservation
        .commit(temporary.clone(), temporary_identity.clone())
        .is_err()
    {
        retained_directory.remove_regular_file_if_same(&temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterTemporaryCommit,
        &temporary,
        &destination,
    );
    if retained_directory.verify_current().is_err() {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .is_err()
            || file
                .metadata()
                .ok()
                .is_none_or(|metadata| metadata.permissions().mode() & 0o777 != 0o600)
        {
            cleanup_tracked_artifact(state, &temporary, &temporary_identity);
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
    }
    if file.write_all(bytes).and_then(|_| file.flush()).is_err() {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    let file_sync_failed = state.take_response_publication_failure(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationFailure::FileSync,
    );
    #[cfg(not(test))]
    let file_sync_failed = false;
    if file_sync_failed || file.sync_all().is_err() {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterTemporaryWrite,
        &temporary,
        &destination,
    );
    let written_identity = match retained_directory
        .verify_current()
        .and_then(|_| retained_directory.verify_opened_regular_file(&temporary, &file, false))
    {
        Ok(identity)
            if crate::path_identity::same_object(&identity, &temporary_identity)
                && crate::path_identity::is_verified_descendant(&identity, &directory_identity) =>
        {
            identity
        }
        _ => {
            cleanup_tracked_artifact(state, &temporary, &temporary_identity);
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
    };
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::BeforeAtomicRename,
        &temporary,
        &destination,
    );
    if retained_directory
        .publish_new_file_atomic(&temporary, &destination)
        .is_err()
    {
        cleanup_tracked_artifact(state, &temporary, &written_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterAtomicRename,
        &temporary,
        &destination,
    );
    let mut published = reconcile_published_response(
        state,
        &retained_directory,
        &destination,
        &file,
        &written_identity,
        &directory_identity,
    )?;
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterFinalVerification,
        &temporary,
        &destination,
    );
    if published.is_some() {
        published = reconcile_published_response(
            state,
            &retained_directory,
            &destination,
            &file,
            &written_identity,
            &directory_identity,
        )?;
    }
    if published.is_some() {
        published = match state.relocate_artifact(&written_identity, &destination) {
            Ok(published) => published,
            Err(_) => {
                cleanup_tracked_artifact(state, &destination, &written_identity);
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
        };
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterRegistryCommit,
        &temporary,
        &destination,
    );
    if published.is_some() {
        published = reconcile_published_response(
            state,
            &retained_directory,
            &destination,
            &file,
            &written_identity,
            &directory_identity,
        )?;
    }
    #[cfg(test)]
    let parent_sync_failed = state.take_response_publication_failure(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationFailure::ParentSync,
    );
    #[cfg(not(test))]
    let parent_sync_failed = false;
    if !parent_sync_failed {
        retained_directory.sync_best_effort();
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterParentSync,
        &temporary,
        &destination,
    );
    if published.is_some() {
        let _ = reconcile_published_response(
            state,
            &retained_directory,
            &destination,
            &file,
            &written_identity,
            &directory_identity,
        )?;
    }
    Ok(())
}

fn reconcile_published_response(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    retained_directory: &crate::path_identity::RetainedDirectory,
    destination: &Path,
    file: &std::fs::File,
    expected: &crate::path_identity::VerifiedPathIdentity,
    directory: &crate::path_identity::VerifiedPathIdentity,
) -> Result<Option<crate::path_identity::VerifiedPathIdentity>, TerminalSnapshotReasonCode> {
    match retained_directory
        .verify_current()
        .and_then(|_| retained_directory.verify_opened_regular_file(destination, file, false))
    {
        Ok(identity)
            if crate::path_identity::same_object(expected, &identity)
                && crate::path_identity::is_verified_descendant(&identity, directory) =>
        {
            Ok(Some(identity))
        }
        Err(_)
            if retained_directory.child_is_absent(destination)
                || crate::path_identity::opened_file_is_delete_pending(file) =>
        {
            state.untrack_artifact(expected);
            Ok(None)
        }
        _ => {
            cleanup_tracked_artifact(state, destination, expected);
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        }
    }
}

fn remove_tracked_processing(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    path: &Path,
    expected: &crate::path_identity::VerifiedPathIdentity,
) {
    cleanup_tracked_artifact(state, path, expected);
}

fn cleanup_tracked_artifact(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    path: &Path,
    expected: &crate::path_identity::VerifiedPathIdentity,
) -> bool {
    state.cleanup_artifact(path, expected)
}

#[cfg(test)]
fn path_is_absent(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn safe_remove(path: &Path, expected: &crate::path_identity::VerifiedPathIdentity) -> bool {
    crate::path_identity::remove_regular_file_if_same(path, expected)
}

fn sweep_directory(
    directory: &Path,
    expected_directory: &crate::path_identity::VerifiedPathIdentity,
    response_directory: bool,
) {
    let Ok(retained_directory) = crate::path_identity::retain_directory(directory) else {
        return;
    };
    if !crate::path_identity::same_object(retained_directory.identity(), expected_directory) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.take(MAX_DIRECTORY_ENTRIES).filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !protocol_cleanup_name(name, response_directory) {
            continue;
        }
        let old = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.is_file())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= RESPONSE_TTL);
        if !old {
            continue;
        }
        let Ok(identity) = crate::path_identity::verify_regular_file(&path) else {
            continue;
        };
        safe_remove(&path, &identity);
    }
    let _ = retained_directory.verify_current();
}

fn protocol_cleanup_name(name: &str, response_directory: bool) -> bool {
    if protocol_request_name(name).is_some() {
        return true;
    }
    let Some(name) = name.strip_prefix('.') else {
        return false;
    };
    let suffixes: &[(&str, bool)] = if response_directory {
        &[(".terminal-snapshot-response-tmp", false)]
    } else {
        &[
            (".terminal-snapshot-request-tmp", true),
            (".terminal-snapshot-processing", false),
            (".terminal-snapshot-cancelled", true),
        ]
    };
    for (suffix, nonce_is_hex) in suffixes {
        let Some(prefix) = name.strip_suffix(suffix) else {
            continue;
        };
        let Some((request_id, nonce)) = prefix.split_once('.') else {
            return false;
        };
        if terminal_snapshot_renderer::validate_uuid(request_id, Some(4)).is_err() {
            return false;
        }
        return if *nonce_is_hex {
            terminal_snapshot_renderer::validate_hex(nonce, 64).is_ok()
        } else {
            terminal_snapshot_renderer::validate_uuid(nonce, Some(4)).is_ok()
        };
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_directory() -> tempfile::TempDir {
        let directory = tempfile::TempDir::new().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        directory
    }

    fn response_temporary_paths(directory: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".terminal-snapshot-response-tmp"))
            })
            .collect()
    }

    fn assert_registry_reclaimed(state: &crate::pty::terminal_snapshot::TerminalSnapshotState) {
        state.sweep_artifacts_for_test(false);
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    fn request() -> HostTerminalSnapshotRequest {
        let issued = chrono::Utc::now();
        let mut request = HostTerminalSnapshotRequest {
            kind: REQUEST_KIND.to_string(),
            version: REQUEST_VERSION,
            request_id: Uuid::new_v4().to_string(),
            token: Uuid::new_v4().to_string(),
            from: "project:wg-1-team/lead".to_string(),
            to: "project:wg-1-team/member".to_string(),
            format: TerminalSnapshotFormat::Json,
            issued_at: canonical_timestamp(issued),
            expires_at: canonical_timestamp(issued + chrono::Duration::seconds(15)),
            nonce: "a".repeat(64),
            confirmation_tag: String::new(),
        };
        request.confirmation_tag = confirmation_tag(&request);
        request
    }

    #[cfg(windows)]
    #[test]
    fn response_parent_replacement_before_retention_leaves_no_secret_residue() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("responses");
        let retired = directory.path().join("retired-responses");
        std::fs::create_dir(&parent).unwrap();
        let parent_for_hook = parent.clone();
        let retired_for_hook = retired.clone();
        state.install_response_publication_hook(Stage::BeforeDirectoryRetention, move |_, _| {
            std::fs::rename(&parent_for_hook, &retired_for_hook).unwrap();
            std::fs::create_dir(&parent_for_hook).unwrap();
        });

        let request_id = Uuid::new_v4();
        let result = publish_response_bytes(&state, &parent, request_id, b"secret-response");

        assert_eq!(result, Err(TerminalSnapshotReasonCode::ResponseUnavailable));
        assert!(
            std::fs::read_dir(&parent).unwrap().next().is_none(),
            "replacement parent was touched"
        );
        assert!(
            std::fs::read_dir(&retired).unwrap().next().is_none(),
            "secret response reached the displaced parent before retention"
        );
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[cfg(windows)]
    #[test]
    fn retained_response_parent_blocks_replacement_at_every_publication_stage() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        for stage in [
            Stage::BeforeTemporaryCreate,
            Stage::BeforeTemporaryCommit,
            Stage::AfterTemporaryCommit,
            Stage::AfterTemporaryWrite,
            Stage::BeforeAtomicRename,
            Stage::AfterAtomicRename,
            Stage::AfterFinalVerification,
            Stage::AfterRegistryCommit,
            Stage::AfterParentSync,
        ] {
            let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
                crate::shutdown::ShutdownSignal::new(),
            );
            let directory = tempfile::TempDir::new().unwrap();
            let parent = directory.path().join("responses");
            let retired = directory.path().join("retired-responses");
            std::fs::create_dir(&parent).unwrap();
            let parent_for_hook = parent.clone();
            let retired_for_hook = retired.clone();
            state.install_response_publication_hook(stage, move |_, _| {
                assert!(
                    std::fs::rename(&parent_for_hook, &retired_for_hook).is_err(),
                    "retained parent was replaceable at {stage:?}"
                );
            });

            let request_id = Uuid::new_v4();
            assert!(
                publish_response_bytes(&state, &parent, request_id, b"secret-response").is_ok()
            );
            let destination = parent.join(format!("{request_id}.json"));
            assert_eq!(std::fs::read(&destination).unwrap(), b"secret-response");
            assert!(!retired.exists());
            assert!(
                std::fs::rename(&parent, &retired).is_err(),
                "registry did not retain the parent after {stage:?}"
            );
            state.sweep_artifacts_for_test(true);
            assert!(!destination.exists());
            assert_eq!(state.test_artifact_counts(), (0, 0, 0));
            std::fs::rename(&parent, &retired).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn response_parent_replacement_before_temporary_create_is_confined_on_unix() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("responses");
        let retired = directory.path().join("retired-responses");
        std::fs::create_dir(&parent).unwrap();
        let parent_for_hook = parent.clone();
        let retired_for_hook = retired.clone();
        state.install_response_publication_hook(Stage::BeforeTemporaryCreate, move |_, _| {
            std::fs::rename(&parent_for_hook, &retired_for_hook).unwrap();
            std::fs::create_dir(&parent_for_hook).unwrap();
        });

        let request_id = Uuid::new_v4();
        let result = publish_response_bytes(&state, &parent, request_id, b"secret-response");

        assert_eq!(result, Err(TerminalSnapshotReasonCode::ResponseUnavailable));
        assert!(std::fs::read_dir(&parent).unwrap().next().is_none());
        assert!(std::fs::read_dir(&retired).unwrap().next().is_none());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn response_parent_replacement_after_write_cleans_retained_object_on_unix() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("responses");
        let retired = directory.path().join("retired-responses");
        std::fs::create_dir(&parent).unwrap();
        let parent_for_hook = parent.clone();
        let retired_for_hook = retired.clone();
        state.install_response_publication_hook(Stage::AfterTemporaryWrite, move |_, _| {
            std::fs::rename(&parent_for_hook, &retired_for_hook).unwrap();
            std::fs::create_dir(&parent_for_hook).unwrap();
            std::fs::write(parent_for_hook.join("replacement-marker"), b"keep").unwrap();
        });

        let request_id = Uuid::new_v4();
        let result = publish_response_bytes(&state, &parent, request_id, b"secret-response");

        assert_eq!(result, Err(TerminalSnapshotReasonCode::ResponseUnavailable));
        assert_eq!(
            std::fs::read(parent.join("replacement-marker")).unwrap(),
            b"keep"
        );
        assert!(std::fs::read_dir(&retired).unwrap().next().is_none());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn tracked_cleanup_uses_original_parent_after_same_spelling_replacement_on_unix() {
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("responses");
        let retired = directory.path().join("retired-responses");
        std::fs::create_dir(&parent).unwrap();
        let response = parent.join("response.json");
        std::fs::write(&response, b"owned response").unwrap();
        let directory_identity = crate::path_identity::verify_directory(&parent).unwrap();
        let identity = crate::path_identity::verify_regular_file(&response).unwrap();
        state
            .reserve_existing_artifact(&parent, &directory_identity, identity.object_id)
            .unwrap()
            .commit_with_ttl(response.clone(), identity, std::time::Duration::ZERO)
            .unwrap();

        std::fs::rename(&parent, &retired).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(&response, b"replacement response").unwrap();
        state.sweep_artifacts_for_test(true);

        assert!(!retired.join("response.json").exists());
        assert_eq!(std::fs::read(response).unwrap(), b"replacement response");
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[test]
    fn confirmation_tag_and_window_are_exact() {
        let request = request();
        assert_eq!(request.confirmation_tag.len(), 64);
        assert!(request.validate(chrono::Utc::now()).is_ok());
        let mut changed = request.clone();
        changed.to.push('x');
        assert!(changed.validate(chrono::Utc::now()).is_err());
    }

    #[test]
    fn request_correlation_rejects_body_ids_that_do_not_match_the_filename() {
        let mut request = request();
        let filename_id = request.request_id.clone();
        request.request_id = Uuid::new_v4().to_string();
        request.confirmation_tag = confirmation_tag(&request);
        assert!(request.validate_correlation(&filename_id).is_err());

        request.request_id = "../audit-secret".to_string();
        request.confirmation_tag = confirmation_tag(&request);
        assert!(request.validate_correlation(&filename_id).is_err());
    }

    #[test]
    fn response_publication_is_canonical_and_never_replaces_a_collision() {
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let directory = tempfile::TempDir::new().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let request_id = Uuid::new_v4();
        let tag = "a".repeat(64);
        assert!(publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &tag,
            TerminalSnapshotReasonCode::InvalidRequest,
        )
        .is_ok());
        let destination = directory.path().join(format!("{request_id}.json"));
        let first = std::fs::read(&destination).unwrap();
        assert!(matches!(
            publish_trusted_failure(
                &state,
                directory.path(),
                request_id,
                &tag,
                TerminalSnapshotReasonCode::Internal,
            ),
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
        assert_eq!(std::fs::read(destination).unwrap(), first);
    }

    #[test]
    fn response_publication_collisions_never_overwrite_or_orphan_owned_temps() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let request_id = Uuid::new_v4();
        let collided_temporary = std::sync::Arc::new(std::sync::Mutex::new(None));
        let collided_temporary_for_hook = std::sync::Arc::clone(&collided_temporary);
        state.install_response_publication_hook(
            Stage::BeforeTemporaryCreate,
            move |temporary, _| {
                std::fs::write(temporary, b"temporary collision").unwrap();
                *collided_temporary_for_hook.lock().unwrap() = Some(temporary.to_path_buf());
            },
        );
        assert!(matches!(
            publish_trusted_failure(
                &state,
                directory.path(),
                request_id,
                &"a".repeat(64),
                TerminalSnapshotReasonCode::InvalidRequest,
            ),
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
        let temporary = collided_temporary.lock().unwrap().clone().unwrap();
        assert_eq!(std::fs::read(&temporary).unwrap(), b"temporary collision");
        assert!(!directory.path().join(format!("{request_id}.json")).exists());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        std::fs::remove_file(temporary).unwrap();
        assert_registry_reclaimed(&state);

        for stage in [Stage::AfterTemporaryCommit, Stage::BeforeAtomicRename] {
            let directory = response_directory();
            let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
                crate::shutdown::ShutdownSignal::new(),
            );
            let request_id = Uuid::new_v4();
            state.install_response_publication_hook(stage, |_, destination| {
                std::fs::write(destination, b"final collision").unwrap();
            });
            assert!(matches!(
                publish_trusted_failure(
                    &state,
                    directory.path(),
                    request_id,
                    &"a".repeat(64),
                    TerminalSnapshotReasonCode::InvalidRequest,
                ),
                Err(TerminalSnapshotReasonCode::ResponseUnavailable)
            ));
            assert_eq!(
                std::fs::read(directory.path().join(format!("{request_id}.json"))).unwrap(),
                b"final collision"
            );
            assert!(response_temporary_paths(directory.path()).is_empty());
            assert_eq!(state.test_artifact_counts(), (0, 0, 0));
            std::fs::remove_file(directory.path().join(format!("{request_id}.json"))).unwrap();
            assert_registry_reclaimed(&state);
        }
    }

    #[test]
    fn temporary_identity_change_before_registry_commit_fails_closed() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let request_id = Uuid::new_v4();
        let displaced = directory.path().join("displaced-empty-temporary");
        let replacement = std::sync::Arc::new(std::sync::Mutex::new(None));
        let replacement_for_hook = std::sync::Arc::clone(&replacement);
        let displaced_for_hook = displaced.clone();
        state.install_response_publication_hook(
            Stage::BeforeTemporaryCommit,
            move |temporary, _| {
                std::fs::rename(temporary, &displaced_for_hook).unwrap();
                std::fs::write(temporary, b"temporary replacement").unwrap();
                *replacement_for_hook.lock().unwrap() = Some(temporary.to_path_buf());
            },
        );

        assert!(matches!(
            publish_trusted_failure(
                &state,
                directory.path(),
                request_id,
                &"a".repeat(64),
                TerminalSnapshotReasonCode::InvalidRequest,
            ),
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
        let replacement = replacement.lock().unwrap().clone().unwrap();
        assert_eq!(
            std::fs::read(&replacement).unwrap(),
            b"temporary replacement"
        );
        assert_eq!(std::fs::read(&displaced).unwrap(), b"");
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        std::fs::remove_file(replacement).unwrap();
        std::fs::remove_file(displaced).unwrap();
        assert_registry_reclaimed(&state);
    }

    #[test]
    fn temporary_identity_change_after_write_never_renames_or_removes_the_replacement() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let request_id = Uuid::new_v4();
        let displaced = directory.path().join("consumed-written-temporary");
        let replacement = std::sync::Arc::new(std::sync::Mutex::new(None));
        let replacement_for_hook = std::sync::Arc::clone(&replacement);
        let displaced_for_hook = displaced.clone();
        state.install_response_publication_hook(Stage::AfterTemporaryWrite, move |temporary, _| {
            std::fs::rename(temporary, &displaced_for_hook).unwrap();
            std::fs::write(temporary, b"post-write temporary replacement").unwrap();
            *replacement_for_hook.lock().unwrap() = Some(temporary.to_path_buf());
        });

        assert!(matches!(
            publish_trusted_failure(
                &state,
                directory.path(),
                request_id,
                &"a".repeat(64),
                TerminalSnapshotReasonCode::InvalidRequest,
            ),
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
        let replacement = replacement.lock().unwrap().clone().unwrap();
        assert_eq!(
            std::fs::read(&replacement).unwrap(),
            b"post-write temporary replacement"
        );
        assert!(std::fs::metadata(&displaced).unwrap().len() > 0);
        assert!(!directory.path().join(format!("{request_id}.json")).exists());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        std::fs::remove_file(replacement).unwrap();
        std::fs::remove_file(displaced).unwrap();
        assert_registry_reclaimed(&state);
    }

    #[test]
    fn every_post_rename_client_consumption_stage_succeeds_and_reuses_capacity() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        for stage in [
            Stage::AfterAtomicRename,
            Stage::AfterFinalVerification,
            Stage::AfterRegistryCommit,
            Stage::AfterParentSync,
        ] {
            let directory = response_directory();
            let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
                crate::shutdown::ShutdownSignal::new(),
            );
            let request_id = Uuid::new_v4();
            state.install_response_publication_hook(stage, |_, destination| {
                let identity = crate::path_identity::verify_regular_file(destination).unwrap();
                assert!(safe_remove(destination, &identity));
            });
            assert!(
                publish_trusted_failure(
                    &state,
                    directory.path(),
                    request_id,
                    &"a".repeat(64),
                    TerminalSnapshotReasonCode::InvalidRequest,
                )
                .is_ok(),
                "client consumption was reclassified at {stage:?}"
            );
            assert!(!directory.path().join(format!("{request_id}.json")).exists());
            assert!(response_temporary_paths(directory.path()).is_empty());
            assert_eq!(state.test_artifact_counts(), (0, 0, 0));
            assert_registry_reclaimed(&state);
            let identity = crate::path_identity::verify_directory(directory.path()).unwrap();
            assert!(state.reserve_artifact(directory.path(), &identity).is_ok());
        }
    }

    #[test]
    fn every_post_rename_mismatch_fails_without_deleting_the_replacement() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        for (index, stage) in [
            Stage::AfterAtomicRename,
            Stage::AfterFinalVerification,
            Stage::AfterRegistryCommit,
            Stage::AfterParentSync,
        ]
        .into_iter()
        .enumerate()
        {
            let directory = response_directory();
            let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
                crate::shutdown::ShutdownSignal::new(),
            );
            let request_id = Uuid::new_v4();
            let displaced = directory.path().join(format!("consumed-{index}.json"));
            let displaced_for_hook = displaced.clone();
            state.install_response_publication_hook(stage, move |_, destination| {
                std::fs::rename(destination, &displaced_for_hook).unwrap();
                std::fs::write(destination, b"mismatched final replacement").unwrap();
            });
            assert!(matches!(
                publish_trusted_failure(
                    &state,
                    directory.path(),
                    request_id,
                    &"a".repeat(64),
                    TerminalSnapshotReasonCode::InvalidRequest,
                ),
                Err(TerminalSnapshotReasonCode::ResponseUnavailable)
            ));
            let destination = directory.path().join(format!("{request_id}.json"));
            assert_eq!(
                std::fs::read(&destination).unwrap(),
                b"mismatched final replacement"
            );
            assert!(std::fs::metadata(&displaced).unwrap().len() > 0);
            assert_eq!(state.test_artifact_counts(), (0, 0, 0));
            std::fs::remove_file(destination).unwrap();
            std::fs::remove_file(displaced).unwrap();
            assert_registry_reclaimed(&state);
        }
    }

    #[test]
    fn response_artifact_reservation_commit_relocation_and_cleanup_are_exact() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let before_create = std::sync::Arc::clone(&state);
        state.install_response_publication_hook(Stage::BeforeTemporaryCreate, move |_, _| {
            assert_eq!(before_create.test_artifact_counts(), (1, 0, 1));
        });
        let after_commit = std::sync::Arc::clone(&state);
        state.install_response_publication_hook(
            Stage::AfterTemporaryCommit,
            move |temporary, _| {
                assert!(temporary.exists());
                assert_eq!(after_commit.test_artifact_counts(), (1, 1, 0));
            },
        );
        let after_relocation = std::sync::Arc::clone(&state);
        state.install_response_publication_hook(
            Stage::AfterRegistryCommit,
            move |temporary, destination| {
                assert!(!temporary.exists());
                assert!(destination.exists());
                assert_eq!(after_relocation.test_artifact_counts(), (1, 1, 0));
            },
        );
        let request_id = Uuid::new_v4();
        assert!(publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        )
        .is_ok());
        assert!(response_temporary_paths(directory.path()).is_empty());
        state.sweep_artifacts_for_test(true);
        assert!(!directory.path().join(format!("{request_id}.json")).exists());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        let identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        assert!(state.reserve_artifact(directory.path(), &identity).is_ok());
    }

    #[test]
    fn file_sync_failure_is_response_unavailable_while_parent_sync_is_best_effort() {
        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationFailure as Failure;

        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let request_id = Uuid::new_v4();
        state.install_response_publication_failure(Failure::FileSync);
        assert!(matches!(
            publish_trusted_failure(
                &state,
                directory.path(),
                request_id,
                &"a".repeat(64),
                TerminalSnapshotReasonCode::InvalidRequest,
            ),
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
        assert!(!directory.path().join(format!("{request_id}.json")).exists());
        assert!(response_temporary_paths(directory.path()).is_empty());
        assert_registry_reclaimed(&state);

        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let request_id = Uuid::new_v4();
        state.install_response_publication_failure(Failure::ParentSync);
        assert!(publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        )
        .is_ok());
        assert!(directory.path().join(format!("{request_id}.json")).exists());
        state.sweep_artifacts_for_test(true);
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[cfg(windows)]
    #[test]
    fn failed_remove_keeps_the_owned_temp_tracked_until_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let blocker = std::sync::Arc::new(std::sync::Mutex::new(None));
        let blocker_for_hook = std::sync::Arc::clone(&blocker);
        state.install_response_publication_hook(Stage::BeforeAtomicRename, move |temporary, _| {
            let mut options = std::fs::OpenOptions::new();
            options
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
            *blocker_for_hook.lock().unwrap() = Some(options.open(temporary).unwrap());
        });
        assert!(matches!(
            publish_trusted_failure(
                &state,
                directory.path(),
                Uuid::new_v4(),
                &"a".repeat(64),
                TerminalSnapshotReasonCode::InvalidRequest,
            ),
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
        assert_eq!(state.test_artifact_counts(), (1, 1, 0));
        assert_eq!(response_temporary_paths(directory.path()).len(), 1);
        drop(blocker.lock().unwrap().take());
        state.sweep_artifacts_for_test(true);
        assert!(response_temporary_paths(directory.path()).is_empty());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[test]
    fn response_consumed_after_atomic_publish_is_not_reclassified_as_failure() {
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let directory = tempfile::TempDir::new().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let request_id = Uuid::new_v4();
        let destination = directory.path().join(format!("{request_id}.json"));
        let destination_for_hook = destination.clone();
        state.install_response_after_publish_hook(move || {
            crate::path_identity::verify_regular_file(&destination_for_hook)
                .expect("published response identity");
            std::fs::remove_file(&destination_for_hook).expect("consume published response");
        });

        assert!(publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        )
        .is_ok());
        assert!(path_is_absent(&destination));
    }

    #[test]
    fn response_consumed_after_registry_commit_releases_registry_capacity() {
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let directory = tempfile::TempDir::new().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let request_id = Uuid::new_v4();
        state.install_response_publication_hook(
            crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterRegistryCommit,
            |_, destination| {
                std::fs::remove_file(destination).expect("consume committed response");
            },
        );

        assert!(publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        )
        .is_ok());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        state.sweep_artifacts_for_test(false);
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[test]
    fn response_replaced_after_registry_commit_fails_without_deleting_replacement() {
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let directory = tempfile::TempDir::new().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let request_id = Uuid::new_v4();
        let displaced = directory.path().join("client-consumed-response");
        state.install_response_publication_hook(
            crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage::AfterRegistryCommit,
            move |_, destination| {
                std::fs::rename(destination, &displaced).expect("consume committed object");
                std::fs::write(destination, b"attacker replacement")
                    .expect("install mismatched final object");
            },
        );

        assert!(matches!(
            publish_trusted_failure(
                &state,
                directory.path(),
                request_id,
                &"a".repeat(64),
                TerminalSnapshotReasonCode::InvalidRequest,
            ),
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
        let destination = directory.path().join(format!("{request_id}.json"));
        assert_eq!(std::fs::read(destination).unwrap(), b"attacker replacement");
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        state.sweep_artifacts_for_test(false);
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[test]
    fn cleanup_names_are_closed_to_protocol_shapes() {
        let request_id = Uuid::new_v4();
        let daemon_id = Uuid::new_v4();
        let nonce = "a".repeat(64);
        assert!(protocol_cleanup_name(&format!("{request_id}.json"), false));
        assert!(protocol_cleanup_name(
            &format!(".{request_id}.{nonce}.terminal-snapshot-request-tmp"),
            false,
        ));
        assert!(protocol_cleanup_name(
            &format!(".{request_id}.{daemon_id}.terminal-snapshot-processing"),
            false,
        ));
        assert!(protocol_cleanup_name(
            &format!(".{request_id}.{daemon_id}.terminal-snapshot-response-tmp"),
            true,
        ));
        assert!(!protocol_cleanup_name("unrelated.json", false));
        assert!(!protocol_cleanup_name(
            &format!(".{request_id}.{daemon_id}.terminal-snapshot-processing"),
            true,
        ));
    }

    #[test]
    fn request_debug_is_structural_and_omits_auth_identity_and_path_canaries() {
        const AUTH_CANARY: &str = "AUTH_1173_H5C8";
        const PATH_CANARY: &str = r"C:\PATH_1173_H5C8\request.json";
        let mut request = request();
        request.kind = AUTH_CANARY.to_string();
        request.request_id = AUTH_CANARY.to_string();
        request.token = AUTH_CANARY.to_string();
        request.from = AUTH_CANARY.to_string();
        request.to = PATH_CANARY.to_string();
        request.issued_at = AUTH_CANARY.to_string();
        request.expires_at = AUTH_CANARY.to_string();
        request.nonce = AUTH_CANARY.to_string();
        request.confirmation_tag = AUTH_CANARY.to_string();

        let diagnostic = format!("{request:?}");
        assert!(!diagnostic.contains(AUTH_CANARY));
        assert!(!diagnostic.contains(PATH_CANARY));
        assert!(diagnostic.contains("version: 1"));
        assert!(diagnostic.contains("format: Json"));
    }
}
