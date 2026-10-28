//! Validation and preparation of foreign Telegram attachments.

use std::fs::Metadata;
use std::future::Future;
use std::io;
use std::path::Path;

use crate::filesystem::attachments::{attachment_metadata, read_attachment_bytes, ReadError};

pub(crate) async fn prepare_telegram_attachment(
    path: &str,
    max: u64,
) -> Result<(Vec<u8>, u64), String> {
    let p = Path::new(path);
    prepare_telegram_attachment_with(path, max, attachment_metadata(p), |capacity, limit| {
        read_attachment_bytes(p, capacity, limit)
    })
    .await
}

async fn prepare_telegram_attachment_with<M, R, F>(
    path: &str,
    max: u64,
    metadata: M,
    read: R,
) -> Result<(Vec<u8>, u64), String>
where
    M: Future<Output = io::Result<Metadata>> + Send,
    R: FnOnce(usize, u64) -> F + Send,
    F: Future<Output = Result<Vec<u8>, ReadError>> + Send,
{
    let lmeta = metadata.await.map_err(|e| format!("stat failed: {}", e))?;
    if is_link_or_reparse(&lmeta) {
        return Err(format!(
            "Symlinks and reparse points are not supported in v1: {}",
            path
        ));
    }
    if !lmeta.is_file() {
        return Err(format!("Not a regular file: {}", path));
    }
    let size = lmeta.len();
    if size == 0 {
        return Err(format!("File is empty: {}", path));
    }
    if size > max {
        return Err(format!(
            "File exceeds Telegram 50 MB limit ({} bytes): {}",
            size, path
        ));
    }

    let bytes = read(size as usize, max + 1)
        .await
        .map_err(|error| match error {
            ReadError::Open(e) => format!("open failed: {}", e),
            ReadError::Read(e) => format!("read failed: {}", e),
        })?;
    if bytes.len() as u64 > max {
        return Err(format!("File grew past 50 MB during read: {}", path));
    }
    Ok((bytes, size))
}

/// True if `meta` describes a symlink or — on Windows — ANY reparse point
/// (junction, mount point, …). Mirrors the helper in `commands/role_templates.rs`;
/// keep both in sync. `FileType::is_symlink()` does not flag Windows junctions
/// or file-typed reparse points, so the raw `FILE_ATTRIBUTE_REPARSE_POINT`
/// (0x400) bit is checked as well.
fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests;
