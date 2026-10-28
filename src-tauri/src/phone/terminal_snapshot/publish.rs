use super::MAX_DIRECTORY_ENTRIES;
use super::RESPONSE_TTL;
use crate::filesystem::terminal_snapshots;
use crate::pty::terminal_snapshot::TerminalSnapshotArtifactCleanupOutcome;
#[cfg(test)]
use crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationStage;
use std::path::Path;
use terminal_snapshot_renderer::TerminalSnapshotReasonCode;
use terminal_snapshot_renderer::MAX_TRANSPORT_BYTES;
use uuid::Uuid;

pub(super) fn publish_response_bytes(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    response_directory: &Path,
    request_id: Uuid,
    bytes: &[u8],
) -> Result<(), TerminalSnapshotReasonCode> {
    if bytes.is_empty() || bytes.len() > MAX_TRANSPORT_BYTES {
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    let validated_directory = crate::path_identity::verify_directory(response_directory)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::BeforeDirectoryRetention,
        response_directory,
        response_directory,
    );
    let retained_directory = crate::path_identity::retain_directory(response_directory)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    if !crate::path_identity::same_object(&validated_directory, retained_directory.identity()) {
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    let directory_identity = retained_directory.identity().clone();
    let reservation = state
        .reserve_artifact(response_directory, &directory_identity)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    let temporary = response_directory.join(format!(
        ".{}.{}.terminal-snapshot-response-tmp",
        request_id,
        Uuid::new_v4()
    ));
    let destination = response_directory.join(format!("{request_id}.json"));
    if destination.parent() != Some(response_directory)
        || terminal_snapshots::snapshot_path_exists(&destination)
    {
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::BeforeTemporaryCreate,
        &temporary,
        &destination,
    );
    let mut file = retained_directory
        .create_new_private_file(&temporary)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    let temporary_identity = retained_directory
        .verify_opened_regular_file(&temporary, &file, true)
        .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::BeforeTemporaryCommit,
        &temporary,
        &destination,
    );
    if reservation
        .commit(temporary.clone(), temporary_identity.clone())
        .is_err()
    {
        retained_directory.remove_regular_file_if_same(&temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::AfterTemporaryCommit,
        &temporary,
        &destination,
    );
    if retained_directory.verify_current().is_err() {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(unix)]
    if !terminal_snapshots::set_private_snapshot_response(&file) {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    if terminal_snapshots::write_snapshot_response(&mut file, bytes).is_err() {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    let file_sync_failed = state.take_response_publication_failure(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationFailure::FileSync,
    );
    #[cfg(not(test))]
    let file_sync_failed = false;
    if file_sync_failed || terminal_snapshots::sync_snapshot_response(&file).is_err() {
        cleanup_tracked_artifact(state, &temporary, &temporary_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::AfterTemporaryWrite,
        &temporary,
        &destination,
    );
    let written_identity = match retained_directory
        .verify_current()
        .and_then(|_| retained_directory.verify_opened_regular_file(&temporary, &file, false))
    {
        Ok(identity)
            if crate::path_identity::same_object(&identity, &temporary_identity)
                && crate::path_identity::is_verified_descendant(&identity, &directory_identity) =>
        {
            identity
        }
        _ => {
            cleanup_tracked_artifact(state, &temporary, &temporary_identity);
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
    };
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::BeforeAtomicRename,
        &temporary,
        &destination,
    );
    if retained_directory
        .publish_new_file_atomic(&temporary, &destination)
        .is_err()
    {
        cleanup_tracked_artifact(state, &temporary, &written_identity);
        return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::AfterAtomicRename,
        &temporary,
        &destination,
    );
    let mut published = reconcile_published_response(
        state,
        &retained_directory,
        &destination,
        &file,
        &written_identity,
        &directory_identity,
    )?;
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::AfterFinalVerification,
        &temporary,
        &destination,
    );
    if published.is_some() {
        published = reconcile_published_response(
            state,
            &retained_directory,
            &destination,
            &file,
            &written_identity,
            &directory_identity,
        )?;
    }
    if published.is_some() {
        published = match state.relocate_artifact(&written_identity, &destination) {
            Ok(published) => published,
            Err(_) => {
                #[cfg(not(unix))]
                cleanup_tracked_artifact(state, &destination, &written_identity);
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
        };
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::AfterRegistryCommit,
        &temporary,
        &destination,
    );
    if published.is_some() {
        published = reconcile_published_response(
            state,
            &retained_directory,
            &destination,
            &file,
            &written_identity,
            &directory_identity,
        )?;
    }
    #[cfg(test)]
    let parent_sync_failed = state.take_response_publication_failure(
        crate::pty::terminal_snapshot::TerminalSnapshotResponsePublicationFailure::ParentSync,
    );
    #[cfg(not(test))]
    let parent_sync_failed = false;
    if !parent_sync_failed {
        retained_directory.sync_best_effort();
    }
    #[cfg(test)]
    state.run_response_publication_hook(
        TerminalSnapshotResponsePublicationStage::AfterParentSync,
        &temporary,
        &destination,
    );
    if published.is_some() {
        let _ = reconcile_published_response(
            state,
            &retained_directory,
            &destination,
            &file,
            &written_identity,
            &directory_identity,
        )?;
    }
    Ok(())
}

fn reconcile_published_response(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    retained_directory: &crate::path_identity::RetainedDirectory,
    destination: &Path,
    file: &std::fs::File,
    expected: &crate::path_identity::VerifiedPathIdentity,
    directory: &crate::path_identity::VerifiedPathIdentity,
) -> Result<Option<crate::path_identity::VerifiedPathIdentity>, TerminalSnapshotReasonCode> {
    match retained_directory
        .verify_current()
        .and_then(|_| retained_directory.verify_opened_regular_file(destination, file, false))
    {
        Ok(identity)
            if crate::path_identity::same_object(expected, &identity)
                && crate::path_identity::is_verified_descendant(&identity, directory) =>
        {
            Ok(Some(identity))
        }
        #[cfg(unix)]
        Err(_) => {
            if cleanup_tracked_artifact(state, destination, expected)
                == TerminalSnapshotArtifactCleanupOutcome::AlreadyAbsent
            {
                Ok(None)
            } else {
                Err(TerminalSnapshotReasonCode::ResponseUnavailable)
            }
        }
        #[cfg(not(unix))]
        Err(_)
            if retained_directory.child_is_absent(destination)
                || crate::path_identity::opened_file_is_delete_pending(file) =>
        {
            state.untrack_artifact(expected);
            Ok(None)
        }
        _ => {
            cleanup_tracked_artifact(state, destination, expected);
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        }
    }
}

pub(super) fn remove_tracked_processing(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    path: &Path,
    expected: &crate::path_identity::VerifiedPathIdentity,
) {
    cleanup_tracked_artifact(state, path, expected);
}

pub(super) fn cleanup_tracked_artifact(
    state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
    path: &Path,
    expected: &crate::path_identity::VerifiedPathIdentity,
) -> TerminalSnapshotArtifactCleanupOutcome {
    state.cleanup_artifact(path, expected)
}

#[cfg(test)]
pub(super) fn path_is_absent(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

pub(super) fn safe_remove(
    path: &Path,
    expected: &crate::path_identity::VerifiedPathIdentity,
) -> bool {
    crate::path_identity::remove_regular_file_if_same(path, expected)
}

pub(super) fn sweep_directory(
    directory: &Path,
    expected_directory: &crate::path_identity::VerifiedPathIdentity,
    response_directory: bool,
) {
    let Ok(retained_directory) = crate::path_identity::retain_directory(directory) else {
        return;
    };
    if !crate::path_identity::same_object(retained_directory.identity(), expected_directory) {
        return;
    }
    let Ok(entries) = terminal_snapshots::snapshot_entries(directory) else {
        return;
    };
    for (path, _) in entries.take(MAX_DIRECTORY_ENTRIES).filter_map(Result::ok) {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !protocol_cleanup_name(name, response_directory) {
            continue;
        }
        let old = terminal_snapshots::snapshot_regular_file_age(&path)
            .is_some_and(|age| age >= RESPONSE_TTL);
        if !old {
            continue;
        }
        let Ok(identity) = crate::path_identity::verify_regular_file(&path) else {
            continue;
        };
        safe_remove(&path, &identity);
    }
    let _ = retained_directory.verify_current();
}

pub(super) fn protocol_cleanup_name(name: &str, response_directory: bool) -> bool {
    if protocol_request_name(name).is_some() {
        return true;
    }
    let Some(name) = name.strip_prefix('.') else {
        return false;
    };
    let suffixes: &[(&str, bool)] = if response_directory {
        &[(".terminal-snapshot-response-tmp", false)]
    } else {
        &[
            (".terminal-snapshot-request-tmp", true),
            (".terminal-snapshot-processing", false),
            (".terminal-snapshot-cancelled", true),
        ]
    };
    for (suffix, nonce_is_hex) in suffixes {
        let Some(prefix) = name.strip_suffix(suffix) else {
            continue;
        };
        let Some((request_id, nonce)) = prefix.split_once('.') else {
            return false;
        };
        if terminal_snapshot_renderer::validate_uuid(request_id, Some(4)).is_err() {
            return false;
        }
        return if *nonce_is_hex {
            terminal_snapshot_renderer::validate_hex(nonce, 64).is_ok()
        } else {
            terminal_snapshot_renderer::validate_uuid(nonce, Some(4)).is_ok()
        };
    }
    false
}

pub(super) fn protocol_request_name(name: &str) -> Option<&str> {
    let id = name.strip_suffix(".json")?;
    terminal_snapshot_renderer::validate_uuid(id, Some(4)).ok()?;
    Some(id)
}
