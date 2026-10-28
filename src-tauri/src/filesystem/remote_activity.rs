//! Physical IO for remote activity documents.

use std::io;
use std::path::Path;

pub(crate) fn read_activity_snapshot(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}
