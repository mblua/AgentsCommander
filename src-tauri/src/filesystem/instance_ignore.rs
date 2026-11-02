//! Physical IO for the running instance ignore file.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

fn apply_safe_open_flags(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
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
    #[cfg(not(any(unix, windows)))]
    {
        let _ = options;
    }
}

pub(crate) fn open_new_ignore(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

pub(crate) fn open_read_ignore(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

pub(crate) fn open_read_write_ignore(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

pub(crate) fn open_append_ignore(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).append(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

pub(crate) fn ensure_ignore_parent(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn ignore_leaf_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn opened_ignore_metadata(file: &File) -> io::Result<Metadata> {
    file.metadata()
}

pub(crate) fn seek_ignore_start(file: &mut File) -> io::Result<u64> {
    file.seek(SeekFrom::Start(0))
}

pub(crate) fn read_ignore_bytes(file: &mut File, bytes: &mut Vec<u8>) -> io::Result<usize> {
    #[expect(
        clippy::verbose_file_reads,
        reason = "Read the safely opened caller handle without reopening its path"
    )]
    file.read_to_end(bytes)
}

pub(crate) fn write_ignore_bytes(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)
}

pub(crate) fn truncate_ignore(file: &File) -> io::Result<()> {
    file.set_len(0)
}

pub(crate) fn try_lock_ignore(file: &File) -> Result<(), std::fs::TryLockError> {
    file.try_lock()
}
