use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::AppHandle;
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::backend::{PtyBackend, SessionBackendKind};
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::local_backend::LocalProcessBackend;
use crate::pty::output::PtyScreenSnapshot;
use crate::resource_monitor::ResourceLaunchRegistration;
use crate::session::profile::IdleTuning;
use crate::telegram::manager::OutputSenderMap;

pub struct PtyManager {
    routes: Arc<Mutex<HashMap<Uuid, SessionBackendKind>>>,
    local_backend: Arc<dyn PtyBackend>,
}

impl PtyManager {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        git_watcher: Arc<GitWatcher>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    ) -> Self {
        let local_backend = Arc::new(LocalProcessBackend::new(
            output_senders,
            idle_detector,
            git_watcher,
            ws_broadcaster,
        ));
        Self {
            routes: Arc::new(Mutex::new(HashMap::new())),
            local_backend,
        }
    }

    #[cfg(test)]
    fn new_for_test(local_backend: Arc<dyn PtyBackend>) -> Self {
        Self {
            routes: Arc::new(Mutex::new(HashMap::new())),
            local_backend,
        }
    }

    pub fn backend_for_kind(&self, kind: SessionBackendKind) -> &dyn PtyBackend {
        match kind {
            SessionBackendKind::LocalProcess => self.local_backend.as_ref(),
        }
    }

    pub fn record_route(&self, id: Uuid, kind: SessionBackendKind) {
        self.routes.lock().unwrap().insert(id, kind);
    }

    pub fn remove_route_if_kind(&self, id: Uuid, kind: SessionBackendKind) {
        let mut routes = self.routes.lock().unwrap();
        if routes.get(&id).copied() == Some(kind) {
            routes.remove(&id);
        }
    }

    fn kind_for_session(&self, id: Uuid) -> Result<SessionBackendKind, AppError> {
        self.routes
            .lock()
            .unwrap()
            .get(&id)
            .copied()
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))
    }

    fn local_process_backend(&self) -> Result<&LocalProcessBackend, AppError> {
        self.local_backend
            .as_any()
            .downcast_ref::<LocalProcessBackend>()
            .ok_or_else(|| AppError::PtyError("local process backend unavailable".to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn<R: tauri::Runtime>(
        &self,
        id: Uuid,
        backend_kind: SessionBackendKind,
        cmd: &str,
        args: &[String],
        cwd: &str,
        cols: u16,
        rows: u16,
        configured_env: &[(String, String)],
        env_remove_keys: &[String],
        extra_env: &[(String, String)],
        idle_tuning: IdleTuning,
        app_handle: AppHandle<R>,
        resource_registration: Option<ResourceLaunchRegistration>,
    ) -> Result<(), AppError> {
        match backend_kind {
            SessionBackendKind::LocalProcess => {
                self.local_process_backend()?.spawn(
                    id,
                    cmd,
                    args,
                    cwd,
                    cols,
                    rows,
                    configured_env,
                    env_remove_keys,
                    extra_env,
                    idle_tuning,
                    app_handle,
                    resource_registration,
                )?;
            }
        }
        self.record_route(id, backend_kind);
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
            .routes
            .lock()
            .unwrap()
            .get(&id)
            .copied()
            .unwrap_or(SessionBackendKind::LocalProcess);
        let result = self.backend_for_kind(kind).kill(id);
        self.remove_route_if_kind(id, kind);
        result
    }

    pub fn terminate_job_for_session(&self, id: Uuid) -> bool {
        let Ok(kind) = self.kind_for_session(id) else {
            return false;
        };
        self.backend_for_kind(kind).terminate_job_for_session(id)
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
            .routes
            .lock()
            .unwrap()
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
}
