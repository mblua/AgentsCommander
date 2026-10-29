//! Physical request/result IO; callers own queue paths, serialization and policy.

use std::io;
use std::path::Path;

pub(crate) fn ensure_queue_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn write_queue_temporary(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

pub(crate) fn rename_queue_file(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub(crate) fn remove_queue_file(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn read_queue_result(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn read_claimed_request(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}
