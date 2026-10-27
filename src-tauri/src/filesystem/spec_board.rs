use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

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

pub(crate) async fn create_spec_directory(path: &Path) -> io::Result<()> {
    tokio::fs::create_dir_all(path).await
}

pub(crate) async fn write_spec_stage(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    tokio::fs::write(path, bytes).await
}

pub(crate) async fn read_spec_bytes(path: &Path) -> io::Result<Vec<u8>> {
    tokio::fs::read(path).await
}

pub(crate) async fn remove_spec_snapshot(path: &Path) -> io::Result<()> {
    tokio::fs::remove_file(path).await
}

pub(crate) async fn commit_spec_stage(temp: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        replace_file_with_temp(temp, target)
    }
    #[cfg(not(windows))]
    {
        tokio::fs::rename(temp, target).await
    }
}

#[cfg(windows)]
fn replace_file_with_temp(temp: &Path, target: &Path) -> std::io::Result<()> {
    let temp_wide: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let target_wide: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;

    let replaced = unsafe { MoveFileExW(temp_wide.as_ptr(), target_wide.as_ptr(), flags) };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) async fn visit_snapshot_contents(
    dir: &Path,
    mut visit: impl FnMut(PathBuf, io::Result<Vec<u8>>),
) -> Result<(), String> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Failed to read snapshot directory: {}", e)),
    };
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read snapshot entry: {}", e))?
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let bytes = tokio::fs::read(&path).await;
        visit(path, bytes);
    }
    Ok(())
}

pub(crate) async fn snapshot_file_metadata(
    dir: &Path,
) -> Result<Vec<(PathBuf, u64, Option<std::time::SystemTime>)>, String> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Failed to read snapshot directory: {}", e)),
    };
    let mut files = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read snapshot entry: {}", e))?
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry
            .metadata()
            .await
            .map_err(|e| format!("Failed to read snapshot metadata: {}", e))?;
        files.push((path, metadata.len(), metadata.modified().ok()));
    }
    Ok(files)
}
