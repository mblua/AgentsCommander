use super::*;

/// Success path: a clean WG dir with no blockers gets renamed and removed.
/// The original path must not exist after the call, and there must be no
/// `.deleting-*` orphan left in the parent.
#[test]
fn try_atomic_delete_wg_removes_clean_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-1-test");
    std::fs::create_dir(&wg_dir).expect("create wg_dir");
    std::fs::write(wg_dir.join("TASK.md"), "# test\n").expect("write TASK.md");
    std::fs::create_dir(wg_dir.join("repo-foo")).expect("create repo-foo");
    std::fs::write(wg_dir.join("repo-foo").join("README.md"), "x").expect("write inside");

    let outcome = try_atomic_delete_wg(&wg_dir);
    assert!(
        matches!(outcome, WgDeleteOutcome::Deleted),
        "clean dir must report Deleted"
    );
    assert!(!wg_dir.exists(), "wg_dir must be gone after delete");

    // Parent must contain no `.deleting-*` orphan.
    let parent = tmp.path();
    let orphans: Vec<_> = std::fs::read_dir(parent)
        .expect("read tempdir")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".deleting-"))
        .collect();
    assert!(
        orphans.is_empty(),
        "no .deleting-* orphan should remain after a clean delete; found {:?}",
        orphans.iter().map(|e| e.file_name()).collect::<Vec<_>>()
    );
}

/// Other-error path: deleting a nonexistent WG dir surfaces as
/// `WgDeleteOutcome::Other` (NotFound), NOT as `Blocked`.
#[test]
fn try_atomic_delete_wg_classifies_missing_dir_as_other() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("does-not-exist");
    let outcome = try_atomic_delete_wg(&missing);
    match outcome {
        WgDeleteOutcome::Other(e) => {
            assert_eq!(
                e.kind(),
                std::io::ErrorKind::NotFound,
                "missing-dir error must classify as NotFound"
            );
        }
        WgDeleteOutcome::Blocked(_) => {
            panic!("missing dir must NOT classify as Blocked")
        }
        WgDeleteOutcome::Deleted => panic!("missing dir cannot be Deleted"),
        WgDeleteOutcome::Partial { .. } => panic!("missing dir cannot be Partial"),
    }
}

#[test]
fn try_atomic_delete_wg_reports_partial_orphan_when_remove_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-1-test");
    std::fs::create_dir(&wg_dir).expect("create wg_dir");
    std::fs::write(wg_dir.join("TASK.md"), "# test\n").expect("write TASK.md");

    let outcome = try_atomic_delete_wg_with_remove(&wg_dir, |_path| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "forced remove failure",
        ))
    });
    match outcome {
        WgDeleteOutcome::Partial { orphan_path, error } => {
            assert!(!wg_dir.exists(), "original workgroup path should be gone");
            assert!(orphan_path.is_dir(), "orphan should remain on disk");
            assert!(orphan_path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("orphan filename")
                .starts_with(".deleting-wg-1-test-"));
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(error.to_string().contains("forced remove failure"));
        }
        WgDeleteOutcome::Deleted => panic!("remove failure cannot be Deleted"),
        WgDeleteOutcome::Blocked(e) => panic!("remove failure cannot be Blocked: {}", e),
        WgDeleteOutcome::Other(e) => panic!("remove failure cannot be Other: {}", e),
    }
}

#[tokio::test]
async fn gui_partial_delete_outcome_returns_error_before_refresh() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-1-test");
    let orphan = tmp.path().join(".deleting-wg-1-test-forced");
    std::fs::create_dir(&wg_dir).expect("create wg_dir");
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));

    let err = delete_workgroup_dir_backend_with_outcome(
        &wg_dir,
        "wg-1-test",
        &manager,
        WgDeleteOutcome::Partial {
            orphan_path: orphan.clone(),
            error: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "forced remove failure",
            ),
        },
    )
    .await
    .expect_err("partial delete must error before caller can refresh");

    assert!(err.contains("Partial room delete"));
    assert!(err.contains(&orphan.to_string_lossy().to_string()));
}

#[test]
fn validate_delete_root_rejects_non_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("wg-1-test");
    std::fs::write(&file, "not a dir").expect("write file");

    assert_eq!(
        validate_delete_root_not_link_or_reparse(&file).unwrap_err(),
        "delete_root_not_directory"
    );
}

#[cfg(unix)]
#[test]
fn validate_delete_root_rejects_symlink_root() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tempdir");
    let real = tmp.path().join("real");
    let link = tmp.path().join("wg-1-test");
    std::fs::create_dir(&real).expect("create real");
    symlink(&real, &link).expect("create symlink");

    assert_eq!(
        validate_delete_root_not_link_or_reparse(&link).unwrap_err(),
        "delete_root_is_symlink"
    );
}

#[test]
fn synthetic_windows_reparse_attribute_is_always_classified_for_rejection() {
    assert!(!metadata_parts_are_link_or_reparse(false, 0));
    assert!(metadata_parts_are_link_or_reparse(false, 0x0000_0400));
    assert!(metadata_parts_are_link_or_reparse(false, 0x0000_0410));
}

#[cfg(windows)]
#[test]
fn validate_delete_root_rejects_reparse_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let real = tmp.path().join("real");
    let junction = tmp.path().join("wg-1-test");
    std::fs::create_dir(&real).expect("create real");
    let output = std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            junction.to_str().expect("junction path"),
            real.to_str().expect("real path"),
        ])
        .output()
        .expect("run mklink");
    if !output.status.success() {
        println!(
            "skipping validate_delete_root reparse check; see docs/testing/destructive-filesystem-regression.md#helper-reparse-root-check: stdout: {} stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    assert_eq!(
        validate_delete_root_not_link_or_reparse(&junction).unwrap_err(),
        "delete_root_is_reparse_point"
    );
}

#[tokio::test]
async fn gui_live_session_refusal_precedes_delete() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-1-test");
    std::fs::create_dir(&wg_dir).expect("create wg_dir");
    let work_dir = wg_dir.join("__agent_dev-rust");
    std::fs::create_dir_all(&work_dir).expect("create work_dir");
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    {
        let guard = manager.read().await;
        guard
            .create_session(
                "shell".to_string(),
                Vec::new(),
                work_dir.to_string_lossy().to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create live session");
    }

    let err = ensure_no_live_sessions_under_manager(&wg_dir, &manager)
        .await
        .expect_err("live session should block delete");
    assert!(err.contains("Cannot delete while live sessions exist"));
    assert!(err.contains("Session 1"));
    assert!(err.contains("Active"));
}

#[tokio::test]
async fn delete_workgroup_dir_backend_returns_blockers_json() {
    #[cfg(not(windows))]
    {
        return;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x00000001;

        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-1-test");
        std::fs::create_dir(&wg_dir).expect("create wg_dir");
        let inside = wg_dir.join("locked.bin");
        std::fs::write(&inside, b"hold me").expect("write inside file");
        let _handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&inside)
            .expect("open with restricted share mode");
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));

        let err = delete_workgroup_dir_backend(&wg_dir, "wg-1-test", &manager)
            .await
            .expect_err("blocked delete should return blockers json");
        let json = err
            .strip_prefix("BLOCKERS:")
            .unwrap_or_else(|| panic!("expected BLOCKERS prefix, got {}", err));
        let report: serde_json::Value = serde_json::from_str(json).expect("blockers json");
        assert_eq!(report["workgroup"], "wg-1-test");
        assert!(!report["rawDeleteError"]
            .as_str()
            .expect("rawDeleteError")
            .is_empty());
        assert!(wg_dir.is_dir(), "blocked tree should remain intact");
    }
}

/// Blocked path (Windows-only): a child file opened without
/// `FILE_SHARE_DELETE` blocks the parent-dir rename with
/// `ERROR_SHARING_VIOLATION` (32). The rename must fail before any file is
/// touched, so the WG dir + child must both be intact afterward.
#[cfg(windows)]
#[test]
fn try_atomic_delete_wg_blocked_with_restrictive_share_mode() {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ only — explicitly NO FILE_SHARE_DELETE.
    const FILE_SHARE_READ: u32 = 0x00000001;

    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-1-test");
    std::fs::create_dir(&wg_dir).expect("create wg_dir");
    let inside = wg_dir.join("locked.bin");
    std::fs::write(&inside, b"hold me").expect("write inside file");

    // Hold a handle that denies DELETE share. Drop scope at end of test.
    let _handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&inside)
        .expect("open with restricted share mode");

    let outcome = try_atomic_delete_wg(&wg_dir);
    match &outcome {
        WgDeleteOutcome::Blocked(_) => {
            assert!(wg_dir.is_dir(), "wg_dir must remain after blocked rename");
            assert!(
                inside.is_file(),
                "inner file must remain after blocked rename"
            );
        }
        WgDeleteOutcome::Deleted => {
            panic!(
                "expected Blocked when child file is held without FILE_SHARE_DELETE; \
                 got Deleted (rename succeeded — Windows behavior may have changed)"
            );
        }
        WgDeleteOutcome::Other(e) => {
            panic!("expected Blocked, got Other({:?}={})", e.kind(), e);
        }
        WgDeleteOutcome::Partial { .. } => {
            panic!("blocked rename cannot report Partial");
        }
    }
}

/// Suffix scheme invariant (#113 follow-up): the orphan name produced on a
/// rename-success-then-remove-fails race must NOT match the
/// `entity_prefix::has_entity_prefix` filters used by `ac_discovery` and
/// `cli::list_peers`. We test this by asserting the format directly:
/// the temp name starts with `.deleting-`, which automatically dodges BOTH
/// the `room-` and the legacy `wg-` prefix.
#[test]
fn temp_name_format_dodges_workgroup_filter() {
    // Inline the same name construction `try_atomic_delete_wg` uses, so the
    // assertion locks the contract independent of fs interaction.
    let original_name = "wg-7-dev-team";
    let temp_name = format!(".deleting-{}-{}", original_name, uuid::Uuid::new_v4());
    assert!(
        temp_name.starts_with(".deleting-"),
        "temp name must start with .deleting- so future cleanup tooling can identify orphans"
    );
    assert!(
        !crate::config::entity_prefix::has_entity_prefix(&temp_name),
        "temp name must NOT match the widened room-/wg- discovery filter (would surface as a ghost room)"
    );
}

/// `is_rename_blocked_by_handle` matches `ERROR_ACCESS_DENIED` (5).
/// MoveFileEx returns 5 — not 32 — when an open handle on a descendant
/// lacks `FILE_SHARE_DELETE`. Empirical, verified by the
/// `try_atomic_delete_wg_blocked_with_restrictive_share_mode` test below.
#[cfg(windows)]
#[test]
fn is_rename_blocked_by_handle_matches_access_denied() {
    let e = std::io::Error::from_raw_os_error(5);
    assert!(
        is_rename_blocked_by_handle(&e),
        "os error 5 (ERROR_ACCESS_DENIED) must classify as a rename blocker"
    );
}

/// `is_rename_blocked_by_handle` is a superset of `is_file_in_use_error` —
/// the existing 32/33/1224 codes still match.
#[cfg(windows)]
#[test]
fn is_rename_blocked_by_handle_matches_file_in_use_codes() {
    for code in [32, 33, 1224] {
        let e = std::io::Error::from_raw_os_error(code);
        assert!(
            is_rename_blocked_by_handle(&e),
            "os error {} must classify as a rename blocker",
            code
        );
    }
}

/// `is_rename_blocked_by_handle` does NOT match unrelated errors. NotFound
/// (2) is the canonical legitimate non-blocker error path.
#[cfg(windows)]
#[test]
fn is_rename_blocked_by_handle_rejects_not_found() {
    let e = std::io::Error::from_raw_os_error(2);
    assert!(
        !is_rename_blocked_by_handle(&e),
        "os error 2 (ERROR_FILE_NOT_FOUND) must NOT classify as blocker"
    );
}

/// Off Windows the helper always returns false — diagnostic isn't run on
/// non-Windows platforms.
#[cfg(not(windows))]
#[test]
fn is_rename_blocked_by_handle_no_op_on_non_windows() {
    let e = std::io::Error::from_raw_os_error(5);
    assert!(
        !is_rename_blocked_by_handle(&e),
        "non-Windows must always return false"
    );
}
