use super::*;

#[tokio::test]
async fn mutation_snapshot_preserves_legacy_projection_and_committed_failures() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("TASK.md");
    std::fs::write(&file, "\nbody 🦀\n").unwrap();
    task_ops::perform(dir.path(), TaskOp::SetUserTitle("café".into())).unwrap();
    task_ops::append_status(
        dir.path(),
        "legacy:0",
        "12345678-1234-4234-8234-123456789abc",
        "ready",
        "project/fixture",
    )
    .unwrap();
    let bytes = std::fs::read(&file).unwrap();
    let result = read_committed_task_snapshot(dir.path().to_path_buf())
        .await
        .unwrap();
    assert_eq!(
        result.task.as_deref(),
        Some(std::str::from_utf8(&bytes).unwrap().trim())
    );
    assert_eq!(result.task_title.as_deref(), Some("USER: café"));
    assert_eq!(result.status.as_deref(), Some("ready"));
    let event = serde_json::to_value(&result).unwrap();
    assert_eq!(event["taskTitle"], "USER: café");
    let error = deliver_committed_task(result, |_| Err("injected emit".into())).unwrap_err();
    assert!(error.contains("already committed"));
    assert!(error.contains("do not repeat Clean"));
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    std::fs::write(dir.path().join("TASK-status.jsonl"), "[]\n").unwrap();
    let error = read_committed_task_snapshot(dir.path().to_path_buf())
        .await
        .unwrap_err();
    assert!(error.contains("already committed"));
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
}

#[tokio::test]
async fn clean_null_topic_and_delivery_failure_do_not_replay_mutation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("TASK.md"), "old body").unwrap();
    task_ops::perform(dir.path(), TaskOp::Clean).unwrap();
    let result = read_committed_task_snapshot(dir.path().to_path_buf())
        .await
        .unwrap();
    assert!(result.status.is_none());
    assert!(result.revision.ends_with(":0"));
    let revision = result.revision.clone();
    let entries = std::fs::read_dir(dir.path()).unwrap().count();
    assert!(deliver_committed_task(result, |_| Err("offline".into())).is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), entries);
    let retry_read = read_committed_task_snapshot(dir.path().to_path_buf())
        .await
        .unwrap();
    assert_eq!(retry_read.revision, revision);
    assert!(serde_json::to_value(retry_read).unwrap()["status"].is_null());
}

#[test]
fn root_keys_preserve_ordinary_paths_and_normalize_windows_unc() {
    let samples = [
        (
            r"C:\PROGRA~1\project\.ac\room-1",
            r"C:\PROGRA~1\project\.ac\room-1",
        ),
        (r"\\server\share\.ac\room-1", r"\\server\share\.ac\room-1"),
        (
            r"\\?\C:\PROGRA~1\project\.ac\room-1",
            r"C:\PROGRA~1\project\.ac\room-1",
        ),
        (
            r"\\?\UNC\server\share\.ac\room-1",
            r"\\server\share\.ac\room-1",
        ),
        (
            r"\??\UNC\server\share\.ac\room-1",
            r"\\server\share\.ac\room-1",
        ),
    ];
    for (raw, windows_key) in samples {
        let expected = if cfg!(windows) { windows_key } else { raw };
        assert_eq!(strip_unc(Path::new(raw)), expected);
    }
}
