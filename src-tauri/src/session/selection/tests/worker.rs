use super::*;

#[tokio::test]
async fn create_auto_selection_precondition_exists_only_for_starting_none() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    coordinator
        .inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);
    let none_ticket = coordinator
        .reserve_create(TrustedCreateIntent::User)
        .await
        .expect("reserve under none");
    assert!(none_ticket.auto_select_precondition.is_some());
    drop(none_ticket);

    manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/work".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("test fixture live session");
    let live_ticket = coordinator
        .reserve_create(TrustedCreateIntent::User)
        .await
        .expect("reserve under live");
    assert!(live_ticket.auto_select_precondition.is_none());
}

#[tokio::test]
async fn worker_task_local_rejects_recursive_external_submission() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    coordinator
        .inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);
    let error = IN_SELECTION_WORKER
        .scope((), async {
            coordinator
                .snapshot()
                .await
                .expect_err("recursive snapshot must fail")
        })
        .await;
    assert_eq!(
        error,
        SelectionCoordinatorError::RecursiveSubmission.to_string()
    );

    let reserve_error = IN_SELECTION_WORKER
        .scope((), async {
            coordinator
                .reserve_create(TrustedCreateIntent::User)
                .await
                .unwrap_err()
        })
        .await;
    assert_eq!(
        reserve_error,
        SelectionCoordinatorError::RecursiveSubmission
    );
    let auto_close_error = IN_SELECTION_WORKER
        .scope((), async {
            match coordinator.reserve_auto_close() {
                Err(error) => error,
                Ok(_) => panic!("recursive auto-close reservation must fail"),
            }
        })
        .await;
    assert_eq!(
        auto_close_error,
        SelectionCoordinatorError::RecursiveSubmission
    );
    let route_error = IN_SELECTION_WORKER
        .scope((), async {
            coordinator
                .container_lifecycle_sender()
                .route_lost(Uuid::new_v4(), 1)
                .await
                .unwrap_err()
        })
        .await;
    assert_eq!(
        route_error,
        SelectionCoordinatorError::RecursiveSubmission.to_string()
    );
    assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
}

/// #1940 - the owned operation turn serializes the whole persisted
/// publication -> restart settlement span: a queued guarded mutation cannot
/// acknowledge until the first operation releases it, and the release lands
/// after the settlement step, never between publication and settlement.
#[tokio::test]
async fn issue_1937_operation_turn_serializes() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let (a_entered_tx, a_entered_rx) = oneshot::channel::<()>();
    let (release_a_tx, release_a_rx) = oneshot::channel::<()>();

    let events_a = Arc::clone(&events);
    let first = tokio::spawn(run_owned_selection_operation(move || {
        let events = Arc::clone(&events_a);
        async move {
            events.lock().unwrap().push("a:publish");
            assert!(
                selection_operation_turn().try_write().is_err(),
                "the publication phase must run under the owned turn"
            );
            a_entered_tx.send(()).expect("signal first enter");
            release_a_rx.await.expect("release first operation");
            assert!(
                selection_operation_turn().try_write().is_err(),
                "the restart settlement span must stay under the owned turn"
            );
            events.lock().unwrap().push("a:settle");
            Ok::<(), String>(())
        }
    }));
    a_entered_rx.await.expect("first operation entered");

    let events_b = Arc::clone(&events);
    let (b_entered_tx, mut b_entered_rx) = oneshot::channel::<()>();
    let second = tokio::spawn(run_owned_selection_operation(move || {
        let events = Arc::clone(&events_b);
        async move {
            events.lock().unwrap().push("b:publish");
            b_entered_tx.send(()).expect("signal second enter");
            Ok::<(), String>(())
        }
    }));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !second.is_finished(),
        "the queued guarded mutation must not acknowledge while the first holds the turn"
    );
    assert!(
        b_entered_rx.try_recv().is_err(),
        "the queued guarded mutation must not enter before the turn is released"
    );
    assert_eq!(&*events.lock().unwrap(), &["a:publish"]);

    release_a_tx.send(()).expect("release the first operation");
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(
        &*events.lock().unwrap(),
        &["a:publish", "a:settle", "b:publish"]
    );
}

/// #1940 - a submission from inside the selection worker must be rejected
/// before waiting, for both the owned helper and the raw acquisition, and
/// rejection must not consume the turn.
#[tokio::test]
async fn issue_1937_operation_turn_rejects_worker() {
    let executed = Arc::new(AtomicBool::new(false));
    let executed_probe = Arc::clone(&executed);
    let error = IN_SELECTION_WORKER
        .scope((), async move {
            run_owned_selection_operation(move || async move {
                executed_probe.store(true, Ordering::SeqCst);
                Ok::<(), String>(())
            })
            .await
        })
        .await
        .unwrap_err();
    assert_eq!(
        error,
        SelectionCoordinatorError::RecursiveSubmission.to_string()
    );
    assert!(
        !executed.load(Ordering::SeqCst),
        "the rejected operation must never run"
    );

    let acquire_error = IN_SELECTION_WORKER
        .scope((), async {
            acquire_selection_operation_turn()
                .await
                .err()
                .expect("worker-local acquisition must be rejected")
        })
        .await;
    assert_eq!(
        acquire_error,
        SelectionCoordinatorError::RecursiveSubmission
    );

    let guard = acquire_selection_operation_turn()
        .await
        .expect("normal acquisition after a rejection");
    drop(guard);
}

/// #1940 - dropping the outer caller detaches the owned task: the turn
/// stays held until the accepted operation finishes, and the next guarded
/// operation observes the final state instead of racing it.
#[tokio::test]
async fn issue_1937_operation_turn_client_drop() {
    let final_state = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let state_for_owner = Arc::clone(&final_state);
    let owner = tokio::spawn(run_owned_selection_operation(move || async move {
        entered_tx.send(()).expect("signal owner enter");
        release_rx.await.expect("release owner operation");
        state_for_owner.store(7, Ordering::SeqCst);
        Ok::<(), String>(())
    }));
    entered_rx.await.expect("owner operation entered");

    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());
    assert_eq!(final_state.load(Ordering::SeqCst), 0);

    let (observed_tx, observed_rx) = oneshot::channel::<usize>();
    let state_for_observer = Arc::clone(&final_state);
    let (done_tx, mut done_rx) = oneshot::channel::<()>();
    let observer = tokio::spawn(run_owned_selection_operation(move || async move {
        observed_tx
            .send(state_for_observer.load(Ordering::SeqCst))
            .expect("report observed state");
        done_tx.send(()).expect("signal observer done");
        Ok::<(), String>(())
    }));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !observer.is_finished(),
        "dropping the outer caller must not release the owned turn"
    );
    assert!(done_rx.try_recv().is_err());

    release_tx.send(()).expect("release the detached owner");
    let observed = observed_rx.await.expect("observer acquired the turn");
    assert_eq!(
        observed, 7,
        "the subsequent operation must observe the owner's final state"
    );
    observer.await.unwrap().unwrap();
}

/// #1940 - coordinator-side restart failures (closed, admission-busy, dead
/// worker channel) and a failed owned task all surface as explicit errors;
/// the persisted subset stays, nothing late is queued, and the turn is
/// released only after the failure classification.
#[tokio::test]
async fn issue_1937_operation_turn_restart_failure() {
    fn restart_request(session_id: Uuid) -> crate::commands::session::RestartJobRequest {
        crate::commands::session::RestartJobRequest {
            session_id,
            agent_id: None,
            requested_profile: None,
            skip_auto_resume: None,
            activate_after: false,
            intent: TrustedRestartIntent::Background,
            communication_override: None,
            enforcement: crate::config::sessions_persistence::default_creation_gate_enforcement(),
        }
    }

    async fn panicking_operation() -> Result<(), String> {
        panic!("simulated operation worker failure");
    }

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    // (a) Coordinator closing: the phase gate rejects before any admission.
    let closing = SelectionCoordinator::new(
        Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
        CancellationToken::new(),
    );
    closing
        .inner
        .phase
        .store(CoordinatorPhase::Closing as u8, Ordering::Release);
    let persisted = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let persisted_a = Arc::clone(&persisted);
    let closing_for_operation = closing.clone();
    let restart_app = app.handle().clone();
    let error = run_owned_selection_operation(move || {
        let persisted = Arc::clone(&persisted_a);
        async move {
            persisted.lock().unwrap().push("a:publish");
            let failure = closing_for_operation
                .restart_lifecycle(restart_app.clone(), restart_request(Uuid::new_v4()))
                .await
                .map(|_| ())
                .unwrap_err();
            assert!(
                selection_operation_turn().try_write().is_err(),
                "the failure classification must stay under the owned turn"
            );
            persisted.lock().unwrap().push("a:classified");
            Err::<(), String>(failure)
        }
    })
    .await
    .unwrap_err();
    assert_eq!(error, SelectionCoordinatorError::Unavailable.to_string());
    assert_eq!(
        &*persisted.lock().unwrap(),
        &["a:publish", "a:classified"],
        "a failed restart must retain the persisted subset"
    );
    assert_eq!(
        closing.inner.sender.capacity(),
        COORDINATOR_QUEUE_CAPACITY,
        "a rejected restart must not leave a late queued job"
    );

    // (b) Admission busy: every permit is taken, so no envelope is sent.
    let busy = SelectionCoordinator::new(
        Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
        CancellationToken::new(),
    );
    busy.inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);
    let mut held_permits = Vec::new();
    while let Ok(permit) = busy.inner.admission.clone().try_acquire_owned() {
        held_permits.push(permit);
    }
    assert_eq!(held_permits.len(), COORDINATOR_ADMISSION_CAPACITY);
    let busy_for_operation = busy.clone();
    let restart_app = app.handle().clone();
    let error = run_owned_selection_operation(move || async move {
        busy_for_operation
            .restart_lifecycle(restart_app.clone(), restart_request(Uuid::new_v4()))
            .await
            .map(|_| ())
    })
    .await
    .unwrap_err();
    assert_eq!(error, SelectionCoordinatorError::Busy.to_string());
    assert_eq!(
        busy.inner.sender.capacity(),
        COORDINATOR_QUEUE_CAPACITY,
        "a busy admission must not consume a queue slot"
    );
    drop(held_permits);

    // (c) Worker gone: phase still says Running, but the worker channel is
    // closed, so the accepted-job channel reports Unavailable.
    let orphaned = SelectionCoordinator::new(
        Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
        CancellationToken::new(),
    );
    orphaned
        .inner
        .phase
        .store(CoordinatorPhase::Running as u8, Ordering::Release);
    drop(orphaned.inner.receiver.lock().unwrap().take());
    let orphaned_for_operation = orphaned.clone();
    let restart_app = app.handle().clone();
    let error = run_owned_selection_operation(move || async move {
        orphaned_for_operation
            .restart_lifecycle(restart_app.clone(), restart_request(Uuid::new_v4()))
            .await
            .map(|_| ())
    })
    .await
    .unwrap_err();
    assert_eq!(error, SelectionCoordinatorError::Unavailable.to_string());
    assert_eq!(
        orphaned.inner.sender.capacity(),
        COORDINATOR_QUEUE_CAPACITY,
        "a dead worker channel must not accept a late job"
    );

    // A real accepted entry releases its provisional capacity while the
    // existing restore barrier is active. Saturation then belongs to the
    // actual second probe, rather than an invented admission receipt.
    let second_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let second_probe = SelectionCoordinator::new(second_manager.clone(), CancellationToken::new());
    let second_app = tauri::test::mock_builder()
        .manage(second_manager)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    second_probe.start(second_app.handle().clone()).unwrap();
    let restore = second_probe.submit_restore_first().await.unwrap();
    let accepted = {
        let coordinator = second_probe.clone();
        let app = second_app.handle().clone();
        tokio::spawn(async move {
            coordinator
                .restart_lifecycle(app, restart_request(Uuid::new_v4()))
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let waiting = second_probe
                .inner
                .retained_replacements
                .lock()
                .unwrap()
                .entries
                .len()
                == 1;
            if waiting
                && second_probe.inner.admission.available_permits()
                    == COORDINATOR_ADMISSION_CAPACITY - 1
            {
                // The restore envelope itself holds the remaining permit.
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("entry accepted and provisional admission released");
    let mut saturation = Vec::new();
    while let Ok(permit) = second_probe.inner.admission.clone().try_acquire_owned() {
        saturation.push(permit);
    }
    // The restore envelope will release one admission permit. Occupy the
    // real queue slots too, so the second probe still observes Busy without
    // racing that release or submitting any synthetic lifecycle job.
    let mut occupied_slots = Vec::new();
    while let Ok(slot) = second_probe.inner.sender.clone().try_reserve_owned() {
        occupied_slots.push(slot);
    }
    assert_eq!(occupied_slots.len(), COORDINATOR_QUEUE_CAPACITY);
    restore.finish();
    let outcome = tokio::time::timeout(Duration::from_secs(2), accepted)
        .await
        .expect("second probe returns without queueing")
        .unwrap();
    assert_eq!(
        outcome.unwrap_err(),
        SelectionCoordinatorError::Busy.to_string()
    );
    assert!(second_probe
        .inner
        .retained_replacements
        .lock()
        .unwrap()
        .entries
        .is_empty());
    drop(occupied_slots);
    drop(saturation);
    second_probe.close_and_join().await;

    // (d) Owned-task failure: a panicking operation maps to the explicit
    // join error, and the turn is not poisoned.
    let error = run_owned_selection_operation(panicking_operation)
        .await
        .unwrap_err();
    assert_eq!(error, "selectionOperationTurnJoinFailed");
    let guard = acquire_selection_operation_turn()
        .await
        .expect("a failed owned task must still release the turn");
    drop(guard);
}
