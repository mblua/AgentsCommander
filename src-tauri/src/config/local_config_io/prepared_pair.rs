use super::pair_image::{
    log_unverified_cleanup_refusal, prepared_pair_image, read_config_pair_physical,
    ConfigPairCleanup, PairTargetGuard, PhysicalState, PreparedConfigPairPlan, PreparedPairError,
};
use super::pair_revision::{read_pair_reservation, validate_pair_paths};
use super::publish::publish_temp_config_guarded;
use super::staging::{acquire_fresh_config_stage_with_sequence, next_shared_staging_sequence};
use super::write_lock::{
    acquire_config_file_write_lock, lock_local_config_writes, ConfigWriterActive,
    CONFIG_LOCK_TIMEOUT,
};
#[cfg(not(windows))]
use crate::filesystem::local_config::sync_local_config_directory;
use crate::filesystem::local_config::{
    canonical_local_config_parent, flush_local_config_stage, link_absent_local_config_stage,
    remove_local_config_stage, sync_local_config_stage, verify_opened_local_config_stage,
    write_local_config_stage, LocalConfigStage,
};
use serde_json::Map;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;

/// Pure preparation. Both closures finish before any publish. Reuses the
/// writer's map equality and pretty+newline serializer; untouched bytes survive.
/// Closures must only edit the supplied maps, never enter another writer.
pub(crate) fn prepare_config_pair_plan<F>(
    before: [PhysicalState; 2],
    cleanup: Option<ConfigPairCleanup<'_>>,
    mutate: F,
) -> Result<PreparedConfigPairPlan, PreparedPairError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    let _writer = ConfigWriterActive::enter("prepare_config_pair_plan")
        .map_err(PreparedPairError::Preparation)?;
    prepare_config_pair_plan_inner(before, cleanup, mutate)
}

pub(super) fn prepare_config_pair_plan_inner<F>(
    before: [PhysicalState; 2],
    cleanup: Option<ConfigPairCleanup<'_>>,
    mutate: F,
) -> Result<PreparedConfigPairPlan, PreparedPairError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    let mut decisions = before[0].map()?;
    let mut state = before[1].map()?;
    let original_decisions = decisions.clone();
    let original_state = state.clone();
    if let Some(cleanup) = cleanup {
        cleanup(&mut decisions, &mut state).map_err(PreparedPairError::Preparation)?;
    }
    let dc = prepared_pair_image(&before[0], &original_decisions, &decisions)?;
    let sc = prepared_pair_image(&before[1], &original_state, &state)?;
    let clean_decisions = decisions.clone();
    let clean_state = state.clone();
    mutate(&mut decisions, &mut state).map_err(PreparedPairError::Preparation)?;
    let df = prepared_pair_image(&dc, &clean_decisions, &decisions)?;
    let sf = prepared_pair_image(&sc, &clean_state, &state)?;
    Ok(PreparedConfigPairPlan {
        stages: [
            before.clone(),
            [before[0].clone(), sc.clone()],
            [dc.clone(), sc],
            [dc, sf.clone()],
            [df, sf],
        ],
    })
}

#[cfg(test)]
pub(crate) fn execute_prepared_config_pair(
    decisions: &Path,
    state: &Path,
    plan: &PreparedConfigPairPlan,
) -> Result<(), PreparedPairError> {
    execute_prepared_config_pair_with_stage(decisions, state, plan, &|_| Ok(()))
}

/// One lock acquisition per side, for the whole execution. The stage seam is
/// private and exercises partial failure in the production execution body.
#[cfg(test)]
pub(super) fn execute_prepared_config_pair_with_stage(
    decisions: &Path,
    state: &Path,
    plan: &PreparedConfigPairPlan,
    on_stage: &dyn Fn(usize) -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    execute_unreserved_config_pair_inner(decisions, state, None, plan, on_stage)
}

/// Retained full-physical P10 execution refuses a durable reservation under
/// the same PAIR locks; it cannot bypass the original packet/protected-activity protocol.
pub(crate) fn execute_unreserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    plan: &PreparedConfigPairPlan,
) -> Result<(), PreparedPairError> {
    execute_unreserved_config_pair_inner(decisions, state, Some(reservation), plan, &|_| Ok(()))
}

pub(super) fn execute_unreserved_config_pair_inner(
    decisions: &Path,
    state: &Path,
    reservation: Option<&Path>,
    plan: &PreparedConfigPairPlan,
    on_stage: &dyn Fn(usize) -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    let _writer = ConfigWriterActive::enter("execute_prepared_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    plan.validate()?;
    let resolve = |path: &Path| -> Result<PathBuf, PreparedPairError> {
        let parent = path
            .parent()
            .ok_or(PreparedPairError::InvalidPlan("PAIR target has no parent"))?;
        let name = path
            .file_name()
            .ok_or(PreparedPairError::InvalidPlan("PAIR target has no name"))?;
        let parent = canonical_local_config_parent(parent)
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        Ok(parent.join(name))
    };
    let decisions = resolve(decisions)?;
    let state = resolve(state)?;
    let same_target = decisions == state
        || (cfg!(windows)
            && decisions.to_string_lossy().to_lowercase()
                == state.to_string_lossy().to_lowercase());
    if same_target {
        return Err(PreparedPairError::InvalidPlan(
            "PAIR targets must be distinct",
        ));
    }
    if let Some(path) = reservation {
        validate_pair_paths(&decisions, &state, path)?;
    }
    let _guard = lock_local_config_writes();
    let _decisions_lock = acquire_config_file_write_lock(&decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _state_lock = acquire_config_file_write_lock(&state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _reservation_lock = reservation
        .map(|path| acquire_config_file_write_lock(path, CONFIG_LOCK_TIMEOUT))
        .transpose()
        .map_err(PreparedPairError::Io)?;
    if let Some(path) = reservation {
        if read_pair_reservation(path)?.is_some() {
            return Err(PreparedPairError::Pending);
        }
    }
    let read = || -> Result<[PhysicalState; 2], PreparedPairError> {
        Ok([
            read_config_pair_physical(&decisions)?,
            read_config_pair_physical(&state)?,
        ])
    };
    let current: [PhysicalState; 2] = read()?;
    let start = plan
        .stages
        .iter()
        .rposition(|tuple| *tuple == current)
        .ok_or(PreparedPairError::Conflict)?;
    for stage in start + 1..5 {
        if read()? != plan.stages[stage - 1] {
            return Err(PreparedPairError::Conflict);
        }
        let side = usize::from(stage == 1 || stage == 3);
        if plan.stages[stage][side] != plan.stages[stage - 1][side] {
            let path = if side == 0 { &decisions } else { &state };
            publish_prepared_pair_image(
                path,
                &plan.stages[stage - 1][side],
                &plan.stages[stage][side],
            )?;
            if read()? != plan.stages[stage] {
                return Err(PreparedPairError::Conflict);
            }
            on_stage(stage)?;
        }
    }
    if read()? != plan.stages[4] {
        return Err(PreparedPairError::Conflict);
    }
    Ok(())
}

pub(super) fn publish_prepared_pair_image(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
) -> Result<(), PreparedPairError> {
    publish_prepared_pair_image_guarded(path, before, after, &PairTargetGuard::new(&|| Ok(())))
}

pub(super) fn publish_prepared_pair_image_guarded(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    publish_prepared_pair_image_with_report(
        path,
        before,
        after,
        guard,
        &log_unverified_cleanup_refusal,
    )
}

pub(super) fn publish_prepared_pair_image_with_report(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
    report_cleanup_refusal: &dyn Fn(&PreparedPairError, &PreparedPairError),
) -> Result<(), PreparedPairError> {
    publish_prepared_pair_image_with_io(
        path,
        before,
        after,
        guard,
        report_cleanup_refusal,
        &mut next_shared_staging_sequence,
        &|file, bytes| {
            write_local_config_stage(file, bytes).map_err(|e| PreparedPairError::Io(e.to_string()))
        },
    )
}

pub(super) type PreparedPairStageWrite<'a> =
    dyn Fn(&mut LocalConfigStage, &[u8]) -> Result<(), PreparedPairError> + 'a;

pub(super) fn publish_prepared_pair_image_with_io(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
    report_cleanup_refusal: &dyn Fn(&PreparedPairError, &PreparedPairError),
    next: &mut dyn FnMut() -> Result<u64, PreparedPairError>,
    write: &PreparedPairStageWrite<'_>,
) -> Result<(), PreparedPairError> {
    publish_prepared_pair_image_with_source_parent(
        path,
        before,
        after,
        guard,
        report_cleanup_refusal,
        (next, write),
        None,
    )
}

pub(super) struct SourceNativePublication<'a> {
    pub(super) parent: &'a crate::path_identity::RetainedDirectory,
    pub(super) moved: std::cell::Cell<bool>,
    pub(super) after_owned: std::cell::RefCell<Option<crate::path_identity::VerifiedPathIdentity>>,
}

pub(super) fn publish_prepared_pair_image_with_source_parent(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
    report_cleanup_refusal: &dyn Fn(&PreparedPairError, &PreparedPairError),
    stage_io: (
        &mut dyn FnMut() -> Result<u64, PreparedPairError>,
        &PreparedPairStageWrite<'_>,
    ),
    source: Option<&SourceNativePublication<'_>>,
) -> Result<(), PreparedPairError> {
    let next = stage_io.0;
    let write = stage_io.1;
    let PhysicalState::Bytes { bytes, .. } = after else {
        return Err(PreparedPairError::InvalidPlan("PAIR never deletes a side"));
    };
    guard.check()?;
    if let Some(source) = source {
        guard.check()?;
        validate_source_publication_children(source.parent, path, path)?;
    }
    // Cleanup ownership begins only after exclusive acquisition succeeds.
    let (tmp, mut file) = acquire_fresh_config_stage_with_sequence(path, guard, next)?;
    let mut stage_identity = None;
    let result = (|| {
        if let Some(source) = source {
            let parent = source.parent;
            guard.check()?;
            validate_source_publication_children(parent, &tmp, path)?;
            stage_identity = Some(
                verify_opened_local_config_stage(parent, &tmp, &file, true)
                    .map_err(PreparedPairError::Io)?,
            );
        }
        guard.check()?;
        write(&mut file, bytes)?;
        guard.check()?;
        flush_local_config_stage(&mut file).map_err(|e| PreparedPairError::Io(e.to_string()))?;
        guard.check()?;
        sync_local_config_stage(&file).map_err(|e| PreparedPairError::Io(e.to_string()))?;
        Ok(())
    })();
    drop(file);
    let result = result.and_then(|()| {
        publish_prepared_stage(
            path,
            &tmp,
            before,
            after,
            guard,
            source,
            stage_identity.as_ref(),
        )
    });
    if let Err(primary) = &result {
        if !guard.failed.get() {
            if let Err(refusal) = guard.check() {
                report_cleanup_refusal(primary, &refusal);
                return Err(refusal);
            }
            cleanup_failed_prepared_stage(path, &tmp, guard, source, stage_identity.as_ref())?;
        }
    }
    if result.is_ok() {
        guard.check()?;
    }
    result
}

fn publish_prepared_stage(
    path: &Path,
    tmp: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
    source: Option<&SourceNativePublication<'_>>,
    stage_identity: Option<&crate::path_identity::VerifiedPathIdentity>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    if read_config_pair_physical(path)? != *before {
        return Err(PreparedPairError::Conflict);
    }
    if *before == PhysicalState::Absent {
        if let Some(source) = source {
            publish_absent_source_image(path, tmp, after, guard, source, stage_identity)?;
        } else {
            // Atomic create-if-absent; rename would clobber a racing Unix creator.
            guard.check()?;
            link_absent_local_config_stage(tmp, path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    PreparedPairError::Conflict
                } else {
                    PreparedPairError::Io(e.to_string())
                }
            })?;
            guard.check()?;
            remove_local_config_stage(tmp).map_err(|e| PreparedPairError::Io(e.to_string()))?;
        }
    } else {
        publish_temp_config_guarded(tmp, path, guard)?;
    }
    guard.check()?;
    #[cfg(not(windows))]
    sync_local_config_directory(
        path.parent()
            .ok_or(PreparedPairError::InvalidPlan("PAIR target has no parent"))?,
    )
    .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    Ok(())
}

fn cleanup_failed_prepared_stage(
    path: &Path,
    tmp: &Path,
    guard: &PairTargetGuard<'_>,
    source: Option<&SourceNativePublication<'_>>,
    stage_identity: Option<&crate::path_identity::VerifiedPathIdentity>,
) -> Result<(), PreparedPairError> {
    if let Some(source) = source {
        let parent = source.parent;
        if !source.moved.get() {
            validate_source_publication_children(parent, tmp, path)?;
            if let Some(owned) = stage_identity {
                // Cleanup owns only this stage, never the published target.
                guard.check()?;
                parent.remove_regular_file_if_same(tmp, owned);
            }
        }
    } else if let Err(e) = remove_local_config_stage(tmp) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("[prepared-pair] temporary cleanup failed: {e}");
        }
    }
    Ok(())
}

fn publish_absent_source_image(
    path: &Path,
    tmp: &Path,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
    source: &SourceNativePublication<'_>,
    stage_identity: Option<&crate::path_identity::VerifiedPathIdentity>,
) -> Result<(), PreparedPairError> {
    let parent = source.parent;
    guard.check()?;
    validate_source_publication_children(parent, tmp, path)?;
    let staged = parent
        .verify_regular_file(tmp)
        .map_err(PreparedPairError::Io)?;
    if stage_identity.is_none_or(|owned| !crate::path_identity::same_object(owned, &staged)) {
        return Err(PreparedPairError::Conflict);
    }
    guard.check()?;
    // Windows remains path-based after verification; post-move checks
    // detect uncertainty rather than promising handle-based exclusion.
    let publication = parent.publish_new_file_atomic(tmp, path);
    source.moved.set(publication.is_ok());
    guard.check()?;
    validate_source_publication_children(parent, tmp, path)?;
    if let Err(error) = publication {
        // Opaque native errors never identify AlreadyExists or ownership.
        return match read_config_pair_physical(path) {
            Ok(PhysicalState::Bytes { .. })
                if parent
                    .verify_regular_file(path)
                    .is_ok_and(|foreign| !crate::path_identity::same_object(&staged, &foreign)) =>
            {
                Err(PreparedPairError::Conflict)
            }
            _ => Err(PreparedPairError::Io(error)),
        };
    }
    let landed = parent
        .verify_regular_file(path)
        .map_err(PreparedPairError::Io)?;
    if !crate::path_identity::same_object(&staged, &landed)
        || read_config_pair_physical(path)? != *after
    {
        return Err(PreparedPairError::Conflict);
    }
    *source.after_owned.borrow_mut() = Some(landed);
    Ok(())
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
