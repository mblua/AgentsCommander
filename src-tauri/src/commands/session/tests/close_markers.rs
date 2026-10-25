use super::*;

// ---- #2413: close markers survive a failed coordinator create ----

const MARKER_CWD: &str = "C:/ac-test/project/.ac/wg-2413-dev-team/__agent_tech-lead";

fn marker_ts(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("valid timestamp")
}

/// Mock app with a clocks store and a recorder for both close-marker events.
fn marker_unit_app() -> (
    tauri::App<tauri::test::MockRuntime>,
    crate::config::coordinator_clocks::CoordinatorClocksState,
    std::sync::mpsc::Receiver<(String, String)>,
) {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(Default::default()));
    app.manage(Arc::clone(&clocks));
    let events = marker_events(&app);
    (app, clocks, events)
}

/// Records `(event, value)` for both close-marker events; `value` is the
/// timestamp field (`null` for a clear).
fn marker_events(
    app: &tauri::App<tauri::test::MockRuntime>,
) -> std::sync::mpsc::Receiver<(String, String)> {
    use tauri::Listener;

    let (sender, receiver) = std::sync::mpsc::channel();
    for (event_name, field) in [
        ("coordinator_auto_close_changed", "autoClosedAt"),
        ("coordinator_manual_close_changed", "manuallyClosedAt"),
    ] {
        let sender = sender.clone();
        app.listen_any(event_name, move |event| {
            let payload: serde_json::Value =
                serde_json::from_str(event.payload()).expect("event payload json");
            let value = match &payload[field] {
                serde_json::Value::String(t) => t.clone(),
                other => other.to_string(),
            };
            let _ = sender.send((event_name.to_string(), value));
        });
    }
    receiver
}

fn drain(events: &std::sync::mpsc::Receiver<(String, String)>) -> Vec<(String, String)> {
    events.try_iter().collect()
}

fn auto_event(value: &str) -> (String, String) {
    (
        "coordinator_auto_close_changed".to_string(),
        value.to_string(),
    )
}

fn manual_event(value: &str) -> (String, String) {
    (
        "coordinator_manual_close_changed".to_string(),
        value.to_string(),
    )
}

/// #2413 T1
#[test]
fn restore_close_markers_puts_back_original_auto_timestamp() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    let t = marker_ts(0);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, t));
    let k = super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    k.failed();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), Some(t));
    assert_eq!(
        drain(&events),
        vec![auto_event("null"), auto_event(&t.to_rfc3339())]
    );
    assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
}

/// #2413 T2
#[test]
fn restore_close_markers_puts_back_original_manual_timestamp() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    let t = marker_ts(0);
    assert!(clocks.lock().unwrap().mark_manually_closed(&fqn, t));
    let k = super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), None);
    k.failed();
    assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), Some(t));
    assert_eq!(
        drain(&events),
        vec![manual_event("null"), manual_event(&t.to_rfc3339())]
    );
}

/// #2413 T3: a manual close during the failing create wins over the auto restore.
#[test]
fn restore_close_markers_manual_wins() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    let t = marker_ts(0);
    let manual = marker_ts(50);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, t));
    let k = super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    assert!(clocks.lock().unwrap().mark_manually_closed(&fqn, manual));
    k.failed();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    assert_eq!(
        clocks.lock().unwrap().manually_closed_at(&fqn),
        Some(manual)
    );
    assert_eq!(drain(&events), vec![auto_event("null")]);
}

/// #2413 T4
#[test]
fn restore_close_markers_noop_when_nothing_cleared() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD)
        .expect("ticket")
        .failed();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), None);
    assert!(drain(&events).is_empty());
    assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
}

/// #2413 T4
#[test]
fn restore_close_markers_succeeded_never_restores() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, marker_ts(0)));
    super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD)
        .expect("ticket")
        .succeeded();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    assert_eq!(drain(&events), vec![auto_event("null")]);
    assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
}

/// #2413 T12: a ticket dropped without an end counts as a failure.
#[test]
fn dropped_ticket_restores_marker() {
    let (app, clocks, _events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    let t = marker_ts(0);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, t));
    drop(super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket"));
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), Some(t));
    assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
}

/// #2413 T13: once the spawn fence is set, no end restores anything.
#[test]
fn fenced_ticket_never_restores() {
    for use_drop in [false, true] {
        let (app, clocks, events) = marker_unit_app();
        let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
        assert!(clocks
            .lock()
            .unwrap()
            .mark_manually_closed(&fqn, marker_ts(5)));
        // `mark_manually_closed` clears auto, so both markers cannot be set at
        // once: a first create escrows manual, a second one escrows auto.
        let first =
            super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
        assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, marker_ts(0)));
        let mut k =
            super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
        drop(first); // hands off: k is still running
        k.session_may_exist = true;
        if use_drop {
            drop(k);
        } else {
            k.failed();
        }
        assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
        assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), None);
        assert!(
            drain(&events).iter().all(|(_, v)| v == "null"),
            "no timestamp event"
        );
        assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
    }

    // Overlap: an unfenced ticket that fails after a fenced end restores nothing.
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, marker_ts(0)));
    let a = super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    let mut k =
        super::super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    k.session_may_exist = true;
    k.failed();
    a.failed();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    assert_eq!(drain(&events), vec![auto_event("null")]);
}

/// #2413 grinch step-9: a failed create's restore paused before its emit must
/// not deliver its timestamp after a newer create's clear. The UI-visible
/// state (last event) must match the clock.
#[test]
fn restore_emit_never_lands_after_newer_create_clear() {
    // The process-wide seam is keyed by FQN; other marker tests must not hit it.
    const CWD: &str = "C:/ac-test/project/.ac/wg-2413-restore-emit/__agent_tech-lead";
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(CWD);
    let t = marker_ts(0);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, t));
    let failing = super::super::note_coordinator_create_on_app(app.handle(), CWD).expect("ticket");
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    super::super::restore_emit_seam::install(&fqn, reached_tx, release_rx);

    let restorer = std::thread::spawn(move || failing.failed());
    reached_rx.recv().expect("restore paused before its emit");
    // The clock holds the restored marker; its event is not out yet.
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), Some(t));

    let handle = app.handle().clone();
    let (newer_done_tx, newer_done_rx) = std::sync::mpsc::channel();
    let newer = std::thread::spawn(move || {
        super::super::note_coordinator_create_on_app(&handle, CWD)
            .expect("ticket")
            .succeeded();
        let _ = newer_done_tx.send(());
    });
    // Without ordering the newer create finishes while the restore is
    // paused; with it, it waits. Either way, release afterwards.
    let _ = newer_done_rx.recv_timeout(Duration::from_millis(200));
    release_tx.send(()).unwrap();
    restorer.join().unwrap();
    newer.join().unwrap();

    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    let auto_events: Vec<_> = drain(&events)
        .into_iter()
        .filter(|(name, _)| name == "coordinator_auto_close_changed")
        .collect();
    assert_eq!(
        auto_events,
        vec![
            auto_event("null"),
            auto_event(&t.to_rfc3339()),
            auto_event("null")
        ],
        "last event must match the cleared clock"
    );
}

/// #2413 T5: the spawn fence is set before the spawn await and cleared only
/// on a spawn `Err` without a launch.
#[test]
fn create_session_inner_fences_marker_ticket_around_spawn() {
    let sources = super::support::session_sources();
    use super::support::{source_chain, source_effect, source_fence, source_mark, source_pending};
    assert!(super::support::source_map_complete(&sources));
    assert!(
        source_fence(&sources)
            && source_pending(&sources)
            && source_mark(&sources)
            && source_effect(&sources)
            && source_chain(&sources)
    );
}

struct MarkerCreateFixture {
    _temp: tempfile::TempDir,
    cwd: String,
    fqn: String,
    app: tauri::App<tauri::test::MockRuntime>,
    clocks: crate::config::coordinator_clocks::CoordinatorClocksState,
    events: std::sync::mpsc::Receiver<(String, String)>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<crate::pty::manager::PtyManager>>,
}

impl MarkerCreateFixture {
    fn new(backend: Arc<dyn crate::pty::backend::PtyBackend>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let cwd =
            crate::path_utils::normalize_windows_verbatim_path(&temp.path().to_string_lossy());
        let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend,
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
            Arc::new(Mutex::new(Default::default()));
        app.manage(Arc::clone(&clocks));
        let events = marker_events(&app);
        super::super::coordinator_create_seam::force(&cwd);
        Self {
            _temp: temp,
            cwd,
            fqn,
            app,
            clocks,
            events,
            session_mgr,
            pty_mgr,
        }
    }

    fn create(&self) -> impl std::future::Future<Output = Result<SessionInfo, String>> + '_ {
        super::super::create_session_inner(
            self.app.handle(),
            &self.session_mgr,
            &self.pty_mgr,
            "missing-ac-test-command".to_string(),
            Vec::new(),
            self.cwd.clone(),
            None,
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
    }

    fn auto(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.clocks.lock().unwrap().auto_closed_at(&self.fqn)
    }

    fn ledger(&self) -> bool {
        self.clocks.lock().unwrap().has_create_ledger(&self.fqn)
    }
}

impl Drop for MarkerCreateFixture {
    fn drop(&mut self) {
        super::super::coordinator_create_seam::unforce(&self.cwd);
    }
}

/// #2413 T6
#[tokio::test]
async fn create_session_inner_restores_auto_closed_marker_on_spawn_failure() {
    let f = MarkerCreateFixture::new(Arc::new(FailingSpawnBackend::default()));
    let t = marker_ts(0);
    assert!(f.clocks.lock().unwrap().mark_auto_closed(&f.fqn, t));
    let err = f.create().await.expect_err("spawn should fail");
    assert!(err.contains("synthetic spawn failure"), "{err}");
    assert_eq!(f.auto(), Some(t));
    assert_eq!(
        drain(&f.events),
        vec![auto_event("null"), auto_event(&t.to_rfc3339())]
    );
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

/// #2413 T7
#[tokio::test]
async fn create_session_inner_restores_manual_closed_marker_on_spawn_failure() {
    let f = MarkerCreateFixture::new(Arc::new(FailingSpawnBackend::default()));
    let t = marker_ts(0);
    assert!(f.clocks.lock().unwrap().mark_manually_closed(&f.fqn, t));
    let err = f.create().await.expect_err("spawn should fail");
    assert!(err.contains("synthetic spawn failure"), "{err}");
    assert_eq!(f.clocks.lock().unwrap().manually_closed_at(&f.fqn), Some(t));
    assert_eq!(
        drain(&f.events),
        vec![manual_event("null"), manual_event(&t.to_rfc3339())]
    );
    close_test_coordinator(&f.app).await;
}

/// #2413 T8: a newer overlapping create that succeeded wins; no stale restore.
#[tokio::test]
async fn create_session_inner_failed_create_does_not_restore_after_newer_create() {
    let f = MarkerCreateFixture::new(Arc::new(FailingSpawnBackend::default()));
    assert!(f
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&f.fqn, marker_ts(0)));
    let barrier = super::super::seed_race_barriers::install_before_project_gate(&f.cwd);
    let driver = async {
        barrier.reached.notified().await;
        assert_eq!(f.auto(), None);
        super::super::note_coordinator_create_on_app(f.app.handle(), &f.cwd)
            .expect("ticket")
            .succeeded();
        barrier.release.notify_one();
    };
    let (result, ()) = tokio::join!(f.create(), driver);
    result.expect_err("spawn should fail");
    assert_eq!(f.auto(), None);
    assert_eq!(drain(&f.events), vec![auto_event("null")]);
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

/// #2413 T11: overlapping creates that all fail restore the marker exactly once.
#[tokio::test]
async fn create_session_inner_two_failed_overlapping_creates_restore_marker() {
    let f = MarkerCreateFixture::new(Arc::new(FailingSpawnBackend::default()));
    let t = marker_ts(0);
    assert!(f.clocks.lock().unwrap().mark_auto_closed(&f.fqn, t));
    let barrier = super::super::seed_race_barriers::install_before_project_gate(&f.cwd);
    let driver = async {
        barrier.reached.notified().await;
        super::super::note_coordinator_create_on_app(f.app.handle(), &f.cwd)
            .expect("ticket")
            .failed();
        // Handed off to the running create: nothing restored yet.
        assert_eq!(f.auto(), None);
        assert_eq!(drain(&f.events), vec![auto_event("null")]);
        barrier.release.notify_one();
    };
    let (result, ()) = tokio::join!(f.create(), driver);
    result.expect_err("spawn should fail");
    assert_eq!(f.auto(), Some(t));
    assert_eq!(drain(&f.events), vec![auto_event(&t.to_rfc3339())]);
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

/// Test backend for #2413 T14/T15.
/// `block`: `spawn` records the id (live) and waits for `release` after signaling `entered`.
/// Otherwise: `spawn` marks the launch witness, then fails with no session.
#[derive(Default)]
struct MarkerSpawnBackend {
    block: bool,
    block_only_first: bool,
    spawn_count: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    live: Mutex<Vec<Uuid>>,
}

impl crate::pty::backend::PtyBackend for MarkerSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            spec.launch_witness.mark();
            let index = self.spawn_count.fetch_add(1, Ordering::SeqCst);
            if self.block && (!self.block_only_first || index == 0) {
                self.live.lock().unwrap().push(spec.id);
                self.entered.notify_one();
                self.release.notified().await;
            }
            Err(crate::errors::AppError::PtyError(
                "synthetic post-launch failure".to_string(),
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

    fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
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

/// #2413 T14: a create cancelled while spawn runs keeps the markers cleared.
#[tokio::test]
async fn create_session_inner_keeps_markers_cleared_when_cancelled_after_spawn() {
    // Variant 1: the create future is dropped mid-spawn.
    let backend = Arc::new(MarkerSpawnBackend {
        block: true,
        ..Default::default()
    });
    let f = MarkerCreateFixture::new(backend.clone());
    assert!(f
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&f.fqn, marker_ts(0)));
    let mut create = Box::pin(f.create());
    tokio::select! {
        _ = backend.entered.notified() => {}
        _ = &mut create => panic!("create must stay parked in spawn"),
    }
    drop(create);
    assert_eq!(f.auto(), None);
    assert!(f.ledger(), "accepted core still owns the in-flight spawn");
    backend.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.ledger() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retained create observes its real spawn error");
    assert_eq!(drain(&f.events), vec![auto_event("null")]);
    close_test_coordinator(&f.app).await;

    // Variant 2: the selection coordinator shuts down mid-spawn.
    let backend = Arc::new(MarkerSpawnBackend {
        block: true,
        ..Default::default()
    });
    let f = MarkerCreateFixture::new(backend.clone());
    assert!(f
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&f.fqn, marker_ts(0)));
    let shutdown = async {
        backend.entered.notified().await;
        f.app
            .state::<crate::session::selection::SelectionCoordinator>()
            .shutdown_token()
            .cancel();
        assert!(
            f.ledger(),
            "shutdown does not cancel the accepted backend effect"
        );
        backend.release.notify_one();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(f.create(), shutdown)
    })
    .await
    .expect("create settles after backend result despite shutdown");
    result.expect_err("shutdown ends the create");
    assert_eq!(f.auto(), None);
    assert_eq!(drain(&f.events), vec![auto_event("null")]);
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

#[tokio::test]
async fn foreign_common_create_second_progresses_while_first_core_retained_after_caller_loss() {
    let backend = Arc::new(MarkerSpawnBackend {
        block: true,
        block_only_first: true,
        ..Default::default()
    });
    let fixture = MarkerCreateFixture::new(backend.clone());
    // This is the existing non-replica common-create path. No legacy
    // replica target stripe is bypassed to manufacture concurrent progress.
    assert!(
        crate::config::teams::pty_input_create_gate_key_from_cwd(std::path::Path::new(
            &fixture.cwd
        ))
        .unwrap()
        .is_none()
    );
    let mut first = Box::pin(fixture.create());
    tokio::select! {
        _ = backend.entered.notified() => {},
        _ = &mut first => panic!("first real core must be parked in backend spawn"),
    }
    drop(first);
    assert!(fixture.ledger(), "first accepted core remains retained");
    let second = tokio::time::timeout(Duration::from_secs(10), fixture.create())
        .await
        .expect(
            "second Foreign create must reach its actual backend result before first is released",
        );
    assert!(second
        .unwrap_err()
        .contains("synthetic post-launch failure"));
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
    assert!(
        fixture.ledger(),
        "second completion cannot drop first create's owner"
    );
    backend.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture.ledger() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first retained core eventually observes its actual result");
    close_test_coordinator(&fixture.app).await;
}

#[tokio::test]
async fn failed_tracker_foreign_common_create_keeps_real_legacy_backend_result() {
    let backend = Arc::new(MarkerSpawnBackend::default());
    let fixture = MarkerCreateFixture::new(backend.clone());
    let tracker = fixture.pty_mgr.lock().unwrap().response_close_state();
    tracker.fail_closed();
    let result = tokio::time::timeout(Duration::from_secs(10), fixture.create())
        .await
        .expect("E2 Foreign/nonClosing must reach real common-create completion");
    assert!(result
        .unwrap_err()
        .contains("synthetic post-launch failure"));
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
    assert!(!fixture.ledger());
    assert!(
        matches!(
            tracker.reserve_create_owner(
                "still-failed",
                Uuid::new_v4(),
                crate::pty::manager::CreateClassification::Foreign {
                    publication_epoch: 0
                }
            ),
            Err(crate::pty::manager::ResponseCloseError::Unavailable)
        ),
        "ordinary create never resets or substitutes the failed tracker"
    );
    close_test_coordinator(&fixture.app).await;
}

/// #2413 T15: a spawn `Err` after the backend launched keeps the markers cleared,
/// even though the backend reports no session.
#[tokio::test]
async fn create_session_inner_keeps_markers_cleared_when_spawn_fails_after_launch() {
    let f = MarkerCreateFixture::new(Arc::new(MarkerSpawnBackend::default()));
    assert!(f
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&f.fqn, marker_ts(0)));
    let err = f.create().await.expect_err("spawn should fail");
    assert!(err.contains("synthetic post-launch failure"), "{err}");
    assert_eq!(f.auto(), None);
    assert_eq!(drain(&f.events), vec![auto_event("null")]);
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

/// #2411 T1b: `create_session_inner_impl` calls the coordinator-create helper
/// exactly once (source guard, same pattern as the archive-gate test below).
#[test]
fn create_session_inner_calls_note_coordinator_create_once() {
    let sources = super::support::session_sources();
    use super::support::source_budgets;
    assert!(super::support::source_map_complete(&sources));
    assert!(source_budgets(&sources));
}

#[test]
fn create_session_inner_keeps_both_archive_activation_gates() {
    let sources = super::support::session_sources();
    use super::support::{source_archive, source_restart};
    assert!(super::support::source_map_complete(&sources));
    assert!(source_archive(&sources) && source_restart(&sources));
}
