use super::*;

#[derive(Clone, Copy)]
pub(super) enum CriticalWaitPoint {
    AdmissionPermit,
    QueueSlot,
}

pub(super) async fn assert_cancelled_critical_waiter_releases_key(
    kind: CriticalAdmissionKind,
    wait_point: CriticalWaitPoint,
) {
    use crate::pty::backend::SessionBackendKind;

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/critical-cancellation".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create critical-cancellation fixture");
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(manager)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build critical-cancellation app");
    coordinator
        .start(app.handle().clone())
        .expect("start critical-cancellation coordinator");
    coordinator
        .inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);

    let reservations = (0..COORDINATOR_QUEUE_CAPACITY)
        .map(|_| {
            coordinator
                .reserve_auto_close()
                .expect("reserve physical queue slot")
        })
        .collect::<Vec<_>>();
    let held_admission = match wait_point {
        CriticalWaitPoint::AdmissionPermit => Some(
            Arc::clone(&coordinator.inner.admission)
                .try_acquire_owned()
                .expect("hold final logical admission permit"),
        ),
        CriticalWaitPoint::QueueSlot => None,
    };

    let waiter = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.critical_probe_for_test(session.id, kind).await })
    };
    await_state_eq(
        "critical waiter reaches requested cancellation point",
        Duration::from_secs(1),
        || {
            let registered = coordinator.critical_key_registered_for_test(session.id, kind);
            match wait_point {
                CriticalWaitPoint::AdmissionPermit => registered,
                CriticalWaitPoint::QueueSlot => {
                    registered && coordinator.inner.admission.available_permits() == 0
                }
            }
        },
        true,
        String::new,
    )
    .await;

    waiter.abort();
    assert!(waiter
        .await
        .expect_err("waiter must be aborted")
        .is_cancelled());
    await_state_eq(
        "aborted waiter removes its critical admission key",
        Duration::from_secs(1),
        || coordinator.critical_key_registered_for_test(session.id, kind),
        false,
        String::new,
    )
    .await;

    drop(held_admission);
    drop(reservations);
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            coordinator.critical_probe_for_test(session.id, kind),
        )
        .await
        .expect("fresh same-kind submission completes")
        .expect("fresh same-kind submission succeeds"),
        CriticalAdmissionOutcome::Completed(())
    );
    // #1580 releases the key before Completed; this bounded wait is kept unchanged.
    await_state_eq(
        "fresh probe releases its critical admission key",
        Duration::from_secs(1),
        || coordinator.critical_key_registered_for_test(session.id, kind),
        false,
        String::new,
    )
    .await;
    assert!(!coordinator.critical_key_registered_for_test(session.id, kind));
    coordinator.close_and_join().await;
}

// #1580 deterministic ordering detector. The worker needs `critical_keys` to free
// the key; the test holds it. "All admission permits returned" happens after the
// send before the fix, and before the (blocked) send after it.
async fn assert_critical_completion_observed_after_release(shutdown_first: bool) {
    use crate::pty::backend::SessionBackendKind;
    use std::future::Future;
    use std::task::{Context, Waker};

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/critical-release-order".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create critical-release-order fixture");
    let token = CancellationToken::new();
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), token.clone());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(manager)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build critical-release-order app");
    coordinator
        .start(app.handle().clone())
        .expect("start critical-release-order coordinator");
    let barrier = coordinator
        .submit_restore_first()
        .await
        .expect("restore barrier holds the worker");

    let kind = CriticalAdmissionKind::RouteLoss;
    let mut probe = std::pin::pin!(coordinator.critical_probe_for_test(session.id, kind));
    let mut cx = Context::from_waker(Waker::noop());
    // Queue capacity drops only in the poll that also sends the envelope.
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            assert!(
                probe.as_mut().poll(&mut cx).is_pending(),
                "probe cannot complete while the barrier holds the worker"
            );
            if coordinator.inner.sender.capacity() == COORDINATOR_QUEUE_CAPACITY - 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("probe queues its envelope behind the barrier");
    assert!(coordinator.critical_key_registered_for_test(session.id, kind));
    assert_eq!(
        coordinator.inner.admission.available_permits(),
        COORDINATOR_ADMISSION_CAPACITY - 2
    );
    if shutdown_first {
        token.cancel();
    }

    // Lexical scope, no `.await` inside: clippy::await_holding_lock, and the
    // worker needs this lock. Do not replace the scope with `drop(keys)`.
    {
        let keys = coordinator.inner.critical_keys.lock().unwrap();
        assert!(keys.contains(&CriticalAdmissionKey {
            session_id: session.id,
            kind,
        }));
        barrier.finish();
        let anchor = Instant::now();
        while coordinator.inner.admission.available_permits() != COORDINATOR_ADMISSION_CAPACITY {
            assert!(
                anchor.elapsed() < Duration::from_secs(10),
                "#1580: worker did not return admission before releasing the critical key"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            probe.as_mut().poll(&mut cx).is_pending(),
            "#1580: completion was observable while the critical key was still registered"
        );
    }

    let result = tokio::time::timeout(Duration::from_secs(5), probe.as_mut())
        .await
        .expect("probe completes once the key lock is free");
    assert!(!coordinator.critical_key_registered_for_test(session.id, kind));
    assert_eq!(
        coordinator.inner.admission.available_permits(),
        COORDINATOR_ADMISSION_CAPACITY
    );
    if shutdown_first {
        assert_eq!(
            result,
            Err(SelectionCoordinatorError::Unavailable.to_string())
        );
    } else {
        assert_eq!(result, Ok(CriticalAdmissionOutcome::Completed(())));
    }
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn critical_success_is_observed_only_after_admission_and_key_release() {
    assert_critical_completion_observed_after_release(false).await;
}

#[tokio::test]
async fn shutdown_drained_critical_failure_is_observed_only_after_admission_and_key_release() {
    assert_critical_completion_observed_after_release(true).await;
}

#[tokio::test]
async fn disposed_envelope_releases_admission_and_critical_key_before_closing_response() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    let key = CriticalAdmissionKey {
        session_id: Uuid::new_v4(),
        kind: CriticalAdmissionKind::BackgroundCleanup,
    };
    assert!(coordinator.inner.critical_keys.lock().unwrap().insert(key));
    let admission = Arc::clone(&coordinator.inner.admission)
        .try_acquire_owned()
        .expect("acquire admission for disposal fixture");
    let (response, mut receiver) = oneshot::channel();
    let envelope = CoordinatorEnvelope {
        _admission: admission,
        _critical_admission: Some(CriticalAdmissionGuard::new(&coordinator.inner, key)),
        job: CoordinatorJob::Snapshot { response },
        _create_ticket: None,
    };

    // Lexical scope; the disposer thread blocks on this lock at the key.
    let disposer = {
        let keys = coordinator.inner.critical_keys.lock().unwrap();
        let disposer = std::thread::spawn(move || drop(envelope));
        let anchor = Instant::now();
        while coordinator.inner.admission.available_permits() != COORDINATOR_ADMISSION_CAPACITY {
            assert!(
                anchor.elapsed() < Duration::from_secs(10),
                "#1580: disposed envelope did not return admission first"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            matches!(
                receiver.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "#1580: disposal closed the caller's response before releasing the critical key"
        );
        assert!(keys.contains(&key));
        disposer
    };
    disposer.join().expect("join envelope disposer");
    assert!(matches!(
        receiver.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(!coordinator.critical_key_registered_for_test(key.session_id, key.kind));
}

#[tokio::test]
async fn restore_barrier_start_acknowledgement_keeps_admission_until_release() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(manager)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build restore admission test app");
    coordinator
        .start(app.handle().clone())
        .expect("start selection coordinator");

    let guard = coordinator
        .submit_restore_first()
        .await
        .expect("submit first restore job");
    assert_eq!(
        coordinator.inner.admission.available_permits(),
        COORDINATOR_ADMISSION_CAPACITY - 1,
        "RestoreBarrier start acknowledgement is not completion; admission stays held"
    );
    guard.finish();
    tokio::time::timeout(Duration::from_secs(1), async {
        while coordinator.inner.admission.available_permits() != COORDINATOR_ADMISSION_CAPACITY {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("restore barrier returns admission after release");
    coordinator.close_and_join().await;
}
