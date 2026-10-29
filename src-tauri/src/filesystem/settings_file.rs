//! Physical operations for settings files; callers retain policy and verification.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(crate) fn ensure_settings_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn settings_path_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn canonical_settings_parent(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn read_settings_text(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn settings_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn write_settings_migration_backup(path: &Path, contents: &str) -> io::Result<()> {
    std::fs::write(path, contents)
}

pub(crate) fn rename_settings_file(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

pub(crate) fn remove_settings_temporary(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn settings_handle_metadata(file: &File) -> io::Result<Metadata> {
    file.metadata()
}

pub(crate) fn write_settings_bytes(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)
}

pub(crate) fn flush_settings_file(file: &mut File) -> io::Result<()> {
    file.flush()
}

pub(crate) fn sync_settings_file(file: &File) -> io::Result<()> {
    file.sync_all()
}

#[cfg(unix)]
pub(crate) fn set_private_settings_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(unix)]
pub(crate) fn sync_settings_directory(path: &Path) -> io::Result<()> {
    File::open(path).and_then(|directory| directory.sync_all())
}

pub(crate) fn open_settings_temporary(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

pub(crate) fn open_settings_lock(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
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

pub(crate) fn open_settings_backup_slot(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

#[cfg(unix)]
pub(crate) fn try_lock_settings_handle(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    let source = std::io::Error::last_os_error();
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    // Compared rather than matched: `EWOULDBLOCK` and `EAGAIN` are the same
    // value on Linux and macOS, which makes the second pattern unreachable.
    let raw = error.raw_os_error();
    if raw == Some(libc::EWOULDBLOCK) || raw == Some(libc::EAGAIN) {
        Ok(false)
    } else {
        Err(source)
    }
}

#[cfg(unix)]
pub(crate) fn unlock_settings_handle(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
pub(crate) fn try_lock_settings_handle(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    let result = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if result != 0 {
        return Ok(true);
    }
    const ERROR_LOCK_VIOLATION: i32 = 33;
    let source = std::io::Error::last_os_error();
    if source.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
        Ok(false)
    } else {
        Err(source)
    }
}

#[cfg(windows)]
pub(crate) fn unlock_settings_handle(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    if unsafe { UnlockFileEx(file.as_raw_handle(), 0, u32::MAX, u32::MAX, &mut overlapped) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Perform one native replacement attempt with the caller's retained wide paths.
///
/// # Safety
/// Both slices must be nonempty and end in NUL. Their references must remain
/// valid throughout the synchronous native call.
#[cfg(windows)]
pub(crate) unsafe fn replace_settings_file_windows_once(
    source: &[u16],
    destination: &[u16],
) -> io::Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    // SAFETY: the caller guarantees live, nonempty, NUL-terminated wide paths.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
