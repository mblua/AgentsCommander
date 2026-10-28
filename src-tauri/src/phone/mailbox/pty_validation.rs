use super::poller::MailboxPoller;
#[cfg(test)]
use super::pty_claim::MailboxTestHooks;
use super::pty_observation::{PtyAuthorityState, PtyTargetState};
use super::settle::{reason_code_from_name, session_has_current_pty_submission_provenance};
use crate::pty::backend::SessionBackendKind;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

impl MailboxPoller {
    pub(crate) async fn validate_and_enqueue_host_pty_input<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        is_app_outbox: bool,
        source_identity: &crate::path_identity::VerifiedPathIdentity,
        store: &crate::api::message_store::MessageStore,
        envelope: &crate::phone::types::PtyInputHostEnvelope,
    ) -> Result<String, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::{
            parse_canonical_pty_timestamp, parse_canonical_uuid_v4, pty_input_confirmation_tag,
            pty_input_host_request_fingerprint, sha256_hex, PtyInputHostFingerprint,
            PtyInputReasonCode as C, PtyInputSourcePlane, PTY_INPUT_FUTURE_SKEW_SECS,
            PTY_INPUT_TTL_SECS, PTY_INPUT_VERSION,
        };
        if is_app_outbox {
            return Err(C::UnsafePath);
        }
        let payload = &envelope.pty_input;
        if envelope.action != "pty-input"
            || !envelope.body.is_empty()
            || envelope.mode != "wake"
            || envelope.get_output
            || !envelope.preferred_agent.is_empty()
            || envelope.priority != "normal"
        {
            return Err(C::MixedPayload);
        }
        if payload.version != PTY_INPUT_VERSION {
            return Err(C::UnsupportedVersion);
        }
        if payload.enter != crate::phone::types::PtyInputEnterMode::AgentSubmit {
            return Err(C::InvalidEnterMode);
        }
        let injection = parse_canonical_uuid_v4(&payload.injection_id).map_err(|_| C::InvalidId)?;
        parse_canonical_uuid_v4(&payload.op_id).map_err(|_| C::InvalidId)?;
        let nonce = parse_canonical_uuid_v4(&payload.nonce).map_err(|_| C::InvalidNonce)?;
        if envelope.id != payload.injection_id
            || payload.op_id != payload.injection_id
            || injection == nonce
            || path.file_stem().and_then(|value| value.to_str())
                != Some(payload.injection_id.as_str())
        {
            return Err(C::InvalidId);
        }
        let issued = parse_canonical_pty_timestamp(&payload.issued_at)?;
        let expires = parse_canonical_pty_timestamp(&payload.expires_at)?;
        if envelope.timestamp != payload.issued_at
            || expires - issued != chrono::Duration::seconds(PTY_INPUT_TTL_SECS)
        {
            return Err(C::InvalidTimestamp);
        }
        let now = chrono::Utc::now();
        if issued > now + chrono::Duration::seconds(PTY_INPUT_FUTURE_SKEW_SECS) {
            return Err(C::InvalidTimestamp);
        }
        if expires <= now {
            return Err(C::Expired);
        }
        crate::pty::inject::validate_pty_input_text(&payload.text).map_err(|error| {
            if error.kind == crate::pty::inject::PtyInputTextErrorKind::TooLarge {
                C::PayloadTooLarge
            } else {
                C::InvalidText
            }
        })?;
        if let Some(agent_id) = payload.agent_id.as_deref() {
            if agent_id == "auto"
                || crate::config::coding_agent_mutations::validate_custom_agent_id(agent_id)
                    .is_err()
            {
                return Err(C::UnsupportedProfile);
            }
        }
        let token = parse_canonical_uuid_v4(&envelope.token).map_err(|_| C::InvalidSessionToken)?;
        let session_manager = app.pty_sessions();
        let session = {
            let manager = session_manager.read().await;
            manager
                .find_unique_live_by_token(token)
                .await
                .map_err(|error| match error {
                    crate::session::manager::UniqueLiveTokenError::NotFound => {
                        C::InvalidSessionToken
                    }
                    crate::session::manager::UniqueLiveTokenError::Ambiguous => {
                        C::AmbiguousSessionToken
                    }
                })?
        };
        if session.backend_kind != SessionBackendKind::LocalProcess {
            return Err(C::SenderBackendNotLocal);
        }
        let session_id = Uuid::parse_str(&session.id).map_err(|_| C::SenderSessionNotLive)?;
        let pty = app.pty_routes();
        if pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .backend_kind(session_id)
            != Some(SessionBackendKind::LocalProcess)
        {
            return Err(C::SenderSessionNotLive);
        }
        let in_memory_project_paths = {
            let settings = app.pty_settings();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut project_paths =
            crate::config::settings::read_pty_input_project_paths_strict_offloaded()
                .await
                .map_err(|_| C::UnsafePath)?
                .unwrap_or(in_memory_project_paths);
        if let Some(replica) = Path::new(&session.working_directory)
            .ancestors()
            .find(|candidate| {
                candidate
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.starts_with("__agent_"))
            })
        {
            if let Some(project) = replica
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
            {
                let project_s = project
                    .to_str()
                    .map(crate::path_utils::normalize_windows_verbatim_path)
                    .ok_or(C::UnsafePath)?;
                if !project_paths.contains(&project_s) {
                    project_paths.push(project_s);
                }
            }
        }
        let route = crate::config::teams::verify_pty_input_route(
            Path::new(&session.working_directory),
            session.is_root_agent,
            &envelope.to,
            &project_paths,
        )
        .map_err(|code| reason_code_from_name(&code))?;
        if envelope.from != route.sender.canonical_fqn {
            return Err(C::SenderIdentityInvalid);
        }
        let expected_outbox = Path::new(&session.working_directory)
            .join(crate::config::agent_local_dir_name())
            .join("outbox");
        let actual_parent = path.parent().ok_or(C::UnsafePath)?;
        let expected_identity =
            crate::path_identity::verify_directory(&expected_outbox).map_err(|_| C::UnsafePath)?;
        let actual_identity =
            crate::path_identity::verify_directory(actual_parent).map_err(|_| C::UnsafePath)?;
        if expected_identity.object_id != actual_identity.object_id {
            return Err(C::UnsafePath);
        }
        let confirmation_tag =
            pty_input_confirmation_tag(&payload.injection_id, &payload.op_id, &payload.nonce);
        let nonce_sha256 = sha256_hex(payload.nonce.as_bytes());
        let fingerprint = pty_input_host_request_fingerprint(&PtyInputHostFingerprint {
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
        let enqueue = store
            .enqueue_pty_input_offloaded(crate::api::message_store::PtyInputEnqueueRequest {
                injection_id: payload.injection_id.clone(),
                sender_fqn: route.sender.canonical_fqn,
                target_fqn: route.target.canonical_fqn,
                op_id: payload.op_id.clone(),
                nonce_sha256,
                request_fingerprint: fingerprint.clone(),
                confirmation_tag: Some(confirmation_tag),
                requested_agent_id: payload.agent_id.clone(),
                payload: payload.text.as_bytes().to_vec(),
                source_plane: PtyInputSourcePlane::HostCli,
                sender_incarnation_fingerprint: route.sender.incarnation_fingerprint,
                sender_identity_fingerprint: route.sender.authority_fingerprint,
                target_identity_fingerprint: route.target.authority_fingerprint,
                authority_session_id: session.id,
                authority_client_id: None,
                authority_client_generation: None,
                issued_at: payload.issued_at.clone(),
                expires_at: payload.expires_at.clone(),
            })
            .await
            .map_err(|error| match error {
                crate::api::message_store::MessageStoreError::IdempotencyConflict => {
                    C::IdempotencyConflict
                }
                crate::api::message_store::MessageStoreError::CapacityExceeded => {
                    C::CapacityExceeded
                }
                _ => C::StoreTransient,
            })?;
        self.replace_host_request_with_marker(
            path,
            source_identity,
            &payload.injection_id,
            &payload.op_id,
        )
        .map_err(|_| C::StoreTransient)?;
        Ok(enqueue.result.injection_id)
    }

    pub(crate) async fn validate_claimed_host_authority<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
    ) -> Option<crate::config::teams::VerifiedPtyInputRoute> {
        let authority_id =
            crate::phone::types::parse_canonical_uuid_v4(&claimed.authority_session_id).ok()?;
        let authority = {
            let manager = app.pty_sessions();
            let guard = manager.read().await;
            guard.get_session(authority_id).await
        }?;
        if matches!(authority.status, SessionStatus::Exited(_))
            || authority.backend_kind != SessionBackendKind::LocalProcess
        {
            return None;
        }
        let in_memory_paths = {
            let settings = app.pty_settings();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut paths =
            match crate::config::settings::read_pty_input_project_paths_strict_offloaded().await {
                Ok(paths) => paths.unwrap_or(in_memory_paths),
                Err(_) => return None,
            };
        if let Ok(sender) = crate::config::teams::verify_pty_input_replica_cwd(Path::new(
            &authority.working_directory,
        )) {
            if let Some(project_path) = sender.ac_root_identity.canonical_path.parent() {
                let project = project_path
                    .to_str()
                    .map(crate::path_utils::normalize_windows_verbatim_path)?;
                if !paths.contains(&project) {
                    paths.push(project);
                }
            }
        }
        let route = crate::config::teams::verify_pty_input_route(
            Path::new(&authority.working_directory),
            authority.is_root_agent,
            &claimed.target_fqn,
            &paths,
        )
        .ok()?;
        if route.sender.canonical_fqn != claimed.sender_fqn
            || route.sender.authority_fingerprint != claimed.sender_identity_fingerprint
            || route.target.authority_fingerprint != claimed.target_identity_fingerprint
        {
            return None;
        }
        let current_cwd =
            crate::path_identity::verify_directory(Path::new(&authority.working_directory)).ok()?;
        let pty = app.pty_routes();
        let manager = pty.lock().unwrap_or_else(|error| error.into_inner());
        if !manager.has_session(authority_id)
            || manager.backend_kind(authority_id) != Some(SessionBackendKind::LocalProcess)
        {
            return None;
        }
        let (saved_cwd, saved_replica, _) = manager.route_identities(authority_id)?;
        if !saved_cwd
            .as_ref()
            .is_some_and(|saved| crate::path_identity::same_object(saved, &current_cwd))
        {
            return None;
        }
        match route.kind {
            crate::config::teams::PtyInputAuthorityKind::Coordinator => {
                if !saved_replica.as_ref().is_some_and(|saved| {
                    crate::path_identity::same_object(saved, &route.sender.replica_identity)
                }) {
                    return None;
                }
            }
            crate::config::teams::PtyInputAuthorityKind::Root => {
                if saved_replica.is_some() {
                    return None;
                }
            }
        }
        drop(manager);
        Some(route)
    }

    pub(crate) async fn validate_selected_pty_target<S: PtyTargetState + ?Sized>(
        &self,
        app: &S,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
        session_id: Uuid,
        expected_backend: SessionBackendKind,
    ) -> Option<SessionInfo> {
        let expires =
            crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at).ok()?;
        if expires <= chrono::Utc::now()
            || app
                .pty_restore()
                .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
            || app
                .pty_purge()
                .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
        {
            return None;
        }
        let session = {
            let manager = app.pty_sessions();
            let guard = manager.read().await;
            guard.get_session(session_id).await
        }?;
        let settings = app.pty_settings().read().await.clone();
        if matches!(session.status, SessionStatus::Exited(_))
            || !session.waiting_for_input
            || session.backend_kind != expected_backend
            || !session_has_current_pty_submission_provenance(
                &SessionInfo::from(&session),
                &settings,
            )
        {
            return None;
        }
        let current_target = crate::config::teams::verify_pty_input_replica_cwd(Path::new(
            &session.working_directory,
        ))
        .ok()?;
        if current_target.canonical_fqn != verified_target.canonical_fqn
            || current_target.authority_fingerprint != verified_target.authority_fingerprint
            || !crate::path_identity::same_object(
                &current_target.replica_identity,
                &verified_target.replica_identity,
            )
        {
            return None;
        }
        let current_cwd =
            crate::path_identity::verify_directory(Path::new(&session.working_directory)).ok()?;
        let route_valid = {
            let manager = app.pty_routes();
            let manager = manager.lock().unwrap_or_else(|error| error.into_inner());
            manager.has_session(session_id)
                && manager.backend_kind(session_id) == Some(expected_backend)
                && manager
                    .route_identities(session_id)
                    .is_some_and(|(cwd, replica, _)| {
                        cwd.as_ref().is_some_and(|saved| {
                            crate::path_identity::same_object(saved, &current_cwd)
                        }) && replica.as_ref().is_some_and(|saved| {
                            crate::path_identity::same_object(
                                saved,
                                &verified_target.replica_identity,
                            )
                        })
                    })
        };
        if !route_valid {
            return None;
        }
        let readiness = app
            .pty_readiness()
            .purge_readiness(&[session_id])
            .into_iter()
            .next()?;
        let activity_required = readiness
            .idle_threshold
            .checked_add(std::time::Duration::from_secs(2))?;
        let activity_ready = readiness
            .activity_age
            .is_some_and(|age| age >= activity_required);
        let resize_ready = match readiness.last_resize_age {
            None => true,
            Some(age) => readiness
                .resize_grace
                .checked_add(std::time::Duration::from_secs(2))
                .is_some_and(|required| age >= required),
        };
        if !activity_ready || !resize_ready || !readiness.watcher_idle {
            return None;
        }
        Some(SessionInfo::from(&session))
    }

    pub(crate) async fn dispatch_pty_input_operation<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        state: &crate::api::message_store::MessageStoreState,
        injection_id: &str,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) {
        let deadline = tokio::time::Instant::now() + super::teardown::OBSERVATION_ALLOWANCE;
        let host = matches!(
            source_plane,
            crate::phone::types::PtyInputSourcePlane::HostCli
        );
        let host_permit = if host {
            match Arc::clone(&self.host_pty_slots).try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => return,
            }
        } else {
            None
        };
        let operation = async move {
            #[cfg(test)]
            if let Some(control) = self
                .test_hooks
                .as_ref()
                .and_then(MailboxTestHooks::replacement_fixture_control)
            {
                if !control.before_source_accept(injection_id).await {
                    return;
                }
            }
            let delivery = self.delivery_poller();
            let app = app.clone();
            let injection_id = injection_id.to_owned();
            let api_client_store = api_client_store.cloned();
            let state = crate::api::message_store::MessageStoreState {
                store: state.store.clone(),
                active_operations: state.active_operations.clone(),
                target_locks: state.target_locks.clone(),
                target_gate: state.target_gate.clone(),
            };
            crate::config::sessions_persistence::run_owned_persist_core(async move {
                let _host_permit = host_permit;
                delivery
                    .dispatch_pty_input_operation_core(
                        &app,
                        &state,
                        &injection_id,
                        source_plane,
                        api_client_store.as_ref(),
                    )
                    .await;
            })
            .await;
        };
        if host {
            let _ = tokio::time::timeout_at(deadline, operation).await;
        } else {
            operation.await;
        }
    }
}
