use super::*;

// ── I2237 P1 - `sessionStart` Fresh delivery path ──

/// T0 - test-local PTY backend with real route/backend liveness semantics.
/// Re-declared locally from the shipped `ScriptedSpawnBackend` shape, which
/// is private to `commands/session.rs` and outside this phase's file list.
#[derive(Default)]
struct LoopSpawnBackend {
    live: Mutex<HashSet<Uuid>>,
    spawned: Mutex<Vec<Uuid>>,
    kills: Mutex<Vec<Uuid>>,
}

impl LoopSpawnBackend {
    fn set_live(&self, id: Uuid) {
        self.live.lock().unwrap().insert(id);
    }

    fn spawned(&self) -> Vec<Uuid> {
        self.spawned.lock().unwrap().clone()
    }

    fn kills(&self) -> Vec<Uuid> {
        self.kills.lock().unwrap().clone()
    }
}

impl crate::pty::backend::PtyBackend for LoopSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.live.lock().unwrap().insert(spec.id);
            self.spawned.lock().unwrap().push(spec.id);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        self.live
            .lock()
            .unwrap()
            .contains(&id)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        self.live
            .lock()
            .unwrap()
            .contains(&id)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.kills.lock().unwrap().push(id);
        self.live.lock().unwrap().remove(&id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.has_session(id).then_some((30, 120))
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: std::path::PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

/// T1 - the exact `.ac` shape `resolve_loop_target` reads: one team, one
/// identity-verified coordinator replica under `wg-1-dev-team`, and the Loop
/// config written into the same `.ac` root so step-1 revalidation is real.
fn loop_delivery_fixture() -> (tempfile::TempDir, LoopConfigToml, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    let team = ac_root.join("_team_dev-team");
    let matrix = ac_root.join("_agent_lead");
    let replica = ac_root.join("wg-1-dev-team").join("__agent_lead");
    for dir in [&team, &matrix, &replica] {
        std::fs::create_dir_all(dir).expect("create fixture dir");
    }
    std::fs::write(
        team.join("config.json"),
        r#"{"agents":["../_agent_lead"],"coordinator":"../_agent_lead"}"#,
    )
    .expect("team config");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_lead"}"#,
    )
    .expect("replica config");

    let config = sample_config();
    write_loop_config(&ac_root, &config).expect("write loop config");
    (tmp, config, project, replica)
}

/// T2a - coordinator managed but deliberately NOT started, so the restart's
/// pre-teardown admission gate fails. Serves AC-4, AC-9 and AC-10.
fn make_loop_delivery_app(
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    backend: Arc<LoopSpawnBackend>,
) -> tauri::App {
    let coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(&session_mgr),
        tokio_util::sync::CancellationToken::new(),
    );
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
    crate::test_support::test_builder()
        .manage(Arc::clone(&session_mgr))
        .manage(pty_mgr)
        .manage(crate::config::settings::new_settings_state(
            AppSettings::default(),
        ))
        .manage(coordinator)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build loop delivery app")
}

/// T2b - coordinator started and past the restore barrier, so an in-process
/// restart is admissible. The manage-list mirrors
/// `commands/session.rs`'s `session_test_app_with_store`; the runtime is Wry
/// because `deliver_loop_prompt` takes an `AppHandle`. Serves AC-12/AC-13.
fn make_restartable_loop_app(
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    backend: Arc<LoopSpawnBackend>,
    settings: AppSettings,
) -> tauri::App {
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(&session_mgr),
        shutdown.token().clone(),
    );
    let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let telegram: crate::telegram::manager::TelegramBridgeState =
        Arc::new(tokio::sync::Mutex::new(
            crate::telegram::manager::TelegramBridgeManager::new(output_senders),
        ));
    let store_dir = tempfile::TempDir::new().expect("create target-gate store");
    let message_store = Arc::new(
        crate::api::message_store::MessageStore::open(
            store_dir
                .path()
                .join(crate::api::message_store::DB_FILENAME),
        )
        .expect("open target-gate store"),
    );
    let target_gate_state = crate::api::message_store::PtyInputTargetGateState::for_root(
        store_dir.path().to_path_buf(),
    );
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
    let app = crate::test_support::test_builder()
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(Arc::new(
            crate::resource_monitor::ResourceMonitorState::new(),
        ))
        .manage(Arc::clone(&session_mgr))
        .manage(pty_mgr)
        .manage(crate::DetachedSessionsState::default())
        .manage(telegram)
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(Arc::new(crate::pty::menu_guard::MenuGuard::new()))
        .manage(coordinator.clone())
        .manage(target_gate_state.clone())
        .manage(shutdown)
        .manage(
            crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                Ok(message_store),
                target_gate_state.gate.clone(),
            ),
        )
        .manage(store_dir)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build restartable loop app");
    coordinator
        .start(app.handle().clone())
        .expect("start coordinator");
    let bootstrap = coordinator.clone();
    std::thread::spawn(move || {
        tauri::async_runtime::block_on(async move {
            bootstrap
                .submit_restore_first()
                .await
                .expect("open coordinator")
                .finish();
        });
    })
    .join()
    .expect("join coordinator bootstrap");
    app
}

/// Creates a session directly in the manager, makes it live in the fake
/// backend, and records the route. Never `create_session_inner`: that would
/// push ids into `backend.spawned()` and defeat the empty-`spawned()` pins.
async fn seed_live_manager_session(
    app: &tauri::App,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    backend: &Arc<LoopSpawnBackend>,
    cwd: &str,
) -> Uuid {
    let session = {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "codex".to_string(),
            Vec::new(),
            cwd.to_string(),
            None,
            None,
            Vec::<SessionRepo>::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session")
    };
    backend.set_live(session.id);
    app.state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap()
        .record_route(
            session.id,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        );
    session.id
}

async fn mark_session_idle(session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>, id: Uuid) {
    let mgr = session_mgr.read().await;
    mgr.mark_idle(id).await;
}

async fn wait_for_new_session(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    old_id: Option<Uuid>,
) -> Uuid {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let found = {
            let mgr = session_mgr.read().await;
            mgr.list_sessions()
                .await
                .into_iter()
                .find(|session| old_id.is_none_or(|old| session.id != old.to_string()))
                .and_then(|session| Uuid::parse_str(&session.id).ok())
        };
        if let Some(id) = found {
            return id;
        }
        assert!(Instant::now() < deadline, "expected session never appeared");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// AC-3 - pins the input the two decision tables consume.
#[tokio::test]
async fn final_busy_check_tracks_manager_idle_state() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LoopSpawnBackend::default());
    let app = make_loop_delivery_app(Arc::clone(&session_mgr), Arc::clone(&backend));
    let id = seed_live_manager_session(&app, &session_mgr, &backend, "C:\\work").await;

    let handle = app.handle().clone();
    assert!(!final_busy_check(&handle, id).await.unwrap());
    mark_session_idle(&session_mgr, id).await;
    assert!(final_busy_check(&handle, id).await.unwrap());
}

/// AC-5 - all six cells of epic §4.5 step 2.
#[test]
fn pre_restart_decision_covers_all_busy_policy_cells() {
    use BusyCoordinatorPolicy::{ForceInject, Skip, WaitUntilIdle};
    for idle in [true, false] {
        for policy in [WaitUntilIdle, ForceInject, Skip] {
            let decision = pre_restart_decision(idle, &policy);
            match (idle, &policy) {
                (true, _) => assert!(matches!(decision, PreRestartDecision::Restart)),
                (false, WaitUntilIdle) => {
                    assert!(matches!(decision, PreRestartDecision::Pending))
                }
                (false, ForceInject) => {
                    assert!(matches!(decision, PreRestartDecision::Restart))
                }
                (false, Skip) => assert!(matches!(decision, PreRestartDecision::Skip)),
            }
        }
    }
    // The two cells that forbid destroying a busy session, pinned explicitly.
    assert!(matches!(
        pre_restart_decision(false, &BusyCoordinatorPolicy::WaitUntilIdle),
        PreRestartDecision::Pending
    ));
    assert!(matches!(
        pre_restart_decision(false, &BusyCoordinatorPolicy::Skip),
        PreRestartDecision::Skip
    ));
    assert!(matches!(
        pre_restart_decision(false, &BusyCoordinatorPolicy::ForceInject),
        PreRestartDecision::Restart
    ));
}

/// AC-6 - all six cells of epic §4.5 step 5. The enum has no `Pending`
/// variant at all, which is the anti-starvation pin; the exhaustive match
/// below cannot compile a pending-like outcome into existence.
#[test]
fn post_restart_decision_has_no_pending_cell() {
    use BusyCoordinatorPolicy::{ForceInject, Skip, WaitUntilIdle};
    for idle in [true, false] {
        for policy in [WaitUntilIdle, ForceInject, Skip] {
            let decision = post_restart_decision(idle, &policy);
            match (idle, &policy) {
                (true, _) => assert!(matches!(decision, PostRestartDecision::Inject)),
                (false, WaitUntilIdle) => {
                    assert!(matches!(decision, PostRestartDecision::Failed))
                }
                (false, ForceInject) => {
                    assert!(matches!(decision, PostRestartDecision::Inject))
                }
                (false, Skip) => assert!(matches!(decision, PostRestartDecision::Skip)),
            }
        }
    }
    assert!(matches!(
        post_restart_decision(false, &BusyCoordinatorPolicy::WaitUntilIdle),
        PostRestartDecision::Failed
    ));
}

/// AC-7 - `Fresh` is always `true`, asserted against the `Accumulate` rule
/// on the same command rather than a hardcoded bool.
#[test]
fn fresh_spawn_skips_provider_auto_resume_where_accumulate_does_not() {
    let settings = AppSettings {
        agents: vec![
            loop_test_agent("codex", "codex"),
            loop_test_agent("muse", "muse"),
        ],
        ..AppSettings::default()
    };
    for id in ["codex", "muse"] {
        let command = loop_command_for(&settings, id);
        let expected_accumulate =
            id == "muse" && cfg!(any(target_os = "macos", target_os = "linux"));
        assert_eq!(
            loop_spawn_skip_auto_resume(false, &command),
            expected_accumulate,
            "Accumulate rule, id={id}"
        );
        assert!(
            spawn_skip_auto_resume(LoopSessionStart::Fresh, false, &command),
            "Fresh must skip provider auto-resume, id={id}"
        );
    }
}

/// AC-8 (zero-effect) - `Accumulate` delegates to the shipped rule for the
/// same agent set the pre-existing Muse test uses.
#[test]
fn accumulate_spawn_is_zero_effect_over_the_shipped_rule() {
    let settings = AppSettings {
        agents: vec![
            loop_test_agent("muse", "muse"),
            loop_test_agent("muse-abs", "/opt/muse/bin/muse"),
            loop_test_agent("muse-args", "muse --workspace /srv/work"),
            loop_test_agent("muse-manual", "muse resume --last"),
            loop_test_agent("claude", "claude"),
            loop_test_agent("codex", "codex"),
            loop_test_agent("agy", "agy"),
            loop_test_agent("pi", "pi"),
        ],
        ..AppSettings::default()
    };
    for id in [
        "muse",
        "muse-abs",
        "muse-args",
        "muse-manual",
        "claude",
        "codex",
        "agy",
        "pi",
    ] {
        let command = loop_command_for(&settings, id);
        for had_existing in [true, false] {
            assert_eq!(
                spawn_skip_auto_resume(LoopSessionStart::Accumulate, had_existing, &command),
                loop_spawn_skip_auto_resume(had_existing, &command),
                "id={id} had_existing={had_existing}"
            );
        }
    }

    let mut adhoc = loop_command_for(&settings, "muse");
    adhoc.resolved_spawn = None;
    assert_eq!(
        spawn_skip_auto_resume(LoopSessionStart::Accumulate, false, &adhoc),
        loop_spawn_skip_auto_resume(false, &adhoc)
    );
    let mut mismatch = loop_command_for(&settings, "muse");
    mismatch.shell_args = vec!["--workspace".to_string(), "/srv/work".to_string()];
    assert_eq!(
        spawn_skip_auto_resume(LoopSessionStart::Accumulate, false, &mismatch),
        loop_spawn_skip_auto_resume(false, &mismatch)
    );
}

/// AC-4 (anti-fall-through) - a Fresh restart that fails before teardown
/// reports `DeliveryFailed` and never injects into the surviving session.
#[tokio::test]
async fn fresh_live_restart_failure_before_teardown_never_falls_through() {
    let (_tmp, config, project, replica) = loop_delivery_fixture();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LoopSpawnBackend::default());
    let app = make_loop_delivery_app(Arc::clone(&session_mgr), Arc::clone(&backend));

    let old_id =
        seed_live_manager_session(&app, &session_mgr, &backend, &replica.to_string_lossy()).await;
    mark_session_idle(&session_mgr, old_id).await;

    let handle = app.handle().clone();
    let report = deliver_loop_prompt(&handle, &project, &config, Uuid::new_v4(), Utc::now()).await;

    assert_eq!(report.kind, LoopAuditKind::DeliveryFailed);
    assert!(report.prompt_snapshot.is_none());
    assert!(report.error.is_some());
    assert!(session_mgr.read().await.get_session(old_id).await.is_some());
    assert!(backend.has_session(old_id));
    assert!(backend.kills().is_empty());
}

/// AC-9 (no destruction against policy) - a busy live session under
/// `WaitUntilIdle` or `Skip` is never restarted, and nothing is spawned or
/// killed. The empty `kills()`/`spawned()` pins catch a restart moved above
/// the busy gate.
#[tokio::test]
async fn fresh_live_busy_session_is_never_destroyed_against_its_policy() {
    for (policy, expected_kind) in [
        (
            BusyCoordinatorPolicy::WaitUntilIdle,
            LoopAuditKind::PendingBusy,
        ),
        (BusyCoordinatorPolicy::Skip, LoopAuditKind::SkippedBusy),
    ] {
        let (tmp, mut config, project, replica) = loop_delivery_fixture();
        config.policy.busy_coordinator = policy.clone();
        write_loop_config(&tmp.path().join("project").join(".ac"), &config)
            .expect("rewrite loop config");

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LoopSpawnBackend::default());
        let app = make_loop_delivery_app(Arc::clone(&session_mgr), Arc::clone(&backend));

        let old_id =
            seed_live_manager_session(&app, &session_mgr, &backend, &replica.to_string_lossy())
                .await;
        // Deliberately NOT `mark_idle`: `waiting_for_input == false` is busy.

        let handle = app.handle().clone();
        let report =
            deliver_loop_prompt(&handle, &project, &config, Uuid::new_v4(), Utc::now()).await;

        assert_eq!(report.kind, expected_kind, "policy={policy:?}");
        assert_eq!(report.session_id, Some(old_id));
        assert!(session_mgr.read().await.get_session(old_id).await.is_some());
        assert!(backend.has_session(old_id));
        assert!(backend.kills().is_empty(), "policy={policy:?}");
        assert!(backend.spawned().is_empty(), "policy={policy:?}");
    }
}

/// AC-10 (zero-effect) - `Accumulate` keeps the shipped pending report and
/// the session survives.
#[tokio::test]
async fn accumulate_live_busy_session_stays_pending_and_survives() {
    let (tmp, mut config, project, replica) = loop_delivery_fixture();
    config.policy.session_start = LoopSessionStart::Accumulate;
    write_loop_config(&tmp.path().join("project").join(".ac"), &config)
        .expect("rewrite loop config");

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LoopSpawnBackend::default());
    let app = make_loop_delivery_app(Arc::clone(&session_mgr), Arc::clone(&backend));

    let old_id =
        seed_live_manager_session(&app, &session_mgr, &backend, &replica.to_string_lossy()).await;

    let handle = app.handle().clone();
    let report = deliver_loop_prompt(&handle, &project, &config, Uuid::new_v4(), Utc::now()).await;

    assert_eq!(report.kind, LoopAuditKind::PendingBusy);
    assert_eq!(report.session_id, Some(old_id));
    assert_eq!(report.completed_at, None);
    assert!(session_mgr.read().await.get_session(old_id).await.is_some());
    assert!(backend.has_session(old_id));
    assert!(backend.kills().is_empty());
    assert!(backend.spawned().is_empty());
}

/// AC-12 (mandatory) - a successful Fresh restart delivers into the new
/// session. `fresh_at_spawn` is captured BEFORE the inject can clear it;
/// the post-delivery clear is asserted separately (#756).
#[tokio::test]
async fn fresh_live_restart_delivers_with_a_fresh_conversation() {
    let (_tmp, config, project, replica) = loop_delivery_fixture();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LoopSpawnBackend::default());
    let app = make_restartable_loop_app(
        Arc::clone(&session_mgr),
        Arc::clone(&backend),
        AppSettings::default(),
    );

    let old_id =
        seed_live_manager_session(&app, &session_mgr, &backend, &replica.to_string_lossy()).await;
    let old_stamp_before = {
        let mgr = session_mgr.read().await;
        mgr.get_session(old_id)
            .await
            .expect("old row")
            .start_fresh_on_restore
    };
    assert!(!old_stamp_before, "fixture starts resume-capable");
    mark_session_idle(&session_mgr, old_id).await;

    let handle = app.handle().clone();
    let spawned_config = config.clone();
    let spawned_project = project.clone();
    let mut delivery = tokio::spawn(async move {
        deliver_loop_prompt(
            &handle,
            &spawned_project,
            &spawned_config,
            Uuid::new_v4(),
            Utc::now(),
        )
        .await
    });

    let new_id = wait_for_new_session(&session_mgr, Some(old_id)).await;
    let fresh_at_spawn = {
        let mgr = session_mgr.read().await;
        mgr.get_session(new_id)
            .await
            .expect("new row")
            .start_fresh_on_restore
    };
    assert!(
        fresh_at_spawn,
        "the restart must stamp the fresh intent at spawn; post-inject reads cannot prove this"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut delivery)
            .await
            .is_err(),
        "delivery must still be waiting for the restarted session to go idle"
    );

    mark_session_idle(&session_mgr, new_id).await;
    let report = delivery.await.expect("join delivery");

    assert_eq!(report.kind, LoopAuditKind::Delivered);
    assert_eq!(
        report.prompt_snapshot.as_deref(),
        Some(config.prompt.body.as_str())
    );
    assert_eq!(report.session_id, Some(new_id));
    assert_ne!(new_id, old_id);
    assert!(backend.kills().contains(&old_id));
    assert!(!backend.has_session(old_id));
    assert!(session_mgr.read().await.get_session(old_id).await.is_none());
    assert!(backend.spawned().contains(&new_id));
    assert!(backend.has_session(new_id));

    let post_delivery_stamp = {
        let mgr = session_mgr.read().await;
        mgr.get_session(new_id)
            .await
            .expect("post-delivery row")
            .start_fresh_on_restore
    };
    assert!(
        !post_delivery_stamp,
        "a successful inject clears the fresh intent (#756)"
    );

    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

/// #2733 T-B1 - an unresolved target is its own kind, never a session.
#[tokio::test]
async fn issue_2733_missing_room_reports_target_unresolved() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join(".ac")).expect("ac root");
    let mut config = sample_config();
    config.target.workgroup = "room-99-missing".to_string();
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");

    let report = deliver_loop_prompt(
        app.handle(),
        tmp.path(),
        &config,
        Uuid::new_v4(),
        Utc::now(),
    )
    .await;

    assert_eq!(report.kind, LoopAuditKind::TargetUnresolved);
    assert!(
        report
            .error
            .as_deref()
            .is_some_and(|e| e.contains("not found in project")),
        "{:?}",
        report.error
    );
    assert_eq!(report.session_id, None);
    assert_eq!(report.target, None);
}

/// #2733 T-B9 - real delivery: unresolved while the room is away, then a
/// real spawn + inject once the room appears again.
#[tokio::test]
async fn issue_2733_real_delivery_resumes_after_room_appears() {
    let (_tmp, config, project, replica) = loop_delivery_fixture();
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_lead","tooling":{"lastCodingAgent":"pi"}}"#,
    )
    .expect("replica tooling config");
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LoopSpawnBackend::default());
    let settings = AppSettings {
        agents: vec![loop_test_agent("pi", "pi --model x")],
        ..AppSettings::default()
    };
    let app = make_restartable_loop_app(Arc::clone(&session_mgr), Arc::clone(&backend), settings);

    let room = project.join(".ac").join("wg-1-dev-team");
    let away = project.join(".ac").join("wg-1-dev-team.away");
    std::fs::rename(&room, &away).expect("move room away");
    let report =
        deliver_loop_prompt(app.handle(), &project, &config, Uuid::new_v4(), Utc::now()).await;
    assert_eq!(report.kind, LoopAuditKind::TargetUnresolved);
    assert_eq!(report.session_id, None);
    assert!(backend.spawned().is_empty());
    assert!(session_mgr.read().await.list_sessions().await.is_empty());

    std::fs::rename(&away, &room).expect("room appears");
    let handle = app.handle().clone();
    let spawned_config = config.clone();
    let spawned_project = project.clone();
    let delivery = tokio::spawn(async move {
        deliver_loop_prompt(
            &handle,
            &spawned_project,
            &spawned_config,
            Uuid::new_v4(),
            Utc::now(),
        )
        .await
    });
    let new_id = wait_for_new_session(&session_mgr, None).await;
    mark_session_idle(&session_mgr, new_id).await;
    let report = delivery.await.expect("join delivery");

    assert_eq!(report.kind, LoopAuditKind::Delivered);
    assert_eq!(report.session_id, Some(new_id));
    assert_eq!(
        report.prompt_snapshot.as_deref(),
        Some(config.prompt.body.as_str())
    );
    assert!(backend.spawned().contains(&new_id));

    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

/// AC-13 - the cold-spawn call site passes the Loop's choice into provider
/// resume wiring, observed through `effective_shell_args`.
#[tokio::test]
async fn cold_spawn_wires_session_start_into_provider_resume() {
    for (session_start, expect_continue) in [
        (LoopSessionStart::Fresh, false),
        (LoopSessionStart::Accumulate, true),
    ] {
        let (tmp, mut config, project, replica) = loop_delivery_fixture();
        config.policy.session_start = session_start;
        write_loop_config(&tmp.path().join("project").join(".ac"), &config)
            .expect("rewrite loop config");
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_lead","tooling":{"lastCodingAgent":"pi"}}"#,
        )
        .expect("replica tooling config");

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LoopSpawnBackend::default());
        let settings = AppSettings {
            agents: vec![loop_test_agent("pi", "pi --model x")],
            ..AppSettings::default()
        };
        let app =
            make_restartable_loop_app(Arc::clone(&session_mgr), Arc::clone(&backend), settings);

        let handle = app.handle().clone();
        let spawned_config = config.clone();
        let spawned_project = project.clone();
        let delivery = tokio::spawn(async move {
            deliver_loop_prompt(
                &handle,
                &spawned_project,
                &spawned_config,
                Uuid::new_v4(),
                Utc::now(),
            )
            .await
        });

        let new_id = wait_for_new_session(&session_mgr, None).await;
        let effective_args = {
            let mgr = session_mgr.read().await;
            mgr.get_session(new_id)
                .await
                .expect("spawned row")
                .effective_shell_args
                .expect("captured effective args")
        };
        assert_eq!(
            effective_args.contains(&"--continue".to_string()),
            expect_continue,
            "session_start={session_start:?} args={effective_args:?}"
        );

        mark_session_idle(&session_mgr, new_id).await;
        let report = delivery.await.expect("join delivery");
        assert_eq!(report.kind, LoopAuditKind::Delivered);

        app.state::<crate::session::selection::SelectionCoordinator>()
            .close_and_join()
            .await;
    }
}

/// AC-18 - the stale pre-check runs BEFORE any teardown. The fixture is
/// `make_restartable_loop_app`, the one app on which a restart would
/// otherwise succeed, so empty `kills()` and `spawned()` prove the correct
/// path never reached the restart rather than that the restart failed.
#[tokio::test]
async fn stale_loop_is_reported_before_any_teardown() {
    let (_tmp, config, project, replica) = loop_delivery_fixture();
    let ac_root = project.join(".ac");
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(LoopSpawnBackend::default());
    let app = make_restartable_loop_app(
        Arc::clone(&session_mgr),
        Arc::clone(&backend),
        AppSettings::default(),
    );

    let old_id =
        seed_live_manager_session(&app, &session_mgr, &backend, &replica.to_string_lossy()).await;
    mark_session_idle(&session_mgr, old_id).await;

    // Make the on-disk Loop stale while the in-memory config stays current:
    // `revalidate_loop_current` then reports `Disabled`.
    let mut disabled = config.clone();
    disabled.loop_def.enabled = false;
    write_loop_config(&ac_root, &disabled).expect("write disabled loop config");

    let report =
        deliver_loop_prompt(app.handle(), &project, &config, Uuid::new_v4(), Utc::now()).await;

    assert_eq!(report.kind, LoopAuditKind::DeliveryFailed);
    assert_eq!(report.session_id, Some(old_id));
    assert!(report.prompt_snapshot.is_none());
    assert!(session_mgr.read().await.get_session(old_id).await.is_some());
    assert!(backend.has_session(old_id));
    assert!(
        backend.kills().is_empty(),
        "the stale check must return before the restart tears the old session down"
    );
    assert!(
        backend.spawned().is_empty(),
        "no replacement session may be spawned for a stale Loop"
    );

    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}
