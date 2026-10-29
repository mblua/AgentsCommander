use std::io::{self, Write};
use std::path::Path;

pub(crate) fn append_orphan_archive_line(path: &Path, line: &str) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{}", line))
}

pub(crate) fn orphan_archive_length(path: &Path) -> io::Result<u64> {
    std::fs::metadata(path).map(|metadata| metadata.len())
}

pub(crate) fn orphan_archive_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn rename_orphan_archive(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub(crate) fn ensure_orphan_archive(path: &Path) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(|_file| ())
}
