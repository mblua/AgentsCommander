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
#[cfg(test)]
use pair_image::PairTargetGuard;
pub(crate) use pair_image::{
    read_config_pair_physical, ConfigPairCleanup, ConfigPairFinish, PhysicalState,
    PreparedConfigPairPlan, PreparedPairError,
};
#[cfg(test)]
use pair_reservation::execute_reserved_config_pair_with_stage;
#[cfg(test)]
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
pub(crate) use pair_revision::{
    protected_config_pair_revision, PairReservation, PairReservationRequest, ProtectedRevision,
};
#[cfg(test)]
use pair_revision::{protected_matches, protected_revision, timestamp_value};
#[cfg(test)]
use pair_revision::{read_pair_reservation, read_pair_tuple};
#[cfg(test)]
pub(crate) use prepared_pair::execute_prepared_config_pair;
#[cfg(test)]
use prepared_pair::execute_prepared_config_pair_with_stage;
#[cfg(test)]
use prepared_pair::publish_prepared_pair_image_with_io;
pub(crate) use prepared_pair::{execute_unreserved_config_pair, prepare_config_pair_plan};
#[cfg(test)]
use prepared_pair::{publish_prepared_pair_image, publish_prepared_pair_image_guarded};
pub(super) use publish::publish_temp_config;
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
#[cfg(test)]
pub(crate) use source_publication::{
    source_checkpoint_native_calls, source_guard_publication_calls,
};
#[cfg(test)]
use staging::publish_pair_side;
#[cfg(test)]
pub(super) use staging::temp_config_path;
#[cfg(test)]
use staging::{
    acquire_fresh_config_stage, acquire_fresh_config_stage_with_sequence,
    next_shared_staging_sequence,
};
#[cfg(test)]
use staging::{fresh_config_stage_path, next_staging_sequence, MAX_STAGING_ATTEMPTS};
#[cfg(test)]
use std::cell::Cell;
#[cfg(test)]
use std::io::Write;
#[cfg(test)]
use write_lock::config_lock_path;
#[cfg(test)]
use write_lock::{
    acquire_config_file_write_lock, lock_local_config_writes, ConfigWriterActive,
    CONFIG_LOCK_TIMEOUT,
};
pub(crate) use write_lock::{acquire_sidecar_write_lock, SidecarWriteLock};

#[cfg(test)]
mod tests;
