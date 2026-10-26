use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn repo_search_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn agent_marker_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn visit_repo_search_children(
    path: &Path,
    mut visit: impl FnMut(PathBuf),
) -> io::Result<()> {
    for entry in std::fs::read_dir(path)?.flatten() {
        visit(entry.path());
    }
    Ok(())
}

pub(crate) async fn git_root_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    tokio::fs::metadata(path).await
}

pub(crate) fn spec_repo_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn spec_repo_git_marker_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) async fn canonical_spec_repo_path(path: &Path) -> io::Result<PathBuf> {
    tokio::fs::canonicalize(path).await
}

pub(crate) async fn spec_repo_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    tokio::fs::metadata(path).await
}

pub(crate) async fn spec_repo_link_state(path: &Path) -> Result<bool, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let meta = std::fs::symlink_metadata(&path)
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

pub(crate) struct SpecRepoChildren(tokio::fs::ReadDir);

pub(crate) async fn open_spec_repo_children(base: &Path) -> io::Result<SpecRepoChildren> {
    tokio::fs::read_dir(base).await.map(SpecRepoChildren)
}

impl SpecRepoChildren {
    pub(crate) async fn next_path(&mut self) -> Option<PathBuf> {
        match self.0.next_entry().await {
            Ok(Some(entry)) => Some(entry.path()),
            Err(_) | Ok(None) => None,
        }
    }
}
