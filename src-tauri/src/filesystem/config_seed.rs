//! Physical operations for configuration seed trees.
use std::io;
use std::path::{Path, PathBuf};
pub(crate) fn inspect_seed_source(path: impl AsRef<Path>) -> io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}
pub(crate) fn remove_seed_scratch(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::remove_dir_all(path)
}
pub(crate) fn seed_entries(path: impl AsRef<Path>) -> io::Result<SeedEntries> {
    std::fs::read_dir(path).map(SeedEntries)
}
pub(crate) fn ensure_seed_directory(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::create_dir_all(path)
}
pub(crate) fn copy_seed_file(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    std::fs::copy(from, to)
}
pub(crate) fn read_seed_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}
pub(crate) fn write_seed_file(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    std::fs::write(path, bytes)
}
pub(crate) fn rename_seed_tree(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    std::fs::rename(from, to)
}
pub(crate) fn inspect_seed_destination(path: impl AsRef<Path>) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}
pub(crate) struct SeedEntries(std::fs::ReadDir);
pub(crate) struct SeedEntry(std::fs::DirEntry);
impl Iterator for SeedEntries {
    type Item = io::Result<SeedEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|r| r.map(SeedEntry))
    }
}
impl SeedEntry {
    pub(crate) fn path(&self) -> PathBuf {
        self.0.path()
    }
    pub(crate) fn file_name(&self) -> std::ffi::OsString {
        self.0.file_name()
    }
    pub(crate) fn file_type(&self) -> io::Result<std::fs::FileType> {
        self.0.file_type()
    }
    #[cfg(windows)]
    pub(crate) fn metadata(&self) -> io::Result<std::fs::Metadata> {
        self.0.metadata()
    }
}
pub(crate) fn is_symlink_or_reparse(entry: &SeedEntry) -> bool {
    match entry.file_type() {
        Ok(ft) if ft.is_symlink() => return true,
        Ok(_) => {}
        Err(_) => return true,
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if let Ok(md) = entry.metadata() {
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return true;
            }
        }
    }
    false
}
#[cfg(windows)]
pub(crate) fn seed_metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x0400 != 0
}
#[cfg(not(windows))]
pub(crate) fn seed_metadata_is_reparse(_metadata: &std::fs::Metadata) -> bool {
    false
}
#[cfg(target_os = "windows")]
pub(crate) struct SeedSettingsFile(std::fs::File);
#[cfg(target_os = "windows")]
pub(crate) fn create_pi_seed_settings(path: &Path) -> io::Result<SeedSettingsFile> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(SeedSettingsFile)
}
#[cfg(target_os = "windows")]
impl io::Write for SeedSettingsFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
