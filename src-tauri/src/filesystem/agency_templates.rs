//! Physical IO for Agency cache trees, manifests, and role templates.

use std::fs::{self, File, Metadata, OpenOptions, ReadDir};
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn remove_agency_tree(path: &Path) -> io::Result<()> {
    fs::remove_dir_all(path)
}

pub(crate) fn ensure_agency_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub(crate) fn remove_agency_file(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

pub(crate) fn read_agency_text(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

pub(crate) fn agency_entries(path: &Path) -> io::Result<ReadDir> {
    fs::read_dir(path)
}

pub(crate) fn write_agency_text(path: &Path, text: &str) -> io::Result<()> {
    fs::write(path, text)
}

pub(crate) fn agency_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn agency_metadata(path: &Path) -> io::Result<Metadata> {
    fs::symlink_metadata(path)
}

pub(crate) fn rename_agency_path(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

pub(crate) fn copy_agency_file(from: &Path, to: &Path) -> io::Result<u64> {
    fs::copy(from, to)
}

pub(crate) fn create_agency_lock(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

pub(crate) fn write_agency_lock(file: &mut File, serialized: &str) -> io::Result<()> {
    writeln!(file, "{}", serialized)
}
