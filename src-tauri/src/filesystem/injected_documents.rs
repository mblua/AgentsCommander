//! Physical reads for injected message sources and sidecars.

use std::fs::Metadata;
use std::io;
use std::path::Path;

pub(crate) fn source_metadata(path: &Path) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn read_source_bytes(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn read_state_text(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}
