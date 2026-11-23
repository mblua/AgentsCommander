use super::*;
use std::cell::Cell;
use uuid::Uuid;

// ---- load settings only when a user-input transition will apply ----

async fn lazy_session(mgr: &SessionManager) -> Uuid {
    mgr.create_session(
        "powershell.exe".to_string(),
        Vec::new(),
        "C:\\tmp".to_string(),
        None,
        None,
        Vec::new(),
        true,
        crate::pty::backend::SessionBackendKind::LocalProcess,
    )
    .await
    .expect("create_session should succeed")
    .id
}

fn saved_rows(dir: &std::path::Path) -> Vec<PersistedSession> {
    let saved = std::fs::read_to_string(dir.join("sessions.json")).expect("read sessions.json");
    serde_json::from_str(&saved).expect("deserialize")
}

#[tokio::test]
async fn lazy_clear_skips_settings_when_nothing_changes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let id = lazy_session(&mgr).await;
    assert!(
        !mgr.get_session(id)
            .await
            .expect("session")
            .start_fresh_on_restore
    );
    let calls = Cell::new(0);
    let load = || {
        calls.set(calls.get() + 1);
        Ok(Vec::new())
    };

    let cleared = clear_user_input_transitions_and_persist_lazy_result(
        &mgr,
        id,
        true,
        Some(temp.path()),
        load,
    )
    .await
    .expect("lazy clear should succeed");
    assert!(!cleared.cleared_start_fresh);
    assert!(!cleared.cleared_raise_hand);
    assert_eq!(calls.get(), 0);
    assert!(!temp.path().join("sessions.json").exists());
}

#[tokio::test]
async fn lazy_clear_skips_settings_for_unknown_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let calls = Cell::new(0);
    let load = || {
        calls.set(calls.get() + 1);
        Ok(Vec::new())
    };

    let cleared = clear_user_input_transitions_and_persist_lazy_result(
        &mgr,
        Uuid::new_v4(),
        true,
        Some(temp.path()),
        load,
    )
    .await
    .expect("lazy clear should succeed");
    assert!(!cleared.cleared_start_fresh);
    assert!(!cleared.cleared_raise_hand);
    assert_eq!(calls.get(), 0);
}

/// Positive control for the two tests above: same counter, same call.
#[tokio::test]
async fn lazy_clear_loads_settings_once_and_lowers_hand() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let id = lazy_session(&mgr).await;
    mgr.set_start_fresh_on_restore(id, true).await;
    mgr.raise_hand(id, chrono::Utc::now())
        .await
        .expect("raise_hand should succeed");
    let calls = Cell::new(0);
    let load_root = || {
        calls.set(calls.get() + 1);
        Ok(vec!["C:\\tmp".to_string()])
    };

    let cleared = clear_user_input_transitions_and_persist_lazy_result(
        &mgr,
        id,
        false,
        Some(temp.path()),
        load_root,
    )
    .await
    .expect("lazy clear should succeed");
    assert!(cleared.cleared_raise_hand);
    assert!(!cleared.cleared_start_fresh);
    assert_eq!(calls.get(), 1);
    let rows = saved_rows(temp.path());
    assert_eq!(rows.len(), 1);
    assert!(rows[0].communication.is_none());
    assert!(rows[0].start_fresh_on_restore);
}

#[tokio::test]
async fn lazy_clear_loads_settings_once_and_clears_fresh() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let id = lazy_session(&mgr).await;
    mgr.set_start_fresh_on_restore(id, true).await;
    let calls = Cell::new(0);
    let load_root = || {
        calls.set(calls.get() + 1);
        Ok(vec!["C:\\tmp".to_string()])
    };

    let cleared = clear_user_input_transitions_and_persist_lazy_result(
        &mgr,
        id,
        true,
        Some(temp.path()),
        load_root,
    )
    .await
    .expect("lazy clear should succeed");
    assert!(cleared.cleared_start_fresh);
    assert_eq!(calls.get(), 1);
    let rows = saved_rows(temp.path());
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].start_fresh_on_restore);

    // N3: a new manager and a new counter for the `clear_fresh = false` case.
    let temp2 = tempfile::tempdir().expect("tempdir");
    let mgr2 = SessionManager::new();
    let id2 = lazy_session(&mgr2).await;
    mgr2.set_start_fresh_on_restore(id2, true).await;
    let calls2 = Cell::new(0);
    let load_root2 = || {
        calls2.set(calls2.get() + 1);
        Ok(vec!["C:\\tmp".to_string()])
    };

    let cleared = clear_user_input_transitions_and_persist_lazy_result(
        &mgr2,
        id2,
        false,
        Some(temp2.path()),
        load_root2,
    )
    .await
    .expect("lazy clear should succeed");
    assert!(!cleared.cleared_start_fresh);
    assert_eq!(calls2.get(), 0);
    let live = mgr2.get_session(id2).await.expect("session");
    assert!(live.start_fresh_on_restore);
}
