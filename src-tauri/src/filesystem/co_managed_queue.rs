//! Physical co-managed queue and message operations; policy stays with callers.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(crate) type CoManagedQueueFile = File;

pub(crate) fn ensure_co_managed_queue(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub(crate) fn create_co_managed_queue_entry(path: &Path) -> io::Result<CoManagedQueueFile> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    options.open(path)
}

pub(crate) fn write_co_managed_queue_bytes(
    file: &mut CoManagedQueueFile,
    bytes: &[u8],
) -> io::Result<()> {
    file.write_all(bytes)
}

pub(crate) fn flush_co_managed_queue_file(file: &mut CoManagedQueueFile) -> io::Result<()> {
    file.flush()
}

pub(crate) fn remove_co_managed_message(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

pub(crate) fn canonical_co_managed_message(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

pub(crate) fn read_co_managed_message_snapshot(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}
