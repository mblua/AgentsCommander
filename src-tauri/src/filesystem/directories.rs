//! Filesystem probes for transcript paths, peer directories and agent programs.

use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn canonical_transcript_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn canonical_peer_cwd(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn agent_program_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn registered_project_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn launch_root_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn canonical_launch_root(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn canonical_automation_executable(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn canonical_session_guard_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
