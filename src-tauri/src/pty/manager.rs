use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use crate::pty::container_backend::ContainerTransportBackend;
use crate::pty::container_tokens::ContainerApiTokenManager;
use crate::pty::docker_runtime::DockerRuntime;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::local_backend::LocalProcessBackend;
use crate::pty::output::PtyScreenSnapshot;
use crate::telegram::manager::OutputSenderMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingSpawn {
    pub cwd: String,
    pub label: String,
}

#[derive(Default)]
struct SpawnRegistry {
    routes: HashMap<Uuid, SessionBackendKind>,
    pending: HashMap<u64, PendingSpawn>,
    next_seq: u64,
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

impl PtyManager {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        git_watcher: Arc<GitWatcher>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        lifecycle_sender: Option<crate::session::selection::ContainerLifecycleSender>,
    ) -> Self {
        let local_backend = Arc::new(LocalProcessBackend::new(
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

    pub fn backend_for_kind(&self, kind: SessionBackendKind) -> Arc<dyn PtyBackend> {
        match kind {
            SessionBackendKind::LocalProcess => self.local_backend.clone(),
            SessionBackendKind::ContainerTransport => self.container_backend.clone(),
        }
    }

    pub fn container_backend(&self) -> Arc<ContainerTransportBackend> {
        self.container_backend.clone()
    }

    pub fn start_container_pending_reaper(&self, shutdown: crate::shutdown::ShutdownSignal) {
        self.container_backend.start_pending_reaper(shutdown);
    }

    pub fn cleanup_container_orphans_on_startup(&self) {
        self.container_backend.cleanup_labeled_orphans_on_startup();
    }

    pub fn stop_all_started_containers_blocking(&self, budget: std::time::Duration) {
        self.container_backend
            .stop_all_started_containers_blocking(budget);
    }

    pub(crate) fn seal_and_drain_container_shutdown_work_blocking(&self) {
        self.container_backend
            .seal_and_drain_shutdown_work_blocking();
    }

    pub fn record_route(&self, id: Uuid, kind: SessionBackendKind) {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .insert(id, kind);
    }

    pub fn remove_route_if_kind(&self, id: Uuid, kind: SessionBackendKind) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if registry.routes.get(&id).copied() == Some(kind) {
            registry.routes.remove(&id);
        }
    }

    fn kind_for_session(&self, id: Uuid) -> Result<SessionBackendKind, AppError> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&id)
            .copied()
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))
    }

    pub fn backend_kind(&self, id: Uuid) -> Option<SessionBackendKind> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&id)
            .copied()
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
                    .map(|id| registry.routes.get(id).copied())
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

    pub async fn spawn(
        manager: &Arc<Mutex<Self>>,
        backend_kind: SessionBackendKind,
        spec: BackendSpawnSpec,
    ) -> Result<(), AppError> {
        let id = spec.id;
        let backend = {
            let manager = manager.lock().unwrap();
            manager.backend_for_kind(backend_kind)
        };
        backend.spawn(spec).await?;
        manager.lock().unwrap().record_route(id, backend_kind);
        Ok(())
    }

    pub fn write(&self, id: Uuid, data: &[u8]) -> Result<(), AppError> {
        let kind = self.kind_for_session(id)?;
        self.backend_for_kind(kind).write(id, data)
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
            .copied()
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
            .copied()
            .unwrap_or(SessionBackendKind::LocalProcess);
        self.backend_for_kind(kind)
            .register_response_watcher(session_id, request_id, response_dir);
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

        fn write(&self, id: Uuid, data: &[u8]) -> Result<(), AppError> {
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

        fn write(&self, _id: Uuid, _data: &[u8]) -> Result<(), AppError> {
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

    #[test]
    fn facade_delegates_local_route_by_session_id() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        backend.set_live(id);
        let manager = PtyManager::new_for_test(backend.clone());
        manager.record_route(id, SessionBackendKind::LocalProcess);

        manager.write(id, b"abc").expect("write");
        manager.resize(id, 120, 30).expect("resize");
        assert!(manager.has_session(id));
        assert_eq!(manager.get_pty_size(id), Some((30, 120)));
        assert!(manager.terminate_job_for_session(id));
        manager.register_response_watcher(id, "r1".to_string(), std::path::PathBuf::from("x"));
        manager.kill(id).expect("kill");

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
        assert!(!manager.has_session(id));
    }

    #[test]
    fn facade_write_without_route_returns_session_not_found() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = PtyManager::new_for_test(backend.clone());

        let err = manager.write(id, b"abc").expect_err("missing route");

        assert!(matches!(err, AppError::SessionNotFound(_)));
        assert!(backend.calls().is_empty());
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
