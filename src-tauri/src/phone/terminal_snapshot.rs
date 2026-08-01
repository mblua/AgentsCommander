use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use terminal_snapshot_renderer::{
    canonical_timestamp, to_ascii_json, TerminalSnapshotFormat, TerminalSnapshotHostResponse,
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
            .field("request_id", &self.request_id)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl HostTerminalSnapshotRequest {
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
                    safe_remove(&path, Some(&identity));
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
        sweep_directory(&request_directory, false);
        sweep_directory(&response_directory, true);
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
                safe_remove(&processing, Some(&claimed));
                continue;
            }
            let app = app.clone();
            let response_directory = response_directory.clone();
            let expected_root = root_identity.clone();
            let snapshot_state = snapshot_state.inner().clone();
            tauri::async_runtime::spawn(async move {
                let _ingress = ingress;
                process_claimed(
                    &app,
                    snapshot_state,
                    processing,
                    response_directory,
                    expected_root,
                    claimed,
                    request_id,
                )
                .await;
            });
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

async fn process_claimed<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    snapshot_state: Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>,
    processing: PathBuf,
    response_directory: PathBuf,
    expected_root: crate::path_identity::VerifiedPathIdentity,
    claimed: crate::path_identity::VerifiedPathIdentity,
    filename_request_id: String,
) {
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
    let (request_id, token, expires_at) = match request.validate(chrono::Utc::now()) {
        Ok(validated) if request.request_id == filename_request_id => validated,
        Ok(_) => {
            record_host_ingress_failure(
                Some(request.request_id.clone()),
                Some(request.format),
                TerminalSnapshotReasonCode::InvalidRequest,
            );
            publish_failure(
                &snapshot_state,
                &response_directory,
                &request,
                TerminalSnapshotReasonCode::InvalidRequest,
            );
            return;
        }
        Err(reason) => {
            record_host_ingress_failure(
                Some(request.request_id.clone()),
                Some(request.format),
                reason,
            );
            publish_failure(&snapshot_state, &response_directory, &request, reason);
            return;
        }
    };
    let publish_error = |reason| {
        record_host_ingress_failure(
            Some(request.request_id.clone()),
            Some(request.format),
            reason,
        );
        publish_failure(&snapshot_state, &response_directory, &request, reason);
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
    let result = snapshot_state
        .execute(
            &context,
            crate::pty::terminal_snapshot::TerminalSnapshotServiceRequest {
                request_id,
                target: request.to.clone(),
                format: request.format,
                source_plane: crate::pty::terminal_snapshot::TerminalSnapshotSourcePlane::HostCli,
                host_authorization_deadline: Some((monotonic_deadline, expires_at)),
            },
            crate::pty::terminal_snapshot::TerminalSnapshotRequesterSelector::Host {
                token,
                expected_root,
                claimed_from: request.from.clone(),
            },
        )
        .await;
    match result {
        Ok(success) => {
            let response = TerminalSnapshotHostResponse::success(
                request.request_id.clone(),
                request.confirmation_tag.clone(),
                canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60)),
                success.result,
            );
            publish_response(
                &snapshot_state,
                &response_directory,
                &request.request_id,
                &response,
            );
        }
        Err(reason) => publish_failure(&snapshot_state, &response_directory, &request, reason),
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

fn publish_failure(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request: &HostTerminalSnapshotRequest,
    reason: TerminalSnapshotReasonCode,
) {
    let response = TerminalSnapshotHostResponse::failure(
        request.request_id.clone(),
        request.confirmation_tag.clone(),
        canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60)),
        reason,
    );
    publish_response(state, response_directory, &request.request_id, &response);
}

fn publish_response(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: &str,
    response: &TerminalSnapshotHostResponse,
) {
    let Ok(directory_identity) = crate::path_identity::verify_directory(response_directory) else {
        return;
    };
    let Ok(reservation) = state.reserve_artifact(response_directory, &directory_identity) else {
        return;
    };
    let bytes = match to_ascii_json(response, MAX_TRANSPORT_BYTES) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    let temporary = response_directory.join(format!(
        ".{}.{}.terminal-snapshot-response-tmp",
        request_id,
        Uuid::new_v4()
    ));
    let destination = response_directory.join(format!("{request_id}.json"));
    if destination.exists() {
        return;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let Ok(mut file) = options.open(&temporary) else {
        return;
    };
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
            return;
        }
    }
    if file
        .write_all(&bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .is_err()
    {
        let _ = std::fs::remove_file(&temporary);
        return;
    }
    let Ok(temporary_identity) =
        crate::path_identity::verify_opened_regular_file(&temporary, &file, false)
    else {
        let _ = std::fs::remove_file(&temporary);
        return;
    };
    if crate::path_identity::publish_new_file_atomic(&temporary, &destination).is_err() {
        safe_remove(&temporary, Some(&temporary_identity));
        return;
    }
    let identity =
        match crate::path_identity::verify_opened_regular_file(&destination, &file, false) {
            Ok(identity) => identity,
            Err(_) => {
                safe_remove(&destination, Some(&temporary_identity));
                return;
            }
        };
    drop(file);
    if reservation
        .commit(destination.clone(), identity.clone())
        .is_err()
    {
        safe_remove(&destination, Some(&identity));
    } else {
        crate::path_identity::sync_parent_best_effort(&destination);
    }
}

fn remove_tracked_processing(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    path: &Path,
    expected: &crate::path_identity::VerifiedPathIdentity,
) {
    if safe_remove(path, Some(expected)) {
        state.untrack_artifact(expected);
    }
}

fn safe_remove(path: &Path, expected: Option<&crate::path_identity::VerifiedPathIdentity>) -> bool {
    if let Some(expected) = expected {
        let Ok(current) = crate::path_identity::verify_regular_file(path) else {
            return !path.exists();
        };
        if !crate::path_identity::same_object(expected, &current) {
            return false;
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

fn sweep_directory(directory: &Path, response_directory: bool) {
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
        safe_remove(&path, Some(&identity));
    }
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
    fn request_debug_omits_secrets_and_targets() {
        let request = request();
        let debug = format!("{request:?}");
        assert!(!debug.contains(&request.token));
        assert!(!debug.contains(&request.nonce));
        assert!(!debug.contains(&request.to));
    }
}
