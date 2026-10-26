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
