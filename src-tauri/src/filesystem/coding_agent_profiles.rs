use std::ffi::OsString;
use std::fs::{self, DirEntry, FileType, Metadata, ReadDir};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn read_profile_text(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

pub(crate) fn profile_path_symlink_metadata(path: &Path) -> io::Result<Metadata> {
    fs::symlink_metadata(path)
}

pub(crate) fn canonical_profile_path(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

pub(crate) fn read_selection_default_bytes(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}

pub(crate) fn profile_path_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn open_profile_directory(path: &Path) -> io::Result<ProfileDirectory> {
    fs::read_dir(path).map(|inner| ProfileDirectory { inner })
}

pub(crate) struct ProfileDirectory {
    inner: ReadDir,
}

impl Iterator for ProfileDirectory {
    type Item = io::Result<ProfileDirectoryEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .next()
            .map(|item| item.map(|inner| ProfileDirectoryEntry { inner }))
    }
}

pub(crate) struct ProfileDirectoryEntry {
    inner: DirEntry,
}

impl ProfileDirectoryEntry {
    pub(crate) fn file_type(&self) -> io::Result<FileType> {
        self.inner.file_type()
    }

    pub(crate) fn file_name(&self) -> OsString {
        self.inner.file_name()
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.inner.path()
    }
}
