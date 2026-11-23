//! Filesystem primitives for settings sources: text and byte reads, metadata probes,
//! canonical paths, creating, writing, syncing and removing stage files, renaming
//! backups and syncing the parent directory. Owns no types and does no parsing.

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

/// Names of the regular files directly in `dir`, sorted; a missing `dir` lists nothing.
pub(crate) fn list_file_names(dir: &Path) -> io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !is_regular_file(&entry.path()) {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

#[allow(dead_code, reason = "I4845 staged; removed in P10")]
pub(crate) fn create_dir_all(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[allow(dead_code, reason = "I4845 staged; removed in P10")]
pub(crate) fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

#[allow(dead_code, reason = "I4845 staged; removed in P10")]
pub(crate) fn is_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_dir())
}
