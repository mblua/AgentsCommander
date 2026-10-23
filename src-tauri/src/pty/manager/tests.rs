use super::*;
use std::time::Duration;

mod response_close;

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

/// Includes store execute-failure and late-bind dispatcher recheck only;
/// the latter is neither valid startup order nor a production commit/admission race.
#[tokio::test]
async fn response_close_waiting_writer_is_counted_and_cancellation_releases_only_its_admission() {
    use std::future::Future;
    let id = Uuid::new_v4();
    let backend = Arc::new(RecordingBackend::default());
    let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    manager
        .lock()
        .unwrap()
        .try_record_route(id, SessionBackendKind::LocalProcess)
        .unwrap();
    let state = manager.lock().unwrap().response_close_state();
    let first = PtyManager::acquire_input_writer(&manager, id)
        .await
        .unwrap();
    let mut waiting = Box::pin(PtyManager::acquire_input_writer(&manager, id));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(waiting.as_mut().poll(&mut context).is_pending());
    assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 2);
    drop(waiting);
    assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 1);
    assert!(backend.calls().is_empty());
    PtyManager::write_with_permit(&first, b"first").unwrap();
    drop(first);
    assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 0);
    assert_eq!(backend.calls(), vec![Call::Write(id, b"first".to_vec())]);
    // Integrated store execute-failure oracle, not the SQL commit-failure branch.
    use crate::api::message_store::{EnqueueRequest, MessageStore};
    for fail_execute in [false, true] {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("store-oracle.sqlite3");
        let store = MessageStore::open(path.clone()).unwrap();
        let tracker = ResponseCloseState::default();
        store.bind_response_close(tracker.clone());
        let row = store
            .enqueue(EnqueueRequest {
                sender_fqn: "project:room/sender".into(),
                target_fqn: "project:room/member".into(),
                op_id: "store-oracle".into(),
                content_type: crate::api::message_store::DEFAULT_CONTENT_TYPE.into(),
                body: "durable-body".into(),
                source_plane: "api_inline".into(),
                source_ref: None,
            })
            .unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let snapshot = || {
            connection.query_row("SELECT message_id,target_fqn,status,body,body_sha256,attempt,lease_owner,lease_until FROM messages WHERE message_id=?1", [&row.message_id], |r| Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?,r.get::<_, String>(2)?,r.get::<_, String>(3)?,r.get::<_, String>(4)?,r.get::<_, i64>(5)?,r.get::<_, Option<String>>(6)?,r.get::<_, Option<String>>(7)?))).unwrap()
        };
        let before = snapshot();
        let key = format!(
            "db:{}:{}:{}",
            "project:room/sender".len(),
            "project:room/sender",
            "store-oracle"
        );
        assert_eq!(
            tracker.inner.lock().unwrap().work[&row.target_fqn].pending[&key],
            before.4
        );
        let delivery = tracker.begin_delivery(&row.target_fqn).unwrap();
        if fail_execute {
            connection.execute_batch("CREATE TRIGGER fail_terminal BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'controlled execute failure'); END;").unwrap();
            assert!(store
                .mark_delivered(&row.message_id, chrono::Utc::now())
                .is_err());
            assert_eq!(snapshot(), before);
            assert_eq!(before.2, "queued");
            tracker.settle_terminal(&row.target_fqn, &key, std::time::Instant::now());
            drop(delivery);
            let inner = tracker.inner.lock().unwrap();
            assert!(inner.failed);
            assert_eq!(inner.work[&row.target_fqn].pending[&key], before.4);
            assert!(!inner.work[&row.target_fqn].terminal.contains_key(&key));
        } else {
            store
                .mark_delivered(&row.message_id, chrono::Utc::now())
                .unwrap();
            drop(delivery);
            assert_eq!(snapshot().2, "delivered");
            let inner = tracker.inner.lock().unwrap();
            assert!(!inner.failed);
            assert!(!inner.work[&row.target_fqn].pending.contains_key(&key));
            assert_eq!(inner.work[&row.target_fqn].terminal[&key], before.4);
        }
    }

    // C: PTY terminal persistence SQL failure before commit, not commit ambiguity.
    for fail_execute in [false, true] {
        use crate::phone::types::{PtyInputPublicStatus, PtyInputReasonCode, PtyInputSourcePlane};
        assert!(crate::phone::types::pty_input_reason_allowed_for_status(
            PtyInputPublicStatus::Rejected,
            Some(PtyInputReasonCode::Expired)
        ));
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("pty-terminal-oracle.sqlite3");
        let store = MessageStore::open(path.clone()).unwrap();
        let tracker = ResponseCloseState::default();
        store.bind_response_close(tracker.clone());
        let id = Uuid::new_v4().to_string();
        let issued = chrono::Utc::now();
        store
            .enqueue_pty_input(crate::api::message_store::PtyInputEnqueueRequest {
                injection_id: id.clone(),
                sender_fqn: "proj:wg-1-team/lead".into(),
                target_fqn: "proj:wg-1-team/dev".into(),
                op_id: id.clone(),
                nonce_sha256: "a".repeat(64),
                request_fingerprint: crate::phone::types::sha256_hex(b"pty-terminal-body"),
                confirmation_tag: Some("b".repeat(64)),
                requested_agent_id: None,
                payload: b"pty-terminal-body".to_vec(),
                source_plane: PtyInputSourcePlane::HostCli,
                sender_incarnation_fingerprint: "c".repeat(64),
                sender_identity_fingerprint: "d".repeat(64),
                target_identity_fingerprint: "e".repeat(64),
                authority_session_id: Uuid::new_v4().to_string(),
                authority_client_id: None,
                authority_client_generation: None,
                issued_at: crate::phone::types::canonical_pty_timestamp(issued),
                expires_at: crate::phone::types::canonical_pty_timestamp(
                    issued + chrono::Duration::minutes(10),
                ),
            })
            .unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let snapshot = || {
            connection.query_row("SELECT injection_id,sender_fqn,target_fqn,op_id,request_fingerprint,status,payload,attempt,lease_owner,lease_until,updated_at FROM pty_input_operations WHERE injection_id=?1", [&id], |r| Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?,r.get::<_, String>(2)?,r.get::<_, String>(3)?,r.get::<_, String>(4)?,r.get::<_, String>(5)?,r.get::<_, Option<Vec<u8>>>(6)?,r.get::<_, i64>(7)?,r.get::<_, Option<String>>(8)?,r.get::<_, Option<String>>(9)?,r.get::<_, String>(10)?))).unwrap()
        };
        let before = snapshot();
        let key = format!("pty:{}:{}:{}", before.1.len(), before.1, before.3);
        assert_eq!(before.5, "queued");
        assert_eq!(before.6.as_deref(), Some(b"pty-terminal-body".as_slice()));
        assert_eq!(
            tracker.inner.lock().unwrap().work[&before.2].pending[&key],
            before.4
        );
        if fail_execute {
            connection.execute_batch("CREATE TRIGGER fail_pty_terminal BEFORE UPDATE ON pty_input_operations BEGIN SELECT RAISE(ABORT, 'controlled precommit SQL failure'); END;").unwrap();
            assert!(matches!(
                store.terminalize_pty_input(
                    &id,
                    PtyInputPublicStatus::Rejected,
                    Some(PtyInputReasonCode::Expired),
                    chrono::Utc::now()
                ),
                Err(crate::api::message_store::MessageStoreError::Sqlite(_))
            ));
            assert_eq!(snapshot(), before);
            let inner = tracker.inner.lock().unwrap();
            assert!(inner.failed);
            assert_eq!(inner.work[&before.2].pending[&key], before.4);
            assert!(!inner.work[&before.2].terminal.contains_key(&key));
            let tombstones: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM pty_input_tombstones WHERE injection_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(tombstones, 0);
        } else {
            let result = store
                .terminalize_pty_input(
                    &id,
                    PtyInputPublicStatus::Rejected,
                    Some(PtyInputReasonCode::Expired),
                    chrono::Utc::now(),
                )
                .unwrap();
            assert_eq!(result.status, PtyInputPublicStatus::Rejected);
            assert!(result.terminal);
            assert_eq!(result.reason.unwrap().code, PtyInputReasonCode::Expired);
            let tombstone: (String, String) = connection.query_row("SELECT status,request_fingerprint FROM pty_input_tombstones WHERE injection_id=?1", [&id], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!(tombstone, ("rejected".into(), before.4.clone()));
            let inner = tracker.inner.lock().unwrap();
            assert!(!inner.failed);
            assert!(!inner.work[&before.2].pending.contains_key(&key));
            assert_eq!(inner.work[&before.2].terminal[&key], before.4);
        }
    }

    // Late-bind recheck only: not valid startup order or a production commit/admission race.
    use crate::api::dispatcher::{dispatch_due_with, DispatcherConfig};
    for closing in [false, true] {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("late-bind-recheck.sqlite3");
        let store = MessageStore::open(path.clone()).unwrap();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let tracker = manager.lock().unwrap().response_close_state();
        for publisher in [
            ResponseClosePublisher::Manager,
            ResponseClosePublisher::Pty,
            ResponseClosePublisher::RawOutput,
            ResponseClosePublisher::Detector,
            ResponseClosePublisher::Menu,
            ResponseClosePublisher::Hold,
        ] {
            tracker.bind_publisher(publisher);
        }
        tracker.mark_work_bootstrapped();
        tracker.try_enable_readiness().unwrap();
        let now = std::time::Instant::now() + Duration::from_secs(3600);
        tracker.apply_settings(true, 30, now);
        let member_id = Uuid::new_v4();
        let coordinator_id = Uuid::new_v4();
        for id in [member_id, coordinator_id] {
            manager
                .lock()
                .unwrap()
                .try_record_route(id, SessionBackendKind::LocalProcess)
                .unwrap();
        }
        let member = ResponseRouteIdentity {
            target: "project:room/member".into(),
            session_id: member_id,
            token: Uuid::new_v4(),
            generation: tracker.generation(member_id).unwrap(),
            carrier_version: 1,
            backend: SessionBackendKind::LocalProcess,
        };
        let coordinator = ResponseRouteIdentity {
            target: "project:room/coordinator".into(),
            session_id: coordinator_id,
            token: Uuid::new_v4(),
            generation: tracker.generation(coordinator_id).unwrap(),
            ..member.clone()
        };
        for (identity, eligible) in [(&member, true), (&coordinator, false)] {
            tracker.publish_manager_row(
                identity.session_id,
                identity.token,
                identity.generation,
                1,
                eligible,
                false,
                now,
            );
            tracker.publish_manager_facts(identity.clone(), eligible, false, now);
        }
        tracker.observe_detector(member_id, false, now);
        tracker.publish_menu(member_id, Some(false), now);
        tracker.publish_hold(member_id, None, true, now);
        let mut admission = tracker
            .reserve_work(&member.target, "policy-request")
            .unwrap();
        admission.accept_durable("policy-body").unwrap();
        drop(admission);
        let context = tracker
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                "policy-request".into(),
                "policy-body".into(),
                now,
            )
            .unwrap();
        tracker.mark_request_submitted(&context, true);
        tracker
            .register_request(
                ResponseDeliveryProof {
                    context,
                    member: member.clone(),
                    coordinator: coordinator.clone(),
                    published_carrier_version: 1,
                },
                now,
            )
            .unwrap();
        let context = tracker
            .admit_return(&member, &coordinator, "ack".into(), "ack-body".into(), now)
            .unwrap();
        tracker
            .observe_return_success(ReturnDeliveryProof { context }, now)
            .unwrap();
        let at = now + Duration::from_secs(30);
        let ticket = tracker.select_ticket(member_id, at).unwrap();
        if closing {
            PtyManager::authority_route_proof(&manager, member_id)
                .unwrap()
                .try_commit_response_close(&tracker, &ticket, at)
                .unwrap();
        }
        let enqueue = |op: &str, target: &str, body: &str| {
            store
                .enqueue(EnqueueRequest {
                    sender_fqn: "project:room/sender".into(),
                    target_fqn: target.into(),
                    op_id: op.into(),
                    content_type: crate::api::message_store::DEFAULT_CONTENT_TYPE.into(),
                    body: body.into(),
                    source_plane: "api_inline".into(),
                    source_ref: None,
                })
                .unwrap()
        };
        let first = enqueue("first", &coordinator.target, "first-body");
        let second = enqueue("second", &member.target, "second-body");
        let connection = rusqlite::Connection::open(&path).unwrap();
        // Explicit stable order without clock sleeps or timestamp collisions.
        connection
            .execute(
                "UPDATE messages SET created_at='2000-01-01T00:00:00Z' WHERE message_id=?1",
                [&first.message_id],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE messages SET created_at='2000-01-01T00:00:01Z' WHERE message_id=?1",
                [&second.message_id],
            )
            .unwrap();
        let snapshot = |id: &str| {
            connection.query_row("SELECT message_id,target_fqn,status,body,body_sha256,attempt,lease_owner,lease_until,next_attempt_at,sender_fqn,last_error FROM messages WHERE message_id=?1", [id], |r| Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?,r.get::<_, String>(2)?,r.get::<_, String>(3)?,r.get::<_, String>(4)?,r.get::<_, i64>(5)?,r.get::<_, Option<String>>(6)?,r.get::<_, Option<String>>(7)?,r.get::<_, String>(8)?,r.get::<_, String>(9)?,r.get::<_, Option<String>>(10)?))).unwrap()
        };
        let before = snapshot(&second.message_id);
        let first_key = format!(
            "db:{}:{}:first",
            "project:room/sender".len(),
            "project:room/sender"
        );
        let second_key = format!(
            "db:{}:{}:second",
            "project:room/sender".len(),
            "project:room/sender"
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let copy = store.clone();
        let bound = tracker.clone();
        let first_id = first.message_id.clone();
        let dispatch_before = chrono::Utc::now();
        let processed = dispatch_due_with(
            &store,
            chrono::Utc::now(),
            &DispatcherConfig::default(),
            move |message| {
                recorded.lock().unwrap().push(message.id.clone());
                if message.id == first_id {
                    copy.bind_response_close(bound.clone());
                    let inner = bound.inner.lock().unwrap();
                    assert!(!inner.failed);
                    assert!(inner.work_bootstrapped);
                    assert_eq!(
                        inner.work["project:room/coordinator"].pending[&first_key],
                        crate::phone::types::sha256_hex(b"first-body")
                    );
                    assert_eq!(
                        inner.work["project:room/member"].pending[&second_key],
                        crate::phone::types::sha256_hex(b"second-body")
                    );
                }
                async { Ok(()) }
            },
        )
        .await
        .unwrap();
        let dispatch_after = chrono::Utc::now();
        assert_eq!(processed, 2);
        assert_eq!(snapshot(&first.message_id).2, "delivered");
        let after = snapshot(&second.message_id);
        if closing {
            assert_eq!(*calls.lock().unwrap(), vec![first.message_id.clone()]);
            assert_eq!(after.2, "queued");
            assert_eq!(after.9, before.9);
            assert_eq!(
                after.10.as_deref(),
                Some(crate::api::message_store::RESPONSE_CLOSING_DEFERRED)
            );
            let due = chrono::DateTime::parse_from_rfc3339(&after.8)
                .unwrap()
                .with_timezone(&chrono::Utc);
            assert!(dispatch_before <= due && due <= dispatch_after);
            let audit: (String, Option<String>) = connection.query_row("SELECT status,detail FROM message_audit WHERE message_id=?1 AND status='lease-released-deferred'", [&second.message_id], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!(audit.0, "lease-released-deferred");
            assert_eq!(
                audit.1.as_deref(),
                Some(crate::api::message_store::RESPONSE_CLOSING_DEFERRED)
            );
            assert_eq!(
                (after.0, after.1, after.3, after.4, after.5, after.6, after.7),
                (before.0, before.1, before.3, before.4, before.5, None, None)
            );
            assert!(tracker.target_is_closing(&member.target));
            assert_eq!(
                tracker.inner.lock().unwrap().work[&member.target].pending[&format!(
                    "db:{}:{}:second",
                    "project:room/sender".len(),
                    "project:room/sender"
                )],
                crate::phone::types::sha256_hex(b"second-body")
            );
        } else {
            assert_eq!(
                *calls.lock().unwrap(),
                vec![first.message_id.clone(), second.message_id.clone()]
            );
            assert_eq!(after.2, "delivered");
        }
        assert!(backend.calls().is_empty());
        // Separate bound-from-start SQL Closing filter, same positive twin.
        let third = enqueue("bound-filter", &member.target, "filter-body");
        let before = snapshot(&third.message_id);
        let rows = store
            .lease_due(chrono::Utc::now(), 10, Duration::from_secs(30), "filter")
            .unwrap();
        if closing {
            assert!(rows.is_empty());
            assert_eq!(snapshot(&third.message_id), before);
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].message_id, third.message_id);
            assert_eq!(rows[0].body, "filter-body");
        }
    }
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
