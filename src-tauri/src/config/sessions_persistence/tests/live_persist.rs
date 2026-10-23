use super::*;

// ---- #698 grinch HIGH: atomic raise-hand + persist with rollback ----

/// Happy path: a coordinator's raised hand reaches `sessions.json` through the
/// single save-lock acquisition.
#[tokio::test]
async fn raise_hand_and_persist_writes_raised_state_to_disk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "powershell.exe".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true, // coordinator
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    let now = chrono::DateTime::parse_from_rfc3339("2026-06-30T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    let outcome = raise_hand_and_persist_to_dir_result(&mgr, session.id, now, temp.path(), None)
        .await
        .expect("raise-hand persist should succeed");
    assert!(matches!(outcome, RaiseHandPersistOutcome::Raised(_)));

    // Live state is raised...
    let live = mgr.list_sessions().await;
    let live_comm = live[0].communication.as_ref().expect("live communication");
    assert_eq!(live_comm.kind, SessionCommunicationKind::RaiseHand);
    assert!(live_comm.visible);

    // ...and so is the durable snapshot.
    let saved =
        std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
    let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
    assert_eq!(rows.len(), 1);
    let comm = rows[0]
        .communication
        .as_ref()
        .expect("communication persisted");
    assert_eq!(comm.kind, SessionCommunicationKind::RaiseHand);
    assert!(comm.visible);
}

/// HIGH grinch fix: when the snapshot save fails, the live raise is rolled back
/// under the same lock, so a raise that did not survive is visible nowhere
/// (memory or disk). Without the rollback, `list-sessions` could report
/// `raisedHand:true` for a raise that never persisted.
#[tokio::test]
async fn raise_hand_and_persist_rolls_back_live_state_on_save_failure() {
    let temp = tempfile::tempdir().expect("tempdir");
    // Point the "dir" at an existing FILE so `save_sessions_to_dir`'s
    // `create_dir_all` fails and the save errors out deterministically.
    let file_as_dir = temp.path().join("sessions-dir-is-a-file");
    std::fs::write(&file_as_dir, "not a directory").expect("write file target");

    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");

    let result = raise_hand_and_persist_to_dir_result(
        &mgr,
        session.id,
        chrono::Utc::now(),
        &file_as_dir,
        None,
    )
    .await;
    assert!(result.is_err(), "save into a file path must fail");

    let live = mgr.list_sessions().await;
    assert!(
        live[0].communication.is_none(),
        "raise must be rolled back after a failed persist"
    );
}

/// HIGH grinch fix: the raise mutation is gated by `sessions_save_lock()`. While
/// another persistence caller holds it, the raise is not even applied (let alone
/// persisted), so no concurrent persist can snapshot a raised-but-unpersisted
/// state.
#[tokio::test]
async fn raise_hand_and_persist_waits_for_save_lock_before_mutating() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");

    let guard = sessions_save_lock().lock().await;
    let mut fut = Box::pin(raise_hand_and_persist_to_dir_result(
        &mgr,
        session.id,
        chrono::Utc::now(),
        temp.path(),
        None,
    ));
    let timed_out = tokio::time::timeout(Duration::from_millis(25), &mut fut)
        .await
        .is_err();
    assert!(
        timed_out,
        "raise-hand persist should wait for the save lock"
    );

    // The mutation has NOT happened yet: the helper is parked at the lock.
    assert!(
        mgr.list_sessions().await[0].communication.is_none(),
        "raise must not be applied while the save lock is held elsewhere"
    );

    drop(guard);
    let outcome = fut.await.expect("raise-hand persist should succeed");
    assert!(matches!(outcome, RaiseHandPersistOutcome::Raised(_)));
    assert!(mgr.list_sessions().await[0].communication.is_some());
}

/// (#747) Acceptance criterion 5's persist-restore-persist half: a hand
/// raised and persisted in run A re-applies onto run B's dormant (Exited)
/// record via `restore_communication` and survives B's own persist, so a
/// SECOND restart still sees it. The user-input clear tail is already
/// pinned by `clear_user_input_transitions_persists_both_cleared_fields`.
#[tokio::test]
async fn dormant_restore_round_trip_preserves_visible_raise_hand() {
    let temp = tempfile::tempdir().expect("tempdir");

    // Run A: live coordinator raises its hand; #698 persists it durably.
    let mgr_a = SessionManager::new();
    let session_a = mgr_a
        .create_session(
            "claude".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    let raise_time = chrono::DateTime::parse_from_rfc3339("2026-06-30T11:00:00+00:00")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let outcome =
        raise_hand_and_persist_to_dir_result(&mgr_a, session_a.id, raise_time, temp.path(), None)
            .await
            .expect("raise+persist should succeed");
    assert!(matches!(outcome, RaiseHandPersistOutcome::Raised(_)));

    let rows = load_sessions_raw_from_dir_for_test(temp.path());
    assert_eq!(rows.len(), 1);
    let persisted_hand = rows[0]
        .communication
        .clone()
        .expect("run A must persist the visible hand");
    assert_eq!(persisted_hand.kind, SessionCommunicationKind::RaiseHand);
    assert!(persisted_hand.visible);

    // Run B (simulated relaunch, default settings): the defer arm creates
    // the record, marks it Exited, then re-applies the persisted hand.
    let mgr_b = SessionManager::new();
    let session_b = mgr_b
        .create_session(
            rows[0].shell.clone(),
            rows[0].shell_args.clone(),
            rows[0].working_directory.clone(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    mgr_b.mark_exited(session_b.id, 0).await;
    assert!(
        mgr_b
            .restore_communication(session_b.id, persisted_hand.clone())
            .await,
        "dormant coordinator must accept the restored hand"
    );

    persist_current_state_to_dir_result(&mgr_b, temp.path())
        .await
        .expect("run B persist should succeed");

    let rows_b = load_sessions_raw_from_dir_for_test(temp.path());
    assert_eq!(rows_b.len(), 1);
    assert!(
        matches!(rows_b[0].status, Some(SessionStatus::Exited(0))),
        "run B must persist the dormant status, got {:?}",
        rows_b[0].status
    );
    let hand_b = rows_b[0]
        .communication
        .as_ref()
        .expect("the restored hand must survive run B's persist");
    assert_eq!(hand_b.kind, SessionCommunicationKind::RaiseHand);
    assert!(hand_b.visible);
    assert_eq!(
        hand_b.updated_at, persisted_hand.updated_at,
        "the original raise time must survive the round trip"
    );
}

// ---- #1669: atomic blocked-menu set/clear + persist, no rollback ----

/// Happy path: a real blocked-menu transition reaches `sessions.json` through
/// the single save-lock acquisition.
#[tokio::test]
async fn set_blocked_menu_and_persist_writes_blocked_state_to_disk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");
    let now = chrono::DateTime::parse_from_rfc3339("2026-06-30T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let message =
        "codex is waiting for you to answer the folder-trust menu in this terminal".to_string();

    let (outcome, error) = set_blocked_menu_and_persist_to_dir(
        &mgr,
        session.id,
        message.clone(),
        now,
        Some(temp.path()),
        None,
    )
    .await;
    assert!(matches!(outcome, BlockedMenuPersistOutcome::Changed(_)));
    assert!(error.is_none(), "the save must succeed: {error:?}");

    // Live state carries the blocked menu...
    let live = mgr.list_sessions().await;
    let live_comm = live[0].communication.as_ref().expect("live communication");
    assert_eq!(live_comm.kind, SessionCommunicationKind::BlockedMenu);
    assert!(live_comm.visible);
    assert_eq!(live_comm.message.as_deref(), Some(message.as_str()));

    // ...and so does the durable snapshot.
    let saved =
        std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
    let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
    assert_eq!(rows.len(), 1);
    let comm = rows[0]
        .communication
        .as_ref()
        .expect("communication persisted");
    assert_eq!(comm.kind, SessionCommunicationKind::BlockedMenu);
    assert!(comm.visible);
    assert_eq!(comm.message.as_deref(), Some(message.as_str()));
}

/// The 250 ms scan-loop steady state: re-setting the IDENTICAL message
/// mutates nothing and saves nothing. The second call is pointed at a
/// file-as-dir that would fail deterministically, so the `None` error proves
/// the save was never attempted.
#[tokio::test]
async fn set_blocked_menu_and_persist_second_identical_call_writes_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");
    let now = chrono::DateTime::parse_from_rfc3339("2026-06-30T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let message =
        "codex is waiting for you to answer the folder-trust menu in this terminal".to_string();

    let (outcome, error) = set_blocked_menu_and_persist_to_dir(
        &mgr,
        session.id,
        message.clone(),
        now,
        Some(temp.path()),
        None,
    )
    .await;
    assert!(matches!(outcome, BlockedMenuPersistOutcome::Changed(_)));
    assert!(error.is_none(), "the first save must succeed: {error:?}");

    // A second temp dir whose "dir" is an existing FILE, so any attempted
    // save would fail deterministically in `create_dir_all`.
    let temp2 = tempfile::tempdir().expect("tempdir");
    let file_as_dir = temp2.path().join("sessions-dir-is-a-file");
    std::fs::write(&file_as_dir, "not a directory").expect("write file target");

    let (outcome2, error2) = set_blocked_menu_and_persist_to_dir(
        &mgr,
        session.id,
        message.clone(),
        now,
        Some(&file_as_dir),
        None,
    )
    .await;
    assert!(
        matches!(outcome2, BlockedMenuPersistOutcome::Unchanged),
        "an identical message must not be a transition"
    );
    assert!(
        error2.is_none(),
        "a None error can only mean the save was never attempted: {error2:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&file_as_dir).expect("read file target"),
        "not a directory",
        "the file-as-dir must still hold its original bytes"
    );
    assert!(
        !temp2.path().join("sessions.json").exists(),
        "no sessions.json may be written beside the file-as-dir"
    );
}

/// D3 regression net: a failed save must NOT roll the live transition back.
/// `evaluate_logical_rows` has already frozen PTY injection and there is no
/// retry path, so a rollback would leave a frozen peer with no toast, no
/// Sidebar indicator and no disk row. This test fails if anyone adds one.
#[tokio::test]
async fn set_blocked_menu_and_persist_keeps_live_state_when_the_save_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file_as_dir = temp.path().join("sessions-dir-is-a-file");
    std::fs::write(&file_as_dir, "not a directory").expect("write file target");

    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");
    assert!(
        mgr.list_sessions().await[0].communication.is_none(),
        "precondition: no communication before the set"
    );

    let (outcome, error) = set_blocked_menu_and_persist_to_dir(
        &mgr,
        session.id,
        "codex is waiting for you to answer the folder-trust menu in this terminal".to_string(),
        chrono::Utc::now(),
        Some(&file_as_dir),
        None,
    )
    .await;
    assert!(
        matches!(outcome, BlockedMenuPersistOutcome::Changed(_)),
        "the transition must still be reported to the caller"
    );
    assert!(error.is_some(), "save into a file path must fail");

    let live = mgr.list_sessions().await;
    let live_comm = live[0]
        .communication
        .as_ref()
        .expect("the blocked menu must survive a failed persist");
    assert_eq!(live_comm.kind, SessionCommunicationKind::BlockedMenu);
    assert!(live_comm.visible);
}

/// The degraded-config-dir limb: with no resolvable config dir the transition
/// is still applied and still reported, so the blocked-menu toast appears.
#[tokio::test]
async fn set_blocked_menu_and_persist_without_a_config_dir_still_reports_the_transition() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");

    let (outcome, error) = set_blocked_menu_and_persist_to_dir(
        &mgr,
        session.id,
        "codex is waiting for you to answer the folder-trust menu in this terminal".to_string(),
        chrono::Utc::now(),
        None,
        None,
    )
    .await;
    assert!(
        matches!(outcome, BlockedMenuPersistOutcome::Changed(_)),
        "the transition must be reported without a config dir"
    );
    assert_eq!(error.as_deref(), Some("Could not determine home directory"));

    let live = mgr.list_sessions().await;
    let live_comm = live[0].communication.as_ref().expect("live communication");
    assert_eq!(live_comm.kind, SessionCommunicationKind::BlockedMenu);
    assert!(live_comm.visible);
}

/// The clear path writes `communication: null` to disk when something was
/// visible, and returns before any save when nothing was.
#[tokio::test]
async fn clear_blocked_menu_and_persist_to_dir_clears_disk_and_skips_the_save_when_nothing_is_set()
{
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");

    let (outcome, error) = set_blocked_menu_and_persist_to_dir(
        &mgr,
        session.id,
        "codex is waiting for you to answer the folder-trust menu in this terminal".to_string(),
        chrono::Utc::now(),
        Some(temp.path()),
        None,
    )
    .await;
    assert!(matches!(outcome, BlockedMenuPersistOutcome::Changed(_)));
    assert!(error.is_none(), "the set must persist: {error:?}");

    let (cleared, clear_error) =
        clear_blocked_menu_and_persist_to_dir(&mgr, session.id, Some(temp.path()), None).await;
    assert!(cleared, "a visible blocked menu must be cleared");
    assert!(clear_error.is_none(), "the clear must persist");
    assert!(
        mgr.list_sessions().await[0].communication.is_none(),
        "live communication must be cleared"
    );
    let saved =
        std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
    let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].communication.is_none(),
        "the cleared state must reach disk"
    );

    // Nothing left to clear: the helper returns BEFORE any save, proven by the
    // file-as-dir target reporting no error.
    let file_as_dir = temp.path().join("sessions-dir-is-a-file");
    std::fs::write(&file_as_dir, "not a directory").expect("write file target");
    let (cleared_again, clear_error_again) =
        clear_blocked_menu_and_persist_to_dir(&mgr, session.id, Some(&file_as_dir), None).await;
    assert!(!cleared_again, "a second clear is not a transition");
    assert!(
        clear_error_again.is_none(),
        "a None error can only mean the save was never attempted: {clear_error_again:?}"
    );
}

/// D3 regression net for the CLEAR path: a failed save must NOT roll the
/// clear back. `cleared` is the caller's publish gate for `communication:
/// null`, so a rollback here would leave the Sidebar blocked-menu indicator
/// and the sticky toast up for a session that is no longer blocked. This
/// test fails if anyone adds one.
#[tokio::test]
async fn clear_blocked_menu_and_persist_keeps_the_clear_when_the_save_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");

    let (outcome, error) = set_blocked_menu_and_persist_to_dir(
        &mgr,
        session.id,
        "codex is waiting for you to answer the folder-trust menu in this terminal".to_string(),
        chrono::Utc::now(),
        Some(temp.path()),
        None,
    )
    .await;
    assert!(matches!(outcome, BlockedMenuPersistOutcome::Changed(_)));
    assert!(
        error.is_none(),
        "precondition: the set must persist: {error:?}"
    );

    let file_as_dir = temp.path().join("sessions-dir-is-a-file");
    std::fs::write(&file_as_dir, "not a directory").expect("write file target");

    let (cleared, clear_error) =
        clear_blocked_menu_and_persist_to_dir(&mgr, session.id, Some(&file_as_dir), None).await;
    assert!(
        cleared,
        "the clear must still be reported to the caller when the save fails"
    );
    assert!(
        clear_error.is_some(),
        "save into a file path must fail: {clear_error:?}"
    );
    assert!(
        mgr.list_sessions().await[0].communication.is_none(),
        "the clear must survive a failed persist"
    );
}

/// The degraded-config-dir limb of the clear path: with no resolvable config
/// dir the clear is still applied and still reported, and the unavailable
/// directory is surfaced as the save error rather than suppressing the clear.
#[tokio::test]
async fn clear_blocked_menu_and_persist_without_a_config_dir_still_reports_the_clear() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");

    let (outcome, _set_error) = set_blocked_menu_and_persist_to_dir(
        &mgr,
        session.id,
        "codex is waiting for you to answer the folder-trust menu in this terminal".to_string(),
        chrono::Utc::now(),
        None,
        None,
    )
    .await;
    assert!(
        matches!(outcome, BlockedMenuPersistOutcome::Changed(_)),
        "precondition: the blocked menu must be live before the clear"
    );

    let (cleared, clear_error) =
        clear_blocked_menu_and_persist_to_dir(&mgr, session.id, None, None).await;
    assert!(cleared, "the clear must be reported without a config dir");
    assert_eq!(
        clear_error.as_deref(),
        Some("Could not determine home directory")
    );
    assert!(
        mgr.list_sessions().await[0].communication.is_none(),
        "live communication must be cleared without a config dir"
    );
}

// ---- #698 grinch MEDIUM: single-critical-section user-input clear ----

/// MEDIUM grinch fix: both user-input transitions (re-arm `start_fresh_on_restore`
/// + lower raise-hand) clear together and the cleared state reaches disk.
#[tokio::test]
async fn clear_user_input_transitions_persists_both_cleared_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");
    mgr.set_start_fresh_on_restore(session.id, true).await;
    mgr.raise_hand(session.id, chrono::Utc::now())
        .await
        .expect("raise_hand should succeed");

    let cleared = clear_user_input_transitions_and_persist_to_dir_result(
        &mgr,
        session.id,
        true,
        Some(temp.path()),
        None,
    )
    .await
    .expect("clear+persist should succeed");
    assert!(cleared.cleared_start_fresh);
    assert!(cleared.cleared_raise_hand);

    // Live state cleared.
    let live = mgr.list_sessions().await;
    assert!(!live[0].start_fresh_on_restore);
    assert!(live[0].communication.is_none());

    // Durable snapshot cleared.
    let saved =
        std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
    let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].start_fresh_on_restore);
    assert!(rows[0].communication.is_none());
}

/// MEDIUM grinch fix: a real user message lowers the hand and re-arms the fresh
/// intent even if the snapshot save fails (no rollback). The in-memory clear must
/// stand; the next persist reconciles disk.
#[tokio::test]
async fn clear_user_input_transitions_keeps_clear_on_save_failure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file_as_dir = temp.path().join("sessions-dir-is-a-file");
    std::fs::write(&file_as_dir, "not a directory").expect("write file target");

    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");
    mgr.set_start_fresh_on_restore(session.id, true).await;
    mgr.raise_hand(session.id, chrono::Utc::now())
        .await
        .expect("raise_hand should succeed");

    let result = clear_user_input_transitions_and_persist_to_dir_result(
        &mgr,
        session.id,
        true,
        Some(&file_as_dir),
        None,
    )
    .await;
    assert!(result.is_err(), "save into a file path must fail");

    // No rollback: the user typed, so the clear stands in memory.
    let live = mgr.list_sessions().await;
    assert!(!live[0].start_fresh_on_restore);
    assert!(live[0].communication.is_none());
}

/// MEDIUM grinch fix: the user-input clear mutation is gated by
/// `sessions_save_lock()`, so a concurrent persist cannot snapshot or write a
/// half-cleared state between the mutation and its save.
#[tokio::test]
async fn clear_user_input_transitions_waits_for_save_lock_before_mutating() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");
    mgr.set_start_fresh_on_restore(session.id, true).await;
    mgr.raise_hand(session.id, chrono::Utc::now())
        .await
        .expect("raise_hand should succeed");

    let guard = sessions_save_lock().lock().await;
    let mut fut = Box::pin(clear_user_input_transitions_and_persist_to_dir_result(
        &mgr,
        session.id,
        true,
        Some(temp.path()),
        None,
    ));
    let timed_out = tokio::time::timeout(Duration::from_millis(25), &mut fut)
        .await
        .is_err();
    assert!(timed_out, "user-input clear should wait for the save lock");

    // The mutation has NOT happened yet: both fields are still set.
    let parked = mgr.list_sessions().await;
    assert!(
        parked[0].start_fresh_on_restore,
        "fresh intent must remain until the lock frees"
    );
    assert!(
        parked[0].communication.is_some(),
        "raise must remain until the lock frees"
    );

    drop(guard);
    let cleared = fut.await.expect("clear+persist should succeed");
    assert!(cleared.cleared_start_fresh && cleared.cleared_raise_hand);
    let after = mgr.list_sessions().await;
    assert!(!after[0].start_fresh_on_restore);
    assert!(after[0].communication.is_none());
}

// ---- (#756) fresh-intent stamp/drop persist wrappers ----

/// (#756) The stamp persists `startFreshOnRestore: true` exactly once: the
/// first call transitions and saves, the second returns Ok(false) (no
/// rewrite needed).
#[tokio::test]
async fn set_start_fresh_persists_true_transition_exactly_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");

    let stamped = write_start_fresh_and_persist_to_dir_result(
        &mgr,
        session.id,
        true,
        Some(temp.path()),
        None,
    )
    .await
    .expect("stamp+persist should succeed");
    assert!(
        stamped,
        "first stamp must report the false -> true transition"
    );

    let saved =
        std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
    let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].start_fresh_on_restore,
        "stamp must reach the snapshot"
    );

    // Second stamp is a no-op: Ok(false), and the file is not rewritten.
    std::fs::remove_file(temp.path().join("sessions.json")).expect("remove snapshot");
    let again = write_start_fresh_and_persist_to_dir_result(
        &mgr,
        session.id,
        true,
        Some(temp.path()),
        None,
    )
    .await
    .expect("idempotent stamp should succeed");
    assert!(!again, "second stamp must be Ok(false)");
    assert!(
        !temp.path().join("sessions.json").exists(),
        "a no-op stamp must not save"
    );
}

/// (#756) The drop persists the true -> false transition.
#[tokio::test]
async fn clear_start_fresh_persists_false_transition() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");
    mgr.set_start_fresh_on_restore(session.id, true).await;

    let dropped = write_start_fresh_and_persist_to_dir_result(
        &mgr,
        session.id,
        false,
        Some(temp.path()),
        None,
    )
    .await
    .expect("drop+persist should succeed");
    assert!(dropped, "drop must report the true -> false transition");

    let live = mgr.list_sessions().await;
    assert!(!live[0].start_fresh_on_restore);
    let saved =
        std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
    let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
    assert_eq!(rows.len(), 1);
    assert!(
        !rows[0].start_fresh_on_restore,
        "drop must reach the snapshot"
    );
}

/// (#756) Dropping an unset record is Ok(false) and does not save.
#[tokio::test]
async fn clear_start_fresh_on_unset_record_is_noop_and_does_not_save() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
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
        .expect("create_session should succeed");

    let dropped = write_start_fresh_and_persist_to_dir_result(
        &mgr,
        session.id,
        false,
        Some(temp.path()),
        None,
    )
    .await
    .expect("no-op drop should succeed");
    assert!(!dropped, "drop on an unset record must be Ok(false)");
    assert!(
        !temp.path().join("sessions.json").exists(),
        "a no-op drop must not save"
    );
}
