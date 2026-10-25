//! P18 selection dispatch tests for the lifecycle execution port.

use super::native_fixture::{
    native_receipt, restart_request, root_request, NativeFixture, PortTestBackend,
};
use crate::commands::session::selection_executor::NativeLifecycleLease;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::selection::{
    LifecycleAdmission, LifecycleEntryOutcome, LifecycleExecutorState, LifecycleLease,
    SelectionCoordinator, SelectionCoordinatorError, SelectionTransaction,
};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

async fn assert_missing_binding_start() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        PortTestBackend::default(),
    ))));
    let coordinator = SelectionCoordinator::new(manager.clone(), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(manager.clone())
        .manage(pty.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert_eq!(
        coordinator.start(app.handle().clone()),
        Err(SelectionCoordinatorError::Unavailable)
    );
    assert_unstarted(&coordinator);
    // Independent negative control exercises the kernel entry directly.
    let direct = SelectionCoordinator::new(manager, CancellationToken::new());
    assert_eq!(
        direct.start_with_executor(app.handle().clone(), None),
        Err(SelectionCoordinatorError::Unavailable)
    );
    assert_unstarted(&direct);
    assert_eq!(
        coordinator
            .restart_lifecycle_with_executor(app.handle().clone(), restart_request(), None)
            .await
            .unwrap_err(),
        SelectionCoordinatorError::Unavailable.to_string()
    );
    assert_eq!(
        coordinator
            .root_lifecycle_with_executor(app.handle().clone(), root_request(), None)
            .await
            .unwrap_err(),
        SelectionCoordinatorError::Unavailable.to_string()
    );
    assert_unstarted(&coordinator);
    assert!(
        app.manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
    );
    coordinator.start(app.handle().clone()).unwrap();
    let state = coordinator.port_test_state();
    assert!(!state.receiver_present);
    assert!(Arc::ptr_eq(state.cleanup_pty.as_ref().unwrap(), &pty));
    assert!(state.cleanup_backend_present);
    assert!(state.worker_present);
    coordinator.submit_restore_first().await.unwrap().finish();
    coordinator.snapshot().await.unwrap();
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn missing_binding_start_preserves_cleanup_receiver_and_capacity_for_retry() {
    assert_missing_binding_start().await;
}
#[tokio::test]
async fn port_native_missing_binding() {
    assert_missing_binding_start().await;
}

#[tokio::test]
async fn duplicate_binding_keeps_original_executor() {
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let original = Arc::clone(
        app.state::<LifecycleExecutorState<SelectionTransaction<tauri::test::MockRuntime>>>()
            .get(),
    );
    let transaction = SelectionTransaction::for_test(app.handle().clone())
        .with_lifecycle_executor(Arc::clone(&original));
    assert!(
        !app.manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
    );
    let managed =
        app.state::<LifecycleExecutorState<SelectionTransaction<tauri::test::MockRuntime>>>();
    assert!(Arc::ptr_eq(&original, managed.get()));
    assert!(Arc::ptr_eq(
        &original,
        &transaction.port_test_executor().unwrap()
    ));
    let mut lease = LifecycleLease::new(Box::new(()), Arc::new(AtomicU8::new(0)));
    assert!(!original.validate(&transaction, &mut lease).await);
}

#[tokio::test]
async fn wrong_payload_is_rejected_without_native_state_or_teardown() {
    // No native dependencies are installed: reaching a native body would panic.
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let executor = Arc::clone(
        app.state::<LifecycleExecutorState<SelectionTransaction<tauri::test::MockRuntime>>>()
            .get(),
    );
    let transaction = SelectionTransaction::for_test(app.handle().clone())
        .with_lifecycle_executor(Arc::clone(&executor));
    let receipt = Arc::new(AtomicU8::new(0));
    let mut lease = LifecycleLease::new(Box::new(()), receipt.clone());
    assert!(!executor.has_claim(&lease));
    assert!(!executor.validate(&transaction, &mut lease).await);
    let restart = restart_request();
    assert!(matches!(
        executor.restart(&transaction, restart, &lease).await,
        LifecycleEntryOutcome::Interrupted
    ));
    let root = root_request();
    assert!(matches!(
        executor.root(&transaction, root, &lease).await,
        LifecycleEntryOutcome::Interrupted
    ));
    assert!(!executor.release(lease));
    assert_eq!(receipt.load(Ordering::Acquire), 0);
    assert_eq!(transaction.port_test_binding_presence(), (false, false));
}

#[tokio::test]
async fn real_authority_explicit_and_fallback_release_free_claim_once() {
    let cwd = tempfile::tempdir().unwrap();
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let source = manager
        .read()
        .await
        .create_session(
            "shell".into(),
            Vec::new(),
            cwd.path().to_string_lossy().into_owned(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let source = source.id;
    let backend = Arc::new(PortTestBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    let tracker = pty.lock().unwrap().response_close_state();
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(manager.clone())
        .manage(pty)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let executor = Arc::clone(
        app.state::<LifecycleExecutorState<SelectionTransaction<tauri::test::MockRuntime>>>()
            .get(),
    );
    let transaction = SelectionTransaction::for_test(app.handle().clone())
        .with_lifecycle_executor(Arc::clone(&executor));
    for explicit in [true, false] {
        let LifecycleAdmission::Ready(lease) = executor
            .prepare(&transaction, Uuid::new_v4(), Some(source), false, None)
            .await
            .unwrap()
        else {
            panic!("native prepare must mint a real authority and claim");
        };
        assert!(executor.has_claim(&lease));
        let receipt = lease
            .downcast_ref::<NativeLifecycleLease>()
            .unwrap()
            .release_receipt
            .clone();
        assert_eq!(receipt.load(Ordering::Acquire), 0);
        let released = if explicit {
            Some(executor.release(lease))
        } else {
            drop(lease);
            None
        };
        assert_eq!(receipt.load(Ordering::Acquire), 1);
        assert!(released.is_none_or(|released| released));
        assert!(tracker.is_healthy());
        // A later real claim must not change the first lease's settled receipt.
        let LifecycleAdmission::Ready(next) = executor
            .prepare(&transaction, Uuid::new_v4(), Some(source), false, None)
            .await
            .unwrap()
        else {
            panic!("released target must accept a fresh authority");
        };
        assert!(executor.has_claim(&next));
        drop(next);
        assert_eq!(receipt.load(Ordering::Acquire), 1);
        assert_eq!(backend.kill_count(source), 0);
        assert!(manager.read().await.get_session(source).await.is_some());
    }
}

fn assert_unstarted(coordinator: &SelectionCoordinator) {
    let state = coordinator.port_test_state();
    assert!(state.cleanup_pty.is_none());
    assert!(!state.cleanup_backend_present);
    assert!(state.receiver_present);
    assert!(!state.worker_present);
    assert_eq!(state.queue_capacity, state.expected_queue_capacity);
    assert_eq!(state.admission_available, state.expected_admission_capacity);
    assert_eq!(
        state.create_tickets_available,
        state.expected_create_ticket_capacity
    );
}

async fn native_returned_release(explicit: bool, failed: bool) {
    let f = NativeFixture::new().await;
    f.coordinator.port_test_running_unstarted();
    let mut entry = f.coordinator.port_test_pending_entry(f.request()).unwrap();
    let lease = f.prepare(entry.operation()).await;
    let executor = f.executor();
    assert!(executor.has_claim(&lease));
    let receipt = native_receipt(&lease);
    assert_eq!(receipt.load(Ordering::Acquire), 0);
    entry.attach_lease(lease);
    let transaction = f.transaction();
    let entered = entry.enter(&transaction);
    assert!(entered.has_accepted_lifecycle_core());
    assert_ne!(entry.nonce(), Uuid::nil());
    let LifecycleEntryOutcome::Returned(result) =
        executor.restart(&entered, f.request(), entry.lease()).await
    else {
        panic!("native archived-project refusal must actually return")
    };
    assert!(result.as_ref().unwrap_err().contains("archived project"));
    entry.observe_return(&result);
    {
        let records = f.receipt_records();
        let record = records
            .iter()
            .find(|r| r.operation == entry.operation())
            .unwrap();
        assert_eq!(record.outcome, Some((true, false)));
        assert!(Arc::ptr_eq(&record.receipt, &receipt));
    }
    drop(entered);
    if failed {
        f.pty.lock().unwrap().response_close_state().fail_closed();
    }
    if explicit {
        entry.release(&*executor).await;
    } else {
        entry.drop_entry();
    }
    assert_eq!(receipt.load(Ordering::Acquire), if failed { 2 } else { 1 });
    assert_eq!(entry.sidecar_released(), !failed);
    if !failed {
        let next = f.prepare(Uuid::new_v4()).await;
        assert!(executor.has_claim(&next));
        drop(next);
        assert_eq!(receipt.load(Ordering::Acquire), 1);
    }
    assert!(f.manager.read().await.get_session(f.source).await.is_some());
    assert!(f.cwd.path().exists());
}

#[tokio::test]
async fn port_native_release_failure_explicit() {
    super::protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::executor_port::",
            "port_native_release_failure_explicit"
        ),
        port_native_release_failure_explicit_body(),
    )
    .await;
}

async fn port_native_release_failure_explicit_body() {
    native_returned_release(true, true).await;
    let f = NativeFixture::new().await;
    let lease = f.prepare(Uuid::new_v4()).await;
    assert!(f.executor().has_claim(&lease));
    let receipt = native_receipt(&lease);
    f.pty.lock().unwrap().response_close_state().fail_closed();
    let released = f.executor().release(lease);
    assert_eq!(receipt.load(Ordering::Acquire), 2);
    assert!(!released);
    super::protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::executor_port::",
        "port_native_release_failure_explicit"
    ));
}

#[tokio::test]
async fn port_native_release_failure_drop() {
    super::protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::executor_port::",
            "port_native_release_failure_drop"
        ),
        port_native_release_failure_drop_body(),
    )
    .await;
}

async fn port_native_release_failure_drop_body() {
    native_returned_release(false, true).await;
    let f = NativeFixture::new().await;
    let lease = f.prepare(Uuid::new_v4()).await;
    assert!(f.executor().has_claim(&lease));
    let receipt = native_receipt(&lease);
    f.pty.lock().unwrap().response_close_state().fail_closed();
    drop(lease);
    assert_eq!(receipt.load(Ordering::Acquire), 2);
    super::protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::executor_port::",
        "port_native_release_failure_drop"
    ));
}

#[tokio::test]
async fn port_native_release_paths() {
    super::protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::executor_port::",
            "port_native_release_paths"
        ),
        port_native_release_paths_body(),
    )
    .await;
}

async fn port_native_release_paths_body() {
    native_returned_release(true, false).await;
    native_returned_release(false, false).await;
    for exit in [
        crate::session::selection::SelectionPortTestExit::Drain,
        crate::session::selection::SelectionPortTestExit::FailedEnqueue,
        crate::session::selection::SelectionPortTestExit::ReceiverLoss,
    ] {
        let f = NativeFixture::new().await;
        f.coordinator.port_test_running_unstarted();
        let mut entry = f.coordinator.port_test_pending_entry(f.request()).unwrap();
        let lease = f.prepare(entry.operation()).await;
        assert!(f.executor().has_claim(&lease));
        let receipt = native_receipt(&lease);
        entry.attach_lease(lease);
        let returned = matches!(
            exit,
            crate::session::selection::SelectionPortTestExit::ReceiverLoss
        );
        let owner = f
            .coordinator
            .port_test_drive(&f.transaction(), entry, f.request(), exit)
            .await;
        assert_eq!(receipt.load(Ordering::Acquire), 1);
        assert_eq!(owner.sidecar_released(), returned);
        let next = f.prepare(Uuid::new_v4()).await;
        assert!(f.executor().has_claim(&next));
        drop(next);
        assert_eq!(receipt.load(Ordering::Acquire), 1);
        let state = f.coordinator.port_test_state();
        assert_eq!(state.admission_available, 65);
    }
    let f = NativeFixture::new().await;
    let lease = f.prepare(Uuid::new_v4()).await;
    assert!(f.executor().has_claim(&lease));
    let receipt = native_receipt(&lease);
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _lease = lease;
        panic!("native lease unwind stimulus");
    }));
    assert!(unwind.is_err());
    assert_eq!(receipt.load(Ordering::Acquire), 1);
    let next = f.prepare(Uuid::new_v4()).await;
    assert!(f.executor().has_claim(&next));
    drop(next);
    assert_eq!(receipt.load(Ordering::Acquire), 1);
    super::protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::executor_port::",
        "port_native_release_paths"
    ));
}

async fn foreign_control(failed: bool, closing: bool, retained: bool) {
    let f = NativeFixture::new().await;
    f.coordinator.port_test_running_unstarted();
    let entry = f.coordinator.port_test_pending_entry(f.request()).unwrap();
    let operation = if retained {
        entry.operation()
    } else {
        Uuid::new_v4()
    };
    let tracker = f.pty.lock().unwrap().response_close_state();
    let target = format!(
        "create-cwd:{}",
        crate::path_identity::verify_directory(f.cwd.path())
            .unwrap()
            .canonical_path
            .display()
    );
    if closing {
        close_native_target(&f, &target);
    }
    assert_eq!(tracker.target_is_closing(&target), closing);
    if failed {
        tracker.fail_closed();
    }
    let t = f.transaction();
    match f
        .executor()
        .prepare(&t, operation, Some(f.source), false, None)
        .await
        .unwrap()
    {
        LifecycleAdmission::Ready(lease) => {
            assert!(!closing);
            if failed {
                assert!(retained);
                assert!(!f.executor().has_claim(&lease));
            } else {
                assert!(
                    f.executor().has_claim(&lease),
                    "healthy Foreign keeps real claim"
                );
            }
            let receipt = native_receipt(&lease);
            let released = f.executor().release(lease);
            assert_eq!(receipt.load(Ordering::Acquire), 1);
            assert!(released);
        }
        LifecycleAdmission::Waiting => assert!(failed && (closing || !retained)),
        LifecycleAdmission::NeedsRootDirectory => panic!("ordinary target is not root"),
    }
    assert_eq!(tracker.target_is_closing(&target), closing);
    assert!(f.manager.read().await.get_session(f.source).await.is_some());
}

fn close_native_target(f: &NativeFixture, target: &str) {
    use crate::pty::manager::{ResponseDeliveryProof, ResponseRouteIdentity, ReturnDeliveryProof};
    use std::time::{Duration, Instant};
    let tracker = f.pty.lock().unwrap().response_close_state();
    use crate::pty::manager::ResponseClosePublisher as P;
    for publisher in [
        P::Manager,
        P::Pty,
        P::RawOutput,
        P::Detector,
        P::Menu,
        P::Hold,
    ] {
        tracker.bind_publisher(publisher);
    }
    tracker.mark_work_bootstrapped();
    tracker.try_enable_readiness().unwrap();
    let now = Instant::now() + Duration::from_secs(3600);
    tracker.apply_settings(true, 30, now);
    let peer = Uuid::new_v4();
    for id in [f.source, peer] {
        f.pty
            .lock()
            .unwrap()
            .try_record_route(id, crate::pty::backend::SessionBackendKind::LocalProcess)
            .unwrap();
    }
    let member = ResponseRouteIdentity {
        target: target.to_string(),
        session_id: f.source,
        token: Uuid::new_v4(),
        generation: tracker.generation(f.source).unwrap(),
        carrier_version: 1,
        backend: crate::pty::backend::SessionBackendKind::LocalProcess,
    };
    let coordinator = ResponseRouteIdentity {
        target: format!("peer:{peer}"),
        session_id: peer,
        token: Uuid::new_v4(),
        generation: tracker.generation(peer).unwrap(),
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
    tracker.observe_detector(f.source, false, now);
    tracker.publish_menu(f.source, Some(false), now);
    tracker.publish_hold(f.source, None, true, now);
    let mut admission = tracker.reserve_work(target, "native-request").unwrap();
    admission.accept_durable("native-body").unwrap();
    drop(admission);
    let context = tracker
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            "native-request".into(),
            "native-body".into(),
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
        .admit_return(
            &member,
            &coordinator,
            "native-ack".into(),
            "native-ack-body".into(),
            now,
        )
        .unwrap();
    tracker
        .observe_return_success(ReturnDeliveryProof { context }, now)
        .unwrap();
    let at = now + Duration::from_secs(30);
    let ticket = tracker.select_ticket(f.source, at).unwrap();
    PtyManager::authority_route_proof(&f.pty, f.source)
        .unwrap()
        .try_commit_response_close(&tracker, &ticket, at)
        .unwrap();
    assert!(tracker.target_is_closing(target));
}

#[tokio::test]
async fn port_native_foreign_failed() {
    super::protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::executor_port::",
            "port_native_foreign_failed"
        ),
        port_native_foreign_failed_body(),
    )
    .await;
}

async fn port_native_foreign_failed_body() {
    foreign_control(true, false, true).await;
    foreign_control(false, false, true).await;
    super::protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::executor_port::",
        "port_native_foreign_failed"
    ));
}

#[tokio::test]
async fn port_native_foreign_closing() {
    super::protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::executor_port::",
            "port_native_foreign_closing"
        ),
        port_native_foreign_closing_body(),
    )
    .await;
}

async fn port_native_foreign_closing_body() {
    foreign_control(true, false, true).await;
    foreign_control(true, true, true).await;
    super::protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::executor_port::",
        "port_native_foreign_closing"
    ));
}

#[tokio::test]
async fn port_native_foreign_unretained() {
    super::protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::executor_port::",
            "port_native_foreign_unretained"
        ),
        port_native_foreign_unretained_body(),
    )
    .await;
}

async fn port_native_foreign_unretained_body() {
    foreign_control(true, false, true).await;
    foreign_control(true, false, false).await;
    super::protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::executor_port::",
        "port_native_foreign_unretained"
    ));
}

#[tokio::test]
async fn port_native_protected_unknown() {
    super::protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::executor_port::",
            "port_native_protected_unknown"
        ),
        port_native_protected_unknown_body(),
    )
    .await;
}

async fn port_native_protected_unknown_body() {
    use super::protected_carrier::ProtectedFixture;
    use crate::config::sessions_persistence::{
        classify_lifecycle_carrier_scope, CreateCarrierScope,
    };
    let f = ProtectedFixture::new(true).await;
    let n = &f.native;
    let executor = n.executor();
    let t = n.transaction();
    let entry = n.coordinator.port_test_pending_entry(n.request()).unwrap();
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    assert!(matches!(
        classify_lifecycle_carrier_scope(
            &permit,
            Some(&f.route.target.canonical_fqn),
            &f.route.target.replica_identity.canonical_path,
        ),
        CreateCarrierScope::Protected { .. }
    ));
    drop(permit);
    let lease = n.prepare(entry.operation()).await;
    assert!(executor.has_claim(&lease));
    let receipt = native_receipt(&lease);
    let released = executor.release(lease);
    assert_eq!(receipt.load(Ordering::Acquire), 1);
    assert!(released);
    let parent = f.route.target.replica_root.parent().unwrap();
    let unknown = f.parent_source.unwrap();
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let canonical = crate::path_identity::verify_directory(parent)
        .unwrap()
        .canonical_path;
    assert!(!canonical.starts_with(&f.route.target.replica_identity.canonical_path));
    assert!(matches!(
        classify_lifecycle_carrier_scope(&permit, None, &canonical),
        CreateCarrierScope::Waiting
    ));
    drop(permit);
    let mut request = n.request();
    request.session_id = unknown;
    let unknown_entry = n.coordinator.port_test_pending_entry(request).unwrap();
    n.pty.lock().unwrap().response_close_state().fail_closed();
    for (source, operation) in [
        (n.source, entry.operation()),
        (unknown, unknown_entry.operation()),
    ] {
        assert!(matches!(
            executor
                .prepare(&t, operation, Some(source), false, None)
                .await
                .unwrap(),
            LifecycleAdmission::Waiting
        ));
        assert_eq!(n.backend.kill_count(source), 0);
        assert!(n.manager.read().await.get_session(source).await.is_some());
    }
    drop(f.carrier);
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    assert!(matches!(
        classify_lifecycle_carrier_scope(&permit, None, &canonical),
        CreateCarrierScope::Foreign { .. }
    ));
    drop(permit);
    let LifecycleAdmission::Ready(lease) = executor
        .prepare(&t, unknown_entry.operation(), Some(unknown), false, None)
        .await
        .unwrap()
    else {
        panic!("same failed tracker Foreign fallback required")
    };
    assert!(!executor.has_claim(&lease));
    let receipt = native_receipt(&lease);
    let released = executor.release(lease);
    assert_eq!(receipt.load(Ordering::Acquire), 1);
    assert!(released);
    let _report = n.coordinator.close_and_join().await;
    super::protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::executor_port::",
        "port_native_protected_unknown"
    ));
}
