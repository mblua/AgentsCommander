use std::io;
use std::path::Path;

pub(crate) fn ensure_opencode_config_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn ensure_isolated_codex_home_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}
