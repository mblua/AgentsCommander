//! Physical access to SelfClear outbox pointers, messages, and replies.

use std::io;
use std::path::Path;

pub(crate) fn read_self_clear_outbox_pointer(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn self_clear_path_is_directory(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn create_self_clear_outbox(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn write_self_clear_outbox_message(path: &Path, content: &str) -> io::Result<()> {
    std::fs::write(path, content)
}

pub(crate) fn self_clear_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn read_self_clear_reply(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}
