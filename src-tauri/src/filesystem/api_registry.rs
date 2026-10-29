//! Native physical IO for the host API credential registry.
//! The API owner retains paths, schema, validation, cache and publication policy.

use std::fs::Metadata;
use std::io::{self, Write};
use std::path::Path;
use std::time::SystemTime;

pub(crate) type RegistryFile = std::fs::File;

pub(crate) fn read_registry_text(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn registry_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub(crate) fn registry_leaf_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn ensure_registry_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn open_registry_lock(parent: &Path, lock_path: &Path) -> Result<RegistryFile, String> {
    crate::path_identity::verify_directory(parent)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(0x0002_0000);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(lock_path)
        .map_err(|_| "api_registry_lock_failed".to_string())?;
    crate::path_identity::verify_opened_regular_file(lock_path, &file, true)
        .map_err(|_| "api_registry_lock_failed".to_string())?;
    Ok(file)
}

pub(crate) fn revalidate_registry_lock(
    parent: &Path,
    lock_path: &Path,
    file: &RegistryFile,
) -> Result<(), String> {
    crate::path_identity::verify_directory(parent)
        .map_err(|_| "api_registry_lock_failed".to_string())?;
    crate::path_identity::verify_opened_regular_file(lock_path, file, true)
        .map(|_| ())
        .map_err(|_| "api_registry_lock_failed".to_string())
}

pub(crate) fn try_lock_registry(file: &RegistryFile) -> Result<(), std::fs::TryLockError> {
    file.try_lock()
}

pub(crate) fn lock_registry(file: &RegistryFile) -> io::Result<()> {
    file.lock()
}

pub(crate) fn create_registry_stage(path: &Path) -> io::Result<RegistryFile> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(0x0002_0000);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

pub(crate) fn write_sync_registry_stage(file: &mut RegistryFile, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
}

pub(crate) fn sync_registry_parent(path: &Path) -> io::Result<()> {
    if let Ok(directory) = std::fs::File::open(path) {
        directory.sync_all()?;
    }
    Ok(())
}

pub(crate) fn remove_registry_stage(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}
