//! Physical operations for capture state; paths and persistence policy belong to capture.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) type CaptureLockFile = File;
pub(crate) type CaptureTryLockError = fs::TryLockError;

pub(crate) fn canonical_capture_room(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

pub(crate) fn read_capture_state(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

pub(crate) fn ensure_capture_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub(crate) fn open_capture_lock(path: &Path) -> io::Result<CaptureLockFile> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

pub(crate) fn try_capture_lock(file: &CaptureLockFile) -> Result<(), CaptureTryLockError> {
    file.try_lock()
}

pub(crate) fn write_capture_stage(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}

pub(crate) fn remove_capture_stage(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

pub(crate) fn replace_capture_state(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)
}
