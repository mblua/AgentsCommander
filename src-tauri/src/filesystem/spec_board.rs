use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn spec_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn spec_path_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) async fn spec_path_metadata(path: &Path) -> io::Result<Metadata> {
    tokio::fs::metadata(path).await
}

pub(crate) async fn canonical_spec_path(path: &Path) -> io::Result<PathBuf> {
    tokio::fs::canonicalize(path).await
}

pub(crate) async fn spec_path_is_link_or_reparse(path: &Path) -> Result<bool, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("Failed to lstat {}: {}", path.display(), e))?;
        if meta.file_type().is_symlink() {
            return Ok(true);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|e| format!("Failed to inspect path link state: {}", e))?
}
