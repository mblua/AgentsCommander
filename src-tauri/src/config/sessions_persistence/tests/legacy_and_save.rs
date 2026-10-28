use super::*;

/// Validation #17: single-repo legacy → one SessionRepo; legacy fields cleared.
#[test]
fn legacy_migration_single_repo_shape() {
    let mut ps = PersistedSession {
        last_prompt: None,
        name: "sess-a".into(),
        shell: "cmd".into(),
        shell_args: vec![],
        working_directory: "C:/x".into(),
        was_active: false,
        git_repos: vec![],
        is_coordinator: false,
        is_root_agent: false,
        agent_id: None,
        agent_label: None,
        requested_profile: None,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        git_branch_source: Some("C:/repos/agentscommander".into()),
        git_branch_prefix: Some("agentscommander".into()),
        id: None,
        status: None,
        waiting_for_input: None,
        created_at: None,
        ..Default::default()
    };

    // Mimic the upgrade pass in load_sessions (single-repo branch).
    if ps.git_repos.is_empty() {
        match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
            (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                ps.git_repos.push(crate::session::session::SessionRepo {
                    label: prefix,
                    source_path: source,
                    branch: None,
                    dirty: None,
                });
            }
            _ => {}
        }
    }

    assert_eq!(ps.git_repos.len(), 1);
    assert_eq!(ps.git_repos[0].label, "agentscommander");
    assert_eq!(ps.git_repos[0].source_path, "C:/repos/agentscommander");
    assert!(ps.git_branch_source.is_none());
    assert!(ps.git_branch_prefix.is_none());
}

/// Issue #248 — status round-trips through serialize/deserialize.
/// Locks the field against future "ignored on restore" misreads now that
/// `should_wake_on_restore` in `lib.rs` consumes it.
#[test]
fn issue_248_status_round_trips_through_persistence() {
    let cases = [SessionStatus::Exited(0), SessionStatus::Running];
    for status in cases {
        let ps = PersistedSession {
            last_prompt: None,
            name: "coord-x".into(),
            shell: "claude".into(),
            shell_args: vec![],
            working_directory: "C:/proj/.ac/_agent_architect".into(),
            was_active: true,
            git_repos: vec![],
            is_coordinator: true,
            is_root_agent: false,
            agent_id: Some("aid-arch".into()),
            agent_label: Some("Architect".into()),
            requested_profile: None,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            git_branch_source: None,
            git_branch_prefix: None,
            id: Some("uuid".into()),
            status: Some(status.clone()),
            waiting_for_input: Some(false),
            created_at: Some("2026-05-17T00:00:00Z".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&ps).expect("serialize");
        let back: PersistedSession = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.status, Some(status));
    }
}

#[test]
fn communication_round_trips_when_present() {
    let ps = PersistedSession {
        name: "coord-x".into(),
        shell: "claude".into(),
        shell_args: vec![],
        working_directory: "C:/proj/.ac/wg-1-dev-team/__agent_tech-lead".into(),
        communication: Some(SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: "2026-06-30T11:00:00+00:00".into(),
            message: None,
        }),
        ..Default::default()
    };

    let json = serde_json::to_value(&ps).expect("serialize");
    assert_eq!(json["communication"]["kind"], "raiseHand");
    assert_eq!(json["communication"]["visible"], true);
    let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back.communication, ps.communication);
}

#[test]
fn is_root_agent_defaults_false_for_legacy_json() {
    let json = r#"{
        "name": "legacy",
        "shell": "cmd",
        "shellArgs": [],
        "workingDirectory": "C:/x"
    }"#;

    let back: PersistedSession = serde_json::from_str(json).expect("deserialize");

    assert!(!back.is_root_agent);
}

#[test]
fn is_root_agent_round_trips_true() {
    let ps = PersistedSession {
        last_prompt: None,
        name: "Root Agent".into(),
        shell: "codex".into(),
        shell_args: vec![],
        working_directory: "C:/tools/.agentscommander/ac-root-agent".into(),
        is_root_agent: true,
        ..Default::default()
    };

    let json = serde_json::to_value(&ps).expect("serialize");
    assert_eq!(json["isRootAgent"], true);
    let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
    assert!(back.is_root_agent);
}

/// Legacy "multi-repo" prefix → git_repos stays empty; legacy fields cleared.
#[test]
fn legacy_migration_multi_repo_shape() {
    let mut ps = PersistedSession {
        last_prompt: None,
        name: "sess-multi".into(),
        shell: "cmd".into(),
        shell_args: vec![],
        working_directory: "C:/x".into(),
        was_active: false,
        git_repos: vec![],
        is_coordinator: false,
        is_root_agent: false,
        agent_id: None,
        agent_label: None,
        requested_profile: None,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        git_branch_source: None,
        git_branch_prefix: Some("multi-repo".into()),
        id: None,
        status: None,
        waiting_for_input: None,
        created_at: None,
        ..Default::default()
    };

    if ps.git_repos.is_empty() {
        match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
            (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                ps.git_repos.push(crate::session::session::SessionRepo {
                    label: prefix,
                    source_path: source,
                    branch: None,
                    dirty: None,
                });
            }
            _ => {}
        }
    }

    assert!(ps.git_repos.is_empty());
    assert!(ps.git_branch_source.is_none());
    assert!(ps.git_branch_prefix.is_none());
}

/// #280 §3.1 — happy path: rename a real tmp file over a real dst.
/// Must succeed on the first attempt (no INFO emission, no diagnostic
/// context returned).
#[test]
fn rename_with_retry_succeeds_on_first_attempt() {
    let tmp = tempfile::tempdir().expect("tmp");
    let src = tmp.path().join("a.tmp");
    let dst = tmp.path().join("a");
    std::fs::write(&src, b"payload").expect("seed src");
    assert!(rename_with_retry(&src, &dst).is_ok());
    assert!(dst.exists());
    assert!(!src.exists());
}

/// #280 §3.1 — the retry loop exhausts all attempts and returns the
/// diagnostic context. We force consistent failure by pointing `tmp`
/// at a non-existent path (`NotFound` on both Windows and Unix).
#[test]
fn rename_with_retry_returns_diagnostics_after_exhaustion() {
    let tmp = tempfile::tempdir().expect("tmp");
    let src = tmp.path().join("does-not-exist.tmp");
    let dst = tmp.path().join("dst");
    let result = rename_with_retry(&src, &dst);
    let (msg, diag) = result.expect_err("should fail every attempt");
    assert!(!msg.is_empty(), "error message must not be empty");
    assert_eq!(diag.attempts, RENAME_ATTEMPTS);
    assert_eq!(diag.pid, std::process::id());
    assert!(!diag.tmp_exists_before);
    assert!(!diag.final_exists_before);
    // Worst-case duration is bounded by the sum of backoffs (260 ms +
    // syscall noise). Generous upper bound to keep the test stable on
    // slow CI.
    assert!(diag.duration < std::time::Duration::from_secs(2));
}

/// #291 — direct regression for the shared-`sessions.json.tmp` race.
///
/// An `Arc<Barrier>` forces all writer threads to enter the
/// write+rename critical section simultaneously, maximizing the
/// chance that the old buggy code (shared temp filename, no mutex)
/// would interleave such that one caller's rename consumed the temp
/// file before another caller's rename could run, surfacing as
/// Windows `ERROR_FILE_NOT_FOUND` (os error 2). With the fix
/// (mutex + per-call unique temp filenames):
///   1. every concurrent save returns Ok,
///   2. the final file is a valid full snapshot (not torn JSON), and
///   3. no `.tmp` files are left behind.
///
/// Without the barrier this test would have a much weaker race-
/// detection signal: on fast SSDs each thread's write+rename can
/// complete before the next thread is even scheduled, so the
/// pre-fix code might pass by accident.
#[test]
fn save_sessions_concurrent_calls_all_succeed_with_valid_snapshot_and_no_stragglers() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let tmp = tempfile::tempdir().expect("tmp");
    let dir = Arc::new(tmp.path().to_path_buf());

    let writers = 16;
    let barrier = Arc::new(Barrier::new(writers));
    let mut handles = Vec::with_capacity(writers);
    for i in 0..writers {
        let dir = Arc::clone(&dir);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let sessions = vec![PersistedSession {
                last_prompt: None,
                name: format!("sess-{}", i),
                shell: "cmd".into(),
                shell_args: vec![],
                working_directory: format!("C:/x/{}", i),
                ..Default::default()
            }];
            // Synchronize: all threads enter the critical section together,
            // maximizing the chance of catching the historical race.
            barrier.wait();
            super::super::save_sessions_to_dir(&dir, &sessions)
        }));
    }

    // Drain all join handles before asserting so the tempdir is not
    // dropped while threads are still writing into it.
    let mut results = Vec::with_capacity(writers);
    for h in handles {
        results.push(h.join().expect("thread panicked"));
    }

    // (1) Every concurrent save returns Ok.
    for (i, r) in results.iter().enumerate() {
        assert!(r.is_ok(), "writer {} failed: {:?}", i, r.as_ref().err());
    }

    // (2) The final file is a valid full snapshot from exactly one
    //     writer (last-writer-wins under the mutex's serialization).
    let final_path = dir.join("sessions.json");
    assert!(final_path.exists(), "sessions.json must exist after writes");
    let contents = std::fs::read_to_string(&final_path).expect("read final");
    let parsed: Vec<PersistedSession> =
        serde_json::from_str(&contents).expect("final file must be valid JSON, not torn");
    assert_eq!(parsed.len(), 1, "snapshot must contain exactly one session");
    assert!(
        parsed[0].name.starts_with("sess-"),
        "final session name must be one of the writers' inputs, got '{}'",
        parsed[0].name
    );

    // (3) No stranded `.tmp` files. Each save consumed its own unique
    //     temp filename via rename; failure paths would have run the
    //     best-effort `remove_file` cleanup.
    let mut stragglers = Vec::new();
    for entry in std::fs::read_dir(dir.as_path()).expect("readdir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".tmp") {
            stragglers.push(name);
        }
    }
    assert!(
        stragglers.is_empty(),
        "expected no .tmp stragglers after concurrent saves, found {:?}",
        stragglers
    );
}

/// #291 — a single save round-trips: file lands at `sessions.json`,
/// deserializes back to the input, and no `.tmp` file is left behind.
#[test]
fn save_sessions_to_dir_round_trips_and_cleans_up_temp() {
    let tmp = tempfile::tempdir().expect("tmp");
    let sessions = vec![PersistedSession {
        last_prompt: None,
        name: "solo".into(),
        shell: "claude".into(),
        shell_args: vec!["--print".into()],
        working_directory: "C:/proj".into(),
        ..Default::default()
    }];

    super::super::save_sessions_to_dir(tmp.path(), &sessions).expect("save");

    let path = tmp.path().join("sessions.json");
    let contents = std::fs::read_to_string(&path).expect("read");
    let back: Vec<PersistedSession> = serde_json::from_str(&contents).expect("parse");
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].name, "solo");

    for entry in std::fs::read_dir(tmp.path()).expect("readdir") {
        let name = entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert!(
            !name.ends_with(".tmp"),
            "found leftover temp file: {}",
            name
        );
    }
}

#[tokio::test]
async fn aggregate_persistence_snapshot_never_observes_half_of_removal_and_selection() {
    let manager = Arc::new(SessionManager::new());
    let first = manager
        .create_session(
            "shell-a".to_string(),
            Vec::new(),
            "C:/a".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let second = manager
        .create_session(
            "shell-b".to_string(),
            Vec::new(),
            "C:/b".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let first_id = first.id.to_string();
    let second_id = second.id.to_string();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let writer_manager = Arc::clone(&manager);
    let writer_barrier = Arc::clone(&barrier);
    let writer = tokio::spawn(async move {
        writer_barrier.wait().await;
        writer_manager
            .destroy_session(second.id)
            .await
            .expect("atomic fixture destroy");
    });

    barrier.wait().await;
    for _ in 0..128 {
        let snapshot = snapshot_sessions(&manager).await;
        let active = snapshot
            .iter()
            .filter(|session| session.was_active)
            .collect::<Vec<_>>();
        assert_eq!(active.len(), 1, "snapshot must have exactly one active row");
        let ids = snapshot
            .iter()
            .filter_map(|session| session.id.as_deref())
            .collect::<Vec<_>>();
        assert!(ids.contains(&active[0].id.as_deref().unwrap()));
        let old = snapshot.len() == 2 && active[0].id.as_deref() == Some(second_id.as_str());
        let new = snapshot.len() == 1 && active[0].id.as_deref() == Some(first_id.as_str());
        assert!(old || new, "observed half-committed persistence snapshot");
        tokio::task::yield_now().await;
    }
    writer.await.unwrap();
}

#[test]
fn characterize_3254_p03_order() {
    crate::logging::test_install_logger();
    let temp = tempfile::tempdir().unwrap();
    let tag = uuid::Uuid::new_v4().to_string();
    let duplicate = format!("p03-duplicate-{tag}");
    let retained = format!("p03-retained-{tag}");
    let temporary = format!("[temp] p03-{tag}");
    let source = format!(r"\\?\C:\p03\{tag}\retained");
    let loser_source = format!("C:/p03/{tag}/loser-source");
    let loser_prefix = format!("loser-prefix-{tag}");
    let input = serde_json::json!([
        {"name": temporary, "shell": "cmd", "shellArgs": [],
         "workingDirectory": format!("C:/p03/{tag}/temp")},
        {"name": duplicate, "shell": "winner-shell", "shellArgs": ["winner-arg"],
         "workingDirectory": format!("C:/p03/{tag}/winner"), "wasActive": true,
         "codingAgentBindingId": "11111111-1111-4111-8111-111111111111",
         "gitRepos": [{"label": "winner-label", "sourcePath": "C:/winner-source"}]},
        {"name": duplicate, "shell": "loser-shell", "shellArgs": [],
         "workingDirectory": format!("C:/p03/{tag}/loser"),
         "gitBranchSource": loser_source, "gitBranchPrefix": loser_prefix},
        {"name": retained, "shell": "cmd", "shellArgs": [],
         "workingDirectory": source, "gitBranchSource": source,
         "gitBranchPrefix": format!("retained-prefix-{tag}"),
         "codingAgentBindingId": "22222222-2222-4222-8222-222222222222"}
    ]);
    let bytes = serde_json::to_vec_pretty(&input).unwrap();
    let path = temp.path().join(format!("p03-{tag}.json"));
    std::fs::write(&path, &bytes).unwrap();
    let before = crate::logging::test_tee_snapshot().len();
    let rows = super::super::load_sessions_from_path_with_settings(&path, None);
    let logs = crate::logging::test_tee_snapshot();
    let messages: Vec<_> = logs[before..]
        .iter()
        .map(|line| {
            let mut parts = line.splitn(3, ' ');
            let level = parts.next().unwrap();
            let _target = parts.next().unwrap();
            (level, parts.next().unwrap())
        })
        .collect();
    let filtered = format!("[sessions] Filtering out temp session '{temporary}' from persistence");
    let dropped = format!("[sessions] Dropping duplicate session by name '{duplicate}'");
    let upgraded = format!(
        "[sessions] Upgrading legacy single-repo session '{retained}' → \
         git_repos[1]={{label:retained-prefix-{tag}, source:{source}}}"
    );
    assert_eq!(rows.len(), 2);
    assert_p03_log_order(
        &messages,
        [&filtered, &dropped, &upgraded],
        [&loser_source, &loser_prefix],
    );
    assert_p03_normalized(&rows, &duplicate, &retained, &tag, &source);
    assert_p03_origins(&rows, &bytes, &path);
    assert_p03_raw(
        &path,
        &temporary,
        &loser_source,
        &loser_prefix,
        &source,
        &tag,
    );
}

#[test]
fn characterize_3254_p04_snapshot_bytes() {
    let temp = tempfile::tempdir().unwrap();
    crate::config::sessions_persistence::save_sessions_to_dir(temp.path(), &[]).unwrap();
    let path = temp.path().join("sessions.json");
    assert_eq!(std::fs::read(&path).unwrap(), b"[]");
    let rows = [PersistedSession {
        name: "p04-fixed".into(),
        shell: "cmd".into(),
        shell_args: vec!["/Q".into()],
        working_directory: "C:/p04".into(),
        ..Default::default()
    }];
    crate::config::sessions_persistence::save_sessions_to_dir(temp.path(), &rows).unwrap();
    let expected = br#"[
  {
    "name": "p04-fixed",
    "shell": "cmd",
    "shellArgs": [
      "/Q"
    ],
    "workingDirectory": "C:/p04",
    "wasActive": false,
    "gitRepos": [],
    "isCoordinator": false,
    "isRootAgent": false,
    "wasDetached": false,
    "startFreshOnRestore": false
  }
]"#;
    let actual = std::fs::read(&path).unwrap();
    assert_eq!(actual, expected);
    assert!(!actual.ends_with(b"\n"));
}

#[test]
fn characterize_3254_p04_save_errors() {
    let temp = tempfile::tempdir().unwrap();
    let occupied = temp.path().join("occupied");
    std::fs::write(&occupied, b"p04 untouched").unwrap();
    let suffix = std::fs::create_dir_all(&occupied).unwrap_err().to_string();
    let error =
        crate::config::sessions_persistence::save_sessions_to_dir(&occupied, &[]).unwrap_err();
    assert_eq!(
        error,
        format!("Failed to create config directory: {suffix}")
    );
    assert_eq!(std::fs::read(&occupied).unwrap(), b"p04 untouched");
}

fn assert_p03_log_order(messages: &[(&str, &str)], expected: [&str; 3], loser: [&str; 2]) {
    let position = |level, message: &str| {
        messages
            .iter()
            .position(|entry| *entry == (level, message))
            .unwrap()
    };
    assert!(position("WARN", expected[0]) < position("WARN", expected[1]));
    assert!(position("WARN", expected[1]) < position("INFO", expected[2]));
    assert!(messages.contains(&("INFO", "[sessions] Deduplicated: 3 → 2 sessions")));
    assert!(!messages.iter().any(|(_, message)| {
        message.contains("Upgrading legacy")
            && (message.contains(loser[0]) || message.contains(loser[1]))
    }));
}

fn assert_p03_normalized(
    rows: &[PersistedSession],
    duplicate: &str,
    retained: &str,
    tag: &str,
    source: &str,
) {
    assert_eq!(rows[0].name, duplicate);
    assert_eq!(rows[0].shell, "winner-shell");
    assert_eq!(rows[0].shell_args, ["winner-arg"]);
    assert!(rows[0].was_active);
    assert_eq!(rows[0].git_repos[0].label, "winner-label");
    assert_eq!(rows[0].git_repos[0].source_path, "C:/winner-source");
    assert_eq!(rows[1].name, retained);
    let expected_path = if cfg!(windows) {
        format!(r"C:\p03\{tag}\retained")
    } else {
        source.to_owned()
    };
    assert_eq!(rows[1].working_directory, expected_path);
    assert_eq!(rows[1].git_repos[0].source_path, rows[1].working_directory);
    assert_eq!(rows[1].git_repos[0].label, format!("retained-prefix-{tag}"));
}

fn assert_p03_origins(rows: &[PersistedSession], bytes: &[u8], path: &std::path::Path) {
    use sha2::{Digest, Sha256};
    for (row, ordinal) in rows.iter().zip([1, 3]) {
        assert!(row.git_branch_source.is_none());
        assert!(row.git_branch_prefix.is_none());
        let origin = row.coding_agent_binding_origin.as_ref().unwrap();
        assert_eq!(
            origin["rawFileSHA256"],
            format!("{:x}", Sha256::digest(bytes))
        );
        assert_eq!(origin["sessionsPath"], path.to_string_lossy().as_ref());
        assert_eq!(origin["originalOrdinal"], ordinal);
    }
}

fn assert_p03_raw(
    path: &std::path::Path,
    temporary: &str,
    loser_source: &str,
    loser_prefix: &str,
    source: &str,
    tag: &str,
) {
    let raw = super::super::load_sessions_raw_from_path(path);
    assert_eq!(raw.len(), 4);
    assert_eq!(raw[0].name, temporary);
    assert_eq!(raw[2].git_branch_source.as_deref(), Some(loser_source));
    assert_eq!(raw[2].git_branch_prefix.as_deref(), Some(loser_prefix));
    assert_eq!(raw[3].working_directory, source);
    assert_eq!(raw[3].git_branch_source.as_deref(), Some(source));
    assert_eq!(
        raw[3].git_branch_prefix,
        Some(format!("retained-prefix-{tag}"))
    );
    assert!(raw[3].git_repos.is_empty());
}
