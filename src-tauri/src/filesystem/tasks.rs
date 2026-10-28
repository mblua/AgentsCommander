//! Filesystem reads for task title text.

use std::path::Path;

pub(crate) fn read_task_title_text(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}
