//! Physical IO for mailbox notices, artifacts and documents; callers own paths and policy.

use std::fs::{File, Metadata};
use std::io::{self, Read, Take, Write};
use std::path::{Path, PathBuf};

pub(crate) fn mailbox_path_symlink_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn canonical_mailbox_path(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn mailbox_path_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::metadata(path)
}

pub(crate) fn read_mailbox_message(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn mailbox_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn mailbox_path_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn mailbox_path_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn create_mailbox_artifact_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

pub(crate) fn ensure_mailbox_archive_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn rename_mailbox_document(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub(crate) fn remove_mailbox_artifact(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn open_pty_metadata_stage(path: &Path) -> io::Result<File> {
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

pub(crate) fn write_pty_metadata_stage(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
}

pub(crate) fn sync_pty_metadata_parent(path: &Path) -> Option<io::Result<()>> {
    File::open(path)
        .ok()
        .map(|parent_handle| parent_handle.sync_all())
}

pub(crate) fn open_self_forget_summary(path: &Path) -> io::Result<File> {
    File::open(path)
}

pub(crate) fn read_self_forget_summary(
    bounded: &mut Take<File>,
    bytes: &mut Vec<u8>,
) -> io::Result<usize> {
    Read::read_to_end(bounded, bytes)
}
