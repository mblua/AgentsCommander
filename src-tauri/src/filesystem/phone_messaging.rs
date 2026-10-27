//! Physical message and archive stage IO; excludes naming, formats, and publication policy.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(crate) fn ensure_message_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub(crate) fn canonical_message_path(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

pub(crate) fn message_symlink_metadata(path: &Path) -> io::Result<Metadata> {
    fs::symlink_metadata(path)
}

pub(crate) fn message_metadata(path: &Path) -> io::Result<Metadata> {
    fs::metadata(path)
}

pub(crate) fn message_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn read_message_snapshot(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}

pub(crate) fn create_message_stage(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

pub(crate) fn write_message_stage(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)
}

pub(crate) fn flush_message_stage(file: &mut File) -> io::Result<()> {
    file.flush()
}

pub(crate) fn link_message_archive(source: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(source, destination)
}

pub(crate) fn remove_message_stage(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}
