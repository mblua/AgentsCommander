//! Physical operations for session context.

use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn ensure_context_directory(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn canonical_context_source(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn write_context_cache(
    path: impl AsRef<Path>,
    bytes: impl AsRef<[u8]>,
) -> io::Result<()> {
    std::fs::write(path, bytes)
}

pub(crate) fn read_context_source(path: impl AsRef<Path>) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn inspect_context_source(path: impl AsRef<Path>) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn remove_context_entry(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn remove_context_link_directory(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::remove_dir(path)
}

pub(crate) fn context_source_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn context_cache_entry_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn context_cache_entries(path: impl AsRef<Path>) -> io::Result<ContextEntries> {
    std::fs::read_dir(path).map(ContextEntries)
}

pub(crate) struct ContextEntries(std::fs::ReadDir);
pub(crate) struct ContextEntry(std::fs::DirEntry);
impl Iterator for ContextEntries {
    type Item = io::Result<ContextEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|r| r.map(ContextEntry))
    }
}
impl ContextEntry {
    pub(crate) fn path(&self) -> PathBuf {
        self.0.path()
    }
    pub(crate) fn metadata(&self) -> io::Result<std::fs::Metadata> {
        self.0.metadata()
    }
}
