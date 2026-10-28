//! Physical filesystem operations for agent creation and memory directories.

use std::fs::{Metadata, ReadDir};
use std::path::{Path, PathBuf};

pub(crate) fn create_agent_directory(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

pub(crate) fn write_agent_prompt(path: &Path, prompt: &str) -> std::io::Result<()> {
    std::fs::write(path, prompt)
}

pub(crate) fn create_memory_directory(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

pub(crate) fn canonical_memory_root(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn memory_directory_metadata(path: &Path) -> std::io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn memory_directory_entries(path: &Path) -> std::io::Result<ReadDir> {
    std::fs::read_dir(path)
}

pub(crate) fn rename_memory_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}
