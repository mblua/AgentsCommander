//! Disk operations for foreign Telegram attachments.

use std::fs::Metadata;
use std::io;
use std::path::Path;

use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug)]
pub(crate) enum ReadError {
    Open(io::Error),
    Read(io::Error),
}

pub(crate) async fn attachment_metadata(path: &Path) -> io::Result<Metadata> {
    tokio::fs::symlink_metadata(path).await
}

pub(crate) async fn read_attachment_bytes(
    path: &Path,
    capacity: usize,
    limit: u64,
) -> Result<Vec<u8>, ReadError> {
    let file = tokio::fs::File::open(path).await.map_err(ReadError::Open)?;
    read_attachment_stream(file, capacity, limit).await
}

async fn read_attachment_stream<R: AsyncRead + Unpin + Send>(
    reader: R,
    capacity: usize,
    limit: u64,
) -> Result<Vec<u8>, ReadError> {
    let mut bytes: Vec<u8> = Vec::with_capacity(capacity);
    reader
        .take(limit)
        .read_to_end(&mut bytes)
        .await
        .map_err(ReadError::Read)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;
