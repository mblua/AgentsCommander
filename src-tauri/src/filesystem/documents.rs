//! Disk operations for repository roles, prompt suites and capture catalogs.

use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn read_repository_role(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn prompt_suite_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn read_prompt_suite(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn canonical_prompt_suite_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn capture_catalog_is_file(path: &Path) -> bool {
    path.is_file()
}
