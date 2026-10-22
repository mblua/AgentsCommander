use super::*;

#[tokio::test]
async fn blocking_seed_tracker_completes_and_removes_started_work() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    coordinator
        .inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);

    assert_eq!(coordinator.run_blocking_seed_work(|| 42_u8).await, Ok(42));
    assert!(coordinator
        .inner
        .blocking_seed_work
        .lock_state()
        .work
        .is_empty());
}

#[tokio::test]
async fn blocking_seed_tracker_is_panic_safe() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    coordinator
        .inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);

    let error = coordinator
        .run_blocking_seed_work(|| -> () { panic!("injected tracker panic") })
        .await
        .expect_err("panic must surface through the response join");
    assert!(matches!(error, BlockingSeedTaskError::Join(_)), "{error:?}");
    assert!(coordinator
        .inner
        .blocking_seed_work
        .lock_state()
        .work
        .is_empty());
}

#[test]
fn blocking_seed_tracker_seal_suppresses_queued_user_work() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let (blocker_started_tx, blocker_started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            blocker_started_tx.send(()).expect("signal blocker");
            release_rx.recv().expect("release blocker");
        });
        blocker_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking pool occupied");

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waiter = {
            let coordinator = coordinator.clone();
            let ran = Arc::clone(&ran);
            tokio::spawn(async move {
                coordinator
                    .run_blocking_seed_work(move || ran.store(true, Ordering::Release))
                    .await
            })
        };
        loop {
            if !coordinator
                .inner
                .blocking_seed_work
                .lock_state()
                .work
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Closing as u8, Ordering::Release);
        for abort in coordinator.inner.blocking_seed_work.seal() {
            abort.abort();
        }
        release_tx.send(()).expect("release blocker");
        blocker.await.expect("join blocker");
        assert!(matches!(
            waiter.await.expect("join waiter"),
            Err(BlockingSeedTaskError::CanceledBeforeStart)
        ));
        assert!(!ran.load(Ordering::Acquire));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_seed_tracker_keeps_detached_started_work_accounted() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    coordinator
        .inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let waiter = {
        let coordinator = coordinator.clone();
        let started = Arc::clone(&started);
        tokio::spawn(async move {
            coordinator
                .run_blocking_seed_work(move || {
                    started.store(true, Ordering::Release);
                    release_rx.recv().expect("release started work");
                })
                .await
        })
    };
    while !started.load(Ordering::Acquire) {
        tokio::task::yield_now().await;
    }
    waiter.abort();
    let _ = waiter.await;
    coordinator
        .inner
        .phase
        .store(CoordinatorPhase::Closing as u8, Ordering::Release);
    assert!(coordinator.inner.blocking_seed_work.seal().is_empty());
    assert!(
        !coordinator
            .inner
            .blocking_seed_work
            .wait_quiescent_until(Instant::now())
            .await
    );

    release_tx.send(()).expect("release worker");
    assert!(
        coordinator
            .inner
            .blocking_seed_work
            .wait_quiescent_until(Instant::now() + Duration::from_secs(1))
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_reports_the_exact_retained_blocking_seed_reason() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    coordinator
        .inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let waiter = {
        let coordinator = coordinator.clone();
        let started = Arc::clone(&started);
        tokio::spawn(async move {
            coordinator
                .run_blocking_seed_work(move || {
                    started.store(true, Ordering::Release);
                    release_rx.recv().expect("release retained work");
                })
                .await
        })
    };
    while !started.load(Ordering::Acquire) {
        tokio::task::yield_now().await;
    }

    let report = coordinator
        .close_and_join_with_budget(Duration::from_millis(20))
        .await;
    waiter.abort();
    let _ = waiter.await;
    release_tx.send(()).expect("release worker");
    assert!(
        coordinator
            .inner
            .blocking_seed_work
            .wait_quiescent_until(Instant::now() + Duration::from_secs(1))
            .await
    );
    assert!(!report.persistence_safe);
    assert_eq!(
        report
            .retained
            .iter()
            .filter(|reason| {
                reason.as_str() == "reason=blocking-seed-transaction-await state=retained"
            })
            .count(),
        1
    );
}

// Stage E (#1064) tracker mutation proofs (plan section 10.4 item 16): the
// poison-recovery and id-overflow paths were previously unexercised.
#[test]
fn blocking_seed_tracker_recovers_from_mutex_poison_and_preserves_work() {
    use std::sync::atomic::AtomicU8;
    let tracker = Arc::new(BlockingSeedWorkTracker::new());
    let phase = AtomicU8::new(CoordinatorPhase::Running as u8);
    let id = tracker.register(&phase).expect("register");

    // Poison the state mutex by panicking while holding it.
    let poisoner = Arc::clone(&tracker);
    let joined = std::thread::spawn(move || {
        let _guard = poisoner.state.lock().expect("lock to poison");
        panic!("intentional poison");
    })
    .join();
    assert!(joined.is_err(), "the poisoning thread must have panicked");

    // lock_state recovers via into_inner and preserves the tracked work map.
    assert!(
        tracker.lock_state().work.contains_key(&id),
        "poison recovery must preserve the tracked work map"
    );
    assert!(
        tracker.begin(id),
        "begin must still work after poison recovery"
    );
    tracker.complete_started(id);
    assert!(tracker.lock_state().work.is_empty());
}

#[test]
fn blocking_seed_tracker_id_overflow_seals_admission_without_reuse() {
    use std::sync::atomic::AtomicU8;
    let tracker = BlockingSeedWorkTracker::new();
    let phase = AtomicU8::new(CoordinatorPhase::Running as u8);
    let first = tracker.register(&phase).expect("first register");

    // Force the id space to its last value; the next register overflows.
    tracker.lock_state().next_id = u64::MAX;
    assert_eq!(
        tracker.register(&phase),
        Err(BlockingSeedTaskError::Unavailable),
        "an exhausted id space must fail closed"
    );

    {
        let state = tracker.lock_state();
        assert!(!state.accepting, "overflow seals admission");
        assert!(state.work.contains_key(&first), "prior ids remain tracked");
        assert!(
            !state.work.contains_key(&u64::MAX),
            "the overflowing id is never inserted or reused"
        );
    }
    assert_eq!(
        tracker.register(&phase),
        Err(BlockingSeedTaskError::Unavailable),
        "after sealing, no new id is admitted"
    );
}

#[test]
fn blocking_seed_tracker_late_attach_after_completion_is_harmless() {
    use std::sync::atomic::AtomicU8;
    let tracker = BlockingSeedWorkTracker::new();
    let phase = AtomicU8::new(CoordinatorPhase::Running as u8);
    let id = tracker.register(&phase).expect("register");
    assert!(tracker.begin(id));
    tracker.complete_started(id);
    assert!(tracker.lock_state().work.is_empty());

    // Attaching an abort handle for an id that already completed aborts the
    // handle immediately outside the mutex and never re-inserts or panics.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let abort = runtime
        .spawn(async {
            std::future::pending::<()>().await;
        })
        .abort_handle();
    tracker.attach_abort(id, abort);
    assert!(
        tracker.lock_state().work.is_empty(),
        "late attach must not re-insert"
    );
}

#[test]
fn exact_coordinator_error_strings_are_stable() {
    assert_eq!(
        SelectionCoordinatorError::Busy.to_string(),
        "selectionCoordinatorBusy"
    );
    assert_eq!(
        SelectionCoordinatorError::Unavailable.to_string(),
        "selectionCoordinatorUnavailable"
    );
    assert_eq!(
        SelectionCoordinatorError::RecursiveSubmission.to_string(),
        "selectionCoordinatorRecursiveSubmission"
    );
}
