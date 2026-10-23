use super::*;

pub(super) fn issue_2837_fixture() -> FixtureRoot {
    let f = FixtureRoot::new("issue-2837");
    std::fs::create_dir_all(f.path()).unwrap();
    f
}
pub(super) fn issue_2837_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub(super) fn issue_2837_fault(point: Option<&str>) {
    IO_FAULT.with(|f| *f.borrow_mut() = (point.map(str::to_string), Vec::new()));
}
pub(super) fn issue_2837_calls() -> Vec<String> {
    IO_FAULT.with(|f| f.borrow().1.clone())
}
pub(super) fn issue_2837_append(root: &Path, revision: &str, text: &str) -> StatusReceipt {
    append_status(
        root,
        revision,
        &issue_2837_id(),
        text,
        "project:room-10/agent",
    )
    .unwrap()
}
pub(super) fn issue_2837_log(root: &Path) -> Vec<u8> {
    std::fs::read(root.join(STATUS_NAME)).unwrap()
}
pub(super) fn issue_2837_lines(root: &Path) -> usize {
    issue_2837_log(root).iter().filter(|c| **c == b'\n').count()
}
pub(super) fn issue_2837_clean(root: &Path) -> Result<EditOutcome, TaskOpError> {
    perform_inner(root, TaskOp::Clean, || fixed_now_at(2026, 10, 2, 21, 0, 0))
}

#[test]
fn issue_2837_legacy_missing_bom_crlf_user_description() {
    let f = issue_2837_fixture();
    let root = f.path();
    let s = read_snapshot(root).unwrap();
    assert_eq!(s.revision, "legacy:0");
    assert!(s.task.is_none() && s.status_record.is_none() && !s.tail_incomplete);
    let task = "\u{feff}---\r\ntitle: 'USER: Hold'\r\n---\r\nOne\r\nTwo\r\n";
    std::fs::write(root.join("TASK.md"), task).unwrap();
    let s = read_snapshot(root).unwrap();
    assert_eq!(s.task.as_deref(), Some(task));
    assert_eq!(s.task_title.as_deref(), Some("USER: Hold"));
    assert_eq!(s.description, "One\r\nTwo\r\n");
    assert!(s.status.is_none());
    issue_2837_append(
        root,
        "legacy:0",
        "Tickets: uno\nFUP: ninguno\t🦀\r\nContinue: sí",
    );
    assert_eq!(std::fs::read_to_string(root.join("TASK.md")).unwrap(), task);
    let s = read_snapshot(root).unwrap();
    issue_2837_append(root, &s.revision, "Replacement complete");
    assert_eq!(
        read_snapshot(root).unwrap().status.as_deref(),
        Some("Replacement complete")
    );
    assert_eq!(issue_2837_lines(root), 2);
    let wire = serde_json::to_value(read_snapshot(root).unwrap()).unwrap();
    assert!(wire.get("workgroupRoot").is_some() && wire.get("statusRecord").is_some());
    std::fs::write(root.join("TASK.md"), vec![b'x'; TASK_LIMIT as usize + 1]).unwrap();
    assert!(matches!(
        read_snapshot(root),
        Err(TaskOpError::SnapshotTooLarge)
    ));
}

#[test]
fn issue_2837_retry_mismatch_overtaken_and_cas() {
    let f = issue_2837_fixture();
    let root = f.path();
    let id = issue_2837_id();
    let first = append_status(root, "legacy:0", &id, "one", "p/a").unwrap();
    assert!(!first.replayed);
    let bytes = issue_2837_log(root);
    issue_2837_fault(None);
    assert!(
        append_status(root, "legacy:0", &id, "one", "p/a")
            .unwrap()
            .replayed
    );
    assert_eq!(issue_2837_calls(), vec!["append_sync"]);
    for (base, author, text) in [
        ("legacy:0", "p/b", "one"),
        ("legacy:0", "p/a", "two"),
        (first.revision.as_str(), "p/a", "one"),
    ] {
        assert!(matches!(
            append_status(root, base, &id, text, author),
            Err(TaskOpError::RequestIdConflict)
        ));
    }
    assert_eq!(issue_2837_log(root), bytes);
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let p = root.to_path_buf();
        let b = barrier.clone();
        let revision = first.revision.clone();
        threads.push(thread::spawn(move || {
            b.wait();
            append_status(&p, &revision, &issue_2837_id(), "next", "p/a")
        }));
    }
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(TaskOpError::RevisionConflict { .. })))
            .count(),
        1
    );
    assert!(matches!(
        append_status(root, "legacy:0", &id, "one", "p/a"),
        Err(TaskOpError::RevisionConflict { .. })
    ));
    assert_eq!(issue_2837_lines(root), 2);
    issue_2837_fault(None);
}

#[test]
fn issue_2837_complete_append_failed_sync_retry_requires_fresh_sync() {
    let f = issue_2837_fixture();
    let root = f.path();
    let id = issue_2837_id();
    issue_2837_fault(Some("append_sync"));
    assert!(matches!(
        append_status(root, "legacy:0", &id, "text", "p/a"),
        Err(TaskOpError::WriteFailed(_))
    ));
    assert_eq!(issue_2837_lines(root), 1);
    let bytes = issue_2837_log(root);
    assert_eq!(read_snapshot(root).unwrap().status.as_deref(), Some("text"));
    issue_2837_fault(Some("append_sync"));
    assert!(matches!(
        append_status(root, "legacy:0", &id, "text", "p/a"),
        Err(TaskOpError::WriteFailed(_))
    ));
    assert_eq!(issue_2837_calls(), vec!["append_sync"]);
    assert_eq!(issue_2837_log(root), bytes);
    issue_2837_fault(None);
    assert!(
        append_status(root, "legacy:0", &id, "text", "p/a")
            .unwrap()
            .replayed
    );
    assert_eq!(issue_2837_calls(), vec!["append_sync"]);
    assert_eq!(issue_2837_log(root), bytes);
}

#[test]
fn issue_2837_first_partial_twenty_bytes_and_split_utf8_repair() {
    for split_utf8 in [false, true] {
        let f = issue_2837_fixture();
        let root = f.path();
        let id = issue_2837_id();
        if split_utf8 {
            let mut bytes = vec![b'x'; 18];
            bytes.extend_from_slice(&[0xf0, 0x9f]);
            std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
        } else {
            issue_2837_fault(Some("append_partial"));
            assert!(matches!(
                append_status(root, "legacy:0", &id, "🦀", "p/a"),
                Err(TaskOpError::WriteFailed(_))
            ));
        }
        let original = issue_2837_log(root);
        assert_eq!(original.len(), 20);
        let s = read_snapshot(root).unwrap();
        assert_eq!(s.revision, "legacy:0");
        assert!(s.status_record.is_none() && s.tail_incomplete);
        for point in [
            "partial_backup_create",
            "partial_backup_write",
            "partial_backup_sync",
        ] {
            issue_2837_fault(Some(point));
            assert!(matches!(
                append_status(root, "legacy:0", &id, "🦀", "p/a"),
                Err(TaskOpError::WriteFailed(_))
            ));
            assert_eq!(issue_2837_log(root), original);
        }
        issue_2837_fault(None);
        let receipt = append_status(root, "legacy:0", &id, "🦀", "p/a").unwrap();
        assert_eq!(receipt.record.sequence, 1);
        assert_eq!(issue_2837_lines(root), 1);
        assert!(!read_snapshot(root).unwrap().tail_incomplete);
        assert!(std::fs::read_dir(root).unwrap().flatten().any(|e| e
            .file_name()
            .to_string_lossy()
            .starts_with("TASK-status.partial.")
            && std::fs::read(e.path()).unwrap() == original));
    }
}

#[test]
fn issue_2837_partial_bounds_and_validation_before_repair() {
    let f = issue_2837_fixture();
    let root = f.path();
    let first = issue_2837_append(root, "legacy:0", "valid");
    let complete = issue_2837_log(root);
    let mut bytes = complete.clone();
    bytes.extend(vec![b'x'; 65_535]);
    std::fs::write(root.join(STATUS_NAME), &bytes).unwrap();
    let s = read_snapshot(root).unwrap();
    assert!(s.tail_incomplete);
    assert_eq!(s.revision, first.revision);
    assert!(matches!(
        append_status(root, "legacy:0", &issue_2837_id(), "wrong base", "p/a"),
        Err(TaskOpError::RevisionConflict { .. })
    ));
    assert!(matches!(
        append_status(root, &first.revision, &issue_2837_id(), " \t", "p/a"),
        Err(TaskOpError::InvalidStatus(_))
    ));
    assert_eq!(issue_2837_log(root), bytes);
    issue_2837_append(root, &first.revision, "next");
    assert!(issue_2837_log(root).starts_with(&complete));
    let mut bytes = issue_2837_log(root);
    bytes.extend(vec![b'x'; 65_536]);
    std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
    assert!(matches!(
        read_snapshot(root),
        Err(TaskOpError::InvalidStatus(_))
    ));
    std::fs::write(root.join(STATUS_NAME), vec![0xf0; 65_535]).unwrap();
    assert!(read_snapshot(root).unwrap().tail_incomplete);
    std::fs::write(root.join(STATUS_NAME), vec![0xf0; 65_536]).unwrap();
    assert!(matches!(
        read_snapshot(root),
        Err(TaskOpError::InvalidStatus(_))
    ));
}

#[test]
fn issue_2837_corrupt_complete_schema_shape_and_sequence() {
    let f = issue_2837_fixture();
    let root = f.path();
    let first = issue_2837_append(root, "legacy:0", "original");
    for field in [
        "schemaVersion",
        "kind",
        "topicId",
        "sequence",
        "requestId",
        "baseRevision",
        "recordedAt",
        "author",
        "status",
    ] {
        let mut value = serde_json::to_value(&first.record).unwrap();
        value.as_object_mut().unwrap().remove(field);
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
        assert!(
            matches!(read_snapshot(root), Err(TaskOpError::InvalidStatus(_))),
            "missing {field}"
        );
    }
    for (field, value) in [
        ("schemaVersion", serde_json::json!(2)),
        ("kind", serde_json::json!("clear")),
        ("topicId", serde_json::json!("bad")),
        ("sequence", serde_json::json!(0)),
        ("sequence", serde_json::json!(MAX_SEQUENCE + 1)),
        ("status", serde_json::json!("\u{7}")),
        ("recordedAt", serde_json::json!("2026-01-01T00:00:00+03:00")),
    ] {
        let mut record = serde_json::to_value(&first.record).unwrap();
        record[field] = value;
        let mut bytes = serde_json::to_vec(&record).unwrap();
        bytes.push(b'\n');
        std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
        assert!(
            matches!(read_snapshot(root), Err(TaskOpError::InvalidStatus(_))),
            "invalid {field}"
        );
    }
    for bytes in [
        b"{broken}\n".to_vec(),
        [vec![b'x'; 65_536], vec![b'\n']].concat(),
    ] {
        std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
        assert!(matches!(
            read_snapshot(root),
            Err(TaskOpError::InvalidStatus(_))
        ));
    }
    let mut record = first.record;
    record.sequence = MAX_SEQUENCE;
    record.base_revision = Some(format!("{}:{}", record.topic_id, MAX_SEQUENCE - 1));
    let mut bytes = serde_json::to_vec(&record).unwrap();
    bytes.push(b'\n');
    std::fs::write(root.join(STATUS_NAME), &bytes).unwrap();
    assert!(matches!(
        append_status(
            root,
            &format!("{}:{}", record.topic_id, MAX_SEQUENCE),
            &issue_2837_id(),
            "overflow",
            "p/a"
        ),
        Err(TaskOpError::SequenceOverflow)
    ));
    assert_eq!(issue_2837_log(root), bytes);
}

#[test]
fn issue_2837_encoded_size_boundary_and_bounded_history_read() {
    let f = issue_2837_fixture();
    let root = f.path();
    let id = issue_2837_id();
    let receipt = append_status(root, "legacy:0", &id, "x", "p/a").unwrap();
    let overhead = issue_2837_log(root).len() - 1;
    // Timestamp formatting is stable for Utc::now(); allow for nanosecond
    // formatter width by explicitly checking encoded rows, not characters.
    let mut r = receipt.record.clone();
    r.status = Some("x".repeat(STATUS_LIMIT - overhead));
    let mut row = serde_json::to_vec(&r).unwrap();
    row.push(b'\n');
    assert_eq!(row.len(), STATUS_LIMIT);
    assert!(validate_record(&row[..row.len() - 1]).is_ok());
    row.insert(row.len() - 2, b'x');
    assert!(matches!(
        validate_record(&row[..row.len() - 1]),
        Err(TaskOpError::InvalidStatus(_))
    ));
    let original = issue_2837_log(root);
    assert!(matches!(
        append_status(
            root,
            &receipt.revision,
            &issue_2837_id(),
            &"\n".repeat(STATUS_LIMIT),
            "p/a"
        ),
        Err(TaskOpError::InvalidStatus(_))
    ));
    assert!(matches!(
        append_status(
            root,
            &receipt.revision,
            &issue_2837_id(),
            &format!("x{}", "\n".repeat(STATUS_LIMIT)),
            "p/a"
        ),
        Err(TaskOpError::StatusTooLarge)
    ));
    assert_eq!(issue_2837_log(root), original);
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(root.join(STATUS_NAME))
        .unwrap();
    for _ in 0..50_000 {
        file.write_all(&original).unwrap();
    }
    drop(file);
    let tail = read_latest_status_locked(root).unwrap();
    assert_eq!(tail.bytes_read as u64, STATUS_WINDOW);
    assert_eq!(tail.record.unwrap(), receipt.record);
}

#[test]
fn issue_2837_clean_pair_matrix_exact_bytes_noop_and_aba() {
    for (task_exists, log_exists) in [(true, true), (true, false), (false, true), (false, false)] {
        let f = issue_2837_fixture();
        let root = f.path();
        let task = b"\xef\xbb\xbf---\r\ntitle: 'USER: Original'\r\n---\r\nbody\r\n";
        let log = b"{corrupt complete}\n\xf0\x9f";
        if task_exists {
            std::fs::write(root.join("TASK.md"), task).unwrap();
        }
        if log_exists {
            std::fs::write(root.join(STATUS_NAME), log).unwrap();
        }
        let result = issue_2837_clean(root).unwrap();
        assert!(matches!(result, EditOutcome::Wrote { .. }));
        let (t, s) = backup_names("20261002-210000", 0);
        if task_exists || log_exists {
            assert_eq!(
                std::fs::read(root.join(t)).unwrap(),
                if task_exists { task.as_slice() } else { &[] }
            );
            assert_eq!(
                std::fs::read(root.join(s)).unwrap(),
                if log_exists { log.as_slice() } else { &[] }
            );
        } else {
            assert!(!root.join(t).exists() && !root.join(s).exists());
        }
        let snapshot = read_snapshot(root).unwrap();
        assert_eq!(snapshot.task_title.as_deref(), Some("Clean"));
        assert_eq!(
            snapshot.status_record.as_ref().unwrap().kind,
            "topic_started"
        );
        assert!(snapshot.revision.ends_with(":0"));
        assert_ne!(snapshot.revision, "legacy:0");
        let bytes = issue_2837_log(root);
        assert!(matches!(
            issue_2837_clean(root),
            Ok(EditOutcome::NoOp { .. })
        ));
        assert_eq!(issue_2837_log(root), bytes);
        assert!(matches!(
            append_status(root, "legacy:0", &issue_2837_id(), "stale", "p/a"),
            Err(TaskOpError::RevisionConflict { .. })
        ));
        issue_2837_append(root, &snapshot.revision, "history after Clean");
        assert!(matches!(
            issue_2837_clean(root),
            Ok(EditOutcome::Wrote { .. })
        ));
        assert_ne!(read_snapshot(root).unwrap().revision, snapshot.revision);
    }
}

#[test]
fn issue_2837_clean_backup_collision_on_either_side() {
    for collision_on_task in [true, false] {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "old").unwrap();
        let (t, s) = backup_names("20261002-210000", 0);
        let foreign = if collision_on_task {
            t.clone()
        } else {
            s.clone()
        };
        std::fs::write(root.join(&foreign), "foreign").unwrap();
        issue_2837_clean(root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(foreign)).unwrap(),
            "foreign"
        );
        let (t1, s1) = backup_names("20261002-210000", 1);
        assert!(root.join(t1).exists() && root.join(s1).exists());
        if !collision_on_task {
            assert!(!root.join(t).exists());
        }
    }
}

#[test]
fn issue_2837_clean_prejournal_failure_matrix_preserves_originals() {
    for point in [
        "task_backup_create",
        "status_backup_create",
        "task_backup_copy",
        "task_backup_sync",
        "status_backup_copy",
        "status_backup_sync",
        "task_stage_create",
        "task_stage_write",
        "task_stage_sync",
        "status_stage_create",
        "status_stage_write",
        "status_stage_sync",
        "journal_create",
        "journal_write",
        "journal_stage_sync",
        "journal_publish",
    ] {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "old task").unwrap();
        std::fs::write(root.join(STATUS_NAME), b"old\npartial\xf0").unwrap();
        let old_task = std::fs::read(root.join("TASK.md")).unwrap();
        let old_log = issue_2837_log(root);
        issue_2837_fault(Some(point));
        assert!(issue_2837_clean(root).is_err(), "{point}");
        assert_eq!(
            std::fs::read(root.join("TASK.md")).unwrap(),
            old_task,
            "{point}"
        );
        assert_eq!(issue_2837_log(root), old_log, "{point}");
        assert!(!root.join(JOURNAL_NAME).exists(), "{point}");
        issue_2837_fault(None);
        issue_2837_clean(root).unwrap();
        assert!(read_snapshot(root).unwrap().status.is_none());
    }
}

#[test]
fn issue_2837_clean_postjournal_failure_matrix_and_repeat_recovery() {
    for point in [
        "after_journal_publish",
        "journal_sync",
        "task_rename",
        "after_task_rename",
        "status_rename",
        "after_status_rename",
        "task_target_sync_open",
        "task_target_sync",
        "status_target_sync_open",
        "status_target_sync",
        "journal_remove",
    ] {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "old task").unwrap();
        std::fs::write(root.join(STATUS_NAME), "old log").unwrap();
        issue_2837_fault(Some(point));
        assert!(
            matches!(
                issue_2837_clean(root),
                Err(TaskOpError::CleanRecoveryPending(_))
            ),
            "{point}"
        );
        assert!(root.join(JOURNAL_NAME).exists(), "{point}");
        issue_2837_fault(None);
        recover_clean_pair(root).unwrap();
        let new_task = std::fs::read(root.join("TASK.md")).unwrap();
        let new_log = issue_2837_log(root);
        assert!(!root.join(JOURNAL_NAME).exists());
        let calls = issue_2837_calls();
        let task_sync = calls.iter().position(|c| c == "task_target_sync").unwrap();
        let status_sync = calls
            .iter()
            .position(|c| c == "status_target_sync")
            .unwrap();
        let remove = calls.iter().position(|c| c == "journal_remove").unwrap();
        assert!(task_sync < status_sync && status_sync < remove);
        recover_clean_pair(root).unwrap();
        assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), new_task);
        assert_eq!(issue_2837_log(root), new_log);
    }
}
