//! Physical operations for the configuration write probe; policy stays in config.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn ensure_candidate_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub(crate) fn create_probe_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

pub(crate) fn write_probe_file(file: &mut File) -> io::Result<()> {
    file.write_all(
        b"AgentsCommander write probe
",
    )
}

pub(crate) fn remove_probe_file(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}
