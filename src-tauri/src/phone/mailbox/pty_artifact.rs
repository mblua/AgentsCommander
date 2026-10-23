use super::poller::MailboxPoller;
use super::pty_claim::write_atomic_pty_metadata;
use super::settle::{reason_code_name, CorrelatedHostPtyMetadata};
use crate::config::settings::SettingsState;
use std::path::Path;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    pub(crate) fn reject_malformed_pty_candidate(&self, path: &Path) {
        use crate::phone::types::{
            PtyInputHostArtifact, PtyInputPublicStatus, PtyInputReason, PtyInputReasonCode,
            PtyInputResult,
        };
        let source_identity = match crate::path_identity::verify_regular_file(path) {
            Ok(identity) => identity,
            Err(_) => return,
        };
        // A malformed document does not independently prove that its filename
        // belongs to the claimed request. Always use a server ID so it cannot
        // replace a correlated terminal artifact for another operation.
        let injection_id = Uuid::new_v4().to_string();
        let mut result = PtyInputResult::new(injection_id.clone(), PtyInputPublicStatus::Rejected);
        result.terminal_at = Some(crate::phone::types::canonical_pty_timestamp(
            chrono::Utc::now(),
        ));
        result.reason = Some(PtyInputReason::from_code(
            PtyInputReasonCode::InvalidEnvelope,
        ));
        let artifact = PtyInputHostArtifact {
            result,
            confirmation_tag: String::new(),
        };
        if let Err(code) = self.write_pty_input_artifact(path, &artifact) {
            log::warn!(
                "[pty-input] malformed candidate artifact failed id={} code={}",
                injection_id,
                code
            );
            return;
        }
        let current_source = match crate::path_identity::verify_regular_file(path) {
            Ok(identity) => identity,
            Err(_) => return,
        };
        if !crate::path_identity::same_object(&source_identity, &current_source) {
            return;
        }
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "[pty-input] malformed candidate cleanup failed id={} code=artifact_unclaimed",
                    injection_id
                );
            }
        }
    }

    async fn correlated_host_pty_metadata<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        envelope: &crate::phone::types::PtyInputHostEnvelope,
    ) -> Option<CorrelatedHostPtyMetadata> {
        use crate::phone::types::{
            parse_canonical_pty_timestamp, parse_canonical_uuid_v4, pty_input_confirmation_tag,
            pty_input_host_request_fingerprint, sha256_hex, PtyInputEnterMode,
            PtyInputHostFingerprint, PTY_INPUT_TTL_SECS, PTY_INPUT_VERSION,
        };

        let payload = &envelope.pty_input;
        let immutable_shape_valid = envelope.action == "pty-input"
            && envelope.body.is_empty()
            && envelope.mode == "wake"
            && !envelope.get_output
            && envelope.preferred_agent.is_empty()
            && envelope.priority == "normal"
            && payload.version == PTY_INPUT_VERSION
            && payload.enter == PtyInputEnterMode::AgentSubmit
            && envelope.id == payload.injection_id
            && payload.op_id == payload.injection_id
            && path.file_stem().and_then(|value| value.to_str())
                == Some(payload.injection_id.as_str());
        if !immutable_shape_valid {
            return None;
        }
        let injection_id = parse_canonical_uuid_v4(&payload.injection_id).ok()?;
        let nonce = parse_canonical_uuid_v4(&payload.nonce).ok()?;
        if injection_id == nonce
            || parse_canonical_uuid_v4(&envelope.token).is_err()
            || crate::pty::inject::validate_pty_input_text(&payload.text).is_err()
        {
            return None;
        }
        let issued = parse_canonical_pty_timestamp(&payload.issued_at).ok()?;
        let expires = parse_canonical_pty_timestamp(&payload.expires_at).ok()?;
        if envelope.timestamp != payload.issued_at
            || expires - issued != chrono::Duration::seconds(PTY_INPUT_TTL_SECS)
        {
            return None;
        }

        let outbox = path.parent()?;
        let root = outbox.parent().and_then(Path::parent)?;
        let owner = self.verified_pty_outbox_owner(path).ok()?;
        let sender_is_root = crate::config::root_agent::verify_live_root_agent_path(root).is_ok();
        let sender_project = if sender_is_root {
            None
        } else {
            let sender = crate::config::teams::verify_pty_input_coordinator_root(root).ok()?;
            sender
                .ac_root_identity
                .canonical_path
                .parent()
                .and_then(|path| {
                    path.to_str()
                        .map(crate::path_utils::normalize_windows_verbatim_path)
                })
        };
        let in_memory_paths = {
            let settings = app.state::<SettingsState>();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut project_paths =
            crate::config::settings::read_pty_input_project_paths_strict_offloaded()
                .await
                .ok()?
                .unwrap_or(in_memory_paths);
        if let Some(project) = sender_project {
            if !project_paths.contains(&project) {
                project_paths.push(project);
            }
        }
        let route = crate::config::teams::verify_pty_input_route(
            root,
            sender_is_root,
            &envelope.to,
            &project_paths,
        )
        .ok()?;
        if owner != route.sender.canonical_fqn || envelope.from != route.sender.canonical_fqn {
            return None;
        }

        let confirmation_tag =
            pty_input_confirmation_tag(&payload.injection_id, &payload.op_id, &payload.nonce);
        let request_fingerprint = pty_input_host_request_fingerprint(&PtyInputHostFingerprint {
            injection_id: &payload.injection_id,
            op_id: &payload.op_id,
            token: &envelope.token,
            sender_fqn: &route.sender.canonical_fqn,
            target_fqn: &route.target.canonical_fqn,
            nonce: &payload.nonce,
            issued_at: &payload.issued_at,
            expires_at: &payload.expires_at,
            text: &payload.text,
            agent_id: payload.agent_id.as_deref(),
            confirmation_tag: &confirmation_tag,
        });
        Some(CorrelatedHostPtyMetadata {
            route,
            confirmation_tag,
            request_fingerprint,
            payload_sha256: sha256_hex(payload.text.as_bytes()),
        })
    }

    async fn reject_unavailable_store_pty_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        source_identity: &crate::path_identity::VerifiedPathIdentity,
        envelope: &crate::phone::types::PtyInputHostEnvelope,
    ) -> bool {
        use crate::phone::types::{
            PtyInputHostArtifact, PtyInputPublicStatus, PtyInputReason, PtyInputReasonCode,
            PtyInputResult, PtyInputSourcePlane,
        };
        let Some(metadata) = self.correlated_host_pty_metadata(app, path, envelope).await else {
            return false;
        };
        let payload = &envelope.pty_input;
        debug_assert_eq!(metadata.request_fingerprint.len(), 64);
        let mut result =
            PtyInputResult::new(payload.injection_id.clone(), PtyInputPublicStatus::Rejected);
        result.op_id = Some(payload.op_id.clone());
        result.sender = Some(metadata.route.sender.canonical_fqn);
        result.target = Some(metadata.route.target.canonical_fqn);
        result.payload_bytes = Some(payload.text.len() as u64);
        result.payload_sha256 = Some(metadata.payload_sha256);
        result.source_plane = Some(PtyInputSourcePlane::HostCli);
        result.issued_at = Some(payload.issued_at.clone());
        result.expires_at = Some(payload.expires_at.clone());
        result.queued_at = Some(payload.issued_at.clone());
        let issued = crate::phone::types::parse_canonical_pty_timestamp(&payload.issued_at)
            .unwrap_or_else(|_| chrono::Utc::now());
        result.terminal_at = Some(crate::phone::types::canonical_pty_timestamp(
            chrono::Utc::now().max(issued),
        ));
        result.reason = Some(PtyInputReason::from_code(PtyInputReasonCode::StoreCorrupt));
        let artifact = PtyInputHostArtifact {
            result,
            confirmation_tag: metadata.confirmation_tag,
        };
        if self.write_pty_input_artifact(path, &artifact).is_err() {
            return true;
        }
        let current_source = crate::path_identity::read_bounded_regular(
            path,
            crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
        )
        .ok()
        .map(|(_, identity)| identity);
        if !current_source.as_ref().is_some_and(|current| {
            crate::path_identity::same_object(source_identity, current)
                && source_identity.content_sha256 == current.content_sha256
        }) {
            return true;
        }
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "[pty-input] unavailable-store source cleanup failed id={} \
                    code=artifact_unclaimed",
                    payload.injection_id
                );
            }
        }
        true
    }

    async fn reject_correlated_host_pty_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        source_identity: &crate::path_identity::VerifiedPathIdentity,
        store: &crate::api::message_store::MessageStore,
        envelope: &crate::phone::types::PtyInputHostEnvelope,
        code: crate::phone::types::PtyInputReasonCode,
    ) -> bool {
        use crate::phone::types::{
            parse_canonical_pty_timestamp, parse_canonical_uuid_v4, pty_input_confirmation_tag,
            pty_input_host_request_fingerprint, sha256_hex, PtyInputEnterMode,
            PtyInputHostArtifact, PtyInputHostFingerprint, PTY_INPUT_TTL_SECS, PTY_INPUT_VERSION,
        };

        let payload = &envelope.pty_input;
        let immutable_shape_valid = envelope.action == "pty-input"
            && envelope.body.is_empty()
            && envelope.mode == "wake"
            && !envelope.get_output
            && envelope.preferred_agent.is_empty()
            && envelope.priority == "normal"
            && payload.version == PTY_INPUT_VERSION
            && payload.enter == PtyInputEnterMode::AgentSubmit
            && envelope.id == payload.injection_id
            && payload.op_id == payload.injection_id
            && path.file_stem().and_then(|value| value.to_str())
                == Some(payload.injection_id.as_str());
        if !immutable_shape_valid {
            return false;
        }
        let Ok(injection_id) = parse_canonical_uuid_v4(&payload.injection_id) else {
            return false;
        };
        let Ok(nonce) = parse_canonical_uuid_v4(&payload.nonce) else {
            return false;
        };
        if injection_id == nonce
            || parse_canonical_uuid_v4(&envelope.token).is_err()
            || crate::pty::inject::validate_pty_input_text(&payload.text).is_err()
        {
            return false;
        }
        let Ok(issued) = parse_canonical_pty_timestamp(&payload.issued_at) else {
            return false;
        };
        let Ok(expires) = parse_canonical_pty_timestamp(&payload.expires_at) else {
            return false;
        };
        if envelope.timestamp != payload.issued_at
            || expires - issued != chrono::Duration::seconds(PTY_INPUT_TTL_SECS)
        {
            return false;
        }

        let Some(outbox) = path.parent() else {
            return false;
        };
        let Some(root) = outbox.parent().and_then(Path::parent) else {
            return false;
        };
        let owner = match self.verified_pty_outbox_owner(path) {
            Ok(owner) => owner,
            Err(_) => return false,
        };
        let sender_is_root = crate::config::root_agent::verify_live_root_agent_path(root).is_ok();
        let sender_project = if sender_is_root {
            None
        } else {
            let sender = match crate::config::teams::verify_pty_input_coordinator_root(root) {
                Ok(sender) => sender,
                Err(_) => return false,
            };
            sender
                .ac_root_identity
                .canonical_path
                .parent()
                .and_then(|path| {
                    path.to_str()
                        .map(crate::path_utils::normalize_windows_verbatim_path)
                })
        };
        let in_memory_paths = {
            let settings = app.state::<SettingsState>();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut project_paths =
            match crate::config::settings::read_pty_input_project_paths_strict_offloaded().await {
                Ok(paths) => paths.unwrap_or(in_memory_paths),
                Err(_) => return false,
            };
        if let Some(project) = sender_project {
            if !project_paths.contains(&project) {
                project_paths.push(project);
            }
        }
        let route = match crate::config::teams::verify_pty_input_route(
            root,
            sender_is_root,
            &envelope.to,
            &project_paths,
        ) {
            Ok(route) => route,
            Err(_) => return false,
        };
        if owner != route.sender.canonical_fqn || envelope.from != route.sender.canonical_fqn {
            return false;
        }

        let confirmation_tag =
            pty_input_confirmation_tag(&payload.injection_id, &payload.op_id, &payload.nonce);
        let request_fingerprint = pty_input_host_request_fingerprint(&PtyInputHostFingerprint {
            injection_id: &payload.injection_id,
            op_id: &payload.op_id,
            token: &envelope.token,
            sender_fqn: &route.sender.canonical_fqn,
            target_fqn: &route.target.canonical_fqn,
            nonce: &payload.nonce,
            issued_at: &payload.issued_at,
            expires_at: &payload.expires_at,
            text: &payload.text,
            agent_id: payload.agent_id.as_deref(),
            confirmation_tag: &confirmation_tag,
        });
        let result = match store
            .record_host_pty_input_rejection_offloaded(
                crate::api::message_store::HostPtyInputRejectionRequest {
                    injection_id: payload.injection_id.clone(),
                    sender_fqn: route.sender.canonical_fqn,
                    target_fqn: route.target.canonical_fqn,
                    op_id: payload.op_id.clone(),
                    nonce_sha256: sha256_hex(payload.nonce.as_bytes()),
                    request_fingerprint,
                    confirmation_tag: confirmation_tag.clone(),
                    sender_incarnation_fingerprint: route.sender.incarnation_fingerprint,
                    payload_sha256: sha256_hex(payload.text.as_bytes()),
                    payload_bytes: payload.text.len() as u64,
                    issued_at: payload.issued_at.clone(),
                    expires_at: payload.expires_at.clone(),
                    reason: code,
                },
                chrono::Utc::now(),
            )
            .await
        {
            Ok(result) => result,
            Err(crate::api::message_store::MessageStoreError::IdempotencyConflict) => {
                return false;
            }
            Err(_) => {
                log::debug!(
                    "[pty-input] correlated ingress persistence deferred id={} \
                    code=store_transient",
                    payload.injection_id
                );
                return true;
            }
        };
        let artifact = PtyInputHostArtifact {
            result,
            confirmation_tag,
        };
        if let Err(artifact_code) = self.write_pty_input_artifact(path, &artifact) {
            log::warn!(
                "[pty-input] correlated ingress artifact failed id={} code={}",
                payload.injection_id,
                artifact_code
            );
            return true;
        }
        let current_source = crate::path_identity::read_bounded_regular(
            path,
            crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
        )
        .ok()
        .map(|(_, identity)| identity);
        if !current_source.as_ref().is_some_and(|current| {
            crate::path_identity::same_object(source_identity, current)
                && source_identity.content_sha256 == current.content_sha256
        }) {
            return true;
        }
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "[pty-input] correlated ingress cleanup failed id={} code=artifact_unclaimed",
                    payload.injection_id
                );
            }
        }
        true
    }

    fn write_pty_input_artifact(
        &self,
        source: &Path,
        artifact: &crate::phone::types::PtyInputHostArtifact,
    ) -> Result<(), &'static str> {
        let directory = match artifact.result.status {
            crate::phone::types::PtyInputPublicStatus::Injected => "delivered",
            crate::phone::types::PtyInputPublicStatus::Rejected => "rejected",
            crate::phone::types::PtyInputPublicStatus::Indeterminate => "indeterminate",
            _ => return Err("invalid_artifact"),
        };
        let outbox = source.parent().ok_or("unsafe_path")?;
        crate::path_identity::verify_directory(outbox).map_err(|_| "unsafe_path")?;
        let artifact_dir = outbox.join(directory);
        if !artifact_dir.exists() {
            std::fs::create_dir(&artifact_dir).map_err(|_| "unsafe_path")?;
        }
        crate::path_identity::verify_directory(&artifact_dir).map_err(|_| "unsafe_path")?;
        let bytes = serde_json::to_vec(artifact).map_err(|_| "invalid_artifact")?;
        if bytes.len() > crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES {
            return Err("invalid_artifact");
        }
        let destination = artifact_dir.join(format!("{}.json", artifact.result.injection_id));
        write_atomic_pty_metadata(&destination, &bytes)?;
        if artifact.result.status == crate::phone::types::PtyInputPublicStatus::Rejected {
            if let Some(reason) = &artifact.result.reason {
                let reason_path =
                    artifact_dir.join(format!("{}.reason.txt", artifact.result.injection_id));
                let fixed = format!("{}: {}", reason_code_name(reason.code), reason.detail);
                write_atomic_pty_metadata(&reason_path, fixed.as_bytes())?;
            }
        }
        Ok(())
    }

    pub(crate) async fn materialize_host_terminal_artifact(
        &self,
        path: &Path,
        store: &crate::api::message_store::MessageStore,
        injection_id: &str,
    ) -> Result<(), &'static str> {
        let (marker, source_identity, owner) =
            self.read_verified_host_marker(path, injection_id)?;
        let result = store
            .query_pty_input_by_injection_offloaded(injection_id.to_string())
            .await
            .map_err(|_| "store_transient")?
            .ok_or("store_corrupt")?;
        if result.source_plane != Some(crate::phone::types::PtyInputSourcePlane::HostCli)
            || result.op_id.as_deref() != Some(marker.op_id.as_str())
            || result.sender.as_deref() != Some(owner.as_str())
        {
            return Err("store_corrupt");
        }
        if !result.terminal {
            return Ok(());
        }
        let confirmation_tag = store
            .host_confirmation_tag_offloaded(injection_id.to_string())
            .await
            .map_err(|_| "store_transient")?
            .ok_or("store_corrupt")?;
        let artifact = crate::phone::types::PtyInputHostArtifact {
            result,
            confirmation_tag,
        };
        self.write_pty_input_artifact(path, &artifact)?;
        let (_, current_source) = crate::path_identity::read_bounded_regular(
            path,
            crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
        )
        .map_err(|_| "unsafe_path")?;
        if !crate::path_identity::same_object(&source_identity, &current_source)
            || source_identity.content_sha256 != current_source.content_sha256
        {
            return Err("unsafe_path");
        }
        // Record the published artifact before deleting the marker. If this
        // update or the later deletion fails, the retained marker drives an
        // idempotent repair on the next poll.
        store
            .mark_host_artifact_offloaded(injection_id.to_string(), chrono::Utc::now())
            .await
            .map_err(|_| "store_transient")?;
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err("artifact_unclaimed");
            }
        }
        Ok(())
    }

    pub(crate) fn replace_host_request_with_marker(
        &self,
        path: &Path,
        source_identity: &crate::path_identity::VerifiedPathIdentity,
        injection_id: &str,
        op_id: &str,
    ) -> Result<(), &'static str> {
        let marker = crate::phone::types::PtyInputQueueMarker {
            kind: "pty-input-marker".to_string(),
            version: crate::phone::types::PTY_INPUT_VERSION,
            injection_id: injection_id.to_string(),
            op_id: op_id.to_string(),
        };
        let bytes = serde_json::to_vec(&marker).map_err(|_| "invalid_envelope")?;
        if path.file_stem().and_then(|value| value.to_str()) != Some(injection_id) {
            return Err("unsafe_path");
        }
        crate::path_identity::replace_regular_file_atomic(
            path,
            source_identity,
            &bytes,
            crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
        )
        .map(|_| ())
        .map_err(|code| {
            if code == "unsafe_path" {
                "unsafe_path"
            } else {
                "store_transient"
            }
        })
    }

    fn read_verified_host_marker(
        &self,
        path: &Path,
        expected_injection_id: &str,
    ) -> Result<
        (
            crate::phone::types::PtyInputQueueMarker,
            crate::path_identity::VerifiedPathIdentity,
            String,
        ),
        &'static str,
    > {
        let (bytes, identity) = crate::path_identity::read_bounded_regular(
            path,
            crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
        )
        .map_err(|_| "unsafe_path")?;
        let value = crate::path_identity::parse_json_no_duplicates(&bytes)
            .map_err(|_| "invalid_artifact")?;
        let marker: crate::phone::types::PtyInputQueueMarker =
            serde_json::from_value(value).map_err(|_| "invalid_artifact")?;
        if marker.kind != "pty-input-marker"
            || marker.version != crate::phone::types::PTY_INPUT_VERSION
            || marker.injection_id != expected_injection_id
            || marker.op_id != marker.injection_id
            || path.file_stem().and_then(|value| value.to_str())
                != Some(marker.injection_id.as_str())
        {
            return Err("invalid_artifact");
        }
        let owner = self
            .verified_pty_outbox_owner(path)
            .map_err(|_| "unsafe_path")?;
        Ok((marker, identity, owner))
    }

    pub(crate) fn verified_pty_outbox_owner(
        &self,
        path: &Path,
    ) -> Result<String, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        let outbox = path.parent().ok_or(C::UnsafePath)?;
        if outbox.file_name().and_then(|value| value.to_str()) != Some("outbox") {
            return Err(C::UnsafePath);
        }
        let local = outbox.parent().ok_or(C::UnsafePath)?;
        let local_dir_name = crate::config::agent_local_dir_name();
        if local.file_name().and_then(|value| value.to_str()) != Some(local_dir_name.as_str()) {
            return Err(C::UnsafePath);
        }
        let root = local.parent().ok_or(C::UnsafePath)?;
        let actual_outbox =
            crate::path_identity::verify_directory(outbox).map_err(|_| C::UnsafePath)?;
        let expected_outbox = crate::path_identity::verify_directory(
            &root
                .join(crate::config::agent_local_dir_name())
                .join("outbox"),
        )
        .map_err(|_| C::UnsafePath)?;
        if !crate::path_identity::same_object(&actual_outbox, &expected_outbox) {
            return Err(C::UnsafePath);
        }
        if let Ok(root_identity) = crate::config::root_agent::verify_live_root_agent_path(root) {
            let actual_root =
                crate::path_identity::verify_directory(root).map_err(|_| C::UnsafePath)?;
            if !crate::path_identity::same_object(&root_identity, &actual_root) {
                return Err(C::UnsafePath);
            }
            return Ok(crate::config::root_agent::ROOT_AGENT_SENDER.to_string());
        }
        crate::config::teams::verify_pty_input_replica_cwd(root)
            .map(|identity| identity.canonical_fqn)
            .map_err(|_| C::UnsafePath)
    }

    pub(crate) async fn process_pty_input_file<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        is_app_outbox: bool,
        bytes: &[u8],
        source_identity: &crate::path_identity::VerifiedPathIdentity,
    ) {
        if bytes.starts_with(&[0xef, 0xbb, 0xbf]) || std::str::from_utf8(bytes).is_err() {
            self.reject_malformed_pty_candidate(path);
            return;
        }
        let value = match crate::path_identity::parse_json_no_duplicates(bytes) {
            Ok(value) => value,
            Err(_) => {
                self.reject_malformed_pty_candidate(path);
                return;
            }
        };
        let state = app.try_state::<crate::api::message_store::MessageStoreState>();

        if value.get("kind").and_then(serde_json::Value::as_str) == Some("pty-input-marker") {
            let Some(state) = state else {
                log::debug!("[pty-input] marker deferred code=store_transient");
                return;
            };
            let store = match &state.store {
                Ok(store) => Arc::clone(store),
                Err(_) => {
                    log::debug!("[pty-input] marker deferred code=store_transient");
                    return;
                }
            };
            let marker: crate::phone::types::PtyInputQueueMarker =
                match serde_json::from_value(value) {
                    Ok(marker) => marker,
                    Err(_) => {
                        self.reject_malformed_pty_candidate(path);
                        return;
                    }
                };
            if marker.version != crate::phone::types::PTY_INPUT_VERSION
                || crate::phone::types::parse_canonical_uuid_v4(&marker.injection_id).is_err()
                || marker.injection_id != marker.op_id
                || path.file_stem().and_then(|value| value.to_str())
                    != Some(marker.injection_id.as_str())
                || is_app_outbox
            {
                self.reject_malformed_pty_candidate(path);
                return;
            }
            let (current_marker, _marker_identity, owner) =
                match self.read_verified_host_marker(path, &marker.injection_id) {
                    Ok(marker) => marker,
                    Err(_) => {
                        self.reject_malformed_pty_candidate(path);
                        return;
                    }
                };
            if current_marker != marker {
                self.reject_malformed_pty_candidate(path);
                return;
            }
            let row = match store
                .query_pty_input_by_injection_offloaded(marker.injection_id.clone())
                .await
            {
                Ok(Some(row)) => row,
                Ok(None) => {
                    self.reject_malformed_pty_candidate(path);
                    return;
                }
                Err(_) => {
                    log::debug!(
                        "[pty-input] marker status deferred id={} code=store_transient",
                        marker.injection_id
                    );
                    return;
                }
            };
            if row.source_plane != Some(crate::phone::types::PtyInputSourcePlane::HostCli)
                || row.op_id.as_deref() != Some(marker.op_id.as_str())
                || row.sender.as_deref() != Some(owner.as_str())
            {
                self.reject_malformed_pty_candidate(path);
                return;
            }
            if row.terminal {
                if let Err(code) = self
                    .materialize_host_terminal_artifact(path, &store, &marker.injection_id)
                    .await
                {
                    log::warn!(
                        "[pty-input] artifact repair failed id={} code={}",
                        marker.injection_id,
                        code
                    );
                }
                return;
            }
            self.dispatch_pty_input_operation(
                app,
                &state,
                &marker.injection_id,
                crate::phone::types::PtyInputSourcePlane::HostCli,
                None,
            )
            .await;
            if let Err(code) = self
                .materialize_host_terminal_artifact(path, &store, &marker.injection_id)
                .await
            {
                if code != "store_corrupt" {
                    log::debug!(
                        "[pty-input] marker remains id={} code={}",
                        marker.injection_id,
                        code
                    );
                }
            }
            return;
        }

        let envelope: crate::phone::types::PtyInputHostEnvelope =
            match serde_json::from_value(value) {
                Ok(envelope) => envelope,
                Err(_) => {
                    self.reject_malformed_pty_candidate(path);
                    return;
                }
            };
        let Some(state) = state else {
            if !self
                .reject_unavailable_store_pty_candidate(app, path, source_identity, &envelope)
                .await
            {
                self.reject_malformed_pty_candidate(path);
            }
            return;
        };
        let store = match &state.store {
            Ok(store) => Arc::clone(store),
            Err(_) => {
                if !self
                    .reject_unavailable_store_pty_candidate(app, path, source_identity, &envelope)
                    .await
                {
                    self.reject_malformed_pty_candidate(path);
                }
                return;
            }
        };
        let validation = self
            .validate_and_enqueue_host_pty_input(
                app,
                path,
                is_app_outbox,
                source_identity,
                &store,
                &envelope,
            )
            .await;
        let injection_id = match validation {
            Ok(injection_id) => injection_id,
            Err(code) => {
                if code == crate::phone::types::PtyInputReasonCode::StoreTransient {
                    log::debug!(
                        "[pty-input] host ingress deferred code={}",
                        reason_code_name(code)
                    );
                    return;
                }
                log::warn!(
                    "[pty-input] host ingress rejected code={}",
                    reason_code_name(code)
                );
                crate::api::audit::record_pty_input(&crate::api::audit::PtyInputAuditMetadata {
                    event: "ingress_rejection".to_string(),
                    injection_id: None,
                    op_id: None,
                    sender_fqn: None,
                    target_fqn: None,
                    payload_bytes: None,
                    payload_sha256: None,
                    source_plane: Some("host_cli".to_string()),
                    selected_session_id: None,
                    selected_backend: None,
                    status: "rejected".to_string(),
                    reason_code: Some(reason_code_name(code).to_string()),
                    timestamp: crate::phone::types::canonical_pty_timestamp(chrono::Utc::now()),
                });
                if !self
                    .reject_correlated_host_pty_candidate(
                        app,
                        path,
                        source_identity,
                        &store,
                        &envelope,
                        code,
                    )
                    .await
                {
                    self.reject_malformed_pty_candidate(path);
                }
                return;
            }
        };
        self.dispatch_pty_input_operation(
            app,
            &state,
            &injection_id,
            crate::phone::types::PtyInputSourcePlane::HostCli,
            None,
        )
        .await;
        if let Err(code) = self
            .materialize_host_terminal_artifact(path, &store, &injection_id)
            .await
        {
            log::debug!(
                "[pty-input] terminal artifact pending id={} code={}",
                injection_id,
                code
            );
        }
    }
}
