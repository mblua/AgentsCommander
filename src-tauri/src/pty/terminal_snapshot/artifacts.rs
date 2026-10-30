#[cfg(all(test, unix))]
use super::TerminalSnapshotUnixCleanupControl;
use super::{
    decrement_counter, TerminalSnapshotArtifactCleanupTaskScope, TerminalSnapshotState,
    SNAPSHOT_ARTIFACT_DIRECTORY_CAP, SNAPSHOT_ARTIFACT_FILE_CAP, SNAPSHOT_ARTIFACT_TTL,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use terminal_snapshot_renderer::TerminalSnapshotReasonCode;

impl TerminalSnapshotState {
    pub(crate) fn start_artifact_cleanup(
        self: &Arc<Self>,
        task_scope: &impl TerminalSnapshotArtifactCleanupTaskScope,
    ) {
        let state = Arc::clone(self);
        task_scope.spawn_owned(Box::pin(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = state.shutdown.token().cancelled() => {
                        let cleanup = Arc::clone(&state);
                        let _ =
                            tokio::task::spawn_blocking(move || cleanup.sweep_artifacts(true))
                                .await;
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        let cleanup = Arc::clone(&state);
                        let _ =
                            tokio::task::spawn_blocking(move || cleanup.sweep_artifacts(false))
                                .await;
                    }
                }
            }
        }));
    }

    pub(crate) fn reserve_artifact(
        &self,
        directory_path: &Path,
        directory_identity: &crate::path_identity::VerifiedPathIdentity,
    ) -> Result<TerminalSnapshotArtifactReservation, TerminalSnapshotReasonCode> {
        self.reserve_artifact_inner(directory_path, directory_identity, None)
    }

    pub(crate) fn reserve_existing_artifact(
        &self,
        directory_path: &Path,
        directory_identity: &crate::path_identity::VerifiedPathIdentity,
        object: crate::path_identity::FileObjectId,
    ) -> Result<TerminalSnapshotArtifactReservation, TerminalSnapshotReasonCode> {
        self.reserve_artifact_inner(directory_path, directory_identity, Some(object))
    }

    pub(super) fn reserve_artifact_inner(
        &self,
        directory_path: &Path,
        directory_identity: &crate::path_identity::VerifiedPathIdentity,
        existing_object: Option<crate::path_identity::FileObjectId>,
    ) -> Result<TerminalSnapshotArtifactReservation, TerminalSnapshotReasonCode> {
        let retained = crate::path_identity::retain_directory(directory_path)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        if !crate::path_identity::same_object(retained.identity(), directory_identity) {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        let mut registry = self
            .artifacts
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        if !registry
            .directories
            .contains_key(&directory_identity.object_id)
            && registry.directories.len() >= SNAPSHOT_ARTIFACT_DIRECTORY_CAP
        {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        if registry.files.len().saturating_add(registry.reservations) >= SNAPSHOT_ARTIFACT_FILE_CAP
            && !existing_object.is_some_and(|object| registry.files.contains_key(&object))
        {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        let directory = TrackedArtifactDirectory {
            identity: directory_identity.clone(),
            retained,
        };
        registry
            .directories
            .entry(directory_identity.object_id)
            .or_insert_with(|| directory.clone());
        registry.reservations += 1;
        *registry
            .directory_reservations
            .entry(directory_identity.object_id)
            .or_default() += 1;
        drop(registry);
        Ok(TerminalSnapshotArtifactReservation {
            registry: Arc::clone(&self.artifacts),
            directory,
            active: true,
        })
    }

    #[cfg(not(unix))]
    pub(crate) fn untrack_artifact(&self, identity: &crate::path_identity::VerifiedPathIdentity) {
        if let Ok(mut registry) = self.artifacts.lock() {
            if let Some(removed) = registry.files.remove(&identity.object_id) {
                remove_idle_artifact_directory(&mut registry, removed.directory);
            }
        }
    }

    #[cfg(unix)]
    pub(super) fn begin_unix_artifact_operation(
        &self,
        expected: &crate::path_identity::VerifiedPathIdentity,
    ) -> Result<UnixArtifactOperation, TerminalSnapshotArtifactCleanupOutcome> {
        let mut registry = self
            .artifacts
            .lock()
            .map_err(|_| TerminalSnapshotArtifactCleanupOutcome::Uncertain)?;
        let tracked = registry
            .files
            .get(&expected.object_id)
            .ok_or(TerminalSnapshotArtifactCleanupOutcome::Conflict)?;
        if tracked.identity.object_id != expected.object_id {
            return Err(TerminalSnapshotArtifactCleanupOutcome::Conflict);
        }
        if tracked.operation_owner.is_some() {
            return Err(TerminalSnapshotArtifactCleanupOutcome::Busy);
        }
        let token = tracked
            .operation_generation
            .checked_add(1)
            .ok_or(TerminalSnapshotArtifactCleanupOutcome::Uncertain)?;
        let directory = registry
            .directories
            .get(&tracked.directory)
            .cloned()
            .ok_or(TerminalSnapshotArtifactCleanupOutcome::Conflict)?;
        let operation = UnixArtifactOperation {
            object: expected.object_id,
            token,
            directory,
            path: tracked.path.clone(),
            identity: tracked.identity.clone(),
            witness: tracked.witness.clone(),
        };
        let tracked = registry
            .files
            .get_mut(&expected.object_id)
            .ok_or(TerminalSnapshotArtifactCleanupOutcome::Conflict)?;
        tracked.operation_generation = token;
        tracked.operation_owner = Some(token);
        Ok(operation)
    }

    #[cfg(unix)]
    pub(super) fn clear_unix_artifact_operation(
        &self,
        operation: &UnixArtifactOperation,
        outcome: TerminalSnapshotArtifactCleanupOutcome,
    ) -> TerminalSnapshotArtifactCleanupOutcome {
        let Ok(mut registry) = self.artifacts.lock() else {
            return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
        };
        let Some(tracked) = registry.files.get_mut(&operation.object) else {
            return TerminalSnapshotArtifactCleanupOutcome::Conflict;
        };
        if tracked.operation_generation != operation.token
            || tracked.operation_owner != Some(operation.token)
        {
            return TerminalSnapshotArtifactCleanupOutcome::Conflict;
        }
        tracked.operation_owner = None;
        outcome
    }

    #[cfg(unix)]
    pub(super) fn finish_unix_artifact_cleanup(
        &self,
        operation: UnixArtifactOperation,
        outcome: crate::path_identity::UnixTrackedCleanupOutcome,
    ) -> TerminalSnapshotArtifactCleanupOutcome {
        match outcome {
            crate::path_identity::UnixTrackedCleanupOutcome::Removed
            | crate::path_identity::UnixTrackedCleanupOutcome::AlreadyAbsent => {
                let public_outcome = if matches!(
                    outcome,
                    crate::path_identity::UnixTrackedCleanupOutcome::Removed
                ) {
                    TerminalSnapshotArtifactCleanupOutcome::Removed
                } else {
                    TerminalSnapshotArtifactCleanupOutcome::AlreadyAbsent
                };
                let Ok(mut registry) = self.artifacts.lock() else {
                    return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
                };
                let valid_owner = registry
                    .files
                    .get(&operation.object)
                    .is_some_and(|tracked| {
                        tracked.operation_generation == operation.token
                            && tracked.operation_owner == Some(operation.token)
                    });
                if !valid_owner {
                    return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
                }
                let Some(removed) = registry.files.remove(&operation.object) else {
                    return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
                };
                remove_idle_artifact_directory(&mut registry, removed.directory);
                public_outcome
            }
            crate::path_identity::UnixTrackedCleanupOutcome::ClaimRetained { path, identity } => {
                if !operation.witness.matches(&operation.identity, &identity)
                    || !crate::path_identity::is_verified_descendant(
                        &identity,
                        &operation.directory.identity,
                    )
                {
                    return self.clear_unix_artifact_operation(
                        &operation,
                        TerminalSnapshotArtifactCleanupOutcome::Uncertain,
                    );
                }
                let Ok(mut registry) = self.artifacts.lock() else {
                    return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
                };
                let Some(tracked) = registry.files.get_mut(&operation.object) else {
                    return TerminalSnapshotArtifactCleanupOutcome::Conflict;
                };
                if tracked.operation_generation != operation.token
                    || tracked.operation_owner != Some(operation.token)
                    || tracked.directory != operation.directory.identity.object_id
                {
                    return TerminalSnapshotArtifactCleanupOutcome::Conflict;
                }
                tracked.path = path;
                tracked.identity = identity;
                tracked.operation_owner = None;
                TerminalSnapshotArtifactCleanupOutcome::PrivateClaimRetained
            }
            crate::path_identity::UnixTrackedCleanupOutcome::SourceRetained => self
                .clear_unix_artifact_operation(
                    &operation,
                    TerminalSnapshotArtifactCleanupOutcome::SourceRetained,
                ),
            crate::path_identity::UnixTrackedCleanupOutcome::Uncertain => self
                .clear_unix_artifact_operation(
                    &operation,
                    TerminalSnapshotArtifactCleanupOutcome::Uncertain,
                ),
        }
    }

    #[cfg(unix)]
    pub(super) fn cleanup_artifact_unix(
        &self,
        expected: &crate::path_identity::VerifiedPathIdentity,
    ) -> TerminalSnapshotArtifactCleanupOutcome {
        let operation = match self.begin_unix_artifact_operation(expected) {
            Ok(operation) => operation,
            Err(outcome) => return outcome,
        };
        #[cfg(test)]
        let outcome = if let Some(control) = self.take_unix_cleanup_control() {
            let TerminalSnapshotUnixCleanupControl {
                claim_leaf,
                mut hook,
            } = control;
            operation
                .directory
                .retained
                .cleanup_unix_tracked_file_with_hook(
                    &operation.path,
                    &operation.identity,
                    &operation.witness,
                    claim_leaf.as_deref(),
                    move |stage, source, claim| hook(stage, source, claim),
                )
        } else {
            operation.directory.retained.cleanup_unix_tracked_file(
                &operation.path,
                &operation.identity,
                &operation.witness,
            )
        };
        #[cfg(not(test))]
        let outcome = operation.directory.retained.cleanup_unix_tracked_file(
            &operation.path,
            &operation.identity,
            &operation.witness,
        );
        self.finish_unix_artifact_cleanup(operation, outcome)
    }

    pub(crate) fn cleanup_artifact(
        &self,
        path: &Path,
        expected: &crate::path_identity::VerifiedPathIdentity,
    ) -> TerminalSnapshotArtifactCleanupOutcome {
        #[cfg(unix)]
        {
            let _ = path;
            self.cleanup_artifact_unix(expected)
        }
        #[cfg(not(unix))]
        {
            let retained = match self.artifacts.lock() {
                Ok(registry) => registry
                    .files
                    .get(&expected.object_id)
                    .and_then(|tracked| registry.directories.get(&tracked.directory))
                    .map(|directory| directory.retained.clone()),
                Err(_) => None,
            };
            let Some(retained) = retained else {
                return TerminalSnapshotArtifactCleanupOutcome::Conflict;
            };
            if retained.remove_regular_file_if_same(path, expected) {
                self.untrack_artifact(expected);
                return TerminalSnapshotArtifactCleanupOutcome::Removed;
            }
            match retained.verify_regular_file(path) {
                Ok(current) if crate::path_identity::same_object(expected, &current) => {
                    TerminalSnapshotArtifactCleanupOutcome::SourceRetained
                }
                _ => {
                    self.untrack_artifact(expected);
                    TerminalSnapshotArtifactCleanupOutcome::AlreadyAbsent
                }
            }
        }
    }

    #[cfg(unix)]
    pub(super) fn relocate_artifact_unix(
        &self,
        expected: &crate::path_identity::VerifiedPathIdentity,
        path: &Path,
    ) -> Result<Option<crate::path_identity::VerifiedPathIdentity>, TerminalSnapshotReasonCode>
    {
        let operation = self
            .begin_unix_artifact_operation(expected)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        if operation.witness.state(&operation.identity)
            == crate::path_identity::UnixFileWitnessState::Unlinked
        {
            let outcome = self.finish_unix_artifact_cleanup(
                operation,
                crate::path_identity::UnixTrackedCleanupOutcome::AlreadyAbsent,
            );
            return if outcome.confirmed_absent() {
                Ok(None)
            } else {
                Err(TerminalSnapshotReasonCode::ResponseUnavailable)
            };
        }
        if operation.witness.state(&operation.identity)
            != crate::path_identity::UnixFileWitnessState::Linked
        {
            self.clear_unix_artifact_operation(
                &operation,
                TerminalSnapshotArtifactCleanupOutcome::Uncertain,
            );
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        let current = match operation.directory.retained.verify_regular_file(path) {
            Ok(current)
                if operation.witness.matches(&operation.identity, &current)
                    && crate::path_identity::is_verified_descendant(
                        &current,
                        &operation.directory.identity,
                    ) =>
            {
                current
            }
            _ => {
                if operation.witness.state(&operation.identity)
                    == crate::path_identity::UnixFileWitnessState::Unlinked
                {
                    let outcome = self.finish_unix_artifact_cleanup(
                        operation,
                        crate::path_identity::UnixTrackedCleanupOutcome::AlreadyAbsent,
                    );
                    return if outcome.confirmed_absent() {
                        Ok(None)
                    } else {
                        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
                    };
                }
                self.clear_unix_artifact_operation(
                    &operation,
                    TerminalSnapshotArtifactCleanupOutcome::SourceRetained,
                );
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
        };
        let mut registry = self
            .artifacts
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        let Some(tracked) = registry.files.get_mut(&operation.object) else {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        };
        if tracked.operation_generation != operation.token
            || tracked.operation_owner != Some(operation.token)
            || tracked.directory != operation.directory.identity.object_id
        {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        tracked.path = path.to_path_buf();
        tracked.identity = current.clone();
        tracked.operation_owner = None;
        Ok(Some(current))
    }

    pub(crate) fn relocate_artifact(
        &self,
        expected: &crate::path_identity::VerifiedPathIdentity,
        path: &Path,
    ) -> Result<Option<crate::path_identity::VerifiedPathIdentity>, TerminalSnapshotReasonCode>
    {
        #[cfg(unix)]
        {
            self.relocate_artifact_unix(expected, path)
        }
        #[cfg(not(unix))]
        {
            let directory = {
                let registry = self
                    .artifacts
                    .lock()
                    .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
                let directory_object = registry
                    .files
                    .get(&expected.object_id)
                    .map(|tracked| tracked.directory)
                    .ok_or(TerminalSnapshotReasonCode::ResponseUnavailable)?;
                registry
                    .directories
                    .get(&directory_object)
                    .cloned()
                    .ok_or(TerminalSnapshotReasonCode::ResponseUnavailable)?
            };
            directory
                .retained
                .verify_current()
                .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
            let current = match directory.retained.verify_regular_file(path) {
                Ok(current) if crate::path_identity::same_object(expected, &current) => current,
                Err(_) if directory.retained.child_is_absent(path) => {
                    self.untrack_artifact(expected);
                    return Ok(None);
                }
                _ => return Err(TerminalSnapshotReasonCode::ResponseUnavailable),
            };
            if !crate::path_identity::is_verified_descendant(&current, &directory.identity) {
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
            let mut registry = self
                .artifacts
                .lock()
                .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
            let Some(tracked) = registry.files.get_mut(&expected.object_id) else {
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            };
            if tracked.directory != directory.identity.object_id {
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
            tracked.path = path.to_path_buf();
            tracked.identity = current.clone();
            Ok(Some(current))
        }
    }

    pub(super) fn sweep_artifacts(&self, force: bool) {
        #[cfg(unix)]
        {
            let files = match self.artifacts.lock() {
                Ok(registry) => registry
                    .files
                    .values()
                    .filter(|tracked| force || Instant::now() >= tracked.expires_at)
                    .map(|tracked| (tracked.path.clone(), tracked.identity.clone()))
                    .collect::<Vec<_>>(),
                Err(_) => return,
            };
            for (path, identity) in files {
                let _ = self.cleanup_artifact(&path, &identity);
            }
        }
        #[cfg(not(unix))]
        {
            let (files, directories) = match self.artifacts.lock() {
                Ok(registry) => (
                    registry.files.values().cloned().collect::<Vec<_>>(),
                    registry.directories.values().cloned().collect::<Vec<_>>(),
                ),
                Err(_) => return,
            };
            let now = Instant::now();
            let mut absent_files = Vec::new();
            for tracked in files {
                if !force && now < tracked.expires_at {
                    continue;
                }
                let Some(directory) = directories
                    .iter()
                    .find(|directory| directory.identity.object_id == tracked.directory)
                else {
                    absent_files.push(tracked.identity.object_id);
                    continue;
                };
                match directory.retained.verify_regular_file(&tracked.path) {
                    Ok(current)
                        if crate::path_identity::same_object(&current, &tracked.identity) =>
                    {
                        if directory
                            .retained
                            .remove_regular_file_if_same(&tracked.path, &tracked.identity)
                        {
                            absent_files.push(tracked.identity.object_id);
                        }
                    }
                    Ok(_) => {
                        absent_files.push(tracked.identity.object_id);
                    }
                    Err(_) if directory.retained.child_is_absent(&tracked.path) => {
                        absent_files.push(tracked.identity.object_id);
                    }
                    _ => {}
                }
            }
            let mut registry = match self.artifacts.lock() {
                Ok(registry) => registry,
                Err(_) => return,
            };
            for object in absent_files {
                registry.files.remove(&object);
            }
            let live_directories: std::collections::HashSet<_> = registry
                .files
                .values()
                .map(|file| file.directory)
                .chain(registry.directory_reservations.keys().copied())
                .collect();
            for directory in directories {
                if !live_directories.contains(&directory.identity.object_id) {
                    registry.directories.remove(&directory.identity.object_id);
                }
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct TrackedArtifactDirectory {
    pub(crate) identity: crate::path_identity::VerifiedPathIdentity,
    pub(crate) retained: crate::path_identity::RetainedDirectory,
}

#[derive(Clone)]
pub(crate) struct TrackedArtifactFile {
    pub(crate) directory: crate::path_identity::FileObjectId,
    pub(crate) path: PathBuf,
    pub(crate) identity: crate::path_identity::VerifiedPathIdentity,
    pub(crate) expires_at: Instant,
    #[cfg(unix)]
    pub(crate) witness: crate::path_identity::RetainedUnixFileWitness,
    #[cfg(unix)]
    pub(crate) operation_generation: u64,
    #[cfg(unix)]
    pub(crate) operation_owner: Option<u64>,
}

#[cfg(unix)]
pub(crate) struct UnixArtifactOperation {
    pub(crate) object: crate::path_identity::FileObjectId,
    pub(crate) token: u64,
    pub(crate) directory: TrackedArtifactDirectory,
    pub(crate) path: PathBuf,
    pub(crate) identity: crate::path_identity::VerifiedPathIdentity,
    pub(crate) witness: crate::path_identity::RetainedUnixFileWitness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSnapshotArtifactCleanupOutcome {
    Removed,
    AlreadyAbsent,
    SourceRetained,
    #[cfg(unix)]
    PrivateClaimRetained,
    #[cfg(unix)]
    Busy,
    Conflict,
    #[cfg(unix)]
    Uncertain,
}

impl TerminalSnapshotArtifactCleanupOutcome {
    #[cfg(unix)]
    pub(crate) fn confirmed_absent(self) -> bool {
        matches!(self, Self::Removed | Self::AlreadyAbsent)
    }
}

#[derive(Default)]
pub(crate) struct ArtifactRegistry {
    pub(crate) directories: HashMap<crate::path_identity::FileObjectId, TrackedArtifactDirectory>,
    pub(crate) files: HashMap<crate::path_identity::FileObjectId, TrackedArtifactFile>,
    pub(crate) reservations: usize,
    pub(crate) directory_reservations: HashMap<crate::path_identity::FileObjectId, usize>,
}

pub(crate) struct TerminalSnapshotArtifactReservation {
    pub(crate) registry: Arc<Mutex<ArtifactRegistry>>,
    pub(crate) directory: TrackedArtifactDirectory,
    pub(crate) active: bool,
}

impl TerminalSnapshotArtifactReservation {
    pub(crate) fn commit(
        self,
        path: PathBuf,
        identity: crate::path_identity::VerifiedPathIdentity,
    ) -> Result<(), TerminalSnapshotReasonCode> {
        self.commit_with_ttl(path, identity, SNAPSHOT_ARTIFACT_TTL)
    }

    pub(crate) fn commit_with_ttl(
        mut self,
        path: PathBuf,
        identity: crate::path_identity::VerifiedPathIdentity,
        ttl: Duration,
    ) -> Result<(), TerminalSnapshotReasonCode> {
        self.directory
            .retained
            .verify_current()
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        let current_file = self
            .directory
            .retained
            .verify_regular_file(&path)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        if !crate::path_identity::same_object(&current_file, &identity)
            || !crate::path_identity::is_verified_descendant(
                &current_file,
                &self.directory.identity,
            )
        {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        #[cfg(unix)]
        let witness = self
            .directory
            .retained
            .retain_unix_file_witness(&path, &identity)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        let expires_at = Instant::now()
            .checked_add(ttl)
            .ok_or(TerminalSnapshotReasonCode::Internal)?;
        if let Some(existing) = registry.files.get_mut(&identity.object_id) {
            #[cfg(unix)]
            if existing.operation_owner.is_some() {
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
            #[cfg(unix)]
            let next_generation = existing
                .operation_generation
                .checked_add(1)
                .ok_or(TerminalSnapshotReasonCode::Internal)?;
            existing.directory = self.directory.identity.object_id;
            existing.path = path;
            existing.identity = identity;
            existing.expires_at = expires_at;
            #[cfg(unix)]
            {
                existing.witness = witness;
                existing.operation_generation = next_generation;
            }
        } else {
            registry.files.insert(
                identity.object_id,
                TrackedArtifactFile {
                    directory: self.directory.identity.object_id,
                    path,
                    identity,
                    expires_at,
                    #[cfg(unix)]
                    witness,
                    #[cfg(unix)]
                    operation_generation: 0,
                    #[cfg(unix)]
                    operation_owner: None,
                },
            );
        }
        release_artifact_reservation(&mut registry, self.directory.identity.object_id);
        self.active = false;
        Ok(())
    }
}

impl Drop for TerminalSnapshotArtifactReservation {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Ok(mut registry) = self.registry.lock() {
            release_artifact_reservation(&mut registry, self.directory.identity.object_id);
        }
    }
}

pub(super) fn release_artifact_reservation(
    registry: &mut ArtifactRegistry,
    directory: crate::path_identity::FileObjectId,
) {
    registry.reservations = registry.reservations.saturating_sub(1);
    decrement_counter(&mut registry.directory_reservations, &directory);
    remove_idle_artifact_directory(registry, directory);
}

pub(super) fn remove_idle_artifact_directory(
    registry: &mut ArtifactRegistry,
    directory: crate::path_identity::FileObjectId,
) {
    if !registry.directory_reservations.contains_key(&directory)
        && !registry
            .files
            .values()
            .any(|file| file.directory == directory)
    {
        registry.directories.remove(&directory);
    }
}
