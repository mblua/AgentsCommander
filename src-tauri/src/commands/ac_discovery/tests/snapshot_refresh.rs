use super::*;

use crate::config::settings::AppSettings;

fn fixture() -> (
    tempfile::TempDir,
    PathBuf,
    SettingsState,
    Arc<tokio::sync::RwLock<SessionManager>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".ac/room-1");
    std::fs::create_dir_all(&root).unwrap();
    let settings = crate::config::settings::new_settings_state(AppSettings {
        project_paths: vec![dir.path().to_string_lossy().into_owned()],
        ..Default::default()
    });
    (
        dir,
        root,
        settings,
        Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
    )
}

fn status_line(sequence: u64, status: &str) -> String {
    serde_json::json!({
        "schemaVersion":1,"kind":"status","topicId":"12345678-1234-4234-8234-123456789abc",
        "sequence":sequence,"requestId":"87654321-4321-4321-8321-123456789abc",
        "baseRevision":if sequence == 1 {
            "legacy:0".to_owned()
        } else {
            format!("12345678-1234-4234-8234-123456789abc:{}", sequence-1)
        },
        "recordedAt":"2026-10-05T18:00:00Z","author":"project/fixture","status":status
    })
    .to_string()
        + "\n"
}

#[test]
fn path_admission_matrix_preserves_original_key_and_fails_closed() {
    let (dir, root, _, _) = fixture();
    let projects = vec![dir.path().to_string_lossy().into_owned()];
    let valid = root.to_string_lossy().into_owned();
    assert_eq!(validate_snapshot_room(&valid, &projects).unwrap(), root);
    let legacy = dir.path().join(".AC/wg-2");
    std::fs::create_dir_all(&legacy).unwrap();
    assert_eq!(
        validate_snapshot_room(&legacy.to_string_lossy(), &projects).unwrap(),
        legacy
    );
    let invalid = [
        dir.path().join("missing"),
        dir.path().join("room-outside"),
        dir.path().join(".ac/no-room"),
    ];
    for candidate in &invalid[1..] {
        std::fs::create_dir_all(candidate).unwrap();
    }
    for candidate in invalid {
        assert!(validate_snapshot_room(&candidate.to_string_lossy(), &projects).is_err());
    }
    let file = root.join("room-file");
    std::fs::write(&file, "x").unwrap();
    assert!(validate_snapshot_room(&file.to_string_lossy(), &projects).is_err());
    assert!(validate_snapshot_room(&valid, &[]).is_err());
    assert!(validate_snapshot_room(
        &valid,
        &[dir.path().join("missing").to_string_lossy().into_owned()]
    )
    .is_err());
    let outside = tempfile::tempdir().unwrap();
    let escaped = outside.path().join(".ac/room-escape");
    std::fs::create_dir_all(&escaped).unwrap();
    assert!(validate_snapshot_room(&escaped.to_string_lossy(), &projects).is_err());
    let traversal = root
        .join("../../..")
        .join(outside.path().file_name().unwrap())
        .join(".ac/room-escape");
    assert!(validate_snapshot_room(&traversal.to_string_lossy(), &projects).is_err());
    let raw = root.join("../room-1").to_string_lossy().into_owned();
    assert_eq!(
        validate_snapshot_room(&raw, &projects).unwrap(),
        PathBuf::from(raw)
    );
    #[cfg(windows)]
    {
        let variant = valid.to_uppercase();
        assert_eq!(
            validate_snapshot_room(&variant, &projects).unwrap(),
            PathBuf::from(variant)
        );
        let canonical = std::fs::canonicalize(&root).unwrap();
        assert!(validate_snapshot_room(&canonical.to_string_lossy(), &projects).is_ok());
    }
}

#[test]
fn status_only_partial_deletion_errors_and_emit_retry_keep_shipped_state() {
    let (_dir, root, _, _) = fixture();
    std::fs::write(
        root.join("TASK.md"),
        "\u{feff}---\ntitle: 'USER: café'\n---\n\n# 日本語 🦀\n",
    )
    .unwrap();
    let initial = poll_task_snapshot(&root, None).unwrap().unwrap();
    assert_eq!(initial.task.as_deref(), Some("日本語 🦀"));
    assert!(poll_task_snapshot(&root, Some(initial.sentinel.clone()))
        .unwrap()
        .is_none());
    let line = status_line(1, "ready");
    std::fs::write(root.join("TASK-status.jsonl"), format!("{line}partial")).unwrap();
    let changed = poll_task_snapshot(&root, Some(initial.sentinel.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(changed.status.as_deref(), Some("ready"));
    assert!(changed.tail_incomplete);
    assert!(deliver_poll_task(Some(&initial), changed, |_| Err("offline".into())).is_err());
    let retry = poll_task_snapshot(&root, Some(initial.sentinel.clone()))
        .unwrap()
        .unwrap();
    let shipped = deliver_poll_task(Some(&initial), retry, |_| Ok(())).unwrap();
    assert!(poll_task_snapshot(&root, Some(shipped.sentinel.clone()))
        .unwrap()
        .is_none());
    std::fs::write(root.join("TASK-status.jsonl"), &line).unwrap();
    let completed = poll_task_snapshot(&root, Some(shipped.sentinel.clone()))
        .unwrap()
        .unwrap();
    assert!(!completed.tail_incomplete);
    assert_eq!(completed.status.as_deref(), Some("ready"));
    std::fs::remove_file(root.join("TASK-status.jsonl")).unwrap();
    let deleted = poll_task_snapshot(&root, Some(completed.sentinel.clone()))
        .unwrap()
        .unwrap();
    assert!(deleted.status.is_none());
    assert_eq!(deleted.revision, "legacy:0");
    std::fs::write(root.join("TASK-clean.pending.json"), "{}").unwrap();
    assert!(poll_task_snapshot(&root, Some(deleted.sentinel.clone())).is_err());
    std::fs::remove_file(root.join("TASK-clean.pending.json")).unwrap();
    for content in [vec![0xff], vec![b'x'; 256 * 1024 + 1]] {
        std::fs::write(root.join("TASK.md"), content).unwrap();
        assert!(poll_task_snapshot(&root, Some(deleted.sentinel.clone())).is_err());
    }
    std::fs::write(root.join("TASK.md"), "body").unwrap();
    std::fs::write(root.join("TASK-status.jsonl"), "[]\n").unwrap();
    assert!(poll_task_snapshot(&root, Some(deleted.sentinel.clone())).is_err());
    assert_eq!(deleted.revision, "legacy:0");
}

#[tokio::test]
async fn snapshot_admission_uses_current_sessions_and_releases_guards_during_io() {
    let (_dir, root, settings, manager) = fixture();
    std::fs::write(root.join("TASK.md"), "body 🦀").unwrap();
    let manager_handle = manager.read().await.clone();
    let session = manager_handle
        .create_session(
            "fixture".into(),
            vec![],
            root.join("__agent_fixture").to_string_lossy().into_owned(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let id = session.id.to_string();
    let path = root.to_string_lossy().into_owned();
    let by_session = task_snapshot_inner(&manager, &settings, Some(&id), None)
        .await
        .unwrap();
    let by_path = task_snapshot_inner(&manager, &settings, None, Some(&path))
        .await
        .unwrap();
    assert_eq!(by_session, by_path);
    for invalid in ["invalid".to_owned(), Uuid::new_v4().to_string()] {
        assert!(
            task_snapshot_inner(&manager, &settings, Some(&invalid), None)
                .await
                .is_err()
        );
    }
    assert!(task_snapshot_inner(&manager, &settings, None, None)
        .await
        .is_err());
    assert!(
        task_snapshot_inner(&manager, &settings, Some(&id), Some(&path))
            .await
            .is_err()
    );
    let outside = manager_handle
        .create_session(
            "fixture".into(),
            vec![],
            "C:/outside-room".into(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    assert!(
        task_snapshot_inner(&manager, &settings, Some(&outside.id.to_string()), None)
            .await
            .is_err()
    );
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("TASK.md.lock"))
        .unwrap();
    lock.lock().unwrap();
    let query_manager = manager.clone();
    let query_settings = settings.clone();
    let query = tokio::spawn(async move {
        task_snapshot_inner(&query_manager, &query_settings, Some(&id), None).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let manager_guard = tokio::time::timeout(Duration::from_secs(1), manager.write())
        .await
        .unwrap();
    let settings_guard = tokio::time::timeout(Duration::from_secs(1), settings.write())
        .await
        .unwrap();
    drop(manager_guard);
    drop(settings_guard);
    lock.unlock().unwrap();
    assert_eq!(query.await.unwrap().unwrap(), by_path);
}

#[tokio::test]
async fn watcher_loaded_room_without_session_and_shared_session_root() {
    use tauri::Listener;
    let (_dir, root, settings, manager) = fixture();
    std::fs::write(root.join("TASK.md"), "body").unwrap();
    let app = crate::test_support::test_builder()
        .manage(settings.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let watcher = DiscoveryBranchWatcher::new(app.handle().clone(), manager.clone());
    let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let capture = events.clone();
    app.listen("workgroup_task_updated", move |event| {
        capture
            .lock()
            .unwrap()
            .push(serde_json::from_str(event.payload()).unwrap())
    });
    let entries = [ReplicaBranchEntry {
        replica_path: root.join("__agent_fixture").to_string_lossy().into_owned(),
        repos: vec![],
        session_name: "fixture".into(),
    }];
    watcher.poll_tasks(&entries).await.unwrap();
    assert_eq!(watcher.task_cache.lock().unwrap().len(), 1);
    assert_eq!(
        events.lock().unwrap()[0]["sessionIds"],
        serde_json::json!([])
    );
    let manager_handle = manager.read().await.clone();
    for _ in 0..2 {
        manager_handle
            .create_session(
                "fixture".into(),
                vec![],
                entries[0].replica_path.clone(),
                None,
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
    }
    std::fs::write(root.join("TASK-status.jsonl"), status_line(1, "updated")).unwrap();
    watcher.poll_tasks(&entries).await.unwrap();
    assert_eq!(watcher.task_cache.lock().unwrap().len(), 1);
    {
        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[1]["sessionIds"].as_array().unwrap().len(), 2);
        assert_eq!(captured[1]["status"], "updated");
    }
    // Existing lock timeout leaves the previous shipped cache intact.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("TASK.md.lock"))
        .unwrap();
    lock.lock().unwrap();
    std::fs::write(
        root.join("TASK-status.jsonl"),
        status_line(2, "after timeout"),
    )
    .unwrap();
    let waiting = watcher.clone();
    let waiting_root = root.clone();
    let query =
        tokio::spawn(async move { waiting.check_workgroup_task(waiting_root, vec![]).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(watcher.task_cache.try_lock().is_ok());
    query.await.unwrap();
    assert_eq!(
        watcher
            .task_cache
            .lock()
            .unwrap()
            .get(&root)
            .unwrap()
            .status
            .as_deref(),
        Some("updated")
    );
    lock.unlock().unwrap();
    watcher.poll_tasks(&entries).await.unwrap();
    assert_eq!(
        watcher
            .task_cache
            .lock()
            .unwrap()
            .get(&root)
            .unwrap()
            .status
            .as_deref(),
        Some("after timeout")
    );
}

#[test]
fn task_sentinels_preserve_presence_metadata_and_errors() {
    use super::super::task_snapshot::task_sentinels;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("missing");
    let absent = task_sentinels(&root).unwrap();
    assert!(absent.task.is_none());
    assert!(absent.status.is_none());
    assert!(!absent.journal);

    std::fs::create_dir(&root).unwrap();
    let task_path = root.join("TASK.md");
    std::fs::write(&task_path, "body 🦀").unwrap();
    let metadata = std::fs::metadata(&task_path).unwrap();
    let present = task_sentinels(&root).unwrap();
    let task = present.task.unwrap();
    assert_eq!(task.len, metadata.len());
    assert_eq!(task.mtime, metadata.modified().ok());
    assert!(present.status.is_none());
    assert!(!present.journal);

    let status_path = root.join("TASK-status.jsonl");
    std::fs::write(&status_path, "status").unwrap();
    assert!(task_sentinels(&root).unwrap().status.is_some());
    std::fs::remove_file(&status_path).unwrap();
    assert!(task_sentinels(&root).unwrap().status.is_none());
    std::fs::write(root.join("TASK-clean.pending.json"), "{}").unwrap();
    assert!(task_sentinels(&root).unwrap().journal);

    let root = Path::new("a\0b");
    let error = std::fs::metadata(root.join("TASK.md")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        task_sentinels(root).err(),
        Some(format!("snapshot sentinel read failed: {}", error))
    );
}
