use super::*;

#[test]
fn status_order_is_exact() {
    assert!(status_rank(&SessionStatus::Active) < status_rank(&SessionStatus::Running));
    assert!(status_rank(&SessionStatus::Running) < status_rank(&SessionStatus::Idle));
    assert!(status_rank(&SessionStatus::Idle) < status_rank(&SessionStatus::Exited(0)));
}

#[test]
fn temporary_only_target_is_unavailable_even_when_its_cwd_vanished() {
    let target = tempfile::TempDir::new().unwrap();
    let target_identity = crate::path_identity::verify_directory(target.path()).unwrap();
    let fact = TerminalSnapshotSessionFact {
        id: Uuid::new_v4(),
        created_at: chrono::Utc::now(),
        name: format!("{TEMP_SESSION_PREFIX}only"),
        status: SessionStatus::Running,
        working_directory: target.path().join("missing").to_string_lossy().to_string(),
        backend_kind: SessionBackendKind::LocalProcess,
    };
    let result = select_target_session(vec![fact], vec![None], target.path(), &target_identity);
    assert!(matches!(
        result,
        Err(TerminalSnapshotReasonCode::TargetUnavailable)
    ));
}

#[test]
fn artifact_registry_removes_only_the_tracked_object() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let directory = tempfile::TempDir::new().unwrap();
    let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    let reservation = state
        .reserve_artifact(directory.path(), &directory_identity)
        .unwrap();
    let path = directory.path().join(format!("{}.json", Uuid::new_v4()));
    std::fs::write(&path, b"secret").unwrap();
    let identity = crate::path_identity::verify_regular_file(&path).unwrap();
    reservation.commit(path.clone(), identity.clone()).unwrap();
    {
        let mut registry = state.artifacts.lock().unwrap();
        registry
            .files
            .get_mut(&identity.object_id)
            .unwrap()
            .expires_at = Instant::now();
    }
    state.sweep_artifacts(false);
    assert!(!path.exists());
    assert!(!state
        .artifacts
        .lock()
        .unwrap()
        .files
        .contains_key(&identity.object_id));
}

#[cfg(not(unix))]
#[test]
fn artifact_registry_releases_a_displaced_object_without_removing_its_replacement() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let directory = tempfile::TempDir::new().unwrap();
    let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    let reservation = state
        .reserve_artifact(directory.path(), &directory_identity)
        .unwrap();
    let path = directory.path().join(format!("{}.json", Uuid::new_v4()));
    let displaced = directory.path().join("displaced-tracked-response");
    std::fs::write(&path, b"tracked response").unwrap();
    let identity = crate::path_identity::verify_regular_file(&path).unwrap();
    reservation.commit(path.clone(), identity.clone()).unwrap();
    std::fs::rename(&path, &displaced).unwrap();
    std::fs::write(&path, b"replacement response").unwrap();
    {
        let mut registry = state.artifacts.lock().unwrap();
        registry
            .files
            .get_mut(&identity.object_id)
            .unwrap()
            .expires_at = Instant::now();
    }

    state.sweep_artifacts(false);
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement response");
    assert_eq!(std::fs::read(&displaced).unwrap(), b"tracked response");
    let registry = state.artifacts.lock().unwrap();
    assert!(!registry.files.contains_key(&identity.object_id));
    assert!(registry.directories.is_empty());
}

#[cfg(unix)]
#[test]
fn artifact_registry_retains_displaced_object_until_witness_proves_absence() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let directory = tempfile::TempDir::new().unwrap();
    let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    let reservation = state
        .reserve_artifact(directory.path(), &directory_identity)
        .unwrap();
    let path = directory.path().join(format!("{}.json", Uuid::new_v4()));
    let displaced = directory.path().join("displaced-tracked-response");
    std::fs::write(&path, b"tracked response").unwrap();
    let identity = crate::path_identity::verify_regular_file(&path).unwrap();
    reservation.commit(path.clone(), identity.clone()).unwrap();
    std::fs::rename(&path, &displaced).unwrap();
    std::fs::write(&path, b"replacement response").unwrap();

    state.sweep_artifacts(true);
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement response");
    assert_eq!(std::fs::read(&displaced).unwrap(), b"tracked response");
    assert_eq!(state.test_artifact_counts(), (1, 1, 0));

    std::fs::remove_file(&displaced).unwrap();
    state.sweep_artifacts(true);
    assert_eq!(std::fs::read(path).unwrap(), b"replacement response");
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[cfg(unix)]
#[test]
fn artifact_cleanup_reclaims_once_only_after_witness_confirmed_removal() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let directory = tempfile::TempDir::new().unwrap();
    let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    let reservation = state
        .reserve_artifact(directory.path(), &directory_identity)
        .unwrap();
    let path = directory.path().join("tracked.json");
    std::fs::write(&path, b"tracked response").unwrap();
    let identity = crate::path_identity::verify_regular_file(&path).unwrap();
    reservation.commit(path.clone(), identity.clone()).unwrap();

    assert_eq!(
        state.cleanup_artifact(&path, &identity),
        TerminalSnapshotArtifactCleanupOutcome::Removed
    );
    assert!(!path.exists());
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    assert_eq!(
        state.cleanup_artifact(&path, &identity),
        TerminalSnapshotArtifactCleanupOutcome::Conflict
    );
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[cfg(unix)]
#[test]
fn artifact_cleanup_claim_collision_retains_entry_and_capacity() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let directory = tempfile::TempDir::new().unwrap();
    let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    let reservation = state
        .reserve_artifact(directory.path(), &directory_identity)
        .unwrap();
    let path = directory.path().join("tracked.json");
    let claim = directory.path().join(".registry-claim-collision");
    std::fs::write(&path, b"tracked response").unwrap();
    std::fs::write(&claim, b"collision response").unwrap();
    let identity = crate::path_identity::verify_regular_file(&path).unwrap();
    reservation.commit(path.clone(), identity.clone()).unwrap();
    state.install_unix_cleanup_hook(
        claim.file_name().map(std::ffi::OsStr::to_os_string),
        |_, _, _| {},
    );

    assert_eq!(
        state.cleanup_artifact(&path, &identity),
        TerminalSnapshotArtifactCleanupOutcome::SourceRetained
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"tracked response");
    assert_eq!(std::fs::read(&claim).unwrap(), b"collision response");
    assert_eq!(state.test_artifact_counts(), (1, 1, 0));

    std::fs::remove_file(claim).unwrap();
    assert_eq!(
        state.cleanup_artifact(&path, &identity),
        TerminalSnapshotArtifactCleanupOutcome::Removed
    );
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[cfg(unix)]
#[test]
fn artifact_cleanup_token_serializes_relocation_and_second_cleanup() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let directory = tempfile::TempDir::new().unwrap();
    let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    let reservation = state
        .reserve_artifact(directory.path(), &directory_identity)
        .unwrap();
    let path = directory.path().join("tracked.json");
    std::fs::write(&path, b"tracked response").unwrap();
    let identity = crate::path_identity::verify_regular_file(&path).unwrap();
    reservation.commit(path.clone(), identity.clone()).unwrap();
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let entered_hook = Arc::clone(&entered);
    let release_hook = Arc::clone(&release);
    state.install_unix_cleanup_hook(None, move |stage, _, _| {
        if stage == crate::path_identity::UnixTrackedCleanupStage::BeforeClaimUnlink {
            entered_hook.wait();
            release_hook.wait();
        }
    });
    let cleanup_state = Arc::clone(&state);
    let cleanup_path = path.clone();
    let cleanup_identity = identity.clone();
    let cleanup = std::thread::spawn(move || {
        cleanup_state.cleanup_artifact(&cleanup_path, &cleanup_identity)
    });

    entered.wait();
    assert_eq!(state.test_artifact_counts(), (1, 1, 0));
    assert_eq!(
        state.cleanup_artifact(&path, &identity),
        TerminalSnapshotArtifactCleanupOutcome::Busy
    );
    assert!(state.relocate_artifact(&identity, &path).is_err());
    release.wait();
    assert_eq!(
        cleanup.join().unwrap(),
        TerminalSnapshotArtifactCleanupOutcome::Removed
    );
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[cfg(unix)]
#[test]
fn artifact_relocation_finds_namespace_move_that_wins_cleanup_race() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let directory = tempfile::TempDir::new().unwrap();
    let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    let reservation = state
        .reserve_artifact(directory.path(), &directory_identity)
        .unwrap();
    let path = directory.path().join("temporary.json");
    let destination = directory.path().join("published.json");
    std::fs::write(&path, b"tracked response").unwrap();
    let identity = crate::path_identity::verify_regular_file(&path).unwrap();
    reservation.commit(path.clone(), identity.clone()).unwrap();
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let entered_hook = Arc::clone(&entered);
    let release_hook = Arc::clone(&release);
    state.install_unix_cleanup_hook(None, move |stage, _, _| {
        if stage == crate::path_identity::UnixTrackedCleanupStage::BeforeClaimRename {
            entered_hook.wait();
            release_hook.wait();
        }
    });
    let cleanup_state = Arc::clone(&state);
    let cleanup_path = path.clone();
    let cleanup_identity = identity.clone();
    let cleanup = std::thread::spawn(move || {
        cleanup_state.cleanup_artifact(&cleanup_path, &cleanup_identity)
    });

    entered.wait();
    std::fs::rename(&path, &destination).unwrap();
    assert!(state.relocate_artifact(&identity, &destination).is_err());
    release.wait();
    assert_eq!(
        cleanup.join().unwrap(),
        TerminalSnapshotArtifactCleanupOutcome::Uncertain
    );
    assert!(state
        .relocate_artifact(&identity, &destination)
        .unwrap()
        .is_some());
    assert_eq!(state.test_artifact_counts(), (1, 1, 0));
    assert_eq!(
        state.cleanup_artifact(&destination, &identity),
        TerminalSnapshotArtifactCleanupOutcome::Removed
    );
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[test]
fn host_final_handoff_discards_success_bytes_after_authority_change() {
    let secret = b"terminal-content-sentinel".to_vec();
    let result = finalize_host_publication(
        secret,
        Err(TerminalSnapshotReasonCode::AuthorityChanged),
        |outcome| {
            assert!(matches!(
                outcome,
                Err(TerminalSnapshotReasonCode::AuthorityChanged)
            ));
            Ok(())
        },
    );
    assert!(matches!(
        result,
        Err(TerminalSnapshotReasonCode::AuthorityChanged)
    ));
}

#[test]
fn host_final_handoff_reports_the_actual_publication_failure() {
    let result = finalize_host_publication(b"safe-envelope".to_vec(), Ok(()), |outcome| {
        assert!(outcome.is_ok());
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    });
    assert!(matches!(
        result,
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    ));
}

#[test]
fn artifact_reservation_release_immediately_reclaims_an_idle_directory_record() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let directory = tempfile::TempDir::new().unwrap();
    let identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    drop(state.reserve_artifact(directory.path(), &identity).unwrap());
    assert!(state.artifacts.lock().unwrap().directories.is_empty());
    assert!(state.reserve_artifact(directory.path(), &identity).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_blocking_work_retains_the_requester_and_global_permit() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let permit = state.admit_requester("requester".to_string()).unwrap();
    let audit =
        TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
    let control = TerminalSnapshotBlockingControl::new(None);
    state.install_blocking_control(
        TerminalSnapshotBlockingStage::TestResourceRetention,
        Arc::clone(&control),
    );
    let task_state = Arc::clone(&state);
    let task = tokio::spawn(async move {
        let result = run_blocking_with_deadline(
            &task_state,
            TerminalSnapshotBlockingStage::TestResourceRetention,
            Instant::now() + Duration::from_secs(60),
            &permit,
            &audit,
            || (),
        )
        .await;
        drop(permit);
        result
    });
    control.wait_until_entered();
    control.expire_deadline();
    assert!(matches!(
        task.await.expect("controlled blocking waiter"),
        Err(TerminalSnapshotReasonCode::SnapshotTimeout)
    ));
    assert!(state.admit_requester("requester".to_string()).is_err());
    control.release();
    control.wait_until_completed();
    assert!(state.admit_requester("requester".to_string()).is_ok());
}

#[test]
fn limiter_records_requester_before_target_promotion() {
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let permit = state.admit_requester("requester".to_string()).unwrap();
    permit.promote_target("target".to_string()).unwrap();
    assert!(state.admit_requester("requester".to_string()).is_err());
    drop(permit);
    assert!(state.admit_requester("requester".to_string()).is_ok());
}
