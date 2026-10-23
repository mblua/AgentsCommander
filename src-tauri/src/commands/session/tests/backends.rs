use super::*;

#[derive(Default)]
pub(super) struct FailingSpawnBackend {
    pub(super) spawned: Mutex<Vec<Uuid>>,
    pub(super) killed: Mutex<Vec<Uuid>>,
    /// #973 - every (cols, rows) a spawn was asked for.
    pub(super) sizes: Mutex<Vec<(u16, u16)>>,
}

impl FailingSpawnBackend {
    pub(super) fn spawned(&self) -> Vec<Uuid> {
        self.spawned.lock().unwrap().clone()
    }

    pub(super) fn killed(&self) -> Vec<Uuid> {
        self.killed.lock().unwrap().clone()
    }

    /// #973 - the sizes the PTY would have been opened at.
    pub(super) fn sizes(&self) -> Vec<(u16, u16)> {
        self.sizes.lock().unwrap().clone()
    }
}

impl crate::pty::backend::PtyBackend for FailingSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.spawned.lock().unwrap().push(spec.id);
            // #973 - the size the ConPTY would have been opened at.
            self.sizes.lock().unwrap().push((spec.cols, spec.rows));
            Err(crate::errors::AppError::PtyError(
                "synthetic spawn failure".to_string(),
            ))
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.killed.lock().unwrap().push(id);
        Ok(())
    }

    fn has_session(&self, _id: Uuid) -> bool {
        false
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
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

/// A `PtyBackend` whose `spawn` parks on a oneshot so a test can observe the
/// `create_session_inner` spawn mark while the PTY is still being spawned.
/// `fail` decides whether the parked spawn ultimately errors (exercising the
/// rollback path) or succeeds and records the session as live.
pub(super) struct GatedSpawnBackend {
    pub(super) started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    pub(super) release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    pub(super) live: Mutex<Vec<Uuid>>,
    pub(super) fail: bool,
}

impl GatedSpawnBackend {
    pub(super) fn new(
        started: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
        fail: bool,
    ) -> Self {
        Self {
            started: Mutex::new(Some(started)),
            release: Mutex::new(Some(release)),
            live: Mutex::new(Vec::new()),
            fail,
        }
    }
}

impl crate::pty::backend::PtyBackend for GatedSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        let started = self.started.lock().unwrap().take().expect("started sender");
        let release = self
            .release
            .lock()
            .unwrap()
            .take()
            .expect("release receiver");
        let fail = self.fail;
        Box::pin(async move {
            // Announce that PtyManager::spawn has released the outer manager
            // mutex and is now parked inside the backend, then wait for the
            // test to inspect the spawn mark before the spawn resolves.
            let _ = started.send(());
            let _ = release.await;
            if fail {
                Err(crate::errors::AppError::PtyError(
                    "synthetic spawn failure".to_string(),
                ))
            } else {
                self.live.lock().unwrap().push(spec.id);
                Ok(())
            }
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.live.lock().unwrap().retain(|live| *live != id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
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

#[derive(Default)]
pub(crate) struct ScriptedSpawnBackend {
    pub(super) live: Mutex<HashSet<Uuid>>,
    pub(super) spawned: Mutex<Vec<Uuid>>,
    pub(super) spawn_count: AtomicUsize,
    pub(super) fail_spawn_number: AtomicUsize,
    pub(super) gate_spawn_number: AtomicUsize,
    pub(super) gate_started: Mutex<Option<tokio::sync::oneshot::Sender<Uuid>>>,
    pub(super) gate_release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

pub(super) struct BarrierDestroyBackend {
    pub(super) live: Mutex<HashSet<Uuid>>,
    pub(super) fail_kill: Mutex<HashSet<Uuid>>,
    pub(super) kills: Mutex<Vec<Uuid>>,
    pub(super) started: Mutex<Option<tokio::sync::oneshot::Sender<Uuid>>>,
    pub(super) release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl BarrierDestroyBackend {
    pub(super) fn new(
        started: tokio::sync::oneshot::Sender<Uuid>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> Self {
        Self {
            live: Mutex::new(HashSet::new()),
            fail_kill: Mutex::new(HashSet::new()),
            kills: Mutex::new(Vec::new()),
            started: Mutex::new(Some(started)),
            release: Mutex::new(Some(release)),
        }
    }

    pub(super) fn fail_kill_for(&self, session_id: Uuid) {
        self.fail_kill.lock().unwrap().insert(session_id);
    }
}

impl crate::pty::backend::PtyBackend for BarrierDestroyBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.live.lock().unwrap().insert(spec.id);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        self.live
            .lock()
            .unwrap()
            .contains(&id)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        self.live
            .lock()
            .unwrap()
            .contains(&id)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.kills.lock().unwrap().push(id);
        if let Some(started) = self.started.lock().unwrap().take() {
            let _ = started.send(id);
            if let Some(release) = self.release.lock().unwrap().take() {
                release
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .map_err(|error| {
                        crate::errors::AppError::PtyError(format!(
                            "destroy barrier release failed: {error}"
                        ))
                    })?;
            }
        }
        if self.fail_kill.lock().unwrap().contains(&id) {
            return Err(crate::errors::AppError::PtyError(
                "synthetic live teardown failure".to_string(),
            ));
        }
        self.live.lock().unwrap().remove(&id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.has_session(id).then_some((30, 120))
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
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

impl ScriptedSpawnBackend {
    pub(crate) fn fail_spawn(&self, number: usize) {
        self.fail_spawn_number.store(number, Ordering::SeqCst);
    }

    pub(super) fn gate_spawn(
        &self,
        number: usize,
        started: tokio::sync::oneshot::Sender<Uuid>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) {
        self.gate_spawn_number.store(number, Ordering::SeqCst);
        *self.gate_started.lock().unwrap() = Some(started);
        *self.gate_release.lock().unwrap() = Some(release);
    }

    pub(super) fn lose_route(&self, session_id: Uuid) {
        self.live.lock().unwrap().remove(&session_id);
    }
}

impl crate::pty::backend::PtyBackend for ScriptedSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            let number = self.spawn_count.fetch_add(1, Ordering::SeqCst) + 1;
            self.spawned.lock().unwrap().push(spec.id);
            if self.gate_spawn_number.load(Ordering::SeqCst) == number {
                if let Some(started) = self.gate_started.lock().unwrap().take() {
                    let _ = started.send(spec.id);
                }
                let release = self.gate_release.lock().unwrap().take();
                if let Some(release) = release {
                    let _ = release.await;
                }
            }
            if self.fail_spawn_number.load(Ordering::SeqCst) == number {
                return Err(crate::errors::AppError::PtyError(
                    "synthetic scripted spawn failure".to_string(),
                ));
            }
            self.live.lock().unwrap().insert(spec.id);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        self.live
            .lock()
            .unwrap()
            .contains(&id)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        self.live
            .lock()
            .unwrap()
            .contains(&id)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.live.lock().unwrap().remove(&id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.has_session(id).then_some((30, 120))
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
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
