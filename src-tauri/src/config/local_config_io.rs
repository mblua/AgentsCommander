mod config_pair;
mod json_object;
mod pair_image;
mod pair_reservation;
mod pair_revision;
mod prepared_pair;
mod publish;
mod source_publication;
mod staging;
mod write_lock;

#[cfg(test)]
use config_pair::update_config_pair;
pub(crate) use config_pair::update_config_pair_guarded;
pub(super) use config_pair::update_config_pair_inner;
#[cfg(test)]
use json_object::update_config_json_object_with_publish;
pub(super) use json_object::write_file_atomic_with_publish;
pub use json_object::{update_config_json_object, write_file_atomic};
pub(super) use pair_image::ConfigPublisher;
use pair_image::{
    log_unverified_cleanup_refusal, ordinary_publish_result, prepared_pair_image, PairTargetGuard,
};
pub(crate) use pair_image::{
    read_config_pair_physical, ConfigPairCleanup, ConfigPairFinish, PhysicalState,
    PreparedConfigPairPlan, PreparedPairError,
};
#[cfg(test)]
use pair_reservation::execute_reserved_config_pair_with_stage;
use pair_reservation::land_pair_activity;
#[cfg(test)]
pub(crate) use pair_reservation::{
    execute_reserved_config_pair, prepare_and_reserve_config_pair, release_reserved_config_pair,
    verify_reserved_config_pair,
};
pub(crate) use pair_reservation::{
    execute_reserved_config_pair_guarded, prepare_and_reserve_config_pair_guarded,
    release_reserved_config_pair_guarded, verify_reserved_config_pair_guarded, PairCompleteProof,
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
use pair_revision::{
    read_pair_reservation, read_pair_tuple, validate_pair_binding, validate_pair_paths,
};
#[cfg(test)]
pub(crate) use prepared_pair::execute_prepared_config_pair;
#[cfg(test)]
use prepared_pair::execute_prepared_config_pair_with_stage;
#[cfg(test)]
use prepared_pair::publish_prepared_pair_image_with_io;
pub(crate) use prepared_pair::{execute_unreserved_config_pair, prepare_config_pair_plan};
use prepared_pair::{
    prepare_config_pair_plan_inner, publish_prepared_pair_image,
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
use source_publication::publish_coordination_bytes_guarded;
#[cfg(test)]
pub(crate) use source_publication::publish_coordination_bytes_with_validation;
pub(crate) use source_publication::{
    prepare_source_publication, publish_source_coordination_bytes_with_validation,
    reopen_source_publication, verify_prepared_source_target, PreparedSourceCapability,
    PreparedSourcePublication,
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
    acquire_fresh_config_stage, acquire_fresh_config_stage_with_sequence,
    next_shared_staging_sequence, ordinary_staging_error, publish_pair_sides, read_pair_side,
};
#[cfg(test)]
use staging::{fresh_config_stage_path, next_staging_sequence, MAX_STAGING_ATTEMPTS};
#[cfg(test)]
use std::cell::Cell;
#[cfg(test)]
use std::io::Write;
#[cfg(test)]
use write_lock::config_lock_path;
use write_lock::{
    acquire_config_file_write_lock, lock_local_config_writes, ConfigWriterActive,
    CONFIG_LOCK_TIMEOUT,
};
pub(crate) use write_lock::{acquire_sidecar_write_lock, SidecarWriteLock};

#[cfg(test)]
mod tests;
