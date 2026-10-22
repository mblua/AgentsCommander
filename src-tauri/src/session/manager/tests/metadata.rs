use super::*;

#[tokio::test]
async fn set_profile_metadata_carries_the_match_fields() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude-mb".to_string(),
            Vec::new(),
            "C:\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    assert!(session.match_tier.is_none());
    assert!(session.original_profile_letter.is_none());

    mgr.set_profile_metadata(
        session.id,
        Some("B".to_string()),
        Some("A".to_string()),
        Vec::new(),
        false,
        Some("hash".to_string()),
        Some("B".to_string()),
        None,
        None,
    )
    .await;

    let stored = mgr
        .get_session(session.id)
        .await
        .expect("session should still exist");
    assert_eq!(stored.match_tier.as_deref(), Some("hash"));
    assert_eq!(stored.original_profile_letter.as_deref(), Some("B"));
}

#[tokio::test]
async fn pending_session_starts_without_match_fields_and_pending_setter_carries_them() {
    let mgr = SessionManager::new();
    let (session, binding) = mgr
        .create_transaction_pending_session(
            "claude-mb".to_string(),
            Vec::new(),
            "C:\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("pending session should be created");
    assert!(session.match_tier.is_none());
    assert!(session.original_profile_letter.is_none());

    mgr.set_pending_profile_metadata(
        binding,
        Some("B".to_string()),
        Some("A".to_string()),
        Vec::new(),
        false,
        Some("commandAndLetter".to_string()),
        Some("B".to_string()),
        None,
        None,
    )
    .await
    .expect("pending metadata should be set");

    let stored = mgr
        .get_pending_session(binding)
        .await
        .expect("pending session should still exist");
    assert_eq!(stored.match_tier.as_deref(), Some("commandAndLetter"));
    assert_eq!(stored.original_profile_letter.as_deref(), Some("B"));
}

#[tokio::test]
async fn set_effective_shell_args_writes_field() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude-mb".to_string(),
            vec!["--dangerously-skip-permissions".to_string()],
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    assert!(session.effective_shell_args.is_none());

    let effective = vec![
        "--dangerously-skip-permissions".to_string(),
        "--continue".to_string(),
    ];
    mgr.set_effective_shell_args(session.id, effective.clone())
        .await;

    let stored = mgr
        .get_session(session.id)
        .await
        .expect("session should still exist");
    assert_eq!(stored.effective_shell_args, Some(effective));
}

// #1063: the deletion-only snapshot is pending-inclusive (unlike every public
// read), excludes `Exited` rows, and returns working directories only.
#[tokio::test]
async fn deletion_snapshot_is_pending_inclusive_and_excludes_exited() {
    let mgr = SessionManager::new();
    let _live = mgr
        .create_session(
            "sh".to_string(),
            Vec::new(),
            "C:\\live".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create live");
    let (_pending, _binding) = mgr
        .create_transaction_pending_session(
            "sh".to_string(),
            Vec::new(),
            "C:\\pending".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create pending");
    let exited = mgr
        .create_session(
            "sh".to_string(),
            Vec::new(),
            "C:\\exited".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create exited");
    mgr.state
        .write()
        .await
        .sessions
        .get_mut(&exited.id)
        .expect("exited session present")
        .status = SessionStatus::Exited(0);

    let public: Vec<String> = mgr
        .list_sessions()
        .await
        .into_iter()
        .map(|session| session.working_directory)
        .collect();
    assert!(public.contains(&"C:\\live".to_string()));
    assert!(
        !public.contains(&"C:\\pending".to_string()),
        "public reads must hide the pending create"
    );

    let snapshot = {
        let mgr = mgr.clone();
        tokio::task::spawn_blocking(move || mgr.live_working_directories_for_deletion_blocking())
            .await
            .unwrap()
    };
    assert!(
        snapshot.contains(&"C:\\live".to_string()),
        "snapshot must include the live session"
    );
    assert!(
        snapshot.contains(&"C:\\pending".to_string()),
        "snapshot must include the pending create (deletion-only)"
    );
    assert!(
        !snapshot.contains(&"C:\\exited".to_string()),
        "snapshot must exclude exited sessions"
    );
}

// Stage E (#1064) deletion-snapshot mutation sentinel (plan section 10.3,
// 10.6, acceptance item 32): the pending-inclusive workdir snapshot must
// include EVERY pending create (not just one) and stays workdir-only while
// public listing keeps every pending hidden. A mutation that mirrored
// `list_sessions` or filtered `pending_create` fails here.
#[tokio::test]
async fn stage_e_deletion_snapshot_includes_every_pending_and_is_workdir_only() {
    let mgr = SessionManager::new();
    let _live = mgr
        .create_session(
            "sh".to_string(),
            Vec::new(),
            "C:\\live".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create live");
    let _pending_a = mgr
        .insert_pending_session_for_test("C:\\pending-a".to_string())
        .await;
    let _pending_b = mgr
        .insert_pending_session_for_test("C:\\pending-b".to_string())
        .await;
    let exited = mgr
        .create_session(
            "sh".to_string(),
            Vec::new(),
            "C:\\exited".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create exited");
    mgr.state
        .write()
        .await
        .sessions
        .get_mut(&exited.id)
        .expect("exited present")
        .status = SessionStatus::Exited(0);

    let snapshot = {
        let mgr = mgr.clone();
        tokio::task::spawn_blocking(move || mgr.live_working_directories_for_deletion_blocking())
            .await
            .unwrap()
    };
    for workdir in ["C:\\live", "C:\\pending-a", "C:\\pending-b"] {
        assert!(
            snapshot.contains(&workdir.to_string()),
            "snapshot must include the non-exited workdir {workdir}"
        );
    }
    assert!(!snapshot.contains(&"C:\\exited".to_string()));
    assert_eq!(
        snapshot.len(),
        3,
        "exactly the three non-exited workdirs, no metadata rows"
    );

    let public: Vec<String> = mgr
        .list_sessions()
        .await
        .into_iter()
        .map(|session| session.working_directory)
        .collect();
    assert!(!public.contains(&"C:\\pending-a".to_string()));
    assert!(!public.contains(&"C:\\pending-b".to_string()));
}

#[tokio::test]
async fn set_effective_shell_args_no_op_on_missing_session() {
    let mgr = SessionManager::new();
    let missing = Uuid::new_v4();
    mgr.set_effective_shell_args(missing, vec!["--continue".to_string()])
        .await;
    assert!(mgr.get_session(missing).await.is_none());
}

// (#630/#631) Re-arm on first user message: a fresh session clears its intent
// exactly once (true -> false), and a second call is a no-op (one-shot gate).
#[tokio::test]
async fn clear_start_fresh_on_restore_is_one_shot() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude-mb".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    // Constructor default is the resume intent (false).
    assert!(!session.start_fresh_on_restore);

    // Stamp the durable fresh intent (as the restart path would).
    mgr.set_start_fresh_on_restore(session.id, true).await;
    let stamped = mgr
        .get_session(session.id)
        .await
        .expect("session should still exist");
    assert!(stamped.start_fresh_on_restore);

    // First user message clears it and reports the true -> false transition.
    assert!(
        mgr.clear_start_fresh_on_restore_if_set(session.id).await,
        "first clear must report the transition so the caller persists once"
    );
    let cleared = mgr
        .get_session(session.id)
        .await
        .expect("session should still exist");
    assert!(!cleared.start_fresh_on_restore);

    // Second message is a no-op: no transition, so the caller does not re-persist.
    assert!(
        !mgr.clear_start_fresh_on_restore_if_set(session.id).await,
        "second clear must be a no-op (one-shot)"
    );
}

#[tokio::test]
async fn clear_start_fresh_on_restore_no_op_on_missing_session() {
    let mgr = SessionManager::new();
    assert!(
        !mgr.clear_start_fresh_on_restore_if_set(Uuid::new_v4())
            .await
    );
}

// (#756) Stamp on an AC-driven clear boundary: transitions exactly once
// (false -> true), a second call is a no-op, and a missing id reports false.
#[tokio::test]
async fn set_start_fresh_on_restore_if_unset_is_one_shot() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude-mb".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    // Constructor default is the resume intent (false).
    assert!(!session.start_fresh_on_restore);

    // First stamp reports the false -> true transition.
    assert!(
        mgr.set_start_fresh_on_restore_if_unset(session.id).await,
        "first stamp must report the transition so the caller persists once"
    );
    let stamped = mgr
        .get_session(session.id)
        .await
        .expect("session should still exist");
    assert!(stamped.start_fresh_on_restore);

    // Second stamp is a no-op: no transition, so the caller does not re-persist.
    assert!(
        !mgr.set_start_fresh_on_restore_if_unset(session.id).await,
        "second stamp must be a no-op (one-shot)"
    );
}

#[tokio::test]
async fn set_start_fresh_on_restore_if_unset_no_op_on_missing_session() {
    let mgr = SessionManager::new();
    assert!(
        !mgr.set_start_fresh_on_restore_if_unset(Uuid::new_v4())
            .await
    );
}
