use super::*;

/// §7.23 (covers G.4.8): `strip_long_prefix_str` handles drive paths,
/// UNC paths, and unprefixed paths uniformly.
#[test]
fn strip_long_prefix_str_handles_drive_unc_and_no_prefix() {
    assert_eq!(
        strip_long_prefix_str(r"\\?\UNC\server\share\proj"),
        r"\\server\share\proj"
    );
    assert_eq!(
        strip_long_prefix_str(r"\??\UNC\server\share\proj"),
        r"\\server\share\proj"
    );
    assert_eq!(
        strip_long_prefix_str(r"\\?\C:\Users\me\proj"),
        r"C:\Users\me\proj"
    );
    assert_eq!(
        strip_long_prefix_str(r"\??\C:\Users\me\proj"),
        r"C:\Users\me\proj"
    );
    assert_eq!(
        strip_long_prefix_str(r"C:\Users\me\proj"),
        r"C:\Users\me\proj"
    );
    // Edge: empty string and prefix-only.
    assert_eq!(strip_long_prefix_str(""), "");
    assert_eq!(strip_long_prefix_str(r"\\?\"), "");
}

/// §7.2: `canonicalize_for_compare` round-trips against
/// `std::fs::canonicalize` + manual prefix strip.
#[test]
fn canonicalize_for_compare_strips_prefix_for_existing_path() {
    let tmp = std::env::temp_dir();
    let canon_via_helper = canonicalize_for_compare(&tmp);
    let canon_raw = std::fs::canonicalize(&tmp).expect("canonicalize tempdir");
    let raw_str = canon_raw.to_string_lossy();
    let expected = strip_long_prefix_str(&raw_str);
    assert_eq!(canon_via_helper.to_string_lossy(), expected);
}

/// §7.3: `scan_ac_sessions` filters sessions by canonical-path prefix.
/// Exercises the path canonicalisation + `PathBuf::starts_with` filter
/// on real disk paths.
#[tokio::test]
async fn scan_ac_sessions_filters_by_canonical_prefix() {
    use crate::session::manager::SessionManager;

    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-test");
    let inside_session_dir = wg_dir.join("__agent_inside");
    let outside_session_dir = tmp.path().join("outside");
    std::fs::create_dir_all(&inside_session_dir).expect("create inside dir");
    std::fs::create_dir_all(&outside_session_dir).expect("create outside dir");

    let mgr = SessionManager::new();
    let _ = mgr
        .create_session(
            "powershell.exe".into(),
            vec![],
            inside_session_dir.to_string_lossy().to_string(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create inside session");
    let _ = mgr
        .create_session(
            "powershell.exe".into(),
            vec![],
            outside_session_dir.to_string_lossy().to_string(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create outside session");
    let mgr = Arc::new(tokio::sync::RwLock::new(mgr));

    let canonical_wg = canonicalize_for_compare(&wg_dir);
    let blockers = scan_ac_sessions(&canonical_wg, &mgr).await;

    assert_eq!(
        blockers.live_sessions.len(),
        1,
        "exactly one session inside wg_dir"
    );
    assert!(
        blockers.live_sessions[0].cwd.contains("__agent_inside"),
        "filtered session must be the inside one, got cwd={}",
        blockers.live_sessions[0].cwd
    );
    assert!(blockers.exited_session_records_ignored.is_empty());
}

#[tokio::test]
async fn scan_ac_sessions_ignores_exited_records_under_workgroup() {
    use crate::session::manager::SessionManager;

    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-test");
    let inside_session_dir = wg_dir.join("__agent_inside");
    std::fs::create_dir_all(&inside_session_dir).expect("create inside dir");

    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "powershell.exe".into(),
            vec![],
            inside_session_dir.to_string_lossy().to_string(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create inside session");
    mgr.mark_exited(session.id, 0).await;
    let mgr = Arc::new(tokio::sync::RwLock::new(mgr));

    let canonical_wg = canonicalize_for_compare(&wg_dir);
    let blockers = scan_ac_sessions(&canonical_wg, &mgr).await;

    assert!(blockers.live_sessions.is_empty());
    assert_eq!(blockers.exited_session_records_ignored.len(), 1);
    let ignored = &blockers.exited_session_records_ignored[0];
    assert_eq!(ignored.status, "Exited(0)");
    assert!(!ignored.waiting_for_input);
}

#[tokio::test]
async fn scan_ac_sessions_splits_live_and_exited_records() {
    use crate::session::manager::SessionManager;

    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-test");
    let live_dir = wg_dir.join("__agent_live");
    let exited_dir = wg_dir.join("__agent_exited");
    std::fs::create_dir_all(&live_dir).expect("create live dir");
    std::fs::create_dir_all(&exited_dir).expect("create exited dir");

    let mgr = SessionManager::new();
    let _live = mgr
        .create_session(
            "powershell.exe".into(),
            vec![],
            live_dir.to_string_lossy().to_string(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create live session");
    let exited = mgr
        .create_session(
            "powershell.exe".into(),
            vec![],
            exited_dir.to_string_lossy().to_string(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create exited session");
    mgr.mark_exited(exited.id, 0).await;
    let mgr = Arc::new(tokio::sync::RwLock::new(mgr));

    let canonical_wg = canonicalize_for_compare(&wg_dir);
    let blockers = scan_ac_sessions(&canonical_wg, &mgr).await;

    assert_eq!(blockers.live_sessions.len(), 1);
    assert_eq!(blockers.exited_session_records_ignored.len(), 1);
    assert!(blockers.live_sessions[0].cwd.contains("__agent_live"));
    assert!(blockers.exited_session_records_ignored[0]
        .cwd
        .contains("__agent_exited"));
}

/// §7.4 (with R.1.a fix): `BlockerReport` JSON shape preserves camelCase
/// at the wire boundary AND the `Platform` enum serializes lowercase.
#[test]
fn blocker_report_serializes_with_camelcase_fields() {
    let report = BlockerReport {
        workgroup: "wg-7-dev-team".into(),
        platform: Platform::Windows,
        diagnostic_available: true,
        restart_manager_available: false,
        restart_manager_error: Some(DiagnosticError {
            message: "RmStartSession failed: WIN32_ERROR=5 (Access denied)".into(),
            code: Some(5),
            meaning: Some("Access denied".into()),
        }),
        raw_os_error: "...".into(),
        raw_delete_error: "... (os error 32)".into(),
        live_sessions: vec![BlockerSession {
            session_id: "abc".into(),
            agent_name: "agentscommander:wg-7-dev-team/architect".into(),
            cwd: r"C:\foo".into(),
        }],
        exited_session_records_ignored: vec![IgnoredSessionRecord {
            session_id: "def".into(),
            agent_name: "agentscommander:wg-7-dev-team/dev-rust".into(),
            cwd: r"C:\foo\__agent_dev-rust".into(),
            status: "Exited(0)".into(),
            waiting_for_input: false,
        }],
        external_processes: vec![BlockerProcess {
            pid: 42,
            name: "git.exe".into(),
            cwd: Some(r"C:\foo".into()),
            files: vec![r"C:\foo\bar".into()],
        }],
        sessions: vec![BlockerSession {
            session_id: "abc".into(),
            agent_name: "agentscommander:wg-7-dev-team/architect".into(),
            cwd: r"C:\foo".into(),
        }],
        processes: vec![BlockerProcess {
            pid: 42,
            name: "git.exe".into(),
            cwd: Some(r"C:\foo".into()),
            files: vec![r"C:\foo\bar".into()],
        }],
    };
    let json: serde_json::Value = serde_json::to_value(&report).expect("serialize");
    // Top-level fields
    for k in &[
        "workgroup",
        "platform",
        "diagnosticAvailable",
        "restartManagerAvailable",
        "restartManagerError",
        "rawOsError",
        "rawDeleteError",
        "liveSessions",
        "exitedSessionRecordsIgnored",
        "externalProcesses",
        "sessions",
        "processes",
    ] {
        assert!(json.get(*k).is_some(), "missing field: {}", k);
    }
    // R.1.a: Platform enum must serialize to lowercase string.
    assert_eq!(
        json.get("platform").and_then(|v| v.as_str()),
        Some("windows"),
        "Platform enum must serialize to lowercase string"
    );
    // BlockerSession fields
    let s = &json["liveSessions"][0];
    for k in &["sessionId", "agentName", "cwd"] {
        assert!(s.get(*k).is_some(), "missing session field: {}", k);
    }
    // BlockerProcess fields
    let p = &json["externalProcesses"][0];
    for k in &["pid", "name", "cwd", "files"] {
        assert!(p.get(*k).is_some(), "missing process field: {}", k);
    }
    let ignored = &json["exitedSessionRecordsIgnored"][0];
    for k in &["sessionId", "agentName", "cwd", "status", "waitingForInput"] {
        assert!(ignored.get(*k).is_some(), "missing ignored field: {}", k);
    }
    // Snake-case must NOT leak at the wire boundary.
    for k in &[
        "diagnostic_available",
        "restart_manager_available",
        "restart_manager_error",
        "raw_os_error",
        "raw_delete_error",
        "live_sessions",
        "exited_session_records_ignored",
        "external_processes",
        "session_id",
        "agent_name",
        "waiting_for_input",
    ] {
        assert!(
            json.get(*k).is_none(),
            "leaked snake_case field at top level: {}",
            k
        );
    }
    for k in &["session_id", "agent_name"] {
        assert!(
            s.get(*k).is_none(),
            "leaked snake_case session field: {}",
            k
        );
    }
    assert!(ignored.get("waiting_for_input").is_none());
}

#[test]
fn win32_error_formats_access_denied() {
    let err = win32_error("RmStartSession failed", 5);
    assert_eq!(err.code, Some(5));
    assert_eq!(err.meaning.as_deref(), Some("Access denied"));
    assert!(err.message.contains("WIN32_ERROR=5 (Access denied)"));
}

#[test]
fn win32_error_formats_unknown_code() {
    let err = win32_error("RmGetList failed", 99999);
    assert_eq!(err.code, Some(99999));
    assert_eq!(err.meaning.as_deref(), Some("Unknown Win32 error"));
    assert!(err
        .message
        .contains("WIN32_ERROR=99999 (Unknown Win32 error)"));
}

fn sample_live_session(name: &str) -> BlockerSession {
    BlockerSession {
        session_id: format!("session-{name}"),
        agent_name: format!("agentscommander:wg-7-dev-team/{name}"),
        cwd: format!(r"C:\work\wg-7-dev-team\__agent_{name}"),
    }
}

fn sample_ignored_session(name: &str) -> IgnoredSessionRecord {
    IgnoredSessionRecord {
        session_id: format!("session-{name}"),
        agent_name: format!("agentscommander:wg-7-dev-team/{name}"),
        cwd: format!(r"C:\work\wg-7-dev-team\__agent_{name}"),
        status: "Exited(0)".into(),
        waiting_for_input: false,
    }
}

fn sample_process(pid: u32) -> BlockerProcess {
    BlockerProcess {
        pid,
        name: "powershell.exe".into(),
        cwd: Some(r"C:\work\wg-7-dev-team".into()),
        files: Vec::new(),
    }
}

#[test]
fn build_blocker_report_live_and_ignored_sessions() {
    let ac_scan = AcSessionScan {
        live_sessions: vec![sample_live_session("dev-rust")],
        exited_session_records_ignored: vec![sample_ignored_session("architect")],
    };
    let report = build_blocker_report(
        "wg-7-dev-team",
        Platform::Windows,
        "sharing violation",
        ac_scan,
        ExternalProcessScan::default(),
    );

    assert_eq!(report.live_sessions.len(), 1);
    assert_eq!(report.exited_session_records_ignored.len(), 1);
    assert_eq!(report.sessions, report.live_sessions);
    assert_eq!(report.processes, report.external_processes);
    assert_eq!(report.raw_os_error, "sharing violation");
    assert_eq!(report.raw_delete_error, "sharing violation");
}

#[test]
fn build_blocker_report_rm_failure_with_cwd_processes() {
    let rm_error = win32_error("RmStartSession failed", 5);
    let report = build_blocker_report(
        "wg-7-dev-team",
        Platform::Windows,
        "sharing violation",
        AcSessionScan::default(),
        ExternalProcessScan {
            processes: vec![sample_process(42)],
            restart_manager_available: false,
            restart_manager_error: Some(rm_error.clone()),
            cwd_scan_error: None,
        },
    );

    assert_eq!(report.restart_manager_error, Some(rm_error));
    assert!(!report.restart_manager_available);
    assert!(report.diagnostic_available);
    assert_eq!(report.processes, report.external_processes);
    assert_eq!(report.sessions, report.live_sessions);
}

#[test]
fn build_blocker_report_rm_failure_no_processes() {
    let rm_error = win32_error("RmGetList probe failed", 87);
    let report = build_blocker_report(
        "wg-7-dev-team",
        Platform::Windows,
        "lock violation",
        AcSessionScan::default(),
        ExternalProcessScan {
            processes: Vec::new(),
            restart_manager_available: false,
            restart_manager_error: Some(rm_error.clone()),
            cwd_scan_error: None,
        },
    );

    assert_eq!(report.restart_manager_error, Some(rm_error));
    assert!(!report.restart_manager_available);
    assert!(!report.diagnostic_available);
    assert_eq!(report.raw_os_error, "lock violation");
    assert_eq!(report.raw_delete_error, "lock violation");
    assert_eq!(report.processes, report.external_processes);
    assert_eq!(report.sessions, report.live_sessions);
}

#[test]
fn blocker_report_serializes_without_windows_scanner_fields_present() {
    let report = build_blocker_report(
        "wg-7-dev-team",
        Platform::Linux,
        "permission denied",
        AcSessionScan::default(),
        ExternalProcessScan::default(),
    );

    let json: serde_json::Value = serde_json::to_value(&report).expect("serialize");
    assert_eq!(
        json.get("restartManagerAvailable")
            .and_then(|v| v.as_bool()),
        Some(false)
    );
    assert!(json.get("restartManagerError").is_none());
    assert!(json.get("externalProcesses").is_some());
}

/// §7.1 (Windows variant): `is_file_in_use_error` matches `ERROR_SHARING_VIOLATION` (32).
#[cfg(windows)]
#[test]
fn is_file_in_use_error_matches_sharing_violation_on_windows() {
    use crate::commands::entity_creation::is_file_in_use_error;
    let e = std::io::Error::from_raw_os_error(32);
    assert!(is_file_in_use_error(&e), "os error 32 must match");
}

/// §7.1 (Windows variant): `is_file_in_use_error` matches `ERROR_LOCK_VIOLATION` (33).
#[cfg(windows)]
#[test]
fn is_file_in_use_error_matches_lock_violation_on_windows() {
    use crate::commands::entity_creation::is_file_in_use_error;
    let e = std::io::Error::from_raw_os_error(33);
    assert!(is_file_in_use_error(&e), "os error 33 must match");
}

/// §7.1 (Windows variant): `is_file_in_use_error` matches `ERROR_USER_MAPPED_FILE` (1224).
/// This is the VSCode / IDE memory-mapped-I/O case — the motivating scenario for the
/// diagnostic. See plan §6.1.
#[cfg(windows)]
#[test]
fn is_file_in_use_error_matches_user_mapped_file_on_windows() {
    use crate::commands::entity_creation::is_file_in_use_error;
    let e = std::io::Error::from_raw_os_error(1224);
    assert!(is_file_in_use_error(&e), "os error 1224 must match");
}

/// §7.1 (Windows variant, negative): `is_file_in_use_error` does NOT match unrelated
/// OS errors. Guards against accidental over-widening of the gate.
#[cfg(windows)]
#[test]
fn is_file_in_use_error_rejects_unrelated_errors_on_windows() {
    use crate::commands::entity_creation::is_file_in_use_error;
    // ERROR_ACCESS_DENIED — separate failure mode, not file-in-use.
    let access_denied = std::io::Error::from_raw_os_error(5);
    assert!(
        !is_file_in_use_error(&access_denied),
        "os error 5 (ERROR_ACCESS_DENIED) must NOT match"
    );
    // ERROR_FILE_NOT_FOUND — not a file-in-use case.
    let not_found = std::io::Error::from_raw_os_error(2);
    assert!(
        !is_file_in_use_error(&not_found),
        "os error 2 (ERROR_FILE_NOT_FOUND) must NOT match"
    );
}

/// §7.1 (non-Windows variant): `is_file_in_use_error` always returns false off Windows.
#[cfg(not(windows))]
#[test]
fn is_file_in_use_error_no_op_on_non_windows() {
    use crate::commands::entity_creation::is_file_in_use_error;
    let e = std::io::Error::from_raw_os_error(32);
    assert!(
        !is_file_in_use_error(&e),
        "non-Windows must always return false"
    );
}

/// §7.22 (covers G.4.7): no valid workgroup name can collide with the
/// `BLOCKERS:` or `DIRTY_REPOS:` sentinel prefixes — `validate_existing_name`
/// rejects both `:` and `_`. Locks the wire-protocol invariant.
#[test]
fn workgroup_names_cannot_collide_with_sentinels() {
    use crate::commands::entity_creation::validate_existing_name;
    // Both prefixes contain ':' (BLOCKERS:) or '_' and ':' (DIRTY_REPOS:),
    // neither of which is in the validator's alphanumeric+'-' whitelist.
    assert!(validate_existing_name("BLOCKERS:foo", "Workgroup").is_err());
    assert!(validate_existing_name("DIRTY_REPOS:foo", "Workgroup").is_err());
    // Bare-prefix-without-colon would be alphanumeric and pass the validator,
    // but bare prefixes aren't sentinel hits (frontend uses startsWith with the colon).
    assert!(validate_existing_name("BLOCKERS", "Workgroup").is_ok());
    assert!(validate_existing_name("DIRTY-REPOS", "Workgroup").is_ok());
}

/// #113 follow-up: `collect_files_to_probe` must include the WG root
/// directory plus every `repo-*`, `__agent_*`, and `messaging/` top-level
/// subdir. Surfacing dir handles is what lets RM detect terminal-cwd /
/// IDE-workspace-open / file-watcher blockers, which file-only registration
/// missed.
#[cfg(windows)]
#[test]
fn collect_files_to_probe_includes_wg_root_and_top_level_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-1-test");
    std::fs::create_dir(&wg_dir).expect("create wg_dir");
    // Top-level dirs the diagnostic should now register.
    let repo_dir = wg_dir.join("repo-foo");
    let agent_dir = wg_dir.join("__agent_dev-rust");
    let messaging_dir = wg_dir.join("messaging");
    std::fs::create_dir(&repo_dir).expect("create repo-foo");
    std::fs::create_dir(&agent_dir).expect("create __agent_dev-rust");
    std::fs::create_dir(&messaging_dir).expect("create messaging");
    // A non-relevant top-level dir must NOT be registered (filter discipline).
    let unrelated = wg_dir.join("docs");
    std::fs::create_dir(&unrelated).expect("create docs");
    // A regular file at WG root — should still appear in the result, just
    // after the dirs.
    std::fs::write(wg_dir.join("TASK.md"), "# t\n").expect("write TASK.md");

    let result = collect_files_to_probe(&wg_dir);

    assert!(
        result.iter().any(|p| p == &wg_dir),
        "result must include WG root, got {:?}",
        result
    );
    assert!(
        result.iter().any(|p| p == &repo_dir),
        "result must include repo-* subdir, got {:?}",
        result
    );
    assert!(
        result.iter().any(|p| p == &agent_dir),
        "result must include __agent_* subdir, got {:?}",
        result
    );
    assert!(
        result.iter().any(|p| p == &messaging_dir),
        "result must include messaging/ subdir, got {:?}",
        result
    );
    assert!(
        !result.iter().any(|p| p == &unrelated),
        "result must NOT include unrelated top-level dirs (got 'docs'); result={:?}",
        result
    );
}

/// #113 follow-up: the dir entries must come BEFORE files in the output
/// so that, under the `MAX_FILES_TO_PROBE` cap, dirs are preferred. The
/// plan calls this out explicitly: dir handles are the new signal, file
/// handles were already covered.
#[cfg(windows)]
#[test]
fn collect_files_to_probe_orders_dirs_before_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-1-test");
    std::fs::create_dir(&wg_dir).expect("create wg_dir");
    let repo_dir = wg_dir.join("repo-foo");
    std::fs::create_dir(&repo_dir).expect("create repo-foo");
    std::fs::write(wg_dir.join("TASK.md"), "# t\n").expect("write TASK.md");
    std::fs::write(repo_dir.join("README.md"), "x").expect("write README.md");

    let result = collect_files_to_probe(&wg_dir);

    let first_file_idx = result.iter().position(|p| p.is_file());
    let last_dir_idx = result.iter().rposition(|p| p.is_dir());
    match (first_file_idx, last_dir_idx) {
        (Some(file_i), Some(dir_i)) => assert!(
            dir_i < file_i,
            "all dirs must precede all files in output; got dirs ending at {} but a file at {}; \
            result={:?}",
            dir_i,
            file_i,
            result
        ),
        // If there are no files (empty WG) or no dirs (would be a bug),
        // the test is moot — but it shouldn't happen with the setup above.
        other => panic!(
            "expected at least one dir and one file in result, got {:?}; result={:?}",
            other, result
        ),
    }
}

#[cfg(windows)]
#[test]
fn merge_blocker_processes_combines_rm_files_and_cwd_by_pid() {
    let merged = merge_blocker_processes(
        vec![BlockerProcess {
            pid: 123,
            name: "git.exe".into(),
            cwd: None,
            files: vec![r"C:\wg\repo\.git\index".into()],
        }],
        vec![BlockerProcess {
            pid: 123,
            name: "powershell.exe".into(),
            cwd: Some(r"C:\wg\repo".into()),
            files: Vec::new(),
        }],
    );

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].pid, 123);
    assert_eq!(merged[0].name, "git.exe");
    assert_eq!(merged[0].cwd.as_deref(), Some(r"C:\wg\repo"));
    assert_eq!(merged[0].files, vec![r"C:\wg\repo\.git\index"]);
}

#[cfg(windows)]
#[test]
fn path_is_under_windows_is_case_insensitive_prefix_aware_and_boundary_safe() {
    assert!(path_is_under_windows(
        Path::new(r"C:\Users\Maria\WG\repo-foo"),
        Path::new(r"c:\users\maria\wg")
    ));
    assert!(path_is_under_windows(
        Path::new(r"\??\C:\Users\Maria\WG\repo-foo"),
        Path::new(r"C:\Users\Maria\WG")
    ));
    assert!(path_is_under_windows(
        Path::new(r"\??\UNC\server\share\WG\repo-foo"),
        Path::new(r"\\server\share\WG")
    ));
    assert!(!path_is_under_windows(
        Path::new(r"C:\Users\Maria\WG2"),
        Path::new(r"C:\Users\Maria\WG")
    ));
}

#[cfg(windows)]
#[test]
fn current_process_cwd_blocker_detects_self_cwd_under_wg() {
    let blocker = current_process_cwd_blocker_from_parts(
        Path::new(r"C:\Users\Maria\WG"),
        4242,
        Path::new(r"C:\Users\Maria\WG\repo-foo"),
        Some(Path::new(
            r"C:\Program Files\AgentsCommander\AgentsCommander.exe",
        )),
    )
    .expect("current process cwd should be reported when it is under the workgroup");

    assert_eq!(blocker.pid, 4242);
    assert_eq!(blocker.name, "AgentsCommander.exe");
    assert_eq!(blocker.cwd.as_deref(), Some(r"C:\Users\Maria\WG\repo-foo"));
    assert!(blocker.files.is_empty());

    assert!(
        current_process_cwd_blocker_from_parts(
            Path::new(r"C:\Users\Maria\WG"),
            4242,
            Path::new(r"C:\Users\Maria\outside"),
            None,
        )
        .is_none(),
        "current process outside the workgroup must not be reported"
    );
}

#[cfg(windows)]
#[test]
fn scan_cwd_processes_windows_detects_child_process_current_dir() {
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-1-test");
    let repo_dir = wg_dir.join("repo-foo");
    std::fs::create_dir_all(&repo_dir).expect("create repo");

    let mut cmd = Command::new("powershell.exe");
    crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
    let child = cmd
        .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
        .current_dir(&repo_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn powershell blocker");
    let child_pid = child.id();
    let _child = ChildGuard(child);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut found = false;
    let mut last_error: Option<String> = None;
    while Instant::now() < deadline {
        match scan_cwd_processes_windows(&wg_dir) {
            Ok(blockers) => {
                found = blockers.iter().any(|p| {
                    p.pid == child_pid
                        && p.cwd
                            .as_deref()
                            .is_some_and(|cwd| path_is_under_windows(Path::new(cwd), &repo_dir))
                });
                if found {
                    break;
                }
            }
            Err(e) => {
                last_error = Some(e);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(
        found,
        "CWD fallback must detect child powershell.exe under WG; last_error={:?}",
        last_error
    );
}

#[cfg(windows)]
#[test]
fn room_lock_resources_small_budget_prefers_directories() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let room = tmp.path();
    let repo = room.join("repo-test");
    std::fs::create_dir(&repo).expect("repo");
    std::fs::write(repo.join("index.lock"), "hot").expect("hot");
    std::fs::write(room.join("TASK.md"), "cold").expect("cold");

    let result = crate::filesystem::room_diagnostics::collect_room_lock_resources(room, 2);

    assert_eq!(result, vec![room.to_path_buf(), repo]);
}
