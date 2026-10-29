//! Physical operations for context templates.

use std::io;
use std::path::Path;

pub(crate) fn inspect_template_source(path: impl AsRef<Path>) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn read_template_bytes(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn remove_template_entry(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn publish_template_link(
    from: impl AsRef<Path>,
    to: impl AsRef<Path>,
) -> io::Result<()> {
    std::fs::hard_link(from, to)
}

pub(crate) fn ensure_template_directory(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) struct TemplateFile(std::fs::File);
pub(crate) fn create_template_stage(path: impl AsRef<Path>) -> io::Result<TemplateFile> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(TemplateFile)
}
pub(crate) fn open_template_directory(path: impl AsRef<Path>) -> io::Result<TemplateFile> {
    std::fs::File::open(path).map(TemplateFile)
}
pub(crate) fn sync_template_test_writer(file: &std::fs::File) -> io::Result<()> {
    file.sync_all()
}
impl TemplateFile {
    pub(crate) fn sync_all(&self) -> io::Result<()> {
        self.0.sync_all()
    }
}
impl io::Write for TemplateFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes)
    }
}
