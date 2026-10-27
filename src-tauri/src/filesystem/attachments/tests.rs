use super::*;

use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::ReadBuf;

#[tokio::test]
async fn attachment_missing_path_is_open_error() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let expected = tokio::fs::File::open(&missing).await.unwrap_err();
    let error = read_attachment_bytes(&missing, 1, 4).await.unwrap_err();
    let ReadError::Open(error) = error else {
        panic!("missing attachment must fail at open");
    };
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert_eq!(error.raw_os_error(), expected.raw_os_error());
    assert_eq!(error.to_string(), expected.to_string());
}

struct FailedReader;

impl AsyncRead for FailedReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::other("attachment-read-sentinel")))
    }
}

#[tokio::test]
async fn attachment_reader_error_is_read_error() {
    let error = read_attachment_stream(FailedReader, 1, 4)
        .await
        .unwrap_err();
    let ReadError::Read(error) = error else {
        panic!("reader failure must retain the read stage");
    };
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(error.to_string(), "attachment-read-sentinel");
}

#[tokio::test]
async fn attachment_reader_stops_at_limit() {
    let bytes = b"12345";
    let result = read_attachment_stream(&bytes[..], 1, 4).await.unwrap();
    assert_eq!(result, b"1234");
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let result = read_attachment_bytes(file.path(), 1, 4).await.unwrap();
    assert_eq!(result, b"1234");
}
