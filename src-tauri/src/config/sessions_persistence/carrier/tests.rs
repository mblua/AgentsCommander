use super::{restore_carrier_scopes_are_empty, restore_test_carrier_scopes_are_empty};
use crate::config::sessions_persistence::{persist_initial_carrier, InitialCarrierIntent};
use crate::pty::backend::SessionBackendKind;
use crate::session::manager::{acquire_carrier_mutation_permit, SessionManager};
use crate::session::selection::{SelectionCoordinator, SelectionCoordinatorError};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn restore_bootstrap_isolates_mock_apps_but_keeps_same_scope_protection() {
    let directory = tempfile::tempdir().expect("publication directory");
    let cwd = tempfile::tempdir().expect("carrier replica");
    let manager = Arc::new(RwLock::new(SessionManager::new()));
    let owner = SelectionCoordinator::new(manager.clone(), CancellationToken::new());
    let owner_app = tauri::test::mock_builder()
        .manage(manager.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("owner app");
    owner
        .start(owner_app.handle().clone())
        .expect("start owner");
    let scope = owner.restore_mint_exclusion().expect("owner restore scope");
    // Mint through real persistence, then model a bootstrap while that same
    // app still owns a live protection. The production publication gate remains
    // held by the normal APIs, never bypassed by this fixture.
    scope.store(false, Ordering::Release);
    let handle = manager.read().await.clone();
    let row = handle
        .create_session(
            "bash".into(),
            vec![],
            cwd.path().to_string_lossy().into_owned(),
            None,
            None,
            vec![],
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("carrier row");
    handle.set_start_fresh_on_restore(row.id, true).await;
    let permit = acquire_carrier_mutation_permit().await;
    let version = handle
        .with_current_carrier_snapshot(&permit, |snapshot| snapshot.versions[&row.id])
        .await;
    drop(permit);
    let intent = InitialCarrierIntent::new(
        format!("bootstrap-test:{}", uuid::Uuid::new_v4()),
        row.id,
        row.token,
        version,
        std::fs::canonicalize(cwd.path()).expect("canonical replica"),
    )
    .with_restore_exclusion(scope.clone());
    let protection = persist_initial_carrier(
        &handle,
        directory.path().to_path_buf(),
        vec![cwd.path().to_string_lossy().into_owned()],
        intent,
    )
    .await
    .expect("persist carrier")
    .expect("confirmed protection");
    scope.store(true, Ordering::Release);

    let other_manager = Arc::new(RwLock::new(SessionManager::new()));
    let other = SelectionCoordinator::new(other_manager.clone(), CancellationToken::new());
    let other_app = tauri::test::mock_builder()
        .manage(other_manager)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("independent app");
    other
        .start(other_app.handle().clone())
        .expect("start other");
    let other_scope = other.restore_mint_exclusion().expect("other restore scope");
    let permit = acquire_carrier_mutation_permit().await;
    assert_eq!(
        restore_carrier_scopes_are_empty(&permit),
        Some(false),
        "the unchanged production ALL-scopes check sees the real protection"
    );
    assert_eq!(
        restore_test_carrier_scopes_are_empty(&permit, &scope),
        Some(false)
    );
    assert_eq!(
        restore_test_carrier_scopes_are_empty(&permit, &other_scope),
        Some(true)
    );
    drop(permit);

    let blocked = owner.submit_restore_first().await;
    assert!(matches!(blocked, Err(error) if error == SelectionCoordinatorError::Busy.to_string()));
    // Before #3233 this returns selectionCoordinatorBusy even with a fresh
    // phase, available admission and an empty channel: only the foreign
    // publication protection makes the old ALL-scopes bootstrap fail.
    let independent = other.submit_restore_first().await;
    assert!(
        independent.is_ok(),
        "independent bootstrap: {:?}",
        independent.err()
    );
    independent.unwrap().finish();
    other.close_and_join().await;
    owner.close_and_join().await;
    drop(protection);
}
