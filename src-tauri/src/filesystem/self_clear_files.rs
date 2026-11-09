//! Physical access to `SelfClear` outbox pointers, messages, and replies.

use std::io;
use std::path::Path;

pub(crate) fn self_clear_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn read_self_clear_reply(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}
