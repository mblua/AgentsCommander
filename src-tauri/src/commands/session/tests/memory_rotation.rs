use super::*;

// Source inventory covers the collected production session/create/restart files.
// It pins the fresh flag, gated rotation, comment and reference budgets. Cross-file
// aliases and callers outside this map still require behavioral coverage.
// The production-shaped resume negative and fresh positive controls below remain
// independent of this inventory.
#[test]
fn rotate_origin_memory_at_spawn_has_one_gated_call_in_session_rs() {
    let sources = super::support::session_sources();
    use super::support::{source_budgets, source_fresh, source_rotation};
    assert!(super::support::source_map_complete(&sources));
    assert!(source_rotation(&sources) && source_fresh(&sources) && source_budgets(&sources));
}

/// #1175 B1. The behavioral guard on #1172 D5's normative property, within the
/// scope D5 of this plan declares. This EXECUTES the launch path and looks at
/// the filesystem, so unlike the reference inventory above it reds under the
/// cross-file alias the issue built.
///
/// The resume half is PRODUCTION-SHAPED, not merely nominal. Round 0 passed
/// `pending_start_fresh = None`, `resolved_spawn = None`, `agent_id = None` and
/// `skip_tooling_save = true`, none of which is what `lib.rs:2313-2372` passes
/// on an ordinary startup restore. Grinch showed the cost: a one-line
/// `let start_fresh = skip_auto_resume || pending_start_fresh.is_some();`
/// left every round-0 guard green while a real restore rotated, because
/// production passes `Some(false)` there and round 0 passed `None`. Each
/// argument below is annotated with the production line it mirrors.
///
/// The fresh half is the POSITIVE CONTROL and is not optional: without it, a
/// green resume half would also be produced by a launch that never reached the
/// rotation chokepoint, or by the feature having been deleted outright.
///
/// Observation happens at every boundary, never only at the end. See D3.
#[tokio::test]
async fn a_production_shaped_startup_restore_never_rotates_while_a_fresh_launch_does() {
    let (_fixture, resumed, fresh) = rotation_fixture();
    // `lib.rs:2314` rebuilds the spawn recipe from settings before restoring.
    let settings = test_settings();
    let spawn = super::super::build_configured_agent_spawn_for_cwd(
        &settings,
        "codex",
        &resumed.replica_cwd,
        None,
    )
    .expect("resolve the configured codex spawn")
    .expect("codex is configured in test_settings");
    // `lib.rs:2334-2340` takes shell, args and label from the rebuilt spawn.
    let shell = spawn.shell.clone();
    let shell_args = spawn.shell_args.clone();
    let agent_label = Some(spawn.trusted_agent_label.clone());

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

    let restore = crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
    let restored = super::super::create_session_inner_for_restore(
        &restore,
        &session_mgr,
        &pty_mgr,
        shell,
        shell_args,
        resumed.replica_cwd.clone(),
        Some("startup restore".to_string()),
        Some("codex".to_string()), // lib.rs:2358, ps.agent_id
        agent_label,               // lib.rs:2339, spawn.trusted_agent_label
        false,                     // lib.rs:2360, "Persist tooling on restore"
        Vec::new(),
        false,       // lib.rs:2362, skip_auto_resume_for_restore(false): RESUME
        Some(spawn), // lib.rs:2363, the rebuilt recipe
        None,
        None,        // lib.rs:2365, headless caller keeps 120x30
        Some(false), // lib.rs:2366, Some(ps.start_fresh_on_restore). LOAD-BEARING.
        None,
        None,
        None,
    )
    .await;
    let restored = restored.expect("the production-shaped restore must launch");

    // The launch must be an ACTUAL provider resume, not just a `false` argument.
    // The Codex injection at `:1999` gates on `agent_kind`, and its body gates
    // again on `if let Some(ref aid) = agent_id` at `:2000`, so this only holds
    // because the rebuilt spawn supplies the agent id at `:1446-1450`.
    let effective = restored
        .effective_shell_args
        .clone()
        .unwrap_or_else(|| restored.shell_args.clone());
    assert!(
        effective.iter().any(|arg| arg == "resume"),
        "#1175: the restore must be an ACTUAL Codex provider resume, got {effective:?}"
    );

    // Boundary 1: observe BOTH sides before the second cause runs.
    assert_resume_left_memory_alone(&resumed, "production-shaped startup restore");
    assert_memory_pristine(&fresh, "fresh side, before its own launch");

    let created = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        fresh.replica_cwd.clone(),
        Some("fresh launch".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true, // a fresh create
        None,
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await;
    assert!(created.is_ok(), "the fresh launch must launch: {created:?}");
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        2,
        "both launches must reach the PTY (this proves nothing about the context block)"
    );

    // Boundary 2: the fresh side rotated, and the resume side is STILL untouched.
    assert_memory_rotated_once(&fresh, "fresh create");
    assert_resume_left_memory_alone(&resumed, "restore, rechecked after the fresh launch");
    close_test_coordinator(&app).await;
}

/// #1175 B2. The same function twice with matched arguments and the same
/// session label, one launch fresh and one launch resuming, in the OPPOSITE
/// order from B1.
///
/// It is deliberately NOT called a one-boolean experiment, and it never was
/// one: the cwd differs, and the second launch runs against a `SessionManager`,
/// selection coordinator and backend the first already mutated. Naming that
/// "the flag alone" would grant exactly the false confidence #1175 exists to
/// remove. What it does establish, together with B1, is that no discriminator
/// based solely on first-versus-second launch explains the result: B1 runs
/// resume then fresh, this runs fresh then resume.
///
/// The cwd differs for a positive reason, not because it has to. Two
/// same-target `CreateSelectionIntent::User` creates are supported and are
/// already exercised by `sequential_user_same_target_creates_remain_compatible`
/// (`:6958`); the dedup gate at `:1609` applies to Background and Suppress
/// only. Separate replicas are used so each side has its own independently
/// seeded, non-empty `memory/` to observe.
///
/// Its resume half uses the same `false` polarity as the #599 reopen of a
/// closed coordinator, but it is NOT that scenario: `is_coordinator` is
/// deterministically false for every cwd in this binary, and the Tauri
/// `create_session` producer is bypassed. Plan D5 records coordinator polarity
/// and outer composition as untested.
///
/// The rotating role is assigned to the OPPOSITE replica from B1's, so no
/// single fixture directory is the one that always rotates.
#[tokio::test]
async fn create_session_inner_rotates_the_fresh_target_and_not_the_resumed_one() {
    let (_fixture, fresh, resumed) = rotation_fixture();
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

    let created = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        fresh.replica_cwd.clone(),
        Some("matched differential".to_string()),
        Some("codex".to_string()),
        Some("Codex".to_string()),
        true,
        Vec::new(),
        true, // FRESH first: the opposite order from B1
        None,
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await;
    assert!(created.is_ok(), "the fresh create must launch: {created:?}");

    // Boundary 1.
    assert_memory_rotated_once(&fresh, "fresh create, observed immediately");
    assert_memory_pristine(&resumed, "resume side, before its own launch");

    let reopened = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        resumed.replica_cwd.clone(),
        Some("matched differential".to_string()),
        Some("codex".to_string()),
        Some("Codex".to_string()),
        true,
        Vec::new(),
        false, // the #599 reopen value: this launch RESUMES
        None,
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await;
    assert!(reopened.is_ok(), "the reopen must launch: {reopened:?}");
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        2,
        "both launches must reach the PTY"
    );

    // Boundary 2.
    assert_resume_left_memory_alone(&resumed, "reopen that resumes");
    assert_memory_rotated_once(&fresh, "fresh side, rechecked after the resume");
    close_test_coordinator(&app).await;
}

#[test]
fn create_session_inner_marks_spawning_until_spawn_returns() {
    use super::support::{
        source_archive, source_chain, source_effect, source_map_complete, source_mark,
        source_pending,
    };
    let sources = super::support::session_sources();
    assert!(
        source_map_complete(&sources),
        "complete creation stage inventory"
    );
    assert!(
        source_chain(&sources),
        "reachable context/backend/publication chain"
    );
    assert!(
        source_pending(&sources),
        "pending creation precedes the stage chain"
    );
    assert!(
        source_mark(&sources),
        "spawn precedes mark drop and error handling"
    );
    assert!(
        source_effect(&sources),
        "backend effect completes before mark drop"
    );
    assert!(
        source_archive(&sources),
        "two archive gates bracket mark and pending creation"
    );
    let normalized = sources
        .get("session/create/core.rs")
        .expect("creation core from the same source map")
        .split_whitespace()
        .collect::<String>();
    let first_gate = normalized
        .find(concat!(
            "enforce_owned_unarchived_for_spawn(app,&cwd,&session_label,&create_owner,",
            "&create_manager,lifecycle_domain,inline_transaction,).await?;"
        ))
        .expect("first archive gate");
    let mark = normalized
        .find("letspawn_mark={")
        .expect("spawn mark immediately before pending-session creation");
    assert!(
        first_gate < mark,
        "spawn mark must be created only after pre-create validation"
    );
}

// Runtime witness for plan section 12: drive create_session_inner with a
// fake backend and assert archive_liveness sees the spawn mark WHILE the PTY
// is spawning, then sees it retired once the PTY exists. Unlike the
// source-scrape guards above, this executes the code, so it reds under a
// string-preserving runtime mutation (e.g. mark_spawning inserting nothing,
// or the mark dropped before PtyManager::spawn).
#[tokio::test]
async fn create_session_inner_holds_a_spawn_mark_until_the_pty_exists() {
    use crate::pty::backend::PtyBackend;

    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let expected_cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = Arc::new(GatedSpawnBackend::new(started_tx, release_rx, false));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let task = {
        let app_handle = app_handle.clone();
        let session_mgr = session_mgr.clone();
        let pty_mgr = pty_mgr.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            super::super::create_session_inner(
                &app_handle,
                &session_mgr,
                &pty_mgr,
                "hold-mark-test-command".to_string(),
                Vec::new(),
                cwd,
                Some("hold-mark".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None, // #973 - no view in this test: 120x30
                CreateSelectionIntent::User,
            )
            .await
        })
    };

    // Park inside the backend's spawn. PtyManager::spawn releases the outer
    // manager mutex before awaiting backend.spawn, so the mark is readable
    // here without racing the spawn task.
    started_rx.await.expect("spawn started");
    let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
    assert_eq!(
        pending,
        vec![crate::pty::manager::PendingSpawn {
            cwd: expected_cwd.clone(),
            label: "hold-mark".to_string(),
        }],
        "spawn mark must be live while the PTY is still being spawned"
    );

    let _ = release_tx.send(());
    let info = task
        .await
        .expect("join create_session_inner")
        .expect("create_session_inner should succeed");

    assert!(
        backend.has_session(Uuid::parse_str(&info.id).expect("session id is a uuid")),
        "the PTY must exist once create_session_inner returns Ok"
    );
    let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
    assert!(
        pending.is_empty(),
        "spawn mark must retire once the PTY exists"
    );
}

// Runtime witness for plan section 12: the mark is held across a FAILING
// spawn exactly as across a succeeding one, and is retired on the rollback
// path. Holding the mark across the in-flight spawn is what makes this red
// under a string-preserving mutation (mark_spawning no-op / drop before
// spawn); the empty-afterwards assertion pins retirement on the failure arm.
#[tokio::test]
async fn create_session_inner_retires_the_spawn_mark_on_spawn_failure() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let expected_cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = Arc::new(GatedSpawnBackend::new(started_tx, release_rx, true));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let task = {
        let app_handle = app_handle.clone();
        let session_mgr = session_mgr.clone();
        let pty_mgr = pty_mgr.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            super::super::create_session_inner(
                &app_handle,
                &session_mgr,
                &pty_mgr,
                "retire-mark-test-command".to_string(),
                Vec::new(),
                cwd,
                Some("retire-mark".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None, // #973 - no view in this test: 120x30
                CreateSelectionIntent::User,
            )
            .await
        })
    };

    started_rx.await.expect("spawn started");
    let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
    assert_eq!(
        pending,
        vec![crate::pty::manager::PendingSpawn {
            cwd: expected_cwd.clone(),
            label: "retire-mark".to_string(),
        }],
        "spawn mark must be live while the failing spawn is in flight"
    );

    let _ = release_tx.send(());
    let err = task
        .await
        .expect("join create_session_inner")
        .expect_err("create_session_inner should fail when the spawn fails");
    assert!(err.contains("synthetic spawn failure"), "{err}");

    let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
    assert!(
        pending.is_empty(),
        "spawn mark must retire after a failed spawn"
    );
    assert!(
        session_mgr.read().await.list_sessions().await.is_empty(),
        "rollback must remove the pre-created record on spawn failure"
    );
}

#[test]
fn restart_session_inner_probes_archive_before_destroying_session() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/session.rs"
    ))
    .expect("read session.rs");
    let probe = source
        .find("crate::config::archive_gate::probe_spawn_refusal(app, &cwd).await?;")
        .expect("restart probe call");
    let replacement_spawn = source
        .find("// 3. Release the old runtime slot")
        .expect("restart teardown marker");

    assert!(
        probe < replacement_spawn,
        "restart must probe archive refusal before spawning or destroying a session"
    );
}
