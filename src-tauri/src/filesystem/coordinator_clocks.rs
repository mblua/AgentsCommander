//! Physical IO for coordinator clocks documents.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn read_clock_document(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn ensure_clock_parent(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn create_clock_stage(path: &Path) -> io::Result<File> {
    File::create(path)
}

pub(crate) fn write_clock_stage(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)
}

pub(crate) fn sync_clock_stage(file: &File) -> io::Result<()> {
    file.sync_all()
}

pub(crate) fn remove_clock_stage(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn clock_workgroup_is_directory(path: &Path) -> bool {
    path.is_dir()
}
