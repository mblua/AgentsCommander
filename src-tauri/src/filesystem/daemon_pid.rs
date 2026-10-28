//! Physical IO for daemon pid documents.

use std::io;
use std::path::Path;

pub(crate) fn ensure_pid_parent(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn write_pid_document(path: &Path, contents: String) -> io::Result<()> {
    std::fs::write(path, contents)
}

pub(crate) fn remove_pid_document(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn read_pid_document(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}
