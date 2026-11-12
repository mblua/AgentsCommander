//! Narrow filesystem identity primitives for privileged PTY input.
//!
//! These helpers do not repair configuration. They reject link/reparse
//! components, bind checks to stable filesystem object IDs, and cap every
//! security-bearing read.

mod identity;
mod json;
mod regular;
mod retained;
mod terminal_output;

pub(crate) use identity::is_link_or_reparse;
#[cfg(any(windows, test))]
pub(crate) use identity::link_flags_indicate_link_or_reparse;
#[cfg(all(unix, test))]
pub(crate) use identity::UnixTrackedCleanupStage;
pub use identity::{
    retain_directory, same_object, verify_component_chain, verify_component_chain_reason,
    verify_directory, verify_directory_reason, verify_opened_regular_file, verify_regular_file,
    FileObjectId, PathRule, RetainedDirectory, VerifiedPathIdentity,
};
#[cfg(unix)]
pub(crate) use identity::{
    RetainedUnixFileWitness, UnixFileWitnessState, UnixTrackedCleanupOutcome,
};
pub use json::parse_json_no_duplicates;
pub use json::scan_json_document;
#[cfg(all(test, windows))]
use regular::atomic_replace_existing;
pub use regular::is_verified_descendant;
#[cfg(not(unix))]
pub use regular::opened_file_is_delete_pending;
pub use regular::paths_equivalent;
pub use regular::publish_new_file_atomic;
pub use regular::read_bounded_regular;
#[cfg(test)]
use regular::read_bounded_regular_inner;
pub use regular::remove_regular_file_if_same;
#[cfg(test)]
use regular::remove_regular_file_if_same_inner;
#[cfg(all(test, windows))]
use regular::remove_windows_regular_file_by_handle_inner;
pub use regular::replace_regular_file_atomic;
pub use regular::sync_parent_best_effort;
pub use terminal_output::create_terminal_snapshot_output;
#[cfg(test)]
use terminal_output::create_terminal_snapshot_output_inner;
#[cfg(test)]
use terminal_output::create_terminal_snapshot_output_with_hooks;
pub use terminal_output::validate_terminal_snapshot_output_path;
#[cfg(all(test, windows))]
use terminal_output::validate_terminal_snapshot_output_path_inner;
#[cfg(all(test, windows))]
use terminal_output::validate_windows_snapshot_output_path;

#[cfg(test)]
mod tests;
