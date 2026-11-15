//! Physical IO for the per-replica Unleash flag store (#4752).

use std::io;
use std::path::Path;

pub(crate) fn read_unleashed_store(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}
