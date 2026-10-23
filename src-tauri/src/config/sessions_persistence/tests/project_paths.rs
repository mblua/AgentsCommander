use super::*;

#[test]
fn filter_sessions_for_project_paths_drops_coordinator_and_non_coordinator_orphans() {
    let project_paths = vec!["C:/projects/current".to_string()];
    let sessions = vec![
        PersistedSession {
            last_prompt: None,
            name: "kept-coordinator".into(),
            working_directory: "C:/projects/current/.ac/wg-1/__agent_tech-lead".into(),
            is_coordinator: true,
            status: Some(SessionStatus::Running),
            ..Default::default()
        },
        PersistedSession {
            last_prompt: None,
            name: "orphan-coordinator".into(),
            working_directory: "C:/projects/removed/.ac/wg-1/__agent_tech-lead".into(),
            is_coordinator: true,
            status: Some(SessionStatus::Running),
            ..Default::default()
        },
        PersistedSession {
            last_prompt: None,
            name: "orphan-member".into(),
            working_directory: "C:/projects/removed/.ac/wg-1/__agent_dev-rust".into(),
            is_coordinator: false,
            status: Some(SessionStatus::Exited(0)),
            ..Default::default()
        },
    ];

    let filtered = filter_sessions_for_project_paths(sessions, &project_paths);

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "kept-coordinator");
}

#[test]
fn session_retention_project_paths_includes_archived_paths() {
    let settings = AppSettings {
        project_paths: vec!["A".to_string()],
        archived_project_paths: vec!["B".to_string()],
        ..AppSettings::default()
    };

    assert_eq!(
        session_retention_project_paths(&settings).unwrap(),
        vec!["A".to_string(), "B".to_string()]
    );
}

#[test]
fn session_retention_distinguishes_valid_empty_from_unavailable() {
    let mut settings = AppSettings::default();
    assert_eq!(session_retention_project_paths(&settings), Ok(Vec::new()));
    settings.project_paths = vec!["cached".into()];
    settings.project_paths_persistence_error = Some("project source unavailable".into());
    assert_eq!(
        session_retention_project_paths(&settings),
        Err("project source unavailable".into())
    );
}

#[test]
fn is_under_normalized_archived_roots_returns_false_for_empty_roots() {
    assert!(!is_under_normalized_archived_roots(
        "Z:/does/not/exist/.ac/wg-1/__agent_dev",
        &[]
    ));
}

#[test]
fn is_under_normalized_archived_roots_short_circuits_before_canonicalizing_path() {
    NORMALIZE_CALLS.with(|calls| calls.set(0));

    assert!(!is_under_normalized_archived_roots(
        "Z:/does/not/exist/x",
        &[]
    ));

    assert_eq!(
        NORMALIZE_CALLS.with(|calls| calls.get()),
        0,
        "empty archived roots must not canonicalize path"
    );

    NORMALIZE_CALLS.with(|calls| calls.set(0));
    assert!(!is_under_normalized_archived_roots(
        "Z:/does/not/exist/x",
        &["z:/somewhere".to_string()]
    ));
    assert_eq!(
        NORMALIZE_CALLS.with(|calls| calls.get()),
        1,
        "non-empty roots must canonicalize path exactly once"
    );
}

#[test]
fn is_under_normalized_archived_roots_matches_nested_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archived = temp.path().join("archived");
    let agent = archived.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&agent).expect("create archived agent");
    let roots = normalize_project_roots(&[archived.to_string_lossy().to_string()]);

    assert!(is_under_normalized_archived_roots(
        &agent.to_string_lossy(),
        &roots
    ));
}

#[test]
fn is_under_normalized_archived_roots_ignores_unnormalized_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archived = temp.path().join("archived");
    let agent = archived.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&agent).expect("create archived agent");
    let raw_root = archived.join(".").to_string_lossy().to_string();

    assert!(!is_under_normalized_archived_roots(
        &agent.to_string_lossy(),
        &[raw_root]
    ));
}

#[test]
fn normalize_project_roots_drops_blank_entries_and_normalizes_each() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("current");
    std::fs::create_dir_all(&project).expect("create project");
    let root_with_dot = project.join(".");
    let roots = normalize_project_roots(&[
        "".to_string(),
        "  ".to_string(),
        root_with_dot.to_string_lossy().to_string(),
    ]);

    assert_eq!(
        roots,
        vec![super::super::normalize_for_project_compare(
            project.as_path()
        )]
    );
}

#[test]
fn filter_sessions_for_project_paths_keeps_archived_when_called_with_retention_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let active = temp.path().join("active");
    let archived = temp.path().join("archived");
    let orphan = temp.path().join("orphan");
    let active_agent = active.join(".ac").join("wg-1").join("__agent_active");
    let archived_agent = archived.join(".ac").join("wg-1").join("__agent_archived");
    let orphan_agent = orphan.join(".ac").join("wg-1").join("__agent_orphan");
    std::fs::create_dir_all(&active_agent).expect("create active agent");
    std::fs::create_dir_all(&archived_agent).expect("create archived agent");
    std::fs::create_dir_all(&orphan_agent).expect("create orphan agent");
    let retention_paths = vec![
        active.to_string_lossy().to_string(),
        archived.to_string_lossy().to_string(),
    ];
    let sessions = vec![
        PersistedSession {
            name: "active".into(),
            working_directory: active_agent.to_string_lossy().to_string(),
            ..Default::default()
        },
        PersistedSession {
            name: "archived".into(),
            working_directory: archived_agent.to_string_lossy().to_string(),
            ..Default::default()
        },
        PersistedSession {
            name: "orphan".into(),
            working_directory: orphan_agent.to_string_lossy().to_string(),
            ..Default::default()
        },
    ];

    let filtered = filter_sessions_for_project_paths(sessions, &retention_paths);

    let names: Vec<&str> = filtered
        .iter()
        .map(|session| session.name.as_str())
        .collect();
    assert_eq!(names, vec!["active", "archived"]);
}

#[test]
fn filter_sessions_for_project_paths_matches_root_with_dot_segment() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("current");
    let agent = project.join(".ac").join("wg-1").join("__agent_keep");
    std::fs::create_dir_all(&agent).expect("create agent");
    let root_with_dot = project.join(".");
    let sessions = vec![PersistedSession {
        name: "keep".into(),
        working_directory: agent.to_string_lossy().to_string(),
        ..Default::default()
    }];

    let filtered =
        filter_sessions_for_project_paths(sessions, &[root_with_dot.to_string_lossy().to_string()]);

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "keep");
}

#[test]
fn filter_sessions_for_normalized_roots_does_not_normalize_its_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("current");
    let agent = project.join(".ac").join("wg-1").join("__agent_drop");
    std::fs::create_dir_all(&agent).expect("create agent");
    let raw_root = project.join(".").to_string_lossy().to_string();
    let sessions = vec![PersistedSession {
        name: "drop".into(),
        working_directory: agent.to_string_lossy().to_string(),
        ..Default::default()
    }];

    let filtered = filter_sessions_for_normalized_roots(sessions, &[raw_root]);

    assert!(filtered.is_empty());
}

#[test]
fn filter_sessions_for_project_paths_normalizes_its_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("current");
    let agent = project.join(".ac").join("wg-1").join("__agent_keep");
    std::fs::create_dir_all(&agent).expect("create agent");
    let raw_root = project.join(".").to_string_lossy().to_string();
    let sessions = vec![PersistedSession {
        name: "keep".into(),
        working_directory: agent.to_string_lossy().to_string(),
        ..Default::default()
    }];

    let filtered = filter_sessions_for_project_paths(sessions, &[raw_root]);

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "keep");
}

#[tokio::test]
async fn filter_sessions_for_project_paths_blocking_runs_off_the_calling_thread() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("current");
    let agent = project.join(".ac").join("wg-1").join("__agent_keep");
    std::fs::create_dir_all(&agent).expect("create agent");
    let calling_thread = std::thread::current().id();
    let slot = FILTER_PROJECT_PATHS_THREAD_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    slot.lock().expect("thread id mutex poisoned").clear();

    let filtered = filter_sessions_for_project_paths_blocking(
        vec![PersistedSession {
            name: "keep".into(),
            working_directory: agent.to_string_lossy().to_string(),
            ..Default::default()
        }],
        &[project.to_string_lossy().to_string()],
    )
    .await
    .expect("filter sessions");
    let recorded = slot.lock().expect("thread id mutex poisoned").clone();

    assert_eq!(filtered.len(), 1);
    assert!(!recorded.is_empty(), "filter thread id should be recorded");
    assert!(
        !recorded.contains(&calling_thread),
        "filter ran on the calling thread"
    );
}

#[tokio::test]
async fn filter_sessions_for_project_paths_blocking_surfaces_join_error() {
    let result = filter_sessions_for_project_paths_blocking(
        vec![PersistedSession {
            name: "__panic_filter_for_test__".into(),
            working_directory: "C:/projects/current/.ac/wg-1/__agent_dev".into(),
            ..Default::default()
        }],
        &["C:/projects/current".to_string()],
    )
    .await;

    let Err(err) = result else {
        panic!("worker panic must surface as JoinError");
    };
    assert!(err.contains("session filter task failed"), "{err}");
}

#[tokio::test]
async fn load_sessions_purging_outside_project_paths_returns_unfiltered_on_filter_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let keep = PersistedSession {
        name: "__panic_filter_for_test__".into(),
        working_directory: "C:/projects/current/.ac/wg-1/__agent_dev".into(),
        ..Default::default()
    };
    let other = PersistedSession {
        name: "other".into(),
        working_directory: "C:/projects/other/.ac/wg-1/__agent_other".into(),
        ..Default::default()
    };
    save_sessions_to_dir(temp.path(), &[keep.clone(), other.clone()]).expect("seed sessions");

    let loaded = load_sessions_purging_outside_project_paths_in_dir(
        temp.path(),
        &["C:/projects/current".to_string()],
        None,
    )
    .await;

    let names: Vec<&str> = loaded.iter().map(|session| session.name.as_str()).collect();
    assert_eq!(names, vec![keep.name.as_str(), other.name.as_str()]);
    let saved = load_sessions_raw_from_dir_for_test(temp.path());
    assert_eq!(saved.len(), 2);
}

#[tokio::test]
async fn purge_sessions_outside_project_paths_rewrites_sessions_json() {
    let temp = tempfile::tempdir().expect("tempdir");
    let current = temp.path().join("current");
    let removed = temp.path().join("removed");
    let current_agent = current.join(".ac").join("wg-1").join("__agent_keep");
    let removed_agent = removed.join(".ac").join("wg-1").join("__agent_old");
    std::fs::create_dir_all(&current_agent).expect("create current agent");
    std::fs::create_dir_all(&removed_agent).expect("create removed agent");

    let sessions = vec![
        PersistedSession {
            last_prompt: None,
            name: "keep".into(),
            working_directory: current_agent.to_string_lossy().to_string(),
            ..Default::default()
        },
        PersistedSession {
            last_prompt: None,
            name: "drop".into(),
            working_directory: removed_agent.to_string_lossy().to_string(),
            ..Default::default()
        },
    ];
    save_sessions_to_dir(temp.path(), &sessions).expect("seed sessions");

    let project_paths = vec![current.to_string_lossy().to_string()];
    let filtered = purge_sessions_outside_project_paths_in_dir(temp.path(), &project_paths)
        .await
        .expect("purge sessions");

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "keep");

    let saved = std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions");
    let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("parse sessions");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "keep");
}

#[tokio::test]
async fn purge_sessions_outside_project_paths_keeps_archived_session_when_retention_paths_used() {
    let temp = tempfile::tempdir().expect("tempdir");
    let active = temp.path().join("active");
    let archived = temp.path().join("archived");
    let orphan = temp.path().join("orphan");
    let active_agent = active.join(".ac").join("wg-1").join("__agent_active");
    let archived_agent = archived.join(".ac").join("wg-1").join("__agent_archived");
    let orphan_agent = orphan.join(".ac").join("wg-1").join("__agent_orphan");
    std::fs::create_dir_all(&active_agent).expect("create active agent");
    std::fs::create_dir_all(&archived_agent).expect("create archived agent");
    std::fs::create_dir_all(&orphan_agent).expect("create orphan agent");
    let sessions = vec![
        PersistedSession {
            name: "active".into(),
            working_directory: active_agent.to_string_lossy().to_string(),
            ..Default::default()
        },
        PersistedSession {
            name: "archived".into(),
            working_directory: archived_agent.to_string_lossy().to_string(),
            ..Default::default()
        },
        PersistedSession {
            name: "orphan".into(),
            working_directory: orphan_agent.to_string_lossy().to_string(),
            ..Default::default()
        },
    ];
    save_sessions_to_dir(temp.path(), &sessions).expect("seed sessions");
    let retention_paths = vec![
        active.to_string_lossy().to_string(),
        archived.to_string_lossy().to_string(),
    ];

    let filtered = purge_sessions_outside_project_paths_in_dir(temp.path(), &retention_paths)
        .await
        .expect("purge sessions");

    let names: Vec<&str> = filtered
        .iter()
        .map(|session| session.name.as_str())
        .collect();
    assert_eq!(names, vec!["active", "archived"]);
    let saved = std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions");
    let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("parse sessions");
    let saved_names: Vec<&str> = rows.iter().map(|session| session.name.as_str()).collect();
    assert_eq!(saved_names, vec!["active", "archived"]);
}

/// #698 grinch HIGH regression — the orphan purge must take
/// `sessions_save_lock()` and re-read `sessions.json` INSIDE that lock, so it
/// can never overwrite a raise-hand that another locked writer persisted
/// after the purge began. We drive the exact interleaving deterministically:
///   1. seed A (retained, no communication) + B (removable),
///   2. hold the save lock and start the purge; it must park at the lock
///      WITHOUT having rewritten the file (B still on disk = no stale
///      pre-read followed by a stale write),
///   3. while the lock is held, overwrite the file with A carrying a visible
///      raiseHand (what a raise-hand persist would have produced),
///   4. release the lock; the purge re-reads the fresh state, keeps A, drops
///      B, and the persisted raiseHand SURVIVES.
///
/// The pre-fix code (load+save outside the lock) would have written its stale
/// pre-read here, dropping A's raiseHand. Note `save_sessions_to_dir` (step 3)
/// takes only the sync `SAVE_SESSIONS_LOCK`, never this async lock, so seeding
/// the file while holding `guard` cannot deadlock.
#[tokio::test]
async fn purge_reads_under_save_lock_and_preserves_concurrent_raise_hand() {
    let temp = tempfile::tempdir().expect("tempdir");
    let current = temp.path().join("current");
    let removed = temp.path().join("removed");
    let current_agent = current.join(".ac").join("wg-1").join("__agent_keep");
    let removed_agent = removed.join(".ac").join("wg-1").join("__agent_old");
    std::fs::create_dir_all(&current_agent).expect("create current agent");
    std::fs::create_dir_all(&removed_agent).expect("create removed agent");

    let kept = PersistedSession {
        name: "keep".into(),
        working_directory: current_agent.to_string_lossy().to_string(),
        is_coordinator: true,
        status: Some(SessionStatus::Running),
        ..Default::default()
    };
    let removable = PersistedSession {
        name: "drop".into(),
        working_directory: removed_agent.to_string_lossy().to_string(),
        ..Default::default()
    };
    save_sessions_to_dir(temp.path(), &[kept.clone(), removable.clone()]).expect("seed");

    let project_paths = vec![current.to_string_lossy().to_string()];

    // Hold the save lock so the purge cannot proceed past its lock acquisition.
    let guard = sessions_save_lock().lock().await;
    let mut purge = Box::pin(purge_sessions_outside_project_paths_in_dir(
        temp.path(),
        &project_paths,
    ));
    let timed_out = tokio::time::timeout(Duration::from_millis(25), &mut purge)
        .await
        .is_err();
    assert!(
        timed_out,
        "purge must wait for the save lock before reading or writing"
    );

    // The purge has NOT rewritten the file yet (it has not even read it):
    // the removable session is still present on disk.
    let parked: Vec<PersistedSession> = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("sessions.json")).expect("read parked"),
    )
    .expect("parse parked");
    assert_eq!(
        parked.len(),
        2,
        "purge must not write while parked at the lock"
    );

    // Simulate a raise-hand that persisted A with a visible raiseHand while
    // the purge was parked.
    let mut raised = kept.clone();
    raised.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::RaiseHand,
        visible: true,
        updated_at: "2026-06-30T13:00:00+00:00".into(),
        message: None,
    });
    save_sessions_to_dir(temp.path(), &[raised, removable]).expect("persist raise-hand");

    drop(guard);
    let filtered = purge.await.expect("purge should succeed");

    // The purge kept A, dropped B, and preserved the raise-hand it re-read
    // under the lock.
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "keep");
    let saved: Vec<PersistedSession> = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("sessions.json")).expect("read final"),
    )
    .expect("parse final");
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].name, "keep");
    let comm = saved[0]
        .communication
        .as_ref()
        .expect("raise-hand must survive the purge");
    assert_eq!(comm.kind, SessionCommunicationKind::RaiseHand);
    assert!(comm.visible);
}

#[test]
fn project_path_comparison_is_boundary_safe() {
    let project_paths = vec!["C:/repo/foo".to_string()];

    assert!(working_directory_under_any_project_path(
        "C:/repo/foo/.ac/wg-1/__agent_a",
        &project_paths
    ));
    assert!(!working_directory_under_any_project_path(
        "C:/repo/foobar/.ac/wg-1/__agent_a",
        &project_paths
    ));
}

#[cfg(windows)]
#[test]
fn project_path_comparison_is_case_insensitive_on_windows() {
    let project_paths = vec![r"C:\Users\Maria\Project".to_string()];

    assert!(working_directory_under_any_project_path(
        r"c:\users\maria\project\.ac\wg-1\__agent_a",
        &project_paths
    ));
}

#[cfg(windows)]
#[test]
fn project_path_comparison_handles_windows_verbatim_cwd() {
    let project_paths = vec![r"C:\Users\Maria\Project".to_string()];

    assert!(working_directory_under_any_project_path(
        r"\\?\C:\Users\Maria\Project\.ac\wg-1\__agent_a",
        &project_paths
    ));
}

#[cfg(windows)]
#[test]
fn deduplicate_treats_windows_verbatim_and_ordinary_cwd_as_same_key() {
    let rows = vec![
        PersistedSession {
            name: "verbatim".to_string(),
            working_directory: r"\\?\C:\repo\.ac\wg-1\__agent_a".to_string(),
            was_active: false,
            ..Default::default()
        },
        PersistedSession {
            name: "ordinary".to_string(),
            working_directory: r"C:\repo\.ac\wg-1\__agent_a".to_string(),
            was_active: true,
            ..Default::default()
        },
    ];

    let deduped = deduplicate(rows);

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].name, "ordinary");
}

#[cfg(windows)]
#[test]
fn load_sessions_from_path_normalizes_windows_verbatim_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("sessions.json");
    let sessions = serde_json::json!([
        {
            "name": "wg-1/a",
            "shell": "cmd",
            "shellArgs": [],
            "workingDirectory": r"\\?\C:\repo\.ac\wg-1\__agent_a",
            "gitRepos": [
                {
                    "label": "repo",
                    "sourcePath": r"\\?\UNC\server\share\repo",
                    "branch": null
                }
            ]
        }
    ]);
    std::fs::write(&path, sessions.to_string()).expect("write sessions");

    let loaded = load_sessions_from_path(&path);

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].working_directory, r"C:\repo\.ac\wg-1\__agent_a");
    assert_eq!(loaded[0].git_repos[0].source_path, r"\\server\share\repo");
}
