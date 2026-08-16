use std::collections::HashMap;
use std::sync::{Arc, Mutex, TryLockError};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::backend::{
    BackendSpawnSpec, PtyBackend, SessionBackendKind, TerminalScreenCopyRead, TerminalScreenRead,
};
use crate::pty::container_backend::ContainerTransportBackend;
use crate::pty::container_tokens::ContainerApiTokenManager;
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::docker_runtime::DockerRuntime;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::local_backend::LocalProcessBackend;
use crate::pty::output::{PtyScreenSnapshot, TerminalOutputCoordinator};
use crate::telegram::manager::OutputSenderMap;

pub(crate) use crate::pty::output::{
    TerminalOutputActivationResult, TerminalOutputControlState, TerminalRendererMetrics,
    TerminalRendererMetricsWire,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingSpawn {
    pub cwd: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PtyRouteRemovalError {
    Busy,
    LockPoisoned,
}

struct RouteEntry {
    kind: SessionBackendKind,
    generation: u64,
    input_gate: Arc<tokio::sync::Mutex<()>>,
    lifecycle_gate: Arc<std::sync::Mutex<()>>,
    canonical_cwd_identity: Option<crate::path_identity::VerifiedPathIdentity>,
    verified_replica_anchor: Option<crate::path_identity::VerifiedPathIdentity>,
}

#[derive(Default)]
struct SpawnRegistry {
    routes: HashMap<Uuid, RouteEntry>,
    pending: HashMap<u64, PendingSpawn>,
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
    route_registry: Arc<std::sync::Mutex<SpawnRegistry>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
    backend: Arc<dyn PtyBackend>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Generation and lifecycle ownership for final sender-route authorization.
/// It carries no backend or credential and cannot write PTY bytes.
pub(crate) struct PtyAuthorityRouteProof {
    session_id: Uuid,
    route_generation: u64,
    route_registry: Arc<std::sync::Mutex<SpawnRegistry>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
}

/// Non-writing proof for one frozen PTY route generation. It carries no input
/// gate and cannot mint a backend write authority.
pub(crate) struct PtySnapshotRouteProof {
    session_id: Uuid,
    route_generation: u64,
    route_kind: SessionBackendKind,
    route_registry: Arc<std::sync::Mutex<SpawnRegistry>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
    backend: Arc<dyn PtyBackend>,
    saved_cwd: crate::path_identity::VerifiedPathIdentity,
    saved_replica: Option<crate::path_identity::VerifiedPathIdentity>,
}

/// A short-lived route lifecycle guard for one synchronous backend write.
pub struct PtyRouteWriteGuard<'a> {
    session_id: Uuid,
    backend: Arc<dyn PtyBackend>,
    _guard: std::sync::MutexGuard<'a, ()>,
    _authority_guard: Option<std::sync::MutexGuard<'a, ()>>,
    _settings_guard:
        Option<tokio::sync::OwnedRwLockReadGuard<crate::config::settings::AppSettings>>,
}

impl<'a> PtyRouteWriteGuard<'a> {
    pub fn write(&self, bytes: &[u8]) -> Result<(), AppError> {
        let authority = BackendWriteAuthority::for_route_guard();
        self.backend.write(&authority, self.session_id, bytes)
    }

    pub(crate) fn retain_authority_guard(
        &mut self,
        authority_guard: std::sync::MutexGuard<'a, ()>,
    ) {
        self._authority_guard = Some(authority_guard);
    }

    pub(crate) fn retain_settings_guard(
        &mut self,
        settings_guard: tokio::sync::OwnedRwLockReadGuard<crate::config::settings::AppSettings>,
    ) {
        self._settings_guard = Some(settings_guard);
    }
}

pub(crate) struct SpawnMark {
    registry: Arc<Mutex<SpawnRegistry>>,
    seq: u64,
}

impl Drop for SpawnMark {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.pending.remove(&self.seq);
    }
}

pub struct PtyManager {
    registry: Arc<Mutex<SpawnRegistry>>,
    local_backend: Arc<dyn PtyBackend>,
    container_backend: Arc<ContainerTransportBackend>,
}

/// An owned shallow route selected while PtyManager is locked. All forwarding happens after the
/// caller has dropped that manager lock, so the shared output coordinator never inherits it.
pub(crate) struct PtyTerminalOutputRoute {
    session_id: Uuid,
    backend: Arc<dyn PtyBackend>,
}

impl PtyTerminalOutputRoute {
    pub(crate) fn activate_terminal_output(
        &self,
        include_history: bool,
    ) -> TerminalOutputActivationResult {
        self.backend
            .activate_terminal_output(self.session_id, include_history)
    }

    pub(crate) fn ready_terminal_output(
        &self,
        generation: u64,
        snapshot_sequence: u64,
    ) -> TerminalOutputControlState {
        self.backend
            .ready_terminal_output(self.session_id, generation, snapshot_sequence)
    }

    pub(crate) fn deactivate_terminal_output(&self, generation: u64) -> TerminalOutputControlState {
        self.backend
            .deactivate_terminal_output(self.session_id, generation)
    }

    pub(crate) fn ack_terminal_output_delivery(
        &self,
        generation: u64,
        first_sequence: u64,
        sequence: u64,
    ) -> TerminalOutputControlState {
        self.backend.ack_terminal_output_delivery(
            self.session_id,
            generation,
            first_sequence,
            sequence,
        )
    }

    pub(crate) fn report_terminal_renderer_metrics(
        &self,
        generation: u64,
        metrics: TerminalRendererMetrics,
    ) -> TerminalOutputControlState {
        self.backend
            .report_terminal_renderer_metrics(self.session_id, generation, metrics)
    }
}

impl PtyManager {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        git_watcher: Arc<GitWatcher>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        lifecycle_sender: Option<crate::session::selection::ContainerLifecycleSender>,
    ) -> Self {
        let coordinator = TerminalOutputCoordinator::new();
        let local_backend: Arc<dyn PtyBackend> = Arc::new(LocalProcessBackend::with_coordinator(
            output_senders.clone(),
            idle_detector.clone(),
            git_watcher,
            ws_broadcaster.clone(),
            Arc::clone(&coordinator),
        ));
        let container_backend = Arc::new(ContainerTransportBackend::with_runtime_and_coordinator(
            output_senders,
            idle_detector,
            ws_broadcaster,
            lifecycle_sender,
            Arc::new(DockerRuntime::new()),
            ContainerApiTokenManager::at_config_dir(),
            coordinator,
        ));
        debug_assert!(local_backend.as_any().is::<LocalProcessBackend>());
        debug_assert!(container_backend.as_any().is::<ContainerTransportBackend>());
        Self {
            registry: Arc::new(Mutex::new(SpawnRegistry::default())),
            local_backend,
            container_backend,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(local_backend: Arc<dyn PtyBackend>) -> Self {
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        Self {
            registry: Arc::new(Mutex::new(SpawnRegistry::default())),
            local_backend,
            container_backend: Arc::new(ContainerTransportBackend::new(
                output_senders,
                idle_detector,
                None,
                None,
            )),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_container_backend(
        local_backend: Arc<dyn PtyBackend>,
        container_backend: Arc<ContainerTransportBackend>,
    ) -> Self {
        Self {
            registry: Arc::new(Mutex::new(SpawnRegistry::default())),
            local_backend,
            container_backend,
        }
    }

    pub(crate) fn backend_for_kind(&self, kind: SessionBackendKind) -> Arc<dyn PtyBackend> {
        match kind {
            SessionBackendKind::LocalProcess => self.local_backend.clone(),
            SessionBackendKind::ContainerTransport => self.container_backend.clone(),
        }
    }

    pub fn container_backend(&self) -> Arc<ContainerTransportBackend> {
        self.container_backend.clone()
    }

    pub(crate) fn terminal_output_route(
        &self,
        session_id: Uuid,
    ) -> Result<PtyTerminalOutputRoute, AppError> {
        let kind = self.kind_for_session(session_id)?;
        Ok(PtyTerminalOutputRoute {
            session_id,
            backend: self.backend_for_kind(kind),
        })
    }

    pub fn start_container_pending_reaper(&self, shutdown: crate::shutdown::ShutdownSignal) {
        self.container_backend.start_pending_reaper(shutdown);
    }

    pub fn cleanup_container_orphans_on_startup(&self) {
        self.container_backend.cleanup_labeled_orphans_on_startup();
    }

    pub fn stop_all_started_containers_blocking(
        &self,
        budget: std::time::Duration,
    ) -> super::container_backend::ContainerShutdownReport {
        self.container_backend
            .stop_all_started_containers_blocking(budget)
    }

    pub fn record_route(&self, id: Uuid, kind: SessionBackendKind) {
        if let Err(error) = self.try_record_route(id, kind) {
            log::warn!("[pty] route registration rejected session={id} code={error}");
        }
    }

    pub fn try_record_route(&self, id: Uuid, kind: SessionBackendKind) -> Result<(), AppError> {
        self.record_route_with_identities(id, kind, None, None)
    }

    pub(crate) fn record_route_with_identities(
        &self,
        id: Uuid,
        kind: SessionBackendKind,
        canonical_cwd_identity: Option<crate::path_identity::VerifiedPathIdentity>,
        verified_replica_anchor: Option<crate::path_identity::VerifiedPathIdentity>,
    ) -> Result<(), AppError> {
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        if registry.routes.contains_key(&id) {
            return Err(AppError::PtyError("duplicate_pty_route".to_string()));
        }
        let generation = registry
            .next_route_generation
            .checked_add(1)
            .ok_or_else(|| AppError::PtyError("route_generation_overflow".to_string()))?;
        registry.next_route_generation = generation;
        registry.routes.insert(
            id,
            RouteEntry {
                kind,
                generation,
                input_gate: Arc::new(tokio::sync::Mutex::new(())),
                lifecycle_gate: Arc::new(std::sync::Mutex::new(())),
                canonical_cwd_identity,
                verified_replica_anchor,
            },
        );
        Ok(())
    }

    pub fn remove_route_if_kind(&self, id: Uuid, kind: SessionBackendKind) {
        let route = {
            let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
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
                let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
                if registry
                    .routes
                    .get(&id)
                    .is_some_and(|entry| entry.kind == kind && entry.generation == generation)
                {
                    registry.routes.remove(&id);
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                log::debug!("[pty] route removal deferred for busy session {id}");
                let registry = Arc::clone(&self.registry);
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
                        registry.routes.remove(&id);
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
    ) -> Result<(), PtyRouteRemovalError> {
        let route = {
            let registry = match self.registry.try_lock() {
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
        let mut registry = match self.registry.try_lock() {
            Ok(registry) => registry,
            Err(TryLockError::Poisoned(_)) => return Err(PtyRouteRemovalError::LockPoisoned),
            Err(TryLockError::WouldBlock) => return Err(PtyRouteRemovalError::Busy),
        };
        if registry
            .routes
            .get(&id)
            .is_some_and(|entry| entry.kind == kind && entry.generation == generation)
        {
            registry.routes.remove(&id);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn poison_route_registry_for_test(&self) {
        let registry = Arc::clone(&self.registry);
        let result = std::panic::catch_unwind(move || {
            let _registry_guard = registry.lock().unwrap();
            panic!("poison the PTY route registry for deterministic test coverage");
        });
        assert!(result.is_err(), "route-registry poison fixture must panic");
    }

    #[cfg(test)]
    pub(crate) fn clear_route_registry_poison_for_test(&self) {
        self.registry.clear_poison();
    }

    fn kind_for_session(&self, id: Uuid) -> Result<SessionBackendKind, AppError> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&id)
            .map(|entry| entry.kind)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))
    }

    pub fn backend_kind(&self, id: Uuid) -> Option<SessionBackendKind> {
        self.registry
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
        let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
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
        let registry = self
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.pending.values().any(|pending| {
            crate::path_identity::verify_directory(std::path::Path::new(&pending.cwd))
                .is_ok_and(|cwd| crate::path_identity::is_verified_descendant(&cwd, replica))
        })
    }

    pub(crate) fn mark_spawning(&self, cwd: &str, label: &str) -> SpawnMark {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        let seq = registry.next_seq;
        registry.next_seq = registry.next_seq.wrapping_add(1);
        registry.pending.insert(
            seq,
            PendingSpawn {
                cwd: cwd.to_string(),
                label: label.to_string(),
            },
        );
        SpawnMark {
            registry: Arc::clone(&self.registry),
            seq,
        }
    }

    pub(crate) fn archive_liveness(&self, ids: &[Uuid]) -> (Vec<PendingSpawn>, Vec<bool>) {
        let (pending, route_kinds) = {
            let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            (
                registry.pending.values().cloned().collect::<Vec<_>>(),
                ids.iter()
                    .map(|id| registry.routes.get(id).map(|entry| entry.kind))
                    .collect::<Vec<_>>(),
            )
        };
        let live = ids
            .iter()
            .zip(route_kinds)
            .map(|(id, kind)| {
                kind.map(|kind| self.backend_for_kind(kind).has_session(*id))
                    .unwrap_or(false)
            })
            .collect();
        (pending, live)
    }

    pub(crate) async fn spawn(
        manager: &Arc<Mutex<Self>>,
        backend_kind: SessionBackendKind,
        spec: BackendSpawnSpec,
    ) -> Result<(), AppError> {
        let id = spec.id;
        let cwd = spec.cwd.clone();
        let backend = {
            let manager = manager
                .lock()
                .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
            manager.backend_for_kind(backend_kind)
        };
        backend.spawn(spec).await?;
        let cwd_identity = match crate::path_identity::verify_directory(std::path::Path::new(&cwd))
        {
            Ok(identity) => identity,
            Err(_) => {
                if backend.kill(id).is_err() {
                    log::warn!("[pty-route] unsafe route cleanup failed session={id}");
                }
                return Err(AppError::PtyError("unsafe_route_cwd".to_string()));
            }
        };
        let verified_replica_anchor =
            crate::config::teams::verify_pty_input_replica_cwd(std::path::Path::new(&cwd))
                .ok()
                .map(|identity| identity.replica_identity);
        let route_result = match manager.lock() {
            Ok(manager) => manager.record_route_with_identities(
                id,
                backend_kind,
                Some(cwd_identity),
                verified_replica_anchor,
            ),
            Err(_) => Err(AppError::PtyError("pty_manager_poisoned".to_string())),
        };
        if let Err(error) = route_result {
            if backend.kill(id).is_err() {
                log::warn!("[pty-route] failed spawn route cleanup session={id}");
            }
            return Err(error);
        }
        Ok(())
    }

    /// Returns true if this manager holds a backend route whose backend has a
    /// live session handle. A true result guarantees that the same id will not
    /// fail `write` with `AppError::SessionNotFound` because routing is missing.
    pub fn has_session(&self, id: Uuid) -> bool {
        let Ok(kind) = self.kind_for_session(id) else {
            return false;
        };
        self.backend_for_kind(kind).has_session(id)
    }

    pub fn context_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        let Ok(kind) = self.kind_for_session(id) else {
            return ContextSessionLiveness::SessionOver;
        };
        self.backend_for_kind(kind).context_session_liveness(id)
    }

    pub fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        let kind = self.kind_for_session(id)?;
        self.backend_for_kind(kind).resize(id, cols, rows)
    }

    pub fn kill(&self, id: Uuid) -> Result<(), AppError> {
        let kind = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&id)
            .map(|entry| entry.kind)
            .unwrap_or(SessionBackendKind::LocalProcess);
        self.kill_for_kind(id, kind)
    }

    /// Clean a pending create through its manager-record backend kind even when
    /// route registration never completed. Cancellation and shutdown rollback
    /// use this for both local and container-backed sessions.
    pub(crate) fn kill_for_kind(&self, id: Uuid, kind: SessionBackendKind) -> Result<(), AppError> {
        let backend = self.backend_for_kind(kind);
        let result = backend.kill(id);
        if result.is_ok() || !backend.has_session(id) {
            self.remove_route_if_kind(id, kind);
        }
        result
    }

    pub fn terminate_job_for_session(&self, id: Uuid) -> bool {
        let Ok(kind) = self.kind_for_session(id) else {
            return false;
        };
        self.backend_for_kind(kind).terminate_job_for_session(id)
    }

    /// #942 - publish the pre-stop witness for a session AC is about to kill outside the
    /// PTY layer (resource-monitor tree kill). No-op when the session has no route.
    pub fn publish_stop_witness(&self, id: Uuid, source: &str) {
        let Ok(kind) = self.kind_for_session(id) else {
            return;
        };
        self.backend_for_kind(kind).publish_stop_witness(id, source);
    }

    pub fn kill_all_jobs(&self) -> (usize, usize) {
        self.backend_for_kind(SessionBackendKind::LocalProcess)
            .kill_all_jobs()
    }

    pub fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        let kind = self.kind_for_session(id).ok()?;
        self.backend_for_kind(kind).get_screen_snapshot(id)
    }

    pub fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        let kind = self.kind_for_session(id).ok()?;
        self.backend_for_kind(kind).get_pty_size(id)
    }

    /// #1032 - forwards to the routed backend. A missing route is not "we could not read":
    /// every route removal is preceded by parser removal, so the session really is over.
    pub fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        let Ok(kind) = self.kind_for_session(id) else {
            return ScreenRowsRead::SessionOver;
        };
        self.backend_for_kind(kind).get_screen_rows(id)
    }

    /// #1171 - forwards to the routed backend. A missing route is `Gone` for the same reason
    /// it is `SessionOver` above: every route removal is preceded by parser removal, so the
    /// session really is over.
    ///
    /// **The watcher engine does not call this on its tick.** It resolves the backend `Arc`
    /// once at registration through `backend_for_kind` and calls the backend directly, which
    /// is what keeps both this mutex and the `registry` mutex out of a 200 ms loop. This
    /// exists for completeness and for callers that hold no `Arc` of their own.
    pub fn screen_rows_since(
        &self,
        id: Uuid,
        seen: Option<crate::pty::watchers::FrameStamp>,
    ) -> crate::pty::watchers::ScreenRowsSince {
        let Ok(kind) = self.kind_for_session(id) else {
            return crate::pty::watchers::ScreenRowsSince::Gone;
        };
        self.backend_for_kind(kind).screen_rows_since(id, seen)
    }

    pub fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: std::path::PathBuf,
    ) {
        let kind = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&session_id)
            .map(|entry| entry.kind)
            .unwrap_or(SessionBackendKind::LocalProcess);
        self.backend_for_kind(kind)
            .register_response_watcher(session_id, request_id, response_dir);
    }

    pub async fn acquire_input_writer(
        manager: &Arc<std::sync::Mutex<PtyManager>>,
        session_id: Uuid,
    ) -> Result<PtyInputPermit, AppError> {
        acquire_input_writer(manager, session_id).await
    }

    pub(crate) fn authority_route_proof(
        manager: &Arc<std::sync::Mutex<PtyManager>>,
        session_id: Uuid,
    ) -> Result<PtyAuthorityRouteProof, AppError> {
        authority_route_proof(manager, session_id)
    }

    pub(crate) fn snapshot_route_proof(
        manager: &Arc<std::sync::Mutex<PtyManager>>,
        session_id: Uuid,
    ) -> Result<PtySnapshotRouteProof, AppError> {
        snapshot_route_proof(manager, session_id)
    }

    pub(crate) fn snapshot_route_proofs(
        manager: &Arc<std::sync::Mutex<PtyManager>>,
        session_ids: &[Uuid],
    ) -> Result<Vec<Option<PtySnapshotRouteProof>>, AppError> {
        let manager_guard = manager
            .lock()
            .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
        let registry = manager_guard
            .registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let mut proofs = Vec::new();
        proofs
            .try_reserve_exact(session_ids.len())
            .map_err(|_| AppError::PtyError("snapshot_route_capacity".to_string()))?;
        for session_id in session_ids {
            let proof = registry.routes.get(session_id).and_then(|entry| {
                Some(PtySnapshotRouteProof {
                    session_id: *session_id,
                    route_generation: entry.generation,
                    route_kind: entry.kind,
                    route_registry: Arc::clone(&manager_guard.registry),
                    route_lifecycle: Arc::clone(&entry.lifecycle_gate),
                    backend: manager_guard.backend_for_kind(entry.kind),
                    saved_cwd: entry.canonical_cwd_identity.clone()?,
                    saved_replica: entry.verified_replica_anchor.clone(),
                })
            });
            proofs.push(proof);
        }
        Ok(proofs)
    }

    pub fn lock_route_for_write(
        permit: &PtyInputPermit,
    ) -> Result<PtyRouteWriteGuard<'_>, AppError> {
        lock_route_for_write(permit)
    }

    pub(crate) fn lock_route_for_verified_write<'a>(
        permit: &'a PtyInputPermit,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> Result<PtyRouteWriteGuard<'a>, AppError> {
        lock_route_for_verified_write(permit, expected_kind, expected_cwd, expected_replica)
    }

    pub fn write_with_permit(permit: &PtyInputPermit, bytes: &[u8]) -> Result<(), AppError> {
        write_with_permit(permit, bytes)
    }
}

fn snapshot_route_proof(
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

fn authority_route_proof(
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
    let (route_registry, input_gate, route_lifecycle, generation, backend) = {
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
        )
    };

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
    })
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

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.local_backend.shutdown_terminal_output();
        self.container_backend.shutdown_terminal_output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Write(Uuid, Vec<u8>),
        Resize(Uuid, u16, u16),
        Kill(Uuid),
        Has(Uuid),
        Snapshot(Uuid),
        Size(Uuid),
        Watcher(Uuid, String),
        TerminateJob(Uuid),
    }

    #[derive(Default)]
    struct RecordingBackend {
        calls: Mutex<Vec<Call>>,
        live: Mutex<std::collections::HashSet<Uuid>>,
    }

    impl RecordingBackend {
        fn set_live(&self, id: Uuid) {
            self.live.lock().unwrap().insert(id);
        }

        fn calls(&self) -> Vec<Call> {
            let mut calls = self.calls.lock().unwrap();
            std::mem::take(&mut *calls)
        }
    }

    impl PtyBackend for RecordingBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
            Box::pin(async move {
                self.live.lock().unwrap().insert(spec.id);
                Ok(())
            })
        }

        fn write(
            &self,
            _authority: &BackendWriteAuthority,
            id: Uuid,
            data: &[u8],
        ) -> Result<(), AppError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Write(id, data.to_vec()));
            Ok(())
        }

        fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Resize(id, cols, rows));
            Ok(())
        }

        fn kill(&self, id: Uuid) -> Result<(), AppError> {
            self.calls.lock().unwrap().push(Call::Kill(id));
            self.live.lock().unwrap().remove(&id);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.calls.lock().unwrap().push(Call::Has(id));
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
            self.calls.lock().unwrap().push(Call::Snapshot(id));
            None
        }

        fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
            self.calls.lock().unwrap().push(Call::Size(id));
            Some((30, 120))
        }

        fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
            ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            session_id: Uuid,
            request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Watcher(session_id, request_id));
        }

        fn terminate_job_for_session(&self, id: Uuid) -> bool {
            self.calls.lock().unwrap().push(Call::TerminateJob(id));
            true
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (1, 2)
        }
    }

    struct DelayedSpawnBackend {
        live: Mutex<std::collections::HashSet<Uuid>>,
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl DelayedSpawnBackend {
        fn new(
            started: tokio::sync::oneshot::Sender<()>,
            release: tokio::sync::oneshot::Receiver<()>,
        ) -> Self {
            Self {
                live: Mutex::new(std::collections::HashSet::new()),
                started: Mutex::new(Some(started)),
                release: Mutex::new(Some(release)),
            }
        }
    }

    impl PtyBackend for DelayedSpawnBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
            let started = self.started.lock().unwrap().take().expect("started sender");
            let release = self
                .release
                .lock()
                .unwrap()
                .take()
                .expect("release receiver");
            Box::pin(async move {
                let _ = started.send(());
                let _ = release.await;
                self.live.lock().unwrap().insert(spec.id);
                Ok(())
            })
        }

        fn write(
            &self,
            _authority: &BackendWriteAuthority,
            _id: Uuid,
            _data: &[u8],
        ) -> Result<(), AppError> {
            Ok(())
        }

        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
            Ok(())
        }

        fn kill(&self, id: Uuid) -> Result<(), AppError> {
            self.live.lock().unwrap().remove(&id);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
            ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    fn test_spawn_spec(id: Uuid) -> BackendSpawnSpec {
        BackendSpawnSpec {
            id,
            agent_id: None,
            coding_agent: None,
            cmd: "cmd".to_string(),
            args: Vec::new(),
            resolved_agent_host_shell: None,
            cwd: ".".to_string(),
            selected_cwd: None,
            cols: 80,
            rows: 24,
            container_image: None,
            configured_env: Vec::new(),
            env_remove_keys: Vec::new(),
            env_unset: Vec::new(),
            extra_env: Vec::new(),
            idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
            output_target: crate::pty::output::PtyOutputTarget::noop(),
            resource_registration: None,
            logical_resource_slot: None,
            container_credential: None,
            container_repo_mounts: Vec::new(),
        }
    }

    #[tokio::test]
    async fn facade_delegates_local_route_by_session_id() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        backend.set_live(id);
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, SessionBackendKind::LocalProcess)
            .unwrap();

        let permit = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        PtyManager::write_with_permit(&permit, b"abc").expect("write");
        drop(permit);
        manager.lock().unwrap().resize(id, 120, 30).expect("resize");
        assert!(manager.lock().unwrap().has_session(id));
        assert_eq!(manager.lock().unwrap().get_pty_size(id), Some((30, 120)));
        assert!(manager.lock().unwrap().terminate_job_for_session(id));
        manager.lock().unwrap().register_response_watcher(
            id,
            "r1".to_string(),
            std::path::PathBuf::from("x"),
        );
        manager.lock().unwrap().kill(id).expect("kill");

        assert_eq!(
            backend.calls(),
            vec![
                Call::Write(id, b"abc".to_vec()),
                Call::Resize(id, 120, 30),
                Call::Has(id),
                Call::Size(id),
                Call::TerminateJob(id),
                Call::Watcher(id, "r1".to_string()),
                Call::Kill(id),
            ]
        );
        assert!(!manager.lock().unwrap().has_session(id));
    }

    #[tokio::test]
    async fn facade_write_without_route_returns_session_not_found() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));

        let err = match PtyManager::acquire_input_writer(&manager, id).await {
            Ok(_) => panic!("missing route unexpectedly acquired"),
            Err(error) => error,
        };

        assert!(matches!(err, AppError::SessionNotFound(_)));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn context_liveness_uses_backend_default_and_missing_route_is_over() {
        let live_id = Uuid::new_v4();
        let missing_id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        backend.set_live(live_id);
        let manager = PtyManager::new_for_test(backend);
        manager.record_route(live_id, SessionBackendKind::LocalProcess);

        assert_eq!(
            manager.context_session_liveness(live_id),
            ContextSessionLiveness::Live
        );
        assert_eq!(
            manager.context_session_liveness(missing_id),
            ContextSessionLiveness::SessionOver
        );
    }

    /// #1171, 9.1.9 - a session with no route is `Gone`, not `Missing`.
    ///
    /// Same reading `get_screen_rows` already gives a routeless id (`:573-580`): every route
    /// removal is preceded by parser removal, so the session really is over. The engine
    /// retires on `Gone`, which is exactly what should happen here; `Missing` would keep it
    /// sampling an id that can never come back.
    #[test]
    fn screen_rows_since_reports_gone_for_a_session_with_no_route() {
        let manager = PtyManager::new_for_test(Arc::new(RecordingBackend::default()));

        assert!(matches!(
            manager.screen_rows_since(Uuid::new_v4(), None),
            crate::pty::watchers::ScreenRowsSince::Gone
        ));
    }

    /// #1171, 9.1.8 - a backend that never heard of the seam keeps working through the trait
    /// default: it reports a frame with NO stamp, and it never reports `Unchanged`, whatever
    /// stamp it is handed. That is the property that lets `stamp` be an `Option` instead of a
    /// fabricated value, and it is what keeps the two `PtyBackend` test fakes compiling.
    #[test]
    fn the_defaulted_seam_reports_no_stamp_and_never_reports_unchanged() {
        use crate::pty::watchers::{FrameStamp, ScreenRowsSince};

        struct DefaultSeamBackend;

        impl PtyBackend for DefaultSeamBackend {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn spawn(
                &self,
                _spec: BackendSpawnSpec,
            ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
                Box::pin(async { Ok(()) })
            }
            fn write(
                &self,
                _authority: &BackendWriteAuthority,
                _id: Uuid,
                _data: &[u8],
            ) -> Result<(), AppError> {
                Ok(())
            }
            fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
                Ok(())
            }
            fn kill(&self, _id: Uuid) -> Result<(), AppError> {
                Ok(())
            }
            fn has_session(&self, _id: Uuid) -> bool {
                true
            }
            fn get_screen_snapshot(&self, _id: Uuid) -> Option<PtyScreenSnapshot> {
                None
            }
            fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
                None
            }
            fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
                ScreenRowsRead::Rows(vec!["only row".to_string()])
            }
            fn register_response_watcher(
                &self,
                _session_id: Uuid,
                _request_id: String,
                _response_dir: std::path::PathBuf,
            ) {
            }
            fn terminate_job_for_session(&self, _id: Uuid) -> bool {
                false
            }
            fn kill_all_jobs(&self) -> (usize, usize) {
                (0, 0)
            }
        }

        let id = Uuid::new_v4();
        let manager = PtyManager::new_for_test(Arc::new(DefaultSeamBackend));
        manager.record_route(id, SessionBackendKind::LocalProcess);

        let first = manager.screen_rows_since(id, None);
        let frame = first.frame().expect("the default must produce a frame");
        assert!(frame.stamp.is_none());
        assert_eq!(frame.rows, vec!["only row".to_string()]);
        assert_eq!(frame.wrapped, vec![false]);
        assert_eq!(frame.cursor_row, 0);

        // Handed a stamp that would match anything, it still refuses to claim "unchanged":
        // it has no sequence of its own to compare against.
        let seen = FrameStamp {
            sequence: 0,
            rows: 1,
            cols: 1,
        };
        assert!(matches!(
            manager.screen_rows_since(id, Some(seen)),
            ScreenRowsSince::Frame(_)
        ));
    }

    #[test]
    fn archive_liveness_reports_pending_spawn_until_mark_drops() {
        let backend = Arc::new(RecordingBackend::default());
        let manager = PtyManager::new_for_test(backend);
        let mark = manager.mark_spawning("C:/repo/.ac/wg-1/__agent_dev", "dev");

        let (pending, live) = manager.archive_liveness(&[]);

        assert_eq!(
            pending,
            vec![PendingSpawn {
                cwd: "C:/repo/.ac/wg-1/__agent_dev".to_string(),
                label: "dev".to_string(),
            }]
        );
        assert!(live.is_empty());
        drop(mark);

        let (pending, _) = manager.archive_liveness(&[]);
        assert!(pending.is_empty());
    }

    #[test]
    fn archive_liveness_reports_backend_live_route() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        backend.set_live(id);
        let manager = PtyManager::new_for_test(backend.clone());
        manager.record_route(id, SessionBackendKind::LocalProcess);

        let (pending, live) = manager.archive_liveness(&[id]);

        assert!(pending.is_empty());
        assert_eq!(live, vec![true]);
        assert_eq!(backend.calls(), vec![Call::Has(id)]);
    }

    #[tokio::test]
    async fn input_permits_serialize_writers_for_one_session() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        backend.set_live(id);
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, SessionBackendKind::LocalProcess)
            .unwrap();
        let first = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        let waiting_manager = Arc::clone(&manager);
        let waiter = tokio::spawn(async move {
            let permit = PtyManager::acquire_input_writer(&waiting_manager, id)
                .await
                .unwrap();
            PtyManager::write_with_permit(&permit, b"second").unwrap();
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        PtyManager::write_with_permit(&first, b"first").unwrap();
        drop(first);
        waiter.await.unwrap();
        assert_eq!(
            backend.calls(),
            vec![
                Call::Write(id, b"first".to_vec()),
                Call::Write(id, b"second".to_vec()),
            ]
        );
    }

    #[tokio::test]
    async fn different_sessions_have_independent_input_gates() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
        {
            let manager = manager.lock().unwrap();
            manager
                .try_record_route(first_id, SessionBackendKind::LocalProcess)
                .unwrap();
            manager
                .try_record_route(second_id, SessionBackendKind::LocalProcess)
                .unwrap();
        }
        let _first = PtyManager::acquire_input_writer(&manager, first_id)
            .await
            .unwrap();
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            PtyManager::acquire_input_writer(&manager, second_id),
        )
        .await
        .expect("another session must not wait on the first input gate")
        .unwrap();
        drop(second);
    }

    #[tokio::test]
    async fn removed_and_recreated_route_invalidates_an_old_permit() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, SessionBackendKind::LocalProcess)
            .unwrap();
        let old = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        {
            let manager = manager.lock().unwrap();
            manager.remove_route_if_kind(id, SessionBackendKind::LocalProcess);
            manager
                .try_record_route(id, SessionBackendKind::LocalProcess)
                .unwrap();
        }
        assert!(PtyManager::write_with_permit(&old, b"stale").is_err());
        drop(old);
        let current = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        PtyManager::write_with_permit(&current, b"current").unwrap();
        assert_eq!(backend.calls(), vec![Call::Write(id, b"current".to_vec())]);
    }

    #[test]
    fn duplicate_route_and_generation_overflow_fail_without_replacement() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = PtyManager::new_for_test(backend);
        manager
            .try_record_route(first_id, SessionBackendKind::LocalProcess)
            .unwrap();
        assert!(manager
            .try_record_route(first_id, SessionBackendKind::ContainerTransport)
            .is_err());
        assert_eq!(
            manager.backend_kind(first_id),
            Some(SessionBackendKind::LocalProcess)
        );
        manager.registry.lock().unwrap().next_route_generation = u64::MAX;
        assert!(manager
            .try_record_route(second_id, SessionBackendKind::LocalProcess)
            .is_err());
        assert_eq!(manager.backend_kind(second_id), None);
    }

    #[tokio::test]
    async fn same_spelling_directory_replacement_fails_verified_route_lock() {
        let id = Uuid::new_v4();
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("replica");
        let retired = temp.path().join("retired");
        std::fs::create_dir(&path).unwrap();
        let original = crate::path_identity::verify_directory(&path).unwrap();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
        manager
            .lock()
            .unwrap()
            .record_route_with_identities(
                id,
                SessionBackendKind::LocalProcess,
                Some(original.clone()),
                Some(original),
            )
            .unwrap();
        let permit = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        std::fs::rename(&path, &retired).unwrap();
        std::fs::create_dir(&path).unwrap();
        let replacement = crate::path_identity::verify_directory(&path).unwrap();
        assert!(PtyManager::lock_route_for_verified_write(
            &permit,
            SessionBackendKind::LocalProcess,
            &replacement,
            &replacement,
        )
        .is_err());
    }

    #[tokio::test]
    async fn verified_write_guard_retains_authority_route_through_first_write() {
        let authority_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let temp = tempfile::TempDir::new().unwrap();
        let authority_path = temp.path().join("authority");
        let target_path = temp.path().join("target");
        std::fs::create_dir(&authority_path).unwrap();
        std::fs::create_dir(&target_path).unwrap();
        let authority_identity = crate::path_identity::verify_directory(&authority_path).unwrap();
        let target_identity = crate::path_identity::verify_directory(&target_path).unwrap();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
        {
            let manager = manager.lock().unwrap();
            manager
                .record_route_with_identities(
                    authority_id,
                    SessionBackendKind::LocalProcess,
                    Some(authority_identity.clone()),
                    Some(authority_identity.clone()),
                )
                .unwrap();
            manager
                .record_route_with_identities(
                    target_id,
                    SessionBackendKind::LocalProcess,
                    Some(target_identity.clone()),
                    Some(target_identity.clone()),
                )
                .unwrap();
        }
        let authority = PtyManager::authority_route_proof(&manager, authority_id).unwrap();
        let permit = PtyManager::acquire_input_writer(&manager, target_id)
            .await
            .unwrap();
        let authority_guard = authority
            .lock_verified(
                SessionBackendKind::LocalProcess,
                &authority_identity,
                Some(&authority_identity),
            )
            .unwrap();
        let mut write_guard = PtyManager::lock_route_for_verified_write(
            &permit,
            SessionBackendKind::LocalProcess,
            &target_identity,
            &target_identity,
        )
        .unwrap();
        write_guard.retain_authority_guard(authority_guard);

        assert_eq!(
            manager
                .lock()
                .unwrap()
                .try_remove_route_if_kind(authority_id, SessionBackendKind::LocalProcess),
            Err(PtyRouteRemovalError::Busy)
        );
        drop(write_guard);
        assert_eq!(
            manager
                .lock()
                .unwrap()
                .try_remove_route_if_kind(authority_id, SessionBackendKind::LocalProcess),
            Ok(())
        );
    }

    #[tokio::test]
    async fn post_spawn_manager_poison_kills_the_unroutable_backend_session() {
        let id = Uuid::new_v4();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let backend = Arc::new(DelayedSpawnBackend::new(started_tx, release_rx));
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let spawn_task = {
            let manager = manager.clone();
            tokio::spawn(async move {
                PtyManager::spawn(
                    &manager,
                    SessionBackendKind::LocalProcess,
                    test_spawn_spec(id),
                )
                .await
            })
        };

        started_rx.await.expect("spawn started");
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = manager.lock().unwrap();
            panic!("poison manager between backend spawn and route publication");
        }));
        assert!(poisoned.is_err());
        release_tx.send(()).expect("release spawn");
        assert!(spawn_task.await.unwrap().is_err());
        assert!(!backend.has_session(id));
    }

    #[tokio::test]
    async fn spawn_does_not_hold_manager_mutex_while_backend_awaits() {
        let id = Uuid::new_v4();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let backend = Arc::new(DelayedSpawnBackend::new(started_tx, release_rx));
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));

        let spawn_task = {
            let manager = manager.clone();
            tokio::spawn(async move {
                PtyManager::spawn(
                    &manager,
                    SessionBackendKind::LocalProcess,
                    test_spawn_spec(id),
                )
                .await
            })
        };

        started_rx.await.expect("spawn started");
        {
            let guard = manager
                .try_lock()
                .expect("manager mutex should be free while backend spawn awaits");
            assert!(!guard.has_session(id));
        }

        release_tx.send(()).expect("release spawn");
        spawn_task.await.expect("spawn task").expect("spawn ok");
        assert!(manager.lock().unwrap().has_session(id));
    }
}
