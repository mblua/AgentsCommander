use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::session::SessionInfo;

use super::admission::{
    BlockingSeedAwaitGuard, BlockingSeedTaskError, BlockingSeedWorkerResult,
    CriticalAdmissionGuard, CriticalAdmissionKey, CriticalAdmissionKind, WorkerCompletionGuard,
};
use super::envelope::CoordinatorEnvelope;
use super::payload::SelectionCoordinatorError;
use super::replacement::{
    LifecycleEffectGuard, LifecycleEffectSlot, OwnedEffectState, OwnedLifecycleEntry,
    OwnedReplacementDescriptor, ReplacementOwner, ReplacementStage,
};
use super::response_close::{
    response_registration_route, response_registration_row, ResponseCloseRegistration,
};
use super::transaction::SelectionTransaction;
use super::turn::IN_SELECTION_WORKER;
use super::worker::worker_loop;
use super::{
    ConditionalResponseClose, CoordinatorInner, CoordinatorJob, LifecycleAdmission,
    LifecycleExecutor, RestartJobRequest, RootJobRequest, SelectionCoordinator,
};

impl SelectionCoordinator {
    pub(crate) fn start_with_executor<R: Runtime>(
        &self,
        app: AppHandle<R>,
        executor: Option<Arc<dyn LifecycleExecutor<SelectionTransaction<R>>>>,
    ) -> Result<(), SelectionCoordinatorError> {
        let Some(executor) = executor else {
            log::error!("[selection] lifecycle executor binding missing at coordinator start");
            return Err(SelectionCoordinatorError::Unavailable);
        };
        if let Some(pty) = app.try_state::<Arc<Mutex<PtyManager>>>() {
            let container_backend = pty
                .inner()
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable)?
                .container_backend();
            let mut cleanup_pty = self
                .inner
                .cleanup_pty
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable)?;
            *cleanup_pty = Some(Arc::downgrade(pty.inner()));
            let mut cleanup_container_backend = self
                .inner
                .cleanup_container_backend
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable)?;
            *cleanup_container_backend = Some(Arc::downgrade(&container_backend));
        }
        let receiver = self
            .inner
            .receiver
            .lock()
            .map_err(|_| SelectionCoordinatorError::Unavailable)?
            .take()
            .ok_or(SelectionCoordinatorError::Unavailable)?;
        let inner = Arc::clone(&self.inner);
        let handle = tauri::async_runtime::spawn(IN_SELECTION_WORKER.scope((), async move {
            worker_loop(app, inner, receiver, executor).await;
        }));
        let mut worker = self
            .inner
            .worker
            .lock()
            .map_err(|_| SelectionCoordinatorError::Unavailable)?;
        *worker = Some(handle);
        Ok(())
    }

    pub(crate) async fn run_blocking_seed_work<F, T>(
        &self,
        work: F,
    ) -> Result<T, BlockingSeedTaskError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let tracker = Arc::clone(&self.inner.blocking_seed_work);
        let id = tracker.register(&self.inner.phase)?;
        let worker_tracker = Arc::clone(&tracker);
        let handle = tokio::task::spawn_blocking(move || {
            if !worker_tracker.begin(id) {
                return BlockingSeedWorkerResult::CanceledBeforeStart;
            }
            let _completion = WorkerCompletionGuard {
                tracker: worker_tracker,
                id,
            };
            BlockingSeedWorkerResult::Completed(work())
        });
        tracker.attach_abort(id, handle.abort_handle());
        let mut waiter = BlockingSeedAwaitGuard {
            tracker,
            id,
            armed: true,
        };
        let joined = handle.await;
        waiter.disarm();
        match joined {
            Ok(BlockingSeedWorkerResult::Completed(result)) => Ok(result),
            Ok(BlockingSeedWorkerResult::CanceledBeforeStart) => {
                Err(BlockingSeedTaskError::CanceledBeforeStart)
            }
            Err(error) if error.is_cancelled() => Err(BlockingSeedTaskError::CanceledBeforeStart),
            Err(error) => Err(BlockingSeedTaskError::Join(error.to_string())),
        }
    }

    pub(super) async fn try_response_close<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        registration: &ResponseCloseRegistration,
    ) -> Result<(), SelectionCoordinatorError> {
        let key = CriticalAdmissionKey {
            session_id: registration.member.session_id,
            kind: CriticalAdmissionKind::ResponseClose,
        };
        let critical = {
            let mut keys = self.inner.critical_keys.lock().map_err(|_| {
                registration.diagnostic_poll_outcome(3, "critical_key_lock_error");
                SelectionCoordinatorError::Unavailable
            })?;
            if !keys.insert(key) {
                registration.diagnostic_poll_outcome(4, "duplicate_critical_key");
                return Ok(());
            }
            CriticalAdmissionGuard::new(&self.inner, key)
        };
        let (admission, slot) = self.try_reserve_envelope().inspect_err(|_| {
            registration.diagnostic_poll_outcome(5, "reserve_envelope_error");
        })?;
        let tracker = app
            .state::<crate::pty::manager::ResponseCloseState>()
            .inner()
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let manager = self.inner.manager.read().await.clone();
        let settings = app.state::<crate::config::settings::SettingsState>();
        let settings = settings.read().await;
        let carrier_permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        if self.restore_is_active() {
            registration.diagnostic_poll_outcome(6, "restore_active");
            return Ok(());
        }
        let mut diagnostic_reason = (7, "ticket_absent");
        let ticket = manager
            .with_current_carrier_snapshot(&carrier_permit, |current| {
                let member =
                    response_registration_row(current, &registration.member).or_else(|| {
                        if log::log_enabled!(
                            target: "agentscommander_lib::response_close_diag",
                            log::Level::Debug
                        ) {
                            diagnostic_reason = (8, "current_member_rejected");
                        }
                        None
                    })?;
                let coordinator = response_registration_row(current, &registration.coordinator)
                    .or_else(|| {
                        if log::log_enabled!(
                            target: "agentscommander_lib::response_close_diag",
                            log::Level::Debug
                        ) {
                            diagnostic_reason = (9, "current_coordinator_rejected");
                        }
                        None
                    })?;
                if member.backend_kind != crate::pty::backend::SessionBackendKind::LocalProcess
                    || member.is_root_agent
                    || member.is_coordinator
                    || member.agent_kind.is_none()
                    || !coordinator.is_coordinator
                    || coordinator.is_root_agent
                {
                    if log::log_enabled!(
                        target: "agentscommander_lib::response_close_diag",
                        log::Level::Debug
                    ) {
                        diagnostic_reason = (10, "role_or_backend_rejected");
                    }
                    return None;
                }
                if !response_registration_route(&pty, &registration.member, member)
                    || !response_registration_route(&pty, &registration.coordinator, coordinator)
                {
                    if log::log_enabled!(
                        target: "agentscommander_lib::response_close_diag",
                        log::Level::Debug
                    ) {
                        diagnostic_reason = (11, "member_or_coordinator_route_rejected");
                    }
                    return None;
                }
                let now = Instant::now();
                tracker.apply_settings(
                    settings.response_close_enabled,
                    settings.response_close_idle_seconds,
                    now,
                );
                tracker.publish_manager_facts(
                    registration.member.clone(),
                    true,
                    crate::session::session::is_working(member),
                    now,
                );
                tracker.publish_manager_facts(
                    registration.coordinator.clone(),
                    false,
                    crate::session::session::is_working(coordinator),
                    now,
                );
                match tracker.select_ticket(member.id, now) {
                    Ok(ticket) => Some(ticket),
                    Err(error) => {
                        if log::log_enabled!(
                            target: "agentscommander_lib::response_close_diag",
                            log::Level::Debug
                        ) {
                            use crate::pty::manager::ResponseCloseError;
                            diagnostic_reason = match error {
                                ResponseCloseError::Unavailable => {
                                    (12, "select_ticket_unavailable")
                                }
                                ResponseCloseError::Unknown => (13, "select_ticket_unknown"),
                                ResponseCloseError::Conflict => (14, "select_ticket_conflict"),
                                ResponseCloseError::ClosingDeferred => {
                                    (15, "select_ticket_closing_deferred")
                                }
                            };
                        }
                        None
                    }
                }
            })
            .await;
        drop(carrier_permit);
        let Some(ticket) = ticket else {
            registration.diagnostic_poll_outcome(diagnostic_reason.0, diagnostic_reason.1);
            return Ok(());
        };
        if ticket.committed_binding().member != registration.member {
            registration.diagnostic_poll_outcome(16, "committed_binding_mismatch");
            return Ok(());
        }
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::ConditionalResponseClose {
                request: ConditionalResponseClose { ticket },
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(critical),
        }));
        registration.diagnostic_poll_outcome(17, "envelope_submitted");
        Ok(())
    }

    pub(crate) async fn restart_lifecycle_with_executor<R: Runtime>(
        &self,
        app: AppHandle<R>,
        request: RestartJobRequest,
        executor: Option<Arc<dyn LifecycleExecutor<SelectionTransaction<R>>>>,
    ) -> Result<SessionInfo, String> {
        let executor =
            executor.ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        self.submit_lifecycle_entry(
            app,
            OwnedReplacementDescriptor::EntryRestart(request),
            executor,
        )
        .await
    }

    pub(crate) async fn root_lifecycle_with_executor<R: Runtime>(
        &self,
        app: AppHandle<R>,
        request: RootJobRequest,
        executor: Option<Arc<dyn LifecycleExecutor<SelectionTransaction<R>>>>,
    ) -> Result<SessionInfo, String> {
        let executor =
            executor.ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        self.submit_lifecycle_entry(
            app,
            OwnedReplacementDescriptor::EntryRoot(request),
            executor,
        )
        .await
    }

    async fn submit_lifecycle_entry<R: Runtime>(
        &self,
        app: AppHandle<R>,
        descriptor: OwnedReplacementDescriptor,
        executor: Arc<dyn LifecycleExecutor<SelectionTransaction<R>>>,
    ) -> Result<SessionInfo, String> {
        // Original real capacity probe precedes acceptance and validation.
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (owner, executor_nonce) = Self::prepare_lifecycle_owner(descriptor);
        let (publication, result) = oneshot::channel();
        self.insert_lifecycle_entry(&owner)?;
        drop(slot);
        drop(admission);
        let coordinator = self.clone();
        crate::config::sessions_persistence::run_owned_persist_core(async move {
            let execution = coordinator
                .execute_accepted_entry(&app, owner.clone(), executor_nonce, executor)
                .await;
            coordinator
                .finish_lifecycle_entry(&owner, executor_nonce, execution, publication)
                .await;
        })
        .await;
        result
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    async fn execute_accepted_entry<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        owner: Arc<ReplacementOwner>,
        executor_nonce: Uuid,
        executor: Arc<dyn LifecycleExecutor<SelectionTransaction<R>>>,
    ) -> (Result<SessionInfo, String>, bool) {
        if let Err(error) = self.wait_for_restore_release().await {
            return (Err(error), true);
        }
        let transaction = SelectionTransaction::new(app.clone(), Weak::new())
            .with_lifecycle_executor(Arc::clone(&executor));
        if matches!(&owner.descriptor, OwnedReplacementDescriptor::EntryRoot(_)) {
            let effect = Self::initial_root_directory_effect(&owner, executor_nonce);
            let Some(effect) = effect else {
                return std::future::pending().await;
            };
            if let Err(error) = executor.ensure_root_directory(effect) {
                return (Err(error), true);
            }
        }
        let mut root_directory = owner.state.lock().ok().and_then(|state| {
            match &state.effects[LifecycleEffectSlot::RootDirectory as usize] {
                OwnedEffectState::Completed {
                    outcome: Ok(Some(path)),
                    ..
                } => Some(path.clone()),
                _ => None,
            }
        });
        let (requested, root_request) = match &owner.descriptor {
            OwnedReplacementDescriptor::EntryRestart(request) => (Some(request.session_id), false),
            OwnedReplacementDescriptor::EntryRoot(_) => (None, true),
            OwnedReplacementDescriptor::Replacement(_) => return std::future::pending().await,
        };
        let authority = loop {
            if let Err(error) = self.wait_for_restore_release().await {
                return (Err(error), true);
            }
            if self.inner.shutdown.is_cancelled() {
                return (
                    Err(SelectionCoordinatorError::Unavailable.to_string()),
                    true,
                );
            }
            let admission = tokio::select! {
                biased;
                _ = self.inner.shutdown.cancelled() => {
                    return (Err(SelectionCoordinatorError::Unavailable.to_string()), true);
                }
                admission = executor.prepare(
                    &transaction, owner.operation, requested, root_request,
                    root_directory.as_deref(),
                ) => admission,
            };
            match admission {
                Ok(LifecycleAdmission::Ready(authority)) => break authority,
                Ok(LifecycleAdmission::Waiting) => {
                    // Every failed probe has dropped its own stripe/exact/claim.
                    tokio::task::yield_now().await;
                }
                Ok(LifecycleAdmission::NeedsRootDirectory) => {
                    let effect = owner.state.lock().ok().and_then(|mut state| {
                        if state.executor_nonce != executor_nonce
                            || !matches!(
                                state.effects[LifecycleEffectSlot::RootDirectory as usize],
                                OwnedEffectState::NotStarted
                            )
                        {
                            return None;
                        }
                        let attempt = Uuid::new_v4();
                        state.effects[LifecycleEffectSlot::RootDirectory as usize] =
                            OwnedEffectState::Running(attempt);
                        Some(LifecycleEffectGuard {
                            owner: owner.clone(),
                            slot: LifecycleEffectSlot::RootDirectory,
                            attempt,
                            completed: false,
                        })
                    });
                    let Some(effect) = effect else {
                        return std::future::pending().await;
                    };
                    match executor.ensure_root_directory(effect) {
                        Ok(path) => root_directory = Some(path),
                        Err(error) => return (Err(error), true),
                    }
                }
                Err(error) => return (Err(error), true),
            }
        };
        if let Ok(mut state) = owner.state.lock() {
            state.claim_reached = executor.has_claim(&authority);
        }
        let (admission, slot) = match self.try_reserve_envelope() {
            Ok(capacity) => capacity,
            Err(error) => {
                let released = executor.release(authority);
                if let Ok(mut state) = owner.state.lock() {
                    state.sidecar_released = released;
                }
                return (Err(error.to_string()), true);
            }
        };
        let (response, receiver) = oneshot::channel();
        let entry = OwnedLifecycleEntry {
            owner: owner.clone(),
            executor_nonce,
            authority: Some(authority),
        };
        if let Ok(mut state) = owner.state.lock() {
            state.stage = ReplacementStage::Queued;
        }
        let job = match &owner.descriptor {
            OwnedReplacementDescriptor::EntryRestart(request) => CoordinatorJob::RestartLifecycle {
                request: request.clone(),
                entry,
                response,
            },
            OwnedReplacementDescriptor::EntryRoot(request) => CoordinatorJob::RootLifecycle {
                request: request.clone(),
                entry,
                response,
            },
            OwnedReplacementDescriptor::Replacement(_) => return std::future::pending().await,
        };
        drop(slot.send(CoordinatorEnvelope {
            job,
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        match receiver.await {
            Ok(result) => (result, false),
            Err(_) => {
                // Lost internal outcome has no public fabricated result or retry.
                if let Ok(mut state) = owner.state.lock() {
                    state.stage = ReplacementStage::Ambiguous;
                    state
                        .retained_reason
                        .get_or_insert_with(|| "lifecycle internal result lost".into());
                }
                std::future::pending().await
            }
        }
    }

    fn initial_root_directory_effect(
        owner: &Arc<ReplacementOwner>,
        executor_nonce: Uuid,
    ) -> Option<LifecycleEffectGuard> {
        owner.state.lock().ok().and_then(|mut state| {
            if state.executor_nonce != executor_nonce
                || state.stage != ReplacementStage::EntryWaiting
            {
                return None;
            }
            let attempt = Uuid::new_v4();
            state.effects[LifecycleEffectSlot::RootDirectory as usize] =
                OwnedEffectState::Running(attempt);
            Some(LifecycleEffectGuard {
                owner: owner.clone(),
                slot: LifecycleEffectSlot::RootDirectory,
                attempt,
                completed: false,
            })
        })
    }
}

pub struct RestoreBarrierGuard {
    pub(super) coordinator: Weak<CoordinatorInner>,
    pub(super) nonce: Uuid,
    pub(super) release: Option<oneshot::Sender<()>>,
}

impl RestoreBarrierGuard {
    pub(crate) fn transaction<R: Runtime>(&self, app: AppHandle<R>) -> SelectionTransaction<R> {
        let mut transaction = SelectionTransaction::new(app, self.coordinator.clone());
        transaction.restore_nonce = Some(self.nonce);
        transaction
    }

    fn release_worker(&mut self) {
        if let Some(release) = self.release.take() {
            if release.send(()).is_err() {
                log::warn!("[selection] restore barrier worker ended before completion signal");
            }
        }
    }

    pub fn finish(mut self) {
        self.release_worker();
    }
}

impl Drop for RestoreBarrierGuard {
    fn drop(&mut self) {
        if self.release.is_some() {
            log::warn!("[selection] restore barrier released by dropped owner");
            self.release_worker();
        }
    }
}
