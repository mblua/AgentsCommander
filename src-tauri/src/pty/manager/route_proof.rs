use crate::errors::AppError;
use crate::pty::backend::SessionBackendKind;
use crate::pty::backend::TerminalScreenCopyRead;
use crate::pty::backend::TerminalScreenRead;
use crate::pty::context_scrape::ContextSessionLiveness;
use crate::pty::manager::PtyAuthorityRouteProof;
use crate::pty::manager::PtyInputPermit;
use crate::pty::manager::PtyManager;
use crate::pty::manager::PtyRouteWriteGuard;
use crate::pty::manager::PtySnapshotRouteProof;
use crate::pty::manager::ResponseCloseError;
use crate::pty::manager::ResponseCloseState;
use crate::pty::manager::ResponseCloseTicket;
use std::sync::Arc;
use uuid::Uuid;

pub(super) fn snapshot_route_proof(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtySnapshotRouteProof, AppError> {
    let manager_guard = manager
        .lock()
        .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
    let registry = manager_guard
        .registry
        .lock()
        .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
    let entry = registry
        .routes
        .get(&session_id)
        .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
    let saved_cwd = entry
        .canonical_cwd_identity
        .clone()
        .ok_or_else(|| AppError::PtyError("unsafe_route_cwd".to_string()))?;
    let saved_replica = entry.verified_replica_anchor.clone();
    Ok(PtySnapshotRouteProof {
        session_id,
        route_generation: entry.generation,
        route_kind: entry.kind,
        route_registry: Arc::clone(&manager_guard.registry),
        route_lifecycle: Arc::clone(&entry.lifecycle_gate),
        backend: manager_guard.backend_for_kind(entry.kind),
        saved_cwd,
        saved_replica,
    })
}

pub(super) fn authority_route_proof(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtyAuthorityRouteProof, AppError> {
    let manager_guard = manager
        .lock()
        .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
    let registry = manager_guard
        .registry
        .lock()
        .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
    let entry = registry
        .routes
        .get(&session_id)
        .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
    Ok(PtyAuthorityRouteProof {
        session_id,
        route_generation: entry.generation,
        route_registry: Arc::clone(&manager_guard.registry),
        route_lifecycle: Arc::clone(&entry.lifecycle_gate),
    })
}

pub async fn acquire_input_writer(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtyInputPermit, AppError> {
    let (route_registry, input_gate, route_lifecycle, generation, backend, response_close) = {
        let manager_guard = manager
            .lock()
            .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
        let registry = manager_guard
            .registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let entry = registry
            .routes
            .get(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        (
            Arc::clone(&manager_guard.registry),
            Arc::clone(&entry.input_gate),
            Arc::clone(&entry.lifecycle_gate),
            entry.generation,
            manager_guard.backend_for_kind(entry.kind),
            manager_guard.response_close.clone(),
        )
    };

    let response_writer = response_close
        .reserve_writer(session_id, generation)
        .map_err(|_| AppError::PtyError("response_close_deferred".to_string()))?;
    let guard = input_gate.lock_owned().await;
    {
        let registry = route_registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let current = registry
            .routes
            .get(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        if current.generation != generation {
            return Err(AppError::PtyError("stale_pty_route".to_string()));
        }
    }

    Ok(PtyInputPermit {
        session_id,
        route_generation: generation,
        route_registry,
        route_lifecycle,
        backend,
        _guard: guard,
        _response_writer: response_writer,
    })
}

pub(super) fn acquire_route_lifecycle(
    gate: &std::sync::Mutex<()>,
) -> Result<std::sync::MutexGuard<'_, ()>, AppError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
    loop {
        match gate.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(AppError::PtyError("pty_route_poisoned".to_string()))
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Err(AppError::PtyError("pty_route_busy".to_string()));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

impl PtyAuthorityRouteProof {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn try_commit_response_close(
        &self,
        state: &ResponseCloseState,
        ticket: &ResponseCloseTicket,
        now: std::time::Instant,
    ) -> Result<Uuid, ResponseCloseError> {
        if self.session_id != ticket.session_id || self.route_generation != ticket.generation {
            return Err(ResponseCloseError::Unknown);
        }
        let _lifecycle = self
            .route_lifecycle
            .try_lock()
            .map_err(|_| ResponseCloseError::Unavailable)?;
        let registry = self
            .route_registry
            .lock()
            .map_err(|_| ResponseCloseError::Unavailable)?;
        if !registry.routes.get(&self.session_id).is_some_and(|route| {
            route.generation == self.route_generation && route.kind == ticket.member.backend
        }) {
            return Err(ResponseCloseError::Unknown);
        }
        state.try_commit_response_close(ticket, now)
    }
}

impl PtySnapshotRouteProof {
    pub(crate) fn backend_kind(&self) -> SessionBackendKind {
        self.route_kind
    }

    pub(crate) fn liveness(&self) -> ContextSessionLiveness {
        self.backend.context_session_liveness(self.session_id)
    }

    pub(crate) fn saved_cwd(&self) -> &crate::path_identity::VerifiedPathIdentity {
        &self.saved_cwd
    }

    pub(crate) fn saved_cwd_matches_path(&self, path: &std::path::Path) -> bool {
        crate::path_identity::verify_directory(path)
            .is_ok_and(|actual| crate::path_identity::same_object(&actual, &self.saved_cwd))
    }

    pub(crate) fn saved_replica(&self) -> Option<&crate::path_identity::VerifiedPathIdentity> {
        self.saved_replica.as_ref()
    }

    pub(crate) fn matches_requester_route(
        &self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: Option<&crate::path_identity::VerifiedPathIdentity>,
    ) -> bool {
        let Ok(registry) = self.route_registry.lock() else {
            return false;
        };
        registry
            .routes
            .get(&self.session_id)
            .is_some_and(|current| {
                let replica_matches = match (
                    current.verified_replica_anchor.as_ref(),
                    self.saved_replica.as_ref(),
                    expected_replica,
                ) {
                    (Some(current), Some(saved), Some(expected)) => {
                        crate::path_identity::same_object(current, saved)
                            && crate::path_identity::same_object(current, expected)
                    }
                    (None, None, None) => true,
                    _ => false,
                };
                current.generation == self.route_generation
                    && current.kind == self.route_kind
                    && current.kind == expected_kind
                    && current
                        .canonical_cwd_identity
                        .as_ref()
                        .is_some_and(|identity| {
                            crate::path_identity::same_object(identity, expected_cwd)
                                && crate::path_identity::same_object(identity, &self.saved_cwd)
                        })
                    && replica_matches
            })
    }

    pub(crate) fn capture_verified(
        &self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> TerminalScreenRead {
        let lifecycle_guard = match acquire_route_lifecycle(&self.route_lifecycle) {
            Ok(guard) => guard,
            Err(_) => return TerminalScreenRead::Unavailable,
        };
        {
            let registry = match self.route_registry.lock() {
                Ok(registry) => registry,
                Err(_) => return TerminalScreenRead::Unavailable,
            };
            let Some(current) = registry.routes.get(&self.session_id) else {
                return TerminalScreenRead::Unavailable;
            };
            let cwd_matches = current
                .canonical_cwd_identity
                .as_ref()
                .is_some_and(|identity| {
                    crate::path_identity::same_object(identity, expected_cwd)
                        && crate::path_identity::same_object(identity, &self.saved_cwd)
                });
            let replica_matches =
                current
                    .verified_replica_anchor
                    .as_ref()
                    .is_some_and(|identity| {
                        crate::path_identity::same_object(identity, expected_replica)
                            && self.saved_replica.as_ref().is_some_and(|saved| {
                                crate::path_identity::same_object(identity, saved)
                            })
                    });
            if current.generation != self.route_generation
                || current.kind != self.route_kind
                || current.kind != expected_kind
                || !cwd_matches
                || !replica_matches
            {
                return TerminalScreenRead::Unavailable;
            }
        }
        let copied = self.backend.copy_terminal_screen(self.session_id);
        drop(lifecycle_guard);
        match copied {
            TerminalScreenCopyRead::Copied(captured) => captured
                .into_model(self.session_id, self.route_kind)
                .map(TerminalScreenRead::Captured)
                .unwrap_or(TerminalScreenRead::Unavailable),
            TerminalScreenCopyRead::Unavailable => TerminalScreenRead::Unavailable,
            TerminalScreenCopyRead::TooLarge => TerminalScreenRead::TooLarge,
        }
    }

    pub(crate) fn matches_current(
        &self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> bool {
        let Ok(registry) = self.route_registry.lock() else {
            return false;
        };
        registry
            .routes
            .get(&self.session_id)
            .is_some_and(|current| {
                current.generation == self.route_generation
                    && current.kind == self.route_kind
                    && current.kind == expected_kind
                    && current
                        .canonical_cwd_identity
                        .as_ref()
                        .is_some_and(|identity| {
                            crate::path_identity::same_object(identity, expected_cwd)
                                && crate::path_identity::same_object(identity, &self.saved_cwd)
                        })
                    && current
                        .verified_replica_anchor
                        .as_ref()
                        .is_some_and(|identity| {
                            crate::path_identity::same_object(identity, expected_replica)
                                && self.saved_replica.as_ref().is_some_and(|saved| {
                                    crate::path_identity::same_object(identity, saved)
                                })
                        })
            })
    }
}

impl PtyAuthorityRouteProof {
    pub(crate) fn lock_verified<'a>(
        &'a self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: Option<&crate::path_identity::VerifiedPathIdentity>,
    ) -> Result<std::sync::MutexGuard<'a, ()>, AppError> {
        let lifecycle_guard = acquire_route_lifecycle(&self.route_lifecycle)?;
        {
            let registry = self
                .route_registry
                .lock()
                .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
            let current = registry
                .routes
                .get(&self.session_id)
                .ok_or_else(|| AppError::SessionNotFound(self.session_id.to_string()))?;
            let cwd_matches = current
                .canonical_cwd_identity
                .as_ref()
                .is_some_and(|identity| crate::path_identity::same_object(identity, expected_cwd));
            let replica_matches = match (current.verified_replica_anchor.as_ref(), expected_replica)
            {
                (Some(current), Some(expected)) => {
                    crate::path_identity::same_object(current, expected)
                }
                (None, None) => true,
                _ => false,
            };
            if current.generation != self.route_generation
                || current.kind != expected_kind
                || !cwd_matches
                || !replica_matches
            {
                return Err(AppError::PtyError("stale_pty_route".to_string()));
            }
        }
        Ok(lifecycle_guard)
    }
}

pub fn lock_route_for_write(permit: &PtyInputPermit) -> Result<PtyRouteWriteGuard<'_>, AppError> {
    let lifecycle_guard = acquire_route_lifecycle(&permit.route_lifecycle)?;
    {
        let registry = permit
            .route_registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let current = registry
            .routes
            .get(&permit.session_id)
            .ok_or_else(|| AppError::SessionNotFound(permit.session_id.to_string()))?;
        if permit
            ._response_writer
            .state
            .is_closing(permit.session_id, permit.route_generation)
            .unwrap_or(true)
        {
            return Err(AppError::PtyError("response_close_deferred".to_string()));
        }
        if current.generation != permit.route_generation {
            return Err(AppError::PtyError("stale_pty_route".to_string()));
        }
    }
    Ok(PtyRouteWriteGuard {
        session_id: permit.session_id,
        backend: Arc::clone(&permit.backend),
        _guard: lifecycle_guard,
        _authority_guard: None,
        _settings_guard: None,
    })
}

pub(crate) fn lock_route_for_verified_write<'a>(
    permit: &'a PtyInputPermit,
    expected_kind: SessionBackendKind,
    expected_cwd: &crate::path_identity::VerifiedPathIdentity,
    expected_replica: &crate::path_identity::VerifiedPathIdentity,
) -> Result<PtyRouteWriteGuard<'a>, AppError> {
    let lifecycle_guard = acquire_route_lifecycle(&permit.route_lifecycle)?;
    {
        let registry = permit
            .route_registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let current = registry
            .routes
            .get(&permit.session_id)
            .ok_or_else(|| AppError::SessionNotFound(permit.session_id.to_string()))?;
        let cwd_matches = current
            .canonical_cwd_identity
            .as_ref()
            .is_some_and(|identity| crate::path_identity::same_object(identity, expected_cwd));
        let replica_matches = current
            .verified_replica_anchor
            .as_ref()
            .is_some_and(|identity| crate::path_identity::same_object(identity, expected_replica));
        if permit
            ._response_writer
            .state
            .is_closing(permit.session_id, permit.route_generation)
            .unwrap_or(true)
        {
            return Err(AppError::PtyError("response_close_deferred".to_string()));
        }
        if current.generation != permit.route_generation
            || current.kind != expected_kind
            || !cwd_matches
            || !replica_matches
        {
            return Err(AppError::PtyError("stale_pty_route".to_string()));
        }
    }
    Ok(PtyRouteWriteGuard {
        session_id: permit.session_id,
        backend: Arc::clone(&permit.backend),
        _guard: lifecycle_guard,
        _authority_guard: None,
        _settings_guard: None,
    })
}

pub fn write_with_permit(permit: &PtyInputPermit, bytes: &[u8]) -> Result<(), AppError> {
    lock_route_for_write(permit)?.write(bytes)
}
