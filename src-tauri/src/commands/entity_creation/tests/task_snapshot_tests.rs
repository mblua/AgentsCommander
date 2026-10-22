use super::*;

#[test]
fn snapshot_facade_is_cli_dto_with_only_root_normalization() {
    let dir = tempfile::tempdir().unwrap();
    for content in [
        None,
        Some(""),
        Some("\u{feff}---\ntitle: 'USER: café'\n---\n\n日本語 🦀\n"),
    ] {
        let file = dir.path().join("TASK.md");
        if let Some(content) = content {
            std::fs::write(&file, content).unwrap();
        } else if file.exists() {
            std::fs::remove_file(&file).unwrap();
        }
        let mut cli = serde_json::to_value(task_ops::read_snapshot(dir.path()).unwrap()).unwrap();
        cli["workgroupRoot"] = serde_json::Value::String(
            crate::path_utils::path_to_string_without_windows_verbatim_prefix(dir.path()),
        );
        let ipc = read_room_task_snapshot_json(dir.path()).unwrap();
        assert_eq!(ipc, cli);
        assert_eq!(ipc["task"], serde_json::to_value(content).unwrap());
        assert!(ipc["status"].is_null());
        assert_eq!(ipc["revision"], "legacy:0");
    }
    let receipt = task_ops::append_status(
        dir.path(),
        "legacy:0",
        "12345678-1234-4234-8234-123456789abc",
        "listo 🦀",
        "project/fixture",
    )
    .unwrap();
    let ipc = read_room_task_snapshot_json(dir.path()).unwrap();
    assert_eq!(ipc["status"], "listo 🦀");
    assert_eq!(ipc["revision"], receipt.revision);
    task_ops::perform(dir.path(), task_ops::TaskOp::Clean).unwrap();
    let clean = read_room_task_snapshot_json(dir.path()).unwrap();
    assert!(clean["status"].is_null());
    assert!(clean["revision"].as_str().unwrap().ends_with(":0"));
    assert_ne!(clean["revision"], "legacy:0");
    assert_eq!(clean["taskTitle"], "Clean");
    assert_eq!(clean["description"], "");
}

#[test]
fn snapshot_facade_propagates_invalid_task_status_and_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let task = dir.path().join("TASK.md");
    for bytes in [vec![0xff], vec![b'x'; 256 * 1024 + 1]] {
        std::fs::write(&task, bytes).unwrap();
        assert!(read_room_task_snapshot_json(dir.path()).is_err());
    }
    std::fs::write(&task, "body").unwrap();
    std::fs::write(dir.path().join("TASK-status.jsonl"), "[]\n").unwrap();
    assert!(read_room_task_snapshot_json(dir.path()).is_err());
    std::fs::remove_file(dir.path().join("TASK-status.jsonl")).unwrap();
    std::fs::write(dir.path().join("TASK-clean.pending.json"), "{}").unwrap();
    assert!(read_room_task_snapshot_json(dir.path()).is_err());
    assert_eq!(std::fs::read_to_string(&task).unwrap(), "body");
}
