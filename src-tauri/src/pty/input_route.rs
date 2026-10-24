use crate::errors::AppError;
use crate::pty::backend_kind::SessionBackendKind;
use crate::pty::response_close::{
    ResponseCloseError, ResponseCloseState, ResponseCloseTicket, WriterAdmission,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, TryLockError};
use uuid::Uuid;

pub(crate) trait RawPtyWriter: Send + Sync {
    fn write(
        &self,
        authority: &BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), AppError>;
}

#[derive(Clone)]
pub(crate) struct PendingRow {
    pub(crate) cwd: String,
    pub(crate) label: String,
}

#[derive(Clone, Default)]
pub(crate) struct RouteRegistry {
    state: Arc<Mutex<RegistryState>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PtyRouteRemovalError {
    Busy,
    LockPoisoned,
}

struct RouteEntry {
    kind: SessionBackendKind,
    writer: Arc<dyn RawPtyWriter>,
    generation: u64,
    input_gate: Arc<tokio::sync::Mutex<()>>,
    lifecycle_gate: Arc<std::sync::Mutex<()>>,
    canonical_cwd_identity: Option<crate::path_identity::VerifiedPathIdentity>,
    verified_replica_anchor: Option<crate::path_identity::VerifiedPathIdentity>,
}

#[derive(Default)]
struct RegistryState {
    routes: HashMap<Uuid, RouteEntry>,
    pending: HashMap<u64, PendingRow>,
    #[cfg(test)]
    removal_completed: Option<std::sync::mpsc::Sender<()>>,
    next_seq: u64,
    next_route_generation: u64,
}

/// Unforgeable safe-code authority for a raw backend PTY write. Only the
/// route-guard chokepoint can issue a production value, so adding an alias or
/// reformatting a call cannot bypass the permit inventory.
#[doc(hidden)]
pub struct BackendWriteAuthority {
    _private: (),
}

impl BackendWriteAuthority {
    fn for_route_guard() -> Self {
        Self { _private: () }
    }

    #[cfg(test)]
    pub(crate) fn for_backend_test() -> Self {
        Self { _private: () }
    }
}

/// Exclusive per-session input ownership. Holding this guard serializes a
/// complete multi-phase submission against every other production writer.
pub struct PtyInputPermit {
    session_id: Uuid,
    route_generation: u64,
    route_registry: Arc<std::sync::Mutex<RegistryState>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
    writer: Arc<dyn RawPtyWriter>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
    _response_writer: WriterAdmission,
}

/// Generation and lifecycle ownership for final sender-route authorization.
/// It carries no backend or credential and cannot write PTY bytes.
pub(crate) struct PtyAuthorityRouteProof {
    session_id: Uuid,
    route_generation: u64,
    route_registry: Arc<std::sync::Mutex<RegistryState>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
}

/// Non-writing proof for one frozen PTY route generation. It carries no input
/// gate and cannot mint a backend write authority.
pub(crate) struct NonWritingRouteProof {
    session_id: Uuid,
    route_generation: u64,
    route_kind: SessionBackendKind,
    route_registry: Arc<std::sync::Mutex<RegistryState>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
    saved_cwd: crate::path_identity::VerifiedPathIdentity,
    saved_replica: Option<crate::path_identity::VerifiedPathIdentity>,
}

/// A short-lived route lifecycle guard for one synchronous backend write.
pub struct PtyRouteWriteGuard<'a> {
    session_id: Uuid,
    writer: Arc<dyn RawPtyWriter>,
    _guard: std::sync::MutexGuard<'a, ()>,
    _authority_guard: Option<std::sync::MutexGuard<'a, ()>>,
    _settings_guard: Option<crate::config::settings::SettingsReadPermit>,
}

impl<'a> PtyRouteWriteGuard<'a> {
    pub fn write(&self, bytes: &[u8]) -> Result<(), AppError> {
        let authority = BackendWriteAuthority::for_route_guard();
        self.writer.write(&authority, self.session_id, bytes)
    }

    pub(crate) fn retain_authority_guard(
        &mut self,
        authority_guard: std::sync::MutexGuard<'a, ()>,
    ) {
        self._authority_guard = Some(authority_guard);
    }

    pub(crate) fn retain_settings_guard(
        &mut self,
        settings_guard: crate::config::settings::SettingsReadPermit,
    ) {
        self._settings_guard = Some(settings_guard);
    }
}

pub(crate) struct PendingRegistration {
    response_close: ResponseCloseState,
    registry: Arc<Mutex<RegistryState>>,
    seq: u64,
}

impl Drop for PendingRegistration {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.pending.remove(&self.seq);
        self.response_close.end_unresolved_spawn();
    }
}

pub(crate) struct InputPreparation {
    session_id: Uuid,
    route_registry: Arc<Mutex<RegistryState>>,
    input_gate: Arc<tokio::sync::Mutex<()>>,
    route_lifecycle: Arc<Mutex<()>>,
    generation: u64,
    writer: Arc<dyn RawPtyWriter>,
    response_close: ResponseCloseState,
}

impl InputPreparation {
    pub(crate) async fn acquire(self) -> Result<PtyInputPermit, AppError> {
        let Self {
            session_id,
            route_registry,
            input_gate,
            route_lifecycle,
            generation,
            writer,
            response_close,
        } = self;
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
            writer,
            _guard: guard,
            _response_writer: response_writer,
        })
    }
}

impl RouteRegistry {
    pub(crate) fn publish(
        &self,
        id: Uuid,
        kind: SessionBackendKind,
        canonical_cwd_identity: Option<crate::path_identity::VerifiedPathIdentity>,
        verified_replica_anchor: Option<crate::path_identity::VerifiedPathIdentity>,
        writer: Arc<dyn RawPtyWriter>,
        response_close: &ResponseCloseState,
    ) -> Result<(), AppError> {
        let mut registry = self
            .state
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        if response_close.uuid_closing(id) {
            return Err(AppError::PtyError("response_close_deferred".to_string()));
        }
        if registry.routes.contains_key(&id) {
            return Err(AppError::PtyError("duplicate_pty_route".to_string()));
        }
        let generation = registry
            .next_route_generation
            .checked_add(1)
            .ok_or_else(|| AppError::PtyError("route_generation_overflow".to_string()))?;
        registry.next_route_generation = generation;
        response_close.register_route(id, generation, std::time::Instant::now());
        registry.routes.insert(
            id,
            RouteEntry {
                kind,
                writer,
                generation,
                input_gate: Arc::new(tokio::sync::Mutex::new(())),
                lifecycle_gate: Arc::new(std::sync::Mutex::new(())),
                canonical_cwd_identity,
                verified_replica_anchor,
            },
        );
        Ok(())
    }

    pub(crate) fn remove_route_if_kind(
        &self,
        id: Uuid,
        kind: SessionBackendKind,
        response_close: &ResponseCloseState,
    ) {
        let route = {
            let registry = self.state.lock().unwrap_or_else(|e| e.into_inner());
            registry
                .routes
                .get(&id)
                .filter(|entry| entry.kind == kind)
                .map(|entry| (Arc::clone(&entry.lifecycle_gate), entry.generation))
        };
        let Some((lifecycle, generation)) = route else {
            return;
        };
        let deferred_lifecycle = Arc::clone(&lifecycle);
        match lifecycle.try_lock() {
            Ok(_lifecycle_guard) => {
                let mut registry = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if registry
                    .routes
                    .get(&id)
                    .is_some_and(|entry| entry.kind == kind && entry.generation == generation)
                {
                    response_close.remove_route(id, generation);
                    registry.routes.remove(&id);
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                log::debug!("[pty] route removal deferred for busy session {id}");
                let registry = Arc::clone(&self.state);
                let response_close = response_close.clone();
                std::thread::spawn(move || {
                    let _lifecycle_guard = deferred_lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
                    if registry
                        .routes
                        .get(&id)
                        .is_some_and(|entry| entry.kind == kind && entry.generation == generation)
                    {
                        response_close.remove_route(id, generation);
                        registry.routes.remove(&id);
                    }
                    #[cfg(test)]
                    if let Some(done) = &registry.removal_completed {
                        let _ = done.send(());
                    }
                });
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                log::warn!("[pty] route removal failed session={id} code=route_lifecycle_poisoned");
            }
        };
    }

    pub(crate) fn try_remove_route_if_kind(
        &self,
        id: Uuid,
        kind: SessionBackendKind,
        response_close: &ResponseCloseState,
    ) -> Result<(), PtyRouteRemovalError> {
        let route = {
            let registry = match self.state.try_lock() {
                Ok(registry) => registry,
                Err(TryLockError::Poisoned(_)) => return Err(PtyRouteRemovalError::LockPoisoned),
                Err(TryLockError::WouldBlock) => return Err(PtyRouteRemovalError::Busy),
            };
            registry
                .routes
                .get(&id)
                .filter(|entry| entry.kind == kind)
                .map(|entry| (Arc::clone(&entry.lifecycle_gate), entry.generation))
        };
        let Some((lifecycle, generation)) = route else {
            return Ok(());
        };
        let _lifecycle_guard = lifecycle
            .try_lock()
            .map_err(|_| PtyRouteRemovalError::Busy)?;
        let mut registry = match self.state.try_lock() {
            Ok(registry) => registry,
            Err(TryLockError::Poisoned(_)) => return Err(PtyRouteRemovalError::LockPoisoned),
            Err(TryLockError::WouldBlock) => return Err(PtyRouteRemovalError::Busy),
        };
        if registry
            .routes
            .get(&id)
            .is_some_and(|entry| entry.kind == kind && entry.generation == generation)
        {
            response_close.remove_route(id, generation);
            registry.routes.remove(&id);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_generation_for_test(&self, generation: u64) {
        self.state.lock().unwrap().next_route_generation = generation;
    }

    #[cfg(test)]
    pub(crate) fn poison_route_registry_for_test(&self) {
        let registry = Arc::clone(&self.state);
        let result = std::panic::catch_unwind(move || {
            let _registry_guard = registry.lock().unwrap();
            panic!("poison the PTY route registry for deterministic test coverage");
        });
        assert!(result.is_err(), "route-registry poison fixture must panic");
    }

    #[cfg(test)]
    pub(crate) fn clear_route_registry_poison_for_test(&self) {
        self.state.clear_poison();
    }

    pub(crate) fn kind_for_session(&self, id: Uuid) -> Result<SessionBackendKind, AppError> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&id)
            .map(|entry| entry.kind)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))
    }

    pub(crate) fn backend_kind(&self, id: Uuid) -> Option<SessionBackendKind> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&id)
            .map(|entry| entry.kind)
    }

    pub(crate) fn route_identities(
        &self,
        id: Uuid,
    ) -> Option<(
        Option<crate::path_identity::VerifiedPathIdentity>,
        Option<crate::path_identity::VerifiedPathIdentity>,
        u64,
    )> {
        let registry = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = registry.routes.get(&id)?;
        Some((
            entry.canonical_cwd_identity.clone(),
            entry.verified_replica_anchor.clone(),
            entry.generation,
        ))
    }

    pub(crate) fn has_pending_spawn_for_replica(
        &self,
        replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> bool {
        let registry = self.state.lock().unwrap_or_else(|error| error.into_inner());
        registry.pending.values().any(|pending| {
            crate::path_identity::verify_directory(std::path::Path::new(&pending.cwd))
                .is_ok_and(|cwd| crate::path_identity::is_verified_descendant(&cwd, replica))
        })
    }

    pub(crate) fn mark_spawning(
        &self,
        cwd: &str,
        label: &str,
        response_close: &ResponseCloseState,
    ) -> PendingRegistration {
        let mut registry = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let seq = registry.next_seq;
        if registry.next_seq == u64::MAX {
            response_close.fail_closed();
        }
        response_close.begin_unresolved_spawn();
        registry.next_seq = registry.next_seq.wrapping_add(1);
        registry.pending.insert(
            seq,
            PendingRow {
                cwd: cwd.to_string(),
                label: label.to_string(),
            },
        );
        PendingRegistration {
            response_close: response_close.clone(),
            registry: Arc::clone(&self.state),
            seq,
        }
    }

    pub(crate) fn joint_archive_snapshot(
        &self,
        ids: &[Uuid],
    ) -> (Vec<PendingRow>, Vec<Option<SessionBackendKind>>) {
        let (pending, route_kinds) = {
            let registry = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                registry.pending.values().cloned().collect::<Vec<_>>(),
                ids.iter()
                    .map(|id| registry.routes.get(id).map(|entry| entry.kind))
                    .collect::<Vec<_>>(),
            )
        };
        (pending, route_kinds)
    }

    pub(crate) fn prepare(
        &self,
        session_id: Uuid,
        response_close: &ResponseCloseState,
    ) -> Result<InputPreparation, AppError> {
        let registry = self
            .state
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let entry = registry
            .routes
            .get(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        Ok(InputPreparation {
            session_id,
            route_registry: Arc::clone(&self.state),
            input_gate: Arc::clone(&entry.input_gate),
            route_lifecycle: Arc::clone(&entry.lifecycle_gate),
            generation: entry.generation,
            writer: Arc::clone(&entry.writer),
            response_close: response_close.clone(),
        })
    }

    pub(crate) fn snapshot_route_proof(
        &self,
        session_ids: &[Uuid],
        require_route: bool,
    ) -> Result<Vec<Option<NonWritingRouteProof>>, AppError> {
        let registry = self
            .state
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let mut proofs = Vec::new();
        proofs
            .try_reserve_exact(session_ids.len())
            .map_err(|_| AppError::PtyError("snapshot_route_capacity".to_string()))?;
        for session_id in session_ids {
            let entry = registry.routes.get(session_id);
            if require_route {
                let entry =
                    entry.ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
                if entry.canonical_cwd_identity.is_none() {
                    return Err(AppError::PtyError("unsafe_route_cwd".to_string()));
                }
            }
            proofs.push(entry.and_then(|entry| {
                Some(NonWritingRouteProof {
                    session_id: *session_id,
                    route_generation: entry.generation,
                    route_kind: entry.kind,
                    route_registry: Arc::clone(&self.state),
                    route_lifecycle: Arc::clone(&entry.lifecycle_gate),
                    saved_cwd: entry.canonical_cwd_identity.clone()?,
                    saved_replica: entry.verified_replica_anchor.clone(),
                })
            }));
        }
        Ok(proofs)
    }

    pub(crate) fn batch_snapshot_proofs(
        &self,
        session_ids: &[Uuid],
    ) -> Result<Vec<Option<NonWritingRouteProof>>, AppError> {
        self.snapshot_route_proof(session_ids, false)
    }

    pub(crate) fn authority_route_proof(
        &self,
        session_id: Uuid,
    ) -> Result<PtyAuthorityRouteProof, AppError> {
        let registry = self
            .state
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let entry = registry
            .routes
            .get(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        Ok(PtyAuthorityRouteProof {
            session_id,
            route_generation: entry.generation,
            route_registry: Arc::clone(&self.state),
            route_lifecycle: Arc::clone(&entry.lifecycle_gate),
        })
    }
}

fn acquire_route_lifecycle(
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
        let (session_id, generation, backend) = ticket.route_identity();
        if self.session_id != session_id || self.route_generation != generation {
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
        if !registry
            .routes
            .get(&self.session_id)
            .is_some_and(|route| route.generation == self.route_generation && route.kind == backend)
        {
            return Err(ResponseCloseError::Unknown);
        }
        state.try_commit_response_close(ticket, now)
    }
}

impl NonWritingRouteProof {
    pub(crate) fn backend_kind(&self) -> SessionBackendKind {
        self.route_kind
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

    pub(crate) fn lock_verified<'a>(
        &'a self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> Result<std::sync::MutexGuard<'a, ()>, AppError> {
        let lifecycle_guard = acquire_route_lifecycle(&self.route_lifecycle)?;
        {
            let registry = match self.route_registry.lock() {
                Ok(registry) => registry,
                Err(_) => return Err(AppError::PtyError("route_registry_poisoned".into())),
            };
            let Some(current) = registry.routes.get(&self.session_id) else {
                return Err(AppError::PtyError("stale_pty_route".into()));
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
                return Err(AppError::PtyError("stale_pty_route".into()));
            }
        }
        Ok(lifecycle_guard)
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

pub(crate) fn lock_route_for_write(
    permit: &PtyInputPermit,
) -> Result<PtyRouteWriteGuard<'_>, AppError> {
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
        if permit._response_writer.is_closing().unwrap_or(true) {
            return Err(AppError::PtyError("response_close_deferred".to_string()));
        }
        if current.generation != permit.route_generation {
            return Err(AppError::PtyError("stale_pty_route".to_string()));
        }
    }
    Ok(PtyRouteWriteGuard {
        session_id: permit.session_id,
        writer: Arc::clone(&permit.writer),
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
        if permit._response_writer.is_closing().unwrap_or(true) {
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
        writer: Arc::clone(&permit.writer),
        _guard: lifecycle_guard,
        _authority_guard: None,
        _settings_guard: None,
    })
}

pub(crate) fn write_with_permit(permit: &PtyInputPermit, bytes: &[u8]) -> Result<(), AppError> {
    lock_route_for_write(permit)?.write(bytes)
}

#[cfg(test)]
mod tests;
