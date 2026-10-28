//! Physical writes of debug log dumps, excluding path and filename selection.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub(crate) async fn write_debug_log_dump(path: &Path, content: &str) -> std::io::Result<()> {
    tokio::fs::write(path, content).await
}

pub(crate) fn ensure_app_log_directory(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

pub(crate) fn open_app_log_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

pub(crate) fn opened_app_log_len(file: &File) -> io::Result<u64> {
    file.metadata().map(|metadata| metadata.len())
}

pub(crate) fn app_log_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn rename_app_log(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub(crate) fn remove_app_log(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn create_app_log(path: &Path) -> io::Result<File> {
    OpenOptions::new().create_new(true).write(true).open(path)
}

pub(crate) fn append_app_log(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)
}
