//! Filesystem helpers for project entities: teams, rooms, roles and Agent Matrix directories.
//! Declares the `deletion`, `directories`, `team_lock`, `copies`, `publication` and `roles`
//! submodules and re-exports `remove_replica_directory_with` and `validate_delete_root_parts`.
//! Owns no types; `agent_matrix_directory_present` is its only own function.

pub(crate) mod deletion;
pub(crate) mod directories;
use std::path::Path;

pub(crate) fn agent_matrix_directory_present(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(test)]
pub(crate) use deletion::{remove_replica_directory_with, validate_delete_root_parts};

pub(crate) mod team_lock;

pub(crate) mod copies;
pub(crate) mod publication;
pub(crate) mod roles;
