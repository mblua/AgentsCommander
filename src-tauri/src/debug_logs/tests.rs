use super::*;

#[tokio::test]
async fn missing_config_directory_preserves_error() {
    assert_eq!(
        save_debug_logs(None, "logs").await.unwrap_err(),
        "No config dir"
    );
}

#[tokio::test]
async fn dump_preserves_utf8_bytes_and_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let content = "before\0\r\nEspañol 日本語 🦀";
    let path = save_debug_logs(Some(dir.path().to_path_buf()), content)
        .await
        .unwrap();
    assert_eq!(path, dir.path().join(DEBUG_LOGS_FILE_NAME));
    assert_eq!(std::fs::read(&path).unwrap(), content.as_bytes());
    let shorter = "短\0\r\n";
    assert_eq!(
        save_debug_logs(Some(dir.path().to_path_buf()), shorter)
            .await
            .unwrap(),
        path
    );
    assert_eq!(std::fs::read(&path).unwrap(), shorter.as_bytes());
}

#[tokio::test]
async fn missing_directory_preserves_write_error_without_creating_it() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    let path = missing.join(DEBUG_LOGS_FILE_NAME);
    let expected = tokio::fs::write(&path, "logs").await.unwrap_err();
    let actual = save_debug_logs(Some(missing.clone()), "logs")
        .await
        .unwrap_err();
    assert_eq!(actual, format!("Failed to write logs: {}", expected));
    assert!(!missing.exists());
}
