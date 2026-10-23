use super::*;

/// #1151 (W6) - a backend that quarantines the first cleanup on a stubborn child and,
/// once armed, BLOCKS inside `observe_tree`, which is `kill_group_inner`'s first
/// backend call and therefore a deterministic barrier. Releasing the gate also makes
/// the child verifiably gone, so the parked retry settles to Terminated.
///
/// The barrier is announced through a Condvar rather than an AtomicBool so the test
/// thread can wait for it with a plain `std` wait. W6 must never need the async
/// runtime to make progress in order to report, because the defect it exists to catch
/// is exactly the one that stops the runtime from making progress.
struct QuarantineGatedBackend {
    root: ProcessIdentity,
    armed: AtomicBool,
    entered: (Mutex<bool>, Condvar),
    /// The thread that ran the armed `observe_tree`, recorded before it blocks. This
    /// is the subject of W6's assertion 1.
    observer: Mutex<Option<std::thread::ThreadId>>,
    child_gone: AtomicBool,
    gate: (Mutex<bool>, Condvar),
}

impl QuarantineGatedBackend {
    fn new(root: ProcessIdentity) -> Self {
        Self {
            root,
            armed: AtomicBool::new(false),
            entered: (Mutex::new(false), Condvar::new()),
            observer: Mutex::new(None),
            child_gone: AtomicBool::new(false),
            gate: (Mutex::new(false), Condvar::new()),
        }
    }

    /// Blocks the calling thread, using `std` only, until the armed `observe_tree` has
    /// recorded its thread and is about to park on the release gate. Returns false on
    /// timeout so the caller reports an assertion failure instead of hanging.
    fn wait_entered(&self, timeout: Duration) -> bool {
        let (lock, condvar) = &self.entered;
        let (entered, wait) = condvar
            .wait_timeout_while(lock.lock().unwrap(), timeout, |entered| !*entered)
            .unwrap();
        !wait.timed_out() && *entered
    }

    fn observer(&self) -> Option<std::thread::ThreadId> {
        *self.observer.lock().unwrap()
    }

    fn child(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.root.pid + 1,
            creation_time_100ns: self.root.creation_time_100ns + 1,
        }
    }

    fn released(&self) -> bool {
        *self.gate.0.lock().unwrap()
    }

    fn release(&self) {
        self.child_gone.store(true, Ordering::SeqCst);
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }

    fn observed(identity: ProcessIdentity, depth: u32, parent_pid: Option<u32>) -> ObservedProcess {
        ObservedProcess {
            identity,
            parent_pid,
            parent_identity: None,
            exe_name: format!("p{}", identity.pid),
            depth,
            private_bytes: None,
            working_set_bytes: None,
            cpu_percent: None,
            kill_allowed: true,
        }
    }
}

impl ProcessTreeBackend for QuarantineGatedBackend {
    fn observe_tree(&self, _root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        if self.armed.load(Ordering::SeqCst) {
            // Record the observer BEFORE announcing the barrier, so a waiter that
            // sees `entered` always sees the thread id too.
            *self.observer.lock().unwrap() = Some(std::thread::current().id());
            {
                let (lock, condvar) = &self.entered;
                *lock.lock().unwrap() = true;
                condvar.notify_all();
            }
            let mut released = self.gate.0.lock().unwrap();
            while !*released {
                released = self.gate.1.wait(released).unwrap();
            }
        }
        let mut processes = vec![Self::observed(self.root, 0, None)];
        if !self.child_gone.load(Ordering::SeqCst) {
            processes.push(Self::observed(self.child(), 1, Some(self.root.pid)));
        }
        Ok(ObservedProcessTree {
            processes,
            errors: Vec::new(),
        })
    }

    fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        if pid == self.root.pid {
            return Ok(Some(self.root));
        }
        if pid == self.child().pid && !self.child_gone.load(Ordering::SeqCst) {
            return Ok(Some(self.child()));
        }
        Ok(None)
    }

    fn terminate_verified(
        &self,
        process: &ObservedProcess,
    ) -> Result<TerminateOutcome, ResourceError> {
        if process.identity == self.child() && !self.child_gone.load(Ordering::SeqCst) {
            return Err(ResourceError::Message(format!(
                "pid {}: process still alive after terminate",
                process.identity.pid
            )));
        }
        Ok(TerminateOutcome::AlreadyGone)
    }

    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        Ok(ProcessMemory::default())
    }
}

// #1151 (W2) - a LIVE public session keeps the coordinator plus Job Object path
// exactly as before: the transaction runs, the session finalizes once, and the
// registry-only orphan path is never entered.
#[tokio::test]
async fn live_public_session_still_uses_coordinator() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/orphan-live-session".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let pty_backend = Arc::new(ResourcePtyBackend::default());
    pty_backend.set_live(session.id);
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(session.id, SessionBackendKind::LocalProcess);
    let root = ProcessIdentity {
        pid: 5300,
        creation_time_100ns: 900,
    };
    let process_backend = Arc::new(GatedBackend::new(root));
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        process_backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    register_running(monitor.as_ref(), session.id, root);
    assert_eq!(monitor.active_agent_groups(), 1);
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(Arc::clone(&monitor))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build live-session orphan-retry app");
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();

    let report = crate::resource_monitor::watchdog::retry_quarantined_group(
        monitor.as_ref(),
        &coordinator,
        session.id,
        root,
    )
    .await;

    assert_eq!(
        report.path,
        crate::resource_monitor::watchdog::QuarantineRetryPath::Coordinator
    );
    assert_eq!(report.root_pid, root.pid);
    assert_eq!(report.state, Some(ResourceGroupState::Terminated));
    assert!(!report.still_counts_toward_admission);
    assert_eq!(report.active_agent_groups, 0);
    assert_eq!(
        manager
            .read()
            .await
            .get_session(session.id)
            .await
            .unwrap()
            .status,
        SessionStatus::Exited(0)
    );
    // Finalized exactly once, through the coordinator: one PTY teardown and the route
    // gone. The registry-only path never touches the PTY at all.
    assert_eq!(pty_backend.terminate_count.load(Ordering::SeqCst), 1);
    assert!(!pty.lock().unwrap().has_session(session.id));
    coordinator.close_and_join().await;
}

// #1151 (W5) - NON-REGRESSION. A destroy-retained root agent keeps its row as Exited,
// so contains_public_or_pending is still true and the coordinator path stays. An
// implementation that keyed the orphan decision on "the PTY is gone" rather than "the
// session row is gone" would silently divert this working path onto the new one.
#[tokio::test]
async fn exited_row_still_uses_coordinator() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/orphan-exited-row".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    // mark_exited's bool reports whether a raise-hand flag was cleared, not success.
    manager.read().await.mark_exited(session.id, 0).await;
    assert_eq!(
        manager
            .read()
            .await
            .get_session(session.id)
            .await
            .unwrap()
            .status,
        SessionStatus::Exited(0)
    );
    let pty_backend = Arc::new(ResourcePtyBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(pty_backend.clone())));
    let root = ProcessIdentity {
        pid: 5400,
        creation_time_100ns: 1000,
    };
    let process_backend = Arc::new(GatedBackend::new(root));
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        process_backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    register_running(monitor.as_ref(), session.id, root);
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&pty))
        .manage(Arc::clone(&monitor))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build exited-row orphan-retry app");
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();

    let report = crate::resource_monitor::watchdog::retry_quarantined_group(
        monitor.as_ref(),
        &coordinator,
        session.id,
        root,
    )
    .await;

    assert_eq!(
        report.path,
        crate::resource_monitor::watchdog::QuarantineRetryPath::Coordinator
    );
    assert_eq!(
        manager
            .read()
            .await
            .get_session(session.id)
            .await
            .unwrap()
            .status,
        SessionStatus::Exited(0)
    );
    coordinator.close_and_join().await;
}

// #1151 (W6) - the orphan retry runs OFF the async runtime. kill_group can cost about
// two seconds per stubborn PID, so inlining it would stall a Tokio worker.
//
// Exactly ONE worker thread is load-bearing, not a style choice: it makes "the async
// runtime" an enumerable set of one OS thread, so the thread-identity assertion below
// is a complete statement rather than a sample. At two workers it degrades to sampling
// one worker out of two, and whether the mutation is caught becomes scheduler
// dependent. That is not acceptable for a regression pin, however it happens to land
// in any one environment.
//
// Every wait here is a `std` wait on the test thread, capped so a failure is an
// assertion and not a hang. `tokio::time` must not appear in this test: a shape that
// needs the runtime to make progress in order to REPORT cannot report the very defect
// that stops the runtime from making progress.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn orphan_retry_runs_off_the_async_runtime() {
    // Harness self-check, taken while the runtime is still idle: spawned tasks really
    // do run on a worker thread rather than on the block_on thread. If a future Tokio
    // ever changed that, every assertion below would be silently vacuous.
    let test_thread = std::thread::current().id();
    let worker_thread = tokio::spawn(async { std::thread::current().id() })
        .await
        .expect("the worker identity probe joins");
    assert_ne!(
        worker_thread, test_thread,
        "spawned tasks must run on the runtime's worker thread, not on the test thread"
    );

    let root = ProcessIdentity {
        pid: 5500,
        creation_time_100ns: 1100,
    };
    let process_backend = Arc::new(QuarantineGatedBackend::new(root));
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        process_backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    // No session row is ever created: that is what makes this group an orphan.
    let session_id = Uuid::new_v4();
    register_running(monitor.as_ref(), session_id, root);
    let quarantine = monitor
        .kill_group(session_id, ResourceKillReason::SessionDestroy)
        .expect("first cleanup runs");
    assert!(quarantine.quarantined);

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        ResourcePtyBackend::default(),
    ))));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(pty)
        .manage(Arc::clone(&monitor))
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build off-runtime orphan-retry app");
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();

    struct ReleaseOnDrop(Arc<QuarantineGatedBackend>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let _release_guard = ReleaseOnDrop(process_backend.clone());
    process_backend.armed.store(true, Ordering::SeqCst);

    // tokio::spawn is load-bearing too: it is what puts an inlined blocking call on
    // the worker thread, instead of on the test thread where it would satisfy
    // assertion 1 for the wrong reason.
    let retry = {
        let monitor = Arc::clone(&monitor);
        let coordinator = coordinator.clone();
        tokio::spawn(async move {
            crate::resource_monitor::watchdog::retry_quarantined_group(
                monitor.as_ref(),
                &coordinator,
                session_id,
                root,
            )
            .await
        })
    };
    assert!(
        process_backend.wait_entered(Duration::from_secs(5)),
        "the orphan retry enters the backend barrier"
    );
    let observer = process_backend
        .observer()
        .expect("the barrier records its thread before parking");

    // Assertion 2's probe, taken while the gate is still held: a task that the runtime
    // polls right now proves the runtime is still polling tasks. The channel is `std`,
    // so its result reaches the test thread whether or not the runtime is alive.
    let (tx, rx) = std::sync::mpsc::channel();
    tokio::spawn(async move {
        let _ = tx.send(());
    });
    let runtime_polled_a_task = rx.recv_timeout(Duration::from_secs(5)).is_ok();

    assert!(
        !process_backend.released(),
        "the barrier must still be held while both facts are captured"
    );
    process_backend.release();

    // Assertion 1: the blocking work ran on neither the runtime's single worker nor
    // the test thread, so it ran on the blocking pool. With one worker those two are
    // the only non-blocking-pool threads, so this is exhaustive. It rejects an inline
    // call and tokio::task::block_in_place alike.
    assert_ne!(
        observer, worker_thread,
        "the orphan retry must not run on the async runtime's worker thread"
    );
    assert_ne!(
        observer, test_thread,
        "the orphan retry must not run on the test thread"
    );
    // Assertion 2: the runtime kept making progress while the blocking call was in
    // flight. This is what rejects std::thread::spawn(..).join(), which is off the
    // runtime by identity yet parks the worker for the whole call. Never assert WHICH
    // thread sent it: block_in_place legitimately migrates the scheduler to a
    // replacement thread, so pinning the sender would add a false-failure mode.
    assert!(
        runtime_polled_a_task,
        "the async runtime must keep polling tasks while the orphan retry blocks"
    );

    let report = retry.await.unwrap();
    assert_eq!(
        report.path,
        crate::resource_monitor::watchdog::QuarantineRetryPath::Orphan
    );
    assert_eq!(report.state, Some(ResourceGroupState::Terminated));
    assert!(!report.still_counts_toward_admission);
    coordinator.close_and_join().await;
}
