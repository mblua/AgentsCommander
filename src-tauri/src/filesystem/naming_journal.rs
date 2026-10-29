//! Physical naming migration journal and sidecar operations.

use std::fs::{File, Metadata, OpenOptions, ReadDir};
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn open_migration_sidecar(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

#[cfg(unix)]
pub(crate) fn sync_journal_directory(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_journal_directory(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
pub(crate) fn replace_journal_file(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let src_wide: Vec<u16> = src.as_os_str().encode_wide().chain(Some(0)).collect();
    let dst_wide: Vec<u16> = dst.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        MoveFileExW(
            src_wide.as_ptr(),
            dst_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn replace_journal_file(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::rename(src, dst)
}

#[cfg(windows)]
pub(crate) fn move_migration_file_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // Deliberately WITHOUT `MOVEFILE_REPLACE_EXISTING`: an existing `to` fails
    // with ERROR_ALREADY_EXISTS (183) and its bytes are never replaced.
    let ok = unsafe { MoveFileExW(from_wide.as_ptr(), to_wide.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn read_journal_bytes(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn migration_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn migration_entries(path: &Path) -> io::Result<ReadDir> {
    std::fs::read_dir(path)
}

pub(crate) fn try_lock_migration(file: &File) -> io::Result<()> {
    file.try_lock().map_err(io::Error::from)
}

pub(crate) fn create_journal_stage(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

pub(crate) fn write_journal_stage(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)
}

pub(crate) fn sync_journal_stage(file: &File) -> io::Result<()> {
    file.sync_all()
}

pub(crate) fn remove_migration_file(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

#[cfg(not(windows))]
pub(crate) fn link_migration_file(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::hard_link(from, to)
}

pub(crate) fn migration_directory_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::metadata(path)
}

pub(crate) fn migration_sidecar_exists(path: &Path) -> bool {
    path.exists()
}
