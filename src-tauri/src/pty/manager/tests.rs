use super::*;
use std::time::Duration;

mod response_close;
mod response_close_writer;

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
        launch_witness: Default::default(),
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

/// #1388, T4 - the trait default reads as "no claim, do not gate".
///
/// Exercised on an existing fake that does not override it, which is the one path
/// production never takes: both production backends override. Its only content is the
/// literal `true` in `backend.rs`, so this is a guard against a future flip to `false`,
/// which would strand every non-answering implementor at the wake settle's cap.
#[test]
fn pty_backend_default_reports_rendered() {
    let backend = RecordingBackend::default();

    assert!(backend.has_rendered_visible_content(Uuid::new_v4()));
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
