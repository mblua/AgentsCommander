//! Filesystem probes for registered project directories.

use std::fs::Metadata;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

pub(crate) fn project_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn project_path_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn project_path_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::metadata(path)
}

pub(crate) fn project_path_symlink_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn canonical_project_path(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn visit_project_kind_children(
    path: &Path,
    mut visit: impl FnMut(io::Result<PathBuf>) -> ControlFlow<()>,
) -> io::Result<ControlFlow<()>> {
    for entry in std::fs::read_dir(path)? {
        if visit(entry.map(|entry| entry.path())).is_break() {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// Directory identity of a canonical directory via an open handle. Follows the
/// (already-resolved) target; does not reuse `seed_manifest`'s reparse-detecting
/// open. Mirrors the reviewed handle pattern without coupling to its types.
#[cfg(windows)]
pub(crate) fn project_directory_identity(path: &Path) -> std::io::Result<(u64, u128)> {
    use std::mem::MaybeUninit;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, info.as_mut_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok((u64::from(info.dwVolumeSerialNumber), u128::from(index)))
}

#[cfg(unix)]
pub(crate) fn project_directory_identity(path: &Path) -> std::io::Result<(u64, u128)> {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::metadata(path)?;
    Ok((md.dev(), u128::from(md.ino())))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn project_directory_identity(_path: &Path) -> std::io::Result<(u64, u128)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "handle identity unavailable on this platform; canonical path equality is the fallback",
    ))
}
