//! Physical IO for activity log documents.

use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(crate) fn append_activity(path: &Path, buffer: &str) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(buffer.as_bytes())
}

pub(crate) fn activity_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::metadata(path)
}

pub(crate) fn activity_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn rename_activity(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub(crate) fn open_activity_tail(path: &Path) -> io::Result<File> {
    File::open(path)
}

pub(crate) fn opened_activity_metadata(file: &File) -> io::Result<Metadata> {
    file.metadata()
}

pub(crate) fn seek_activity_tail(file: &mut File, position: SeekFrom) -> io::Result<u64> {
    file.seek(position)
}

pub(crate) fn read_activity_tail(file: &mut File, bytes: &mut Vec<u8>) -> io::Result<usize> {
    file.read_to_end(bytes)
}
