use super::root_key;
use super::shutdown_work::ContainerShutdownProducer;
use super::start_request::build_start_request;
use super::{
    ContainerCredentialBinding, ContainerSessionState, ContainerTransportBackend, PendingSession,
    RemovedSessionResources, CONTAINER_DIAGNOSTIC_LOG_TAIL_LINES,
};
use crate::errors::AppError;
use crate::pty::backend::BackendSpawnSpec;
use crate::pty::backend::PtyBackend;
use crate::pty::container_credentials::CopyOutcome;
use crate::pty::container_runtime::{
    ContainerDiagnostics, ContainerRuntime, ContainerRuntimeHandle, DEFAULT_CONTAINER_WORKDIR,
};
use crate::pty::output::PtyOutputTarget;
use crate::resource_monitor::ResourceLogicalAgentSlot;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Notify};
use uuid::Uuid;

struct RuntimeStartContext<'a> {
    id: Uuid,
    runtime: Arc<dyn ContainerRuntime>,
    request: crate::pty::container_runtime::ContainerStartRequest,
    token: crate::pty::container_tokens::ContainerApiToken,
    launch_witness: &'a crate::pty::backend::LaunchWitness,
    attach_notify: Arc<Notify>,
}

pub(super) struct ContainerSpawnCancellationGuard<'a> {
    pub(super) backend: &'a ContainerTransportBackend,
    pub(super) session_id: Uuid,
    pub(super) canceled: Arc<AtomicBool>,
    pub(super) late_handle: Arc<Mutex<Option<ContainerRuntimeHandle>>>,
    pub(super) shutdown_producer: Option<ContainerShutdownProducer>,
    pub(super) armed: bool,
}

impl ContainerSpawnCancellationGuard<'_> {
    pub(super) fn disarm(&mut self) {
        self.armed = false;
        self.shutdown_producer.take();
    }

    pub(super) fn cancel_without_unwind(&mut self) {
        self.canceled.store(true, Ordering::Release);
        if let Some(handle) = self
            .late_handle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            if let Some(runtime) = self.backend.runtime.clone() {
                if let Some(producer) = self.shutdown_producer.as_ref() {
                    self.backend.spawn_shutdown_owned_runtime_stop(
                        producer,
                        runtime,
                        handle,
                        "canceled-late-runtime",
                    );
                } else {
                    let resources = RemovedSessionResources {
                        session_id: handle.session_id,
                        runtime_handle: Some(handle),
                        api_client_id: None,
                        logical_resource_slot: None,
                        container_credential_path: None,
                    };
                    let entry = self.backend.cleanup_ownership.retain(
                        None,
                        Some(runtime),
                        resources,
                        "canceled-late-runtime",
                    );
                    self.backend.schedule_retained_cleanup(entry, None);
                }
            }
        }
        if let Some(resources) = self.backend.remove_session_state(self.session_id) {
            self.backend.cleanup_removed_resources_async_owned(
                resources,
                "spawn-canceled",
                self.shutdown_producer.as_ref(),
            );
        }
    }
}

impl Drop for ContainerSpawnCancellationGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if catch_unwind(AssertUnwindSafe(|| self.cancel_without_unwind())).is_err() {
            log::error!(
                "[container-transport] spawn cancellation cleanup panicked session={}",
                self.session_id
            );
        }
    }
}

impl ContainerTransportBackend {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn create_pending_session(
        &self,
        id: Uuid,
        cwd: &str,
        rows: u16,
        cols: u16,
        idle_tuning: crate::session::profile::IdleTuning,
        output_target: PtyOutputTarget,
        logical_resource_slot: Option<ResourceLogicalAgentSlot>,
        attach_notify: Option<Arc<Notify>>,
        container_credential_path: Option<PathBuf>,
    ) -> Result<String, AppError> {
        let ticket = format!("acst-{}-{}", Uuid::new_v4(), Uuid::new_v4());
        let ticket_hash = crate::api::auth::hash_token(&ticket);
        let pending = PendingSession {
            root_key: root_key(cwd),
            ticket_hash,
            ticket_expires_at: Instant::now() + self.tuning.ticket_ttl,
            output_target,
            idle_tuning,
            rows,
            cols,
            runtime_handle: None,
            api_client_id: None,
            credential_binding: None,
            logical_resource_slot,
            attach_notify,
            container_credential_path,
        };

        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(&id) {
            return Err(AppError::PtyError(format!(
                "container transport session {} already exists",
                id
            )));
        }
        sessions.insert(id, ContainerSessionState::Pending(pending));

        #[cfg(test)]
        self.issued_tickets_for_test
            .lock()
            .unwrap()
            .insert(id, ticket.clone());

        log::info!(
            "[container-transport] issued one-time attach ticket for session {}",
            id
        );
        Ok(ticket)
    }

    pub(super) fn install_credential_binding(
        &self,
        session_id: Uuid,
        binding: ContainerCredentialBinding,
    ) -> Result<(), AppError> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let state = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        let client_id = binding.client_id.clone();
        match state {
            ContainerSessionState::Pending(pending) => {
                pending.api_client_id = Some(client_id);
                pending.credential_binding = Some(binding);
            }
            ContainerSessionState::Attaching(attaching) => {
                attaching.api_client_id = Some(client_id);
                attaching.credential_binding = Some(binding);
            }
            ContainerSessionState::Active(active) => {
                active.api_client_id = Some(client_id);
                active.credential_binding = Some(binding);
            }
        }
        Ok(())
    }

    pub(super) fn install_runtime_handle(
        &self,
        session_id: Uuid,
        handle: ContainerRuntimeHandle,
    ) -> Result<(), AppError> {
        let mut sessions = self.sessions.lock().unwrap();
        let state = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        match state {
            ContainerSessionState::Pending(pending) => pending.runtime_handle = Some(handle),
            ContainerSessionState::Attaching(attaching) => attaching.runtime_handle = Some(handle),
            ContainerSessionState::Active(active) => active.runtime_handle = Some(handle),
        }
        Ok(())
    }

    pub(super) async fn spawn_runtime_backed(
        &self,
        spec: BackendSpawnSpec,
    ) -> Result<(), AppError> {
        let runtime = self
            .runtime
            .clone()
            .ok_or_else(|| AppError::Other("container runtime is not configured".to_string()))?;
        let token_manager = self.token_manager.clone().ok_or_else(|| {
            AppError::Other("container API token manager is not configured".to_string())
        })?;
        let BackendSpawnSpec {
            id,
            agent_id: _,
            coding_agent: _,
            cmd,
            args,
            resolved_agent_host_shell: _,
            cwd,
            selected_cwd,
            cols,
            rows,
            container_image,
            configured_env,
            env_remove_keys,
            env_unset,
            extra_env: _,
            idle_tuning,
            output_target,
            resource_registration: _,
            logical_resource_slot,
            container_credential,
            container_repo_mounts,
            launch_witness,
        } = spec;

        let shutdown_producer = self.shutdown_work.register_producer().ok_or_else(|| {
            AppError::Other("container runtime start rejected during shutdown".to_string())
        })?;

        let attach_notify = Arc::new(Notify::new());
        let ticket = self.create_pending_session(
            id,
            &cwd,
            rows,
            cols,
            idle_tuning,
            output_target,
            logical_resource_slot,
            Some(attach_notify.clone()),
            container_credential.as_ref().map(|p| p.dest.clone()),
        )?;
        let canceled = Arc::new(AtomicBool::new(false));
        let late_handle = Arc::new(Mutex::new(None));
        let mut cancellation_guard = ContainerSpawnCancellationGuard {
            backend: self,
            session_id: id,
            canceled: Arc::clone(&canceled),
            late_handle: Arc::clone(&late_handle),
            shutdown_producer: Some(shutdown_producer),
            armed: true,
        };

        if let Some(plan) = container_credential.as_ref() {
            match crate::pty::container_credentials::copy_in(plan) {
                // #930 - a copied token is only USED if the agent skips its
                // interactive first-run wizard: Claude gates that on
                // .claude.json flags, not on the credential file. Same gate as
                // the copy, same best-effort contract.
                Ok(CopyOutcome::Copied) => {
                    crate::pty::container_credentials::ensure_first_run_state(
                        plan,
                        DEFAULT_CONTAINER_WORKDIR,
                    )
                }
                // grinch Finding 1 - an F2 skip is NOT a copy: the dest dir or
                // leaf is a symlink/junction, so no token was written. Stamping
                // here would suppress the login wizard on a container with no
                // credential. With no token, that wizard is the correct UX.
                Ok(CopyOutcome::SkippedReparse) => log::warn!(
                    "[container-cred] copy-in skipped for session {}; not stamping first-run state",
                    id
                ),
                // Best-effort, mirror config-seed: never abort the spawn.
                Err(e) => log::warn!("[container-cred] copy-in failed for session {}: {}", id, e),
            }
            // F1 - if a concurrent teardown removed the session while we were
            // copying, its remove_copied ran before the file existed (no-op). The
            // shared std::Mutex serializes this check against teardown, so all
            // interleavings converge on a deleted file. Extracted into a method
            // so the race branch is unit-testable (see tests).
            self.remove_credential_if_orphaned(id, plan);
        }

        let token = match token_manager.mint_for_session(id, &cwd) {
            Ok(token) => token,
            Err(err) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "mint-failure",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(err);
            }
        };
        let root_identity = match crate::path_identity::verify_directory(Path::new(&cwd)) {
            Ok(identity) => identity,
            Err(_) => {
                token_manager.revoke(&token.client_id);
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "install-binding-failure",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(AppError::Other(
                    "container credential binding root is unsafe".to_string(),
                ));
            }
        };
        let binding = ContainerCredentialBinding {
            client_id: token.client_id.clone(),
            credential_generation: token.credential_generation.clone(),
            bound_session_id: token.bound_session_id.clone(),
            bound_root_object_id: root_identity.object_id,
            credential_token_hash: token.token_hash.clone(),
        };
        if let Err(err) = self.install_credential_binding(id, binding) {
            token_manager.revoke(&token.client_id);
            if let Some(resources) = self.remove_session_state(id) {
                self.cleanup_removed_resources_offloaded_owned(
                    resources,
                    "install-binding-failure",
                    cancellation_guard.shutdown_producer.as_ref(),
                )
                .await;
            }
            return Err(err);
        }

        let request = match build_start_request(
            id,
            &cmd,
            args,
            &cwd,
            rows,
            cols,
            configured_env,
            env_remove_keys,
            env_unset,
            container_image,
            selected_cwd.as_deref(),
            ticket,
            &token,
            container_repo_mounts,
            #[cfg(test)]
            self.runtime_settings_override.as_ref(),
        ) {
            Ok(request) => request,
            Err(err) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "build-request-failure",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(err);
            }
        };
        self.start_runtime_and_attach(
            RuntimeStartContext {
                id,
                runtime,
                request,
                token,
                launch_witness: &launch_witness,
                attach_notify,
            },
            &mut cancellation_guard,
        )
        .await
    }

    async fn start_runtime_and_attach(
        &self,
        context: RuntimeStartContext<'_>,
        cancellation_guard: &mut ContainerSpawnCancellationGuard<'_>,
    ) -> Result<(), AppError> {
        let RuntimeStartContext {
            id,
            runtime,
            request,
            token,
            launch_witness,
            attach_notify,
        } = context;
        let canceled = &cancellation_guard.canceled;
        let late_handle = &cancellation_guard.late_handle;
        let Some(start_producer) = cancellation_guard.shutdown_producer.as_ref() else {
            return Err(AppError::Other(
                "container runtime start lost shutdown ownership".to_string(),
            ));
        };
        let start_runtime = runtime.clone();
        let canceled_for_start = Arc::clone(canceled);
        let late_handle_for_start = Arc::clone(late_handle);
        let canceled_cleanup_ownership = Arc::clone(&self.cleanup_ownership);
        let canceled_cleanup_epoch = self.cleanup_attempt_epoch.fetch_add(1, Ordering::Relaxed);
        let (start_result_sender, start_result_receiver) = oneshot::channel();
        // #2413 - the runtime may start from here on; every later `Err` is fenced.
        launch_witness.mark();
        start_producer.spawn_owned(Some(id), "runtime-start", move |control| {
            let result = (|| {
                let handle = start_runtime.start(request, control)?;
                let canceled_handle = {
                    let mut slot = late_handle_for_start
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    *slot = Some(handle);
                    if canceled_for_start.load(Ordering::Acquire) || control.shutdown_requested() {
                        slot.take()
                    } else {
                        None
                    }
                };
                if let Some(handle) = canceled_handle {
                    let resources = RemovedSessionResources {
                        session_id: handle.session_id,
                        runtime_handle: Some(handle),
                        api_client_id: None,
                        logical_resource_slot: None,
                        container_credential_path: None,
                    };
                    let entry = canceled_cleanup_ownership.retain(
                        None,
                        Some(Arc::clone(&start_runtime)),
                        resources,
                        "canceled-late-runtime",
                    );
                    canceled_cleanup_ownership.attempt(entry, canceled_cleanup_epoch, control);
                }
                Ok::<(), AppError>(())
            })();
            if start_result_sender.send(result).is_err() {
                log::debug!(
                    "[container-transport] runtime start completed after caller cancellation"
                );
            }
        });
        let start_result = start_result_receiver.await;
        let handle = match start_result {
            Ok(Ok(())) => late_handle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .ok_or_else(|| {
                    AppError::Other("container runtime start was canceled".to_string())
                })?,
            Ok(Err(err)) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "runtime-start-failure",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(err);
            }
            Err(err) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "runtime-start-panic",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(AppError::Other(format!(
                    "container runtime start task failed: {err}"
                )));
            }
        };
        if let Err(err) = self.install_runtime_handle(id, handle.clone()) {
            let resources = RemovedSessionResources {
                session_id: id,
                runtime_handle: Some(handle),
                api_client_id: Some(token.client_id),
                logical_resource_slot: None,
                // F3 - the session was already removed from the map before this
                // branch, so its credential deletion was initiated on that path;
                // remove_copied idempotency makes a missed second delete a no-op.
                container_credential_path: None,
            };
            self.cleanup_removed_resources_offloaded_owned(
                resources,
                "install-runtime-failure",
                cancellation_guard.shutdown_producer.as_ref(),
            )
            .await;
            return Err(err);
        }

        if self.has_session(id) {
            cancellation_guard.disarm();
            return Ok(());
        }
        match tokio::time::timeout(self.tuning.handshake_timeout, attach_notify.notified()).await {
            Ok(_) if self.has_session(id) => {
                cancellation_guard.disarm();
                Ok(())
            }
            _ => {
                if let Some(resources) = self.remove_session_state(id) {
                    let diagnostics = if let (Some(runtime), Some(handle)) =
                        (self.runtime.clone(), resources.runtime_handle.clone())
                    {
                        Some(Self::collect_container_diagnostics(runtime, handle).await)
                    } else {
                        None
                    };
                    if let Some(diagnostics) = diagnostics.as_ref() {
                        log::error!(
                            "[container-transport] attach timeout diagnostics for session {}:\n{}",
                            id,
                            diagnostics.log_summary()
                        );
                    }
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "handshake-timeout",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                    return Err(Self::handshake_timeout_error(
                        self.tuning.handshake_timeout,
                        diagnostics.as_ref(),
                    ));
                }
                Err(Self::handshake_timeout_error(
                    self.tuning.handshake_timeout,
                    None,
                ))
            }
        }
    }

    pub(super) async fn collect_container_diagnostics(
        runtime: Arc<dyn ContainerRuntime>,
        handle: ContainerRuntimeHandle,
    ) -> ContainerDiagnostics {
        let fallback_handle = handle.clone();
        let task = tokio::task::spawn_blocking(move || {
            runtime.diagnostics(&handle, CONTAINER_DIAGNOSTIC_LOG_TAIL_LINES)
        });
        match task.await {
            Ok(diagnostics) => diagnostics,
            Err(err) => ContainerDiagnostics::unavailable(
                &fallback_handle,
                format!("container diagnostics task failed: {err}"),
            ),
        }
    }

    pub(super) fn handshake_timeout_error(
        timeout: Duration,
        diagnostics: Option<&ContainerDiagnostics>,
    ) -> AppError {
        let mut message = format!("container bridge did not attach within {timeout:?}");
        if let Some(diagnostics) = diagnostics {
            message.push_str("; ");
            message.push_str(&diagnostics.ui_summary());
        }
        AppError::PtyError(message)
    }
}
