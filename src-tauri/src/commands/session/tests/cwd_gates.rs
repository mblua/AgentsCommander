use super::*;

// ──────────────────────────────────────────────────────────────────────
// §1295 gate tests (13-16)
// ──────────────────────────────────────────────────────────────────────

/// Test 13 (dispatch c): the impl creation gate blocks an unregistered cwd
/// (zero residue) and accepts a registered existing root when forced with
/// `Enforce`.
#[tokio::test]
async fn create_session_inner_gate_blocks_unregistered_cwd() {
    use crate::config::sessions_persistence::CreationGateEnforcement;

    // Use the strict-replica fixture so `pty_input_create_gate_key_from_cwd`
    // classifies the cwd as a known replica (returns None, not Err) rather
    // than erroring on a bare tempdir path.

    // Outside-roots cwd (empty project_paths) with Enforce -> rejected, no
    // row, no spawn.
    let (temp, first_cwd, _second) = strict_target_fixture();
    let outside_cwd = first_cwd;
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let err = super::super::create_session_inner_impl(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        outside_cwd,
        Some("gated".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::User,
        None,
        None,
        None,
        None,
        false,
        None,
        CreationGateEnforcement::Enforce,
        None,
        None,
        None,
        None,
    )
    .await;
    let err = match err {
        Ok(_) => panic!("expected the gate to reject the unregistered cwd"),
        Err(e) => e,
    };
    assert!(err.contains("sessionCreateBlocked"), "{err}");
    assert!(err.contains("outside all registered projects"), "{err}");
    assert!(
        session_mgr.read().await.list_sessions().await.is_empty(),
        "rejected create leaves zero residue"
    );
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        0,
        "rejected create leaves zero residue"
    );
    close_test_coordinator(&app).await;

    // Registered existing root with Enforce -> Ok and a row is created.
    let project_root = temp.path().join("project");
    let reg_cwd = temp
        .path()
        .join("project")
        .join(".ac")
        .join("wg-1-team")
        .join("__agent_dev-one")
        .to_string_lossy()
        .to_string();
    std::fs::create_dir_all(&reg_cwd).unwrap();
    let settings = AppSettings {
        project_paths: vec![project_root.to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let session_mgr2 = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend2 = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr2 = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend2.clone(),
    )));
    let app2 = session_test_app(settings, Arc::clone(&session_mgr2), Arc::clone(&pty_mgr2));
    let reg_cwd = reg_cwd.to_string();
    let outcome = super::super::create_session_inner_impl(
        app2.handle(),
        &session_mgr2,
        &pty_mgr2,
        "codex".to_string(),
        Vec::new(),
        reg_cwd,
        Some("gated-ok".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::User,
        None,
        None,
        None,
        None,
        false,
        None,
        CreationGateEnforcement::Enforce,
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(
        outcome.is_ok(),
        "registered existing root must pass the gate"
    );
    assert_eq!(
        session_mgr2.read().await.list_sessions().await.len(),
        1,
        "row created"
    );
    close_test_coordinator(&app2).await;
}

/// Test 14 (B2/N4b): restart of a LIVE outside-roots session DECLINES before
/// teardown when forced with `Enforce`: the row survives with its original
/// status, the PTY handle count is unchanged, and no replacement spawns.
#[tokio::test]
async fn restart_of_live_outside_roots_declines_before_teardown() {
    use crate::config::sessions_persistence::CreationGateEnforcement;

    let temp = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    // Create a LIVE session at an unregistered cwd (gate Skip in the test
    // build lets the create through).
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &temp.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    assert!(backend.has_session(old_id), "old PTY is live");
    let settings = app.state::<crate::config::settings::SettingsState>();

    let err = super::super::restart_session_inner_with_intent(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(true),
        true,
        crate::session::selection::TrustedRestartIntent::User,
        None,
        CreationGateEnforcement::Enforce,
    )
    .await
    .unwrap_err();
    assert!(err.contains("sessionCreateBlocked"), "{err}");

    // No teardown side effects: row survives with its original (non-Exited)
    // status, PTY untouched, no replacement spawn.
    let row = session_mgr
        .read()
        .await
        .get_session(old_id)
        .await
        .expect("row survives");
    assert!(
        !matches!(
            row.status,
            crate::session::session::SessionStatus::Exited(_)
        ),
        "declare must not flip the row to Exited"
    );
    assert!(backend.has_session(old_id), "PTY handle not torn down");
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        1,
        "no replacement spawn"
    );
    close_test_coordinator(&app).await;
}

/// Test 15 (N4d): the restore-wake gate exempts root-agent and archived-root
/// cwds (existence waived) and refuses an outside-roots cwd, all through the
/// same `enforce_creation_gate` the restore path runs.
#[tokio::test]
async fn restore_wake_gate_exceptions() {
    use crate::config::sessions_persistence::{enforce_creation_gate, CreationGateEnforcement};
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));

    // Root-agent cwd: the Root must be verified.
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let root = crate::config::root_agent::root_agent_dir().expect("root dir resolves");
    std::fs::create_dir_all(&root).expect("Root fixture creation succeeds");
    let settings = app.state::<crate::config::settings::SettingsState>();
    assert!(
        enforce_creation_gate(settings.inner(), &root, CreationGateEnforcement::Enforce)
            .await
            .is_ok(),
        "root-agent cwd is gate-exempt"
    );
    close_test_coordinator(&app).await;

    // Archived-root cwd: allowed with existence waived.
    let archived_dir = tempfile::tempdir().unwrap();
    let archived_root = archived_dir.path().to_string_lossy().to_string();
    let missing_under_archived = archived_dir
        .path()
        .join("not-there")
        .join("__agent_x")
        .to_string_lossy()
        .to_string();
    let session_mgr2 = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app2 = session_test_app(
        AppSettings {
            archived_project_paths: vec![archived_root.clone()],
            ..AppSettings::default()
        },
        Arc::clone(&session_mgr2),
        Arc::clone(&pty_mgr),
    );
    let settings2 = app2.state::<crate::config::settings::SettingsState>();
    assert!(
        enforce_creation_gate(
            settings2.inner(),
            &missing_under_archived,
            CreationGateEnforcement::Enforce
        )
        .await
        .is_ok(),
        "archived-root restore-wake is gate-exempt"
    );
    close_test_coordinator(&app2).await;

    // Outside-roots cwd: refused.
    let session_mgr3 = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app3 = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr3),
        Arc::clone(&pty_mgr),
    );
    let outside = archived_dir
        .path()
        .join("elsewhere")
        .to_string_lossy()
        .to_string();
    let settings3 = app3.state::<crate::config::settings::SettingsState>();
    let err = enforce_creation_gate(
        settings3.inner(),
        &outside,
        CreationGateEnforcement::Enforce,
    )
    .await
    .unwrap_err();
    assert!(err.contains("sessionCreateBlocked"), "{err}");
    close_test_coordinator(&app3).await;
}

/// Test 16 (S5/N4): the Tauri `create_session` command with `cwd: None`
/// resolves to the home dir and (in the test build, where the gate defaults
/// to Skip) creates Ok. The production refusal of that default is covered by
/// the pure-function predicate tests in sessions_persistence.
#[tokio::test]
async fn create_session_command_without_cwd_falls_back_to_home() {
    // §1295 S5: the command resolves cwd:None to the home dir (gate Skip in
    // the test build lets the inner create through). The production refusal
    // of the home-dir default is covered by the pure predicate test in
    // sessions_persistence.
    let resolved = super::super::resolve_create_session_cwd(None);
    let expected_home = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if !expected_home.is_empty() {
        assert_eq!(
            resolved,
            crate::path_utils::normalize_windows_verbatim_path(&expected_home),
            "cwd: None falls back to home"
        );
    }

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    // The inner create (gate defaults to Skip in the test build) succeeds at
    // the home cwd, mirroring the `cwd: None` command path.
    super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        resolved,
        Some("home-default".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("create with home default succeeds in the test build");
    assert_eq!(session_mgr.read().await.list_sessions().await.len(), 1);
    close_test_coordinator(&app).await;
}
