use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(crate) fn text(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn bytes(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn probe(path: &Path) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn marker(path: &Path) -> io::Result<Metadata> {
    std::fs::metadata(path)
}

pub(crate) fn canonical(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn agents_stage(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

pub(crate) fn migration_stage(path: &Path) -> io::Result<File> {
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
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

pub(crate) fn write_stage(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)
}

pub(crate) fn sync_stage(file: &File) -> io::Result<()> {
    file.sync_all()
}

pub(crate) fn remove_stage(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn rename_backup(temp: &Path, backup: &Path) -> io::Result<()> {
    std::fs::rename(temp, backup)
}

#[cfg(unix)]
pub(crate) fn sync_parent(parent: &Path) -> io::Result<()> {
    File::open(parent).and_then(|directory| directory.sync_all())
}
