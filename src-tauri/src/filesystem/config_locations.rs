//! Physical filesystem operations for AC roots, replica locations and instance directories.

use std::fs::Metadata;
use std::path::{Path, PathBuf};

pub(crate) fn canonical_ac_root(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn shared_directory_metadata(path: &Path) -> std::io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn create_shared_directory(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

pub(crate) fn ensure_shared_directories(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn ac_root_is_directory(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn canonical_replica_location(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn replica_matrix_metadata(path: &Path) -> std::io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn replica_role_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn ensure_instance_outbox(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}
