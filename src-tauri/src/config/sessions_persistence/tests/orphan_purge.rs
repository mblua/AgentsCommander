use super::*;

// ──────────────────────────────────────────────────────────────────────
// §1295 tests: purge persist prune / archive / dedup / gate
// ──────────────────────────────────────────────────────────────────────

/// AC1 loop-regression anchor (test 1): a nonexistent-cwd outside-roots row
/// is reaped exactly once, recorded (B1), and a SECOND purge is byte-
/// identical with zero WARN movement and frozen counters.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn purge_reaps_nonexistent_cwd_outside_roots_once_and_is_byte_idempotent() {
    let _serial = ORPHAN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_orphan_counters();
    reset_orphan_warned();
    let temp = tempfile::tempdir().expect("tempdir");
    let orphan_agent = temp
        .path()
        .join("missing-project")
        .join("wg-1")
        .join("__agent_old");
    let ps = PersistedSession {
        name: "orphan".into(),
        working_directory: orphan_agent.to_string_lossy().to_string(),
        ..Default::default()
    };
    save_sessions_to_dir(temp.path(), &[ps]).expect("seed sessions");
    let file_path = temp.path().join("sessions.json");
    let before_bytes = std::fs::read(&file_path).expect("read seed");

    let project_paths = vec![temp.path().join("current").to_string_lossy().to_string()];
    let filtered = purge_sessions_outside_project_paths_in_dir(temp.path(), &project_paths)
        .await
        .expect("purge");
    assert!(filtered.is_empty(), "kept set must drop the orphan");
    assert!(
        load_sessions_raw_from_dir_for_test(temp.path()).is_empty(),
        "orphan removed from sessions.json"
    );

    // ONE archive record, disposition=reaped, reason=outsideRetainedRootsMissing.
    let archive =
        std::fs::read_to_string(temp.path().join(ORPHAN_ARCHIVE_FILENAME)).expect("read archive");
    let lines: Vec<&str> = archive.lines().collect();
    assert_eq!(lines.len(), 1, "B1: exactly one archive record");
    let record: serde_json::Value = serde_json::from_str(lines[0]).expect("parse record");
    assert_eq!(record["schemaVersion"], 1);
    assert_eq!(record["reason"], "outsideRetainedRootsMissing");
    assert_eq!(record["disposition"], "reaped");
    assert_eq!(record["session"]["name"], "orphan");
    let after_bytes = std::fs::read(&file_path).expect("read rewritten");
    assert_ne!(
        before_bytes, after_bytes,
        "first pass rewrites sessions.json"
    );
    let first_counts = orphan_counters();
    assert_eq!(first_counts.archived, 0);
    assert_eq!(first_counts.reaped, 1);
    assert_eq!(orphan_warned_len(), 1);

    // Second pass: byte-identical sessions.json, no new archive record, no
    // counter movement, no new WARN.
    let after_first_bytes = std::fs::read(&file_path).expect("read after first");
    let filtered2 = purge_sessions_outside_project_paths_in_dir(temp.path(), &project_paths)
        .await
        .expect("second purge");
    assert!(filtered2.is_empty());
    let after_second_bytes = std::fs::read(&file_path).expect("read after second");
    assert_eq!(
        after_first_bytes, after_second_bytes,
        "AC1: second purge byte-identical"
    );
    let second_counts = orphan_counters();
    assert_eq!(second_counts.archived, 0);
    assert_eq!(second_counts.reaped, 1, "counters frozen on second pass");
    assert_eq!(orphan_warned_len(), 1, "no repeat WARN (run-level dedup)");
}

/// Test 2: an EXISTING-cwd outside-roots row is archived (not reaped) with a
/// round-trippable session blob; a second purge is clean and byte-identical.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn purge_archives_existing_cwd_outside_roots() {
    let _serial = ORPHAN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_orphan_counters();
    reset_orphan_warned();
    let temp = tempfile::tempdir().expect("tempdir");
    let orphan_agent = temp.path().join("other").join("wg-1").join("__agent_old");
    std::fs::create_dir_all(&orphan_agent).expect("create orphan agent");
    let ps = PersistedSession {
        name: "orphan".into(),
        working_directory: orphan_agent.to_string_lossy().to_string(),
        ..Default::default()
    };
    save_sessions_to_dir(temp.path(), &[ps]).expect("seed sessions");
    let project_paths = vec![temp.path().join("current").to_string_lossy().to_string()];

    let filtered = purge_sessions_outside_project_paths_in_dir(temp.path(), &project_paths)
        .await
        .expect("purge");
    assert!(filtered.is_empty());
    assert!(load_sessions_raw_from_dir_for_test(temp.path()).is_empty());
    let archive =
        std::fs::read_to_string(temp.path().join(ORPHAN_ARCHIVE_FILENAME)).expect("read archive");
    let lines: Vec<&str> = archive.lines().collect();
    assert_eq!(lines.len(), 1);
    let record: serde_json::Value = serde_json::from_str(lines[0]).expect("parse");
    assert_eq!(record["reason"], "outsideRetainedRoots");
    assert_eq!(record["disposition"], "archived");
    assert_eq!(record["session"]["name"], "orphan");
    let blob: PersistedSession =
        serde_json::from_value(record["session"].clone()).expect("session blob round-trip");
    assert_eq!(
        blob.working_directory,
        orphan_agent.to_string_lossy().to_string()
    );

    // Second purge clean + byte-identical.
    let b1 = std::fs::read(temp.path().join("sessions.json")).expect("read b1");
    let filtered2 = purge_sessions_outside_project_paths_in_dir(temp.path(), &project_paths)
        .await
        .expect("second purge");
    assert!(filtered2.is_empty());
    let b2 = std::fs::read(temp.path().join("sessions.json")).expect("read b2");
    assert_eq!(b1, b2, "second purge byte-identical");
}

/// Test 3: root-agent and archived-root rows survive both purge cycles and
/// produce NO archive records.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn purge_keeps_root_agent_and_archived_root_rows() {
    let _serial = ORPHAN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_orphan_counters();
    reset_orphan_warned();
    let temp = tempfile::tempdir().expect("tempdir");
    let current = temp.path().join("current");
    let archived = temp.path().join("archived");
    std::fs::create_dir_all(&current).expect("create current");
    std::fs::create_dir_all(&archived).expect("create archived");
    let root_agent = temp.path().join("root-agent-x");
    let root_row = PersistedSession {
        name: "root".into(),
        working_directory: root_agent.to_string_lossy().to_string(),
        is_root_agent: true,
        ..Default::default()
    };
    let archived_agent = archived.join("wg-1").join("__agent_archived");
    std::fs::create_dir_all(&archived_agent).expect("create archived agent");
    let arch_row = PersistedSession {
        name: "archived".into(),
        working_directory: archived_agent.to_string_lossy().to_string(),
        ..Default::default()
    };
    save_sessions_to_dir(temp.path(), &[root_row.clone(), arch_row.clone()]).expect("seed");
    let retention = vec![
        current.to_string_lossy().to_string(),
        archived.to_string_lossy().to_string(),
    ];

    let filtered = purge_sessions_outside_project_paths_in_dir(temp.path(), &retention)
        .await
        .expect("purge");
    let names: Vec<&str> = filtered.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["root", "archived"]);
    assert!(
        !temp.path().join(ORPHAN_ARCHIVE_FILENAME).exists(),
        "no archive records for kept rows"
    );
    let filtered2 = purge_sessions_outside_project_paths_in_dir(temp.path(), &retention)
        .await
        .expect("second purge");
    assert_eq!(filtered2.len(), 2, "both rows survive both cycles");
}

/// AC3 (test 4): a dormant orphan leaves the manager + disk on the first
/// persist and is recorded; a live orphan keeps running in RAM, is dropped
/// from disk, counts live_kept, and is NOT recorded (its recipe is still
/// live). A second persist is silent with frozen counters.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn persist_prunes_dormant_orphan_keeps_live_orphan() {
    let _serial = ORPHAN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_orphan_counters();
    reset_orphan_warned();
    let temp = tempfile::tempdir().expect("tempdir");
    let current = temp.path().join("current");
    std::fs::create_dir_all(&current).expect("create current");
    let other = temp.path().join("other");
    let dormant_cwd = other
        .join("wg-1")
        .join("__agent_dormant")
        .to_string_lossy()
        .to_string();
    let live_cwd = other
        .join("wg-1")
        .join("__agent_live")
        .to_string_lossy()
        .to_string();
    let mgr = SessionManager::new();
    let dormant = mgr
        .create_session(
            "dormant".into(),
            vec![],
            dormant_cwd.clone(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create dormant");
    let live = mgr
        .create_session(
            "live".into(),
            vec![],
            live_cwd.clone(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create live");
    mgr.mark_exited(dormant.id, 0).await;

    let retention = vec![current.to_string_lossy().to_string()];
    persist_current_state_to_dir_for_project_paths_result(
        &mgr,
        temp.path(),
        Some(&retention),
        PersistMode::PruneDormant,
    )
    .await
    .expect("persist");

    // (create_session auto-names rows "Session N"; assert by id, not name.)
    let dormant_id = dormant.id.to_string();
    let live_id = live.id.to_string();
    let rows = mgr.list_sessions().await;
    assert!(
        !rows.iter().any(|s| s.id == dormant_id),
        "dormant orphan removed from manager"
    );
    assert!(
        rows.iter().any(|s| s.id == live_id),
        "live orphan keeps running in manager"
    );
    assert!(
        load_sessions_raw_from_dir_for_test(temp.path()).is_empty(),
        "both orphans dropped from the disk snapshot"
    );
    // ONE archive record (only the dormant one; the live one is kept live).
    let archive =
        std::fs::read_to_string(temp.path().join(ORPHAN_ARCHIVE_FILENAME)).expect("archive");
    assert_eq!(
        archive.lines().count(),
        1,
        "live orphan is not recorded (B1 applies to dropped rows)"
    );
    let record: serde_json::Value = serde_json::from_str(archive.lines().next().unwrap()).unwrap();
    assert_eq!(
        record["session"]["name"], "Session 1",
        "the dormant row is the recorded one"
    );
    let first = orphan_counters();
    assert_eq!(first.reaped, 1);
    assert_eq!(first.archived, 0);
    assert_eq!(first.live_kept, 1);
    let warned_after_first = orphan_warned_len();
    assert_eq!(
        warned_after_first, 2,
        "two distinct orphan cwds warn once each"
    );

    // Second persist: silent (no archive growth, counters frozen, no new warns).
    let archive_after_first =
        std::fs::read_to_string(temp.path().join(ORPHAN_ARCHIVE_FILENAME)).expect("archive");
    persist_current_state_to_dir_for_project_paths_result(
        &mgr,
        temp.path(),
        Some(&retention),
        PersistMode::PruneDormant,
    )
    .await
    .expect("second persist");
    let archive_after_second =
        std::fs::read_to_string(temp.path().join(ORPHAN_ARCHIVE_FILENAME)).expect("archive");
    assert_eq!(
        archive_after_first, archive_after_second,
        "no archive growth on repeat"
    );
    let second = orphan_counters();
    assert_eq!(second.reaped, 1, "no NEW reaping on the repeat sweep");
    assert_eq!(
        second.live_kept, 2,
        "the persistent live orphan is re-seen and kept live"
    );
    assert_eq!(
        orphan_warned_len(),
        warned_after_first,
        "no new WARN on repeat sweep"
    );
}

/// Test 5: a persist with nothing orphaned emits no archive file and moves
/// no counters.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn persist_is_silent_when_nothing_orphaned() {
    let _serial = ORPHAN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_orphan_counters();
    reset_orphan_warned();
    let temp = tempfile::tempdir().expect("tempdir");
    let current = temp.path().join("current");
    let in_root = current.join("wg-1").join("__agent_keep");
    std::fs::create_dir_all(&in_root).expect("create in-root agent");
    let mgr = SessionManager::new();
    mgr.create_session(
        "keep".into(),
        vec![],
        in_root.to_string_lossy().to_string(),
        None,
        None,
        Vec::new(),
        false,
        crate::pty::backend::SessionBackendKind::LocalProcess,
    )
    .await
    .expect("create kept session");
    let retention = vec![current.to_string_lossy().to_string()];
    persist_current_state_to_dir_for_project_paths_result(
        &mgr,
        temp.path(),
        Some(&retention),
        PersistMode::NoPrune,
    )
    .await
    .expect("persist");
    assert!(
        !temp.path().join(ORPHAN_ARCHIVE_FILENAME).exists(),
        "no archive file when nothing orphaned"
    );
    let saved = load_sessions_raw_from_dir_for_test(temp.path());
    assert_eq!(saved.len(), 1);
    assert_eq!(orphan_warned_len(), 0);
    assert_eq!(
        orphan_counters(),
        super::super::OrphanSweepCounts::default()
    );
}

/// Test 6 (dispatch req): the transition border. A NO-PRUNE (transitional)
/// persist keeps a dormant orphan in RAM (transition-owned) while archiving
/// it once and dropping it from disk; the subsequent STEADY PruneDormant
/// persist removes it from RAM but does NOT re-append the archive record
/// (B1-once first-sighting tie). warned stays 1 across the boundary.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn transition_border_no_prune_keeps_then_steady_prune_removes() {
    let _serial = ORPHAN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_orphan_counters();
    reset_orphan_warned();
    reset_orphan_archived();
    let temp = tempfile::tempdir().expect("tempdir");
    let current = temp.path().join("current");
    std::fs::create_dir_all(&current).expect("create current");
    let orphan_cwd = temp
        .path()
        .join("other")
        .join("wg-1")
        .join("__agent_border")
        .to_string_lossy()
        .to_string();
    let mgr = SessionManager::new();
    let row = mgr
        .create_session(
            "border".into(),
            vec![],
            orphan_cwd.clone(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create orphan row");
    mgr.mark_exited(row.id, 0).await;
    mgr.set_active_only(row.id).await.expect("select row");
    let id_str = row.id.to_string();

    let retention = vec![current.to_string_lossy().to_string()];

    // No-prune (transitional) persist: row stays in RAM, archived once,
    // dropped from disk.
    persist_current_state_to_dir_for_project_paths_result(
        &mgr,
        temp.path(),
        Some(&retention),
        PersistMode::NoPrune,
    )
    .await
    .expect("no-prune persist");
    let rows = mgr.list_sessions().await;
    assert!(
        rows.iter().any(|s| s.id == id_str),
        "no-prune persist keeps the dormant row in RAM"
    );
    assert_eq!(mgr.selection_payload().await.id(), Some(row.id));
    let archive =
        std::fs::read_to_string(temp.path().join(ORPHAN_ARCHIVE_FILENAME)).expect("archive");
    assert_eq!(archive.lines().count(), 1, "no-prune persist archives once");
    assert!(
        load_sessions_raw_from_dir_for_test(temp.path()).is_empty(),
        "no-prune persist drops the row from disk"
    );
    let counts_no_prune = orphan_counters();
    assert_eq!(
        counts_no_prune.live_kept, 0,
        "a dormant row is archived, not live_kept"
    );
    assert_eq!(counts_no_prune.reaped + counts_no_prune.archived, 1);
    let warned = orphan_warned_len();
    assert_eq!(warned, 1);

    // Steady prune: row leaves RAM; archive NOT re-appended (B1-once tie).
    persist_current_state_to_dir_for_project_paths_result(
        &mgr,
        temp.path(),
        Some(&retention),
        PersistMode::PruneDormant,
    )
    .await
    .expect("steady prune");
    let rows = mgr.list_sessions().await;
    assert!(
        !rows.iter().any(|s| s.id == id_str),
        "steady prune removes the dormant row from RAM"
    );
    let archive_after =
        std::fs::read_to_string(temp.path().join(ORPHAN_ARCHIVE_FILENAME)).expect("archive");
    assert_eq!(
        archive_after.lines().count(),
        1,
        "B1-once: archive not re-appended on the steady prune"
    );
    assert_eq!(
        orphan_warned_len(),
        warned,
        "no new WARN from the steady prune"
    );
    let counts_prune = orphan_counters();
    assert_eq!(
        counts_prune.reaped + counts_prune.archived,
        1,
        "exactly one archive total across the boundary"
    );
    assert!(
        counts_prune.suppressed_repeat >= 1,
        "the repeat cwd sighting is suppressed"
    );
    assert_eq!(mgr.get_session(row.id).await.map(|s| s.name), None);
}

/// N4c (test 6): run purge (site A) then persist (site B) in one process for
/// the SAME orphan cwd; the run-level registry collapses the second sighting
/// (warned stays 1, suppressed_repeat becomes 1).
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn cross_site_dedup_purge_then_persist_warns_once() {
    let _serial = ORPHAN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_orphan_counters();
    reset_orphan_warned();
    let temp = tempfile::tempdir().expect("tempdir");
    let current = temp.path().join("current");
    std::fs::create_dir_all(&current).expect("create current");
    let orphan_cwd = temp
        .path()
        .join("other")
        .join("wg-1")
        .join("__agent_old")
        .to_string_lossy()
        .to_string();
    let retention = vec![current.to_string_lossy().to_string()];

    // Disk row (site A).
    let disk_ps = PersistedSession {
        name: "disk-orphan".into(),
        working_directory: orphan_cwd.clone(),
        ..Default::default()
    };
    save_sessions_to_dir(temp.path(), &[disk_ps]).expect("seed disk row");
    let filtered = purge_sessions_outside_project_paths_in_dir(temp.path(), &retention)
        .await
        .expect("purge");
    assert!(filtered.is_empty());
    assert_eq!(orphan_warned_len(), 1, "first sighting warns");

    // Manager row (site B) with the same cwd.
    let mgr = SessionManager::new();
    mgr.create_session(
        "manager-orphan".into(),
        vec![],
        orphan_cwd,
        None,
        None,
        Vec::new(),
        false,
        crate::pty::backend::SessionBackendKind::LocalProcess,
    )
    .await
    .expect("create manager row");
    persist_current_state_to_dir_for_project_paths_result(
        &mgr,
        temp.path(),
        Some(&retention),
        PersistMode::NoPrune,
    )
    .await
    .expect("persist");

    assert_eq!(
        orphan_warned_len(),
        1,
        "N1: same orphan warns exactly once per run"
    );
    let counts = orphan_counters();
    assert_eq!(
        counts.suppressed_repeat, 1,
        "second sighting suppressed across sites"
    );
}

/// N4a (test 7): the site-C append helper writes the restoreCwdMissing
/// record with the locking variant and leaves sessions.json untouched.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn site_c_record_reason_restore_cwd_missing() {
    let _serial = ORPHAN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let ps = PersistedSession {
        name: "restore-skip".into(),
        working_directory: "C:/gone/project/.ac/wg-1/__agent_x".to_string(),
        ..Default::default()
    };
    let seed = PersistedSession {
        name: "keep".into(),
        working_directory: "C:/here/project".to_string(),
        ..Default::default()
    };
    save_sessions_to_dir(temp.path(), &[seed]).expect("seed");
    let before_bytes = std::fs::read(temp.path().join("sessions.json")).expect("read before");

    append_orphan_archive_record(temp.path(), "restoreCwdMissing", "archived", &ps).await;

    // sessions.json byte-identical (site C leaves the row's disk fate alone).
    let after_bytes = std::fs::read(temp.path().join("sessions.json")).expect("read after");
    assert_eq!(
        before_bytes, after_bytes,
        "site C does not touch sessions.json"
    );
    let archive =
        std::fs::read_to_string(temp.path().join(ORPHAN_ARCHIVE_FILENAME)).expect("archive");
    let lines: Vec<&str> = archive.lines().collect();
    assert_eq!(lines.len(), 1);
    let record: serde_json::Value = serde_json::from_str(lines[0]).expect("parse");
    assert_eq!(record["reason"], "restoreCwdMissing");
    assert_eq!(record["disposition"], "archived");
    assert_eq!(record["session"]["name"], "restore-skip");
}

/// N4e/N5 (test 8): append below cap, then with a tiny `#[cfg(test)]` cap
/// force rotations so `.1/.2/.3` shift and nothing beyond `.3` is kept;
/// appends still work after rotation. Best-effort: a held-open append
/// handle during rotation must not panic.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn archive_appends_and_rotates_best_effort() {
    let _serial = ORPHAN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    let ps = PersistedSession {
        name: "rot".into(),
        working_directory: "C:/x".into(),
        ..Default::default()
    };
    set_test_orphan_archive_cap(Some(200));
    for _ in 0..40 {
        append_orphan_archive_record(&dir, "outsideRetainedRoots", "archived", &ps).await;
    }
    set_test_orphan_archive_cap(None);

    let base = dir.join(ORPHAN_ARCHIVE_FILENAME);
    let one = dir.join(format!("{}.1", ORPHAN_ARCHIVE_FILENAME));
    let four = dir.join(format!("{}.4", ORPHAN_ARCHIVE_FILENAME));
    assert!(base.exists(), "active archive file exists");
    assert!(one.exists(), ".1 rotated");
    // KEEP=3: at most .1/.2/.3 retained; a .4 must never appear.
    assert!(!four.exists(), "ORPHAN_ARCHIVE_KEEP=3: .4 must not exist");
    // With the cap restored, an append lands in the active file (no rotation).
    append_orphan_archive_record(&dir, "outsideRetainedRoots", "archived", &ps).await;
    assert!(
        std::fs::metadata(&base)
            .map(|m| m.len() > 0)
            .unwrap_or(false),
        "append still works after rotation"
    );

    // Best-effort rotation with a held handle (Windows sharing-violation
    // simulation): must not panic, and appends still work after release.
    set_test_orphan_archive_cap(Some(200));
    let handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&base)
        .expect("hold archive handle");
    append_orphan_archive_record(&dir, "outsideRetainedRoots", "archived", &ps).await;
    drop(handle);
    append_orphan_archive_record(&dir, "outsideRetainedRoots", "archived", &ps).await;
    assert!(base.exists());
    set_test_orphan_archive_cap(None);
}
