//! Physical operations for context skills.

use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn canonical_skill_source(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn inspect_skill_source(path: impl AsRef<Path>) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn skill_source_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn skill_source_entries(path: impl AsRef<Path>) -> io::Result<ContextEntries> {
    std::fs::read_dir(path).map(ContextEntries)
}

pub(crate) struct ContextEntries(std::fs::ReadDir);
pub(crate) struct ContextEntry(std::fs::DirEntry);
impl Iterator for ContextEntries {
    type Item = io::Result<ContextEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|r| r.map(ContextEntry))
    }
}
impl ContextEntry {
    pub(crate) fn path(&self) -> PathBuf {
        self.0.path()
    }
    pub(crate) fn file_name(&self) -> std::ffi::OsString {
        self.0.file_name()
    }
    pub(crate) fn file_type(&self) -> io::Result<std::fs::FileType> {
        self.0.file_type()
    }
}

pub(crate) struct SkillFrontmatterReader(std::fs::File);
pub(crate) fn open_skill_frontmatter(path: &Path) -> io::Result<SkillFrontmatterReader> {
    std::fs::File::open(path).map(SkillFrontmatterReader)
}
impl io::Read for SkillFrontmatterReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.0.read(bytes)
    }
}
