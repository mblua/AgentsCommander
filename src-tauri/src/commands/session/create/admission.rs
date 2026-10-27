use crate::commands::session::lifecycle::LifecycleEntryAuthority;
#[cfg(test)]
use crate::commands::session::restore_emit_seam;
use crate::config::coordinator_clocks::ClearedCloseMarkers;
use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::resource_monitor::AgentLaunchPermit;
use crate::resource_monitor::ResourceLimits;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::acquire_carrier_mutation_permit;
use crate::session::manager::PendingCreateBinding;
use crate::session::manager::SessionManager;
use crate::session::selection::SelectionCoordinator;
use crate::session::selection::SelectionTransaction;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use uuid::Uuid;

/// (#2413) One in-flight coordinator create. Must end with `succeeded()` or
/// `failed()`; Drop without either counts as `failed()` (cancel/panic safety);
/// `failed()`/Drop with the spawn fence set counts as `succeeded()`.
pub(crate) struct CoordinatorCreateTicket<R: tauri::Runtime> {
    pub(crate) app: tauri::AppHandle<R>,
    coordinator_order: Arc<crate::config::coordinator_clocks::CoordinatorCreateOrder>,
    pub(crate) cwd: String,
    pub(crate) fqn: String,
    pub(crate) done: bool,
    /// Spawn fence: true once a PTY may exist. Then every end counts as success.
    pub(crate) session_may_exist: bool,
}

impl<R: tauri::Runtime> CoordinatorCreateTicket<R> {
    pub(crate) fn succeeded(mut self) {
        self.done = true;
        self.finish_ok();
    }

    pub(crate) fn failed(mut self) {
        self.finish_failed();
    }

    fn finish_ok(&self) {
        if let Some(clocks) = self
            .app
            .try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
            g.finish_coordinator_create(&self.fqn, true);
        }
    }

    fn finish_failed(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        if self.session_may_exist {
            self.finish_ok();
            log::info!(
                "[coordinator-create] create failed after spawn attempt; close markers stay \
                 cleared fqn={}",
                self.fqn
            );
            return;
        }
        let Some(clocks) = self
            .app
            .try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        else {
            return;
        };
        // Held through the emits below (see `CLOSE_MARKER_EVENT_ORDER`).
        self.coordinator_order.with_order(|| {
            let (escrow, manual, auto) = {
                let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
                let escrow = g.finish_coordinator_create(&self.fqn, false);
                let mut manual = None;
                let mut auto = None;
                if let Some(c) = escrow {
                    if let Some(t) = c.manually_closed_at {
                        if g.mark_manually_closed(&self.fqn, t) {
                            manual = Some(t);
                        }
                    }
                    if let Some(t) = c.auto_closed_at {
                        if g.mark_auto_closed(&self.fqn, t) {
                            auto = Some(t);
                        }
                    }
                }
                (escrow, manual, auto)
            };
            #[cfg(test)]
            restore_emit_seam::hit(&self.fqn);
            if let Some(t) = manual {
                let _ = self.app.emit(
                "coordinator_manual_close_changed",
                serde_json::json!({ "replicaPath": self.cwd, "manuallyClosedAt": t.to_rfc3339() }),
            );
            }
            if let Some(t) = auto {
                let _ = self.app.emit(
                    "coordinator_auto_close_changed",
                    serde_json::json!({ "replicaPath": self.cwd, "autoClosedAt": t.to_rfc3339() }),
                );
            }
            if manual.is_some() || auto.is_some() {
                log::info!(
                "[coordinator-create] restored close markers after failed create fqn={} auto={:?} \
                 manual={:?}",
                self.fqn,
                auto,
                manual
            );
            } else if escrow.is_none() {
                log::info!(
                "[coordinator-create] failed create handed markers to an in-flight create fqn={}",
                self.fqn
            );
            }
        });
    }
}

impl<R: tauri::Runtime> Drop for CoordinatorCreateTicket<R> {
    fn drop(&mut self) {
        self.finish_failed();
    }
}

/// The accepted create core retains this record independently of its waiter.
/// Reservations are renewed only under U, against current ALL-scope facts.
pub(crate) struct CreateResourceAdmission {
    pub(crate) monitor: Arc<ResourceMonitorState>,
    pub(crate) settings: SettingsState,
    pub(crate) permit: Option<AgentLaunchPermit>,
    pub(crate) needed: bool,
    pub(crate) in_region: bool,
    pub(crate) released: bool,
}

impl Drop for CreateResourceAdmission {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            self.monitor.release_unregistered_permit(permit);
        }
    }
}

pub(crate) struct CreateResourceRegion<'a> {
    local: &'a mut Option<AgentLaunchPermit>,
    resource: Arc<Mutex<CreateResourceAdmission>>,
}

impl Drop for CreateResourceRegion<'_> {
    fn drop(&mut self) {
        if let Ok(mut resource) = self.resource.lock() {
            *self.local = resource.permit.take();
            resource.in_region = false;
        }
    }
}

pub(crate) async fn with_create_resource_region<T>(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    local: &mut Option<AgentLaunchPermit>,
    effect: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let resource = owner
        .lock()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?
        .resource
        .clone();
    let Some(resource) = resource else {
        return effect.await;
    };
    {
        let mut state = resource
            .lock()
            .map_err(|_| "resource admission unavailable".to_string())?;
        if state.in_region || state.permit.is_some() {
            return Err("resource admission already owned".to_string());
        }
        state.permit = local.take();
        state.in_region = true;
        state.released = false;
    }
    let region = CreateResourceRegion { local, resource };
    let result = effect.await;
    drop(region);
    result
}

pub(crate) async fn revalidate_owned_create_effect(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    resource: &mut Option<AgentLaunchPermit>,
    monitor: &Arc<ResourceMonitorState>,
    manager: &SessionManager,
    pty: &Arc<Mutex<PtyManager>>,
) -> Result<(), String> {
    let continuation = owner
        .lock()
        .ok()
        .and_then(|owner| owner.owned_replacement.clone());
    let Some(continuation) = continuation else {
        release_resource_launch_permit(monitor, resource);
        return Err("targetCreateGateUnavailable".into());
    };
    let result = with_create_resource_region(owner, resource, async {
        if continuation.validate_pre_effect_region(manager, pty).await {
            Ok(())
        } else {
            Err("targetCreateGateUnavailable".to_string())
        }
    })
    .await;
    if result.is_err() {
        release_resource_launch_permit(monitor, resource);
        if let Ok(mut owner) = owner.lock() {
            owner.blocked_bound = true;
        }
    }
    result
}

#[derive(Clone)]
pub(crate) struct CreateCapacityContext {
    pub(crate) ticket: Arc<Mutex<Option<crate::session::selection::CreateFinalizationTicket>>>,
    pub(crate) coordinator: SelectionCoordinator,
}

pub(crate) struct OwnedCreateTargetLease {
    pub(crate) key: String,
    pub(crate) gate: Arc<crate::api::message_store::PtyInputTargetGate>,
    pub(crate) replica: crate::path_identity::VerifiedPathIdentity,
    // Exact precedes stripe in actual drop order.
    pub(crate) guards: Option<(
        crate::api::message_store::PtyInputTargetGuard,
        crate::api::message_store::PtyInputStripeGuard,
    )>,
}

pub(crate) struct OwnedCreateAdmission {
    pub(crate) preheld_target: bool,
    pub(crate) owned_replacement: Option<crate::session::selection::OwnedReplacementContinuation>,
    pub(crate) replacement_pty: Option<Arc<Mutex<PtyManager>>>,
    pub(crate) tracker: crate::pty::manager::ResponseCloseState,
    pub(crate) target: String,
    pub(crate) cwd: PathBuf,
    pub(crate) nonce: Uuid,
    pub(crate) binding: PendingCreateBinding,
    pub(crate) reservation: Option<crate::pty::manager::CreateOwnerReservation>,
    pub(crate) failed_foreign: bool,
    pub(crate) owned_target: Option<OwnedCreateTargetLease>,
    pub(crate) capacity: Option<CreateCapacityContext>,
    pub(crate) resource: Option<Arc<Mutex<CreateResourceAdmission>>>,
    pub(crate) blocked_bound: bool,
    pub(crate) capacity_error: Option<String>,
}

impl OwnedCreateAdmission {
    fn classify_current(
        &self,
        scope: crate::config::sessions_persistence::CreateCarrierScope,
        current: &crate::session::manager::CarrierSnapshot,
    ) -> Option<(crate::pty::manager::CreateClassification, bool)> {
        use crate::config::sessions_persistence::CreateCarrierScope;
        use crate::pty::manager::CreateClassification;
        Some(match scope {
            CreateCarrierScope::Foreign { publication_epoch } => {
                (CreateClassification::Foreign { publication_epoch }, true)
            }
            CreateCarrierScope::Waiting => return None,
            CreateCarrierScope::Protected {
                target,
                session_id,
                token,
                version,
                epoch,
                preparing_owner,
            } => {
                let row = current.rows.iter().find(|row| {
                    row.id == session_id
                        && row.token == token
                        && !current.pending_ids.contains(&row.id)
                })?;
                if current.versions.get(&session_id) != Some(&version) {
                    return None;
                }
                let generation = self.tracker.generation(session_id)?;
                let identity = crate::pty::manager::ResponseRouteIdentity {
                    target,
                    session_id,
                    token,
                    generation,
                    backend: row.backend_kind,
                    carrier_version: version,
                };
                match preparing_owner {
                    Some(owner_nonce) => (
                        CreateClassification::Preparing {
                            owner_nonce,
                            identity,
                            publication_epoch: epoch,
                        },
                        owner_nonce == self.nonce,
                    ),
                    None => (
                        CreateClassification::Protected {
                            identity,
                            publication_epoch: epoch,
                        },
                        false,
                    ),
                }
            }
        })
    }

    pub(crate) fn reserve_current(
        &mut self,
        scope: crate::config::sessions_persistence::CreateCarrierScope,
        current: &crate::session::manager::CarrierSnapshot,
    ) -> bool {
        use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseCloseError};
        self.failed_foreign = false;
        self.reservation = None;
        let Some((classification, may_mutate)) = self.classify_current(scope, current) else {
            return false;
        };
        // Generic G never reserves an exclusive Protected claim while waiting
        // for R. That would veto the owner whose progress G needs.
        self.reservation = None;
        if !may_mutate {
            return false;
        }
        let foreign = matches!(&classification, CreateClassification::Foreign { .. });
        let protected_generation = match &classification {
            CreateClassification::Protected { identity, .. }
            | CreateClassification::Preparing { identity, .. } => {
                Some((identity.session_id, identity.generation))
            }
            _ => None,
        };
        match self
            .tracker
            .reserve_create_owner(&self.target, self.nonce, classification)
        {
            Ok(CreateAdmission::Reserved(reservation)) => {
                self.reservation = Some(reservation);
                may_mutate
                    && protected_generation.is_none_or(|(id, generation)| {
                        self.tracker.generation(id) == Some(generation)
                    })
            }
            Err(ResponseCloseError::Unavailable)
                if foreign && !self.tracker.target_is_closing(&self.target) =>
            {
                // Unavailable is permanently latched by this exact tracker.
                // The upper owner stays alive; this is not a reset or a proof
                // inferred from missing facts, poison, or a different tracker.
                self.failed_foreign = true;
                self.failed_foreign
            }
            _ => false,
        }
    }
}

pub(crate) fn release_owned_create_target(owner: &Arc<Mutex<OwnedCreateAdmission>>) {
    let released = owner.lock().ok().map(|mut owner| {
        owner.reservation = None;
        let guards = owner
            .owned_target
            .as_mut()
            .and_then(|lease| lease.guards.take());
        (guards, owner.capacity.clone(), owner.resource.clone())
    });
    let Some((guards, capacity, resource)) = released else {
        return;
    };
    drop(guards);
    if let Some(resource) = resource {
        if let Ok(mut resource) = resource.lock() {
            if resource.in_region && resource.needed {
                if let Some(permit) = resource.permit.take() {
                    resource.monitor.release_unregistered_permit(permit);
                }
                resource.released = true;
            }
        }
    }
    if let Some(capacity) = capacity {
        let unbound = capacity
            .ticket
            .lock()
            .ok()
            .and_then(|mut ticket| {
                ticket
                    .as_mut()
                    .map(|ticket| ticket.release_capacity_for_wait())
            })
            .unwrap_or(false);
        if !unbound {
            if let Ok(mut owner) = owner.lock() {
                owner.blocked_bound = true;
            }
        }
    }
}

pub(crate) fn resume_owned_create_capacity(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
) -> Result<(), String> {
    let capacity = owner
        .lock()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?
        .capacity
        .clone();
    let Some(capacity) = capacity else {
        return Ok(());
    };
    let mut ticket = capacity
        .ticket
        .lock()
        .map_err(|_| "create ticket unavailable".to_string())?;
    let Some(ticket) = ticket.as_mut() else {
        return Err("create ticket unavailable".to_string());
    };
    ticket
        .resume_unbound_capacity(&capacity.coordinator)
        .map_err(|error| error.to_string())
}

pub(crate) async fn resume_create_resource_admission(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
) -> Result<(), String> {
    let resource = owner
        .lock()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?
        .resource
        .clone();
    let Some(resource) = resource else {
        return Ok(());
    };
    let (monitor, settings) = {
        let resource = resource
            .lock()
            .map_err(|_| "resource admission unavailable".to_string())?;
        if !resource.in_region || !resource.released || !resource.needed {
            return Ok(());
        }
        (resource.monitor.clone(), resource.settings.clone())
    };
    loop {
        // Do not occupy quota needed by the protected owner while G waits.
        // This is only a preflight probe; target/U/M admission still follows.
        let permit = acquire_carrier_mutation_permit().await;
        let eligible = owner.lock().ok().is_some_and(|owner| {
            use crate::config::sessions_persistence::CreateCarrierScope;
            match crate::config::sessions_persistence::classify_create_carrier_scope(
                &permit,
                &owner.target,
                &owner.cwd,
            ) {
                CreateCarrierScope::Foreign { .. } => true,
                CreateCarrierScope::Protected {
                    preparing_owner: Some(nonce),
                    ..
                } => nonce == owner.nonce,
                _ => false,
            }
        });
        drop(permit);
        if eligible {
            break;
        }
        tokio::task::yield_now().await;
    }
    // Fresh limits, outside U/S/target guards. No retry on a real quota error.
    let cfg = settings.read().await;
    let next = monitor.try_reserve_agent_slot(ResourceLimits::from(&*cfg))?;
    drop(cfg);
    let mut state = match resource.lock() {
        Ok(state) => state,
        Err(_) => {
            if let Some(permit) = next {
                monitor.release_unregistered_permit(permit);
            }
            return Err("resource admission unavailable".to_string());
        }
    };
    if !state.in_region || !state.released || state.permit.is_some() {
        if let Some(permit) = next {
            monitor.release_unregistered_permit(permit);
        }
        return Err("resource admission ownership changed".to_string());
    }
    state.permit = next;
    state.released = false;
    Ok(())
}

pub(crate) async fn ensure_owned_create_target(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
) -> Result<(), String> {
    // Archive invokes this preparation before acquiring its settings write
    // guard. Its later settings-first admission remains a pure U check.
    let replacement = owner
        .lock()
        .ok()
        .and_then(|owner| owner.owned_replacement.clone());
    if let Some(replacement) = replacement {
        if !replacement.validate_source().await {
            return Err("targetCreateGateUnavailable".into());
        }
    }
    let (blocked_bound, capacity_error) = {
        let owner = owner
            .lock()
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        (owner.blocked_bound, owner.capacity_error.clone())
    };
    if let Some(error) = capacity_error {
        return Err(error);
    }
    if blocked_bound {
        // A new conflicting protection across admitted Pending is an invariant
        // violation. Keep the accepted core/binding unsafe, with no target or
        // finalizer capacity and no invented public outcome or repeated effect.
        return std::future::pending().await;
    }
    resume_create_resource_admission(owner).await?;
    let required = {
        let owner = owner
            .lock()
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        owner
            .owned_target
            .as_ref()
            .filter(|lease| lease.guards.is_none())
            .map(|lease| {
                (
                    lease.gate.clone(),
                    lease.key.clone(),
                    lease.replica.clone(),
                    owner.cwd.clone(),
                )
            })
    };
    let Some((gate, key, replica, cwd)) = required else {
        return Ok(());
    };
    let stripe = gate
        .acquire_target_lock(&key)
        .await
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let exact = gate.acquire_exact(&key).await;
    let current = crate::path_identity::verify_directory(&replica.canonical_path)
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let current_key = crate::config::teams::pty_input_create_gate_key_from_cwd(&cwd)
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    if current.object_id != replica.object_id
        || current.canonical_path != replica.canonical_path
        || current_key.as_deref() != Some(key.as_str())
    {
        return Err("targetCreateGateUnavailable".to_string());
    }
    let mut owner = owner
        .lock()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let Some(lease) = owner.owned_target.as_mut() else {
        return Err("targetCreateGateUnavailable".to_string());
    };
    if lease.key != key || lease.guards.is_some() {
        return Err("targetCreateGateUnavailable".to_string());
    }
    lease.guards = Some((exact, stripe));
    Ok(())
}

pub(crate) async fn try_acquire_create_publication_region(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
) -> Option<crate::session::manager::CarrierMutationPermit> {
    try_acquire_create_publication_region_checked(owner, manager, || true).await
}

pub(crate) async fn try_acquire_create_publication_region_checked(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
    context_is_current: impl Fn() -> bool,
) -> Option<crate::session::manager::CarrierMutationPermit> {
    if owner
        .lock()
        .ok()?
        .owned_target
        .as_ref()
        .is_some_and(|lease| lease.guards.is_none())
    {
        return None;
    }
    let permit = acquire_carrier_mutation_permit().await;
    let scope = owner.lock().ok().map(|owner| {
        crate::config::sessions_persistence::classify_create_carrier_scope(
            &permit,
            &owner.target,
            &owner.cwd,
        )
    })?;
    let admitted = manager
        .with_current_carrier_snapshot(&permit, |current| {
            owner.lock().ok().is_some_and(|mut owner| {
                context_is_current()
                    && match (
                        owner.owned_replacement.as_ref(),
                        owner.replacement_pty.as_ref(),
                    ) {
                        (Some(continuation), Some(pty)) => continuation.validate_create_region(
                            &permit,
                            current,
                            &owner.tracker,
                            pty,
                        ),
                        (None, _) => owner.reserve_current(scope, current),
                        _ => false,
                    }
            })
        })
        .await;
    if admitted {
        if let Err(error) = resume_owned_create_capacity(owner) {
            if let Ok(mut owner) = owner.lock() {
                owner.capacity_error = Some(error);
            }
            drop(permit);
            release_owned_create_target(owner);
            return None;
        }
        Some(permit)
    } else {
        drop(permit);
        release_owned_create_target(owner);
        None
    }
}

pub(crate) async fn acquire_create_publication_region(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
) -> Result<crate::session::manager::CarrierMutationPermit, String> {
    loop {
        ensure_owned_create_target(owner).await?;
        if let Some(permit) = try_acquire_create_publication_region(owner, manager).await {
            return Ok(permit);
        }
        tokio::task::yield_now().await;
    }
}

/// A borrowed lifecycle lease cannot be released by common create. A changed
/// region returns through the explicit private outcome to its lexical owner;
/// generic creates release and reacquire their actual owned lease and capacity.
pub(crate) async fn acquire_lifecycle_create_publication_region<R: tauri::Runtime>(
    owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
    domain: Option<&LifecycleEntryAuthority>,
    transaction: Option<&SelectionTransaction<R>>,
) -> Result<crate::session::manager::CarrierMutationPermit, String> {
    let replacement = owner
        .lock()
        .ok()
        .and_then(|owner| owner.owned_replacement.clone());
    if let Some(replacement) = replacement {
        if !replacement.validate_source().await {
            return Err("targetCreateGateUnavailable".into());
        }
        return match try_acquire_create_publication_region(owner, manager).await {
            Some(permit) => Ok(permit),
            None => {
                if let Ok(mut owner) = owner.lock() {
                    owner.blocked_bound = true;
                }
                Err("targetCreateGateUnavailable".into())
            }
        };
    }
    let Some(domain) = domain else {
        if owner.lock().ok().is_some_and(|owner| owner.preheld_target) {
            return match try_acquire_create_publication_region(owner, manager).await {
                Some(permit) => Ok(permit),
                None => {
                    if let Ok(mut owner) = owner.lock() {
                        owner.blocked_bound = true;
                    }
                    Err("targetCreateGateUnavailable".into())
                }
            };
        }
        if let Some(transaction) =
            transaction.filter(|transaction| transaction.has_restore_context())
        {
            if transaction.has_active_restore_capability() {
                if let Some(permit) =
                    try_acquire_create_publication_region_checked(owner, manager, || {
                        transaction.has_active_restore_capability()
                    })
                    .await
                {
                    return Ok(permit);
                }
            }
            transaction.invalidate_restore_ownership();
            release_owned_create_target(owner);
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
        return acquire_create_publication_region(owner, manager).await;
    };
    let active_context = transaction.is_some_and(|transaction| {
        transaction.has_accepted_lifecycle_core() || transaction.has_active_restore_capability()
    });
    let cwd = owner.lock().ok().map(|owner| owner.cwd.clone());
    let valid_destination = cwd.is_some_and(|cwd| {
        crate::config::teams::pty_input_create_gate_key_from_cwd(&cwd)
            .is_ok_and(|key| domain.validates_destination(&cwd.to_string_lossy(), key.as_deref()))
    });
    if active_context
        && valid_destination
        && !domain
            .interrupted
            .load(std::sync::atomic::Ordering::Acquire)
    {
        if let Some(permit) = try_acquire_create_publication_region_checked(owner, manager, || {
            transaction.is_some_and(|transaction| {
                transaction.has_accepted_lifecycle_core()
                    || transaction.has_active_restore_capability()
            })
        })
        .await
        {
            return Ok(permit);
        }
    }
    if let Some(transaction) = transaction {
        transaction.invalidate_restore_ownership();
    }
    domain
        .interrupted
        .store(true, std::sync::atomic::Ordering::Release);
    Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string())
}

/// The accepted archive operation owns its application and inputs. Cancelling
/// the create waiter cannot detach native mutation from settings publication.
pub(crate) async fn enforce_owned_unarchived_for_spawn<R: tauri::Runtime>(
    app: &AppHandle<R>,
    cwd: &str,
    session_label: &str,
    create_owner: &Arc<Mutex<OwnedCreateAdmission>>,
    manager: &SessionManager,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
    inline_transaction: Option<&SelectionTransaction<R>>,
) -> Result<(), String> {
    if lifecycle_domain.is_some()
        || inline_transaction.is_some_and(|transaction| transaction.has_restore_context())
    {
        // The actual outer lifecycle/restore executor already owns this effect.
        // Its borrow and outcome must return to that same executor, not a child.
        let tracker = app
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .response_close_state();
        return crate::config::archive_gate::enforce_unarchived_for_spawn(
            app,
            cwd,
            session_label,
            || async { ensure_owned_create_target(create_owner).await },
            || async {
                acquire_lifecycle_create_publication_region(
                    create_owner,
                    manager,
                    lifecycle_domain,
                    inline_transaction,
                )
                .await
                .map(Some)
            },
            |written| {
                tracker.apply_settings(
                    written.response_close_enabled,
                    written.response_close_idle_seconds,
                    std::time::Instant::now(),
                )
            },
        )
        .await;
    }
    let create_owner = create_owner.clone();
    let manager = manager.clone();
    let app = app.clone();
    let cwd = cwd.to_owned();
    let session_label = session_label.to_owned();
    let tracker = app
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .response_close_state();
    crate::config::sessions_persistence::run_owned_persist_core(async move {
        crate::config::archive_gate::enforce_unarchived_for_spawn(
            &app,
            &cwd,
            &session_label,
            || {
                let owner = create_owner.clone();
                async move { ensure_owned_create_target(&owner).await }
            },
            || {
                let owner = create_owner.clone();
                let manager = manager.clone();
                async move {
                    let admitted = try_acquire_create_publication_region(&owner, &manager).await;
                    if admitted.is_none()
                        && owner.lock().ok().is_some_and(|owner| owner.preheld_target)
                    {
                        if let Ok(mut owner) = owner.lock() {
                            owner.blocked_bound = true;
                        }
                        return Err("targetCreateGateUnavailable".to_string());
                    }
                    Ok::<_, String>(admitted)
                }
            },
            |written| {
                tracker.apply_settings(
                    written.response_close_enabled,
                    written.response_close_idle_seconds,
                    std::time::Instant::now(),
                )
            },
        )
        .await
    })
    .await
}

pub(crate) fn release_resource_launch_permit(
    monitor: &Arc<ResourceMonitorState>,
    permit: &mut Option<AgentLaunchPermit>,
) {
    if let Some(permit) = permit.take() {
        monitor.release_unregistered_permit(permit);
    }
}

/// #552 Coordinator create/reopen: seed the badge clock and clear both close markers.
/// #552 seed the badge clock for a coordinator's first spawn so it shows 0m
/// immediately, and clear any "auto-closed" marker. An auto-closed coordinator
/// is DESTROYED, so its reopen flows through this create path (the "create
/// in-place" branch of handleReplicaClick), NOT restart_session_inner.
/// seed_if_absent never overwrites, so a respawn does NOT reset the badge.
/// (#2413) Returns the create's marker ticket (`None` = no clocks state); the
/// caller ends it with `succeeded()` or `failed()` so a failed create puts the
/// cleared markers back.
pub(crate) fn note_coordinator_create_on_app<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &str,
) -> Option<CoordinatorCreateTicket<R>> {
    let clocks = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()?;
    {
        // agent_fqn_from_path returns String (teams.rs:80), not Option.
        let fqn = crate::config::teams::agent_fqn_from_path(cwd);
        let now = chrono::Utc::now();
        let coordinator_order = app
            .state::<crate::app_state::AppOwners>()
            .coordinator_order
            .clone();
        // Held through the emits below: a restore can never emit its timestamp
        // after this create's clear has emitted null.
        coordinator_order.with_order(|| {
            let (seeded, cleared_auto, cleared_manual) = {
                let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
                let seeded = g.seed_if_absent(&fqn, now);
                let auto_before = g.auto_closed_at(&fqn);
                let manual_before = g.manually_closed_at(&fqn);
                let cleared_auto = g.clear_auto_closed(&fqn);
                let cleared_manual = g.clear_manually_closed(&fqn);
                g.begin_coordinator_create(
                    &fqn,
                    ClearedCloseMarkers {
                        auto_closed_at: auto_before.filter(|_| cleared_auto),
                        manually_closed_at: manual_before.filter(|_| cleared_manual),
                    },
                );
                (seeded, cleared_auto, cleared_manual)
            };
            if seeded {
                let _ = app.emit(
                "coordinator_clock_updated",
                serde_json::json!({ "replicaPath": cwd, "lastUserMessageAt": now.to_rfc3339() }),
            );
            }
            if cleared_auto {
                let _ = app.emit(
                    "coordinator_auto_close_changed",
                    serde_json::json!({ "replicaPath": cwd, "autoClosedAt": null }),
                );
            }
            if cleared_manual {
                let _ = app.emit(
                    "coordinator_manual_close_changed",
                    serde_json::json!({ "replicaPath": cwd, "manuallyClosedAt": null }),
                );
            }
            Some(CoordinatorCreateTicket {
                app: app.clone(),
                coordinator_order: coordinator_order.clone(),
                cwd: cwd.to_string(),
                fqn,
                done: false,
                session_may_exist: false,
            })
        })
    }
}
