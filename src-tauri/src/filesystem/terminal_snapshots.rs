//! Physical response IO and directory observations; snapshot policy stays with phone.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) fn snapshot_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn snapshot_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn snapshot_entries(
    path: &Path,
) -> io::Result<impl Iterator<Item = io::Result<(PathBuf, OsString)>>> {
    std::fs::read_dir(path)
        .map(|entries| entries.map(|entry| entry.map(|entry| (entry.path(), entry.file_name()))))
}

pub(crate) fn snapshot_regular_file_age(path: &Path) -> Option<Duration> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
}

pub(crate) fn private_snapshot_directory_mode(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.permissions().mode() & 0o777 == 0o700)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

pub(crate) fn private_snapshot_file_mode(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

#[cfg(unix)]
pub(crate) fn set_private_snapshot_response(file: &File) -> bool {
    use std::os::unix::fs::PermissionsExt;
    !(file
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .is_err()
        || file
            .metadata()
            .ok()
            .is_none_or(|metadata| metadata.permissions().mode() & 0o777 != 0o600))
}

pub(crate) fn write_snapshot_response(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes).and_then(|_| file.flush())
}

pub(crate) fn sync_snapshot_response(file: &File) -> io::Result<()> {
    file.sync_all()
}
