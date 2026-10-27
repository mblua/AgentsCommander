use super::*;

#[tokio::test]
async fn rejects_empty_attachment() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let path = file.path().to_str().unwrap();
    assert_eq!(
        prepare_telegram_attachment(path, 50 * 1024 * 1024).await,
        Err(format!("File is empty: {path}"))
    );
}

#[tokio::test]
async fn rejects_over_limit_attachment() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let max = 50 * 1024 * 1024;
    file.as_file().set_len(max + 1).unwrap();
    let path = file.path().to_str().unwrap();
    assert_eq!(
        prepare_telegram_attachment(path, max).await,
        Err(format!(
            "File exceeds Telegram 50 MB limit ({} bytes): {path}",
            max + 1
        ))
    );
}

#[tokio::test]
async fn rejects_non_regular_attachment() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_str().unwrap();
    assert_eq!(
        prepare_telegram_attachment(path, 50 * 1024 * 1024).await,
        Err(format!("Not a regular file: {path}"))
    );
}

#[tokio::test]
async fn preserves_stat_error() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let path = missing.to_str().unwrap();
    let error = tokio::fs::symlink_metadata(&missing).await.unwrap_err();
    assert_eq!(
        prepare_telegram_attachment(path, 50 * 1024 * 1024).await,
        Err(format!("stat failed: {error}"))
    );
}

#[tokio::test]
async fn rejects_growth_after_metadata() {
    let file = tempfile::NamedTempFile::new().unwrap();
    file.as_file().set_len(1).unwrap();
    let path = file.path().to_str().unwrap();
    let result = prepare_telegram_attachment_with(
        path,
        3,
        tokio::fs::symlink_metadata(file.path()),
        |capacity, limit| async move {
            assert_eq!(capacity, 1);
            assert_eq!(limit, 4);
            Ok(vec![1, 2, 3, 4])
        },
    )
    .await;
    assert_eq!(
        result,
        Err(format!("File grew past 50 MB during read: {path}"))
    );
}

#[tokio::test]
async fn accepts_exact_limit_and_preserves_stat_size() {
    let file = tempfile::NamedTempFile::new().unwrap();
    file.as_file().set_len(1).unwrap();
    let result = prepare_telegram_attachment_with(
        file.path().to_str().unwrap(),
        3,
        tokio::fs::symlink_metadata(file.path()),
        |capacity, limit| async move {
            assert_eq!(capacity, 1);
            assert_eq!(limit, 4);
            Ok(vec![1, 2, 3])
        },
    )
    .await;
    assert_eq!(result, Ok((vec![1, 2, 3], 1)));
}

#[tokio::test]
async fn preserves_open_and_read_error_text() {
    let file = tempfile::NamedTempFile::new().unwrap();
    file.as_file().set_len(1).unwrap();
    let path = file.path().to_str().unwrap();
    for (open, sentinel) in [
        (true, "attachment-open-sentinel"),
        (false, "attachment-read-sentinel"),
    ] {
        let error = io::Error::other(sentinel);
        let stage = if open { "open" } else { "read" };
        let expected = format!("{stage} failed: {error}");
        let result = prepare_telegram_attachment_with(
            path,
            3,
            tokio::fs::symlink_metadata(file.path()),
            |_, _| async move {
                Err(if open {
                    ReadError::Open(error)
                } else {
                    ReadError::Read(error)
                })
            },
        )
        .await;
        assert_eq!(result, Err(expected));
    }
}

#[tokio::test]
async fn rejects_before_read() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let path = file.path().to_str().unwrap();
    for size in [0, 4] {
        file.as_file().set_len(size).unwrap();
        let result = prepare_telegram_attachment_with(
            path,
            3,
            tokio::fs::symlink_metadata(file.path()),
            |_, _| async { panic!("rejected attachment must not be read") },
        )
        .await;
        let expected = if size == 0 {
            format!("File is empty: {path}")
        } else {
            format!("File exceeds Telegram 50 MB limit ({size} bytes): {path}")
        };
        assert_eq!(result, Err(expected));
    }
}
