mod config_pair;
mod json_object;
mod pair_reservation;
mod pair_revision;
mod prepared_pair;
mod publish;
mod source_publication;
mod staging;
mod write_lock;

#[cfg(test)]
use config_pair::update_config_pair;
pub(crate) use config_pair::{update_config_pair_guarded, ConfigPairCleanup, ConfigPairFinish};
pub(super) use config_pair::{update_config_pair_inner, ConfigPublisher};
#[cfg(test)]
use json_object::update_config_json_object_with_publish;
pub(super) use json_object::write_file_atomic_with_publish;
pub use json_object::{update_config_json_object, write_file_atomic};
#[cfg(test)]
use pair_reservation::execute_reserved_config_pair_with_stage;
#[cfg(test)]
pub(crate) use pair_reservation::{
    execute_reserved_config_pair, prepare_and_reserve_config_pair, release_reserved_config_pair,
    verify_reserved_config_pair,
};
use pair_reservation::{
    execute_reserved_config_pair_with_validation, land_pair_activity, read_pair_reservation,
    read_pair_tuple, validate_pair_binding, validate_pair_paths,
};
pub(crate) use pair_reservation::{
    prepare_and_reserve_config_pair_guarded, release_reserved_config_pair_guarded,
    verify_reserved_config_pair_guarded, PairCompleteProof,
};
use pair_revision::{
    changed_pair_sides, pair_plan_digest, protected_matches, protected_plan, protected_revision,
    timestamp_cmp, timestamp_only_overlay, timestamp_physical, timestamp_value, PairActivityIntent,
    TimestampPhysical,
};
pub(crate) use pair_revision::{
    merge_monotonic_activity, protected_config_pair_revision, PairReservation,
    PairReservationRequest, ProtectedRevision,
};
#[cfg(test)]
pub(crate) use prepared_pair::execute_prepared_config_pair;
#[cfg(test)]
use prepared_pair::execute_prepared_config_pair_with_stage;
#[cfg(test)]
use prepared_pair::publish_prepared_pair_image_with_io;
pub(crate) use prepared_pair::{
    execute_unreserved_config_pair, prepare_config_pair_plan, read_config_pair_physical,
    PhysicalState, PreparedConfigPairPlan, PreparedPairError,
};
use prepared_pair::{
    prepare_config_pair_plan_inner, prepared_pair_image, publish_prepared_pair_image,
    publish_prepared_pair_image_guarded, publish_prepared_pair_image_with_source_parent,
    SourceNativePublication,
};
pub(super) use publish::publish_temp_config;
use publish::publish_temp_config_guarded;
#[cfg(test)]
use publish::publish_temp_config_with_attempt;
#[cfg(test)]
use publish::{
    format_publish_error, is_transient_publish_error, publish_temp_config_retry_with_attempt,
    PUBLISH_BACKOFFS_MS, TRANSIENT_PUBLISH_OS_ERRORS,
};
#[cfg(all(test, windows))]
use publish::{publish_path_wide, replace_temp_config_once};
#[cfg(test)]
pub(crate) use source_publication::publish_coordination_bytes_with_validation;
pub(crate) use source_publication::{
    prepare_source_publication, publish_source_coordination_bytes_with_validation,
    reopen_source_publication, verify_prepared_source_target, PreparedSourceCapability,
    PreparedSourcePublication,
};
use source_publication::{
    publish_coordination_bytes_guarded, validate_source_publication_children,
};
#[cfg(test)]
pub(crate) use source_publication::{
    source_checkpoint_native_calls, source_guard_publication_calls,
};
#[cfg(test)]
use staging::publish_pair_side;
#[cfg(test)]
pub(super) use staging::temp_config_path;
use staging::{
    acquire_fresh_config_stage, acquire_fresh_config_stage_with_sequence, next_staging_sequence,
    ordinary_staging_error, publish_pair_sides, read_pair_side, STAGING_SEQUENCE,
};
#[cfg(test)]
use staging::{fresh_config_stage_path, MAX_STAGING_ATTEMPTS};
use std::cell::Cell;
#[cfg(test)]
use std::io::Write;
use std::path::Path;
#[cfg(test)]
use write_lock::config_lock_path;
use write_lock::{
    acquire_config_file_write_lock, lock_local_config_writes, ConfigWriterActive,
    CONFIG_LOCK_TIMEOUT,
};
pub(crate) use write_lock::{acquire_sidecar_write_lock, SidecarWriteLock};

#[cfg(test)]
mod tests;

// Synchronous borrowed validation only; no filesystem authority is constructed here.
struct PairTargetGuard<'a> {
    validate: &'a dyn Fn() -> Result<(), PreparedPairError>,
    failed: Cell<bool>,
}

impl<'a> PairTargetGuard<'a> {
    fn new(validate: &'a dyn Fn() -> Result<(), PreparedPairError>) -> Self {
        Self {
            validate,
            failed: Cell::new(false),
        }
    }

    fn check(&self) -> Result<(), PreparedPairError> {
        if self.failed.get() {
            return Err(PreparedPairError::Conflict);
        }
        match (self.validate)() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.failed.set(true);
                Err(error)
            }
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn execute_reserved_config_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    execute_reserved_config_pair_with_validation(
        decisions,
        state,
        reservation,
        operation_id,
        &|_| Ok(()),
        validate,
    )
}

fn log_unverified_cleanup_refusal(primary: &PreparedPairError, refusal: &PreparedPairError) {
    log::warn!(
        "[prepared-pair] cleanup refused; primary={primary}; refusal={refusal}; staging retained"
    );
}

fn ordinary_publish_result(result: Result<(), PreparedPairError>) -> Result<(), String> {
    result.map_err(|error| match error {
        PreparedPairError::Io(reason) => reason,
        other => other.to_string(),
    })
}
