//! Physical operations for co-managed config; its owner retains paths and policy.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub(crate) type CoManagedLockFile = File;
pub(crate) type CoManagedTryLockError = fs::TryLockError;

pub(crate) fn read_co_managed_config(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

pub(crate) fn ensure_co_managed_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub(crate) fn open_co_managed_lock(path: &Path) -> io::Result<CoManagedLockFile> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

pub(crate) fn try_co_managed_lock(file: &CoManagedLockFile) -> Result<(), CoManagedTryLockError> {
    file.try_lock()
}

pub(crate) fn create_co_managed_stage(path: &Path) -> io::Result<CoManagedLockFile> {
    File::create(path)
}

pub(crate) fn write_co_managed_stage(file: &mut CoManagedLockFile, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)
}

pub(crate) fn flush_co_managed_stage(file: &mut CoManagedLockFile) -> io::Result<()> {
    file.flush()
}

pub(crate) fn sync_co_managed_stage(file: &CoManagedLockFile) -> io::Result<()> {
    file.sync_all()
}

pub(crate) fn remove_co_managed_stage(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

pub(crate) fn replace_co_managed_config(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)
}
