//! Filesystem access for agent command setup: creates the OpenCode config directory
//! and the isolated Codex home directory. Owns no types; directory contents are
//! managed elsewhere.

use std::io;
use std::path::Path;

pub(crate) fn ensure_opencode_config_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn ensure_isolated_codex_home_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}
