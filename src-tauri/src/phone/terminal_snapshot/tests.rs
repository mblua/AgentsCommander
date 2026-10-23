use super::*;
use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage;
use std::path::{Path, PathBuf};
use terminal_snapshot_renderer::{
    canonical_timestamp, TerminalSnapshotFormat, TerminalSnapshotReasonCode,
};
use uuid::Uuid;

fn response_directory() -> tempfile::TempDir {
    let directory = tempfile::TempDir::new().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    directory
}

fn response_temporary_paths(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".terminal-snapshot-response-tmp"))
        })
        .collect()
}

fn assert_registry_reclaimed(state: &crate::pty::terminal_snapshot::TerminalSnapshotState) {
    state.sweep_artifacts_for_test(false);
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

fn request() -> HostTerminalSnapshotRequest {
    let issued = chrono::Utc::now();
    let mut request = HostTerminalSnapshotRequest {
        kind: REQUEST_KIND.to_string(),
        version: REQUEST_VERSION,
        request_id: Uuid::new_v4().to_string(),
        token: Uuid::new_v4().to_string(),
        from: "project:wg-1-team/lead".to_string(),
        to: "project:wg-1-team/member".to_string(),
        format: TerminalSnapshotFormat::Json,
        issued_at: canonical_timestamp(issued),
        expires_at: canonical_timestamp(issued + chrono::Duration::seconds(15)),
        nonce: "a".repeat(64),
        confirmation_tag: String::new(),
    };
    request.confirmation_tag = confirmation_tag(&request);
    request
}

#[cfg(windows)]
#[test]
fn response_parent_replacement_before_retention_leaves_no_secret_residue() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("responses");
    let retired = directory.path().join("retired-responses");
    std::fs::create_dir(&parent).unwrap();
    let parent_for_hook = parent.clone();
    let retired_for_hook = retired.clone();
    state.install_response_publication_hook(Stage::BeforeDirectoryRetention, move |_, _| {
        std::fs::rename(&parent_for_hook, &retired_for_hook).unwrap();
        std::fs::create_dir(&parent_for_hook).unwrap();
    });

    let request_id = Uuid::new_v4();
    let result = publish_response_bytes(&state, &parent, request_id, b"secret-response");

    assert_eq!(result, Err(TerminalSnapshotReasonCode::ResponseUnavailable));
    assert!(
        std::fs::read_dir(&parent).unwrap().next().is_none(),
        "replacement parent was touched"
    );
    assert!(
        std::fs::read_dir(&retired).unwrap().next().is_none(),
        "secret response reached the displaced parent before retention"
    );
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[cfg(windows)]
#[test]
fn retained_response_parent_blocks_replacement_at_every_publication_stage() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    for stage in [
        Stage::BeforeTemporaryCreate,
        Stage::BeforeTemporaryCommit,
        Stage::AfterTemporaryCommit,
        Stage::AfterTemporaryWrite,
        Stage::BeforeAtomicRename,
        Stage::AfterAtomicRename,
        Stage::AfterFinalVerification,
        Stage::AfterRegistryCommit,
        Stage::AfterParentSync,
    ] {
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("responses");
        let retired = directory.path().join("retired-responses");
        std::fs::create_dir(&parent).unwrap();
        let parent_for_hook = parent.clone();
        let retired_for_hook = retired.clone();
        state.install_response_publication_hook(stage, move |_, _| {
            assert!(
                std::fs::rename(&parent_for_hook, &retired_for_hook).is_err(),
                "retained parent was replaceable at {stage:?}"
            );
        });

        let request_id = Uuid::new_v4();
        assert!(publish_response_bytes(&state, &parent, request_id, b"secret-response").is_ok());
        let destination = parent.join(format!("{request_id}.json"));
        assert_eq!(std::fs::read(&destination).unwrap(), b"secret-response");
        assert!(!retired.exists());
        assert!(
            std::fs::rename(&parent, &retired).is_err(),
            "registry did not retain the parent after {stage:?}"
        );
        state.sweep_artifacts_for_test(true);
        assert!(!destination.exists());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        std::fs::rename(&parent, &retired).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn response_parent_replacement_before_temporary_create_is_confined_on_unix() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("responses");
    let retired = directory.path().join("retired-responses");
    std::fs::create_dir(&parent).unwrap();
    let parent_for_hook = parent.clone();
    let retired_for_hook = retired.clone();
    state.install_response_publication_hook(Stage::BeforeTemporaryCreate, move |_, _| {
        std::fs::rename(&parent_for_hook, &retired_for_hook).unwrap();
        std::fs::create_dir(&parent_for_hook).unwrap();
    });

    let request_id = Uuid::new_v4();
    let result = publish_response_bytes(&state, &parent, request_id, b"secret-response");

    assert_eq!(result, Err(TerminalSnapshotReasonCode::ResponseUnavailable));
    assert!(std::fs::read_dir(&parent).unwrap().next().is_none());
    assert!(std::fs::read_dir(&retired).unwrap().next().is_none());
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[cfg(unix)]
#[test]
fn response_parent_replacement_after_write_cleans_retained_object_on_unix() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("responses");
    let retired = directory.path().join("retired-responses");
    std::fs::create_dir(&parent).unwrap();
    let parent_for_hook = parent.clone();
    let retired_for_hook = retired.clone();
    state.install_response_publication_hook(Stage::AfterTemporaryWrite, move |_, _| {
        std::fs::rename(&parent_for_hook, &retired_for_hook).unwrap();
        std::fs::create_dir(&parent_for_hook).unwrap();
        std::fs::write(parent_for_hook.join("replacement-marker"), b"keep").unwrap();
    });

    let request_id = Uuid::new_v4();
    let result = publish_response_bytes(&state, &parent, request_id, b"secret-response");

    assert_eq!(result, Err(TerminalSnapshotReasonCode::ResponseUnavailable));
    assert_eq!(
        std::fs::read(parent.join("replacement-marker")).unwrap(),
        b"keep"
    );
    assert!(std::fs::read_dir(&retired).unwrap().next().is_none());
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[cfg(unix)]
#[test]
fn tracked_cleanup_uses_original_parent_after_same_spelling_replacement_on_unix() {
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("responses");
    let retired = directory.path().join("retired-responses");
    std::fs::create_dir(&parent).unwrap();
    let response = parent.join("response.json");
    std::fs::write(&response, b"owned response").unwrap();
    let directory_identity = crate::path_identity::verify_directory(&parent).unwrap();
    let identity = crate::path_identity::verify_regular_file(&response).unwrap();
    state
        .reserve_existing_artifact(&parent, &directory_identity, identity.object_id)
        .unwrap()
        .commit_with_ttl(response.clone(), identity, std::time::Duration::ZERO)
        .unwrap();

    std::fs::rename(&parent, &retired).unwrap();
    std::fs::create_dir(&parent).unwrap();
    std::fs::write(&response, b"replacement response").unwrap();
    state.sweep_artifacts_for_test(true);

    assert!(!retired.join("response.json").exists());
    assert_eq!(std::fs::read(response).unwrap(), b"replacement response");
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[test]
fn confirmation_tag_and_window_are_exact() {
    let request = request();
    assert_eq!(request.confirmation_tag.len(), 64);
    assert!(request.validate(chrono::Utc::now()).is_ok());
    let mut changed = request.clone();
    changed.to.push('x');
    assert!(changed.validate(chrono::Utc::now()).is_err());
}

#[test]
fn request_correlation_rejects_body_ids_that_do_not_match_the_filename() {
    let mut request = request();
    let filename_id = request.request_id.clone();
    request.request_id = Uuid::new_v4().to_string();
    request.confirmation_tag = confirmation_tag(&request);
    assert!(request.validate_correlation(&filename_id).is_err());

    request.request_id = "../audit-secret".to_string();
    request.confirmation_tag = confirmation_tag(&request);
    assert!(request.validate_correlation(&filename_id).is_err());
}

#[test]
fn response_publication_is_canonical_and_never_replaces_a_collision() {
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let directory = tempfile::TempDir::new().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let request_id = Uuid::new_v4();
    let tag = "a".repeat(64);
    assert!(publish_trusted_failure(
        &state,
        directory.path(),
        request_id,
        &tag,
        TerminalSnapshotReasonCode::InvalidRequest,
    )
    .is_ok());
    let destination = directory.path().join(format!("{request_id}.json"));
    let first = std::fs::read(&destination).unwrap();
    assert!(matches!(
        publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &tag,
            TerminalSnapshotReasonCode::Internal,
        ),
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    ));
    assert_eq!(std::fs::read(destination).unwrap(), first);
}

#[test]
fn response_publication_collisions_never_overwrite_or_orphan_owned_temps() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    let directory = response_directory();
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let request_id = Uuid::new_v4();
    let collided_temporary = std::sync::Arc::new(std::sync::Mutex::new(None));
    let collided_temporary_for_hook = std::sync::Arc::clone(&collided_temporary);
    state.install_response_publication_hook(Stage::BeforeTemporaryCreate, move |temporary, _| {
        std::fs::write(temporary, b"temporary collision").unwrap();
        *collided_temporary_for_hook.lock().unwrap() = Some(temporary.to_path_buf());
    });
    assert!(matches!(
        publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        ),
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    ));
    let temporary = collided_temporary.lock().unwrap().clone().unwrap();
    assert_eq!(std::fs::read(&temporary).unwrap(), b"temporary collision");
    assert!(!directory.path().join(format!("{request_id}.json")).exists());
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    std::fs::remove_file(temporary).unwrap();
    assert_registry_reclaimed(&state);

    for stage in [Stage::AfterTemporaryCommit, Stage::BeforeAtomicRename] {
        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let request_id = Uuid::new_v4();
        state.install_response_publication_hook(stage, |_, destination| {
            std::fs::write(destination, b"final collision").unwrap();
        });
        assert!(matches!(
            publish_trusted_failure(
                &state,
                directory.path(),
                request_id,
                &"a".repeat(64),
                TerminalSnapshotReasonCode::InvalidRequest,
            ),
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
        assert_eq!(
            std::fs::read(directory.path().join(format!("{request_id}.json"))).unwrap(),
            b"final collision"
        );
        assert!(response_temporary_paths(directory.path()).is_empty());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        std::fs::remove_file(directory.path().join(format!("{request_id}.json"))).unwrap();
        assert_registry_reclaimed(&state);
    }
}

#[test]
fn temporary_identity_change_before_registry_commit_fails_closed() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    let directory = response_directory();
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let request_id = Uuid::new_v4();
    let displaced = directory.path().join("displaced-empty-temporary");
    let replacement = std::sync::Arc::new(std::sync::Mutex::new(None));
    let replacement_for_hook = std::sync::Arc::clone(&replacement);
    let displaced_for_hook = displaced.clone();
    state.install_response_publication_hook(Stage::BeforeTemporaryCommit, move |temporary, _| {
        std::fs::rename(temporary, &displaced_for_hook).unwrap();
        std::fs::write(temporary, b"temporary replacement").unwrap();
        *replacement_for_hook.lock().unwrap() = Some(temporary.to_path_buf());
    });

    assert!(matches!(
        publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        ),
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    ));
    let replacement = replacement.lock().unwrap().clone().unwrap();
    assert_eq!(
        std::fs::read(&replacement).unwrap(),
        b"temporary replacement"
    );
    assert_eq!(std::fs::read(&displaced).unwrap(), b"");
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    std::fs::remove_file(replacement).unwrap();
    std::fs::remove_file(displaced).unwrap();
    assert_registry_reclaimed(&state);
}

#[test]
fn temporary_identity_change_after_write_never_renames_or_removes_the_replacement() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    let directory = response_directory();
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let request_id = Uuid::new_v4();
    let displaced = directory.path().join("consumed-written-temporary");
    let replacement = std::sync::Arc::new(std::sync::Mutex::new(None));
    let replacement_for_hook = std::sync::Arc::clone(&replacement);
    let displaced_for_hook = displaced.clone();
    state.install_response_publication_hook(Stage::AfterTemporaryWrite, move |temporary, _| {
        std::fs::rename(temporary, &displaced_for_hook).unwrap();
        std::fs::write(temporary, b"post-write temporary replacement").unwrap();
        *replacement_for_hook.lock().unwrap() = Some(temporary.to_path_buf());
    });

    assert!(matches!(
        publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        ),
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    ));
    let replacement = replacement.lock().unwrap().clone().unwrap();
    assert_eq!(
        std::fs::read(&replacement).unwrap(),
        b"post-write temporary replacement"
    );
    assert!(std::fs::metadata(&displaced).unwrap().len() > 0);
    assert!(!directory.path().join(format!("{request_id}.json")).exists());
    #[cfg(unix)]
    assert_eq!(state.test_artifact_counts(), (1, 1, 0));
    #[cfg(not(unix))]
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    std::fs::remove_file(replacement).unwrap();
    std::fs::remove_file(displaced).unwrap();
    #[cfg(unix)]
    state.sweep_artifacts_for_test(true);
    assert_registry_reclaimed(&state);
}

#[test]
fn every_post_rename_client_consumption_stage_succeeds_and_reuses_capacity() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    for stage in [
        Stage::AfterAtomicRename,
        Stage::AfterFinalVerification,
        Stage::AfterRegistryCommit,
        Stage::AfterParentSync,
    ] {
        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let request_id = Uuid::new_v4();
        state.install_response_publication_hook(stage, |_, destination| {
            let identity = crate::path_identity::verify_regular_file(destination).unwrap();
            assert!(safe_remove(destination, &identity));
        });
        assert!(
            publish_trusted_failure(
                &state,
                directory.path(),
                request_id,
                &"a".repeat(64),
                TerminalSnapshotReasonCode::InvalidRequest,
            )
            .is_ok(),
            "client consumption was reclassified at {stage:?}"
        );
        assert!(!directory.path().join(format!("{request_id}.json")).exists());
        assert!(response_temporary_paths(directory.path()).is_empty());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        assert_registry_reclaimed(&state);
        let identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        assert!(state.reserve_artifact(directory.path(), &identity).is_ok());
    }
}

#[test]
fn every_post_rename_mismatch_fails_without_deleting_the_replacement() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    for (index, stage) in [
        Stage::AfterAtomicRename,
        Stage::AfterFinalVerification,
        Stage::AfterRegistryCommit,
        Stage::AfterParentSync,
    ]
    .into_iter()
    .enumerate()
    {
        let directory = response_directory();
        let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
            crate::shutdown::ShutdownSignal::new(),
        );
        let request_id = Uuid::new_v4();
        let displaced = directory.path().join(format!("consumed-{index}.json"));
        let displaced_for_hook = displaced.clone();
        state.install_response_publication_hook(stage, move |_, destination| {
            std::fs::rename(destination, &displaced_for_hook).unwrap();
            std::fs::write(destination, b"mismatched final replacement").unwrap();
        });
        assert!(matches!(
            publish_trusted_failure(
                &state,
                directory.path(),
                request_id,
                &"a".repeat(64),
                TerminalSnapshotReasonCode::InvalidRequest,
            ),
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
        let destination = directory.path().join(format!("{request_id}.json"));
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"mismatched final replacement"
        );
        assert!(std::fs::metadata(&displaced).unwrap().len() > 0);
        #[cfg(unix)]
        assert_eq!(state.test_artifact_counts(), (1, 1, 0));
        #[cfg(not(unix))]
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        std::fs::remove_file(destination).unwrap();
        std::fs::remove_file(displaced).unwrap();
        #[cfg(unix)]
        state.sweep_artifacts_for_test(true);
        assert_registry_reclaimed(&state);
    }
}

#[test]
fn response_artifact_reservation_commit_relocation_and_cleanup_are_exact() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    let directory = response_directory();
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let before_create = std::sync::Arc::clone(&state);
    state.install_response_publication_hook(Stage::BeforeTemporaryCreate, move |_, _| {
        assert_eq!(before_create.test_artifact_counts(), (1, 0, 1));
    });
    let after_commit = std::sync::Arc::clone(&state);
    state.install_response_publication_hook(Stage::AfterTemporaryCommit, move |temporary, _| {
        assert!(temporary.exists());
        assert_eq!(after_commit.test_artifact_counts(), (1, 1, 0));
    });
    let after_relocation = std::sync::Arc::clone(&state);
    state.install_response_publication_hook(
        Stage::AfterRegistryCommit,
        move |temporary, destination| {
            assert!(!temporary.exists());
            assert!(destination.exists());
            assert_eq!(after_relocation.test_artifact_counts(), (1, 1, 0));
        },
    );
    let request_id = Uuid::new_v4();
    assert!(publish_trusted_failure(
        &state,
        directory.path(),
        request_id,
        &"a".repeat(64),
        TerminalSnapshotReasonCode::InvalidRequest,
    )
    .is_ok());
    assert!(response_temporary_paths(directory.path()).is_empty());
    state.sweep_artifacts_for_test(true);
    assert!(!directory.path().join(format!("{request_id}.json")).exists());
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    let identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    assert!(state.reserve_artifact(directory.path(), &identity).is_ok());
}

#[cfg(unix)]
#[test]
fn committed_response_cleanup_uses_final_barrier_and_preserves_replacement() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationFailure as Failure;

    let directory = response_directory();
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let replacement = std::sync::Arc::new(std::sync::Mutex::new(None));
    let replacement_for_hook = std::sync::Arc::clone(&replacement);
    state.install_unix_cleanup_hook(None, move |stage, source, _| {
        if stage == crate::path_identity::UnixTrackedCleanupStage::BeforeClaimUnlink {
            std::fs::write(source, b"foreign temporary replacement").unwrap();
            *replacement_for_hook.lock().unwrap() = Some(source.to_path_buf());
        }
    });
    state.install_response_publication_failure(Failure::FileSync);

    assert!(matches!(
        publish_trusted_failure(
            &state,
            directory.path(),
            Uuid::new_v4(),
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        ),
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    ));
    let replacement = replacement.lock().unwrap().clone().unwrap();
    assert_eq!(
        std::fs::read(&replacement).unwrap(),
        b"foreign temporary replacement"
    );
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    std::fs::remove_file(replacement).unwrap();
    assert_registry_reclaimed(&state);
}

#[test]
fn file_sync_failure_is_response_unavailable_while_parent_sync_is_best_effort() {
    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationFailure as Failure;

    let directory = response_directory();
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let request_id = Uuid::new_v4();
    state.install_response_publication_failure(Failure::FileSync);
    assert!(matches!(
        publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        ),
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    ));
    assert!(!directory.path().join(format!("{request_id}.json")).exists());
    assert!(response_temporary_paths(directory.path()).is_empty());
    assert_registry_reclaimed(&state);

    let directory = response_directory();
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let request_id = Uuid::new_v4();
    state.install_response_publication_failure(Failure::ParentSync);
    assert!(publish_trusted_failure(
        &state,
        directory.path(),
        request_id,
        &"a".repeat(64),
        TerminalSnapshotReasonCode::InvalidRequest,
    )
    .is_ok());
    assert!(directory.path().join(format!("{request_id}.json")).exists());
    state.sweep_artifacts_for_test(true);
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[cfg(windows)]
#[test]
fn failed_remove_keeps_the_owned_temp_tracked_until_retry() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage as Stage;

    let directory = response_directory();
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let blocker = std::sync::Arc::new(std::sync::Mutex::new(None));
    let blocker_for_hook = std::sync::Arc::clone(&blocker);
    state.install_response_publication_hook(Stage::BeforeAtomicRename, move |temporary, _| {
        let mut options = std::fs::OpenOptions::new();
        options
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        *blocker_for_hook.lock().unwrap() = Some(options.open(temporary).unwrap());
    });
    assert!(matches!(
        publish_trusted_failure(
            &state,
            directory.path(),
            Uuid::new_v4(),
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        ),
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    ));
    assert_eq!(state.test_artifact_counts(), (1, 1, 0));
    assert_eq!(response_temporary_paths(directory.path()).len(), 1);
    drop(blocker.lock().unwrap().take());
    state.sweep_artifacts_for_test(true);
    assert!(response_temporary_paths(directory.path()).is_empty());
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[test]
fn response_consumed_after_atomic_publish_is_not_reclassified_as_failure() {
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let directory = tempfile::TempDir::new().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let request_id = Uuid::new_v4();
    let destination = directory.path().join(format!("{request_id}.json"));
    let destination_for_hook = destination.clone();
    state.install_response_after_publish_hook(move || {
        crate::path_identity::verify_regular_file(&destination_for_hook)
            .expect("published response identity");
        std::fs::remove_file(&destination_for_hook).expect("consume published response");
    });

    assert!(publish_trusted_failure(
        &state,
        directory.path(),
        request_id,
        &"a".repeat(64),
        TerminalSnapshotReasonCode::InvalidRequest,
    )
    .is_ok());
    assert!(path_is_absent(&destination));
}

#[test]
fn response_consumed_after_registry_commit_releases_registry_capacity() {
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let directory = tempfile::TempDir::new().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let request_id = Uuid::new_v4();
    state.install_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::AfterRegistryCommit,
        |_, destination| {
            std::fs::remove_file(destination).expect("consume committed response");
        },
    );

    assert!(publish_trusted_failure(
        &state,
        directory.path(),
        request_id,
        &"a".repeat(64),
        TerminalSnapshotReasonCode::InvalidRequest,
    )
    .is_ok());
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    state.sweep_artifacts_for_test(false);
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[test]
fn response_replaced_after_registry_commit_fails_without_deleting_replacement() {
    let state = crate::pty::terminal_snapshot::TerminalSnapshotState::new(
        crate::shutdown::ShutdownSignal::new(),
    );
    let directory = tempfile::TempDir::new().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let request_id = Uuid::new_v4();
    let displaced = directory.path().join("client-consumed-response");
    state.install_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::AfterRegistryCommit,
        move |_, destination| {
            std::fs::rename(destination, &displaced).expect("consume committed object");
            std::fs::write(destination, b"attacker replacement")
                .expect("install mismatched final object");
        },
    );

    assert!(matches!(
        publish_trusted_failure(
            &state,
            directory.path(),
            request_id,
            &"a".repeat(64),
            TerminalSnapshotReasonCode::InvalidRequest,
        ),
        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
    ));
    let destination = directory.path().join(format!("{request_id}.json"));
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"attacker replacement"
    );
    #[cfg(unix)]
    assert_eq!(state.test_artifact_counts(), (1, 1, 0));
    #[cfg(not(unix))]
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    #[cfg(unix)]
    {
        std::fs::remove_file(directory.path().join("client-consumed-response")).unwrap();
        state.sweep_artifacts_for_test(true);
    }
    state.sweep_artifacts_for_test(false);
    assert_eq!(state.test_artifact_counts(), (0, 0, 0));
}

#[test]
fn cleanup_names_are_closed_to_protocol_shapes() {
    let request_id = Uuid::new_v4();
    let daemon_id = Uuid::new_v4();
    let nonce = "a".repeat(64);
    assert!(protocol_cleanup_name(&format!("{request_id}.json"), false));
    assert!(protocol_cleanup_name(
        &format!(".{request_id}.{nonce}.terminal-snapshot-request-tmp"),
        false,
    ));
    assert!(protocol_cleanup_name(
        &format!(".{request_id}.{daemon_id}.terminal-snapshot-processing"),
        false,
    ));
    assert!(protocol_cleanup_name(
        &format!(".{request_id}.{daemon_id}.terminal-snapshot-response-tmp"),
        true,
    ));
    assert!(!protocol_cleanup_name("unrelated.json", false));
    assert!(!protocol_cleanup_name(
        &format!(".{request_id}.{daemon_id}.terminal-snapshot-processing"),
        true,
    ));
}

#[test]
fn request_debug_is_structural_and_omits_auth_identity_and_path_canaries() {
    const AUTH_CANARY: &str = "AUTH_1173_H5C8";
    const PATH_CANARY: &str = r"C:\PATH_1173_H5C8\request.json";
    let mut request = request();
    request.kind = AUTH_CANARY.to_string();
    request.request_id = AUTH_CANARY.to_string();
    request.token = AUTH_CANARY.to_string();
    request.from = AUTH_CANARY.to_string();
    request.to = PATH_CANARY.to_string();
    request.issued_at = AUTH_CANARY.to_string();
    request.expires_at = AUTH_CANARY.to_string();
    request.nonce = AUTH_CANARY.to_string();
    request.confirmation_tag = AUTH_CANARY.to_string();

    let diagnostic = format!("{request:?}");
    assert!(!diagnostic.contains(AUTH_CANARY));
    assert!(!diagnostic.contains(PATH_CANARY));
    assert!(diagnostic.contains("version: 1"));
    assert!(diagnostic.contains("format: Json"));
}
