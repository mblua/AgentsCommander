//! Physical operations for project creation and AC root exclusions.

use std::io;
use std::path::Path;

pub(crate) fn create_project_directory_all(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn create_project_ac_root(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

pub(crate) fn ac_root_gitignore_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn read_ac_root_gitignore(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn write_ac_root_gitignore(path: &Path, content: &str) -> io::Result<()> {
    std::fs::write(path, content)
}
