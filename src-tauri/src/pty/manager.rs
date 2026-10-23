use std::collections::HashMap;
use std::sync::{Arc, Mutex, TryLockError};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use crate::pty::container_backend::ContainerTransportBackend;
use crate::pty::container_tokens::ContainerApiTokenManager;
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::docker_runtime::DockerRuntime;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::local_backend::LocalProcessBackend;
use crate::pty::output::PtyScreenSnapshot;
use crate::telegram::manager::OutputSenderMap;

pub(crate) use crate::pty::output::TerminalOutputAttachError;

mod response_close;
mod route_proof;

pub(crate) use response_close::{
    CreateAdmission, CreateClassification, CreateOwnerReservation, ResponseCloseCurrentSettlement,
    ResponseCloseError, ResponseClosePublisher, ResponseCloseSettlementEvidence,
    ResponseCloseSettlementOutcome, ResponseCloseState, ResponseCloseTicket,
    ResponseDeliveryContext, ResponseDeliveryProof, ResponseRouteIdentity, ReturnDeliveryProof,
    ReturnObservationContext, WorkAdmission, WorkDeliveryReservation, WriterAdmission,
};
pub(crate) use route_proof::lock_route_for_verified_write;
pub use route_proof::{acquire_input_writer, lock_route_for_write, write_with_permit};
use route_proof::{authority_route_proof, snapshot_route_proof};

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
    _response_writer: WriterAdmission,
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
    _settings_guard: Option<crate::config::settings::SettingsReadPermit>,
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
        settings_guard: crate::config::settings::SettingsReadPermit,
    ) {
        self._settings_guard = Some(settings_guard);
    }
}

pub(crate) struct SpawnMark {
    response_close: ResponseCloseState,
    registry: Arc<Mutex<SpawnRegistry>>,
    seq: u64,
}

impl Drop for SpawnMark {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.pending.remove(&self.seq);
        self.response_close.end_unresolved_spawn();
    }
}

pub struct PtyManager {
    response_close: ResponseCloseState,
    registry: Arc<Mutex<SpawnRegistry>>,
    local_backend: Arc<dyn PtyBackend>,
    container_backend: Arc<ContainerTransportBackend>,
}

/// An owned shallow route selected while PtyManager is locked. All forwarding happens after the
/// caller has dropped that manager lock, so the output path never inherits it.
pub(crate) struct PtyTerminalOutputRoute {
    session_id: Uuid,
    backend: Arc<dyn PtyBackend>,
}

impl PtyTerminalOutputRoute {
    pub(crate) fn activate_terminal_output(
        &self,
        label: &str,
        include_history: bool,
    ) -> Result<Option<PtyScreenSnapshot>, TerminalOutputAttachError> {
        self.backend
            .activate_terminal_output(self.session_id, label, include_history)
    }

    pub(crate) fn detach_terminal_output(&self, label: &str) {
        self.backend.detach_terminal_output(self.session_id, label);
    }
}

impl PtyManager {
    pub(crate) fn bind_response_close(&mut self, state: ResponseCloseState) {
        state.bind_publisher(ResponseClosePublisher::Pty);
        self.response_close = state;
    }
    pub(crate) fn response_close_state(&self) -> ResponseCloseState {
        self.response_close.clone()
    }
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        git_watcher: Arc<GitWatcher>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        lifecycle_sender: Option<crate::session::selection::ContainerLifecycleSender>,
    ) -> Self {
        let local_backend: Arc<dyn PtyBackend> = Arc::new(LocalProcessBackend::new(
            output_senders.clone(),
            idle_detector.clone(),
            git_watcher,
            ws_broadcaster.clone(),
        ));
        let container_backend = Arc::new(ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            ws_broadcaster,
            lifecycle_sender,
            Arc::new(DockerRuntime::new()),
            ContainerApiTokenManager::at_config_dir(),
        ));
        debug_assert!(local_backend.as_any().is::<LocalProcessBackend>());
        debug_assert!(container_backend.as_any().is::<ContainerTransportBackend>());
        Self {
            response_close: ResponseCloseState::default(),
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
            response_close: ResponseCloseState::default(),
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
            response_close: ResponseCloseState::default(),
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

    /// #1363 - a destroyed window releases every terminal-output attachment it held, in both
    /// backends. The window that died is the only thing identified, so a session another window
    /// still watches keeps delivering.
    pub(crate) fn release_window_attachments(&self, label: &str) {
        self.local_backend.release_window_attachments(label);
        self.container_backend.release_window_attachments(label);
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
        if self.response_close.uuid_closing(id) {
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
        self.response_close
            .register_route(id, generation, std::time::Instant::now());
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
                    self.response_close.remove_route(id, generation);
                    registry.routes.remove(&id);
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                log::debug!("[pty] route removal deferred for busy session {id}");
                let registry = Arc::clone(&self.registry);
                let response_close = self.response_close.clone();
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
            self.response_close.remove_route(id, generation);
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
        if registry.next_seq == u64::MAX {
            self.response_close.fail_closed();
        }
        self.response_close.begin_unresolved_spawn();
        registry.next_seq = registry.next_seq.wrapping_add(1);
        registry.pending.insert(
            seq,
            PendingSpawn {
                cwd: cwd.to_string(),
                label: label.to_string(),
            },
        );
        SpawnMark {
            response_close: self.response_close.clone(),
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

    /// #1388 - forwards to the routed backend. An unrouted id yields `true` ("no
    /// claim"), matching the trait default.
    ///
    /// **This deliberately inverts its two nearest siblings**, which answer
    /// conservatively for an unrouted id: `has_session` returns `false` (`:589-594`)
    /// and `context_session_liveness` returns `SessionOver` (`:596-601`). Here
    /// "cannot tell" must not gate, which is the rule `backend.rs:231-235` already
    /// states for `Unavailable`. A permissive answer on a vanished route leads to an
    /// injection that fails loudly with `AppError::SessionNotFound`; `false` would
    /// silently hold a live session to the 90s cap on a routing transient. Fail loud
    /// beats stall silent.
    pub fn has_rendered_visible_content(&self, id: Uuid) -> bool {
        let Ok(kind) = self.kind_for_session(id) else {
            return true;
        };
        self.backend_for_kind(kind).has_rendered_visible_content(id)
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

    /// #2586 - best-effort rows for an observation, not a gate. `Unavailable` and
    /// `SessionOver` both answer `None`: an observer must not branch on why.
    pub fn screen_rows_snapshot(&self, id: Uuid) -> Option<Vec<String>> {
        match self.get_screen_rows(id) {
            ScreenRowsRead::Rows(rows) => Some(rows),
            ScreenRowsRead::Unavailable | ScreenRowsRead::SessionOver => None,
        }
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

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.local_backend.shutdown_terminal_output();
        self.container_backend.shutdown_terminal_output();
    }
}

#[cfg(test)]
mod tests;
