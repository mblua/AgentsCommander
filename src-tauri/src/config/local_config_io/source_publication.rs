use super::{
    acquire_fresh_config_stage, log_unverified_cleanup_refusal, next_staging_sequence,
    publish_prepared_pair_image_guarded, publish_prepared_pair_image_with_source_parent,
    read_config_pair_physical, PairTargetGuard, PhysicalState, PreparedPairError,
    SourceNativePublication, STAGING_SEQUENCE,
};
#[cfg(test)]
use std::cell::Cell;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

#[cfg(test)]
thread_local! {
    static SOURCE_GUARDED_PUBLICATION_CALLS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
/// Counts invocations on this calling thread only. Driver fixtures are synchronous;
/// worker tests return their own per-thread observations explicitly.
pub(crate) fn source_guard_publication_calls() -> usize {
    SOURCE_GUARDED_PUBLICATION_CALLS.with(std::cell::Cell::get)
}

/// P03 forwards borrowed coordination authority through every existing P22 cut.
/// The callback only validates; it must not acquire locks or advance a journal.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn publish_coordination_bytes_with_validation(
    path: &Path,
    before: &PhysicalState,
    bytes: &[u8],
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    #[cfg(test)]
    SOURCE_GUARDED_PUBLICATION_CALLS.with(|calls| calls.set(calls.get() + 1));
    publish_coordination_bytes_guarded(path, before, bytes, &PairTargetGuard::new(validate))
}

/// Durable original-object witness; no paths or authority supplied by a client.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum PreparedSourcePublication {
    Prepared {
        #[serde(rename = "stageObject")]
        stage_object: (u64, u64),
        #[serde(rename = "parentObject")]
        parent_object: (u64, u64),
        #[serde(rename = "stageLeaf")]
        stage_leaf: String,
        #[serde(rename = "intentDigest")]
        intent_digest: String,
    },
}

impl PreparedSourcePublication {
    pub(crate) fn validate_record(&self, digest: &str) -> bool {
        let Self::Prepared {
            stage_leaf,
            intent_digest,
            ..
        } = self;
        let mut parts = Path::new(stage_leaf).components();
        intent_digest == digest
            && matches!(parts.next(), Some(std::path::Component::Normal(_)))
            && parts.next().is_none()
            && !stage_leaf.contains(['/', '\\', ':'])
    }

    fn stage_path(&self, parent: &crate::path_identity::RetainedDirectory) -> PathBuf {
        let Self::Prepared { stage_leaf, .. } = self;
        parent.identity().canonical_path.join(stage_leaf)
    }

    fn matches_parent(&self, parent: &crate::path_identity::RetainedDirectory) -> bool {
        let Self::Prepared { parent_object, .. } = self;
        let id = parent.identity().object_id;
        *parent_object == (id.volume, id.file)
    }

    fn matches_object(&self, object: &crate::path_identity::VerifiedPathIdentity) -> bool {
        let Self::Prepared { stage_object, .. } = self;
        *stage_object == (object.object_id.volume, object.object_id.file)
    }
}

/// Non-clonable borrowed capability. Drop only closes the handle: even an
/// uncertain checkpoint may be durable, so this path never unlinks staging.
pub(crate) struct PreparedSourceCapability<'a> {
    parent: &'a crate::path_identity::RetainedDirectory,
    target: &'a Path,
    bytes: &'a [u8],
    stage: PathBuf,
    file: std::fs::File,
    proof: PreparedSourcePublication,
    guard: PairTargetGuard<'a>,
}

impl PreparedSourceCapability<'_> {
    pub(crate) fn proof(&self) -> &PreparedSourcePublication {
        &self.proof
    }

    pub(crate) fn check(&self) -> Result<(), PreparedPairError> {
        let result = (|| {
            self.guard.check()?;
            validate_source_publication_children(self.parent, &self.stage, self.target)?;
            let current = self
                .parent
                .verify_opened_regular_file(&self.stage, &self.file, false)
                .map_err(PreparedPairError::Io)?;
            if !self.proof.matches_parent(self.parent)
                || !self.proof.matches_object(&current)
                || read_config_pair_physical(&self.stage)?
                    != PhysicalState::from_bytes(self.bytes.to_vec())
            {
                return Err(PreparedPairError::Conflict);
            }
            self.guard.check()
        })();
        if result.is_err() {
            self.guard.failed.set(true);
        }
        result
    }

    pub(crate) fn commit(
        &mut self,
        validate_checkpoint: &dyn Fn() -> Result<(), PreparedPairError>,
    ) -> Result<(), PreparedPairError> {
        let result = (|| {
            self.guard.check()?;
            validate_checkpoint()?;
            validate_source_publication_children(self.parent, &self.stage, self.target)?;
            self.verify_stage()?;
            self.guard.check()?;
            if read_config_pair_physical(self.target)? != PhysicalState::Absent {
                return Err(PreparedPairError::Conflict);
            }
            // The complete journal/revision and original authority are read-only
            // checks immediately before this invocation's single native attempt.
            validate_checkpoint()?;
            self.guard.check()?;
            #[cfg(test)]
            SOURCE_CHECKPOINT_NATIVE_CALLS.with(|n| n.set(n.get() + 1));
            #[cfg(test)]
            SOURCE_GUARDED_PUBLICATION_CALLS.with(|n| n.set(n.get() + 1));
            let native = self
                .parent
                .publish_new_file_atomic(&self.stage, self.target);
            self.guard.check()?;
            native.map_err(PreparedPairError::Io)?;
            verify_prepared_source_target(
                self.parent,
                self.target,
                self.bytes,
                &self.proof,
                &|| self.guard.check(),
            )?;
            #[cfg(not(windows))]
            std::fs::File::open(self.target.parent().ok_or(PreparedPairError::Conflict)?)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| PreparedPairError::Io(e.to_string()))?;
            self.guard.check()
        })();
        if result.is_err() {
            self.guard.failed.set(true);
        }
        result
    }

    fn verify_stage(&mut self) -> Result<(), PreparedPairError> {
        use std::io::{Read, Seek, SeekFrom};
        let opened = self
            .parent
            .verify_opened_regular_file(&self.stage, &self.file, false)
            .map_err(PreparedPairError::Io)?;
        if !self.proof.matches_parent(self.parent) || !self.proof.matches_object(&opened) {
            return Err(PreparedPairError::Conflict);
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        let mut current = Vec::new();
        self.file
            .read_to_end(&mut current)
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        if current != self.bytes {
            return Err(PreparedPairError::Conflict);
        }
        self.guard.check()?;
        self.file
            .flush()
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        self.file
            .sync_all()
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        let current = self
            .parent
            .verify_opened_regular_file(&self.stage, &self.file, false)
            .map_err(PreparedPairError::Io)?;
        if !self.proof.matches_object(&current) {
            return Err(PreparedPairError::Conflict);
        }
        self.guard.check()
    }
}

pub(crate) fn prepare_source_publication<'a>(
    parent: &'a crate::path_identity::RetainedDirectory,
    target: &'a Path,
    bytes: &'a [u8],
    intent_digest: &str,
    validate: &'a dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<PreparedSourceCapability<'a>, PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    validate_source_publication_children(parent, target, target)?;
    PhysicalState::from_bytes(bytes.to_vec()).map()?;
    if read_config_pair_physical(target)? != PhysicalState::Absent {
        return Err(PreparedPairError::Conflict);
    }
    let (stage, mut file) = acquire_fresh_config_stage(target, &guard)?;
    let own = parent
        .verify_opened_regular_file(&stage, &file, true)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    file.write_all(bytes)
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    guard.check()?;
    file.flush()
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    file.sync_all()
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    let id = parent.identity().object_id;
    let proof = PreparedSourcePublication::Prepared {
        stage_object: (own.object_id.volume, own.object_id.file),
        parent_object: (id.volume, id.file),
        stage_leaf: stage
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(PreparedPairError::Conflict)?
            .into(),
        intent_digest: intent_digest.into(),
    };
    drop(file);
    let mut cap = reopen_source_publication(parent, target, bytes, &proof, validate)?;
    // Keep this attempt's original sticky guard, never a reset after refusal.
    cap.guard = guard;
    cap.verify_stage()?;
    Ok(cap)
}

pub(crate) fn reopen_source_publication<'a>(
    parent: &'a crate::path_identity::RetainedDirectory,
    target: &'a Path,
    bytes: &'a [u8],
    proof: &PreparedSourcePublication,
    validate: &'a dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<PreparedSourceCapability<'a>, PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    if !proof.matches_parent(parent) {
        return Err(PreparedPairError::Conflict);
    }
    let stage = proof.stage_path(parent);
    validate_source_publication_children(parent, &stage, target)?;
    // Identity is checked through the retained parent around the opened handle;
    // opening alone never proves ownership or authorizes a new stage.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&stage)
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    let mut cap = PreparedSourceCapability {
        parent,
        target,
        bytes,
        stage,
        file,
        proof: proof.clone(),
        guard,
    };
    cap.verify_stage()?;
    Ok(cap)
}

pub(crate) fn verify_prepared_source_target(
    parent: &crate::path_identity::RetainedDirectory,
    target: &Path,
    bytes: &[u8],
    proof: &PreparedSourcePublication,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    validate()?;
    validate_source_publication_children(parent, target, target)?;
    let before = parent
        .verify_regular_file(target)
        .map_err(PreparedPairError::Io)?;
    if !proof.matches_parent(parent)
        || !proof.matches_object(&before)
        || read_config_pair_physical(target)? != PhysicalState::from_bytes(bytes.to_vec())
    {
        return Err(PreparedPairError::Conflict);
    }
    let after = parent
        .verify_regular_file(target)
        .map_err(PreparedPairError::Io)?;
    if !crate::path_identity::same_object(&before, &after) {
        return Err(PreparedPairError::Conflict);
    }
    validate()
}

#[cfg(test)]
thread_local! {
    static SOURCE_CHECKPOINT_NATIVE_CALLS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn source_checkpoint_native_calls() -> usize {
    SOURCE_CHECKPOINT_NATIVE_CALLS.with(Cell::get)
}

/// Internal evidence distinguishes verified own publication from identical foreign bytes.
pub(crate) struct SourceCoordinationPublication {
    pub(crate) result: Result<(), PreparedPairError>,
    #[cfg(test)]
    pub(crate) published_identity: Option<crate::path_identity::VerifiedPathIdentity>,
    #[cfg(test)]
    pub(crate) guard_rejected: bool,
}

/// Only the new source/ledger/journal path borrows this native capability.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn publish_source_coordination_bytes_with_validation(
    path: &Path,
    before: &PhysicalState,
    bytes: &[u8],
    parent: &crate::path_identity::RetainedDirectory,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> SourceCoordinationPublication {
    if *before != PhysicalState::Absent {
        return SourceCoordinationPublication {
            result: publish_coordination_bytes_with_validation(path, before, bytes, validate),
            #[cfg(test)]
            published_identity: None,
            #[cfg(test)]
            guard_rejected: false,
        };
    }
    #[cfg(test)]
    SOURCE_GUARDED_PUBLICATION_CALLS.with(|calls| calls.set(calls.get() + 1));
    let guard = PairTargetGuard::new(validate);
    let source = SourceNativePublication {
        parent,
        moved: std::cell::Cell::new(false),
        after_owned: std::cell::RefCell::new(None),
    };
    let result = (|| {
        guard.check()?;
        let after = PhysicalState::from_bytes(bytes.to_vec());
        after.map()?;
        if read_config_pair_physical(path)? != *before {
            return Err(PreparedPairError::Conflict);
        }
        publish_prepared_pair_image_with_source_parent(
            path,
            before,
            &after,
            &guard,
            &log_unverified_cleanup_refusal,
            (
                &mut || next_staging_sequence(&STAGING_SEQUENCE),
                &|file, bytes| {
                    file.write_all(bytes)
                        .map_err(|e| PreparedPairError::Io(e.to_string()))
                },
            ),
            Some(&source),
        )
    })();
    SourceCoordinationPublication {
        result,
        #[cfg(test)]
        published_identity: source.after_owned.into_inner(),
        #[cfg(test)]
        guard_rejected: guard.failed.get(),
    }
}

pub(super) fn validate_source_publication_children(
    parent: &crate::path_identity::RetainedDirectory,
    stage: &Path,
    target: &Path,
) -> Result<(), PreparedPairError> {
    parent.verify_current().map_err(PreparedPairError::Io)?;
    for child in [stage, target] {
        let directory = child
            .parent()
            .ok_or(PreparedPairError::InvalidPlan("source target parent"))?;
        let identity =
            crate::path_identity::verify_directory(directory).map_err(PreparedPairError::Io)?;
        if child.file_name().is_none()
            || !crate::path_identity::same_object(parent.identity(), &identity)
            || !crate::path_identity::paths_equivalent(directory, &parent.identity().canonical_path)
        {
            return Err(PreparedPairError::Conflict);
        }
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn publish_coordination_bytes_guarded(
    path: &Path,
    before: &PhysicalState,
    bytes: &[u8],
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    let after = PhysicalState::from_bytes(bytes.to_vec());
    after.map()?;
    if read_config_pair_physical(path)? != *before {
        return Err(PreparedPairError::Conflict);
    }
    if before == &after {
        return Ok(());
    }
    publish_prepared_pair_image_guarded(path, before, &after, guard)
}
