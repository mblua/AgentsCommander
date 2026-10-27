pub(crate) mod deletion;
pub(crate) mod directories;
use std::path::Path;

pub(crate) fn agent_matrix_directory_present(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(test)]
pub(crate) use deletion::{remove_replica_directory_with, validate_delete_root_parts};

pub(crate) mod team_lock;
