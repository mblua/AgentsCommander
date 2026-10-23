use super::issue_2837::issue_2837_append;
use super::issue_2837::issue_2837_calls;
use super::issue_2837::issue_2837_clean;
use super::issue_2837::issue_2837_fault;
use super::issue_2837::issue_2837_fixture;
use super::issue_2837::issue_2837_id;
use super::issue_2837::issue_2837_lines;
use super::issue_2837::issue_2837_log;
use super::*;

#[test]
fn issue_2837_already_new_pair_retries_both_syncs_before_cleanup() {
    for fail_sync in ["task_target_sync", "status_target_sync"] {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "old").unwrap();
        issue_2837_fault(Some("after_status_rename"));
        assert!(matches!(
            issue_2837_clean(root),
            Err(TaskOpError::CleanRecoveryPending(_))
        ));
        let task = std::fs::read(root.join("TASK.md")).unwrap();
        let log = issue_2837_log(root);
        let journal = std::fs::read(root.join(JOURNAL_NAME)).unwrap();
        let backup = std::fs::read(root.join("TASK.20261002-210000.bak.md")).unwrap();
        for _ in 0..2 {
            issue_2837_fault(Some(fail_sync));
            assert!(matches!(
                read_snapshot(root),
                Err(TaskOpError::CleanRecoveryPending(_))
            ));
            assert_eq!(std::fs::read(root.join(JOURNAL_NAME)).unwrap(), journal);
            assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), task);
            assert_eq!(issue_2837_log(root), log);
            let calls = issue_2837_calls();
            assert!(!calls
                .iter()
                .any(|c| c.contains("rename") || c == "journal_remove"));
            assert_eq!(
                calls
                    .iter()
                    .filter(|c| c.as_str() == "task_target_sync")
                    .count(),
                1
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|c| c.as_str() == "status_target_sync")
                    .count(),
                1
            );
        }
        issue_2837_fault(None);
        let s = read_snapshot(root).unwrap();
        assert_eq!(s.status_record.unwrap().sequence, 0);
        assert_eq!(
            issue_2837_calls(),
            vec![
                "journal_sync",
                "task_target_sync_open",
                "task_target_sync",
                "status_target_sync_open",
                "status_target_sync",
                "journal_remove"
            ]
        );
        assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), task);
        assert_eq!(issue_2837_log(root), log);
        assert_eq!(
            std::fs::read(root.join("TASK.20261002-210000.bak.md")).unwrap(),
            backup
        );
        assert!(!root.join(JOURNAL_NAME).exists());
    }
}

#[test]
fn issue_2837_recovery_external_edits_missing_evidence_invalid_journal() {
    for mode in ["task", "status", "backup", "stage", "journal"] {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "old").unwrap();
        issue_2837_fault(Some("task_rename"));
        assert!(issue_2837_clean(root).is_err());
        issue_2837_fault(None);
        let journal: CleanJournal =
            serde_json::from_slice(&std::fs::read(root.join(JOURNAL_NAME)).unwrap()).unwrap();
        match mode {
            "task" => std::fs::write(root.join("TASK.md"), "external").unwrap(),
            "status" => std::fs::write(root.join(STATUS_NAME), "external").unwrap(),
            "backup" => std::fs::remove_file(root.join(journal.task.backup.unwrap())).unwrap(),
            "stage" => std::fs::remove_file(root.join(journal.task.stage)).unwrap(),
            "journal" => {
                let mut value = serde_json::to_value(journal).unwrap();
                value["task"]["stage"] = serde_json::json!("../foreign");
                std::fs::write(root.join(JOURNAL_NAME), serde_json::to_vec(&value).unwrap())
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let task = std::fs::read(root.join("TASK.md")).unwrap();
        let log = std::fs::read(root.join(STATUS_NAME)).ok();
        assert!(
            matches!(
                read_snapshot(root),
                Err(TaskOpError::CleanRecoveryConflict(_))
            ),
            "{mode}"
        );
        assert!(root.join(JOURNAL_NAME).exists());
        assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), task);
        assert_eq!(std::fs::read(root.join(STATUS_NAME)).ok(), log);
    }
}

#[test]
fn issue_2837_append_title_body_clean_concurrency() {
    let f = issue_2837_fixture();
    let root = f.path();
    let first = issue_2837_append(root, "legacy:0", "status");
    let original = issue_2837_log(root);
    perform(root, TaskOp::SetTitle("title".into())).unwrap();
    perform(root, TaskOp::AppendBody("description".into())).unwrap();
    assert_eq!(issue_2837_log(root), original);
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for op in 0..3 {
        let p = root.to_path_buf();
        let b = barrier.clone();
        let revision = first.revision.clone();
        handles.push(thread::spawn(move || {
            b.wait();
            match op {
                0 => {
                    append_status(&p, &revision, &issue_2837_id(), "new status", "p/a").map(|_| ())
                }
                1 => perform(&p, TaskOp::AppendBody("new description".into())).map(|_| ()),
                _ => perform(&p, TaskOp::Clean).map(|_| ()),
            }
        }));
    }
    for (i, h) in handles.into_iter().enumerate() {
        let r = h.join().unwrap();
        assert!(r.is_ok() || i == 0 && matches!(r, Err(TaskOpError::RevisionConflict { .. })));
    }
    let s = read_snapshot(root).unwrap();
    assert_eq!(s.task_title.as_deref(), Some("Clean"));
    assert_eq!(s.status_record.unwrap().kind, "topic_started");
}

#[test]
fn issue_2837_child_fixture() {
    let Some(root) = std::env::var_os("AC_TASK_OPS_CHILD_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    if std::env::var_os("AC_TASK_OPS_CHILD_LOCK").is_some() {
        let _held = LockGuard::acquire(
            &root.join("TASK.md.lock"),
            LOCK_TIMEOUT_5S,
            LOCK_STALE_AFTER_5M,
        )
        .unwrap();
        std::fs::write(root.join("child-ready"), "ready").unwrap();
        std::thread::sleep(Duration::from_secs(30));
    } else {
        let _ = issue_2837_clean(&root);
    }
}

fn issue_2837_child(root: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args([
        "--exact",
        "cli::task_ops::tests::issue_2837_recovery::issue_2837_child_fixture",
        "--nocapture",
    ])
    .env("AC_TASK_OPS_CHILD_FIXTURE", root);
    cmd
}

#[test]
fn issue_2837_kernel_lock_child_exit_and_stable_file() {
    let f = issue_2837_fixture();
    let root = f.path();
    std::fs::write(root.join("TASK.md.lock"), "old stale contents").unwrap();
    let mut child = issue_2837_child(root)
        .env("AC_TASK_OPS_CHILD_LOCK", "1")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("child-ready").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    if !root.join("child-ready").exists() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child failed to acquire lock");
    }
    assert!(matches!(
        LockGuard::acquire(
            &root.join("TASK.md.lock"),
            Duration::from_millis(100),
            Duration::ZERO
        ),
        Err(TaskOpError::LockTimeout)
    ));
    child.kill().unwrap();
    child.wait().unwrap();
    let held = LockGuard::acquire(
        &root.join("TASK.md.lock"),
        Duration::from_secs(1),
        Duration::ZERO,
    )
    .unwrap();
    drop(held);
    assert_eq!(
        std::fs::read_to_string(root.join("TASK.md.lock")).unwrap(),
        "old stale contents"
    );
    assert!(root.join("TASK.md.lock").exists());
    std::fs::create_dir(root.join("directory-lock")).unwrap();
    assert!(matches!(
        LockGuard::acquire(&root.join("directory-lock"), Duration::ZERO, Duration::ZERO),
        Err(TaskOpError::LockIo(..))
    ));
}

#[test]
fn issue_2837_process_exit_publication_boundaries_recover_pair() {
    for point in [
        "after_journal_publish",
        "after_task_rename",
        "after_status_rename",
        "task_target_sync",
        "status_target_sync",
        "journal_remove",
    ] {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "prior task").unwrap();
        std::fs::write(root.join(STATUS_NAME), "prior log\npartial").unwrap();
        let status = issue_2837_child(root)
            .env("AC_TASK_OPS_CHILD_BOUNDARY", point)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(71), "{point}");
        assert!(root.join(JOURNAL_NAME).exists());
        let s = read_snapshot(root).unwrap();
        assert_eq!(s.task_title.as_deref(), Some("Clean"));
        assert_eq!(s.status_record.unwrap().kind, "topic_started");
        assert!(!root.join(JOURNAL_NAME).exists());
        assert_eq!(
            std::fs::read_to_string(root.join("TASK.20261002-210000.bak.md")).unwrap(),
            "prior task"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("TASK-status.20261002-210000.bak.jsonl")).unwrap(),
            "prior log\npartial"
        );
    }
}

#[test]
fn issue_2837_unknown_fields_and_preceding_corrupt_row_partial() {
    let f = issue_2837_fixture();
    let root = f.path();
    let r = issue_2837_append(root, "legacy:0", "text");
    let mut value = serde_json::to_value(&r.record).unwrap();
    value["futureV1Extension"] = serde_json::json!({"ok":true});
    let mut row = serde_json::to_vec(&value).unwrap();
    row.push(b'\n');
    std::fs::write(root.join(STATUS_NAME), &row).unwrap();
    assert_eq!(read_snapshot(root).unwrap().status.as_deref(), Some("text"));
    let mut bytes = row;
    bytes.extend_from_slice(b"{broken}\n\xf0\x9f");
    std::fs::write(root.join(STATUS_NAME), &bytes).unwrap();
    assert!(matches!(
        read_snapshot(root),
        Err(TaskOpError::InvalidStatus(_))
    ));
    assert!(append_status(root, &r.revision, &issue_2837_id(), "next", "p/a").is_err());
    assert_eq!(issue_2837_log(root), bytes);
}

#[test]
fn issue_2837_recovery_preserves_foreign_unused_stage() {
    let f = issue_2837_fixture();
    let root = f.path();
    std::fs::write(root.join("TASK.md"), "old").unwrap();
    issue_2837_fault(Some("after_status_rename"));
    assert!(issue_2837_clean(root).is_err());
    issue_2837_fault(None);
    let journal: CleanJournal =
        serde_json::from_slice(&std::fs::read(root.join(JOURNAL_NAME)).unwrap()).unwrap();
    let foreign = root.join(&journal.task.stage);
    std::fs::write(&foreign, "foreign replacement of unused stage").unwrap();
    recover_clean_pair(root).unwrap();
    assert_eq!(
        std::fs::read_to_string(foreign).unwrap(),
        "foreign replacement of unused stage"
    );
    assert!(!root.join(JOURNAL_NAME).exists());
}

#[test]
fn issue_2837_clean_external_change_before_journal_keeps_pair() {
    let f = issue_2837_fixture();
    let root = f.path();
    std::fs::write(root.join("TASK.md"), "old").unwrap();
    std::fs::write(root.join(STATUS_NAME), "old log").unwrap();
    IO_MUTATION.with(|v| {
        *v.borrow_mut() = Some((
            "before_source_recheck".into(),
            root.join(STATUS_NAME),
            b"external history".to_vec(),
        ))
    });
    assert!(matches!(
        issue_2837_clean(root),
        Err(TaskOpError::CleanRecoveryConflict(_))
    ));
    assert_eq!(
        std::fs::read_to_string(root.join("TASK.md")).unwrap(),
        "old"
    );
    assert_eq!(issue_2837_log(root), b"external history");
    assert!(!root.join(JOURNAL_NAME).exists());
}

#[test]
fn issue_2837_clean_external_change_immediately_before_rename() {
    let f = issue_2837_fixture();
    let root = f.path();
    std::fs::write(root.join("TASK.md"), "old").unwrap();
    IO_MUTATION.with(|v| {
        *v.borrow_mut() = Some((
            "task_rename".into(),
            root.join("TASK.md"),
            b"external task".to_vec(),
        ))
    });
    assert!(matches!(
        issue_2837_clean(root),
        Err(TaskOpError::CleanRecoveryConflict(_))
    ));
    assert_eq!(
        std::fs::read_to_string(root.join("TASK.md")).unwrap(),
        "external task"
    );
    assert!(root.join(JOURNAL_NAME).exists());
    assert!(!root.join(STATUS_NAME).exists());
}
#[test]
fn issue_2837_append_exact_encoded_limit_and_one_byte_over() {
    let first = issue_2837_fixture();
    let root = first.path();
    issue_2837_append(root, "legacy:0", "x");
    let overhead = issue_2837_log(root).len() - 1;
    let exact = issue_2837_fixture();
    let root = exact.path();
    let text = "x".repeat(STATUS_LIMIT - overhead);
    issue_2837_append(root, "legacy:0", &text);
    assert_eq!(issue_2837_log(root).len(), STATUS_LIMIT);
    let over = issue_2837_fixture();
    let root = over.path();
    assert!(matches!(
        append_status(
            root,
            "legacy:0",
            &issue_2837_id(),
            &format!("{text}x"),
            "project:room-10/agent"
        ),
        Err(TaskOpError::StatusTooLarge)
    ));
    assert!(!root.join(STATUS_NAME).exists());
}

#[test]
fn issue_2837_backup_partial_failure_cleanup_and_finished_empty_side() {
    let f = issue_2837_fixture();
    let root = f.path();
    std::fs::write(root.join("TASK.md"), "prior description").unwrap();
    let original = vec![b'x'; 200_000];
    std::fs::write(root.join(STATUS_NAME), &original).unwrap();
    issue_2837_fault(Some("status_backup_after_write"));
    assert!(matches!(
        issue_2837_clean(root),
        Err(TaskOpError::WriteFailed(_))
    ));
    issue_2837_fault(None);
    assert_eq!(issue_2837_log(root), original);
    assert_eq!(
        std::fs::read_to_string(root.join("TASK.20261002-210000.bak.md")).unwrap(),
        "prior description"
    );
    assert!(!root.join("TASK-status.20261002-210000.bak.jsonl").exists());
    let f = issue_2837_fixture();
    let root = f.path();
    std::fs::write(root.join(STATUS_NAME), "prior log").unwrap();
    issue_2837_fault(Some("status_backup_sync"));
    assert!(matches!(
        issue_2837_clean(root),
        Err(TaskOpError::WriteFailed(_))
    ));
    issue_2837_fault(None);
    assert_eq!(
        std::fs::read(root.join("TASK.20261002-210000.bak.md")).unwrap(),
        Vec::<u8>::new()
    );
    assert!(!root.join("TASK-status.20261002-210000.bak.jsonl").exists());
    assert!(!root.join("TASK.md").exists());
    assert_eq!(issue_2837_log(root), b"prior log");
}
fn issue_2837_assert_backup_fault_calls(point: &str) {
    let calls = issue_2837_calls();
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == point).count(),
        1,
        "must execute requested boundary"
    );
    if point == "task_backup_after_write" {
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.as_str() == "task_backup_write")
                .count(),
            1,
            "fail immediately after the first block"
        );
        assert!(!calls.iter().any(|c| c == "task_backup_sync"));
    }
    if point == "task_backup_write" {
        assert!(!calls.iter().any(|c| c == "task_backup_after_write"));
    }
    if point == "status_backup_write" {
        assert!(calls.iter().any(|c| c == "task_backup_sync"));
        assert!(!calls.iter().any(|c| c == "status_backup_after_write"));
    }
}

fn issue_2837_assert_failed_backup_pair(
    root: &Path,
    original_task: &[u8],
    original_log: &[u8],
    task_backup_completed: bool,
) {
    assert_eq!(
        std::fs::read(root.join("TASK.md")).unwrap(),
        original_task,
        "original TASK unchanged"
    );
    assert_eq!(issue_2837_log(root), original_log, "original log unchanged");
    assert!(
        !root.join(JOURNAL_NAME).exists(),
        "no reset transaction published"
    );
    let (task_backup, status_backup) = backup_names("20261002-210000", 0);
    assert!(
        !root.join(&status_backup).exists(),
        "own empty log reservation removed"
    );
    if task_backup_completed {
        assert_eq!(
            std::fs::read(root.join(&task_backup)).unwrap(),
            original_task,
            "completed TASK backup preserved"
        );
    } else {
        assert!(
            !root.join(&task_backup).exists(),
            "own empty/partial TASK reservation removed"
        );
    }
    let backups = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".bak."))
        .count();
    assert_eq!(backups, usize::from(task_backup_completed));
    assert!(!std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(".tmp.")));
}

fn issue_2837_backup_write_failure_case(point: &str, task_backup_completed: bool) {
    let fixture = issue_2837_fixture();
    let root = fixture.path();
    let original_task = format!(
        "\u{feff}---\r\ntitle: 'USER: Keep original'\r\n---\r\n{}\r\n",
        "original description 🦀\r\n".repeat(6_000)
    )
    .into_bytes();
    assert!(
        original_task.len() > 65_536,
        "TASK must span multiple copy blocks"
    );
    let original_log = b"{old completed row}\npartial\xf0\x9f";
    std::fs::write(root.join("TASK.md"), &original_task).unwrap();
    std::fs::write(root.join(STATUS_NAME), original_log).unwrap();
    issue_2837_fault(Some(point));
    assert!(
        matches!(issue_2837_clean(root), Err(TaskOpError::WriteFailed(_))),
        "{point}"
    );
    issue_2837_assert_backup_fault_calls(point);
    issue_2837_fault(None);
    issue_2837_assert_failed_backup_pair(root, &original_task, original_log, task_backup_completed);
    let (task_backup, _) = backup_names("20261002-210000", 0);
    assert!(
        matches!(issue_2837_clean(root), Ok(EditOutcome::Wrote { .. })),
        "retry succeeds"
    );
    let suffix = u32::from(task_backup_completed);
    let (retry_task_backup, retry_status_backup) = backup_names("20261002-210000", suffix);
    assert_eq!(
        std::fs::read(root.join(retry_task_backup)).unwrap(),
        original_task,
        "retry archives exact TASK"
    );
    assert_eq!(
        std::fs::read(root.join(retry_status_backup)).unwrap(),
        original_log,
        "retry archives exact log including partial UTF-8"
    );
    if task_backup_completed {
        assert_eq!(
            std::fs::read(root.join(task_backup)).unwrap(),
            original_task,
            "retry cannot overwrite finished archive"
        );
    }
    let snapshot = read_snapshot(root).unwrap();
    assert_eq!(snapshot.task_title.as_deref(), Some("Clean"));
    assert_eq!(snapshot.description, "");
    let seed = snapshot.status_record.unwrap();
    assert_eq!(seed.kind, "topic_started");
    assert_eq!(seed.sequence, 0);
    assert_eq!(issue_2837_lines(root), 1);
    assert!(!root.join(JOURNAL_NAME).exists());
}

#[test]
fn issue_2837_task_backup_write_failure_preserves_originals_and_retries() {
    issue_2837_backup_write_failure_case("task_backup_write", false);
}

#[test]
fn issue_2837_status_backup_write_failure_preserves_finished_task_and_retries() {
    issue_2837_backup_write_failure_case("status_backup_write", true);
}

#[test]
fn issue_2837_task_backup_after_write_cleans_first_partial_block_and_retries() {
    issue_2837_backup_write_failure_case("task_backup_after_write", false);
}
