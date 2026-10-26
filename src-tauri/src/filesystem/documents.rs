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

pub(crate) fn read_local_role_template(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn local_template_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn local_template_has_entries(path: &Path) -> io::Result<bool> {
    std::fs::read_dir(path).map(|mut entries| entries.next().is_some())
}

pub(crate) fn read_configured_context(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn configured_context_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn visit_local_template_entries<B>(
    dir: &Path,
    mut visit: impl FnMut(
        io::Result<(PathBuf, std::ffi::OsString, io::Result<std::fs::Metadata>)>,
    ) -> std::ops::ControlFlow<B>,
) -> io::Result<std::ops::ControlFlow<B>> {
    for entry in std::fs::read_dir(dir)? {
        let item = entry.map(|entry| (entry.path(), entry.file_name(), entry.metadata()));
        if let std::ops::ControlFlow::Break(value) = visit(item) {
            return Ok(std::ops::ControlFlow::Break(value));
        }
    }
    Ok(std::ops::ControlFlow::Continue(()))
}

pub(crate) fn loop_prompt_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}

pub(crate) fn read_loop_prompt(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}
