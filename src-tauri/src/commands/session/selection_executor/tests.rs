//! Native adapter rejects malformed leases before accessing native state.

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

mod executor_port;
