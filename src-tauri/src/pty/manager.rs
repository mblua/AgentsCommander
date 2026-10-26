#[cfg(test)]
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
use crate::pty::output_senders::OutputSenderMap;

pub(crate) use crate::pty::output::TerminalOutputAttachError;

pub(crate) use crate::pty::response_close;
mod route_proof;

pub(crate) use response_close::{
    CreateAdmission, CreateClassification, CreateOwnerReservation, ResponseCloseCurrentSettlement,
    ResponseCloseError, ResponseClosePublisher, ResponseCloseSettlementEvidence,
    ResponseCloseSettlementOutcome, ResponseCloseState, ResponseCloseTicket,
    ResponseDeliveryContext, ResponseDeliveryProof, ResponseRouteIdentity, ReturnDeliveryProof,
    ReturnObservationContext, WorkAdmission, WorkDeliveryReservation,
};
pub(crate) use route_proof::lock_route_for_verified_write;
pub use route_proof::{acquire_input_writer, lock_route_for_write, write_with_permit};
use route_proof::{authority_route_proof, snapshot_route_proof};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingSpawn {
    pub cwd: String,
    pub label: String,
}

pub use crate::pty::input_route::{BackendWriteAuthority, PtyInputPermit, PtyRouteWriteGuard};
use crate::pty::input_route::{NonWritingRouteProof, PendingRegistration, RouteRegistry};
pub(crate) use crate::pty::input_route::{PtyAuthorityRouteProof, PtyRouteRemovalError};

pub(crate) struct PtySnapshotRouteProof {
    proof: NonWritingRouteProof,
    backend: Arc<dyn PtyBackend>,
    session_id: Uuid,
}

pub(crate) struct SpawnMark {
    _registration: PendingRegistration,
}

pub struct PtyManager {
    response_close: ResponseCloseState,
    registry: RouteRegistry,
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
        lifecycle_sender: Option<crate::pty::container_lifecycle::ContainerLifecycleSender>,
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
            registry: RouteRegistry::default(),
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
            registry: RouteRegistry::default(),
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
            registry: RouteRegistry::default(),
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
        self.registry.publish(
            id,
            kind,
            canonical_cwd_identity,
            verified_replica_anchor,
            crate::pty::backend::raw_writer(self.backend_for_kind(kind)),
            &self.response_close,
        )
    }

    pub fn remove_route_if_kind(&self, id: Uuid, kind: SessionBackendKind) {
        self.registry
            .remove_route_if_kind(id, kind, &self.response_close);
    }

    pub(crate) fn try_remove_route_if_kind(
        &self,
        id: Uuid,
        kind: SessionBackendKind,
    ) -> Result<(), PtyRouteRemovalError> {
        self.registry
            .try_remove_route_if_kind(id, kind, &self.response_close)
    }

    #[cfg(test)]
    pub(crate) fn poison_route_registry_for_test(&self) {
        self.registry.poison_route_registry_for_test();
    }

    #[cfg(test)]
    pub(crate) fn clear_route_registry_poison_for_test(&self) {
        self.registry.clear_route_registry_poison_for_test();
    }

    fn kind_for_session(&self, id: Uuid) -> Result<SessionBackendKind, AppError> {
        self.registry.kind_for_session(id)
    }

    pub fn backend_kind(&self, id: Uuid) -> Option<SessionBackendKind> {
        self.registry.backend_kind(id)
    }

    pub(crate) fn route_identities(
        &self,
        id: Uuid,
    ) -> Option<(
        Option<crate::path_identity::VerifiedPathIdentity>,
        Option<crate::path_identity::VerifiedPathIdentity>,
        u64,
    )> {
        self.registry.route_identities(id)
    }

    pub(crate) fn has_pending_spawn_for_replica(
        &self,
        replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> bool {
        self.registry.has_pending_spawn_for_replica(replica)
    }

    pub(crate) fn mark_spawning(&self, cwd: &str, label: &str) -> SpawnMark {
        SpawnMark {
            _registration: self
                .registry
                .mark_spawning(cwd, label, &self.response_close),
        }
    }

    pub(crate) fn archive_liveness(&self, ids: &[Uuid]) -> (Vec<PendingSpawn>, Vec<bool>) {
        let (pending, route_kinds) = self.registry.joint_archive_snapshot(ids);
        let pending = pending
            .into_iter()
            .map(|row| PendingSpawn {
                cwd: row.cwd,
                label: row.label,
            })
            .collect();
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
            .backend_kind(id)
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
            .backend_kind(session_id)
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
        let proofs = manager_guard.registry.batch_snapshot_proofs(session_ids)?;
        Ok(session_ids
            .iter()
            .zip(proofs)
            .map(|(id, proof)| {
                proof.map(|proof| PtySnapshotRouteProof {
                    backend: manager_guard.backend_for_kind(proof.backend_kind()),
                    proof,
                    session_id: *id,
                })
            })
            .collect())
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
