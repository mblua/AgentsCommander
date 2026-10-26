//! Filesystem probes for registered project directories.

use std::fs::{FileType, Metadata};
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

pub(crate) fn visit_team_project_children_strict(
    base: &Path,
    scanned: &mut usize,
    mut visit: impl FnMut(String, PathBuf) -> Result<(), String>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(base).map_err(|_| "unsafe_path".to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "unsafe_path".to_string())?;
        *scanned = scanned.saturating_add(1);
        if *scanned > 1_024 {
            return Err("invalid_target".to_string());
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "unsafe_path".to_string())?;
        if name.starts_with('.') {
            continue;
        }
        let metadata = entry.metadata().map_err(|_| "unsafe_path".to_string())?;
        if !metadata.is_dir() {
            continue;
        }

        visit(name, entry.path())?;
    }
    Ok(())
}

pub(crate) fn visit_discovery_project_children(base: &Path, mut visit: impl FnMut(PathBuf)) {
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(path);
            }
        }
    }
}

pub(crate) fn visit_profile_project_children(base: &Path, mut visit: impl FnMut(PathBuf)) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            visit(entry.path());
        }
    }
}

pub(crate) fn visit_source_inventory_children(
    base: &Path,
    mut cut: impl FnMut(&'static str) -> io::Result<()>,
    mut visit: impl FnMut(PathBuf, FileType),
) -> io::Result<()> {
    cut("readDir")?;
    for item in std::fs::read_dir(base)? {
        cut("entry")?;
        let item = item?;
        cut("fileType")?;
        let kind = item.file_type()?;
        visit(item.path(), kind);
    }
    Ok(())
}

pub(crate) fn canonical_refresh_project(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}
