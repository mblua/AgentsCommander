//! Native adapter rejects malformed leases before accessing native state.

use self::protected_carrier::ProtectedFixture;
use super::*;

fn lease(empty_native: bool) -> (LifecycleLease, Arc<AtomicU8>) {
    let receipt = Arc::new(AtomicU8::new(0));
    let payload: Box<dyn std::any::Any + Send + Sync> = if empty_native {
        Box::new(NativeLifecycleLease {
            authority: None,
            release_receipt: Arc::clone(&receipt),
        })
    } else {
        Box::new(())
    };
    (LifecycleLease::new(payload, Arc::clone(&receipt)), receipt)
}

async fn assert_rejected(empty_native: bool) {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let transaction = SelectionTransaction::for_test(app.handle().clone());
    let executor: &dyn LifecycleExecutor<SelectionTransaction<tauri::test::MockRuntime>> =
        &NativeLifecycleExecutor;
    let expected_receipt = if empty_native { 2 } else { 0 };

    let (mut candidate, receipt) = lease(empty_native);
    assert!(!executor.validate(&transaction, &mut candidate).await);
    assert_eq!(receipt.load(Ordering::Acquire), 0);
    drop(candidate);
    assert_eq!(receipt.load(Ordering::Acquire), expected_receipt);

    let (candidate, receipt) = lease(empty_native);
    assert!(!executor.has_claim(&candidate));
    drop(candidate);
    assert_eq!(receipt.load(Ordering::Acquire), expected_receipt);

    let (candidate, receipt) = lease(empty_native);
    assert!(!executor.release(candidate));
    assert_eq!(receipt.load(Ordering::Acquire), expected_receipt);

    let (candidate, receipt) = lease(empty_native);
    let request = RestartJobRequest {
        session_id: Uuid::new_v4(),
        agent_id: None,
        requested_profile: None,
        skip_auto_resume: None,
        activate_after: false,
        intent: crate::session::selection::TrustedRestartIntent::User,
        communication_override: None,
        enforcement: crate::config::sessions_persistence::CreationGateEnforcement::Enforce,
    };
    assert!(matches!(
        executor.restart(&transaction, request, &candidate).await,
        LifecycleEntryOutcome::Interrupted
    ));
    drop(candidate);
    assert_eq!(receipt.load(Ordering::Acquire), expected_receipt);

    let (candidate, receipt) = lease(empty_native);
    let request = RootJobRequest {
        requested_agent_id: None,
        requested_profile: None,
        skip_auto_resume_for_new_session: false,
        intent: crate::session::selection::TrustedCreateIntent::User,
        select_after: false,
    };
    assert!(matches!(
        executor.root(&transaction, request, &candidate).await,
        LifecycleEntryOutcome::Interrupted
    ));
    drop(candidate);
    assert_eq!(receipt.load(Ordering::Acquire), expected_receipt);
}

#[tokio::test]
async fn wrong_payload_rejects_all_native_lease_operations() {
    assert_rejected(false).await;
}

#[tokio::test]
async fn empty_native_payload_rejects_all_native_lease_operations() {
    assert_rejected(true).await;
}

#[tokio::test]
async fn port_native_wrong_payload_lease() {
    assert_rejected(false).await;
}

async fn assert_ordinary_target_released(f: &native_fixture::NativeFixture, source: Uuid) {
    let executor = f.executor();
    let transaction = f.transaction();
    let LifecycleAdmission::Ready(lease) = executor
        .prepare(&transaction, Uuid::new_v4(), Some(source), false, None)
        .await
        .unwrap()
    else {
        panic!("original target must be reacquired")
    };
    assert!(executor.has_claim(&lease));
    let receipt = native_fixture::native_receipt(&lease);
    let released = executor.release(lease);
    assert_eq!(receipt.load(Ordering::Acquire), 1);
    assert!(released);
}

async fn public_native_case(root: bool, interrupted: bool) {
    use native_fixture::{root_request, NativeFixture};
    let f = NativeFixture::running().await;
    prepare_public_native_root(&f, root, interrupted).await;
    let initial = crate::path_identity::verify_directory(f.cwd.path()).unwrap();
    let spawns = f.backend.spawn_count();
    let (at_a, resume_a) = f.schedule.gate(false);
    let (at_b, resume_b) = f.schedule.gate(true);
    let coordinator = f.coordinator.clone();
    let app = f.app.handle().clone();
    let mut request = f.request();
    request.skip_auto_resume = Some(true);
    let caller = tokio::spawn(async move {
        if root {
            coordinator.root_lifecycle(app, root_request()).await
        } else {
            coordinator.restart_lifecycle(app, request).await
        }
    });
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(20), at_a)
            .await
            .unwrap()
            .unwrap(),
        (true, true, 0)
    );
    let backup = f.cwd.path().with_extension("native-backup");
    if interrupted {
        std::fs::rename(f.cwd.path(), &backup).unwrap();
        std::fs::create_dir(f.cwd.path()).unwrap();
        assert_ne!(
            initial.object_id,
            crate::path_identity::verify_directory(f.cwd.path())
                .unwrap()
                .object_id
        );
    }
    resume_a.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(20), at_b)
            .await
            .unwrap()
            .unwrap(),
        (!interrupted, true, 0)
    );
    let (receipt, trace, operation) = public_native_receipts(&f, root, interrupted);
    resume_b.send(()).unwrap();
    let mut caller = Some(caller);
    let result = if interrupted {
        None
    } else {
        Some(caller.take().unwrap().await.unwrap().unwrap())
    };
    f.coordinator.snapshot().await.unwrap();
    assert_eq!(receipt.load(Ordering::Acquire), 1);
    let events = assert_public_native_trace(&trace, operation, root, interrupted);
    if root || interrupted {
        assert_eq!(f.backend.spawn_count(), spawns);
    }
    if interrupted {
        let caller = caller.take().unwrap();
        assert!(!caller.is_finished());
        assert_public_native_interrupted(&f, root, &backup, &initial).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
    } else {
        assert_public_native_success(&f, root, result.unwrap(), &initial).await;
    }
    reacquire_public_native_target(&f, root, interrupted, operation, events[0].1).await;
    let state = f.coordinator.port_test_state();
    assert_eq!(
        (
            state.admission_available,
            state.queue_capacity,
            state.create_tickets_available
        ),
        (65, 64, 16)
    );
    let _report = f.coordinator.close_and_join().await;
}

#[tokio::test]
async fn port_native_release_paths_public_restart_success() {
    protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::",
            "port_native_release_paths_public_restart_success"
        ),
        port_native_release_paths_public_restart_success_body(),
    )
    .await;
}

async fn port_native_release_paths_public_restart_success_body() {
    public_native_case(false, false).await;
    protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::",
        "port_native_release_paths_public_restart_success"
    ));
}
#[tokio::test]
async fn port_native_release_paths_public_restart_interrupted() {
    protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::",
            "port_native_release_paths_public_restart_interrupted"
        ),
        port_native_release_paths_public_restart_interrupted_body(),
    )
    .await;
}

async fn port_native_release_paths_public_restart_interrupted_body() {
    public_native_case(false, true).await;
    protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::",
        "port_native_release_paths_public_restart_interrupted"
    ));
}
#[tokio::test]
async fn port_native_release_paths_public_root_success() {
    protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::",
            "port_native_release_paths_public_root_success"
        ),
        port_native_release_paths_public_root_success_body(),
    )
    .await;
}

async fn port_native_release_paths_public_root_success_body() {
    public_native_case(true, false).await;
    protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::",
        "port_native_release_paths_public_root_success"
    ));
}
#[tokio::test]
async fn port_native_release_paths_public_root_interrupted() {
    protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::",
            "port_native_release_paths_public_root_interrupted"
        ),
        port_native_release_paths_public_root_interrupted_body(),
    )
    .await;
}

async fn port_native_release_paths_public_root_interrupted_body() {
    public_native_case(true, true).await;
    protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::",
        "port_native_release_paths_public_root_interrupted"
    ));
}

#[tokio::test]
async fn port_native_trace_restore() {
    protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::",
            "port_native_trace_restore"
        ),
        port_native_trace_restore_body(),
    )
    .await;
}

async fn port_native_trace_restore_body() {
    use native_fixture::NativeFixture;
    let mut f = NativeFixture::build(false).await;
    f.coordinator.start(f.app.handle().clone()).unwrap();
    let guard = f.coordinator.submit_restore_first().await.unwrap();
    let transaction = guard.transaction(f.app.handle().clone());
    let nonce = transaction.port_test_restore_nonce();
    assert!(nonce.is_some_and(|id| id != Uuid::nil()));
    assert_eq!(nonce, f.coordinator.port_test_active_restore_nonce());
    assert!(transaction.has_active_restore_capability());
    f.source = f.restore_source(&transaction).await;
    assert_restored_native_source(&f).await;
    let (at_a, resume_a) = f.schedule.gate(false);
    let (at_b, resume_b) = f.schedule.gate(true);
    let coordinator = f.coordinator.clone();
    let app = f.app.handle().clone();
    let mut request = f.request();
    request.skip_auto_resume = Some(true);
    let caller = tokio::spawn(async move { coordinator.restart_lifecycle(app, request).await });
    let (operation, executor_nonce, stage) =
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                if let Some(entry) = f.coordinator.port_test_retained_entry(f.source) {
                    break entry;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    assert_ne!(operation, Uuid::nil());
    assert_ne!(executor_nonce, Uuid::nil());
    assert_eq!(stage, "EntryWaiting");
    assert!(f.receipt_records().is_empty());
    assert_eq!(nonce, f.coordinator.port_test_active_restore_nonce());
    guard.finish();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(20), at_a)
            .await
            .unwrap()
            .unwrap(),
        (true, true, 0)
    );
    resume_a.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(20), at_b)
            .await
            .unwrap()
            .unwrap(),
        (true, true, 0)
    );
    let receipt = {
        let records = f.receipt_records();
        let record = records.iter().find(|r| r.operation == operation).unwrap();
        assert_eq!(record.outcome, Some((true, true)));
        assert_eq!(record.receipt.load(Ordering::Acquire), 0);
        record.receipt.clone()
    };
    resume_b.send(()).unwrap();
    let result = caller.await.unwrap().unwrap();
    assert_restored_native_release(&f, &transaction, &result, &receipt).await;
    let _report = f.coordinator.close_and_join().await;
    protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::",
        "port_native_trace_restore"
    ));
}

#[tokio::test]
async fn port_native_trace_caller_loss() {
    protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::",
            "port_native_trace_caller_loss"
        ),
        port_native_trace_caller_loss_body(),
    )
    .await;
}

async fn port_native_trace_caller_loss_body() {
    use crate::pty::backend::PtyBackend;
    use native_fixture::NativeFixture;
    let f = NativeFixture::running().await;
    let (spawned, resume_spawn) = f.backend.gate_spawn();
    let (at_b, resume_b) = f.schedule.gate(true);
    let coordinator = f.coordinator.clone();
    let app = f.app.handle().clone();
    let mut request = f.request();
    request.skip_auto_resume = Some(true);
    let caller = tokio::spawn(async move { coordinator.restart_lifecycle(app, request).await });
    let replacement = tokio::time::timeout(std::time::Duration::from_secs(20), spawned)
        .await
        .unwrap()
        .unwrap();
    let receipt = {
        let records = f.receipt_records();
        let matched: Vec<_> = records
            .iter()
            .filter(|r| r.source == Some(f.source))
            .collect();
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].receipt.load(Ordering::Acquire), 0);
        assert_eq!(matched[0].outcome, None);
        matched[0].receipt.clone()
    };
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(receipt.load(Ordering::Acquire), 0);
    resume_spawn.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(20), at_b)
            .await
            .unwrap()
            .unwrap(),
        (true, true, 0)
    );
    {
        let records = f.receipt_records();
        let record = records
            .iter()
            .find(|r| Arc::ptr_eq(&r.receipt, &receipt))
            .unwrap();
        assert_eq!(record.outcome, Some((true, true)));
    }
    resume_b.send(()).unwrap();
    f.coordinator.snapshot().await.unwrap();
    assert_eq!(receipt.load(Ordering::Acquire), 1);
    assert!(f.backend.has_session(replacement));
    assert!(f
        .manager
        .read()
        .await
        .get_session(replacement)
        .await
        .is_some());
    let _report = f.coordinator.close_and_join().await;
    protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::",
        "port_native_trace_caller_loss"
    ));
}

#[tokio::test]
async fn port_native_trace_root_directory() {
    protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::",
            "port_native_trace_root_directory"
        ),
        port_native_trace_root_directory_body(),
    )
    .await;
}

async fn port_native_trace_root_directory_body() {
    use crate::session::selection::LifecycleEffectSlot;
    use native_fixture::NativeFixture;
    let f = NativeFixture::running().await;
    f.manager
        .read()
        .await
        .set_is_root_agent(f.source, true)
        .await;
    let executor = f.executor();
    let transaction = f.transaction();
    assert!(matches!(
        executor
            .prepare(&transaction, Uuid::new_v4(), Some(f.source), false, None,)
            .await
            .unwrap(),
        LifecycleAdmission::NeedsRootDirectory
    ));
    let configured = crate::config::root_agent::ensure_root_agent_dir().unwrap();
    for duplicate in [false, true] {
        let mut entry = f.coordinator.port_test_pending_entry(f.request()).unwrap();
        let LifecycleAdmission::Ready(lease) = executor
            .prepare(
                &transaction,
                entry.operation(),
                Some(f.source),
                false,
                Some(&configured),
            )
            .await
            .unwrap()
        else {
            panic!("real root authority required")
        };
        let receipt = native_fixture::native_receipt(&lease);
        entry.attach_lease(lease);
        let entered = entry.enter(&transaction);
        let effect = entered
            .start_lifecycle_effect(LifecycleEffectSlot::RootDirectory)
            .unwrap();
        let path = executor.ensure_root_directory(effect).unwrap();
        assert_eq!(path, configured);
        assert_eq!(entered.lifecycle_root_directory_outcome(), Some(Ok(path)));
        if duplicate {
            assert!(entered
                .start_lifecycle_effect(LifecycleEffectSlot::RootDirectory)
                .is_none());
        }
        drop(entered);
        entry.drop_entry();
        assert_eq!(receipt.load(Ordering::Acquire), 1);
    }
    let _report = f.coordinator.close_and_join().await;
    protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::",
        "port_native_trace_root_directory"
    ));
}

mod executor_port;
mod native_fixture;
mod protected_carrier;

#[tokio::test]
async fn port_native_trace_waiting() {
    protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::",
            "port_native_trace_waiting"
        ),
        port_native_trace_waiting_body(),
    )
    .await;
}

async fn port_native_trace_waiting_body() {
    use crate::config::sessions_persistence::{classify_create_carrier_scope, CreateCarrierScope};
    use crate::pty::backend::PtyBackend;
    use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseRouteIdentity};
    let f = ProtectedFixture::new(false).await;
    let n = &f.native;
    let tracker = n.pty.lock().unwrap().response_close_state();
    let manager = n.manager.read().await.clone();
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let classification = manager
        .with_current_carrier_snapshot(&permit, |current| {
            let row = current.rows.iter().find(|row| row.id == n.source).unwrap();
            let CreateCarrierScope::Protected { epoch, .. } = classify_create_carrier_scope(
                &permit,
                &f.route.target.canonical_fqn,
                &f.route.target.replica_root,
            ) else {
                panic!("actual Protected classification required")
            };
            CreateClassification::Protected {
                identity: ResponseRouteIdentity {
                    target: f.route.target.canonical_fqn.clone(),
                    session_id: n.source,
                    token: row.token,
                    carrier_version: current.versions[&row.id],
                    generation: tracker.generation(row.id).unwrap(),
                    backend: row.backend_kind,
                },
                publication_epoch: epoch,
            }
        })
        .await;
    let CreateAdmission::Reserved(held) = tracker
        .reserve_create_owner(
            &f.route.target.canonical_fqn,
            Uuid::new_v4(),
            classification,
        )
        .unwrap()
    else {
        panic!("actual conflict reservation required")
    };
    drop(permit);
    let (at_b, resume_b) = n.schedule.gate(true);
    let coordinator = n.coordinator.clone();
    let app = n.app.handle().clone();
    let mut request = n.request();
    request.skip_auto_resume = Some(true);
    let caller = tokio::spawn(async move { coordinator.restart_lifecycle(app, request).await });
    tokio::time::timeout(std::time::Duration::from_secs(20), n.schedule.waiting())
        .await
        .unwrap();
    assert!(!caller.is_finished());
    assert!(n.receipt_records().is_empty());
    assert_eq!(tokio::spawn(async { 7 }).await.unwrap(), 7);
    drop(held);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(20), at_b)
            .await
            .unwrap()
            .unwrap(),
        (true, true, 0)
    );
    let records = n.receipt_records();
    let matched: Vec<_> = records
        .iter()
        .filter(|r| r.source == Some(n.source))
        .collect();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].outcome, Some((true, true)));
    let receipt = matched[0].receipt.clone();
    resume_b.send(()).unwrap();
    let result = caller.await.unwrap().unwrap();
    n.coordinator.snapshot().await.unwrap();
    assert_eq!(receipt.load(Ordering::Acquire), 1);
    assert_ne!(result.id, n.source.to_string());
    assert!(n.backend.has_session(Uuid::parse_str(&result.id).unwrap()));
    let _report = n.coordinator.close_and_join().await;
    protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::",
        "port_native_trace_waiting"
    ));
}

#[tokio::test]
async fn port_native_wrong_payload_fixture() {
    protected_carrier::run_isolated(
        concat!(
            "commands::session::selection_executor::tests::",
            "port_native_wrong_payload_fixture"
        ),
        port_native_wrong_payload_fixture_body(),
    )
    .await;
}

async fn port_native_wrong_payload_fixture_body() {
    use crate::pty::backend::PtyBackend;
    use crate::session::selection::{
        port_test_binding, OwnedReplacementWork, ReplacementAdmissionStep,
        ReplacementFixtureControl, ReplacementFixtureOutcome, ResolvedReplacement,
    };
    let f = ProtectedFixture::new(false).await;
    tokio::time::timeout(std::time::Duration::from_secs(20), f.retire())
        .await
        .expect("actual retirement must finish");
    let n = &f.native;
    assert!(!n.backend.has_session(n.source));
    assert!(n.pty.lock().unwrap().backend_kind(n.source).is_none());
    let manager = n.manager.read().await.clone();
    let tracker = n.pty.lock().unwrap().response_close_state();
    let t = n.transaction();
    let mut reserved = None;
    let binding = t
        .prepare_inline_create_binding(port_test_binding(Uuid::new_v4()), &mut reserved)
        .unwrap();
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let continuation = manager
        .with_current_carrier_snapshot(&permit, |current| {
            let carrier = crate::config::sessions_persistence::retained_carrier_for_replacement(
                &permit,
                &crate::config::config_dir().unwrap(),
                &f.route.target.canonical_fqn,
                n.source,
                current,
            )
            .expect("real retired carrier");
            let previous = current
                .rows
                .iter()
                .find(|row| row.id == n.source)
                .unwrap()
                .clone();
            let cwd = previous.working_directory.clone();
            let prepared = n.coordinator.prepare_owned_replacement(
                ResolvedReplacement {
                    target: f.route.target.canonical_fqn.clone(),
                    directory: crate::config::config_dir().unwrap(),
                    shell: previous.shell.clone(),
                    shell_args: previous.shell_args.clone(),
                    requested_agent_id: previous.agent_id.clone(),
                    requested_profile: None,
                    launch_recipe: serde_json::json!({"native-port": "inert"}),
                    cwd_identity: crate::path_identity::verify_directory(std::path::Path::new(
                        &cwd,
                    ))
                    .unwrap()
                    .into(),
                    replica_identity: Some(f.route.target.replica_identity.clone().into()),
                    cwd,
                    backend: previous.backend_kind,
                    previous,
                    carrier,
                    work: OwnedReplacementWork::OrdinaryOwned {
                        message_id: "native-port".into(),
                        body: "inert".into(),
                        payload: serde_json::Value::Null,
                        recipient: f.route.target.canonical_fqn.clone(),
                        request: serde_json::Value::Null,
                    },
                },
                binding,
            );
            let ReplacementAdmissionStep::Accepted(continuation) = n
                .coordinator
                .admit_owned_replacement(&permit, current, &tracker, &n.pty, prepared)
                .unwrap()
            else {
                panic!("genuine replacement admission required")
            };
            continuation
        })
        .await;
    drop(permit);
    let control = Arc::new(ReplacementFixtureControl::default());
    let kills = n.backend.kill_count(n.source);
    let before = n.coordinator.snapshot().await.unwrap();
    let result = n
        .executor()
        .execute_fixture(
            &t,
            OpaqueFixtureInput {
                continuation: continuation.with_fixture_control(control),
                payload: Box::new(()),
            },
        )
        .await;
    assert_eq!(result.outcome, ReplacementFixtureOutcome::Error);
    let result = result
        .payload
        .downcast::<crate::commands::session::FixtureReplacementResult>()
        .expect("typed error result");
    assert!(result.outcome.is_err());
    assert!(result.ordinary_lease.is_none());
    assert_eq!(n.backend.kill_count(n.source), kills);
    assert!(!n.backend.has_session(binding.session_id()));
    assert!(manager.get_pending_session(binding).await.is_none());
    assert_eq!(n.coordinator.snapshot().await.unwrap(), before);
    let _report = n.coordinator.close_and_join().await;
    protected_carrier::complete_isolated(concat!(
        "commands::session::selection_executor::tests::",
        "port_native_wrong_payload_fixture"
    ));
}

async fn prepare_public_native_root(
    f: &native_fixture::NativeFixture,
    root: bool,
    interrupted: bool,
) {
    use crate::pty::backend::PtyBackend;
    use crate::pty::manager::PtyManager;
    if root {
        f.manager
            .read()
            .await
            .set_is_root_agent(f.source, true)
            .await;
        if interrupted {
            let teardown = crate::commands::session::restart::teardown_old_for_restart(
                &f.transaction(),
                f.source,
                false,
            )
            .await;
            assert!(teardown.is_ok());
            f.manager.read().await.mark_exited(f.source, 0).await;
            let row = f.manager.read().await.get_session(f.source).await.unwrap();
            assert!(matches!(
                row.status,
                crate::session::session::SessionStatus::Exited(_)
            ));
            assert!(matches!(
                PtyManager::snapshot_route_proof(&f.pty, f.source),
                Err(crate::errors::AppError::SessionNotFound(id)) if id == f.source.to_string()
            ));
            assert!(!f.backend.has_session(f.source));
        }
    }
}

fn public_native_receipts(
    f: &native_fixture::NativeFixture,
    root: bool,
    interrupted: bool,
) -> (
    Arc<AtomicU8>,
    crate::session::selection::SelectionPortTestTrace,
    Uuid,
) {
    use crate::session::selection::SelectionPortTestEvent as Event;
    let records = f.receipt_records();
    let matched: Vec<_> = records
        .iter()
        .filter(|r| r.source == if root { None } else { Some(f.source) })
        .collect();
    assert_eq!(matched.len(), 1);
    let record = matched[0];
    assert_eq!(record.outcome, Some((!interrupted, !interrupted)));
    assert_eq!(record.receipt.load(Ordering::Acquire), 0);
    assert!(record.claimed);
    assert!(!record
        .trace
        .lock()
        .unwrap()
        .iter()
        .any(|(_, _, event)| matches!(
            event,
            Event::TransactionDropped
                | Event::EntryDropped
                | Event::HoldReleased
                | Event::ResponseDropped
                | Event::ResponseAttempted
        )));
    (
        record.receipt.clone(),
        record.trace.clone(),
        record.operation,
    )
}

fn assert_public_native_trace(
    trace: &crate::session::selection::SelectionPortTestTrace,
    operation: Uuid,
    root: bool,
    interrupted: bool,
) -> Vec<(
    Uuid,
    Uuid,
    crate::session::selection::SelectionPortTestEvent,
)> {
    use crate::session::selection::SelectionPortTestEvent as Event;
    let mut expected = if interrupted {
        vec![
            Event::Validate(true),
            Event::Accepted(true),
            Event::NativeEntered,
            Event::NativeOutcome(false, false),
            Event::TransactionDropped,
            Event::EntryDropped,
            Event::HoldReleased,
            Event::ResponseDropped,
        ]
    } else {
        vec![
            Event::Validate(true),
            Event::Accepted(true),
            Event::NativeEntered,
            Event::NativeOutcome(true, true),
            Event::ReleaseAwaitBefore,
            Event::ReleaseAwaitAfter,
            Event::TransactionDropped,
            Event::EntryDropped,
            Event::HoldReleased,
            Event::ResponseAttempted,
        ]
    };
    if root {
        expected.insert(0, Event::RootDirectoryComplete(true));
    }
    let events = trace.lock().unwrap().clone();
    assert_eq!(
        events.iter().map(|(_, _, e)| *e).collect::<Vec<_>>(),
        expected
    );
    assert!(events
        .iter()
        .all(|(op, nonce, _)| *op == operation && *nonce == events[0].1));
    assert_ne!(events[0].1, Uuid::nil());
    events
}

async fn assert_public_native_interrupted(
    f: &native_fixture::NativeFixture,
    root: bool,
    backup: &std::path::Path,
    initial: &crate::path_identity::VerifiedPathIdentity,
) {
    assert!(f.manager.read().await.get_session(f.source).await.is_some());
    assert_eq!(f.backend.kill_count(f.source), usize::from(root));
    std::fs::remove_dir(f.cwd.path()).unwrap();
    std::fs::rename(backup, f.cwd.path()).unwrap();
    assert_eq!(
        initial.object_id,
        crate::path_identity::verify_directory(f.cwd.path())
            .unwrap()
            .object_id
    );
}

async fn assert_public_native_success(
    f: &native_fixture::NativeFixture,
    root: bool,
    row: crate::session::session::SessionInfo,
    initial: &crate::path_identity::VerifiedPathIdentity,
) {
    use crate::pty::backend::PtyBackend;
    let id = Uuid::parse_str(&row.id).unwrap();
    assert_eq!(id == f.source, root);
    assert!(f.backend.has_session(id));
    if root {
        assert_eq!(f.backend.kill_count(f.source), 0);
        assert_eq!(
            initial.object_id,
            crate::path_identity::verify_directory(f.cwd.path())
                .unwrap()
                .object_id
        );
    } else {
        assert!(f.manager.read().await.get_session(f.source).await.is_none());
        assert_ordinary_target_released(f, id).await;
    }
}

async fn reacquire_public_native_target(
    f: &native_fixture::NativeFixture,
    root: bool,
    interrupted: bool,
    operation: Uuid,
    nonce: Uuid,
) {
    let executor = f.executor();
    let transaction = f.transaction();
    if root {
        let path = {
            let outcomes = f.schedule.root_outcomes();
            let matched: Vec<_> = outcomes
                .iter()
                .filter(|(op, _, _)| *op == operation)
                .collect();
            assert_eq!(matched.len(), 1);
            assert_eq!(matched[0].1, nonce);
            matched[0].2.clone().unwrap()
        };
        let LifecycleAdmission::Ready(lease) = executor
            .prepare(&transaction, Uuid::new_v4(), None, true, Some(&path))
            .await
            .unwrap()
        else {
            panic!("original root target must be reacquired")
        };
        assert!(executor.has_claim(&lease));
        let receipt = native_fixture::native_receipt(&lease);
        let released = executor.release(lease);
        assert_eq!(receipt.load(Ordering::Acquire), 1);
        assert!(released);
    } else {
        let source = if interrupted {
            f.source
        } else {
            f.create_source().await
        };
        assert_ordinary_target_released(f, source).await;
    }
}

async fn assert_restored_native_source(f: &native_fixture::NativeFixture) {
    use crate::pty::backend::PtyBackend;
    use crate::pty::manager::PtyManager;
    let row = f.manager.read().await.get_session(f.source).await.unwrap();
    assert!(!matches!(
        row.status,
        crate::session::session::SessionStatus::Exited(_)
    ));
    assert!(!row.is_root_agent);
    assert!(PtyManager::snapshot_route_proof(&f.pty, f.source)
        .unwrap()
        .saved_cwd_matches_path(f.cwd.path()));
    let manager = f.manager.read().await.clone();
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    assert!(
        manager
            .with_current_carrier_snapshot(&permit, |current| !current
                .pending_ids
                .contains(&f.source))
            .await
    );
    drop(permit);
    assert!(f.backend.has_session(f.source));
}

async fn assert_restored_native_release(
    f: &native_fixture::NativeFixture,
    transaction: &SelectionTransaction<tauri::test::MockRuntime>,
    result: &crate::session::session::SessionInfo,
    receipt: &AtomicU8,
) {
    use crate::pty::backend::PtyBackend;
    assert_ne!(result.id, f.source.to_string());
    f.coordinator.snapshot().await.unwrap();
    assert_eq!(receipt.load(Ordering::Acquire), 1);
    assert_eq!(f.coordinator.port_test_active_restore_nonce(), None);
    assert!(!transaction.has_active_restore_capability());
    assert!(f.backend.has_session(Uuid::parse_str(&result.id).unwrap()));
    assert!(f.manager.read().await.get_session(f.source).await.is_none());
    assert_ordinary_target_released(f, Uuid::parse_str(&result.id).unwrap()).await;
    let original = f.create_source().await;
    assert_ordinary_target_released(f, original).await;
}
